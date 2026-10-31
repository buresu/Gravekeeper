//! Linux. Per-user settings belong to the desktop environment, so they are read and written
//! the way its own settings app does: KConfig files through `kreadconfig6` / `kwriteconfig6`
//! on KDE Plasma 6, GSettings through `gsettings` on GNOME. Other desktops report the setting
//! as unsupported.
//!
//! Values replaced when a setting is turned off are kept in a per-user backup file (see
//! [`Backups`]) and put back when it is turned on again; with no backup the desktop's
//! default is restored.
//!
//! Machine-wide settings (hibernation, automatic logon, the firewall, automatic updates) are
//! changed by the elevated helper through systemd and the display manager's or firewall's own
//! configuration; each is read without elevation. What they replace is recorded under `/etc`
//! or `/var/lib/gravekeeper`, never in the user's backup file.

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

// --- Automatic logon -------------------------------------------------------------------------
//
// Set in the display manager's configuration, by the elevated helper. The account is the one
// that asked for the change: pkexec passes its uid to the helper as `PKEXEC_UID`, which the
// caller cannot forge, and the request itself names no account. No password is involved;
// the display manager's own PAM service starts the session.
//
// - Plasma Login Manager (an SDDM fork): `[Autologin] User` and `Session` in
//   `/etc/plasmalogin.conf`, which its settings page also writes and which overrides the
//   `.conf.d` drop-ins. Turning it off empties `User`, so a drop-in cannot turn it back on.
// - GDM: `[daemon] AutomaticLoginEnable` and `AutomaticLogin` in `custom.conf` (`/etc/gdm` or,
//   on Debian and Ubuntu, `/etc/gdm3`). GDM starts the user's default session.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisplayManager {
    PlasmaLogin,
    Gdm,
}

/// The display manager systemd starts, from the `display-manager.service` alias.
fn display_manager() -> io::Result<DisplayManager> {
    let unit = std::fs::read_link("/etc/systemd/system/display-manager.service").ok();
    let name = unit
        .as_deref()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    match name {
        "plasmalogin.service" => Ok(DisplayManager::PlasmaLogin),
        "gdm.service" | "gdm3.service" => Ok(DisplayManager::Gdm),
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "needs Plasma Login Manager or GDM on Linux",
        )),
    }
}

const PLASMA_LOGIN_CONF: &str = "/etc/plasmalogin.conf";
/// Read in this order, so later files override earlier ones; the main file comes last.
const PLASMA_LOGIN_DROP_INS: [&str; 2] = [
    "/usr/lib/plasmalogin/plasmalogin.conf.d",
    "/etc/plasmalogin.conf.d",
];

fn gdm_conf() -> PathBuf {
    if Path::new("/etc/gdm3").is_dir() {
        PathBuf::from("/etc/gdm3/custom.conf")
    } else {
        PathBuf::from("/etc/gdm/custom.conf")
    }
}

/// A configuration file's text, or empty when it does not exist.
fn read_config(path: &Path) -> io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(io::Error::new(
            err.kind(),
            format!("{}: {err}", path.display()),
        )),
    }
}

/// The value of `key` in `[section]` of an INI-style file (KConfig, GKeyFile), the last one if
/// it is set more than once.
fn ini_get(text: &str, section: &str, key: &str) -> Option<String> {
    let mut current = "";
    let mut found = None;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name;
        } else if current == section
            && !line.starts_with(['#', ';'])
            && let Some((name, value)) = line.split_once('=')
            && name.trim() == key
        {
            found = Some(value.trim().to_owned());
        }
    }
    found
}

/// Sets `key` in `[section]` of an INI-style file, or removes it with `None`, keeping every
/// other line (comments included) as it is. A missing section is added at the end.
fn ini_set(text: &str, section: &str, key: &str, value: Option<&str>) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut done = value.is_none();
    // Index in `out` just after the last non-blank line of the first matching section.
    let mut insert_at = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.to_owned();
        } else if current == section
            && !trimmed.starts_with(['#', ';'])
            && trimmed
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == key)
        {
            // Replace the first assignment, drop any repeats.
            if let Some(value) = value
                && !std::mem::replace(&mut done, true)
            {
                out.push(format!("{key}={value}"));
                insert_at = None;
            }
            continue;
        }
        out.push(line.to_owned());
        if current == section && !trimmed.is_empty() && !done {
            insert_at = Some(out.len());
        }
    }
    if let Some(value) = value.filter(|_| !done) {
        let line = format!("{key}={value}");
        match insert_at {
            Some(index) => out.insert(index, line),
            None => {
                if out.last().is_some_and(|l| !l.trim().is_empty()) {
                    out.push(String::new());
                }
                out.push(format!("[{section}]"));
                out.push(line);
            }
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// The `*.conf` files of a drop-in directory, in the order they are read.
fn drop_in_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "conf"))
        .collect();
    files.sort();
    files
}

