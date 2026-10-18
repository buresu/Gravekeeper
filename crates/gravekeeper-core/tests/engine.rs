//! End-to-end tests of the engine with real processes (the `gk-test-helper` binary).

use std::sync::Arc;
use std::time::{Duration, Instant};

use gravekeeper_core::api::{AppState, EventKind, ExitReason};
use gravekeeper_core::config::{self, AppId, AppSpec, Config, RestartMode};
use gravekeeper_core::paths::Paths;
use gravekeeper_core::process::Stream;
use gravekeeper_core::{Command, CoreHandle, Snapshot, StartError};

const HELPER: &str = env!("CARGO_BIN_EXE_gk-test-helper");
const TIMEOUT: Duration = Duration::from_secs(15);

fn helper_app(name: &str, args: &[&str]) -> AppSpec {
    let mut app = AppSpec::new(name, HELPER);
    app.args = args.iter().map(|a| a.to_string()).collect();
    app.startup_grace = Duration::ZERO;
    app.stop_timeout = Duration::from_secs(5);
    app
}

fn start(dir: &tempfile::TempDir) -> CoreHandle {
    CoreHandle::start(Paths::in_dir(dir.path()), Arc::new(|| {})).expect("engine starts")
}

fn start_with(apps: Vec<AppSpec>) -> (tempfile::TempDir, CoreHandle) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::in_dir(dir.path());
    config::save(
        &paths.config_file,
        &Config {
            apps,
            ..Config::default()
        },
    )
    .unwrap();
    let core = start(&dir);
    (dir, core)
}

fn wait_for(core: &CoreHandle, what: &str, pred: impl Fn(&Snapshot) -> bool) -> Arc<Snapshot> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let snapshot = core.snapshot();
        if pred(&snapshot) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; apps: {:#?}",
            snapshot.apps
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn state_is(id: &AppId, state: AppState) -> impl Fn(&Snapshot) -> bool + '_ {
    move |s| s.app(id).is_some_and(|a| a.state == state)
}

fn has_event(s: &Snapshot, id: &AppId, kind: EventKind) -> bool {
    s.events
        .iter()
        .any(|e| e.kind == kind && e.app_id.as_ref() == Some(id))
}

#[test]
fn autostarts_and_stops_on_request() {
    let mut app = helper_app("sleeper", &["sleep", "60"]);
    app.autostart = true;
    let id = app.id.clone();
    let (_dir, core) = start_with(vec![app]);

    let snapshot = wait_for(&core, "running", state_is(&id, AppState::Running));
    assert!(snapshot.app(&id).unwrap().pid.is_some());

    core.send(Command::Stop(id.clone()));
    let snapshot = wait_for(&core, "stopped", state_is(&id, AppState::Stopped));
    let status = snapshot.app(&id).unwrap();
    assert_eq!(
        status.last_exit.as_ref().unwrap().reason,
        ExitReason::UserStop
    );
    assert!(status.pid.is_none());
    assert!(has_event(&snapshot, &id, EventKind::AppStopped));
}

#[test]
fn start_delay_is_honored() {
    let mut app = helper_app("late", &["sleep", "60"]);
    app.autostart = true;
    app.start_delay = Duration::from_secs(1);
    let id = app.id.clone();
    let (_dir, core) = start_with(vec![app]);

    let waiting = core.snapshot();
    assert_eq!(waiting.app(&id).unwrap().state, AppState::Waiting);
    assert!(waiting.app(&id).unwrap().next_attempt.is_some());
    wait_for(&core, "running", state_is(&id, AppState::Running));
}

#[test]
fn crash_loop_backs_off_then_gives_up() {
    let mut app = helper_app("crasher", &["exit", "3"]);
    app.restart.max_restarts = 2;
    app.restart.backoff_initial = Duration::from_secs(1);
    let id = app.id.clone();
    let (_dir, core) = start_with(vec![app]);

    core.send(Command::Start(id.clone()));
    let snapshot = wait_for(&core, "gave up", state_is(&id, AppState::GaveUp));

    let starts = snapshot
        .events
        .iter()
        .filter(|e| e.kind == EventKind::AppStarted)
        .count();
    assert_eq!(starts, 3, "initial start + 2 restarts");
    assert!(has_event(&snapshot, &id, EventKind::RestartScheduled));
    assert!(has_event(&snapshot, &id, EventKind::RestartGaveUp));
    let status = snapshot.app(&id).unwrap();
    assert_eq!(
        status.last_exit.as_ref().unwrap().reason,
        ExitReason::Exited(3)
    );

    // A manual start resets the failure count.
    core.send(Command::Start(id.clone()));
    wait_for(&core, "restarting again", |s| {
        s.app(&id)
            .is_some_and(|a| a.state == AppState::Backoff && a.failures == 1)
    });
}

