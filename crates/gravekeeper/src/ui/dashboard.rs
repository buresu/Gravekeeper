//! Overview: application counts, system resources, alerts, upcoming schedules, problems.

use std::time::SystemTime;

use eframe::egui::{self, RichText};
use gravekeeper_core::Snapshot;
use gravekeeper_core::api::{AppState, EventLevel};
use gravekeeper_core::config::AppId;

use super::widgets::{self, format_bytes, format_duration, format_relative, format_time};
use super::{icons, theme};

/// Returns the application the user clicked, to open it.
pub fn show(ui: &mut egui::Ui, snapshot: &Snapshot) -> Option<AppId> {
    let mut open = None;
    let p = theme::palette(ui);
    let count =
        |pred: &dyn Fn(AppState) -> bool| snapshot.apps.iter().filter(|a| pred(a.state)).count();

    let uptime = SystemTime::now()
        .duration_since(snapshot.started_at)
        .unwrap_or_default();
    let mut subtitle = format!("Supervising for {}", format_duration(uptime));
    if let Some(boot) = snapshot.boot_time {
        let up = SystemTime::now().duration_since(boot).unwrap_or_default();
        subtitle.push_str(&format!(" · system up {}", format_duration(up)));
    }
    widgets::page_header(ui, "Dashboard", Some(&subtitle), |_| {});

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.columns(4, |cols| {
                widgets::stat_tile(
                    &mut cols[0],
                    icons::PLAY,
                    "Running",
                    count(&|s| s == AppState::Running),
                    p.success,
                );
                widgets::stat_tile(
                    &mut cols[1],
                    icons::RESTART,
                    "Restarting",
                    count(&|s| {
                        matches!(
                            s,
                            AppState::Backoff | AppState::Starting | AppState::Waiting
                        )
                    }),
                    p.warning,
                );
                widgets::stat_tile(
                    &mut cols[2],
                    icons::WARNING_CIRCLE,
                    "Gave up",
                    count(&|s| s == AppState::GaveUp),
                    p.danger,
                );
                widgets::stat_tile(
                    &mut cols[3],
                    icons::STOP,
                    "Stopped",
                    count(&|s| {
                        matches!(
                            s,
                            AppState::Stopped | AppState::Disabled | AppState::Stopping
                        )
                    }),
                    p.neutral,
                );
            });
            ui.add_space(12.0);

            let attention: Vec<_> = snapshot
                .apps
                .iter()
                .filter(|a| matches!(a.state, AppState::GaveUp | AppState::Backoff))
                .collect();
            if !attention.is_empty() || !snapshot.metrics.active_alerts.is_empty() {
                widgets::card(ui, |ui| {
                    widgets::section_title(
                        ui,
                        &widgets::icon_text(icons::WARNING, "Needs attention"),
                    );
                    for alert in &snapshot.metrics.active_alerts {
                        ui.horizontal(|ui| {
                            widgets::pill(ui, "Resource", p.warning);
                            ui.label(alert);
                        });
                    }
                    for status in attention {
                        ui.horizontal(|ui| {
                            widgets::state_pill(ui, status);
                            if ui.link(&status.name).clicked() {
                                open = Some(status.id.clone());
                            }
                            if let Some(exit) = &status.last_exit {
                                ui.label(RichText::new(exit.reason.to_string()).color(p.text_weak));
                            }
                        });
                    }
                });
                ui.add_space(12.0);
            }

            ui.columns(2, |cols| {
                widgets::card(&mut cols[0], |ui| resources(ui, snapshot));
                widgets::card(&mut cols[1], |ui| upcoming(ui, snapshot));
            });
            ui.add_space(12.0);

            widgets::card(ui, |ui| {
                widgets::section_title(ui, "Recent problems");
                let problems: Vec<_> = snapshot
                    .events
                    .iter()
                    .rev()
                    .filter(|e| e.level >= EventLevel::Warning)
                    .take(8)
                    .collect();
                if problems.is_empty() {
                    ui.label(
                        RichText::new(widgets::icon_text(
                            icons::CHECK_CIRCLE,
                            "No warnings or errors",
                        ))
                        .color(p.success),
                    );
                }
                egui::Grid::new("recent_problems")
                    .num_columns(3)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        for event in problems {
                            ui.label(
                                RichText::new(format_time(event.at))
                                    .monospace()
                                    .color(p.text_weak),
                            );
                            widgets::pill(
                                ui,
                                event.app_name.as_deref().unwrap_or("Gravekeeper"),
                                widgets::level_color(event.level, &p),
                            );
                            ui.label(&event.message);
                            ui.end_row();
                        }
                    });
            });
        });
    open
}

