//! Restarting, shutting down and sleeping the computer for a schedule.
//!
//! The OS may accept a restart or shutdown and still not carry it out, so one that has not
//! happened is tried again. While one is on its way, applications are not started
//! automatically.

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use super::{Engine, Internal, format_duration};
use crate::api::{EventKind, EventLevel, PendingPower};
use crate::power::{Operation, Power};
use crate::schedule::ScheduleId;

const ATTEMPTS: u32 = 3;
/// Schedules leave a computer alone that started less than this long ago. This is longer
/// than [`crate::schedule::LATE`], so a run that comes due again after the restart it caused
/// (its time could not be saved, say) can never restart the computer in a loop.
pub(super) const MIN_UPTIME: Duration = Duration::from_secs(600);

pub(super) struct State {
    power: Arc<dyn Power>,
    pending: Option<Pending>,
    /// Asked to sleep, and not known to be awake again.
    sleeping: bool,
}

struct Pending {
    schedule: ScheduleId,
    /// Restart or shut down.
    operation: Operation,
    attempt: u32,
    /// When the computer goes down, once the OS has accepted it.
    at: Option<(Instant, SystemTime)>,
    /// When to try again, if this process is still there.
    check: Instant,
}

pub(super) struct Slept {
    schedule: ScheduleId,
    at: SystemTime,
    result: io::Result<()>,
}

impl State {
    pub fn new(power: Arc<dyn Power>) -> Self {
        Self {
            power,
            pending: None,
            sleeping: false,
        }
    }

    /// The computer is about to go down: applications must not be started automatically.
    pub fn holds_starts(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| p.at.is_some())
    }

    /// The countdown is over: applications that exit now were closed by the OS.
    pub fn under_way(&self) -> Option<Operation> {
        let pending = self.pending.as_ref()?;
        let (at, _) = pending.at?;
        (at <= Instant::now()).then_some(pending.operation)
    }

    pub fn pending_power(&self) -> Option<PendingPower> {
        let pending = self.pending.as_ref()?;
        Some(PendingPower {
            operation: pending.operation,
            at: pending.at?.1,
        })
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.pending.as_ref().map(|p| p.check)
    }
}

impl Engine {
    /// Why a schedule must leave the computer alone now.
    pub(super) fn too_soon_after_boot(&self) -> Option<String> {
        match self.computer.power.uptime() {
            Some(up) if up >= MIN_UPTIME => None,
            Some(up) => Some(format!(
                "the computer started only {} ago",
                format_duration(up)
            )),
            None => Some("it is not known how long the computer has been up".into()),
        }
    }

    /// Why the computer cannot be asked anything else now.
    fn busy(&self) -> Result<(), String> {
        match &self.computer.pending {
            Some(pending) => Err(format!(
                "the computer is already about to {}",
                pending.operation.verb()
            )),
            None => Ok(()),
        }
    }

    pub(super) fn shut_down_computer(
        &mut self,
        id: &ScheduleId,
        name: &str,
        operation: Operation,
    ) -> Outcome {
        self.busy()?;
        self.try_shutdown(id, name, operation, 1)
    }

    /// Puts the computer to sleep on a thread of its own, since the call may return only after
    /// it has woken up again. The outcome is reported then.
    pub(super) fn sleep_computer(&mut self, id: &ScheduleId, at: SystemTime) -> Result<(), String> {
        self.busy()?;
        if self.computer.sleeping {
            return Err("the computer is already being put to sleep".into());
        }
        // Whatever is to wake it must be set before it sleeps.
        self.arm_wake();
        let power = Arc::clone(&self.computer.power);
        let tx = self.internal_tx.clone();
        let schedule = id.clone();
        std::thread::Builder::new()
            .name("gravekeeper-sleep".into())
            .spawn(move || {
                let result = power.sleep();
                let _ = tx.send(Internal::Slept(Slept {
                    schedule,
                    at,
                    result,
                }));
            })
            .map_err(|err| format!("cannot put the computer to sleep: {err}"))?;
        self.computer.sleeping = true;
        Ok(())
    }

    pub(super) fn on_slept(&mut self, slept: Slept) {
        self.computer.sleeping = false;
        let outcome = match slept.result {
            Ok(()) => Ok("put the computer to sleep".into()),
            Err(err) => Err(format!("cannot put the computer to sleep: {err}")),
        };
        self.finish_schedule(&slept.schedule, slept.at, outcome);
    }

    fn try_shutdown(
        &mut self,
        id: &ScheduleId,
        name: &str,
        operation: Operation,
        attempt: u32,
    ) -> Outcome {
        let power = &self.computer.power;
        let (grace, settle) = (power.grace(), power.settle());
        // The OS refuses messages that are too long.
        let name: String = name.chars().take(100).collect();
        let restart = operation == Operation::Restart;
        let result = power.shut_down(restart, grace, &format!("Gravekeeper: {name}"));

        let now = Instant::now();
        let check = now + grace + settle;
        let pending = |at| Pending {
            schedule: id.clone(),
            operation,
            attempt,
            at,
            check,
        };
        let err = match result {
            Ok(()) => {
                self.computer.pending =
                    Some(pending(Some((now + grace, SystemTime::now() + grace))));
                return Ok(format!(
                    "the computer {} in {}",
                    operation.verb_s(),
                    format_duration(grace)
                ));
            }
            Err(err) => err,
        };
        let hopeless = matches!(
            err.kind(),
            io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
        );
        let next = if hopeless {
            String::new()
        } else if attempt < ATTEMPTS {
            self.computer.pending = Some(pending(None));
            format!("; trying again in {}", format_duration(grace + settle))
        } else {
            format!("; gave up after {ATTEMPTS} attempts")
        };
        Err(format!("cannot {}: {err}{next}", operation.describe()))
    }

    /// Still here: if the computer was to have gone down by now, it did not.
    pub(super) fn on_shutdown_timer(&mut self) {
        let now = Instant::now();
        let Some(pending) = self.computer.pending.take_if(|p| p.check <= now) else {
            return;
        };
        let id = pending.schedule;
        let Some(spec) = self.config.schedules.iter().find(|s| s.id == id) else {
            return; // removed meanwhile
        };
        let operation = pending.operation;
        if spec.action.operation() != Some(operation) {
            return; // changed meanwhile
        }
        let name = spec.name.clone();
        if pending.at.is_some() {
            if pending.attempt >= ATTEMPTS {
                let message = format!(
                    "the computer did not {}; gave up after {ATTEMPTS} attempts",
                    operation.verb()
                );
                self.finish_schedule(&id, SystemTime::now(), Err(message));
                return;
            }
            self.emit(
                EventLevel::Error,
                EventKind::ScheduleFailed,
                None,
                format!(
                    "{name}: the computer did not {}; trying again",
                    operation.verb()
                ),
            );
        }
        let outcome = self.try_shutdown(&id, &name, operation, pending.attempt + 1);
        self.finish_schedule(&id, SystemTime::now(), outcome);
    }

    pub(super) fn cancel_shutdown(&mut self) {
        let Some(pending) = self.computer.pending.take() else {
            self.reject("the computer is not about to restart or shut down".into());
            return;
        };
        if pending.at.is_some()
            && let Err(err) = self.computer.power.cancel()
        {
            self.computer.pending = Some(pending);
            self.reject(format!("cannot cancel it: {err}"));
            return;
        }
        let message = format!(
            "cancelled; the computer does not {}",
            pending.operation.verb()
        );
        self.finish_schedule(&pending.schedule, SystemTime::now(), Err(message));
    }
}

type Outcome = Result<String, String>;
