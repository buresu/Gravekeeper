//! macOS. Settings are changed with the tools the system itself provides: `defaults` for
//! preference files, `pmset` for power management, `socketfilterfw` for the application
//! firewall. Everything is read without elevation.
//!
//! Unlike Windows and Linux, the power settings (sleep, display and disk off, hibernation) are
//! machine-wide on macOS — System Settings asks for an administrator to change them — so they
//! go through the elevated helper too (see [`super::POWER_NEEDS_ADMIN`]). The per-user ones
//! (App Nap, the screen saver, reopening apps at login) are changed in-process.
//!
//! Values replaced when a setting is turned off are recorded and put back when it is turned
//! on again; with no record the macOS default is restored. Machine-wide values are recorded in
//! [`RECORD`] by the elevated helper, per-user ones in the user's data directory.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn unsupported(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, why.to_owned())
}

fn spawn(program: &str, args: &[&str]) -> io::Result<Output> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| match err.kind() {
            io::ErrorKind::NotFound => {
                io::Error::new(io::ErrorKind::NotFound, format!("{program} was not found"))
            }
            _ => io::Error::other(format!("cannot run {program}: {err}")),
        })
}

fn failure(program: &str, output: &Output) -> io::Error {
    io::Error::other(format!(
        "{program} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// Runs a tool and returns its trimmed stdout.
fn run(program: &str, args: &[&str]) -> io::Result<String> {
    let output = spawn(program, args)?;
    if !output.status.success() {
        return Err(failure(program, &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

// --- Record of replaced values ---------------------------------------------------------------

/// Machine-wide, since the elevated helper writes it and may run as another account.
const RECORD: &str = "/Library/Application Support/Gravekeeper/os-settings-backup.json";

/// Values replaced by turning a setting off, keyed by where they came from.
struct Record {
    path: PathBuf,
    values: BTreeMap<String, String>,
}

impl Record {
    fn load() -> io::Result<Self> {
        Self::load_from(PathBuf::from(RECORD))
    }

    /// For per-user values, which must not go in the machine-wide record.
    fn load_user() -> io::Result<Self> {
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
        let json = serde_json::to_vec_pretty(&self.values).expect("strings serialize");
        write_file(&self.path, &json, 0o644)
    }
}

/// Writes a file atomically with the given mode, creating its directory. The file is created
/// with that mode, so a secret is never readable by others, even briefly.
fn write_file(path: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&temp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temp)?;
    // The umask may have taken bits away from `mode`.
    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    file.write_all(contents)?;
    file.sync_all()?;
    std::fs::rename(&temp, path)
}

fn remove_file(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

// --- Preferences (`defaults`) ----------------------------------------------------------------

/// One preference key, as `defaults` names it: a file path for machine-wide preferences under
/// `/Library/Preferences`, or a domain for the user's own.
#[derive(Debug, Clone, Copy)]
struct Pref {
    domain: &'static str,
    key: &'static str,
    /// The `defaults write` type: `-bool`, `-int` or `-string`.
    kind: &'static str,
    /// Kept per computer, in `~/Library/Preferences/ByHost` (the screen saver's settings).
    current_host: bool,
}

impl Pref {
    const fn new(domain: &'static str, key: &'static str, kind: &'static str) -> Self {
        Pref {
            domain,
            key,
            kind,
            current_host: false,
        }
    }

    const fn current_host(self) -> Self {
        Pref {
            current_host: true,
            ..self
        }
    }

    /// The record key.
    fn id(&self) -> String {
        let host = if self.current_host {
            "currentHost:"
        } else {
            ""
        };
        format!("defaults:{host}{}/{}", self.domain, self.key)
    }

    fn args<'a>(&'a self, verb: &'a str) -> Vec<&'a str> {
        let mut args = Vec::with_capacity(6);
        if self.current_host {
            args.push("-currentHost");
        }
        args.extend([verb, self.domain, self.key]);
        args
    }

    /// The value as `defaults read` prints it (booleans as `1` / `0`), or `None` when it is
    /// not set.
    fn read(&self) -> io::Result<Option<String>> {
        let output = spawn("defaults", &self.args("read"))?;
        if output.status.success() {
            return Ok(Some(
                String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            ));
        }
        // "The domain/default pair of (…, …) does not exist", or "Could not find key".
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("does not exist")
            || stderr.contains("Could not find")
            || stderr.contains("not found")
        {
            return Ok(None);
        }
        Err(failure("defaults", &output))
    }

    /// Booleans are given as `1` / `0`, the way [`Pref::read`] returns them.
    fn write(&self, value: &str) -> io::Result<()> {
        let value = match self.kind {
            "-bool" if value == "0" => "false",
            "-bool" => "true",
            _ => value,
        };
        let mut args = self.args("write");
        args.extend([self.kind, value]);
        run("defaults", &args).map(drop)
    }

    /// Removes the key, so the default applies. A key that is not set is not an error.
    fn delete(&self) -> io::Result<()> {
        match self.read()? {
            Some(_) => run("defaults", &self.args("delete")).map(drop),
            None => Ok(()),
        }
    }
}

// --- Power management (`pmset`) --------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    Ac,
    Battery,
    Ups,
}

impl Source {
    fn from_heading(heading: &str) -> Option<Self> {
        match heading {
            "AC Power" => Some(Source::Ac),
            "Battery Power" => Some(Source::Battery),
            "UPS Power" => Some(Source::Ups),
            _ => None,
        }
    }

    /// The `pmset` option that selects it.
    fn flag(self) -> &'static str {
        match self {
            Source::Ac => "-c",
            Source::Battery => "-b",
            Source::Ups => "-u",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Source::Ac => "ac",
            Source::Battery => "battery",
            Source::Ups => "ups",
        }
    }
}

/// The settings of each power source this Mac has, from `pmset -g custom`. A computer
/// without a battery has no "Battery Power" section, so its battery settings are ignored.
fn parse_pmset(output: &str) -> BTreeMap<Source, BTreeMap<String, String>> {
    let mut sources = BTreeMap::new();
    let mut current = None;
    for line in output.lines() {
        if let Some(heading) = line.trim_end().strip_suffix(':')
            && !line.starts_with(char::is_whitespace)
        {
            current = Source::from_heading(heading.trim());
            if let Some(source) = current {
                sources.insert(source, BTreeMap::new());
            }
            continue;
        }
        // ` sleep                1 (sleep prevented by powerd)`: the value is the first word
        // after the name. Names may contain spaces ("Sleep On Power Button"), but none of the
        // ones used here do.
        let (Some(source), Some((key, rest))) = (current, line.trim().split_once(' ')) else {
            continue;
        };
        if let Some(value) = rest.split_whitespace().next() {
            sources
                .get_mut(&source)
                .expect("inserted with its heading")
                .insert(key.to_owned(), value.to_owned());
        }
    }
    sources
}

fn power_settings() -> io::Result<BTreeMap<Source, BTreeMap<String, String>>> {
    Ok(parse_pmset(&run("pmset", &["-g", "custom"])?))
}

// --- Switchable values -----------------------------------------------------------------------

/// Where macOS keeps one value.
enum Store {
    Pref(Pref),
    /// A `pmset` setting of one power source.
    Pmset {
        source: Source,
        key: &'static str,
    },
}

impl Store {
    /// The record key.
    fn id(&self) -> String {
        match self {
            Store::Pref(pref) => pref.id(),
            Store::Pmset { source, key } => format!("pmset:{}/{key}", source.name()),
        }
    }

    fn write(&self, value: &str) -> io::Result<()> {
        match self {
            Store::Pref(pref) => pref.write(value),
            Store::Pmset { source, key } => run("pmset", &[source.flag(), key, value]).map(drop),
        }
    }
}

/// One value of a setting that can be switched off.
struct Entry {
    store: Store,
    /// As read; `None` when the preference is not set.
    current: Option<String>,
    /// The value that switches it off.
    off: &'static str,
    /// The macOS default: what an unset preference means, and what a power setting is set to
    /// when turned on with nothing recorded.
    default: &'static str,
}

impl Entry {
    fn pref(pref: Pref, off: &'static str, default: &'static str) -> io::Result<Self> {
        Ok(Entry {
            current: pref.read()?,
            store: Store::Pref(pref),
            off,
            default,
        })
    }

    fn is_off(&self) -> bool {
        self.current.as_deref().unwrap_or(self.default) == self.off
    }

    /// Puts back a recorded value, or with none the default: an unset preference, or the
    /// default power setting.
    fn restore(&self, saved: Option<&str>) -> io::Result<()> {
        match (&self.store, saved) {
            (_, Some(value)) => self.store.write(value),
            (Store::Pref(pref), None) => pref.delete(),
            (Store::Pmset { .. }, None) => self.store.write(self.default),
        }
    }
}

/// `key` of every power source this Mac has, with the default for each. Empty when the Mac
/// does not have the setting.
fn pmset_entries(
    key: &'static str,
    off: &'static str,
    default: impl Fn(Source) -> &'static str,
) -> io::Result<Vec<Entry>> {
    Ok(power_settings()?
        .into_iter()
        .filter_map(|(source, settings)| {
            Some(Entry {
                current: Some(settings.get(key)?.clone()),
                store: Store::Pmset { source, key },
                off,
                default: default(source),
            })
        })
        .collect())
}

/// On if any entry is not off.
fn entries_on(entries: &[Entry]) -> bool {
    entries.iter().any(|entry| !entry.is_off())
}

/// For machine-wide values, in the elevated helper.
fn set_entries(entries: &[Entry], enabled: bool) -> io::Result<()> {
    set_entries_in(&mut Record::load()?, entries, enabled)
}

/// For per-user values, in Gravekeeper's own process.
fn set_user_entries(entries: &[Entry], enabled: bool) -> io::Result<()> {
    set_entries_in(&mut Record::load_user()?, entries, enabled)
}

fn set_entries_in(record: &mut Record, entries: &[Entry], enabled: bool) -> io::Result<()> {
    if enabled {
        for entry in entries {
            let saved = record.values.remove(&entry.store.id());
            // Only an entry still off is restored; one changed since in System Settings is
            // left as it is.
            if entry.is_off() {
                entry.restore(saved.as_deref())?;
            }
        }
        record.save()
    } else {
        for entry in entries {
            if let Some(value) = &entry.current
                && value != entry.off
            {
                record.values.insert(entry.store.id(), value.clone());
            }
        }
        // Saved before anything changes, so a failure never loses the values to restore.
        record.save()?;
        for entry in entries.iter().filter(|entry| !entry.is_off()) {
            entry.store.write(entry.off)?;
        }
        Ok(())
    }
}

// --- Power (sleep, display and disk off) -----------------------------------------------------
//
// `sleep`, `displaysleep` and `disksleep` of each power source, in minutes; 0 = never, which
// is what System Settings' "Prevent automatic sleeping when the display is off" sets. With
// nothing recorded, turning one on sets a real wait, as on Windows: 30 / 15 minutes of system
// sleep (AC / battery), and the macOS defaults of 10 / 2 minutes for the display and 10 for
// disks. Only hard disk drives (such as external ones) spin down; SSDs are not affected.

fn sleep_entries() -> io::Result<Vec<Entry>> {
    pmset_entries("sleep", "0", |source| match source {
        Source::Battery => "15",
        Source::Ac | Source::Ups => "30",
    })
}

fn display_entries() -> io::Result<Vec<Entry>> {
    pmset_entries("displaysleep", "0", |source| match source {
        Source::Battery => "2",
        Source::Ac | Source::Ups => "10",
    })
}

pub fn automatic_sleep() -> io::Result<bool> {
    Ok(entries_on(&sleep_entries()?))
}

pub fn set_automatic_sleep(enabled: bool) -> io::Result<()> {
    set_entries(&sleep_entries()?, enabled)
}

pub fn display_off() -> io::Result<bool> {
    Ok(entries_on(&display_entries()?))
}

pub fn set_display_off(enabled: bool) -> io::Result<()> {
    set_entries(&display_entries()?, enabled)
}

fn disk_entries() -> io::Result<Vec<Entry>> {
    let entries = pmset_entries("disksleep", "0", |_| "10")?;
    if entries.is_empty() {
        return Err(unsupported("this Mac has no disk sleep setting"));
    }
    Ok(entries)
}

pub fn disk_off() -> io::Result<bool> {
    Ok(entries_on(&disk_entries()?))
}

pub fn set_disk_off(enabled: bool) -> io::Result<()> {
    set_entries(&disk_entries()?, enabled)
}

// --- Restart after a power failure, wake for network access ----------------------------------
//
// `autorestart` and `womp` (Wake on LAN, "Wake for network access") of each power source, 1 =
// on. Both are machine-wide and run in the elevated helper. Laptops have no `autorestart`, and
// the setting is then unavailable.

fn switch_entries(key: &'static str, missing: &str) -> io::Result<Vec<Entry>> {
    let entries = pmset_entries(key, "0", |_| "1")?;
    if entries.is_empty() {
        return Err(unsupported(missing));
    }
    Ok(entries)
}

fn restart_entries() -> io::Result<Vec<Entry>> {
    switch_entries(
        "autorestart",
        "this Mac cannot start up after a power failure",
    )
}

fn wake_entries() -> io::Result<Vec<Entry>> {
    switch_entries("womp", "this Mac cannot wake for network access")
}

pub fn restart_after_power_failure() -> io::Result<bool> {
    Ok(entries_on(&restart_entries()?))
}

pub fn set_restart_after_power_failure(enabled: bool) -> io::Result<()> {
    set_entries(&restart_entries()?, enabled)
}

pub fn wake_for_network() -> io::Result<bool> {
    Ok(entries_on(&wake_entries()?))
}

pub fn set_wake_for_network(enabled: bool) -> io::Result<()> {
    set_entries(&wake_entries()?, enabled)
}

// --- Hibernation -----------------------------------------------------------------------------
//
// `hibernatemode` of each power source: 0 = sleep only, 3 = also write memory to disk (the
// laptop default), 25 = hibernate (Intel only). Desktops with Apple silicon do not hibernate
// and `pmset` lists no `hibernatemode` for them, so the setting is unavailable there. Turning
// it off also deletes the sleep image, freeing as much disk space as there is memory.

const SLEEP_IMAGE: &str = "/var/vm/sleepimage";

fn hibernate_entries() -> io::Result<Vec<Entry>> {
    let entries = pmset_entries("hibernatemode", "0", |_| "3")?;
    if entries.is_empty() {
        return Err(unsupported("this Mac does not hibernate"));
    }
    Ok(entries)
}

pub fn hibernate_enabled() -> io::Result<bool> {
    Ok(entries_on(&hibernate_entries()?))
}

pub fn set_hibernate(enabled: bool) -> io::Result<()> {
    set_entries(&hibernate_entries()?, enabled)?;
    if !enabled {
        remove_file(Path::new(SLEEP_IMAGE))?;
    }
    Ok(())
}

// --- Automatic updates -----------------------------------------------------------------------
//
// The switches of System Settings > General > Software Update > Automatic Updates, in
// `/Library/Preferences/com.apple.SoftwareUpdate`. "Off" turns them all off, including the
// check itself; updating by hand still works. App Store apps have their own switch, which is
// left alone, like the software centres on Linux.

const SOFTWARE_UPDATE: &str = "/Library/Preferences/com.apple.SoftwareUpdate";

/// The keys, and what an unset key means.
const UPDATE_KEYS: [(&str, &str); 5] = [
    ("AutomaticCheckEnabled", "1"),
    ("AutomaticDownload", "1"),
    ("AutomaticallyInstallMacOSUpdates", "0"),
    ("CriticalUpdateInstall", "1"),
    ("ConfigDataInstall", "1"),
];

fn update_entries() -> io::Result<Vec<Entry>> {
    UPDATE_KEYS
        .into_iter()
        .map(|(key, default)| Entry::pref(Pref::new(SOFTWARE_UPDATE, key, "-bool"), "0", default))
        .collect()
}

pub fn automatic_updates() -> io::Result<bool> {
    Ok(entries_on(&update_entries()?))
}

pub fn set_automatic_updates(enabled: bool) -> io::Result<()> {
    set_entries(&update_entries()?, enabled)
}

// --- Automatic logon -------------------------------------------------------------------------
//
// As System Settings > Users & Groups does it: `autoLoginUser` in
// `/Library/Preferences/com.apple.loginwindow`, and the password in `/etc/kcpassword`
// (readable by root only), XOR-obfuscated with a fixed key — that is macOS's own format, not
// encryption. The account is the one signed in at the screen (the owner of `/dev/console`),
// which the request cannot change. The elevated helper asks for its password in a dialog and
// checks it through PAM before saving, so a typo does not leave the Mac at the login window.
// macOS does not sign in automatically while FileVault is on.

const AUTO_LOGIN_USER: Pref = Pref::new(
    "/Library/Preferences/com.apple.loginwindow",
    "autoLoginUser",
    "-string",
);
const KCPASSWORD: &str = "/etc/kcpassword";

fn filevault_on() -> io::Result<bool> {
    // Prints `true` or `false`; the exit status is 1 for false.
    let output = spawn("fdesetup", &["isactive"])?;
    match String::from_utf8_lossy(&output.stdout).trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(failure("fdesetup", &output)),
    }
}

fn no_filevault() -> io::Result<()> {
    if filevault_on()? {
        return Err(unsupported(
            "FileVault is on, and macOS does not sign in automatically with it",
        ));
    }
    Ok(())
}

pub fn auto_login() -> io::Result<Option<String>> {
    no_filevault()?;
    Ok(AUTO_LOGIN_USER.read()?.filter(|user| !user.is_empty()))
}

pub fn set_auto_login(enabled: bool) -> io::Result<()> {
    if !enabled {
        AUTO_LOGIN_USER.delete()?;
        return remove_file(Path::new(KCPASSWORD));
    }
    no_filevault()?;
    let user = console_user()?;
    let password = ask_password(&user)?;
    if !pam::password_is_correct(&user, &password)? {
        return Err(io::Error::other(format!(
            "that is not the password of {user}"
        )));
    }
    write_file(
        Path::new(KCPASSWORD),
        &kcpassword(password.as_bytes()),
        0o600,
    )?;
    AUTO_LOGIN_USER.write(&user)
}

/// The account signed in at the screen.
fn console_user() -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;

    let uid = std::fs::metadata("/dev/console")?.uid();
    if uid == 0 {
        return Err(io::Error::other(
            "nobody is signed in at the screen, or root is; sign in as the user to sign in automatically",
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
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(io::Error::other(format!("unusable account name {name:?}")));
    }
    Ok(name)
}

/// Asks for the password in a dialog. The password comes back on the dialog's stdout, never
/// on a command line.
fn ask_password(user: &str) -> io::Result<String> {
    let user = user.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "text returned of (display dialog \"Enter the password of {user} to sign in \
         automatically at startup.\" default answer \"\" with hidden answer \
         with title \"Gravekeeper\" with icon caution)"
    );
    let output = spawn("osascript", &["-e", &script])?;
    if !output.status.success() {
        if String::from_utf8_lossy(&output.stderr).contains("(-128)") {
            return Err(io::Error::other("the password prompt was cancelled"));
        }
        return Err(failure("osascript", &output));
    }
    let mut password = String::from_utf8(output.stdout)
        .map_err(|_| io::Error::other("the password is not valid UTF-8"))?;
    // osascript ends its result with a line feed; a password cannot contain one.
    if password.ends_with('\n') {
        password.pop();
    }
    if password.is_empty() {
        return Err(io::Error::other("no password was entered"));
    }
    Ok(password)
}

/// The `/etc/kcpassword` format: the password and a NUL, padded with NULs to a multiple of 12
/// bytes (macOS 13 and later need this; earlier versions read up to the NUL), XORed with a
/// fixed 11-byte key.
fn kcpassword(password: &[u8]) -> Vec<u8> {
    const KEY: [u8; 11] = [
        0x7D, 0x89, 0x52, 0x23, 0xD2, 0xBC, 0xDD, 0xEA, 0xA3, 0xB9, 0x1F,
    ];
    let mut bytes = password.to_vec();
    bytes.push(0);
    bytes.resize(bytes.len().next_multiple_of(12), 0);
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte ^= KEY[i % KEY.len()];
    }
    bytes
}

/// Password checks through PAM's `checkpw` service, as `checkpw(3)` does: the password is
/// handed over as the authentication token, so no conversation is needed.
mod pam {
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::io;

    const PAM_SUCCESS: c_int = 0;
    const PAM_CONV_ERR: c_int = 6;
    const PAM_AUTH_ERR: c_int = 9;
    const PAM_USER_UNKNOWN: c_int = 13;
    const PAM_AUTHTOK: c_int = 6;

    #[repr(C)]
    struct PamConv {
        conv: extern "C" fn(c_int, *mut *const c_void, *mut *mut c_void, *mut c_void) -> c_int,
        appdata_ptr: *mut c_void,
    }

    #[link(name = "pam")]
    unsafe extern "C" {
        fn pam_start(
            service: *const c_char,
            user: *const c_char,
            conv: *const PamConv,
            handle: *mut *mut c_void,
        ) -> c_int;
        fn pam_set_item(handle: *mut c_void, item: c_int, value: *const c_void) -> c_int;
        fn pam_authenticate(handle: *mut c_void, flags: c_int) -> c_int;
        fn pam_end(handle: *mut c_void, status: c_int) -> c_int;
    }

    /// `checkpw` uses `use_first_pass`, so it never asks; anything that does is refused.
    extern "C" fn no_conversation(
        _: c_int,
        _: *mut *const c_void,
        _: *mut *mut c_void,
        _: *mut c_void,
    ) -> c_int {
        PAM_CONV_ERR
    }

    pub fn password_is_correct(user: &str, password: &str) -> io::Result<bool> {
        let invalid = |_| io::Error::other("the name or password contains a NUL");
        let user = CString::new(user).map_err(invalid)?;
        let password = CString::new(password).map_err(invalid)?;
        let conv = PamConv {
            conv: no_conversation,
            appdata_ptr: std::ptr::null_mut(),
        };
        let mut handle = std::ptr::null_mut();
        // SAFETY: the strings and `conv` outlive the PAM transaction, which ends below.
        let mut status =
            unsafe { pam_start(c"checkpw".as_ptr(), user.as_ptr(), &conv, &mut handle) };
        if status != PAM_SUCCESS {
            return Err(io::Error::other(format!("pam_start failed ({status})")));
        }
        // SAFETY: `handle` is the open transaction; PAM copies the token.
        status = unsafe { pam_set_item(handle, PAM_AUTHTOK, password.as_ptr().cast()) };
        if status == PAM_SUCCESS {
            // SAFETY: as above.
            status = unsafe { pam_authenticate(handle, 0) };
        }
        // SAFETY: ends the transaction opened above; `handle` is not used afterwards.
        unsafe { pam_end(handle, status) };
        match status {
            PAM_SUCCESS => Ok(true),
            PAM_AUTH_ERR | PAM_USER_UNKNOWN => Ok(false),
            _ => Err(io::Error::other(format!(
                "cannot check the password (PAM error {status})"
            ))),
        }
    }
}

// --- Firewall --------------------------------------------------------------------------------
//
// The application firewall of System Settings > Network > Firewall, through `socketfilterfw`.
// Reading needs no rights; turning it on or off is machine-wide and runs in the elevated
// helper. State 2 ("block all incoming connections") counts as on.

const SOCKETFILTERFW: &str = "/usr/libexec/ApplicationFirewall/socketfilterfw";

/// `Firewall is enabled. (State = 1)`.
fn parse_firewall_state(output: &str) -> Option<bool> {
    let state = output.split("State = ").nth(1)?;
    let digits: String = state.chars().take_while(char::is_ascii_digit).collect();
    Some(digits.parse::<u32>().ok()? != 0)
}

pub fn firewall_enabled() -> io::Result<bool> {
    let output = run(SOCKETFILTERFW, &["--getglobalstate"])?;
    parse_firewall_state(&output).ok_or_else(|| {
        io::Error::other(format!("unexpected answer from socketfilterfw: {output:?}"))
    })
}

pub fn set_firewall(enabled: bool) -> io::Result<()> {
    let state = if enabled { "on" } else { "off" };
    run(SOCKETFILTERFW, &["--setglobalstate", state]).map(drop)
}

// --- App Nap ---------------------------------------------------------------------------------
//
// macOS slows down apps whose windows are hidden or covered (timers coalesced, low CPU and
// I/O priority), which can stall supervised programs on an unattended machine. The per-user
// `NSAppSleepDisabled` in the global domain turns it off for every app of this user; apps
// read it at launch, so it applies to apps started afterwards. No administrator rights are
// needed. On is the default, so turning it on removes the key.

const APP_SLEEP_DISABLED: Pref = Pref::new("NSGlobalDomain", "NSAppSleepDisabled", "-bool");

/// `defaults` prints a boolean as `1` / `0`, but a value written as a string or by hand may
/// read `YES` or `true`.
fn parse_defaults_bool(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "1" | "yes" | "true")
}

pub fn app_nap_enabled() -> io::Result<bool> {
    Ok(!APP_SLEEP_DISABLED
        .read()?
        .as_deref()
        .is_some_and(parse_defaults_bool))
}

pub fn set_app_nap(enabled: bool) -> io::Result<()> {
    if enabled {
        APP_SLEEP_DISABLED.delete()
    } else {
        APP_SLEEP_DISABLED.write("1")
    }
}

// --- Screen saver ----------------------------------------------------------------------------
//
// `idleTime` (seconds, 0 = never) in the user's per-computer `com.apple.screensaver`, which
// System Settings > Lock Screen > "Start Screen Saver when inactive" sets. Per-user, so
// changed in-process. Unset means the macOS default, which counts as on.

const SCREEN_SAVER_IDLE: Pref =
    Pref::new("com.apple.screensaver", "idleTime", "-int").current_host();

fn screen_saver_entries() -> io::Result<Vec<Entry>> {
    Ok(vec![Entry::pref(SCREEN_SAVER_IDLE, "0", "1200")?])
}

pub fn screen_saver_enabled() -> io::Result<bool> {
    Ok(entries_on(&screen_saver_entries()?))
}

pub fn set_screen_saver(enabled: bool) -> io::Result<()> {
    set_user_entries(&screen_saver_entries()?, enabled)
}

// --- Reopening apps at login -----------------------------------------------------------------
//
// The user's `com.apple.loginwindow`: `TALLogoutSavesState` is the "Reopen windows when
// logging back in" checkbox of the restart and shut-down dialogs (macOS remembers its last
// state there), and `LoginwindowLaunchesRelaunchApps` whether apps saved at logout are opened.
// Gravekeeper starts supervised apps itself, so macOS reopening them as well would start them
// twice. Per-user, so changed in-process.

fn reopen_entries() -> io::Result<Vec<Entry>> {
    ["TALLogoutSavesState", "LoginwindowLaunchesRelaunchApps"]
        .into_iter()
        .map(|key| Entry::pref(Pref::new("com.apple.loginwindow", key, "-bool"), "0", "1"))
        .collect()
}

/// Either key being off is enough: nothing is saved, or nothing saved is opened.
pub fn reopen_apps_enabled() -> io::Result<bool> {
    Ok(!reopen_entries()?.iter().any(Entry::is_off))
}

pub fn set_reopen_apps(enabled: bool) -> io::Result<()> {
    set_user_entries(&reopen_entries()?, enabled)
}

// --- Bluetooth Setup Assistant ---------------------------------------------------------------
//
// `BluetoothAutoSeekKeyboard` and `BluetoothAutoSeekPointingDevice` in
// `/Library/Preferences/com.apple.Bluetooth`: whether the Bluetooth Setup Assistant opens at
// startup when no keyboard or mouse is found, which would cover the screen of a Mac running
// without them. Machine-wide; applies at the next startup.

const BLUETOOTH: &str = "/Library/Preferences/com.apple.Bluetooth";

fn bluetooth_entries() -> io::Result<Vec<Entry>> {
    [
        "BluetoothAutoSeekKeyboard",
        "BluetoothAutoSeekPointingDevice",
    ]
    .into_iter()
    .map(|key| Entry::pref(Pref::new(BLUETOOTH, key, "-int"), "0", "1"))
    .collect()
}

pub fn bluetooth_setup_assistant() -> io::Result<bool> {
    Ok(entries_on(&bluetooth_entries()?))
}

pub fn set_bluetooth_setup_assistant(enabled: bool) -> io::Result<()> {
    set_entries(&bluetooth_entries()?, enabled)
}

// --- Not available on macOS ------------------------------------------------------------------

/// Focus (Do Not Disturb) has no supported interface for other apps since macOS 12: it can be
/// switched only in Control Center or through a user's own Shortcuts.
pub fn notifications_enabled() -> io::Result<bool> {
    Err(unsupported(
        "macOS lets apps neither read nor switch Focus (Do Not Disturb)",
    ))
}

pub fn set_notifications(_enabled: bool) -> io::Result<()> {
    Err(unsupported(
        "macOS lets apps neither read nor switch Focus (Do Not Disturb)",
    ))
}

pub fn security_notifications_enabled() -> io::Result<bool> {
    Err(unsupported("only on Windows"))
}

pub fn set_security_notifications(_enabled: bool) -> io::Result<()> {
    Err(unsupported("only on Windows"))
}

pub fn set_security_notifications_user(_enabled: bool) -> io::Result<()> {
    Err(unsupported("only on Windows"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP: &str = "AC Power:\n Sleep On Power Button 1\n standby              0\n \
                           displaysleep         0\n sleep                0\n disksleep            10\n";

    const LAPTOP: &str = "Battery Power:\n sleep                1\n displaysleep         2\n \
                          hibernatemode        3\nAC Power:\n sleep                1 (sleep prevented \
                          by sharingd, powerd)\n displaysleep         10\n hibernatemode        3\n \
                          hibernatefile        /var/vm/sleepimage\n";

    #[test]
    fn pmset_sections_are_parsed() {
        let desktop = parse_pmset(DESKTOP);
        assert_eq!(desktop.keys().copied().collect::<Vec<_>>(), [Source::Ac]);
        assert_eq!(desktop[&Source::Ac]["sleep"], "0");
        assert_eq!(desktop[&Source::Ac]["disksleep"], "10");
        assert!(!desktop[&Source::Ac].contains_key("hibernatemode"));

        let laptop = parse_pmset(LAPTOP);
        assert_eq!(
            laptop.keys().copied().collect::<Vec<_>>(),
            [Source::Ac, Source::Battery]
        );
        // Annotations after the value are dropped.
        assert_eq!(laptop[&Source::Ac]["sleep"], "1");
        assert_eq!(laptop[&Source::Battery]["displaysleep"], "2");
        assert_eq!(laptop[&Source::Ac]["hibernatefile"], "/var/vm/sleepimage");
    }

    #[test]
    fn firewall_states_are_parsed() {
        assert_eq!(
            parse_firewall_state("Firewall is disabled. (State = 0)"),
            Some(false)
        );
        assert_eq!(
            parse_firewall_state("Firewall is enabled. (State = 1)"),
            Some(true)
        );
        assert_eq!(
            parse_firewall_state(
                "Firewall is blocking all non-essential incoming connections. (State = 2)"
            ),
            Some(true)
        );
        assert_eq!(parse_firewall_state("Firewall is enabled."), None);
    }

    #[test]
    fn kcpassword_matches_the_macos_format() {
        // Every length pads to a multiple of 12 and keeps a NUL after the password.
        assert_eq!(kcpassword(b"").len(), 12);
        assert_eq!(kcpassword(b"abcdefghijk").len(), 12);
        assert_eq!(kcpassword(b"abcdefghijkl").len(), 24);
        let encoded = kcpassword(b"password");
        assert_eq!(
            encoded[..8],
            [0x0D, 0xE8, 0x21, 0x50, 0xA5, 0xD3, 0xAF, 0x8E]
        );
        // The NUL and the padding are XORed too, so they show the key.
        assert_eq!(encoded[8..11], [0xA3, 0xB9, 0x1F]);
        assert_eq!(encoded[11], 0x7D);
    }

    #[test]
    fn defaults_booleans_are_parsed() {
        for value in ["1", "YES", "true"] {
            assert!(parse_defaults_bool(value), "{value}");
        }
        for value in ["0", "NO", "false", ""] {
            assert!(!parse_defaults_bool(value), "{value}");
        }
    }

    fn pref(key: &'static str, current: Option<&str>, default: &'static str) -> Entry {
        Entry {
            store: Store::Pref(Pref::new("test", key, "-bool")),
            current: current.map(str::to_owned),
            off: "0",
            default,
        }
    }

    #[test]
    fn per_computer_preferences_use_current_host() {
        assert_eq!(
            SCREEN_SAVER_IDLE.args("read"),
            ["-currentHost", "read", "com.apple.screensaver", "idleTime"]
        );
        assert_eq!(
            SCREEN_SAVER_IDLE.id(),
            "defaults:currentHost:com.apple.screensaver/idleTime"
        );
        assert_eq!(
            AUTO_LOGIN_USER.args("delete"),
            [
                "delete",
                "/Library/Preferences/com.apple.loginwindow",
                "autoLoginUser"
            ]
        );
    }

    #[test]
    fn unset_preferences_mean_their_default() {
        assert!(entries_on(&[pref("a", None, "1")]));
        assert!(!entries_on(&[
            pref("a", None, "0"),
            pref("b", Some("0"), "1")
        ]));
        assert!(entries_on(&[
            pref("a", Some("0"), "1"),
            pref("b", Some("1"), "0")
        ]));
    }

    #[test]
    fn record_round_trips_and_keeps_only_values_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/os-settings-backup.json");
        let mut record = Record::load_from(path.clone()).unwrap();
        assert!(record.values.is_empty());
        // Already off: nothing is written, so this runs without `defaults`.
        let entries = [pref("set", Some("0"), "1")];
        set_entries_in(&mut record, &entries, false).unwrap();
        assert!(record.values.is_empty());

        record.values.insert("pmset:ac/sleep".into(), "30".into());
        record.save().unwrap();
        let loaded = Record::load_from(path.clone()).unwrap();
        assert_eq!(loaded.values["pmset:ac/sleep"], "30");
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644);
    }

    #[test]
    fn secret_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kcpassword");
        write_file(&path, b"x", 0o600).unwrap();
        write_file(&path, b"y", 0o600).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"y");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_unknown_account_has_no_password() {
        assert!(!pam::password_is_correct("gravekeeper-no-such-user", "x").unwrap());
    }

    #[test]
    fn the_current_account_has_a_name() {
        // SAFETY: getuid has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        assert!(!user_name(uid).unwrap().is_empty());
    }
}
