//! Event history.

use eframe::egui::{self, RichText};
use gravekeeper_core::Snapshot;
use gravekeeper_core::api::{Event, EventLevel};
use gravekeeper_core::config::AppId;

use super::widgets::{self, format_relative, format_time, level_color, level_label};
use super::{icons, theme};

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
        let p = theme::palette(ui);
        let subtitle = if snapshot.history_available {
            "Recent activity; older events are kept in the history database."
        } else {
            "History database unavailable: events are not saved across restarts."
        };
        widgets::page_header(ui, "Events", Some(subtitle), |_| {});

        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                for level in [EventLevel::Info, EventLevel::Warning, EventLevel::Error] {
                    let label = match level {
                        EventLevel::Info => "All",
                        EventLevel::Warning => "Warnings",
                        EventLevel::Error => "Errors",
                    };
                    ui.selectable_value(&mut self.min_level, level, label);
                }
                ui.separator();
                let app_name = |id: &Option<AppId>| match id {
                    None => "All sources".to_owned(),
                    Some(id) => snapshot
                        .app(id)
                        .map_or_else(|| id.to_string(), |a| a.name.clone()),
                };
                let source = ui.label("Source");
                egui::ComboBox::from_id_salt("event_app")
                    .width(180.0)
                    .selected_text(app_name(&self.app))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.app, None, "All sources");
                        for app in &snapshot.apps {
                            ui.selectable_value(&mut self.app, Some(app.id.clone()), &app.name);
                        }
                    })
                    .response
                    .labelled_by(source.id);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text(widgets::icon_text(icons::SEARCH, "Search"))
                            .desired_width(220.0),
                    );
                });
            });
            ui.add_space(6.0);

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
                widgets::empty_state(
                    ui,
                    icons::LIST,
                    "No events",
                    "Nothing matches these filters.",
                    None,
                );
                return;
            }
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    egui::Grid::new("events")
                        .num_columns(4)
                        .striped(true)
                        .spacing([16.0, 8.0])
                        .min_col_width(60.0)
                        .show(ui, |ui| {
                            for header in ["Time", "Level", "Source", "Message"] {
                                ui.label(RichText::new(header).small().strong().color(p.text_weak));
                            }
                            ui.end_row();
                            for event in events {
                                ui.label(
                                    RichText::new(format_relative(event.at)).color(p.text_weak),
                                )
                                .on_hover_text(format_time(event.at));
                                widgets::pill(
                                    ui,
                                    level_label(event.level),
                                    level_color(event.level, &p),
                                );
                                ui.label(event.app_name.as_deref().unwrap_or("Gravekeeper"));
                                ui.label(&event.message);
                                ui.end_row();
                            }
                        });
                });
        });
    }
}
