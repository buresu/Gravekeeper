//! System and per-application resource sampling, and threshold alerts.
//!
//! Sampling runs on its own thread because some queries (disks on network mounts in
//! particular) can block. Values the platform cannot provide are `None` and shown as
//! unsupported, never estimated.

use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::config::secs;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MonitoringSettings {
    pub enabled: bool,
    #[serde(with = "secs")]
    pub interval: Duration,
    /// Alert thresholds in percent; 0 turns an alert off.
    pub cpu_alert: u8,
    pub memory_alert: u8,
    /// Applies to every listed disk.
    pub disk_alert: u8,
    /// A value must stay at or above its threshold this long before an alert is raised.
    #[serde(with = "secs")]
    pub sustain: Duration,
}

impl Default for MonitoringSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            interval: Duration::from_secs(5),
            cpu_alert: 0,
            memory_alert: 90,
            disk_alert: 90,
            sustain: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub used: u64,
    pub total: u64,
}

impl Usage {
    pub fn percent(self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.used as f64 / self.total as f64 * 100.0) as f32
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskUsage {
    pub mount: String,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProcessUsage {
    /// Percent of the whole machine (all cores together are 100%).
    pub cpu_percent: f32,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemSample {
    pub at: SystemTime,
    /// `None` until two samples have been taken, or if unsupported.
    pub cpu_percent: Option<f32>,
    /// Memory in use (total minus available).
    pub memory: Option<Usage>,
    pub disks: Vec<DiskUsage>,
    /// Usage of the supervised processes' main process, by PID.
    pub processes: HashMap<u32, ProcessUsage>,
}

enum Control {
    Interval(Duration),
}

/// Background sampler. Dropping it lets the thread finish after its current sample; it is
/// not joined, because a sample can hang on an unresponsive (network) disk.
pub struct Sampler {
    control: std_mpsc::Sender<Control>,
}

impl Sampler {
    /// Samples every `interval` and sends the results to `out`. `pids` is read before each
    /// sample to know which processes to measure.
    pub fn start(
        interval: Duration,
        pids: Arc<Mutex<Vec<u32>>>,
        out: mpsc::UnboundedSender<SystemSample>,
    ) -> std::io::Result<Self> {
        let (control, rx) = std_mpsc::channel();
        std::thread::Builder::new()
            .name("gravekeeper-monitor".into())
            .spawn(move || run(interval, pids, out, rx))?;
        Ok(Self { control })
    }

    pub fn set_interval(&self, interval: Duration) {
        let _ = self.control.send(Control::Interval(interval));
    }
}

fn run(
    mut interval: Duration,
    pids: Arc<Mutex<Vec<u32>>>,
    out: mpsc::UnboundedSender<SystemSample>,
    control: std_mpsc::Receiver<Control>,
) {
    use sysinfo::{Disks, ProcessRefreshKind, ProcessesToUpdate, System};

    let mut system = System::new();
    let mut disks = Disks::new_with_refreshed_list();
    // CPU usage is measured between two refreshes.
    system.refresh_cpu_usage();
    let mut cpu_valid = false;
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f32;

    loop {
        match control.recv_timeout(interval) {
            Ok(Control::Interval(new)) => {
                interval = new.max(Duration::from_secs(1));
                continue;
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => return,
            Err(std_mpsc::RecvTimeoutError::Timeout) => {}
        }

        system.refresh_cpu_usage();
        system.refresh_memory();
        disks.refresh(true);
        let pids: Vec<sysinfo::Pid> = pids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|&p| sysinfo::Pid::from_u32(p))
            .collect();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );

        let cpu_percent = cpu_valid.then(|| system.global_cpu_usage());
        cpu_valid = true;
        let memory = (system.total_memory() > 0).then(|| Usage {
            used: system
                .total_memory()
                .saturating_sub(system.available_memory()),
            total: system.total_memory(),
        });
        let mut seen = std::collections::HashSet::new();
        let disks: Vec<DiskUsage> = disks
            .list()
            .iter()
            // Read-only mounts (e.g. an AppImage's own squashfs) are always "full" and
            // cannot fill up further, so they would only raise false alerts.
            .filter(|d| d.total_space() > 0 && !d.is_removable() && !d.is_read_only())
            .filter_map(|d| {
                let mount = d.mount_point().to_string_lossy().into_owned();
                let shown = interesting_mount(&mount, &d.file_system().to_string_lossy());
                (shown && seen.insert(mount.clone())).then(|| DiskUsage {
                    mount,
                    usage: Usage {
                        used: d.total_space().saturating_sub(d.available_space()),
                        total: d.total_space(),
                    },
                })
            })
            .collect();
        let processes = pids
            .iter()
            .filter_map(|pid| {
                let p = system.process(*pid)?;
                Some((
                    pid.as_u32(),
                    ProcessUsage {
                        cpu_percent: p.cpu_usage() / cores,
                        memory_bytes: p.memory(),
                    },
                ))
            })
            .collect();

        let sample = SystemSample {
            at: SystemTime::now(),
            cpu_percent,
            memory,
            disks,
            processes,
        };
        if out.send(sample).is_err() {
            return;
        }
    }
}

/// Hides pseudo and system-internal file systems.
fn interesting_mount(mount: &str, fs: &str) -> bool {
    const PSEUDO: &[&str] = &[
        "squashfs", "overlay", "tmpfs", "devtmpfs", "ramfs", "efivarfs",
    ];
    if PSEUDO.contains(&fs) {
        return false;
    }
    // AppImages mount themselves here (FUSE squashfs).
    if mount.starts_with("/tmp/.mount_") {
        return false;
    }
    // macOS lists the sealed system volume's helpers; only the data volume matters.
    !(mount.starts_with("/System/Volumes/") && mount != "/System/Volumes/Data")
}

/// Percentage points a value must drop below its threshold before an alert clears, so a
/// value hovering around the threshold does not flap.
pub const HYSTERESIS: f32 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Level {
    #[default]
    Normal,
    Above(Instant),
    Alerting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Alert,
    Recover,
}

/// Debounced threshold check for one metric.
#[derive(Debug, Default)]
pub struct Threshold {
    level: Level,
}

impl Threshold {
    pub fn is_alerting(&self) -> bool {
        self.level == Level::Alerting
    }

    pub fn update(
        &mut self,
        value: f32,
        limit: u8,
        now: Instant,
        sustain: Duration,
    ) -> Option<Transition> {
        if limit == 0 {
            let was_alerting = self.is_alerting();
            self.level = Level::Normal;
            return was_alerting.then_some(Transition::Recover);
        }
        let limit = f32::from(limit);
        match self.level {
            Level::Alerting if value < limit - HYSTERESIS => {
                self.level = Level::Normal;
                Some(Transition::Recover)
            }
            Level::Alerting => None,
            _ if value < limit => {
                self.level = Level::Normal;
                None
            }
            Level::Above(since) if now.duration_since(since) >= sustain => {
                self.level = Level::Alerting;
                Some(Transition::Alert)
            }
            Level::Above(_) => None,
            Level::Normal if sustain.is_zero() => {
                self.level = Level::Alerting;
                Some(Transition::Alert)
            }
            Level::Normal => {
                self.level = Level::Above(now);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn alerts_only_after_the_sustain_period() {
        let mut t = Threshold::default();
        let start = Instant::now();
        assert_eq!(t.update(95.0, 90, start, MIN), None);
        assert_eq!(t.update(95.0, 90, start + MIN / 2, MIN), None);
        assert_eq!(
            t.update(95.0, 90, start + MIN, MIN),
            Some(Transition::Alert)
        );
        assert_eq!(t.update(99.0, 90, start + 2 * MIN, MIN), None);
    }

    #[test]
    fn a_dip_restarts_the_sustain_period() {
        let mut t = Threshold::default();
        let start = Instant::now();
        t.update(95.0, 90, start, MIN);
        t.update(50.0, 90, start + MIN / 2, MIN);
        assert_eq!(t.update(95.0, 90, start + MIN, MIN), None);
        assert_eq!(
            t.update(95.0, 90, start + 2 * MIN, MIN),
            Some(Transition::Alert)
        );
    }

    #[test]
    fn recovery_needs_hysteresis() {
        let mut t = Threshold::default();
        let now = Instant::now();
        assert_eq!(
            t.update(91.0, 90, now, Duration::ZERO),
            Some(Transition::Alert)
        );
        assert_eq!(t.update(88.0, 90, now, Duration::ZERO), None);
        assert_eq!(
            t.update(84.0, 90, now, Duration::ZERO),
            Some(Transition::Recover)
        );
    }

    #[test]
    fn turning_an_alert_off_recovers_it() {
        let mut t = Threshold::default();
        let now = Instant::now();
        t.update(99.0, 90, now, Duration::ZERO);
        assert_eq!(
            t.update(99.0, 0, now, Duration::ZERO),
            Some(Transition::Recover)
        );
        assert_eq!(t.update(99.0, 0, now, Duration::ZERO), None);
    }

    #[test]
    fn usage_percent() {
        assert_eq!(Usage { used: 1, total: 4 }.percent(), 25.0);
        assert_eq!(Usage { used: 1, total: 0 }.percent(), 0.0);
    }

    #[test]
    fn pseudo_file_systems_are_hidden() {
        assert!(interesting_mount("/", "ext4"));
        assert!(!interesting_mount("/snap/core/1", "squashfs"));
        assert!(!interesting_mount(
            "/tmp/.mount_gravekXYZ",
            "fuse.gravekeeper.AppImage"
        ));
        assert!(!interesting_mount("/System/Volumes/VM", "apfs"));
        assert!(interesting_mount("/System/Volumes/Data", "apfs"));
    }

    #[test]
    fn sampler_produces_samples() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let pids = Arc::new(Mutex::new(vec![std::process::id()]));
        let sampler = Sampler::start(Duration::from_millis(50), pids, tx).unwrap();
        let first = rx.blocking_recv().unwrap();
        let second = rx.blocking_recv().unwrap();
        drop(sampler);
        assert!(
            first.cpu_percent.is_none(),
            "first sample has no CPU baseline"
        );
        assert!(second.cpu_percent.is_some());
        assert!(second.memory.is_some_and(|m| m.total > 0));
        assert!(second.processes.contains_key(&std::process::id()));
    }
}
