//! Restart decisions, kept free of I/O and real time so they can be tested exactly.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::config::RestartPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    RetryAfter(Duration),
    GiveUp,
}

/// Tracks recent failures of one application.
#[derive(Debug, Default)]
pub struct RestartTracker {
    /// Times of failures since the last stable run, oldest first.
    failures: VecDeque<Instant>,
    consecutive: u32,
}

impl RestartTracker {
    /// Forgets all failures (after a manual start).
    pub fn reset(&mut self) {
        self.failures.clear();
        self.consecutive = 0;
    }

    pub fn consecutive(&self) -> u32 {
        self.consecutive
    }

    /// Records a failed run that lasted `ran_for` and decides what to do next.
    ///
    /// A run of at least `stable_after` clears the history first, so only failures in quick
    /// succession count. More than `max_restarts` failures within `window` gives up.
    pub fn on_failure(
        &mut self,
        now: Instant,
        ran_for: Duration,
        policy: &RestartPolicy,
        jitter: f64,
    ) -> Decision {
        if ran_for >= policy.stable_after {
            self.reset();
        }
        while self
            .failures
            .front()
            .is_some_and(|&t| now.duration_since(t) > policy.window)
        {
            self.failures.pop_front();
        }
        self.consecutive += 1;
        if self.failures.len() >= policy.max_restarts as usize {
            return Decision::GiveUp;
        }
        self.failures.push_back(now);
        Decision::RetryAfter(backoff(self.consecutive, policy, jitter))
    }
}

/// `initial * 2^(attempt-1)`, capped at `max`, plus up to 10% (`jitter` in 0..1) so that
/// several applications failing together do not restart in lockstep.
fn backoff(attempt: u32, policy: &RestartPolicy, jitter: f64) -> Duration {
    let exponent = attempt.saturating_sub(1).min(20);
    let delay = policy
        .backoff_initial
        .saturating_mul(1 << exponent)
        .min(policy.backoff_max);
    delay + delay.mul_f64(0.1 * jitter.clamp(0.0, 1.0))
}

/// A jitter value in 0..1 that differs between calls; good enough without a RNG crate.
pub fn jitter() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    f64::from(nanos % 1000) / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    fn policy() -> RestartPolicy {
        RestartPolicy {
            backoff_initial: S,
            backoff_max: 10 * S,
            max_restarts: 3,
            window: 60 * S,
            stable_after: 30 * S,
            ..RestartPolicy::default()
        }
    }

    #[test]
    fn backs_off_exponentially_up_to_the_maximum() {
        let p = RestartPolicy {
            max_restarts: 100,
            ..policy()
        };
        let mut t = RestartTracker::default();
        let now = Instant::now();
        let delays: Vec<_> = (0..6)
            .map(|_| match t.on_failure(now, Duration::ZERO, &p, 0.0) {
                Decision::RetryAfter(d) => d.as_secs(),
                Decision::GiveUp => panic!("gave up"),
            })
            .collect();
        assert_eq!(delays, [1, 2, 4, 8, 10, 10]);
    }

    #[test]
    fn gives_up_after_too_many_failures_in_the_window() {
        let p = policy();
        let mut t = RestartTracker::default();
        let start = Instant::now();
        for i in 0..3 {
            assert!(matches!(
                t.on_failure(start + i * S, Duration::ZERO, &p, 0.0),
                Decision::RetryAfter(_)
            ));
        }
        assert_eq!(
            t.on_failure(start + 3 * S, Duration::ZERO, &p, 0.0),
            Decision::GiveUp
        );
    }

    #[test]
    fn old_failures_leave_the_window() {
        let p = policy();
        let mut t = RestartTracker::default();
        let start = Instant::now();
        for i in 0..3 {
            t.on_failure(start + i * S, Duration::ZERO, &p, 0.0);
        }
        // 70 s later the first failures are outside the 60 s window.
        assert!(matches!(
            t.on_failure(start + 70 * S, Duration::ZERO, &p, 0.0),
            Decision::RetryAfter(_)
        ));
    }

    #[test]
    fn a_stable_run_resets_backoff_and_history() {
        let p = policy();
        let mut t = RestartTracker::default();
        let now = Instant::now();
        for _ in 0..3 {
            t.on_failure(now, Duration::ZERO, &p, 0.0);
        }
        assert_eq!(t.on_failure(now, 45 * S, &p, 0.0), Decision::RetryAfter(S));
        assert_eq!(t.consecutive(), 1);
    }

    #[test]
    fn zero_max_restarts_never_restarts() {
        let p = RestartPolicy {
            max_restarts: 0,
            ..policy()
        };
        let mut t = RestartTracker::default();
        assert_eq!(
            t.on_failure(Instant::now(), Duration::ZERO, &p, 0.0),
            Decision::GiveUp
        );
    }

    #[test]
    fn jitter_adds_at_most_ten_percent() {
        let p = policy();
        assert_eq!(backoff(1, &p, 1.0), S + S / 10);
        assert_eq!(backoff(1, &p, 7.0), S + S / 10);
        assert!((0.0..1.0).contains(&jitter()));
    }
}
