mod monitoring;
mod notify;
mod schedules;

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::{mpsc, oneshot};

use crate::api::{
    AppState, AppStatus, Command, Event, EventKind, EventLevel, ExitInfo, ExitReason, Snapshot,
};
use crate::config::{self, AppId, AppSpec, Config, RestartMode};
use crate::logfile::{LogWriter, LogWriterThread};
use crate::paths::{Paths, create_private_dir};
use crate::process::{ExitKind, FileSink, LogBuffer, LogLine, ManagedChild, Stream};
use crate::schedule::ScheduleId;
use crate::store::Store;
use crate::supervisor::{self, Decision, RestartTracker};

/// Called by the engine whenever the snapshot changes, so a front-end can redraw.
///
/// It must be cheap and must not block (e.g. `egui::Context::request_repaint`).
pub type Notifier = Arc<dyn Fn() + Send + Sync>;

/// Events kept in the snapshot (the database keeps more).
const RECENT_EVENTS: usize = 500;
/// Timestamped copies of the configuration kept in `<data>/backups`.
const CONFIG_BACKUPS: usize = 20;

#[derive(Debug)]
pub enum StartError {
    /// Another Gravekeeper is already supervising with the same data directory.
    AlreadyRunning,
    Io(io::Error),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => f.write_str("Gravekeeper is already running"),
            Self::Io(err) => write!(f, "cannot start the engine: {err}"),
        }
    }
}

impl std::error::Error for StartError {}

impl From<io::Error> for StartError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

struct Shared {
    snapshot: RwLock<Arc<Snapshot>>,
    logs: RwLock<HashMap<AppId, Arc<LogBuffer>>>,
    schedule_logs: RwLock<HashMap<ScheduleId, Arc<LogBuffer>>>,
}

/// Owner-side handle to the engine running on its own thread.
pub struct CoreHandle {
    commands: mpsc::UnboundedSender<Command>,
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// Held for the lifetime of the engine so only one instance supervises.
    _lock: File,
}

impl CoreHandle {
    /// Loads the configuration and starts the engine on a dedicated thread.
    pub fn start(paths: Paths, notifier: Notifier) -> Result<Self, StartError> {
        create_private_dir(&paths.data_dir)?;
        let lock = File::create(paths.lock_file())?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(StartError::AlreadyRunning),
            Err(std::fs::TryLockError::Error(err)) => return Err(err.into()),
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (commands, commands_rx) = mpsc::unbounded_channel();
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();

        let mut engine = Engine::new(paths, commands_rx, internal_tx, internal_rx, notifier);
        engine.boot();
        let shared = Arc::clone(&engine.shared);

        let thread = std::thread::Builder::new()
            .name("gravekeeper-core".into())
            .spawn(move || runtime.block_on(engine.run()))?;

        Ok(Self {
            commands,
            shared,
            thread: Mutex::new(Some(thread)),
            _lock: lock,
        })
    }

    pub fn send(&self, command: Command) {
        if self.commands.send(command).is_err() {
            tracing::warn!("engine is not running; command dropped");
        }
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(
            &self
                .shared
                .snapshot
                .read()
                .unwrap_or_else(|e| e.into_inner()),
        )
    }

    /// Output lines of an application with `seq >= from`, oldest first.
    pub fn logs_since(&self, id: &AppId, from: u64) -> Vec<LogLine> {
        let logs = self.shared.logs.read().unwrap_or_else(|e| e.into_inner());
        logs.get(id).map(|log| log.since(from)).unwrap_or_default()
    }

    /// Output of a schedule's `run` actions with `seq >= from`, oldest first.
    pub fn schedule_logs_since(&self, id: &ScheduleId, from: u64) -> Vec<LogLine> {
        let logs = self
            .shared
            .schedule_logs
            .read()
            .unwrap_or_else(|e| e.into_inner());
        logs.get(id).map(|log| log.since(from)).unwrap_or_default()
    }

    /// Stops all applications, then the engine, and waits for it to finish.
    pub fn shutdown(&self) {
        let Some(thread) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return;
        };
        self.send(Command::Shutdown);
        if thread.join().is_err() {
            tracing::error!("engine thread panicked");
        }
    }
}