fn plasma_login_value(key: &str) -> io::Result<Option<String>> {
    let mut files: Vec<PathBuf> = PLASMA_LOGIN_DROP_INS
        .iter()
        .flat_map(|dir| drop_in_files(Path::new(dir)))
        .collect();
    files.push(PathBuf::from(PLASMA_LOGIN_CONF));
    let mut value = None;
    for file in files {
        if let Some(found) = ini_get(&read_config(&file)?, "Autologin", key) {
            value = Some(found);
        }
    }
    Ok(value)
}

pub fn auto_login() -> io::Result<Option<String>> {
    match display_manager()? {
        DisplayManager::PlasmaLogin => {
            Ok(plasma_login_value("User")?.filter(|user| !user.is_empty()))
        }
        DisplayManager::Gdm => {
            let text = read_config(&gdm_conf())?;
            let enabled = ini_get(&text, "daemon", "AutomaticLoginEnable")
                .and_then(|value| parse_systemd_bool(&value))
                .unwrap_or(false);
            Ok(enabled
                .then(|| ini_get(&text, "daemon", "AutomaticLogin"))
                .flatten()
                .filter(|user| !user.is_empty()))
        }
    }
}

/// The account that asked for the change: pkexec's caller, or this process's own user when
/// Gravekeeper runs the task in-process.
fn requesting_user() -> io::Result<String> {
    let uid = match std::env::var("PKEXEC_UID") {
        Ok(uid) => uid
            .parse()
            .map_err(|_| io::Error::other("PKEXEC_UID is not a number"))?,
        // SAFETY: getuid has no preconditions and cannot fail.
        Err(_) => unsafe { libc::getuid() },
    };
    if uid == 0 {
        return Err(io::Error::other(
            "automatic logon as root is not supported; run Gravekeeper as the user to sign in",
        ));
    }
    user_name(uid)
}

fn user_name(uid: libc::uid_t) -> io::Result<String> {
    use std::ffi::CStr;

    let mut buffer = vec![0u8; 16 * 1024];
    // SAFETY: an all-zero passwd is a valid out-parameter; getpwuid_r fills it.
    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    // SAFETY: `passwd` and `buffer` outlive the call and the strings it points into `buffer`.
    let code = unsafe {
        libc::getpwuid_r(
            uid,
            &mut passwd,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code));
    }
    if result.is_null() {
        return Err(io::Error::other(format!("no account has uid {uid}")));
    }
    // SAFETY: on success `pw_name` is a NUL-terminated string inside `buffer`.
    let name = unsafe { CStr::from_ptr(passwd.pw_name) };
    let name = name.to_string_lossy().into_owned();
    // Goes into a configuration file line, so anything that could break it is refused.
    if name.is_empty() || name.chars().any(|c| c.is_control() || c == '[' || c == '=') {
        return Err(io::Error::other(format!("unusable account name {name:?}")));
    }
    Ok(name)
}

/// The session to start: the one already configured, else Plasma, else the first installed.
fn plasma_login_session() -> io::Result<String> {
    if let Some(session) = plasma_login_value("Session")?.filter(|s| !s.is_empty()) {
        return Ok(session);
    }
    let dirs = ["/usr/share/wayland-sessions", "/usr/share/xsessions"];
    if Path::new(dirs[0]).join("plasma.desktop").is_file() {
        return Ok("plasma.desktop".into());
    }
    dirs.iter()
        .flat_map(|dir| {
            let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "desktop"))
                .collect();
            files.sort();
            files
        })
        .find_map(|path| path.file_name()?.to_str().map(str::to_owned))
        .ok_or_else(|| io::Error::other("no desktop session is installed"))
}

/// Applies `edit` to a configuration file and writes it back atomically.
fn edit_config(path: &Path, edit: impl FnOnce(String) -> String) -> io::Result<()> {
    let text = read_config(path)?;
    write_config_file(path, &edit(text))
}

