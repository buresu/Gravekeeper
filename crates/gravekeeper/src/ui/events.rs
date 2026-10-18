//! Event history.

use eframe::egui::{self, RichText};
use gravekeeper_core::Snapshot;
use gravekeeper_core::api::{Event, EventLevel};
use gravekeeper_core::config::AppId;

use super::widgets::{format_time, level_color};

pub struct EventsView {
    min_level: EventLevel,
    app: Option<AppId>,
    search: String,
}

impl Default for EventsView {
    fn default() -> Self {
        Self {
            min_level: EventLevel::Info,
            app: None,
            search: String::new(),
        }
    }
}

impl EventsView {
    pub fn show(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        ui.horizontal(|ui| {
            ui.label("Show");
            egui::ComboBox::from_id_salt("event_level")
                .selected_text(level_label(self.min_level))
                .show_ui(ui, |ui| {
                    for level in [EventLevel::Info, EventLevel::Warning, EventLevel::Error] {
                        ui.selectable_value(&mut self.min_level, level, level_label(level));
                    }
                });
            ui.label("from");
            let app_name = |id: &Option<AppId>| match id {
                None => "all sources".to_owned(),
                Some(id) => snapshot
                    .app(id)
                    .map_or_else(|| id.to_string(), |a| a.name.clone()),
            };
            egui::ComboBox::from_id_salt("event_app")
                .selected_text(app_name(&self.app))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.app, None, "all sources");
                    for app in &snapshot.apps {
                        ui.selectable_value(&mut self.app, Some(app.id.clone()), &app.name);
                    }
                });
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search")
                    .desired_width(180.0),
            );
        });
        if !snapshot.history_available {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "History database unavailable: events are not saved across restarts.",
            );
        }
        ui.separator();

        let search = self.search.to_lowercase();
        let events: Vec<&Event> = snapshot
            .events
            .iter()
            .rev()
            .filter(|e| e.level >= self.min_level)
            .filter(|e| self.app.is_none() || e.app_id == self.app)
            .filter(|e| {
                search.is_empty()
                    || e.message.to_lowercase().contains(&search)
                    || e.app_name
                        .as_ref()
                        .is_some_and(|n| n.to_lowercase().contains(&search))
            })
            .collect();

        if events.is_empty() {
            ui.label("No events.");
            return;
        }
        let dark = ui.visuals().dark_mode;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Grid::new("events")
                    .num_columns(4)
                    .striped(true)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        for event in events {
                            ui.monospace(format_time(event.at));
                            ui.label(
                                RichText::new(level_label(event.level))
                                    .color(level_color(event.level, dark)),
                            );
                            ui.label(event.app_name.as_deref().unwrap_or("Gravekeeper"));
                            ui.label(&event.message);
                            ui.end_row();
                        }
                    });
            });
    }
}

fn level_label(level: EventLevel) -> &'static str {
    match level {
        EventLevel::Info => "Info",
        EventLevel::Warning => "Warning",
        EventLevel::Error => "Error",
    }
}
