//! Event history.

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
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

        widgets::card_fill(ui, |ui| {
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
            // A real table: rows span the full width (the message column takes the rest) and
            // only visible rows are laid out.
            let row_height = ui.spacing().interact_size.y + 4.0;
            TableBuilder::new(ui)
                .striped(true)
                .resizable(false)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(90.0))
                .column(Column::exact(80.0))
                .column(Column::initial(150.0).at_least(80.0).clip(true))
                .column(Column::remainder().clip(true))
                .min_scrolled_height(0.0)
                .header(24.0, |mut header| {
                    for title in ["Time", "Level", "Source", "Message"] {
                        header.col(|ui| {
                            ui.label(RichText::new(title).small().strong().color(p.text_weak));
                        });
                    }
                })
                .body(|body| {
                    body.rows(row_height, events.len(), |mut row| {
                        let event = events[row.index()];
                        row.col(|ui| {
                            ui.label(RichText::new(format_relative(event.at)).color(p.text_weak))
                                .on_hover_text(format_time(event.at));
                        });
                        row.col(|ui| {
                            widgets::pill(
                                ui,
                                level_label(event.level),
                                level_color(event.level, &p),
                            );
                        });
                        row.col(|ui| {
                            ui.label(event.app_name.as_deref().unwrap_or("Gravekeeper"));
                        });
                        row.col(|ui| {
                            ui.add(egui::Label::new(&event.message).truncate())
                                .on_hover_text(&event.message);
                        });
                    });
                });
        });
    }
}
