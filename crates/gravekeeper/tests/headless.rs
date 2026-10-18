//! Runs the real `gravekeeper --headless` binary and drives it with signals.
#![cfg(unix)]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_gravekeeper");
const TIMEOUT: Duration = Duration::from_secs(20);

const CONFIG: &str = r#"
schema_version = 2
[monitoring]
enabled = false
[[app]]
id = "sleeper"
name = "Sleeper"
program = "/bin/sleep"
args = ["600"]
autostart = true
startup_grace = 0
"#;

fn start(home: &Path, config: &str) -> Child {
    std::fs::write(home.join("config.toml"), config).unwrap();
    Command::new(BIN)
        .arg("--headless")
        .env("GRAVEKEEPER_HOME", home)
        .env("RUST_LOG", "info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start gravekeeper")
}

fn log(home: &Path) -> String {
    std::fs::read_to_string(home.join("logs/gravekeeper.log")).unwrap_or_default()
}

fn wait_for_log(home: &Path, needle: &str) -> String {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let log = log(home);
        if log.contains(needle) {
            return log;
        }
        assert!(Instant::now() < deadline, "{needle:?} not logged:\n{log}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The PID from the first `started (pid N)` line.
fn started_pid(log: &str) -> u32 {
    let rest = &log[log.find("started (pid ").expect("started") + 13..];
    rest[..rest.find(')').unwrap()].parse().unwrap()
}

fn signal(pid: u32, sig: &str) {
    let status = Command::new("kill")
        .args([&format!("-{sig}"), &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

fn alive(pid: u32) -> bool {
    // A zombie is not running, but `kill -0` still succeeds on it.
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|stat| !stat.contains(") Z "))
        .unwrap_or_else(|_| {
            Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
}

fn wait_until_gone(pid: u32) {
    let deadline = Instant::now() + TIMEOUT;
    while alive(pid) {
        assert!(Instant::now() < deadline, "process {pid} still running");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "gravekeeper did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn sigterm_stops_applications_and_exits_cleanly() {
    let home = tempfile::tempdir().unwrap();
    let mut gk = start(home.path(), CONFIG);
    let app = started_pid(&wait_for_log(home.path(), "started (pid "));

    signal(gk.id(), "TERM");
    assert!(wait_exit(&mut gk).success());
    wait_until_gone(app);
    let log = log(home.path());
    assert!(log.contains("Gravekeeper stopped"), "{log}");
}

#[test]
fn sighup_reloads_the_configuration() {
    let home = tempfile::tempdir().unwrap();
    let mut gk = start(home.path(), CONFIG);
    wait_for_log(home.path(), "started (pid ");

    let extra = format!(
        "{CONFIG}\n[[app]]\nid = \"second\"\nname = \"Second\"\nprogram = \"/bin/sleep\"\nargs = [\"600\"]\n"
    );
    std::fs::write(home.path().join("config.toml"), extra).unwrap();
    signal(gk.id(), "HUP");
    wait_for_log(home.path(), "configuration reloaded");

    signal(gk.id(), "TERM");
    assert!(wait_exit(&mut gk).success());
}

#[test]
fn a_second_instance_refuses_to_start() {
    let home = tempfile::tempdir().unwrap();
    let mut first = start(home.path(), CONFIG);
    wait_for_log(home.path(), "running headless");

    let second = Command::new(BIN)
        .arg("--headless")
        .env("GRAVEKEEPER_HOME", home.path())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(second.code(), Some(3));

    signal(first.id(), "TERM");
    wait_exit(&mut first);
}

/// If Gravekeeper itself is killed, the kernel terminates its direct children.
#[cfg(target_os = "linux")]
#[test]
fn supervised_processes_die_with_a_killed_supervisor() {
    let home = tempfile::tempdir().unwrap();
    let mut gk = start(home.path(), CONFIG);
    let app = started_pid(&wait_for_log(home.path(), "started (pid "));
    assert!(alive(app));

    signal(gk.id(), "KILL");
    wait_exit(&mut gk);
    wait_until_gone(app);
}

#[test]
fn cli_reports_usage_errors() {
    let out = Command::new(BIN).arg("--bogus").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"));
    let version = Command::new(BIN).arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("gravekeeper "));
}