fn resources(ui: &mut egui::Ui, snapshot: &Snapshot) {
    let p = theme::palette(ui);
    let metrics = &snapshot.metrics;
    widgets::section_title(ui, "System");
    if !snapshot.config.monitoring.enabled {
        ui.label(RichText::new("Resource monitoring is turned off (Settings).").color(p.text_weak));
        return;
    }
    if metrics.at.is_none() {
        ui.label(RichText::new("Collecting the first sample…").color(p.text_weak));
        return;
    }
    let width = (ui.available_width() - 200.0).clamp(80.0, 240.0);
    egui::Grid::new("resources")
        .num_columns(3)
        .spacing([12.0, 10.0])
        .show(ui, |ui| {
            ui.label(widgets::icon_text(icons::CPU, "CPU"));
            ui.label(match metrics.cpu_percent {
                Some(cpu) => format!("{cpu:.0}%"),
                None => "measuring…".into(),
            });
            widgets::sparkline(ui, &metrics.cpu_history, p.progress, width);
            ui.end_row();

            ui.label(widgets::icon_text(icons::MEMORY, "Memory"));
            match metrics.memory {
                Some(m) => {
                    ui.label(format!("{:.0}%", m.percent()))
                        .on_hover_text(format!(
                            "{} of {}",
                            format_bytes(m.used),
                            format_bytes(m.total)
                        ));
                    widgets::sparkline(ui, &metrics.memory_history, p.progress, width);
                }
                None => {
                    ui.label("unsupported");
                    ui.label("");
                }
            }
            ui.end_row();

            for disk in &metrics.disks {
                let percent = disk.usage.percent();
                let color = if percent >= f32::from(snapshot.config.monitoring.disk_alert.max(1)) {
                    p.warning
                } else {
                    p.progress
                };
                ui.label(widgets::icon_text(icons::DISK, &disk.mount));
                ui.label(format!("{percent:.0}%")).on_hover_text(format!(
                    "{} free of {}",
                    format_bytes(disk.usage.total.saturating_sub(disk.usage.used)),
                    format_bytes(disk.usage.total)
                ));
                widgets::usage_bar(ui, percent, color, width);
                ui.end_row();
            }
        });
}

fn upcoming(ui: &mut egui::Ui, snapshot: &Snapshot) {
    let p = theme::palette(ui);
    widgets::section_title(ui, "Upcoming schedules");
    let mut upcoming: Vec<_> = snapshot
        .schedules
        .iter()
        .filter_map(|s| s.next_run.map(|t| (t, s)))
        .collect();
    upcoming.sort_by_key(|(t, _)| *t);
    if upcoming.is_empty() {
        ui.label(RichText::new("Nothing scheduled.").color(p.text_weak));
        return;
    }
    egui::Grid::new("upcoming")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            for (at, schedule) in upcoming.iter().take(6) {
                ui.label(widgets::icon_text(icons::CLOCK, &schedule.name));
                ui.label(RichText::new(format_relative(*at)).color(p.text_weak))
                    .on_hover_text(format_time(*at));
                ui.end_row();
            }
        });
}
