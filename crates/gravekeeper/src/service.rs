//! Registering the headless supervisor as a per-user OS service.
//!
//! This is separate from "launch at login" (`autostart.rs`), which starts the GUI in the
//! desktop session. A service runs `gravekeeper --headless` without a window, is restarted
//! if it fails, and (on Linux, with lingering enabled) can run without anyone logged in.
//! Only one of the two can supervise at a time: the second instance finds the first one's
//! lock and exits.

use std::process::ExitCode;

use crate::cli::ServiceCommand;

pub fn run(command: ServiceCommand) -> ExitCode {
    let result = match command {
        ServiceCommand::Install => imp::install(),
        ServiceCommand::Uninstall => imp::uninstall(),
        ServiceCommand::Status => imp::status(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn current_exe() -> Result<std::path::PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("cannot locate this program: {e}"))
}

/// Runs a command, failing with its stderr.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_tool(program: &str, args: &[&str]) -> Result<std::process::Output, String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(format!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// A systemd user unit in `~/.config/systemd/user`.
#[cfg(target_os = "linux")]
mod imp {
    use std::path::{Path, PathBuf};

    use super::{current_exe, run_tool};

    const UNIT: &str = "gravekeeper.service";

    fn unit_path() -> Result<PathBuf, String> {
        gravekeeper_core::paths::config_base_dir()
            .map(|d| d.join("systemd/user").join(UNIT))
            .ok_or_else(|| "cannot determine the home directory".into())
    }

    fn systemctl(args: &[&str]) -> Result<std::process::Output, String> {
        let mut all = vec!["--user"];
        all.extend_from_slice(args);
        run_tool("systemctl", &all)
    }

    pub fn install() -> Result<(), String> {
        let path = unit_path()?;
        if crate::autostart::is_enabled().unwrap_or(false) {
            println!(
                "Note: \"Launch at login\" is also on. Whichever starts first supervises; \
                 consider turning it off in Settings."
            );
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        std::fs::write(&path, unit(&current_exe()?))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", UNIT])?;
        println!("Installed and started {UNIT} ({}).", path.display());
        if !lingering() {
            println!(
                "It runs while you are logged in. To keep it running after logout and start it \
                 at boot, run: loginctl enable-linger {}",
                std::env::var("USER").unwrap_or_else(|_| "$USER".into())
            );
        }
        Ok(())
    }

    pub fn uninstall() -> Result<(), String> {
        let path = unit_path()?;
        if !path.exists() {
            println!("{UNIT} is not installed.");
            return Ok(());
        }
        // Stopping lets Gravekeeper stop its applications first.
        let _ = systemctl(&["disable", "--now", UNIT]);
        std::fs::remove_file(&path)
            .map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
        systemctl(&["daemon-reload"])?;
        println!("Stopped and removed {UNIT}.");
        Ok(())
    }

    pub fn status() -> Result<(), String> {
        let path = unit_path()?;
        if !path.exists() {
            println!("not installed");
            return Ok(());
        }
        // `is-active` exits non-zero when inactive, so read its output instead.
        let active = std::process::Command::new("systemctl")
            .args(["--user", "is-active", UNIT])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_else(|_| "unknown".into());
        println!("installed ({}), {active}", path.display());
        Ok(())
    }

    fn lingering() -> bool {
        let user = std::env::var("USER").unwrap_or_default();
        run_tool("loginctl", &["show-user", &user, "-p", "Linger"])
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "Linger=yes")
            .unwrap_or(false)
    }

    pub(super) fn unit(exe: &Path) -> String {
        format!(
            "[Unit]\n\
             Description=Gravekeeper application supervisor\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={} --headless\n\
             # `systemctl --user reload gravekeeper` re-reads the configuration.\n\
             ExecReload=/bin/kill -HUP $MAINPID\n\
             Restart=on-failure\n\
             RestartSec=5\n\
             # Only Gravekeeper gets SIGTERM; it stops its applications itself (each within its\n\
             # stop timeout) before systemd kills anything left.\n\
             KillMode=mixed\n\
             TimeoutStopSec=120\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            quote(&exe.to_string_lossy())
        )
    }

    /// Quotes a word for systemd `ExecStart` (systemd.syntax, systemd.service "Command lines").
    pub(super) fn quote(word: &str) -> String {
        let mut out = String::from('"');
        for c in word.chars() {
            match c {
                '"' | '\\' => {
                    out.push('\\');
                    out.push(c);
                }
                '%' => out.push_str("%%"),
                '$' => out.push_str("$$"),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    }
}

/// A LaunchAgent in `~/Library/LaunchAgents`, kept alive by launchd.
#[cfg(target_os = "macos")]
mod imp {
    use std::path::{Path, PathBuf};

    use super::{current_exe, run_tool};

    const LABEL: &str = "com.gravekeeper.supervisor";

    fn agent_path() -> Result<PathBuf, String> {
        gravekeeper_core::paths::home_dir()
            .map(|h| h.join(format!("Library/LaunchAgents/{LABEL}.plist")))
            .ok_or_else(|| "cannot determine the home directory".into())
    }

    fn domain() -> String {
        // SAFETY: getuid has no preconditions and cannot fail.
        format!("gui/{}", unsafe { libc::getuid() })
    }

    pub fn install() -> Result<(), String> {
        let path = agent_path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        std::fs::write(&path, plist(&current_exe()?))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        // Replace a previously loaded version, if any.
        let _ = run_tool(
            "launchctl",
            &["bootout", &domain(), &path.to_string_lossy()],
        );
        run_tool(
            "launchctl",
            &["bootstrap", &domain(), &path.to_string_lossy()],
        )?;
        println!("Installed and started {LABEL} ({}).", path.display());
        Ok(())
    }

    pub fn uninstall() -> Result<(), String> {
        let path = agent_path()?;
        if !path.exists() {
            println!("{LABEL} is not installed.");
            return Ok(());
        }
        let _ = run_tool(
            "launchctl",
            &["bootout", &domain(), &path.to_string_lossy()],
        );
        std::fs::remove_file(&path)
            .map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
        println!("Stopped and removed {LABEL}.");
        Ok(())
    }

    pub fn status() -> Result<(), String> {
        let path = agent_path()?;
        if !path.exists() {
            println!("not installed");
            return Ok(());
        }
        let loaded = run_tool("launchctl", &["print", &format!("{}/{LABEL}", domain())]).is_ok();
        println!(
            "installed ({}), {}",
            path.display(),
            if loaded { "loaded" } else { "not loaded" }
        );
        Ok(())
    }

    fn plist(exe: &Path) -> String {
        let exe = exe
            .to_string_lossy()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>--headless</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ExitTimeOut</key>
    <integer>120</integer>
    <key>ProcessType</key>
    <string>Background</string>
</dict>
</plist>
"#
        )
    }
}

/// Not implemented yet: a Windows service needs a service control handler and runs in
/// session 0, away from the user's desktop.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    const MESSAGE: &str = "Running as a service is not supported on this platform yet. \
         Use \"Launch at login\" in Settings instead.";

    pub fn install() -> Result<(), String> {
        Err(MESSAGE.into())
    }

    pub fn uninstall() -> Result<(), String> {
        Err(MESSAGE.into())
    }

    pub fn status() -> Result<(), String> {
        println!("not supported on this platform");
        Ok(())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::imp::{quote, unit};

    #[test]
    fn exec_start_is_quoted() {
        assert_eq!(quote("/opt/my app/gk"), "\"/opt/my app/gk\"");
        assert_eq!(quote(r#"a"b\c%d$e"#), r#""a\"b\\c%%d$$e""#);
    }

    #[test]
    fn unit_runs_headless() {
        let unit = unit(std::path::Path::new("/usr/bin/gravekeeper"));
        assert!(unit.contains("ExecStart=\"/usr/bin/gravekeeper\" --headless\n"));
        assert!(unit.contains("KillMode=mixed"));
        assert!(unit.contains("ExecReload=/bin/kill -HUP $MAINPID\n"));
    }
}
