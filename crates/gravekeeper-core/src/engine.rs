use std::io;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::mpsc;

use crate::api::{Command, Snapshot};

/// Called by the engine whenever the snapshot changes, so a front-end can redraw.
///
/// It must be cheap and must not block (e.g. `egui::Context::request_repaint`).
pub type Notifier = Arc<dyn Fn() + Send + Sync>;

const TICK: Duration = Duration::from_secs(1);

/// Owner-side handle to the engine running on its own thread.
pub struct CoreHandle {
    commands: mpsc::UnboundedSender<Command>,
    snapshot: Arc<RwLock<Snapshot>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl CoreHandle {
    /// Starts the engine on a dedicated thread with its own Tokio runtime.
    pub fn start(notifier: Notifier) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (commands, rx) = mpsc::unbounded_channel();
        let started_at = SystemTime::now();
        let snapshot = Arc::new(RwLock::new(Snapshot {
            started_at,
            uptime: Duration::ZERO,
        }));

        let engine = Engine {
            commands: rx,
            snapshot: Arc::clone(&snapshot),
            notifier,
            started: Instant::now(),
        };
        let thread = std::thread::Builder::new()
            .name("gravekeeper-core".into())
            .spawn(move || runtime.block_on(engine.run()))?;

        Ok(Self {
            commands,
            snapshot,
            thread: Mutex::new(Some(thread)),
        })
    }

    pub fn send(&self, command: Command) {
        if self.commands.send(command).is_err() {
            tracing::warn!("engine is not running; command dropped");
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        read_lock(&self.snapshot).clone()
    }

    /// Asks the engine to stop and waits for its thread to finish.
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

struct Engine {
    commands: mpsc::UnboundedReceiver<Command>,
    snapshot: Arc<RwLock<Snapshot>>,
    notifier: Notifier,
    started: Instant,
}

impl Engine {
    async fn run(mut self) {
        tracing::info!("engine started");
        let mut tick = tokio::time::interval(TICK);
        loop {
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(Command::Shutdown) | None => break,
                },
                _ = tick.tick() => self.update_snapshot(),
            }
        }
        tracing::info!("engine stopped");
    }

    fn update_snapshot(&self) {
        let mut snapshot = self.snapshot.write().unwrap_or_else(|e| e.into_inner());
        snapshot.uptime = self.started.elapsed();
        drop(snapshot);
        (self.notifier)();
    }
}

fn read_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn notifies_and_shuts_down() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let core = CoreHandle::start(Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        }))
        .unwrap();

        // The first interval tick fires immediately.
        let deadline = Instant::now() + Duration::from_secs(5);
        while calls.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(calls.load(Ordering::Relaxed) > 0);

        core.shutdown();
        // A second shutdown (and the one in Drop) must be harmless.
        core.shutdown();
    }
}
