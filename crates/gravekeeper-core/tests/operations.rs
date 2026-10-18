//! End-to-end tests of the operational features: start order, schedules, webhooks, log
//! files, monitoring and configuration backups.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gravekeeper_core::api::{AppState, EventKind};
use gravekeeper_core::config::{self, AppSpec, Config};
use gravekeeper_core::paths::Paths;
use gravekeeper_core::schedule::{Action, MissedRun, ScheduleId, ScheduleSpec, When};
use gravekeeper_core::webhook::WebhookSpec;
use gravekeeper_core::{Command, CoreHandle, Snapshot};

const HELPER: &str = env!("CARGO_BIN_EXE_gk-test-helper");
const TIMEOUT: Duration = Duration::from_secs(20);

fn helper_app(name: &str, args: &[&str]) -> AppSpec {
    let mut app = AppSpec::new(name, HELPER);
    app.args = args.iter().map(|a| a.to_string()).collect();
    app.startup_grace = Duration::ZERO;
    app.stop_timeout = Duration::from_secs(5);
    app
}

fn run_schedule(name: &str, args: &[&str], timeout: Duration) -> ScheduleSpec {
    ScheduleSpec {
        id: ScheduleId::generate(),
        name: name.into(),
        enabled: true,
        when: When::Daily {
            time: "03:00".parse().unwrap(),
        },
        action: Action::Run {
            program: HELPER.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: None,
            timeout,
        },
        missed: MissedRun::Skip,
        time_zone: Some("UTC".into()),
    }
}

/// Monitoring is off unless a test turns it on, to keep tests quiet and fast.
fn quiet(config: Config) -> Config {
    let mut config = config;
    config.monitoring.enabled = false;
    config
}

fn start_with(config: Config) -> (tempfile::TempDir, CoreHandle) {
    let dir = tempfile::tempdir().unwrap();
    config::save(&Paths::in_dir(dir.path()).config_file, &config).unwrap();
    let core = start(&dir);
    (dir, core)
}

fn start(dir: &tempfile::TempDir) -> CoreHandle {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        // See engine.rs: a concurrently forked test child can briefly hold the lock.
        match CoreHandle::start(Paths::in_dir(dir.path()), Arc::new(|| {})) {
            Err(gravekeeper_core::StartError::AlreadyRunning) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            result => return result.expect("engine starts"),
        }
    }
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
            "timed out waiting for {what}; events: {:#?}",
            snapshot.events
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn has(s: &Snapshot, kind: EventKind) -> bool {
    s.events.iter().any(|e| e.kind == kind)
}

#[test]
fn autostarts_happen_in_list_order() {
    let mut first = helper_app("first", &["sleep", "60"]);
    first.autostart = true;
    first.start_delay = Duration::from_secs(1);
    let mut second = helper_app("second", &["sleep", "60"]);
    second.autostart = true; // no delay of its own, but it waits for `first`
    let (_dir, core) = start_with(quiet(Config {
        apps: vec![first, second],
        ..Config::default()
    }));

    let snapshot = wait_for(&core, "both running", |s| {
        s.apps.iter().all(|a| a.state == AppState::Running)
    });
    let started: Vec<_> = snapshot
        .events
        .iter()
        .filter(|e| e.kind == EventKind::AppStarted)
        .map(|e| e.app_name.clone().unwrap())
        .collect();
    assert_eq!(started, ["first", "second"]);
}

#[test]
fn apps_can_be_reordered() {
    let a = helper_app("a", &["sleep", "1"]);
    let b = helper_app("b", &["sleep", "1"]);
    let id = b.id.clone();
    let (dir, core) = start_with(quiet(Config {
        apps: vec![a, b],
        ..Config::default()
    }));
    core.send(Command::MoveApp(id, -1));
    wait_for(&core, "reordered", |s| s.apps[0].name == "b");
    let on_disk = config::load(&Paths::in_dir(dir.path()).config_file).unwrap();
    assert_eq!(on_disk.apps[0].name, "b");
    // Each save keeps a timestamped backup.
    assert!(
        std::fs::read_dir(dir.path().join("backups"))
            .unwrap()
            .count()
            >= 1
    );
}

