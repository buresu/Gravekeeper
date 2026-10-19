//! Launching the GUI when the user logs in.
//!
//! This is separate from running the supervisor as an OS service (planned for Phase 3): it
//! only starts Gravekeeper inside the user's desktop session, minimized to the tray.

use std::io;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::PathBuf;

const MINIMIZED_ARG: &str = "--minimized";

/// Whether Gravekeeper is registered to start at login, as recorded by the OS.
pub fn is_enabled() -> io::Result<bool> {
    imp::is_enabled()
}

/// Registers or unregisters the current executable to start at login.
pub fn set_enabled(enabled: bool) -> io::Result<()> {
    if enabled {
        imp::enable(&std::env::current_exe()?)
    } else {
        imp::disable()
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn require(dir: Option<PathBuf>) -> io::Result<PathBuf> {
    dir.ok_or_else(|| io::Error::other("cannot determine the home directory"))
}

/// XDG autostart entry in `~/.config/autostart`.
#[cfg(target_os = "linux")]
mod imp {
    use std::io;
    use std::path::{Path, PathBuf};

    fn entry_path() -> io::Result<PathBuf> {
        Ok(super::require(gravekeeper_core::paths::config_base_dir())?
            .join("autostart/gravekeeper.desktop"))
    }

    pub fn is_enabled() -> io::Result<bool> {
        match std::fs::read_to_string(entry_path()?) {
            Ok(text) => Ok(!text.lines().any(|l| l.trim() == "Hidden=true")),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(err),
        }
    }

    pub fn enable(exe: &Path) -> io::Result<()> {
        let path = entry_path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, desktop_entry(exe))
    }

    pub fn disable() -> io::Result<()> {
        match std::fs::remove_file(entry_path()?) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }

    pub(super) fn desktop_entry(exe: &Path) -> String {
        format!(
            "[Desktop Entry]\nType=Application\nName=Gravekeeper\nComment=Application supervisor\n\
             Exec={} {}\nIcon=gravekeeper\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
            quote_exec_arg(&exe.to_string_lossy()),
            super::MINIMIZED_ARG
        )
    }

    /// Quotes an argument for the `Exec` key (Desktop Entry Specification, "The Exec key").
    pub(super) fn quote_exec_arg(arg: &str) -> String {
        let mut out = String::from('"');
        for c in arg.chars() {
            match c {
                '"' | '`' | '$' | '\\' => {
                    out.push('\\');
                    out.push(c);
                }
                '%' => out.push_str("%%"),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    }
}

/// A per-user LaunchAgent in `~/Library/LaunchAgents`. Takes effect at the next login.
#[cfg(target_os = "macos")]
mod imp {
    use std::io;
    use std::path::{Path, PathBuf};

    const LABEL: &str = "com.gravekeeper.login";

    fn agent_path() -> io::Result<PathBuf> {
        Ok(super::require(gravekeeper_core::paths::home_dir())?
            .join(format!("Library/LaunchAgents/{LABEL}.plist")))
    }

    pub fn is_enabled() -> io::Result<bool> {
        Ok(agent_path()?.exists())
    }

    pub fn enable(exe: &Path) -> io::Result<()> {
        let path = agent_path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#,
            xml_escape(&exe.to_string_lossy()),
            super::MINIMIZED_ARG
        );
        std::fs::write(path, plist)
    }

    pub fn disable() -> io::Result<()> {
        match std::fs::remove_file(agent_path()?) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
}

/// A value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
///
/// Turning a startup app off in Task Manager or Settings keeps the `Run` value and records
/// the choice under `Explorer\StartupApproved\Run` instead, so both keys are consulted.
#[cfg(windows)]
mod imp {
    use std::io;
    use std::path::Path;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APPROVED_KEY: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
    const VALUE: &str = "Gravekeeper";

    fn to_io(err: impl std::fmt::Display) -> io::Error {
        io::Error::other(err.to_string())
    }

    pub fn is_enabled() -> io::Result<bool> {
        let Ok(key) = windows_registry::CURRENT_USER.open(RUN_KEY) else {
            return Ok(false);
        };
        Ok(key.get_string(VALUE).is_ok() && !disabled_by_user())
    }

    /// Whether the entry was turned off in Task Manager or Settings. The value is 12 bytes:
    /// a flag whose low bit marks it disabled (02 on, 03 off), then when that happened.
    fn disabled_by_user() -> bool {
        windows_registry::CURRENT_USER
            .open(APPROVED_KEY)
            .and_then(|key| key.get_bytes(VALUE))
            .is_ok_and(|bytes| bytes.first().is_some_and(|flag| flag & 1 == 1))
    }

    /// Removes the Task Manager / Settings choice; without it the entry counts as enabled.
    fn clear_approval() -> io::Result<()> {
        let Ok(key) = windows_registry::CURRENT_USER
            .options()
            .read()
            .write()
            .open(APPROVED_KEY)
        else {
            return Ok(());
        };
        if key.get_bytes(VALUE).is_ok() {
            key.remove_value(VALUE).map_err(to_io)?;
        }
        Ok(())
    }

    pub fn enable(exe: &Path) -> io::Result<()> {
        let key = windows_registry::CURRENT_USER
            .create(RUN_KEY)
            .map_err(to_io)?;
        let command = format!("\"{}\" {}", exe.display(), super::MINIMIZED_ARG);
        key.set_string(VALUE, command).map_err(to_io)?;
        clear_approval()
    }

    pub fn disable() -> io::Result<()> {
        let key = windows_registry::CURRENT_USER
            .create(RUN_KEY)
            .map_err(to_io)?;
        if key.get_string(VALUE).is_ok() {
            key.remove_value(VALUE).map_err(to_io)?;
        }
        clear_approval()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod imp {
    use std::io;
    use std::path::Path;

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "not supported on this platform")
    }

    pub fn is_enabled() -> io::Result<bool> {
        Ok(false)
    }

    pub fn enable(_exe: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn disable() -> io::Result<()> {
        Err(unsupported())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::imp::{desktop_entry, quote_exec_arg};

    #[test]
    fn exec_arguments_are_quoted() {
        assert_eq!(quote_exec_arg("/opt/my app/gk"), "\"/opt/my app/gk\"");
        assert_eq!(quote_exec_arg(r#"a"b$c\d%e"#), r#""a\"b\$c\\d%%e""#);
    }

    #[test]
    fn entry_starts_minimized() {
        let entry = desktop_entry(std::path::Path::new("/usr/bin/gravekeeper"));
        assert!(entry.contains("Exec=\"/usr/bin/gravekeeper\" --minimized\n"));
    }
}