pub fn set_auto_login(enabled: bool) -> io::Result<()> {
    let manager = display_manager()?;
    let user = if enabled {
        Some(requesting_user()?)
    } else {
        None
    };
    match manager {
        DisplayManager::PlasmaLogin => {
            let session = match &user {
                Some(_) => Some(plasma_login_session()?),
                None => None,
            };
            edit_config(Path::new(PLASMA_LOGIN_CONF), |text| {
                let text = ini_set(
                    &text,
                    "Autologin",
                    "User",
                    Some(user.as_deref().unwrap_or("")),
                );
                match &session {
                    Some(session) => ini_set(&text, "Autologin", "Session", Some(session)),
                    None => text,
                }
            })
        }
        DisplayManager::Gdm => edit_config(&gdm_conf(), |text| {
            let text = ini_set(
                &text,
                "daemon",
                "AutomaticLoginEnable",
                Some(if user.is_some() { "True" } else { "False" }),
            );
            ini_set(&text, "daemon", "AutomaticLogin", user.as_deref())
        }),
    }
}

// --- Firewall --------------------------------------------------------------------------------
//
// ufw (Debian, Ubuntu) or firewalld (Fedora, openSUSE), whichever is installed; if both are,
// the one that is on. Turning it on or off is machine-wide and runs in the elevated helper:
// `ufw --force enable|disable` (`--force` skips its "may disrupt SSH" question), or
// `systemctl enable|disable --now firewalld`. Turning ufw on also enables `ufw.service` where it
// is disabled (Arch), or the firewall would not come back after a reboot. The state is read without elevation: ufw's
// `ENABLED` in `/etc/ufw/ufw.conf`, which is what it applies at boot, and firewalld's unit.

#[derive(Debug, Clone, PartialEq, Eq)]
enum Firewall {
    Ufw(PathBuf),
    Firewalld,
}

const UFW_CONF: &str = "/etc/ufw/ufw.conf";

fn ufw_binary() -> Option<PathBuf> {
    ["/usr/sbin/ufw", "/usr/bin/ufw", "/sbin/ufw"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

fn firewalld_installed() -> bool {
    unit_states(&["firewalld.service"]).is_ok_and(|states| !states.is_empty())
}

fn firewall() -> io::Result<Firewall> {
    let ufw = ufw_binary();
    let firewalld = firewalld_installed();
    match (ufw, firewalld) {
        (Some(ufw), true) => Ok(if !ufw_enabled()? && firewalld_enabled()? {
            Firewall::Firewalld
        } else {
            Firewall::Ufw(ufw)
        }),
        (Some(ufw), false) => Ok(Firewall::Ufw(ufw)),
        (None, true) => Ok(Firewall::Firewalld),
        (None, false) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "needs ufw or firewalld on Linux",
        )),
    }
}

/// `ENABLED=yes` in ufw.conf (a shell-style file, so the value may be quoted).
fn ufw_conf_enabled(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("ENABLED="))
        .next_back()
        .is_some_and(|value| value.trim_matches(['"', '\'']).eq_ignore_ascii_case("yes"))
}

fn ufw_enabled() -> io::Result<bool> {
    Ok(ufw_conf_enabled(&read_config(Path::new(UFW_CONF))?))
}

fn firewalld_enabled() -> io::Result<bool> {
    let enabled = unit_states(&["firewalld.service"])?
        .iter()
        .any(|(_, state)| unit_state_enabled(state));
    Ok(enabled || unit_active("firewalld.service"))
}

pub fn firewall_enabled() -> io::Result<bool> {
    match firewall()? {
        Firewall::Ufw(_) => ufw_enabled(),
        Firewall::Firewalld => firewalld_enabled(),
    }
}

pub fn set_firewall(enabled: bool) -> io::Result<()> {
    match firewall()? {
        Firewall::Ufw(ufw) => {
            let ufw = ufw.to_string_lossy();
            let args: &[&str] = if enabled {
                &["--force", "enable"]
            } else {
                &["disable"]
            };
            run(&ufw, args)?;
            // `ENABLED` is applied at boot by ufw.service, which Arch ships disabled. Turning
            // it off needs no change: the service then finds `ENABLED=no` and does nothing.
            if enabled
                && unit_states(&["ufw.service"])?
                    .iter()
                    .any(|(_, state)| state == "disabled")
            {
                run("systemctl", &["enable", "ufw.service"])?;
            }
            Ok(())
        }
        Firewall::Firewalld => {
            let action = if enabled { "enable" } else { "disable" };
            run("systemctl", &[action, "--now", "firewalld.service"]).map(drop)
        }
    }
}

