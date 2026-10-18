//! Spawning and stopping supervised processes.
//!
//! Each child runs in its own process group (Unix) or Job Object (Windows) so that stopping it
//! also stops its descendants. stdout/stderr are drained continuously into a [`LogBuffer`] so a
//! chatty child never blocks on a full pipe.

mod output;

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use serde::{Deserialize, Serialize};
use tokio::io::BufReader;
use tokio::task::JoinHandle;

pub use output::{LogBuffer, LogLine, MAX_LINE_BYTES, Stream};

/// What to run. Arguments are passed directly to the program; no shell is involved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    /// Added to (or overriding) the inherited environment.
    pub env: Vec<(OsString, OsString)>,
}

impl ProcessSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }
}

/// How a process ended, as reported by the OS.
///
/// This alone does not tell *why* it ended (user stop, timeout, crash); the supervisor
/// combines it with its own intent to decide that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExitKind {
    Code(i32),
    /// Terminated by a signal (Unix only).
    Signal(i32),
    /// Neither a code nor a signal was available.
    Unknown,
}

impl ExitKind {
    pub fn from_status(status: ExitStatus) -> Self {
        if let Some(code) = status.code() {
            return Self::Code(code);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = status.signal() {
                return Self::Signal(signal);
            }
        }
        Self::Unknown
    }

    pub fn is_success(self) -> bool {
        self == Self::Code(0)
    }

    /// A heuristic: crash-like signals on Unix, NTSTATUS error codes (0xCxxxxxxx) on Windows.
    /// Programs can return such codes deliberately, so treat this as a hint only.
    pub fn looks_like_crash(self) -> bool {
        match self {
            #[cfg(unix)]
            Self::Signal(sig) => {
                const SIGILL: i32 = 4;
                const SIGABRT: i32 = 6;
                const SIGFPE: i32 = 8;
                const SIGSEGV: i32 = 11;
                #[cfg(target_os = "linux")]
                const SIGBUS: i32 = 7;
                #[cfg(not(target_os = "linux"))]
                const SIGBUS: i32 = 10;
                matches!(sig, SIGILL | SIGABRT | SIGFPE | SIGSEGV | SIGBUS)
            }
            #[cfg(windows)]
            Self::Code(code) => (code as u32) & 0xC000_0000 == 0xC000_0000,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopOutcome {
    pub exit: ExitKind,
    /// The process did not exit within the grace period and was killed.
    pub forced: bool,
}

/// A running child process and the tasks draining its output.
///
/// Dropping it kills the whole process tree.
pub struct ManagedChild {
    child: Box<dyn ChildWrapper>,
    pid: u32,
    exit: Option<ExitKind>,
    pumps: Vec<JoinHandle<()>>,
}

impl ManagedChild {
    /// Spawns `spec`. Must be called from within a Tokio runtime.
    pub fn spawn(spec: &ProcessSpec, log: Arc<LogBuffer>) -> io::Result<Self> {
        let mut command = CommandWrap::with_new(&spec.program, |cmd| {
            cmd.args(&spec.args)
                .envs(spec.env.iter().map(|(k, v)| (k, v)))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(cwd) = &spec.cwd {
                cmd.current_dir(cwd);
            }
        });
        #[cfg(unix)]
        command.wrap(process_wrap::tokio::ProcessGroup::leader());
        #[cfg(windows)]
        {
            command.wrap(process_wrap::tokio::JobObject);
            // Gravekeeper has no console, so each console child would otherwise open its own
            // console window. Output is captured through pipes regardless.
            command.wrap(process_wrap::tokio::CreationFlags(
                windows::Win32::System::Threading::CREATE_NO_WINDOW,
            ));
        }
        command.wrap(KillOnDrop);

        let mut child = command.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("spawned process has no pid"))?;

        let mut pumps = Vec::with_capacity(2);
        if let Some(stdout) = child.stdout().take() {
            let log = Arc::clone(&log);
            pumps.push(tokio::spawn(async move {
                output::pump_lines(BufReader::new(stdout), Stream::Stdout, &log).await;
            }));
        }
        if let Some(stderr) = child.stderr().take() {
            pumps.push(tokio::spawn(async move {
                output::pump_lines(BufReader::new(stderr), Stream::Stderr, &log).await;
            }));
        }

        tracing::debug!(pid, program = %spec.program.display(), "spawned");
        Ok(Self {
            child,
            pid,
            exit: None,
            pumps,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Waits for the process to exit. Cancel-safe; returns the cached result once exited.
    ///
    /// Platform difference: on Windows this also waits for every process left in the Job
    /// Object, while on Unix it returns when the direct child exits.
    pub async fn wait(&mut self) -> io::Result<ExitKind> {
        if let Some(exit) = self.exit {
            return Ok(exit);
        }
        let exit = ExitKind::from_status(self.child.wait().await?);
        self.exit = Some(exit);
        Ok(exit)
    }

    /// Asks the process tree to exit, then kills it if it is still running after `grace`.
    ///
    /// On Unix this sends SIGTERM to the process group. On Windows there is no graceful
    /// request yet, so the Job Object is terminated immediately.
    pub async fn stop(&mut self, grace: Duration) -> io::Result<StopOutcome> {
        if let Some(exit) = self.exit {
            return Ok(StopOutcome {
                exit,
                forced: false,
            });
        }

        #[cfg(unix)]
        {
            // SIGTERM is 15 on every Unix we target.
            const SIGTERM: i32 = 15;
            if let Err(err) = self.child.signal(SIGTERM) {
                tracing::debug!(pid = self.pid, %err, "SIGTERM failed; process may be gone");
            }
            if let Ok(exit) = tokio::time::timeout(grace, self.wait()).await {
                return Ok(StopOutcome {
                    exit: exit?,
                    forced: false,
                });
            }
        }
        #[cfg(not(unix))]
        let _ = grace;

        tracing::debug!(pid = self.pid, "killing process tree");
        self.child.start_kill()?;
        Ok(StopOutcome {
            exit: self.wait().await?,
            forced: true,
        })
    }

    /// Kills whatever is left of the process tree after the main process exited on its own
    /// (descendants that kept running). Best effort; does nothing if the tree is gone.
    pub fn kill_remaining(&mut self) {
        let _ = self.child.start_kill();
    }

    /// Waits until both output streams reach EOF, or `timeout` passes. EOF can be delayed
    /// indefinitely by descendants that inherited the pipes.
    pub async fn drain_output(&mut self, timeout: Duration) -> bool {
        let pumps = std::mem::take(&mut self.pumps);
        let all = async {
            for pump in pumps {
                let _ = pump.await;
            }
        };
        tokio::time::timeout(timeout, all).await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_kind_success() {
        assert!(ExitKind::Code(0).is_success());
        assert!(!ExitKind::Code(1).is_success());
        assert!(!ExitKind::Unknown.is_success());
    }

    #[cfg(unix)]
    #[test]
    fn crash_heuristic_unix() {
        assert!(ExitKind::Signal(11).looks_like_crash());
        assert!(!ExitKind::Signal(15).looks_like_crash());
        assert!(!ExitKind::Code(1).looks_like_crash());
    }

    #[cfg(windows)]
    #[test]
    fn crash_heuristic_windows() {
        assert!(ExitKind::Code(0xC000_0005_u32 as i32).looks_like_crash());
        assert!(!ExitKind::Code(1).looks_like_crash());
    }
}