#[test]
fn clean_exit_is_not_restarted_on_failure_policy() {
    let mut app = helper_app("oneshot", &["exit", "0"]);
    app.restart.mode = RestartMode::OnFailure;
    app.startup_grace = Duration::ZERO;
    let id = app.id.clone();
    let (_dir, core) = start_with(vec![app]);

    core.send(Command::Start(id.clone()));
    wait_for(&core, "exit recorded", |s| {
        has_event(s, &id, EventKind::AppExited) && s.app(&id).unwrap().state == AppState::Stopped
    });
    // Longer than the 1 s backoff a restart would use.
    std::thread::sleep(Duration::from_millis(1500));
    let snapshot = core.snapshot();
    assert_eq!(snapshot.app(&id).unwrap().state, AppState::Stopped);
    assert!(!has_event(&snapshot, &id, EventKind::RestartScheduled));
}

#[test]
fn early_exit_counts_as_failed_start() {
    let mut app = helper_app("flaky", &["exit", "0"]);
    app.startup_grace = Duration::from_secs(5);
    app.restart.max_restarts = 0;
    let id = app.id.clone();
    let (_dir, core) = start_with(vec![app]);

    core.send(Command::Start(id.clone()));
    let snapshot = wait_for(&core, "gave up", state_is(&id, AppState::GaveUp));
    let exit = snapshot
        .events
        .iter()
        .find(|e| e.kind == EventKind::AppExited)
        .unwrap();
    assert!(exit.message.contains("during startup"), "{}", exit.message);
}

#[test]
fn paused_apps_are_not_restarted() {
    let app = helper_app("crasher", &["exit", "1"]);
    let id = app.id.clone();
    let (_dir, core) = start_with(vec![app]);

    core.send(Command::SetPaused(id.clone(), true));
    core.send(Command::Start(id.clone()));
    let snapshot = wait_for(&core, "exit recorded", |s| {
        has_event(s, &id, EventKind::AppExited) && s.app(&id).unwrap().state == AppState::Stopped
    });
    assert!(snapshot.app(&id).unwrap().paused);
    assert!(!has_event(&snapshot, &id, EventKind::RestartScheduled));
}

#[test]
fn restart_replaces_the_process() {
    let app = helper_app("sleeper", &["sleep", "60"]);
    let id = app.id.clone();
    let (_dir, core) = start_with(vec![app]);

    core.send(Command::Start(id.clone()));
    let first = wait_for(&core, "running", state_is(&id, AppState::Running))
        .app(&id)
        .unwrap()
        .pid;
    core.send(Command::Restart(id.clone()));
    wait_for(&core, "new pid", |s| {
        s.app(&id)
            .is_some_and(|a| a.state == AppState::Running && a.pid.is_some() && a.pid != first)
    });
}

