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
use std::path::{Path, PathBuf};
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
        groups: Vec<&'static str>,
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

    /// The `kreadconfig6` / `kwriteconfig6` arguments naming a KConfig entry.
    fn kde_args<'a>(file: &'a str, groups: &[&'a str], key: &'a str) -> Vec<&'a str> {
        let mut args = vec!["--file", file];
        for group in groups {
            args.extend(["--group", group]);
        }
        args.extend(["--key", key]);
        args
    }

    /// `None` when the value is not set, so the desktop's default applies (KDE only;
    /// `gsettings` always reports the effective value).
    fn read(&self) -> io::Result<Option<String>> {
        match self {
            Store::Kde { file, groups, key } => {
                let value = run("kreadconfig6", &Self::kde_args(file, groups, key))?;
                Ok((!value.is_empty()).then_some(value))
            }
            Store::Gnome { schema, key } => run("gsettings", &["get", schema, key]).map(Some),
        }
    }

    /// Sets the value, or with `None` returns it to the desktop's default.
    fn write(&self, value: Option<&str>) -> io::Result<()> {
        match self {
            Store::Kde { file, groups, key } => {
                // `--notify` lets programs watching the file (KConfigWatcher) apply the change
                // at once, as when the desktop's settings app saves it.
                let mut args = Self::kde_args(file, groups, key);
                args.push("--notify");
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
                groups: vec![profile, group],
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

// --- Notifications ---------------------------------------------------------------------------
//
// Both desktops turn notifications off with their "Do not disturb" mode, which keeps them in
// the history but shows no pop-ups and plays no sounds.
//
// KDE: Plasma has no plain switch; Do not disturb lasts until `[DoNotDisturb] Until` in
// `plasmanotifyrc` (its "until manually disabled" is a year ahead). Turning notifications off
// sets a date far enough ahead never to expire; turning them on removes it. Plasma watches the
// file, so either applies at once.
//
// GNOME: `org.gnome.desktop.notifications show-banners`, the Do not disturb switch itself.

/// Effectively forever, in the comma-separated form KConfig writes dates in.
const DND_FOREVER: &str = "9999,12,31,0,0,0";

fn dnd_until() -> Store {
    Store::Kde {
        file: "plasmanotifyrc",
        groups: vec!["DoNotDisturb"],
        key: "Until",
    }
}

fn gnome_banners() -> Entry {
    Entry {
        store: Store::Gnome {
            schema: "org.gnome.desktop.notifications",
            key: "show-banners",
        },
        off: "false",
        on_battery: false,
    }
}

/// A KConfig date-time: `2026,8,25,9,2,57.196` (local time, as KConfig writes it), or ISO 8601.
fn parse_kconfig_datetime(value: &str) -> Option<jiff::civil::DateTime> {
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() != 6 {
        return value.parse().ok();
    }
    let number = |i: usize| parts[i].trim().parse::<i16>().ok();
    let seconds: f64 = parts[5].trim().parse().ok()?;
    jiff::civil::DateTime::new(
        number(0)?,
        number(1)?.try_into().ok()?,
        number(2)?.try_into().ok()?,
        number(3)?.try_into().ok()?,
        number(4)?.try_into().ok()?,
        seconds.trunc() as i8,
        0,
    )
    .ok()
}

pub fn notifications_enabled() -> io::Result<bool> {
    match desktop()? {
        Desktop::Kde => {
            let until = dnd_until().read()?;
            // An unreadable date counts as no Do not disturb, like a past one.
            let now = jiff::Zoned::now().datetime();
            Ok(!until
                .as_deref()
                .and_then(parse_kconfig_datetime)
                .is_some_and(|until| until > now))
        }
        Desktop::Gnome => entries_on(&[gnome_banners()]),
    }
}

pub fn set_notifications(enabled: bool) -> io::Result<()> {
    match desktop()? {
        Desktop::Kde => dnd_until().write((!enabled).then_some(DND_FOREVER)),
        Desktop::Gnome => set_entries(&[gnome_banners()], enabled),
    }
}

// --- Hibernation -----------------------------------------------------------------------------
//
// System-wide, so it is changed by the elevated helper: a drop-in in `/etc/systemd/sleep.conf.d`
// sets `AllowHibernation=no`, along with the two modes that hibernate as part of them, and
// logind then reports hibernation as unavailable to the desktop. Turning it on removes the
// drop-in. The state is the effective `AllowHibernation`, read without elevation from the
// merged configuration that `systemd-analyze cat-config` prints.
//
// Unlike Windows nothing is freed: the swap space used to hibernate is left as it is.

const SLEEP_DROP_INS: &str = "/etc/systemd/sleep.conf.d";
const HIBERNATE_DROP_IN: &str = "90-gravekeeper-hibernation.conf";

fn hibernate_drop_in(allow: bool) -> String {
    let value = if allow { "yes" } else { "no" };
    format!(
        "# Written by Gravekeeper (System > Hibernation). Delete this file to undo.\n\
         [Sleep]\n\
         AllowHibernation={value}\n\
         AllowSuspendThenHibernate={value}\n\
         AllowHybridSleep={value}\n"
    )
}

/// systemd's boolean spellings; `None` for anything else, which systemd ignores.
fn parse_systemd_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "yes" | "y" | "true" | "t" | "on" => Some(true),
        "0" | "no" | "n" | "false" | "f" | "off" => Some(false),
        _ => None,
    }
}

/// The effective `AllowHibernation` of a merged `sleep.conf`: the last valid assignment in a
/// `[Sleep]` section wins, and hibernation is allowed by default.
fn allows_hibernation(config: &str) -> bool {
    let mut section = "";
    let mut allowed = true;
    for line in config.lines().map(str::trim) {
        if line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name;
        } else if section == "Sleep"
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "AllowHibernation"
            && let Some(value) = parse_systemd_bool(value.trim())
        {
            allowed = value;
        }
    }
    allowed
}

