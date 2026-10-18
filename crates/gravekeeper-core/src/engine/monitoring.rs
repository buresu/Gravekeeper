//! Resource samples: per-application usage, history for charts, threshold alerts.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::mpsc;

use super::Engine;
use crate::api::{EventKind, EventLevel, Metrics};
use crate::monitor::{
    MonitoringSettings, ProcessUsage, Sampler, SystemSample, Threshold, Transition,
};

/// Samples kept for the charts.
const HISTORY: usize = 120;

#[derive(Default)]
pub(super) struct State {
    sampler: Option<(Sampler, MonitoringSettings)>,
    rx: Option<mpsc::UnboundedReceiver<SystemSample>>,
    pids: Arc<Mutex<Vec<u32>>>,
    latest: Option<SystemSample>,
    cpu_history: VecDeque<f32>,
    memory_history: VecDeque<f32>,
    /// Keyed by metric (`cpu`, `memory`, `disk:/home`).
    thresholds: HashMap<String, Check>,
}

#[derive(Default)]
struct Check {
    threshold: Threshold,
    label: String,
    value: f32,
}

impl State {
    pub async fn recv(&mut self) -> Option<SystemSample> {
        match &mut self.rx {
            Some(rx) => rx.recv().await,
            None => std::future::pending().await,
        }
    }

    pub fn usage(&self, pid: u32) -> Option<ProcessUsage> {
        self.latest.as_ref()?.processes.get(&pid).copied()
    }

    pub fn set_pids(&self, pids: impl Iterator<Item = u32>) {
        *self.pids.lock().unwrap_or_else(|e| e.into_inner()) = pids.collect();
    }

    pub fn metrics(&self) -> Metrics {
        let Some(latest) = &self.latest else {
            return Metrics::default();
        };
        let mut active_alerts: Vec<String> = self
            .thresholds
            .values()
            .filter(|c| c.threshold.is_alerting())
            .map(|c| format!("{} {:.0}%", c.label, c.value))
            .collect();
        active_alerts.sort();
        Metrics {
            at: Some(latest.at),
            cpu_percent: latest.cpu_percent,
            memory: latest.memory,
            disks: latest.disks.clone(),
            cpu_history: self.cpu_history.iter().copied().collect(),
            memory_history: self.memory_history.iter().copied().collect(),
            active_alerts,
        }
    }
}

impl Engine {
    /// Starts, restarts or stops sampling to match the configuration.
    pub(super) fn start_monitoring(&mut self) {
        let settings = self.config.monitoring.clone();
        let state = &mut self.monitoring;
        if state.sampler.as_ref().is_some_and(|(_, s)| *s == settings) {
            return;
        }
        if let Some((sampler, running)) = &mut state.sampler
            && settings.enabled
            && (MonitoringSettings {
                interval: running.interval,
                ..settings.clone()
            }) == *running
        {
            sampler.set_interval(settings.interval);
            *running = settings;
            return;
        }
        state.sampler = None;
        state.rx = None;
        if !settings.enabled {
            state.latest = None;
            state.cpu_history.clear();
            state.memory_history.clear();
            state.thresholds.clear();
            return;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        match Sampler::start(settings.interval, Arc::clone(&state.pids), tx) {
            Ok(sampler) => {
                state.sampler = Some((sampler, settings));
                state.rx = Some(rx);
            }
            Err(err) => tracing::warn!(%err, "cannot start resource monitoring"),
        }
    }

    pub(super) fn on_sample(&mut self, sample: SystemSample) {
        let settings = self.config.monitoring.clone();
        let mut checks = Vec::new();
        if let Some(cpu) = sample.cpu_percent {
            checks.push(("cpu".to_owned(), "CPU".to_owned(), cpu, settings.cpu_alert));
        }
        if let Some(memory) = sample.memory {
            checks.push((
                "memory".into(),
                "memory".into(),
                memory.percent(),
                settings.memory_alert,
            ));
        }
        for disk in &sample.disks {
            checks.push((
                format!("disk:{}", disk.mount),
                format!("disk {}", disk.mount),
                disk.usage.percent(),
                settings.disk_alert,
            ));
        }

        let state = &mut self.monitoring;
        // Forget disks that are gone.
        state
            .thresholds
            .retain(|key, _| checks.iter().any(|(k, ..)| k == key));
        let now = Instant::now();
        let mut transitions = Vec::new();
        for (key, label, value, limit) in checks {
            let check = state.thresholds.entry(key).or_default();
            check.label = label.clone();
            check.value = value;
            if let Some(t) = check.threshold.update(value, limit, now, settings.sustain) {
                transitions.push((t, label, value, limit));
            }
        }

        push_capped(&mut state.cpu_history, sample.cpu_percent.unwrap_or(0.0));
        push_capped(
            &mut state.memory_history,
            sample.memory.map_or(0.0, |m| m.percent()),
        );
        state.latest = Some(sample);

        for (transition, label, value, limit) in transitions {
            match transition {
                Transition::Alert => self.emit(
                    EventLevel::Warning,
                    EventKind::ResourceAlert,
                    None,
                    format!("{label} usage at {value:.0}% (threshold {limit}%)"),
                ),
                Transition::Recover => self.emit(
                    EventLevel::Info,
                    EventKind::ResourceRecovered,
                    None,
                    format!("{label} usage back to {value:.0}%"),
                ),
            }
        }
    }
}

fn push_capped(history: &mut VecDeque<f32>, value: f32) {
    if history.len() == HISTORY {
        history.pop_front();
    }
    history.push_back(value);
}