// --- systemd units ---------------------------------------------------------------------------

/// The installed units among `units`, with their enablement state (`enabled`, `disabled`,
/// …), as `systemctl list-unit-files` reports them. Needs no elevation.
fn unit_states(units: &[&str]) -> io::Result<Vec<(String, String)>> {
    let output = Command::new("systemctl")
        .args(["list-unit-files", "--no-legend", "--no-pager"])
        .args(units)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| io::Error::other(format!("cannot run systemctl: {err}")))?;
    // It exits with 1 when none of the units is installed, which is an answer, not an error.
    Ok(parse_unit_states(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_unit_states(output: &str) -> Vec<(String, String)> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.to_owned(), fields.next()?.to_owned()))
        })
        .collect()
}

fn unit_state_enabled(state: &str) -> bool {
    matches!(state, "enabled" | "enabled-runtime")
}

fn unit_active(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", unit])
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

// --- Automatic updates -----------------------------------------------------------------------
//
// Linux has no one switch: distributions that update on their own do it from systemd timers,
// so those are what is turned off — apt's daily timers on Debian and Ubuntu (which also run
// unattended-upgrades), dnf-automatic's on Fedora and RHEL. A distribution with none of them
// installed (Arch, for instance) does not update on its own and reports the setting as
// unsupported. Software centres' own per-user update settings are not touched.
//
// Turning updates off disables and stops the timers that were enabled, after recording them in
// `/var/lib/gravekeeper/disabled-update-units` (machine-wide, so readable whichever account the
// helper runs as); turning them on re-enables exactly those. With no record the distribution's
// usual timers are enabled. Units are never masked or edited, so the change is plain
// `systemctl enable|disable`.

const UPDATE_TIMERS: &[&str] = &[
    "apt-daily.timer",
    "apt-daily-upgrade.timer",
    "dnf-automatic.timer",
    "dnf-automatic-download.timer",
    "dnf-automatic-install.timer",
    "dnf-automatic-notifyonly.timer",
    "dnf5-automatic.timer",
];
/// Enabled when turning updates on with no record of what was turned off: Debian's defaults,
/// and the main timer of whichever dnf-automatic is installed.
const DEFAULT_UPDATE_TIMERS: &[&str] = &[
    "apt-daily.timer",
    "apt-daily-upgrade.timer",
    "dnf5-automatic.timer",
    "dnf-automatic.timer",
];
const UPDATE_RECORD: &str = "/var/lib/gravekeeper/disabled-update-units";

fn installed_update_timers() -> io::Result<Vec<(String, String)>> {
    let installed = unit_states(UPDATE_TIMERS)?;
    if installed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this distribution has no automatic update timers (apt-daily, dnf-automatic)",
        ));
    }
    Ok(installed)
}

pub fn automatic_updates() -> io::Result<bool> {
    Ok(installed_update_timers()?
        .iter()
        .any(|(_, state)| unit_state_enabled(state)))
}

pub fn set_automatic_updates(enabled: bool) -> io::Result<()> {
    let installed = installed_update_timers()?;
    let record = Path::new(UPDATE_RECORD);
    let recorded = read_config(record)?;
    let mut recorded: Vec<&str> = recorded
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if enabled {
        let is_installed = |unit: &&str| installed.iter().any(|(name, _)| name == unit);
        recorded.retain(is_installed);
        let units: Vec<&str> = if recorded.is_empty() {
            let mut defaults: Vec<&str> = DEFAULT_UPDATE_TIMERS
                .iter()
                .copied()
                .filter(is_installed)
                .collect();
            // Only one dnf-automatic: dnf5's if both are listed.
            if defaults.contains(&"dnf5-automatic.timer") {
                defaults.retain(|unit| *unit != "dnf-automatic.timer");
            }
            defaults
        } else {
            recorded
        };
        if !units.is_empty() {
            run(
                "systemctl",
                &[&["enable", "--now"], units.as_slice()].concat(),
            )?;
        }
        match std::fs::remove_file(record) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    } else {
        let units: Vec<&str> = installed
            .iter()
            .filter(|(_, state)| unit_state_enabled(state))
            .map(|(name, _)| name.as_str())
            .collect();
        for unit in &units {
            if !recorded.contains(unit) {
                recorded.push(unit);
            }
        }
        // Recorded first, so a failure never leaves timers disabled with nothing to restore.
        write_config_file(record, &(recorded.join("\n") + "\n"))?;
        if !units.is_empty() {
            run(
                "systemctl",
                &[&["disable", "--now"], units.as_slice()].concat(),
            )?;
        }
        Ok(())
    }
}