impl Drop for CoreHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Messages from tasks and threads back to the engine.
enum Internal {
    Exited(Exited),
    ScheduleFinished(schedules::Finished),
    Webhook(notify::Outcome),
}

/// Message from a process task back to the engine.
struct Exited {
    id: AppId,
    generation: u64,
    result: io::Result<ProcessEnd>,
}

struct ProcessEnd {
    kind: ExitKind,
    /// Ended because the engine asked it to stop.
    requested: bool,
    forced: bool,
}

struct AppRuntime {
    state: AppState,
    paused: bool,
    since: SystemTime,
    next_attempt: Option<Instant>,
    last_exit: Option<ExitInfo>,
    tracker: RestartTracker,
    log: Arc<LogBuffer>,
    run: Option<Run>,
    generation: u64,
    /// Start again once the current stop completes.
    restart_after_stop: bool,
}

struct Run {
    pid: u32,
    started: Instant,
    stop: Option<oneshot::Sender<Duration>>,
}

impl AppRuntime {
    fn new(spec: &AppSpec, log: LogBuffer) -> Self {
        Self {
            state: if spec.enabled {
                AppState::Stopped
            } else {
                AppState::Disabled
            },
            paused: false,
            since: SystemTime::now(),
            next_attempt: None,
            last_exit: None,
            tracker: RestartTracker::default(),
            log: Arc::new(log),
            run: None,
            generation: 0,
            restart_after_stop: false,
        }
    }

    fn set_state(&mut self, state: AppState) {
        self.state = state;
        self.since = SystemTime::now();
        if !matches!(state, AppState::Waiting | AppState::Backoff) {
            self.next_attempt = None;
        }
    }

    fn note(&self, text: &str) {
        self.log.push(Stream::Meta, text.as_bytes());
    }
}

struct Engine {
    paths: Paths,
    commands: mpsc::UnboundedReceiver<Command>,
    internal_tx: mpsc::UnboundedSender<Internal>,
    internal_rx: mpsc::UnboundedReceiver<Internal>,
    notifier: Notifier,
    shared: Arc<Shared>,
    started_at: SystemTime,
    boot_time: Option<SystemTime>,
    config: Config,
    config_error: Option<String>,
    apps: HashMap<AppId, AppRuntime>,
    events: VecDeque<Event>,
    store: Option<Store>,
    /// Writes application and schedule output to files (if enabled at startup).
    log_files: Option<(LogWriter, LogWriterThread)>,
    schedules: schedules::State,
    monitoring: monitoring::State,
    notify: notify::State,
    /// Startup has finished; schedules planned from now on do not catch up missed runs.
    booted: bool,
}

impl Engine {
    fn new(
        paths: Paths,
        commands: mpsc::UnboundedReceiver<Command>,
        internal_tx: mpsc::UnboundedSender<Internal>,
        internal_rx: mpsc::UnboundedReceiver<Internal>,
        notifier: Notifier,
    ) -> Self {
        let started_at = SystemTime::now();
        let shared = Arc::new(Shared {
            snapshot: RwLock::new(Arc::new(Snapshot {
                started_at,
                config: Config::default(),
                config_path: paths.config_file.clone(),
                data_dir: paths.data_dir.clone(),
                config_error: None,
                boot_time: None,
                apps: Vec::new(),
                schedules: Vec::new(),
                metrics: Default::default(),
                events: Vec::new(),
                history_available: false,
            })),
            logs: RwLock::new(HashMap::new()),
            schedule_logs: RwLock::new(HashMap::new()),
        });
        let boot_time = match sysinfo::System::boot_time() {
            0 => None,
            secs => Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
        };
        Self {
            paths,
            commands,
            internal_tx,
            internal_rx,
            notifier,
            shared,
            started_at,
            boot_time,
            config: Config::default(),
            config_error: None,
            apps: HashMap::new(),
            events: VecDeque::new(),
            store: None,
            log_files: None,
            schedules: schedules::State::default(),
            monitoring: monitoring::State::default(),
            notify: notify::State::default(),
            booted: false,
        }
    }

