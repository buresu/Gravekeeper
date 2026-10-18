//! Scheduled tasks: when they run (pure calendar arithmetic) and what they do.
//!
//! Times are local to the schedule's time zone (the system zone by default). Around DST
//! changes a wall-clock time that does not exist runs at the next valid instant (the clock
//! moves forward), and a time that occurs twice runs once, at the earlier occurrence.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use serde::{Deserialize, Serialize};

use crate::config::{AppId, secs};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScheduleId(pub String);

impl ScheduleId {
    pub fn generate() -> Self {
        Self(crate::config::generate_id())
    }
}

impl fmt::Display for ScheduleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleSpec {
    pub id: ScheduleId,
    pub name: String,
    #[serde(default = "crate::config::yes")]
    pub enabled: bool,
    pub when: When,
    pub action: Action,
    /// What to do about a run that was due while Gravekeeper was not running (or the
    /// computer was asleep).
    #[serde(default)]
    pub missed: MissedRun,
    /// IANA time zone name such as `Asia/Tokyo`; the system time zone when absent.
    #[serde(default)]
    pub time_zone: Option<String>,
}

impl ScheduleSpec {
    pub fn time_zone(&self) -> Result<TimeZone, String> {
        match &self.time_zone {
            None => Ok(TimeZone::system()),
            Some(name) => {
                TimeZone::get(name).map_err(|e| format!("unknown time zone {name:?}: {e}"))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum When {
    /// A single run at a local date and time.
    Once {
        at: DateTime,
    },
    Daily {
        time: Time,
    },
    /// On the given days of the week.
    Weekly {
        days: Vec<Day>,
        time: Time,
    },
    /// Repeatedly, counted from the previous run (or from when the schedule was set up).
    Interval {
        #[serde(with = "secs")]
        every: Duration,
    },
    /// Once per boot of the computer, this long after it started.
    AfterBoot {
        #[serde(with = "secs")]
        delay: Duration,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Day {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Day {
    pub const ALL: [Self; 7] = [
        Self::Mon,
        Self::Tue,
        Self::Wed,
        Self::Thu,
        Self::Fri,
        Self::Sat,
        Self::Sun,
    ];

    fn of(date: Date) -> Self {
        Self::ALL[date.weekday().to_monday_zero_offset() as usize]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Action {
    Start {
        app: AppId,
    },
    Stop {
        app: AppId,
    },
    Restart {
        app: AppId,
    },
    /// Runs a program directly (no shell); killed if it runs longer than `timeout`.
    Run {
        program: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        cwd: Option<PathBuf>,
        #[serde(default = "default_run_timeout", with = "secs")]
        timeout: Duration,
    },
}

impl Action {
    /// The application this action controls, if any.
    pub fn app(&self) -> Option<&AppId> {
        match self {
            Self::Start { app } | Self::Stop { app } | Self::Restart { app } => Some(app),
            Self::Run { .. } => None,
        }
    }
}

pub fn default_run_timeout() -> Duration {
    Duration::from_secs(3600)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MissedRun {
    /// Record that the run was missed and wait for the next one.
    #[default]
    Skip,
    /// Run once as soon as possible, however many runs were missed.
    RunOnce,
}

/// The first time strictly after `after` at which `when` fires, or `None` if it never will.
///
/// `boot` is when the computer started; `AfterBoot` never fires without it.
pub fn next_after(
    when: &When,
    tz: &TimeZone,
    after: Timestamp,
    boot: Option<Timestamp>,
) -> Option<Timestamp> {
    match when {
        When::Once { at } => {
            let t = resolve(*at, tz)?;
            (t > after).then_some(t)
        }
        When::Daily { time } => next_on_days(&Day::ALL, *time, tz, after),
        When::Weekly { days, time } => next_on_days(days, *time, tz, after),
        When::Interval { every } => after.checked_add(nonzero(*every)).ok(),
        When::AfterBoot { delay } => {
            let t = boot?.checked_add(*delay).ok()?;
            (t > after).then_some(t)
        }
    }
}

fn nonzero(d: Duration) -> Duration {
    d.max(Duration::from_secs(1))
}

fn next_on_days(days: &[Day], time: Time, tz: &TimeZone, after: Timestamp) -> Option<Timestamp> {
    let start = after.to_zoned(tz.clone()).date();
    // Eight days covers a full week plus today, whatever the starting point.
    (0..8)
        .filter_map(|offset| start.checked_add(jiff::Span::new().days(offset)).ok())
        .filter(|date| days.contains(&Day::of(*date)))
        .filter_map(|date| resolve(date.to_datetime(time), tz))
        .find(|t| *t > after)
}

/// Local wall-clock time to an instant: gaps move forward, folds take the earlier instant.
fn resolve(local: DateTime, tz: &TimeZone) -> Option<Timestamp> {
    tz.to_ambiguous_zoned(local)
        .compatible()
        .ok()
        .map(|z: Zoned| z.timestamp())
}

/// A run more than this late counts as missed (Gravekeeper was not running, or the computer
/// was asleep) and is handled by the schedule's [`MissedRun`] policy.
pub const LATE: Duration = Duration::from_secs(120);

/// When a schedule should run next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub next: Option<Timestamp>,
    /// A run that was due earlier and did not happen.
    pub missed: Option<Timestamp>,
}

/// Plans a schedule from its last run.
///
/// With `catch_up` (at startup), a run that should have happened since `last_run` is
/// reported as missed and, with [`MissedRun::RunOnce`], made due immediately. Without it
/// (after an edit), only future runs are considered. `AfterBoot` schedules run once per
/// boot however late Gravekeeper starts, and are never "missed".
pub fn plan(
    spec: &ScheduleSpec,
    tz: &TimeZone,
    last_run: Option<Timestamp>,
    now: Timestamp,
    boot: Option<Timestamp>,
    catch_up: bool,
) -> Plan {
    let none = Plan {
        next: None,
        missed: None,
    };
    if !spec.enabled {
        return none;
    }
    if let When::AfterBoot { delay } = spec.when {
        let Some(boot) = boot else { return none };
        if last_run.is_some_and(|last| last >= boot) {
            return none;
        }
        let due = boot.checked_add(delay).unwrap_or(boot);
        return Plan {
            next: Some(due.max(now)),
            missed: None,
        };
    }
    let interval = matches!(spec.when, When::Interval { .. });
    if !catch_up {
        let base = if interval {
            last_run.unwrap_or(now)
        } else {
            now
        };
        return Plan {
            next: next_after(&spec.when, tz, base, boot).map(|t| t.max(now)),
            missed: None,
        };
    }
    let Some(due) = next_after(&spec.when, tz, last_run.unwrap_or(now), boot) else {
        return none;
    };
    if last_run.is_some()
        && now.duration_since(due) > jiff::SignedDuration::try_from(LATE).unwrap_or_default()
    {
        let next = match spec.missed {
            MissedRun::RunOnce => Some(now),
            MissedRun::Skip => next_after(&spec.when, tz, now, boot),
        };
        return Plan {
            next,
            missed: Some(due),
        };
    }
    Plan {
        next: Some(due),
        missed: None,
    }
}

/// Checks a schedule on its own.
pub fn validate(spec: &ScheduleSpec) -> Vec<String> {
    let mut errors = Vec::new();
    if spec.id.0.trim().is_empty() {
        errors.push("id must not be empty".into());
    }
    if spec.name.trim().is_empty() {
        errors.push("name must not be empty".into());
    }
    if let Err(err) = spec.time_zone() {
        errors.push(err);
    }
    match &spec.when {
        When::Weekly { days, .. } if days.is_empty() => {
            errors.push("choose at least one day of the week".into())
        }
        When::Interval { every } if every.is_zero() => {
            errors.push("interval must be at least 1 second".into())
        }
        _ => {}
    }
    if let Action::Run {
        program, timeout, ..
    } = &spec.action
    {
        if program.as_os_str().is_empty() {
            errors.push("program must not be empty".into());
        }
        if timeout.is_zero() {
            errors.push("timeout must be at least 1 second".into());
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tz(name: &str) -> TimeZone {
        TimeZone::get(name).unwrap()
    }

    fn at(s: &str, tz: &TimeZone) -> Timestamp {
        resolve(s.parse().unwrap(), tz).unwrap()
    }

    fn local(t: Timestamp, tz: &TimeZone) -> String {
        t.to_zoned(tz.clone())
            .strftime("%Y-%m-%d %H:%M %a")
            .to_string()
    }

    #[test]
    fn daily_is_today_or_tomorrow() {
        let tz = tz("Asia/Tokyo");
        let when = When::Daily {
            time: "09:00".parse().unwrap(),
        };
        let next = |s| local(next_after(&when, &tz, at(s, &tz), None).unwrap(), &tz);
        assert_eq!(next("2026-10-19T08:59"), "2026-10-19 09:00 Mon");
        // Strictly after: exactly 09:00 moves on to the next day.
        assert_eq!(next("2026-10-19T09:00"), "2026-10-20 09:00 Tue");
    }

    #[test]
    fn weekly_picks_the_next_listed_day() {
        let tz = tz("UTC");
        let when = When::Weekly {
            days: vec![Day::Mon, Day::Fri],
            time: "18:30".parse().unwrap(),
        };
        let next = |s| local(next_after(&when, &tz, at(s, &tz), None).unwrap(), &tz);
        assert_eq!(next("2026-10-19T19:00"), "2026-10-23 18:30 Fri");
        assert_eq!(next("2026-10-23T18:30"), "2026-10-26 18:30 Mon");
    }

    #[test]
    fn spring_forward_gap_runs_at_the_next_valid_time() {
        // 2026-03-08 02:00-03:00 does not exist in New York.
        let tz = tz("America/New_York");
        let when = When::Daily {
            time: "02:30".parse().unwrap(),
        };
        let next = next_after(&when, &tz, at("2026-03-08T00:00", &tz), None).unwrap();
        assert_eq!(local(next, &tz), "2026-03-08 03:30 Sun");
    }

    #[test]
    fn fall_back_fold_runs_once_at_the_earlier_time() {
        // 2026-11-01 01:00-02:00 happens twice in New York.
        let tz = tz("America/New_York");
        let when = When::Daily {
            time: "01:30".parse().unwrap(),
        };
        let first = next_after(&when, &tz, at("2026-11-01T00:00", &tz), None).unwrap();
        assert_eq!(first.to_zoned(tz.clone()).offset().seconds(), -4 * 3600);
        // After the first run, the repeated 01:30 is not used again.
        let second = next_after(&when, &tz, first, None).unwrap();
        assert_eq!(local(second, &tz), "2026-11-02 01:30 Mon");
    }

    #[test]
    fn once_fires_only_in_the_future() {
        let tz = tz("UTC");
        let when = When::Once {
            at: "2026-12-24T20:00".parse().unwrap(),
        };
        assert!(next_after(&when, &tz, at("2026-12-01T00:00", &tz), None).is_some());
        assert!(next_after(&when, &tz, at("2026-12-25T00:00", &tz), None).is_none());
    }

    #[test]
    fn interval_and_after_boot() {
        let tz = tz("UTC");
        let t0 = at("2026-10-19T00:00", &tz);
        let every = When::Interval {
            every: Duration::from_secs(90),
        };
        assert_eq!(
            next_after(&every, &tz, t0, None).unwrap(),
            t0.checked_add(Duration::from_secs(90)).unwrap()
        );

        let boot = When::AfterBoot {
            delay: Duration::from_secs(300),
        };
        assert_eq!(next_after(&boot, &tz, t0, None), None);
        let fire = next_after(&boot, &tz, t0, Some(t0)).unwrap();
        assert_eq!(fire, t0.checked_add(Duration::from_secs(300)).unwrap());
        // Once per boot.
        assert_eq!(next_after(&boot, &tz, fire, Some(t0)), None);
    }

    fn daily(missed: MissedRun) -> ScheduleSpec {
        ScheduleSpec {
            id: ScheduleId("d".into()),
            name: "daily".into(),
            enabled: true,
            when: When::Daily {
                time: "09:00".parse().unwrap(),
            },
            action: Action::Start {
                app: AppId("a".into()),
            },
            missed,
            time_zone: Some("UTC".into()),
        }
    }

    #[test]
    fn plan_without_history_starts_from_now() {
        let tz = tz("UTC");
        let p = plan(
            &daily(MissedRun::RunOnce),
            &tz,
            None,
            at("2026-10-19T12:00", &tz),
            None,
            true,
        );
        assert_eq!(local(p.next.unwrap(), &tz), "2026-10-20 09:00 Tue");
        assert_eq!(p.missed, None);
    }

    #[test]
    fn plan_reports_missed_runs() {
        let tz = tz("UTC");
        let last = Some(at("2026-10-17T09:00", &tz));
        let now = at("2026-10-19T12:00", &tz);

        let skip = plan(&daily(MissedRun::Skip), &tz, last, now, None, true);
        assert_eq!(local(skip.missed.unwrap(), &tz), "2026-10-18 09:00 Sun");
        assert_eq!(local(skip.next.unwrap(), &tz), "2026-10-20 09:00 Tue");

        let once = plan(&daily(MissedRun::RunOnce), &tz, last, now, None, true);
        assert_eq!(once.next, Some(now));

        // Edits do not catch up.
        let edit = plan(&daily(MissedRun::RunOnce), &tz, last, now, None, false);
        assert_eq!(
            edit,
            Plan {
                next: Some(at("2026-10-20T09:00", &tz)),
                missed: None
            }
        );
    }

    #[test]
    fn slightly_late_runs_are_not_missed() {
        let tz = tz("UTC");
        let last = Some(at("2026-10-18T09:00", &tz));
        let now = at("2026-10-19T09:01", &tz);
        let p = plan(&daily(MissedRun::Skip), &tz, last, now, None, true);
        assert_eq!(p.missed, None);
        assert_eq!(p.next, Some(at("2026-10-19T09:00", &tz)));
    }

    #[test]
    fn after_boot_runs_once_per_boot_even_when_late() {
        let tz = tz("UTC");
        let mut spec = daily(MissedRun::Skip);
        spec.when = When::AfterBoot {
            delay: Duration::from_secs(60),
        };
        let boot = at("2026-10-19T08:00", &tz);
        let now = at("2026-10-19T10:00", &tz);
        assert_eq!(
            plan(&spec, &tz, None, now, Some(boot), true).next,
            Some(now)
        );
        let ran = Some(at("2026-10-19T08:01", &tz));
        assert_eq!(plan(&spec, &tz, ran, now, Some(boot), true).next, None);
        let previous_boot = Some(at("2026-10-18T08:01", &tz));
        assert_eq!(
            plan(&spec, &tz, previous_boot, now, Some(boot), true).next,
            Some(now)
        );
    }

    #[test]
    fn disabled_schedules_never_run() {
        let tz = tz("UTC");
        let mut spec = daily(MissedRun::RunOnce);
        spec.enabled = false;
        assert_eq!(
            plan(&spec, &tz, None, Timestamp::now(), None, true).next,
            None
        );
    }

    #[test]
    fn parses_from_toml() {
        #[derive(Deserialize)]
        struct Doc {
            schedule: Vec<ScheduleSpec>,
        }
        let doc: Doc = toml::from_str(
            r#"
            [[schedule]]
            id = "s1"
            name = "Nightly restart"
            when = { kind = "weekly", days = ["mon", "thu"], time = "03:00" }
            action = { kind = "restart", app = "web" }
            missed = "run-once"
            time_zone = "Asia/Tokyo"

            [[schedule]]
            id = "s2"
            name = "Cleanup"
            when = { kind = "interval", every = 600 }
            action = { kind = "run", program = "/usr/bin/cleanup", args = ["--all"] }
            "#,
        )
        .unwrap();
        assert_eq!(doc.schedule[0].missed, MissedRun::RunOnce);
        assert!(validate(&doc.schedule[0]).is_empty());
        match &doc.schedule[1].action {
            Action::Run { timeout, .. } => assert_eq!(*timeout, default_run_timeout()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn validation() {
        let spec = ScheduleSpec {
            id: ScheduleId("x".into()),
            name: "x".into(),
            enabled: true,
            when: When::Weekly {
                days: vec![],
                time: Time::midnight(),
            },
            action: Action::Run {
                program: PathBuf::new(),
                args: vec![],
                cwd: None,
                timeout: Duration::ZERO,
            },
            missed: MissedRun::Skip,
            time_zone: Some("Mars/Olympus".into()),
        };
        let errors = validate(&spec);
        assert_eq!(errors.len(), 4, "{errors:?}");
    }
}
