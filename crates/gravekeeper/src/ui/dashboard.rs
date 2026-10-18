//! Overview: application counts, system resources, alerts, upcoming schedules, problems.

use std::time::SystemTime;

use eframe::egui::{self, RichText};
use gravekeeper_core::Snapshot;
use gravekeeper_core::api::{AppState, EventLevel};
use gravekeeper_core::config::AppId;

use super::widgets::{
    self, format_bytes, format_duration, format_relative, format_time, level_color,
};

/// Returns the application the user clicked, to open it.
pub fn show(ui: &mut egui::Ui, snapshot: &Snapshot) -> Option<AppId> {
    let mut open = None;
    let dark = ui.visuals().dark_mode;
    let count =
        |pred: &dyn Fn(AppState) -> bool| snapshot.apps.iter().filter(|a| pred(a.state)).count();

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                tile(
                    ui,
                    "Running",
                    count(&|s| s == AppState::Running),
                    widgets::state_color(AppState::Running, dark),
                );
                tile(
                    ui,
                    "Restarting",
                    count(&|s| {
                        matches!(
                            s,
                            AppState::Backoff | AppState::Starting | AppState::Waiting
                        )
                    }),
                    widgets::state_color(AppState::Backoff, dark),
                );
                tile(
                    ui,
                    "Gave up",
                    count(&|s| s == AppState::GaveUp),
                    widgets::state_color(AppState::GaveUp, dark),
                );
                tile(
                    ui,
                    "Stopped",
                    count(&|s| {
                        matches!(
                            s,
                            AppState::Stopped | AppState::Disabled | AppState::Stopping
                        )
                    }),
                    widgets::state_color(AppState::Stopped, dark),
                );
            });
            let uptime = SystemTime::now()
                .duration_since(snapshot.started_at)
                .unwrap_or_default();
            let mut line = format!("Gravekeeper running for {}", format_duration(uptime));
            if let Some(boot) = snapshot.boot_time {
                let up = SystemTime::now().duration_since(boot).unwrap_or_default();
                line.push_str(&format!(" · system up {}", format_duration(up)));
            }
            ui.label(RichText::new(line).weak());
            ui.add_space(8.0);

            resources(ui, snapshot);
            ui.add_space(8.0);

            let attention: Vec<_> = snapshot
                .apps
                .iter()
                .filter(|a| matches!(a.state, AppState::GaveUp | AppState::Backoff))
                .collect();
            ui.strong("Needs attention");
            if attention.is_empty() && snapshot.metrics.active_alerts.is_empty() {
                ui.label("Nothing — all applications are fine.");
            }
            for alert in &snapshot.metrics.active_alerts {
                ui.colored_label(
                    widgets::state_color(AppState::Backoff, dark),
                    format!("⚠ {alert}"),
                );
            }
            for status in attention {
                ui.horizontal(|ui| {
                    widgets::state_badge(ui, status);
                    if ui.link(&status.name).clicked() {
                        open = Some(status.id.clone());
                    }
                    if let Some(exit) = &status.last_exit {
                        ui.label(RichText::new(exit.reason.to_string()).weak());
                    }
                });
            }
            ui.add_space(8.0);

            let mut upcoming: Vec<_> = snapshot
                .schedules
                .iter()
                .filter_map(|s| s.next_run.map(|t| (t, s)))
                .collect();
            upcoming.sort_by_key(|(t, _)| *t);
            if !upcoming.is_empty() {
                ui.strong("Upcoming schedules");
                egui::Grid::new("upcoming")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        for (at, schedule) in upcoming.iter().take(5) {
                            ui.label(&schedule.name);
                            ui.label(format!("{} ({})", format_time(*at), format_relative(*at)));
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
            }

            ui.strong("Recent problems");
            let problems: Vec<_> = snapshot
                .events
                .iter()
                .rev()
                .filter(|e| e.level >= EventLevel::Warning)
                .take(10)
                .collect();
            if problems.is_empty() {
                ui.label("None.");
            }
            egui::Grid::new("recent_problems")
                .num_columns(3)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    for event in problems {
                        ui.monospace(format_time(event.at));
                        ui.label(
                            RichText::new(event.app_name.as_deref().unwrap_or("Gravekeeper"))
                                .color(level_color(event.level, dark)),
                        );
                        ui.label(&event.message);
                        ui.end_row();
                    }
                });
        });
    open
}

fn resources(ui: &mut egui::Ui, snapshot: &Snapshot) {
    let metrics = &snapshot.metrics;
    ui.strong("System");
    if !snapshot.config.monitoring.enabled {
        ui.label("Resource monitoring is turned off (Settings).");
        return;
    }
    if metrics.at.is_none() {
        ui.label("Collecting the first sample…");
        return;
    }
    let dark = ui.visuals().dark_mode;
    let line_color = widgets::state_color(AppState::Running, dark);
    egui::Grid::new("resources")
        .num_columns(3)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            ui.label("CPU");
            ui.label(match metrics.cpu_percent {
                Some(cpu) => format!("{cpu:.0}%"),
                None => "measuring…".into(),
            });
            widgets::sparkline(ui, &metrics.cpu_history, line_color);
            ui.end_row();

            ui.label("Memory");
            match metrics.memory {
                Some(m) => {
                    ui.label(format!(
                        "{:.0}% ({} of {})",
                        m.percent(),
                        format_bytes(m.used),
                        format_bytes(m.total)
                    ));
                    widgets::sparkline(ui, &metrics.memory_history, line_color);
                }
                None => {
                    ui.label("unsupported on this system");
                    ui.label("");
                }
            }
            ui.end_row();

            for disk in &metrics.disks {
                ui.label(format!("Disk {}", disk.mount));
                ui.label(format!(
                    "{:.0}% ({} free)",
                    disk.usage.percent(),
                    format_bytes(disk.usage.total.saturating_sub(disk.usage.used))
                ));
                ui.add(egui::ProgressBar::new(disk.usage.percent() / 100.0).desired_width(160.0));
                ui.end_row();
            }
        });
}

fn tile(ui: &mut egui::Ui, label: &str, value: usize, color: egui::Color32) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_min_width(110.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(value.to_string()).size(28.0).color(color));
                ui.label(label);
            });
        });
}
