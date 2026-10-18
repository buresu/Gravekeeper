//! Running scheduled tasks.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use jiff::Timestamp;
use tokio::sync::mpsc;

use super::{Engine, Internal, format_duration};
use crate::api::{AppState, EventKind, EventLevel, ScheduleRun, ScheduleStatus};
use crate::process::{ExitKind, LogBuffer, ManagedChild, ProcessSpec, Stream};
use crate::schedule::{self, Action, MissedRun, ScheduleId, ScheduleSpec};

/// Re-check the wall clock at least this often, so sleep/resume and clock changes are
/// noticed even though timers run on the monotonic clock.
const RECHECK: Duration = Duration::from_secs(30);
/// Output lines kept in memory per schedule.
const LOG_LINES: usize = 1000;

#[derive(Default)]
pub(super) struct State {
    /// When each schedule last ran, as persisted in the history database.
    pub last_runs: HashMap<ScheduleId, SystemTime>,
    runtimes: HashMap<ScheduleId, Runtime>,
}

struct Runtime {
    spec: ScheduleSpec,
    next: Option<SystemTime>,
    last: Option<ScheduleRun>,
    running: Option<Running>,
    generation: u64,
    problem: Option<String>,
    log: Arc<LogBuffer>,
}

struct Running {
    generation: u64,
    started: Instant,
}

pub(super) struct Finished {
    id: ScheduleId,
    generation: u64,
    result: io::Result<ExitKind>,
    timed_out: bool,
}

fn to_ts(t: SystemTime) -> Option<Timestamp> {
    Timestamp::try_from(t).ok()
}

impl Engine {
    /// Brings schedule runtimes in line with the configuration. New and changed schedules
    /// are planned; at startup (`catch_up`) missed runs are detected.
    pub(super) fn plan_schedules(&mut self, catch_up: bool) {
        let specs = self.config.schedules.clone();
        self.schedules
            .runtimes
            .retain(|id, _| specs.iter().any(|s| &s.id == id));
        self.shared
            .schedule_logs
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|id, _| specs.iter().any(|s| &s.id == id));