pub fn hibernate_enabled() -> io::Result<bool> {
    let config = run("systemd-analyze", &["cat-config", "systemd/sleep.conf"])?;
    Ok(allows_hibernation(&config))
}

pub fn set_hibernate(enabled: bool) -> io::Result<()> {
    set_hibernate_in(Path::new(SLEEP_DROP_INS), enabled, hibernate_enabled)
}

/// `effective` reads the resulting state, for when removing the drop-in is not enough.
fn set_hibernate_in(
    dir: &Path,
    enabled: bool,
    effective: impl FnOnce() -> io::Result<bool>,
) -> io::Result<()> {
    let path = dir.join(HIBERNATE_DROP_IN);
    if enabled {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        // Still off: other configuration forbids it, which turning it on overrides.
        if !effective()? {
            write_config_file(&path, &hibernate_drop_in(true))?;
        }
        Ok(())
    } else {
        write_config_file(&path, &hibernate_drop_in(false))
    }
}

/// Writes a world-readable configuration file atomically, creating its directory.
fn write_config_file(path: &Path, contents: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, contents)?;
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o644))?;
    std::fs::rename(&temp, path)
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
    fn kconfig_dates_are_parsed() {
        let date = |s: &str| s.parse::<jiff::civil::DateTime>().unwrap();
        assert_eq!(
            parse_kconfig_datetime("2026,8,25,9,2,57.196"),
            Some(date("2026-08-25T09:02:57"))
        );
        assert_eq!(
            parse_kconfig_datetime(DND_FOREVER),
            Some(date("9999-12-31T00:00:00"))
        );
        assert_eq!(
            parse_kconfig_datetime("2026-10-30T12:00:00"),
            Some(date("2026-10-30T12:00:00"))
        );
        assert_eq!(parse_kconfig_datetime("2026,13,1,0,0,0"), None);
        assert_eq!(parse_kconfig_datetime("soon"), None);
    }

    #[test]
    fn the_last_allow_hibernation_wins() {
        assert!(allows_hibernation(""));
        assert!(allows_hibernation("[Sleep]\n#AllowHibernation=no\n"));
        let config = "# /etc/systemd/sleep.conf\n[Sleep]\nAllowHibernation=no\n\
                      # /etc/systemd/sleep.conf.d/x.conf\n[Sleep]\nAllowHibernation = yes\n";
        assert!(allows_hibernation(config));
        assert!(!allows_hibernation(&hibernate_drop_in(false)));
        assert!(allows_hibernation(&hibernate_drop_in(true)));
        // Other sections and invalid values are ignored.
        assert!(allows_hibernation(
            "[Other]\nAllowHibernation=no\n[Sleep]\nAllowHibernation=maybe\n"
        ));
    }

    #[test]
    fn hibernation_drop_in_is_written_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sleep.conf.d").join(HIBERNATE_DROP_IN);
        set_hibernate_in(&dir.path().join("sleep.conf.d"), false, || unreachable!()).unwrap();
        assert!(!allows_hibernation(
            &std::fs::read_to_string(&path).unwrap()
        ));

        set_hibernate_in(&dir.path().join("sleep.conf.d"), true, || Ok(true)).unwrap();
        assert!(!path.exists());

        // Something else still forbids it: an explicit "yes" overrides.
        set_hibernate_in(&dir.path().join("sleep.conf.d"), true, || Ok(false)).unwrap();
        assert!(allows_hibernation(&std::fs::read_to_string(&path).unwrap()));
    }

    #[test]
    fn store_ids_name_their_source() {
        let kde = Store::Kde {
            file: POWERDEVIL_RC,
            groups: vec!["AC", "Display"],
            key: "TurnOffDisplayWhenIdle",
        };
        assert_eq!(
            kde.id(),
            "kde:powerdevilrc/AC/Display/TurnOffDisplayWhenIdle"
        );
    }
}