#[test]
fn run_schedules_report_success_failure_and_timeout() {
    let ok = run_schedule("ok", &["exit", "0"], Duration::from_secs(30));
    let bad = run_schedule("bad", &["exit", "4"], Duration::from_secs(30));
    let slow = run_schedule("slow", &["sleep", "60"], Duration::from_secs(1));
    let ids = [ok.id.clone(), bad.id.clone(), slow.id.clone()];
    let (_dir, core) = start_with(quiet(Config {
        schedules: vec![ok, bad, slow],
        ..Config::default()
    }));
    let next = core.snapshot().schedules[0].next_run;
    assert!(next.is_some(), "a daily schedule has a next run");

    for id in &ids {
        core.send(Command::RunSchedule(id.clone()));
    }
    let snapshot = wait_for(&core, "all three finished", |s| {
        s.schedules
            .iter()
            .all(|r| r.last_run.is_some() && !r.running)
    });
    let summary = |i: usize| snapshot.schedules[i].last_run.clone().unwrap();
    assert_eq!(summary(0).success, Some(true), "{:?}", summary(0));
    assert!(summary(1).success == Some(false) && summary(1).summary.contains("code 4"));
    assert!(summary(2).success == Some(false) && summary(2).summary.contains("timed out"));
    // A manual run does not move the timetable.
    assert_eq!(snapshot.schedules[0].next_run, next);
    let output: Vec<_> = core
        .schedule_logs_since(&ids[2], 0)
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert!(output.iter().any(|l| l.contains("timed out")), "{output:?}");
}

#[test]
fn interval_schedules_control_applications() {
    let app = helper_app("target", &["sleep", "60"]);
    let schedule = ScheduleSpec {
        id: ScheduleId::generate(),
        name: "kick".into(),
        enabled: true,
        when: When::Interval {
            every: Duration::from_secs(1),
        },
        action: Action::Start {
            app: app.id.clone(),
        },
        missed: MissedRun::Skip,
        time_zone: None,
    };
    let id = app.id.clone();
    let (_dir, core) = start_with(quiet(Config {
        apps: vec![app],
        schedules: vec![schedule],
        ..Config::default()
    }));
    let snapshot = wait_for(&core, "started by the schedule", |s| {
        s.app(&id).is_some_and(|a| a.state == AppState::Running)
    });
    assert!(has(&snapshot, EventKind::ScheduleSucceeded));
}

#[test]
fn runs_missed_while_stopped_are_caught_up() {
    let mut schedule = run_schedule("nightly", &["exit", "0"], Duration::from_secs(30));
    schedule.missed = MissedRun::RunOnce;
    let id = schedule.id.clone();
    let (dir, core) = start_with(quiet(Config {
        schedules: vec![schedule],
        ..Config::default()
    }));
    core.send(Command::RunSchedule(id.clone()));
    wait_for(&core, "first run", |s| has(s, EventKind::ScheduleSucceeded));
    core.shutdown();
    drop(core);

    // Pretend the last run was three days ago.
    let db = rusqlite::Connection::open(dir.path().join("history.sqlite3")).unwrap();
    db.execute(
        "UPDATE schedule_runs SET last_run_ms = last_run_ms - 3 * 86400000",
        [],
    )
    .unwrap();
    drop(db);

    let core = start(&dir);
    let snapshot = wait_for(&core, "catch-up run", |s| {
        has(s, EventKind::ScheduleMissed)
            && s.events
                .iter()
                .filter(|e| e.kind == EventKind::ScheduleSucceeded)
                .count()
                == 2
    });
    let missed = snapshot
        .events
        .iter()
        .find(|e| e.kind == EventKind::ScheduleMissed)
        .unwrap();
    assert!(
        missed.message.contains("running it now"),
        "{}",
        missed.message
    );
}

/// A tiny HTTP server answering with the given status codes in turn (then 200), recording
/// request bodies.
fn http_server(statuses: Vec<u16>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/hook/secret-token",
        listener.local_addr().unwrap()
    );
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&bodies);
    std::thread::spawn(move || {
        let mut statuses = statuses.into_iter();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            recorded
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&body).into_owned());
            let status = statuses.next().unwrap_or(200);
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            );
        }
    });
    (url, bodies)
}

