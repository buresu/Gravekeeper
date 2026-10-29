//! Linux. Per-user settings belong to the desktop environment, so they are read and written
//! the way its own settings app does: KConfig files through `kreadconfig6` / `kwriteconfig6`
//! on KDE Plasma 6, GSettings through `gsettings` on GNOME. Other desktops report the setting
//! as unsupported.
//!
//! Values replaced when a setting is turned off are kept in a per-user backup file (see
//! [`Backups`]) and put back when it is turned on again; with no backup the desktop's
//! default is restored.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "not supported on this platform yet",
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Desktop {
    Kde,
    Gnome,
}

fn desktop() -> io::Result<Desktop> {
    let current = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    parse_desktop(&current).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "needs KDE Plasma or GNOME on Linux",
        )
    })
}

/// `XDG_CURRENT_DESKTOP` is a colon-separated list, e.g. `ubuntu:GNOME`.
fn parse_desktop(current: &str) -> Option<Desktop> {
    current.split(':').find_map(|name| match name {
        "KDE" => Some(Desktop::Kde),
        "GNOME" => Some(Desktop::Gnome),
        _ => None,
    })
}

/// Runs a tool and returns its trimmed stdout.
fn run(program: &str, args: &[&str]) -> io::Result<String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| match err.kind() {
            io::ErrorKind::NotFound => {
                io::Error::new(io::ErrorKind::NotFound, format!("{program} was not found"))
            }
            _ => io::Error::other(format!("cannot run {program}: {err}")),
        })?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "{program} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Whether this computer runs on a battery. Peripherals (mice, headsets) also appear under
/// `power_supply`, with `scope` set to `Device`, and do not count.
fn has_battery() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    let read = |path: PathBuf| std::fs::read_to_string(path).unwrap_or_default();
    entries.flatten().any(|entry| {
        let dir = entry.path();
        read(dir.join("type")).trim() == "Battery" && read(dir.join("scope")).trim() != "Device"
    })
}

// --- Backups ---------------------------------------------------------------------------------

/// Values replaced by turning a setting off, keyed by where they came from, in
/// `os-settings-backup.json` in Gravekeeper's data directory. Only per-user values are kept
/// here: the file belongs to the user running Gravekeeper.
struct Backups {
    path: PathBuf,
    values: BTreeMap<String, String>,
}

impl Backups {
    fn load() -> io::Result<Self> {
        let paths = gravekeeper_core::paths::Paths::from_env()?;
        Self::load_from(paths.data_dir.join("os-settings-backup.json"))
    }

    fn load_from(path: PathBuf) -> io::Result<Self> {
        let values = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {err}", path.display()),
                )
            })?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(err) => return Err(err),
        };
        Ok(Self { path, values })
    }

    fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(&self.values).expect("strings serialize");
        // Written aside and renamed, so a crash never leaves a half-written file.
        let temp = self.path.with_extension("json.tmp");
        std::fs::write(&temp, json)?;
        std::fs::rename(&temp, &self.path)
    }
}

// --- Per-user values -------------------------------------------------------------------------

/// Where a desktop keeps one value.
enum Store {
    /// A KConfig entry: file (under `~/.config`), group path and key.
    Kde {
        file: &'static str,
        groups: [&'static str; 2],
        key: &'static str,
    },
    /// A GSettings key. Values are in GVariant text form, as `gsettings get` prints them.
    Gnome {
        schema: &'static str,
        key: &'static str,
    },
}

impl Store {
    /// The backup key.
    fn id(&self) -> String {
        match self {
            Store::Kde { file, groups, key } => format!("kde:{file}/{}/{key}", groups.join("/")),
            Store::Gnome { schema, key } => format!("gsettings:{schema}/{key}"),
        }
    }

    /// `None` when the value is not set, so the desktop's default applies (KDE only;
    /// `gsettings` always reports the effective value).
    fn read(&self) -> io::Result<Option<String>> {
        match self {
            Store::Kde { file, groups, key } => {
                let value = run(
                    "kreadconfig6",
                    &[
                        "--file", file, "--group", groups[0], "--group", groups[1], "--key", key,
                    ],
                )?;
                Ok((!value.is_empty()).then_some(value))
            }
            Store::Gnome { schema, key } => run("gsettings", &["get", schema, key]).map(Some),
        }
    }