    /// Opens history, loads the configuration and schedules autostarts. Runs on the
    /// caller's thread so the first snapshot is complete when `CoreHandle::start` returns.
    fn boot(&mut self) {
        match Store::open(&self.paths.history_db(), RECENT_EVENTS) {
            Ok((store, recalled)) => {
                self.events.extend(recalled.events);
                self.schedules.last_runs = recalled.schedule_runs;
                self.schedules.plans = recalled.schedule_plans;
                self.store = Some(store);
            }
            Err(err) => self.emit(
                EventLevel::Warning,
                EventKind::HistoryUnavailable,
                None,
                format!("event history is unavailable ({err}); events are kept in memory only"),
            ),
        }
        self.emit(
            EventLevel::Info,
            EventKind::EngineStarted,
            None,
            "Gravekeeper started".into(),
        );

        let loaded = config::load(&self.paths.config_file);
        let logging = match &loaded {
            Ok(config) => config.logging.clone(),
            Err(_) => Default::default(),
        };
        if logging.app_log_files {
            match LogWriter::start(logging.max_file_mb * 1024 * 1024, logging.keep_files) {
                Ok(writer) => self.log_files = Some(writer),
                Err(err) => tracing::warn!(%err, "cannot start the log file writer"),
            }
        }
        match loaded {
            Ok(config) => self.apply_config(config),
            Err(err) => self.config_failed(err.to_string()),
        }

        let now = Instant::now();
        let uptime = self
            .boot_time
            .and_then(|boot| SystemTime::now().duration_since(boot).ok());
        for spec in &self.config.apps {
            if spec.enabled && spec.autostart {
                // Without a known boot time, `boot_delay` cannot be honored; start as if the
                // computer had been up long enough.
                let boot_wait =
                    uptime.map_or(Duration::ZERO, |up| spec.boot_delay.saturating_sub(up));
                let app = self
                    .apps
                    .get_mut(&spec.id)
                    .expect("runtime exists for every app");
                app.set_state(AppState::Waiting);
                app.next_attempt = Some(now + spec.start_delay.max(boot_wait));
            }
        }
        self.booted = true;
        self.start_monitoring();
        self.publish();
    }

