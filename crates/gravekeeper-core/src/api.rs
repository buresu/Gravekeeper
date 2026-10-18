//! Messages exchanged between the engine and its front-ends.
//!
//! Everything here is serializable so the same types can become the IPC protocol once the
//! engine runs as its own process (Phase 4).

use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::config::{AppId, AppSpec, Config, GeneralSettings};

/// A request from a front-end to the engine. Failures are reported as [`Event`]s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Stop all applications and terminate the engine.
    Shutdown,
    Start(AppId),
    Stop(AppId),
    Restart(AppId),
    /// While paused, exits are recorded but never trigger a restart.
    SetPaused(AppId, bool),
    /// Adds or replaces an application. Changes take effect at its next start.
    SaveApp(Box<AppSpec>),
    /// Stops the application if needed and removes it.
    RemoveApp(AppId),
    SaveGeneral(GeneralSettings),
    /// Re-reads the configuration file (after fixing it by hand, for example).
    ReloadConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppState {
    Disabled,
    Stopped,
    /// Waiting for its start delay before an automatic start.
    Waiting,
    /// Running, but not yet for the startup grace period.
    Starting,
    Running,
    Stopping,
    /// Waiting before the next restart attempt.
    Backoff,
    /// Restarted too often; needs a manual start.
    GaveUp,
}

impl AppState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "Disabled",
            Self::Stopped => "Stopped",
            Self::Waiting => "Waiting",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
            Self::Backoff => "Restarting",
            Self::GaveUp => "Gave up",
        }
    }

    /// A process exists (or is being stopped).
    pub fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
}

/// Why a process is no longer running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExitReason {
    /// Stopped on request and exited within the stop timeout.
    UserStop,
    /// Stopped on request but had to be killed after the stop timeout.
    StopTimeoutKilled,
    Exited(i32),
    Signaled(i32),
    Unknown,
    SpawnFailed(String),
}

impl fmt::Display for ExitReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserStop => f.write_str("stopped"),
            Self::StopTimeoutKilled => f.write_str("killed after stop timeout"),
            Self::Exited(code) => write!(f, "exited with code {code}"),
            Self::Signaled(sig) => write!(f, "terminated by signal {sig}"),
            Self::Unknown => f.write_str("exited (status unknown)"),
            Self::SpawnFailed(err) => write!(f, "failed to start: {err}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitInfo {
    pub reason: ExitReason,
    pub at: SystemTime,
    /// The exit status resembles a crash (see `ExitKind::looks_like_crash`). A hint only.
    pub crash_hint: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppStatus {
    pub id: AppId,
    pub name: String,
    pub state: AppState,
    pub paused: bool,
    pub pid: Option<u32>,
    /// When the current state was entered.
    pub since: SystemTime,
    /// When the next automatic start happens (in `Waiting` and `Backoff`).
    pub next_attempt: Option<SystemTime>,
    pub last_exit: Option<ExitInfo>,
    /// Consecutive failed runs since the last stable run or manual start.
    pub failures: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EventLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    EngineStarted,
    EngineStopped,
    AppStarted,
    AppStopped,
    /// Exited without being asked to.
    AppExited,
    AppStartFailed,
    RestartScheduled,
    RestartGaveUp,
    ConfigSaved,
    ConfigError,
    CommandRejected,
    HistoryUnavailable,
}

impl EventKind {
    const ALL: [Self; 12] = [
        Self::EngineStarted,
        Self::EngineStopped,
        Self::AppStarted,
        Self::AppStopped,
        Self::AppExited,
        Self::AppStartFailed,
        Self::RestartScheduled,
        Self::RestartGaveUp,
        Self::ConfigSaved,
        Self::ConfigError,
        Self::CommandRejected,
        Self::HistoryUnavailable,
    ];

    /// Stable name used in the history database.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EngineStarted => "engine_started",
            Self::EngineStopped => "engine_stopped",
            Self::AppStarted => "app_started",
            Self::AppStopped => "app_stopped",
            Self::AppExited => "app_exited",
            Self::AppStartFailed => "app_start_failed",
            Self::RestartScheduled => "restart_scheduled",
            Self::RestartGaveUp => "restart_gave_up",
            Self::ConfigSaved => "config_saved",
            Self::ConfigError => "config_error",
            Self::CommandRejected => "command_rejected",
            Self::HistoryUnavailable => "history_unavailable",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

impl EventLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [Self::Info, Self::Warning, Self::Error]
            .into_iter()
            .find(|l| l.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub at: SystemTime,
    pub level: EventLevel,
    pub kind: EventKind,
    pub app_id: Option<AppId>,
    pub app_name: Option<String>,
    pub message: String,
}

/// A read-only view of the engine state. Published as a whole after every change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub started_at: SystemTime,
    pub config: Config,
    pub config_path: PathBuf,
    /// Set when the configuration file could not be loaded. While set, the engine does not
    /// overwrite the file.
    pub config_error: Option<String>,
    /// In configuration order.
    pub apps: Vec<AppStatus>,
    /// Most recent events, oldest first.
    pub events: Vec<Event>,
    /// False if the history database could not be opened; events then live only in memory.
    pub history_available: bool,
}

impl Snapshot {
    pub fn app(&self, id: &AppId) -> Option<&AppStatus> {
        self.apps.iter().find(|a| &a.id == id)
    }

    pub fn spec(&self, id: &AppId) -> Option<&AppSpec> {
        self.config.apps.iter().find(|a| &a.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names_round_trip() {
        for kind in EventKind::ALL {
            assert_eq!(EventKind::parse(kind.as_str()), Some(kind));
        }
        for level in [EventLevel::Info, EventLevel::Warning, EventLevel::Error] {
            assert_eq!(EventLevel::parse(level.as_str()), Some(level));
        }
    }
}