#[test]
fn captures_output_with_lifecycle_notes() {
    let app = helper_app("talker", &["lines", "2"]);
    let id = app.id.clone();
    let (_dir, core) = start_with(vec![app]);

    core.send(Command::Start(id.clone()));
    wait_for(&core, "exit", |s| has_event(s, &id, EventKind::AppExited));
    let deadline = Instant::now() + TIMEOUT;
    let lines = loop {
        let lines = core.logs_since(&id, 0);
        if lines.iter().filter(|l| l.stream != Stream::Meta).count() == 4
            || Instant::now() > deadline
        {
            break lines;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let text: Vec<_> = lines.iter().map(|l| l.text.as_str()).collect();
    assert!(text[0].starts_with("started (pid "), "{text:?}");
    assert!(
        text.contains(&"out 1") && text.contains(&"err 1"),
        "{text:?}"
    );
    assert!(
        core.logs_since(&id, lines.last().unwrap().seq + 1)
            .is_empty()
    );
}

#[test]
fn spawn_failure_is_reported() {
    let mut app = AppSpec::new("ghost", "/definitely/not/a/program");
    app.restart.max_restarts = 0;
    let id = app.id.clone();
    let (_dir, core) = start_with(vec![app]);

    core.send(Command::Start(id.clone()));
    let snapshot = wait_for(&core, "gave up", state_is(&id, AppState::GaveUp));
    assert!(matches!(
        snapshot
            .app(&id)
            .unwrap()
            .last_exit
            .as_ref()
            .unwrap()
            .reason,
        ExitReason::SpawnFailed(_)
    ));
    assert!(has_event(&snapshot, &id, EventKind::AppStartFailed));
}

#[test]
fn edits_are_saved_and_applied() {
    let dir = tempfile::tempdir().unwrap();
    let core = start(&dir);
    let mut app = helper_app("new", &["sleep", "60"]);
    let id = app.id.clone();

    core.send(Command::SaveApp(Box::new(app.clone())));
    wait_for(&core, "app added", state_is(&id, AppState::Stopped));

    app.enabled = false;
    core.send(Command::SaveApp(Box::new(app.clone())));
    wait_for(&core, "disabled", state_is(&id, AppState::Disabled));

    let on_disk = config::load(&Paths::in_dir(dir.path()).config_file).unwrap();
    assert_eq!(on_disk.apps, vec![app]);

    core.send(Command::RemoveApp(id.clone()));
    wait_for(&core, "removed", |s| s.apps.is_empty());
}

#[test]
fn disabling_a_running_app_stops_it() {
    let mut app = helper_app("sleeper", &["sleep", "60"]);
    let id = app.id.clone();
    let (_dir, core) = start_with(vec![app.clone()]);

    core.send(Command::Start(id.clone()));
    wait_for(&core, "running", state_is(&id, AppState::Running));
    app.enabled = false;
    core.send(Command::SaveApp(Box::new(app)));
    wait_for(&core, "disabled", state_is(&id, AppState::Disabled));
}

#[test]
fn broken_config_is_reported_and_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::in_dir(dir.path());
    std::fs::write(
        &paths.config_file,
        "schema_version = 1\n[[app]]\nname = 'x'",
    )
    .unwrap();
    let core = start(&dir);

    let snapshot = core.snapshot();
    assert!(snapshot.config_error.is_some());
    core.send(Command::SaveApp(Box::new(helper_app(
        "new",
        &["sleep", "1"],
    ))));
    wait_for(&core, "rejection", |s| {
        s.events
            .iter()
            .any(|e| e.kind == EventKind::CommandRejected)
    });
    assert_eq!(
        std::fs::read_to_string(&paths.config_file).unwrap(),
        "schema_version = 1\n[[app]]\nname = 'x'"
    );

    // After fixing the file, a reload clears the error.
    std::fs::write(&paths.config_file, "schema_version = 1\n").unwrap();
    core.send(Command::ReloadConfig);
    wait_for(&core, "error cleared", |s| s.config_error.is_none());
}

#[test]
fn only_one_engine_per_data_directory() {
    let dir = tempfile::tempdir().unwrap();
    let _first = start(&dir);
    let second = CoreHandle::start(Paths::in_dir(dir.path()), Arc::new(|| {}));
    assert!(matches!(second, Err(StartError::AlreadyRunning)));
}

#[test]
fn history_survives_restart_of_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let core = start(&dir);
    assert!(core.snapshot().history_available);
    core.shutdown();
    drop(core);

    // Other tests fork concurrently; until a forked child execs it briefly shares the lock
    // file descriptor, so the lock can look taken for a moment.
    let deadline = Instant::now() + TIMEOUT;
    let core = loop {
        match CoreHandle::start(Paths::in_dir(dir.path()), Arc::new(|| {})) {
            Err(StartError::AlreadyRunning) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            result => break result.expect("engine restarts"),
        }
    };
    let kinds: Vec<_> = core.snapshot().events.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [
            EventKind::EngineStarted,
            EventKind::EngineStopped,
            EventKind::EngineStarted
        ]
    );
}

#[test]
fn shutdown_stops_running_apps() {
    let mut app = helper_app("sleeper", &["sleep", "60"]);
    app.autostart = true;
    let id = app.id.clone();
    let (_dir, core) = start_with(vec![app]);
    let pid = wait_for(&core, "running", state_is(&id, AppState::Running))
        .app(&id)
        .unwrap()
        .pid
        .unwrap();

    core.shutdown();
    let mut system = sysinfo::System::new();
    let pid = sysinfo::Pid::from_u32(pid);
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    assert!(
        system
            .process(pid)
            .is_none_or(|p| p.status() == sysinfo::ProcessStatus::Zombie)
    );
}