// --- Windows only ---------------------------------------------------------------------

pub fn security_notifications_enabled() -> io::Result<bool> {
    Err(unsupported())
}

pub fn set_security_notifications(_enabled: bool) -> io::Result<()> {
    Err(unsupported())
}

pub fn set_security_notifications_user(_enabled: bool) -> io::Result<()> {
    Err(unsupported())
}

// --- macOS only ------------------------------------------------------------------------

pub fn app_nap_enabled() -> io::Result<bool> {
    Err(unsupported())
}

pub fn set_app_nap(_enabled: bool) -> io::Result<()> {
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
    fn ini_values_are_read() {
        let text = "[Autologin]\nUser=a\n# User=b\n[Other]\nUser=c\n[Autologin]\nUser = d\n";
        assert_eq!(ini_get(text, "Autologin", "User").as_deref(), Some("d"));
        assert_eq!(ini_get(text, "Autologin", "Session"), None);
        assert_eq!(
            ini_get("[Autologin]\nUser=\n", "Autologin", "User").as_deref(),
            Some("")
        );
    }

    #[test]
    fn ini_set_replaces_in_place() {
        let text = "# comment\n[Autologin]\nRelogin=false\nUser=old\n\n[Theme]\nCurrent=breeze\n";
        assert_eq!(
            ini_set(text, "Autologin", "User", Some("new")),
            "# comment\n[Autologin]\nRelogin=false\nUser=new\n\n[Theme]\nCurrent=breeze\n"
        );
        assert_eq!(
            ini_set(text, "Autologin", "User", None),
            "# comment\n[Autologin]\nRelogin=false\n\n[Theme]\nCurrent=breeze\n"
        );
    }

    #[test]
    fn ini_set_adds_missing_keys_and_sections() {
        // Into the existing section, before the blank line that ends it.
        assert_eq!(
            ini_set(
                "[daemon]\n# x\n\n[security]\n",
                "daemon",
                "AutomaticLogin",
                Some("u")
            ),
            "[daemon]\n# x\nAutomaticLogin=u\n\n[security]\n"
        );
        assert_eq!(
            ini_set("[Theme]\nCurrent=breeze\n", "Autologin", "User", Some("u")),
            "[Theme]\nCurrent=breeze\n\n[Autologin]\nUser=u\n"
        );
        assert_eq!(
            ini_set("", "Autologin", "User", Some("u")),
            "[Autologin]\nUser=u\n"
        );
        // Removing what is not there changes nothing.
        assert_eq!(ini_set("[A]\nB=c\n", "A", "D", None), "[A]\nB=c\n");
    }

    #[test]
    fn ini_set_drops_repeated_keys() {
        assert_eq!(
            ini_set("[A]\nK=1\nK=2\n", "A", "K", Some("3")),
            "[A]\nK=3\n"
        );
    }

    #[test]
    fn the_current_user_has_a_name() {
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        assert!(!user_name(uid).unwrap().is_empty());
    }

    #[test]
    fn ufw_conf_is_read() {
        assert!(ufw_conf_enabled("# comment\nENABLED=yes\nLOGLEVEL=low\n"));
        assert!(ufw_conf_enabled("ENABLED=\"yes\"\n"));
        assert!(!ufw_conf_enabled("ENABLED=no\n"));
        assert!(!ufw_conf_enabled("ENABLED=yes\nENABLED=no\n"));
        assert!(!ufw_conf_enabled(""));
    }

    #[test]
    fn unit_states_are_parsed() {
        let output = "apt-daily.timer         enabled  enabled\n\
                      apt-daily-upgrade.timer disabled enabled\n";
        assert_eq!(
            parse_unit_states(output),
            [
                ("apt-daily.timer".into(), "enabled".into()),
                ("apt-daily-upgrade.timer".into(), "disabled".into())
            ]
        );
        assert!(parse_unit_states("").is_empty());
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