        let now = SystemTime::now();
        for spec in specs {
            if self
                .schedules
                .runtimes
                .get(&spec.id)
                .is_some_and(|rt| rt.spec == spec)
            {
                continue;
            }
            if !self.schedules.runtimes.contains_key(&spec.id) {
                let log = Arc::new(self.new_log(LOG_LINES, || format!("schedule-{}.log", spec.id)));
                self.shared
                    .schedule_logs
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(spec.id.clone(), Arc::clone(&log));
                self.schedules.runtimes.insert(
                    spec.id.clone(),
                    Runtime {
                        spec: spec.clone(),
                        next: None,
                        last: None,
                        running: None,
                        generation: 0,
                        problem: None,
                        log,
                    },
                );
            }
            self.plan_one(&spec, now, catch_up);
        }
    }

    fn plan_one(&mut self, spec: &ScheduleSpec, now: SystemTime, catch_up: bool) {
        let last_run = self.schedules.last_runs.get(&spec.id).copied();
        let boot = self.boot_time.and_then(to_ts);
        let (plan, problem) = match (spec.time_zone(), to_ts(now)) {
            (Ok(tz), Some(now_ts)) => (
                Some(schedule::plan(
                    spec,
                    &tz,
                    last_run.and_then(to_ts),
                    now_ts,
                    boot,
                    catch_up,
                )),
                None,
            ),
            (Err(err), _) => (None, Some(err)),
            (_, None) => (None, Some("the system clock is invalid".into())),
        };
        let rt = self
            .schedules
            .runtimes
            .get_mut(&spec.id)
            .expect("created before planning");
        rt.spec = spec.clone();
        rt.problem = problem;
        rt.next = plan.and_then(|p| p.next).map(SystemTime::from);
        if let Some(missed) = plan.and_then(|p| p.missed) {
            self.missed(spec, SystemTime::from(missed));
        }
    }

    fn missed(&mut self, spec: &ScheduleSpec, due: SystemTime) {
        let then = jiff::Timestamp::try_from(due)
            .map(|t| {
                t.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_default();
        let what = match spec.missed {
            MissedRun::RunOnce => "running it now",
            MissedRun::Skip => "skipped",
        };
        self.emit(
            EventLevel::Warning,
            EventKind::ScheduleMissed,
            spec.action.app(),
            format!("{}: missed the run due at {then}; {what}", spec.name),
        );
    }

    pub(super) fn schedule_deadline(&self) -> Option<Instant> {
        let now = SystemTime::now();
        let soonest = self
            .schedules
            .runtimes
            .values()
            .filter_map(|rt| rt.next)
            .min()?;
        let wait = soonest.duration_since(now).unwrap_or_default();
        Some(Instant::now() + wait.min(RECHECK))
    }

    pub(super) fn on_schedule_timers(&mut self) {
        let now = SystemTime::now();
        let due: Vec<ScheduleId> = self
            .schedules
            .runtimes
            .iter()
            .filter(|(_, rt)| rt.next.is_some_and(|t| t <= now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let rt = &self.schedules.runtimes[&id];
            let spec = rt.spec.clone();
            let late = rt
                .next
                .and_then(|t| now.duration_since(t).ok())
                .unwrap_or_default();
            let after_boot = matches!(spec.when, schedule::When::AfterBoot { .. });
            if late > schedule::LATE && !after_boot {
                // Usually the computer was asleep.
                self.missed(&spec, rt.next.expect("due"));
                if spec.missed == MissedRun::Skip {
                    self.reschedule(&id, now);
                    continue;
                }
            }
            self.run_schedule(&id, false);
        }
    }

    /// Plans the run after `after` (following a run or a skipped one).
    fn reschedule(&mut self, id: &ScheduleId, after: SystemTime) {
        let boot = self.boot_time.and_then(to_ts);
        let rt = self.schedules.runtimes.get_mut(id).expect("exists");
        rt.next = match (rt.spec.time_zone(), to_ts(after)) {
            (Ok(tz), Some(after)) if rt.spec.enabled => {
                schedule::next_after(&rt.spec.when, &tz, after, boot).map(SystemTime::from)
            }
            _ => None,
        };
    }

    pub(super) fn run_schedule_now(&mut self, id: &ScheduleId) {
        if self.schedules.runtimes.contains_key(id) {
            self.run_schedule(id, true);
        } else {
            self.reject(format!("unknown schedule {id}"));
        }
    }

    fn run_schedule(&mut self, id: &ScheduleId, manual: bool) {
        let now = SystemTime::now();
        let spec = self.schedules.runtimes[id].spec.clone();
        if !manual {
            self.reschedule(id, now);
        }
        if self.schedules.runtimes[id].running.is_some() {
            self.emit(
                EventLevel::Warning,
                EventKind::ScheduleMissed,
                None,
                format!(
                    "{}: skipped because the previous run is still going",
                    spec.name
                ),
            );
            return;
        }
        self.schedules.last_runs.insert(id.clone(), now);
        if let Some(store) = &self.store {
            store.record_schedule_run(id.clone(), now);
        }

        let outcome = match &spec.action {
            Action::Start { app } | Action::Stop { app } | Action::Restart { app } => {
                Some(self.control_app(&spec.action, app))
            }
            Action::Run {
                program,
                args,
                cwd,
                timeout,
            } => {
                let process = ProcessSpec {
                    program: program.clone(),
                    args: args.iter().map(Into::into).collect(),
                    cwd: cwd.clone(),
                    env: Vec::new(),
                };
                let rt = self.schedules.runtimes.get_mut(id).expect("exists");
                match ManagedChild::spawn(&process, Arc::clone(&rt.log)) {
                    Ok(child) => {
                        rt.generation += 1;
                        rt.running = Some(Running {
                            generation: rt.generation,
                            started: Instant::now(),
                        });
                        rt.log.push(
                            Stream::Meta,
                            format!("started (pid {})", child.pid()).as_bytes(),
                        );
                        tokio::spawn(run_task(
                            id.clone(),
                            rt.generation,
                            child,
                            *timeout,
                            self.internal_tx.clone(),
                        ));
                        None
                    }
                    Err(err) => Some(Err(format!("failed to start: {err}"))),
                }
            }
        };
        if let Some(outcome) = outcome {
            self.finish_schedule(id, now, outcome);
        }
    }

    fn control_app(
        &mut self,
        action: &Action,
        app: &crate::config::AppId,
    ) -> Result<String, String> {
        let Some(state) = self.apps.get(app).map(|a| a.state) else {
            return Err("the application no longer exists".into());
        };
        let name = self.name(app);
        if state == AppState::Disabled {
            return Err(format!("{name} is disabled"));
        }
        match action {
            Action::Start { .. } => {
                self.start_manually(app);
                Ok(format!("started {name}"))
            }
            Action::Stop { .. } => {
                self.stop(app, false);
                Ok(format!("stopped {name}"))
            }
            Action::Restart { .. } => {
                if self.apps[app].state.is_active() {
                    self.stop(app, true);
                } else {
                    self.start_manually(app);
                }
                Ok(format!("restarted {name}"))
            }
            Action::Run { .. } => unreachable!("handled by the caller"),
        }
    }

    pub(super) fn on_schedule_finished(&mut self, finished: Finished) {
        let Some(rt) = self.schedules.runtimes.get_mut(&finished.id) else {
            return; // removed while running
        };
        let Some(running) = rt.running.take_if(|r| r.generation == finished.generation) else {
            return;
        };
        let ran_for = format_duration(running.started.elapsed());
        let outcome = match (finished.timed_out, finished.result) {
            (true, _) => Err(format!("timed out after {ran_for} and was killed")),
            (false, Ok(ExitKind::Code(0))) => Ok(format!("finished in {ran_for}")),
            (false, Ok(ExitKind::Code(code))) => {
                Err(format!("exited with code {code} after {ran_for}"))
            }
            (false, Ok(ExitKind::Signal(sig))) => {
                Err(format!("terminated by signal {sig} after {ran_for}"))
            }
            (false, Ok(ExitKind::Unknown)) => {
                Err(format!("ended with unknown status after {ran_for}"))
            }
            (false, Err(err)) => Err(format!("lost track of the process: {err}")),
        };
        let text = match &outcome {
            Ok(s) | Err(s) => s.clone(),
        };
        rt.log.push(Stream::Meta, text.as_bytes());
        let started = SystemTime::now() - running.started.elapsed();
        self.finish_schedule(&finished.id, started, outcome);
    }

    fn finish_schedule(
        &mut self,
        id: &ScheduleId,
        at: SystemTime,
        outcome: Result<String, String>,
    ) {
        let Some(rt) = self.schedules.runtimes.get_mut(id) else {
            return;
        };
        let name = rt.spec.name.clone();
        let app = rt.spec.action.app().cloned();
        let (success, summary) = match outcome {
            Ok(summary) => (true, summary),
            Err(summary) => (false, summary),
        };
        rt.last = Some(ScheduleRun {
            at,
            success: Some(success),
            summary: summary.clone(),
        });
        let (level, kind) = if success {
            (EventLevel::Info, EventKind::ScheduleSucceeded)
        } else {
            (EventLevel::Error, EventKind::ScheduleFailed)
        };
        self.emit(level, kind, app.as_ref(), format!("{name}: {summary}"));
    }

    pub(super) fn schedule_statuses(&self) -> Vec<ScheduleStatus> {
        self.config
            .schedules
            .iter()
            .filter_map(|spec| {
                let rt = self.schedules.runtimes.get(&spec.id)?;
                Some(ScheduleStatus {
                    id: spec.id.clone(),
                    name: spec.name.clone(),
                    enabled: spec.enabled,
                    next_run: rt.next,
                    last_run: rt.last.clone().or_else(|| {
                        // Before the first run in this session, show the persisted time.
                        self.schedules
                            .last_runs
                            .get(&spec.id)
                            .map(|&at| ScheduleRun {
                                at,
                                success: None,
                                summary: "in an earlier session (see Events for the result)".into(),
                            })
                    }),
                    running: rt.running.is_some(),
                    problem: rt.problem.clone(),
                })
            })
            .collect()
    }
}

/// Runs a `run` action's process, killing it after `timeout`.
async fn run_task(
    id: ScheduleId,
    generation: u64,
    mut child: ManagedChild,
    timeout: Duration,
    tx: mpsc::UnboundedSender<Internal>,
) {
    enum Outcome {
        Exited(io::Result<ExitKind>),
        TimedOut,
    }
    let outcome = tokio::select! {
        exit = child.wait() => Outcome::Exited(exit),
        () = tokio::time::sleep(timeout) => Outcome::TimedOut,
    };
    let (result, timed_out) = match outcome {
        Outcome::Exited(exit) => {
            child.kill_remaining();
            (exit, false)
        }
        Outcome::TimedOut => (
            child.stop(Duration::from_secs(5)).await.map(|o| o.exit),
            true,
        ),
    };
    let _ = tx.send(Internal::ScheduleFinished(Finished {
        id,
        generation,
        result,
        timed_out,
    }));
}
