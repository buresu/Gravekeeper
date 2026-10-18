//! Overview: counts by state, applications needing attention, recent problems.

use std::time::SystemTime;

use eframe::egui::{self, RichText};
use gravekeeper_core::Snapshot;
use gravekeeper_core::api::{AppState, EventLevel};
use gravekeeper_core::config::AppId;

use super::widgets::{self, format_duration, format_time, level_color};

/// Returns the application the user clicked, to open it.
pub fn show(ui: &mut egui::Ui, snapshot: &Snapshot) -> Option<AppId> {
    let mut open = None;
    let dark = ui.visuals().dark_mode;
    let count =
        |pred: &dyn Fn(AppState) -> bool| snapshot.apps.iter().filter(|a| pred(a.state)).count();

    ui.horizontal(|ui| {
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
    ui.label(
        RichText::new(format!(
            "Gravekeeper running for {}",
            format_duration(uptime)
        ))
        .weak(),
    );
    ui.add_space(8.0);

    let attention: Vec<_> = snapshot
        .apps
        .iter()
        .filter(|a| matches!(a.state, AppState::GaveUp | AppState::Backoff))
        .collect();
    ui.strong("Needs attention");
    if attention.is_empty() {
        ui.label("Nothing — all applications are fine.");
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
    open
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