#[test]
fn webhooks_deliver_with_retries() {
    let (url, bodies) = http_server(vec![500]);
    let mut app = helper_app("crashy", &["exit", "3"]);
    app.restart.mode = config::RestartMode::Never;
    let id = app.id.clone();
    let (_dir, core) = start_with(quiet(Config {
        apps: vec![app],
        webhooks: vec![WebhookSpec::new("ops", url)],
        ..Config::default()
    }));
    core.send(Command::Start(id));

    let deadline = Instant::now() + TIMEOUT;
    while bodies.lock().unwrap().len() < 2 {
        assert!(Instant::now() < deadline, "no retry arrived");
        std::thread::sleep(Duration::from_millis(50));
    }
    let bodies = bodies.lock().unwrap();
    let body: serde_json::Value = serde_json::from_str(&bodies[1]).unwrap();
    assert_eq!(body["kind"], "app_exited");
    assert_eq!(body["app"], "crashy");
    assert_eq!(bodies[0], bodies[1], "the retry sends the same message");
}

#[test]
fn webhook_failures_are_reported_without_the_url() {
    let (url, _bodies) = http_server(vec![404]);
    let hook = WebhookSpec::new("broken", url);
    let id = hook.id.clone();
    let (_dir, core) = start_with(quiet(Config {
        webhooks: vec![hook],
        ..Config::default()
    }));
    core.send(Command::TestWebhook(id));
    let snapshot = wait_for(&core, "failure", |s| has(s, EventKind::WebhookFailed));
    let failure = snapshot
        .events
        .iter()
        .find(|e| e.kind == EventKind::WebhookFailed)
        .unwrap();
    assert!(failure.message.contains("404"), "{}", failure.message);
    assert!(
        !failure.message.contains("secret-token"),
        "{}",
        failure.message
    );
}

#[test]
fn test_messages_are_confirmed() {
    let (url, bodies) = http_server(vec![]);
    let mut hook = WebhookSpec::new("chat", url);
    hook.format = gravekeeper_core::webhook::PayloadFormat::Slack;
    let id = hook.id.clone();
    let (_dir, core) = start_with(quiet(Config {
        webhooks: vec![hook],
        ..Config::default()
    }));
    core.send(Command::TestWebhook(id));
    wait_for(&core, "delivered", |s| has(s, EventKind::WebhookDelivered));
    let body: serde_json::Value = serde_json::from_str(&bodies.lock().unwrap()[0]).unwrap();
    assert!(body["text"].as_str().unwrap().contains("test message"));
}

#[test]
fn application_output_is_written_to_log_files() {
    let app = helper_app("talker", &["lines", "2"]);
    let id = app.id.clone();
    let (dir, core) = start_with(quiet(Config {
        apps: vec![app],
        ..Config::default()
    }));
    core.send(Command::Start(id.clone()));
    wait_for(&core, "exit", |s| has(s, EventKind::AppExited));
    core.shutdown();
    drop(core);

    let log = std::fs::read_to_string(dir.path().join(format!("logs/{id}.log"))).unwrap();
    assert!(log.contains(" out out 1\n"), "{log}");
    assert!(log.contains(" err err 1\n"), "{log}");
    assert!(log.contains(" --- started (pid "), "{log}");
}

#[test]
fn monitoring_samples_and_alerts() {
    let mut config = Config::default();
    config.monitoring.interval = Duration::from_secs(1);
    config.monitoring.memory_alert = 1; // certainly exceeded
    config.monitoring.sustain = Duration::ZERO;
    let mut app = helper_app("sleeper", &["sleep", "60"]);
    app.autostart = true;
    let id = app.id.clone();
    config.apps = vec![app];
    let (_dir, core) = start_with(config);

    let snapshot = wait_for(&core, "alert and usage", |s| {
        has(s, EventKind::ResourceAlert)
            && s.metrics.cpu_percent.is_some()
            && s.app(&id).is_some_and(|a| a.usage.is_some())
    });
    assert!(snapshot.metrics.memory.is_some());
    assert!(!snapshot.metrics.active_alerts.is_empty());
    assert!(!snapshot.metrics.memory_history.is_empty());

    // Raising the threshold clears the alert.
    let mut monitoring = snapshot.config.monitoring.clone();
    monitoring.memory_alert = 100;
    core.send(Command::SaveMonitoring(monitoring));
    wait_for(&core, "recovery", |s| has(s, EventKind::ResourceRecovered));
}