    async fn run(mut self) {
        self.start_notifications();
        loop {
            self.arm_wake();
            let deadline = [self.next_deadline(), self.schedule_deadline()]
                .into_iter()
                .flatten()
                .min();
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(Command::Shutdown) | None => break,
                    Some(command) => self.handle(command),
                },
                Some(message) = self.internal_rx.recv() => self.on_internal(message),
                Some(sample) = self.monitoring.recv() => self.on_sample(sample),
                () = sleep_until(deadline) => {
                    self.on_timers();
                    self.on_schedule_timers();
                }
            }
            self.publish();
        }
        self.stop_all().await;
        self.emit(
            EventLevel::Info,
            EventKind::EngineStopped,
            None,
            "Gravekeeper stopped".into(),
        );
        self.publish();
    }

    fn on_internal(&mut self, message: Internal) {
        match message {
            Internal::Exited(exited) => self.on_exited(exited),
            Internal::ScheduleFinished(finished) => self.on_schedule_finished(finished),
            Internal::Webhook(outcome) => self.on_webhook_outcome(outcome),
        }
    }

    fn handle(&mut self, command: Command) {
        tracing::debug!(?command, "command");
        match command {
            Command::Shutdown => {}
            Command::Start(id) => self.with_app(&id, |e, id| e.start_manually(id)),
            Command::Stop(id) => self.with_app(&id, |e, id| e.stop(id, false)),
            Command::Restart(id) => self.with_app(&id, |e, id| {
                if e.apps[id].state.is_active() {
                    e.stop(id, true);
                } else {
                    e.start_manually(id);
                }
            }),
            Command::SetPaused(id, paused) => self.with_app(&id, |e, id| {
                let app = e.apps.get_mut(id).expect("checked by with_app");
                app.paused = paused;
                if paused && matches!(app.state, AppState::Backoff | AppState::Waiting) {
                    app.set_state(AppState::Stopped);
                }
            }),
            Command::SaveApp(spec) => {
                let mut config = self.config.clone();
                match config.apps.iter_mut().find(|a| a.id == spec.id) {
                    Some(existing) => *existing = *spec,
                    None => config.apps.push(*spec),
                }
                self.save_config(config);
            }
            Command::RemoveApp(id) => {
                let mut config = self.config.clone();
                config.apps.retain(|a| a.id != id);
                self.save_config(config);
            }
            Command::MoveApp(id, offset) => {
                let mut config = self.config.clone();
                if let Some(from) = config.apps.iter().position(|a| a.id == id) {
                    let last = config.apps.len() as i64 - 1;
                    let to = (from as i64 + i64::from(offset)).clamp(0, last) as usize;
                    let app = config.apps.remove(from);
                    config.apps.insert(to, app);
                    self.save_config(config);
                }
            }
            Command::SaveGeneral(general) => {
                let config = Config {
                    general,
                    ..self.config.clone()
                };
                self.save_config(config);
            }
            Command::SaveMonitoring(monitoring) => {
                let config = Config {
                    monitoring,
                    ..self.config.clone()
                };
                self.save_config(config);
                self.start_monitoring();
            }
            Command::SaveLogging(logging) => {
                let config = Config {
                    logging,
                    ..self.config.clone()
                };
                self.save_config(config);
            }
            Command::SaveSchedule(spec) => {
                let mut config = self.config.clone();
                match config.schedules.iter_mut().find(|s| s.id == spec.id) {
                    Some(existing) => *existing = *spec,
                    None => config.schedules.push(*spec),
                }
                self.save_config(config);
            }
            Command::RemoveSchedule(id) => {
                let mut config = self.config.clone();
                config.schedules.retain(|s| s.id != id);
                self.save_config(config);
            }
            Command::RunSchedule(id) => self.run_schedule_now(&id),
            Command::SaveWebhook(spec) => {
                let mut config = self.config.clone();
                match config.webhooks.iter_mut().find(|w| w.id == spec.id) {
                    Some(existing) => *existing = *spec,
                    None => config.webhooks.push(*spec),
                }
                self.save_config(config);
            }
            Command::RemoveWebhook(id) => {
                let mut config = self.config.clone();
                config.webhooks.retain(|w| w.id != id);
                self.save_config(config);
            }
            Command::TestWebhook(id) => self.test_webhook(&id),
            Command::ReloadConfig => match config::load(&self.paths.config_file) {
                Ok(config) => {
                    self.config_error = None;
                    self.apply_config(config);
                    self.start_monitoring();
                    self.emit(
                        EventLevel::Info,
                        EventKind::ConfigSaved,
                        None,
                        "configuration reloaded".into(),
                    );
                }
                Err(err) => self.config_failed(err.to_string()),
            },
        }
    }

    fn with_app(&mut self, id: &AppId, f: impl FnOnce(&mut Self, &AppId)) {
        if self.apps.contains_key(id) {
            f(self, id);
        } else {
            self.reject(format!("unknown application {id}"));
        }
    }

    fn config_failed(&mut self, message: String) {
        self.emit(
            EventLevel::Error,
            EventKind::ConfigError,
            None,
            message.clone(),
        );
        self.config_error = Some(message);
    }

    fn reject(&mut self, message: String) {
        self.emit(
            EventLevel::Warning,
            EventKind::CommandRejected,
            None,
            message,
        );
    }

    fn save_config(&mut self, config: Config) {
        if self.config_error.is_some() {
            self.reject(
                "the configuration file has errors; fix it and reload before making changes".into(),
            );
            return;
        }
        if let Err(err) = crate::logfile::backup(
            &self.paths.config_file,
            &self.paths.data_dir.join("backups"),
            CONFIG_BACKUPS,
        ) {
            tracing::warn!(%err, "cannot back up the configuration");
        }
        if let Err(err) = config::save(&self.paths.config_file, &config) {
            self.reject(format!("configuration not saved: {err}"));
            return;
        }
        self.apply_config(config);
        self.emit(
            EventLevel::Info,
            EventKind::ConfigSaved,
            None,
            "configuration saved".into(),
        );
    }

    /// Makes the runtime state match `config`: adds, disables and removes applications.
    fn apply_config(&mut self, config: Config) {
        let removed: Vec<AppId> = self
            .apps
            .keys()
            .filter(|id| !config.apps.iter().any(|a| &a.id == *id))
            .cloned()
            .collect();
        for id in removed {
            if let Some(mut app) = self.apps.remove(&id) {
                // The process task keeps running until the stop completes; its result is
                // ignored because the runtime is gone.
                if let Some(stop) = app.run.as_mut().and_then(|r| r.stop.take()) {
                    let timeout = self
                        .spec(&id)
                        .map_or(Duration::from_secs(10), |s| s.stop_timeout);
                    let _ = stop.send(timeout);
                }
            }
            self.write_logs().remove(&id);
        }

        for spec in &config.apps {
            let log_lines = config.general.log_lines;
            let log = self.new_log(log_lines, || format!("{}.log", spec.id));
            let app = self
                .apps
                .entry(spec.id.clone())
                .or_insert_with(|| AppRuntime::new(spec, log));
            self.shared
                .logs
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(spec.id.clone(), Arc::clone(&app.log));
            match (spec.enabled, app.state) {
                (true, AppState::Disabled) => app.set_state(AppState::Stopped),
                // Active ones are stopped below and become Disabled when they exit.
                (false, state) if !state.is_active() && state != AppState::Disabled => {
                    app.set_state(AppState::Disabled)
                }
                _ => {}
            }
        }
        let disabled_running: Vec<AppId> = config
            .apps
            .iter()
            .filter(|s| !s.enabled && self.apps[&s.id].state.is_active())
            .map(|s| s.id.clone())
            .collect();
        self.config = config;
        for id in disabled_running {
            self.stop(&id, false);
        }
        self.plan_schedules(!self.booted);
    }

    /// An output buffer, also written to `<data>/logs/<file>` when log files are enabled.
    fn new_log(&self, capacity: usize, file: impl FnOnce() -> String) -> LogBuffer {
        match &self.log_files {
            Some((writer, _)) => LogBuffer::with_file(
                capacity.max(1),
                FileSink {
                    writer: writer.clone(),
                    path: Arc::new(self.logs_dir().join(file())),
                },
            ),
            None => LogBuffer::new(capacity.max(1)),
        }
    }

    fn logs_dir(&self) -> PathBuf {
        self.paths.data_dir.join("logs")
    }

    fn write_logs(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<AppId, Arc<LogBuffer>>> {
        self.shared.logs.write().unwrap_or_else(|e| e.into_inner())
    }

    fn spec(&self, id: &AppId) -> Option<&AppSpec> {
        self.config.apps.iter().find(|a| &a.id == id)
    }

    fn start_manually(&mut self, id: &AppId) {
        let app = &self.apps[id];
        match app.state {
            AppState::Disabled => self.reject(format!("{} is disabled", self.name(id))),
            AppState::Starting | AppState::Running => {}
            AppState::Stopping => self.apps.get_mut(id).expect("exists").restart_after_stop = true,
            AppState::Stopped | AppState::Waiting | AppState::Backoff | AppState::GaveUp => {
                self.apps.get_mut(id).expect("exists").tracker.reset();
                self.spawn(id);
            }
        }
    }

    fn spawn(&mut self, id: &AppId) {
        let spec = self.spec(id).expect("runtime has a spec").clone();
        let app = self.apps.get_mut(id).expect("exists");
        match ManagedChild::spawn(&spec.process_spec(), Arc::clone(&app.log)) {
            Ok(child) => {
                app.generation += 1;
                let (stop_tx, stop_rx) = oneshot::channel();
                app.run = Some(Run {
                    pid: child.pid(),
                    started: Instant::now(),
                    stop: Some(stop_tx),
                });
                app.set_state(AppState::Starting);
                app.note(&format!("started (pid {})", child.pid()));
                tokio::spawn(watch(
                    id.clone(),
                    app.generation,
                    child,
                    stop_rx,
                    self.internal_tx.clone(),
                ));
                let message = format!("started (pid {})", app.run.as_ref().map_or(0, |r| r.pid));
                self.emit(EventLevel::Info, EventKind::AppStarted, Some(id), message);
            }
            Err(err) => {
                let reason = ExitReason::SpawnFailed(err.to_string());
                app.note(&reason.to_string());
                app.last_exit = Some(ExitInfo {
                    reason: reason.clone(),
                    at: SystemTime::now(),
                    crash_hint: false,
                });
                self.emit(
                    EventLevel::Error,
                    EventKind::AppStartFailed,
                    Some(id),
                    reason.to_string(),
                );
                self.after_failure(id, Duration::ZERO);
            }
        }
    }

    fn stop(&mut self, id: &AppId, restart_after: bool) {
        let timeout = self
            .spec(id)
            .map_or(Duration::from_secs(10), |s| s.stop_timeout);
        let app = self.apps.get_mut(id).expect("exists");
        app.restart_after_stop = restart_after;
        match app.state {
            AppState::Starting | AppState::Running => {
                if let Some(stop) = app.run.as_mut().and_then(|r| r.stop.take()) {
                    let _ = stop.send(timeout);
                }
                app.set_state(AppState::Stopping);
                app.note("stopping");
            }
            AppState::Waiting | AppState::Backoff | AppState::GaveUp => {
                app.set_state(AppState::Stopped)
            }
            AppState::Stopping | AppState::Stopped | AppState::Disabled => {}
        }
    }

    fn on_exited(&mut self, exited: Exited) {
        let Some(app) = self.apps.get_mut(&exited.id) else {
            return; // removed while stopping
        };
        if app.generation != exited.generation {
            return;
        }
        let id = exited.id;
        let ran_for = app
            .run
            .take()
            .map_or(Duration::ZERO, |r| r.started.elapsed());
        let end = match exited.result {
            Ok(end) => end,
            Err(err) => {
                tracing::error!(%err, app = %id, "lost track of process");
                ProcessEnd {
                    kind: ExitKind::Unknown,
                    requested: app.state == AppState::Stopping,
                    forced: false,
                }
            }
        };
        let reason = match (end.requested, end.forced, end.kind) {
            (true, true, _) => ExitReason::StopTimeoutKilled,
            (true, false, _) => ExitReason::UserStop,
            (false, _, ExitKind::Code(code)) => ExitReason::Exited(code),
            (false, _, ExitKind::Signal(sig)) => ExitReason::Signaled(sig),
            (false, _, ExitKind::Unknown) => ExitReason::Unknown,
        };
        let crash_hint = !end.requested && end.kind.looks_like_crash();
        app.note(&reason.to_string());
        app.last_exit = Some(ExitInfo {
            reason: reason.clone(),
            at: SystemTime::now(),
            crash_hint,
        });

        if end.requested {
            app.set_state(AppState::Stopped);
            let restart = std::mem::take(&mut app.restart_after_stop);
            let disabled = !self.spec(&id).is_some_and(|s| s.enabled);
            if disabled {
                self.apps
                    .get_mut(&id)
                    .expect("exists")
                    .set_state(AppState::Disabled);
            }
            let level = if end.forced {
                EventLevel::Warning
            } else {
                EventLevel::Info
            };
            self.emit(level, EventKind::AppStopped, Some(&id), reason.to_string());
            if restart && !disabled {
                self.start_manually(&id);
            }
            return;
        }

        let paused = app.paused;
        let spec = self.spec(&id).expect("runtime has a spec");
        let during_startup = ran_for < spec.startup_grace;
        let wants_restart = match spec.restart.mode {
            RestartMode::Never => false,
            RestartMode::Always => true,
            RestartMode::OnFailure => during_startup || !end.kind.is_success(),
        };
        let mut message = format!("{reason} after {}", format_duration(ran_for));
        if during_startup {
            message.push_str(" (during startup)");
        }
        if crash_hint {
            message.push_str(" (looks like a crash)");
        }
        let level = if end.kind.is_success() && !during_startup {
            EventLevel::Info
        } else {
            EventLevel::Warning
        };
        self.emit(level, EventKind::AppExited, Some(&id), message);

        if wants_restart && !paused {
            self.after_failure(&id, ran_for);
        } else {
            self.apps
                .get_mut(&id)
                .expect("exists")
                .set_state(AppState::Stopped);
        }
    }

    /// Schedules a restart or gives up, according to the restart policy.
    fn after_failure(&mut self, id: &AppId, ran_for: Duration) {
        let spec = self.spec(id).expect("runtime has a spec").clone();
        let app = self.apps.get_mut(id).expect("exists");
        if spec.restart.mode == RestartMode::Never || app.paused {
            app.set_state(AppState::Stopped);
            return;
        }
        let now = Instant::now();
        match app
            .tracker
            .on_failure(now, ran_for, &spec.restart, supervisor::jitter())
        {
            Decision::RetryAfter(delay) => {
                app.set_state(AppState::Backoff);
                app.next_attempt = Some(now + delay);
                let attempt = app.tracker.consecutive();
                app.note(&format!(
                    "restarting in {} (attempt {attempt})",
                    format_duration(delay)
                ));
                let message = format!(
                    "restarting in {} (attempt {attempt})",
                    format_duration(delay)
                );
                self.emit(
                    EventLevel::Info,
                    EventKind::RestartScheduled,
                    Some(id),
                    message,
                );
            }
            Decision::GiveUp => {
                app.set_state(AppState::GaveUp);
                app.note("gave up restarting");
                let message = format!(
                    "gave up after {} restarts within {}; start it manually",
                    spec.restart.max_restarts,
                    format_duration(spec.restart.window)
                );
                self.emit(
                    EventLevel::Error,
                    EventKind::RestartGaveUp,
                    Some(id),
                    message,
                );
            }
        }
    }

    /// The first application, in start order, that is still waiting for its automatic
    /// start. Applications below it wait for it, so automatic starts happen in order.
    fn waiting_head(&self) -> Option<&AppId> {
        self.config.apps.iter().map(|s| &s.id).find(|id| {
            self.apps
                .get(*id)
                .is_some_and(|a| a.state == AppState::Waiting)
        })
    }

    fn next_deadline(&self) -> Option<Instant> {
        let head = self.waiting_head();
        self.apps
            .iter()
            .filter_map(|(id, app)| match app.state {
                AppState::Waiting if Some(id) == head => app.next_attempt,
                AppState::Waiting => None,
                AppState::Backoff => app.next_attempt,
                AppState::Starting => {
                    let grace = self.spec(id)?.startup_grace;
                    app.run.as_ref().map(|r| r.started + grace)
                }
                _ => None,
            })
            .min()
    }

    fn on_timers(&mut self) {
        let now = Instant::now();
        let mut due = Vec::new();
        // Automatic starts in order: each one waits for those above it.
        while let Some(head) = self.waiting_head().cloned() {
            if !self.apps[&head].next_attempt.is_some_and(|t| t <= now) {
                break;
            }
            self.spawn(&head);
            if self.apps[&head].state == AppState::Waiting {
                break; // cannot happen, but never loop forever
            }
        }
        for (id, app) in &mut self.apps {
            match app.state {
                AppState::Backoff if app.next_attempt.is_some_and(|t| t <= now) => {
                    due.push(id.clone());
                }
                AppState::Starting => {
                    let grace = self
                        .config
                        .apps
                        .iter()
                        .find(|a| &a.id == id)
                        .map_or(Duration::ZERO, |s| s.startup_grace);
                    if app.run.as_ref().is_some_and(|r| r.started + grace <= now) {
                        // Keep `since` at the process start so it shows the uptime.
                        app.state = AppState::Running;
                    }
                }
                _ => {}
            }
        }
        for id in due {
            self.spawn(&id);
        }
    }

    /// Stops every running application and waits (bounded) for them to exit.
    async fn stop_all(&mut self) {
        let active: Vec<AppId> = self
            .apps
            .iter()
            .filter(|(_, a)| a.state.is_active())
            .map(|(id, _)| id.clone())
            .collect();
        if active.is_empty() {
            return;
        }
        let longest = active
            .iter()
            .filter_map(|id| self.spec(id).map(|s| s.stop_timeout))
            .max()
            .unwrap_or_default();
        for id in &active {
            self.stop(id, false);
        }
        let deadline = tokio::time::Instant::now() + longest + Duration::from_secs(5);
        while self.apps.values().any(|a| a.state.is_active()) {
            match tokio::time::timeout_at(deadline, self.internal_rx.recv()).await {
                Ok(Some(message)) => self.on_internal(message),
                Ok(None) | Err(_) => {
                    tracing::warn!("some applications did not stop in time; killing them");
                    break;
                }
            }
        }
    }

    fn name(&self, id: &AppId) -> String {
        self.spec(id)
            .map_or_else(|| id.to_string(), |s| s.name.clone())
    }

    fn emit(&mut self, level: EventLevel, kind: EventKind, app: Option<&AppId>, message: String) {
        let app_name = app.map(|id| self.name(id));
        match level {
            EventLevel::Info => tracing::info!(app = app_name.as_deref(), "{message}"),
            EventLevel::Warning => tracing::warn!(app = app_name.as_deref(), "{message}"),
            EventLevel::Error => tracing::error!(app = app_name.as_deref(), "{message}"),
        }
        let event = Event {
            at: SystemTime::now(),
            level,
            kind,
            app_id: app.cloned(),
            app_name,
            message,
        };
        if let Some(store) = &self.store {
            store.record(event.clone());
        }
        self.notify(&event);
        if self.events.len() == RECENT_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    fn publish(&self) {
        let now = Instant::now();
        let wall = SystemTime::now();
        let apps = self
            .config
            .apps
            .iter()
            .filter_map(|spec| {
                let app = self.apps.get(&spec.id)?;
                Some(AppStatus {
                    id: spec.id.clone(),
                    name: spec.name.clone(),
                    state: app.state,
                    paused: app.paused,
                    pid: app.run.as_ref().map(|r| r.pid),
                    since: app.since,
                    next_attempt: app
                        .next_attempt
                        .map(|t| wall + t.saturating_duration_since(now)),
                    last_exit: app.last_exit.clone(),
                    failures: app.tracker.consecutive(),
                    usage: app.run.as_ref().and_then(|r| self.monitoring.usage(r.pid)),
                })
            })
            .collect();
        self.monitoring.set_pids(
            self.apps
                .values()
                .filter_map(|a| a.run.as_ref().map(|r| r.pid)),
        );
        let snapshot = Snapshot {
            started_at: self.started_at,
            config: self.config.clone(),
            config_path: self.paths.config_file.clone(),
            data_dir: self.paths.data_dir.clone(),
            config_error: self.config_error.clone(),
            boot_time: self.boot_time,
            apps,
            schedules: self.schedule_statuses(),
            metrics: self.monitoring.metrics(),
            events: self.events.iter().cloned().collect(),
            history_available: self.store.is_some(),
        };
        *self
            .shared
            .snapshot
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Arc::new(snapshot);
        (self.notifier)();
    }
}

/// Owns a running child: reports its exit, or stops it when asked.
async fn watch(
    id: AppId,
    generation: u64,
    mut child: ManagedChild,
    stop: oneshot::Receiver<Duration>,
    tx: mpsc::UnboundedSender<Internal>,
) {
    enum Outcome {
        Exited(io::Result<ExitKind>),
        StopRequested(Duration),
    }
    let outcome = tokio::select! {
        exit = child.wait() => Outcome::Exited(exit),
        Ok(timeout) = stop => Outcome::StopRequested(timeout),
    };
    let result = match outcome {
        Outcome::Exited(exit) => {
            child.kill_remaining();
            exit.map(|kind| ProcessEnd {
                kind,
                requested: false,
                forced: false,
            })
        }
        Outcome::StopRequested(timeout) => child.stop(timeout).await.map(|o| ProcessEnd {
            kind: o.exit,
            requested: true,
            forced: o.forced,
        }),
    };
    let _ = tx.send(Internal::Exited(Exited {
        id,
        generation,
        result,
    }));
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => std::future::pending().await,
    }
}

pub(crate) fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0 => format!("{}ms", d.as_millis()),
        1..60 => format!("{s}s"),
        60..3600 if s.is_multiple_of(60) => format!("{}m", s / 60),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, s / 60 % 60),
    }
}