    /// Sets the value, or with `None` returns it to the desktop's default.
    fn write(&self, value: Option<&str>) -> io::Result<()> {
        match self {
            Store::Kde { file, groups, key } => {
                let mut args = vec![
                    "--file", file, "--group", groups[0], "--group", groups[1], "--key", key,
                ];
                match value {
                    Some(value) => args.push(value),
                    None => args.push("--delete"),
                }
                run("kwriteconfig6", &args).map(drop)
            }
            Store::Gnome { schema, key } => match value {
                Some(value) => run("gsettings", &["set", schema, key, value]).map(drop),
                None => run("gsettings", &["reset", schema, key]).map(drop),
            },
        }
    }
}

/// One value of a setting that can be switched off.
struct Entry {
    store: Store,
    /// The value that switches it off.
    off: &'static str,
    /// Applies only on battery power, so it is ignored when judging the state of a computer
    /// without one.
    on_battery: bool,
}

/// On if any entry that applies to this computer is not off. An unset KDE value means the
/// default, which is on for every setting handled here.
fn entries_on(entries: &[Entry]) -> io::Result<bool> {
    let battery = has_battery();
    for entry in entries.iter().filter(|e| battery || !e.on_battery) {
        if entry.store.read()?.as_deref() != Some(entry.off) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn set_entries(entries: &[Entry], enabled: bool) -> io::Result<()> {
    let mut backups = Backups::load()?;
    if enabled {
        for entry in entries {
            let saved = backups.values.remove(&entry.store.id());
            // Only an entry still off is restored; one changed since in the desktop's own
            // settings is left as it is.
            if entry.store.read()?.as_deref() == Some(entry.off) {
                entry.store.write(saved.as_deref())?;
            }
        }
        backups.save()?;
    } else {
        let mut current = Vec::with_capacity(entries.len());
        for entry in entries {
            let value = entry.store.read()?;
            if let Some(value) = &value
                && value != entry.off
            {
                backups.values.insert(entry.store.id(), value.clone());
            }
            current.push(value);
        }
        // Saved before anything changes, so a failure never loses the values to restore.
        backups.save()?;
        for (entry, value) in entries.iter().zip(current) {
            if value.as_deref() != Some(entry.off) {
                entry.store.write(Some(entry.off))?;
            }
        }
    }
    Ok(())
}

// --- Power (sleep, display off) --------------------------------------------------------------
//
// KDE: PowerDevil's per-profile `AutoSuspendAction` (0 = do nothing) and
// `TurnOffDisplayWhenIdle` in `powerdevilrc`, for the AC, Battery and LowBattery profiles. The
// timeouts are left alone. PowerDevil does not watch the file, so it is told to re-read it.
//
// GNOME: `sleep-inactive-{ac,battery}-type` (`'nothing'` = never) of gnome-settings-daemon,
// and `org.gnome.desktop.session idle-delay` (0 = never), after which the screen blanks.
// GSettings changes apply at once.

const POWERDEVIL_RC: &str = "powerdevilrc";
const KDE_PROFILES: [&str; 3] = ["AC", "Battery", "LowBattery"];
const GNOME_POWER: &str = "org.gnome.settings-daemon.plugins.power";

fn kde_profiles(group: &'static str, key: &'static str, off: &'static str) -> Vec<Entry> {
    KDE_PROFILES
        .into_iter()
        .map(|profile| Entry {
            store: Store::Kde {
                file: POWERDEVIL_RC,
                groups: [profile, group],
                key,
            },
            off,
            on_battery: profile != "AC",
        })
        .collect()
}

fn sleep_entries(desktop: Desktop) -> Vec<Entry> {
    match desktop {
        Desktop::Kde => kde_profiles("SuspendAndShutdown", "AutoSuspendAction", "0"),
        Desktop::Gnome => [
            ("sleep-inactive-ac-type", false),
            ("sleep-inactive-battery-type", true),
        ]
        .into_iter()
        .map(|(key, on_battery)| Entry {
            store: Store::Gnome {
                schema: GNOME_POWER,
                key,
            },
            off: "'nothing'",
            on_battery,
        })
        .collect(),
    }
}

fn display_entries(desktop: Desktop) -> Vec<Entry> {
    match desktop {
        Desktop::Kde => kde_profiles("Display", "TurnOffDisplayWhenIdle", "false"),
        Desktop::Gnome => vec![Entry {
            store: Store::Gnome {
                schema: "org.gnome.desktop.session",
                key: "idle-delay",
            },
            off: "uint32 0",
            on_battery: false,
        }],
    }
}

/// Makes PowerDevil apply the edited `powerdevilrc`, as its settings page does.
fn reload_powerdevil() -> zbus::Result<()> {
    let connection = zbus::blocking::Connection::session()?;
    for method in ["reparseConfiguration", "refreshStatus"] {
        connection.call_method(
            Some("org.kde.Solid.PowerManagement"),
            "/org/kde/Solid/PowerManagement",
            Some("org.kde.Solid.PowerManagement"),
            method,
            &(),
        )?;
    }
    Ok(())
}

fn set_power(desktop: Desktop, entries: &[Entry], enabled: bool) -> io::Result<()> {
    set_entries(entries, enabled)?;
    if desktop == Desktop::Kde {
        reload_powerdevil().map_err(|err| {
            io::Error::other(format!(
                "saved, but PowerDevil did not reload its settings: {err}"
            ))
        })?;
    }
    Ok(())
}

pub fn automatic_sleep() -> io::Result<bool> {
    entries_on(&sleep_entries(desktop()?))
}

pub fn set_automatic_sleep(enabled: bool) -> io::Result<()> {
    let desktop = desktop()?;
    set_power(desktop, &sleep_entries(desktop), enabled)
}

pub fn display_off() -> io::Result<bool> {
    entries_on(&display_entries(desktop()?))
}

pub fn set_display_off(enabled: bool) -> io::Result<()> {
    let desktop = desktop()?;
    set_power(desktop, &display_entries(desktop), enabled)
}

// --- Not implemented yet ---------------------------------------------------------------------

pub fn automatic_updates() -> io::Result<bool> {
    Err(unsupported())
}

pub fn set_automatic_updates(_enabled: bool) -> io::Result<()> {
    Err(unsupported())
}

pub fn auto_login() -> io::Result<Option<String>> {
    Ok(None)
}

pub fn set_auto_login(_enabled: bool) -> io::Result<()> {
    Err(unsupported())
}

pub fn hibernate_enabled() -> io::Result<bool> {
    Err(unsupported())
}

pub fn set_hibernate(_enabled: bool) -> io::Result<()> {
    Err(unsupported())
}

pub fn notifications_enabled() -> io::Result<bool> {
    Err(unsupported())
}

pub fn set_notifications(_enabled: bool) -> io::Result<()> {
    Err(unsupported())
}

pub fn security_notifications_enabled() -> io::Result<bool> {
    Err(unsupported())
}

pub fn set_security_notifications(_enabled: bool) -> io::Result<()> {
    Err(unsupported())
}

pub fn set_security_notifications_user(_enabled: bool) -> io::Result<()> {
    Err(unsupported())
}

pub fn firewall_enabled() -> io::Result<bool> {
    Err(unsupported())
}

pub fn set_firewall(_enabled: bool) -> io::Result<()> {
    Err(unsupported())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktops_are_found_in_the_list() {
        assert_eq!(parse_desktop("KDE"), Some(Desktop::Kde));
        assert_eq!(parse_desktop("ubuntu:GNOME"), Some(Desktop::Gnome));
        assert_eq!(parse_desktop("X-Cinnamon"), None);
        assert_eq!(parse_desktop(""), None);
    }

    #[test]
    fn backups_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/os-settings-backup.json");
        let mut backups = Backups::load_from(path.clone()).unwrap();
        assert!(backups.values.is_empty());
        backups
            .values
            .insert("gsettings:a/b".into(), "uint32 300".into());
        backups.save().unwrap();
        let loaded = Backups::load_from(path).unwrap();
        assert_eq!(loaded.values.get("gsettings:a/b").unwrap(), "uint32 300");
    }

    #[test]
    fn store_ids_name_their_source() {
        let kde = Store::Kde {
            file: POWERDEVIL_RC,
            groups: ["AC", "Display"],
            key: "TurnOffDisplayWhenIdle",
        };
        assert_eq!(
            kde.id(),
            "kde:powerdevilrc/AC/Display/TurnOffDisplayWhenIdle"
        );
    }
}
