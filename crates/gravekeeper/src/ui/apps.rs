//! Application list, details with controls, and the output viewer.

use std::time::SystemTime;

use eframe::egui::{self, Margin, RichText};
use gravekeeper_core::api::{AppState, AppStatus};
use gravekeeper_core::config::AppId;
use gravekeeper_core::{Command, CoreHandle, Snapshot};

use super::editor::{Editor, EditorAction};
use super::logview::LogView;
use super::widgets::{self, format_duration, format_relative, format_time};
use super::{icons, theme};

#[derive(Default)]
pub struct AppsView {
    pub selected: Option<AppId>,
    editor: Option<Editor>,
    log: Option<LogView>,
    confirm_remove: Option<AppId>,
}

impl AppsView {
    pub fn select(&mut self, id: AppId) {
        self.selected = Some(id);
        self.editor = None;
    }

    fn start_adding(&mut self) {
        let editor = Editor::new_app();
        self.selected = Some(editor.id().clone());
        self.editor = Some(editor);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, core: &CoreHandle, snapshot: &Snapshot) {
        if self
            .selected
            .as_ref()
            .is_some_and(|id| snapshot.app(id).is_none())
            && !self.editor.as_ref().is_some_and(|e| e.is_new)
        {
            self.selected = None;
        }
        if self.selected.is_none() && self.editor.is_none() {
            self.selected = snapshot.apps.first().map(|a| a.id.clone());
        }
        let editable = snapshot.config_error.is_none();

        let subtitle = format!(
            "{} supervised · automatic starts go top to bottom",
            snapshot.apps.len()
        );
        widgets::page_header(ui, "Applications", Some(&subtitle), |ui| {
            if widgets::primary_button(ui, editable, icons::PLUS, "Add application").clicked() {
                self.start_adding();
            }
        });

        let p = theme::palette(ui);
        egui::Panel::left("app_list")
            .resizable(true)
            .default_size(260.0)
            .min_size(200.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().inner_margin(Margin {
                right: 16,
                ..Margin::ZERO
            }))
            .show(ui, |ui| {
                widgets::card_fill(ui, |ui| {
                    if snapshot.apps.is_empty() {
                        widgets::hint(ui, "No applications yet.");
                    }
                    egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            for status in &snapshot.apps {
                                let selected = self.selected.as_ref() == Some(&status.id)
                                    && !self.editor.as_ref().is_some_and(|e| e.is_new);
                                let response = widgets::list_row(
                                    ui,
                                    &status.id,
                                    &status.name,
                                    selected,
                                    |ui| {
                                        ui.horizontal(|ui| {
                                            widgets::dot(
                                                ui,
                                                widgets::state_color(status.state, &p),
                                            );
                                            ui.label(RichText::new(&status.name).strong());
                                        });
                                        ui.label(
                                            RichText::new(row_detail(status))
                                                .small()
                                                .color(p.text_weak),
                                        );
                                    },
                                );
                                if response.clicked() {
                                    self.select(status.id.clone());
                                }
                            }
                        });
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| {
                if let Some(editor) = &mut self.editor {
                    let others: Vec<&str> = snapshot
                        .config
                        .apps
                        .iter()
                        .filter(|a| &a.id != editor.id())
                        .map(|a| a.name.as_str())
                        .collect();
                    match editor.show(ui, &others) {
                        EditorAction::None => {}
                        EditorAction::Cancel => {
                            if editor.is_new {
                                self.selected = None;
                            }
                            self.editor = None;
                        }
                        EditorAction::Save(spec) => {
                            self.selected = Some(spec.id.clone());
                            core.send(Command::SaveApp(spec));
                            self.editor = None;
                        }
                    }
                } else if let Some(status) = self.selected.as_ref().and_then(|id| snapshot.app(id))
                {
                    self.details(ui, core, snapshot, status);
                } else if snapshot.apps.is_empty()
                    && widgets::empty_state(
                        ui,
                        icons::APPS,
                        "No applications yet",
                        "Add a program for Gravekeeper to start, watch and restart.",
                        editable.then_some((icons::PLUS, "Add application")),
                    )
                {
                    self.start_adding();
                }
            });

        self.confirm_removal(ui, core, snapshot);
    }

    fn details(
        &mut self,
        ui: &mut egui::Ui,
        core: &CoreHandle,
        snapshot: &Snapshot,
        status: &AppStatus,
    ) {
        let p = theme::palette(ui);
        let id = &status.id;
        let editable = snapshot.config_error.is_none();
        let state = status.state;

        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&status.name).text_style(egui::TextStyle::Heading));
                widgets::state_pill(ui, status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button(icons::GEAR, |ui| {
                        let position = snapshot.apps.iter().position(|a| &a.id == id).unwrap_or(0);
                        let pause = if status.paused {
                            widgets::icon_text(icons::PLAY_PAUSE, "Resume supervision")
                        } else {
                            widgets::icon_text(icons::PAUSE, "Pause supervision")
                        };
                        if ui
                            .button(pause)
                            .on_hover_text("While paused, exits are recorded but never restarted.")
                            .clicked()
                        {
                            core.send(Command::SetPaused(id.clone(), !status.paused));
                        }
                        ui.separator();
                        if widgets::button(
                            ui,
                            editable && position > 0,
                            icons::ARROW_UP,
                            "Start earlier",
                        )
                        .clicked()
                        {
                            core.send(Command::MoveApp(id.clone(), -1));
                        }
                        if widgets::button(
                            ui,
                            editable && position + 1 < snapshot.apps.len(),
                            icons::ARROW_DOWN,
                            "Start later",
                        )
                        .clicked()
                        {
                            core.send(Command::MoveApp(id.clone(), 1));
                        }
                        ui.separator();
                        if widgets::danger_button(ui, editable, icons::TRASH, "Remove…").clicked()
                        {
                            self.confirm_remove = Some(id.clone());
                        }
                    })
                    .response
                    .on_hover_text("More actions");
                    if widgets::button(ui, editable, icons::EDIT, "Edit").clicked()
                        && let Some(spec) = snapshot.spec(id)
                    {
                        self.editor = Some(Editor::from_spec(spec));
                    }
                    let can_restart = matches!(state, AppState::Starting | AppState::Running);
                    if widgets::button(ui, can_restart, icons::RESTART, "Restart").clicked() {
                        core.send(Command::Restart(id.clone()));
                    }
                    let can_stop = state.is_active() && state != AppState::Stopping
                        || matches!(
                            state,
                            AppState::Waiting | AppState::Backoff | AppState::GaveUp
                        );
                    if widgets::button(ui, can_stop, icons::STOP, "Stop").clicked() {
                        core.send(Command::Stop(id.clone()));
                    }
                    let can_start = !state.is_active() && state != AppState::Disabled;
                    if widgets::primary_button(ui, can_start, icons::PLAY, "Start").clicked() {
                        core.send(Command::Start(id.clone()));
                    }
                });
            });
            ui.add_space(6.0);

            widgets::properties(ui, "app_status", |ui| {
                if let Some(spec) = snapshot.spec(id) {
                    widgets::property_label(ui, "Command");
                    let mut command = spec.program.to_string_lossy().into_owned();
                    for arg in &spec.args {
                        command.push(' ');
                        command.push_str(arg);
                    }
                    ui.add(egui::Label::new(RichText::new(&command).monospace()).truncate())
                        .on_hover_text(&command);
                    ui.end_row();
                    if let Some(cwd) = &spec.cwd {
                        widgets::property_label(ui, "Working directory");
                        ui.label(RichText::new(cwd.display().to_string()).monospace());
                        ui.end_row();
                    }
                }
                if let Some(pid) = status.pid {
                    widgets::property_label(ui, "Process");
                    let usage = match status.usage {
                        Some(u) => format!(
                            "PID {pid} · CPU {:.1}% · {}",
                            u.cpu_percent,
                            widgets::format_bytes(u.memory_bytes)
                        ),
                        None => format!("PID {pid}"),
                    };
                    ui.label(usage)
                        .on_hover_text("Main process only. CPU: all cores together are 100%.");
                    ui.end_row();
                }
                widgets::property_label(
                    ui,
                    if state.is_active() {
                        "Running for"
                    } else {
                        "Since"
                    },
                );
                let since_text = if state.is_active() {
                    format_duration(
                        SystemTime::now()
                            .duration_since(status.since)
                            .unwrap_or_default(),
                    )
                } else {
                    format_relative(status.since)
                };
                ui.label(since_text)
                    .on_hover_text(format_time(status.since));
                ui.end_row();
                if let Some(next) = status.next_attempt {
                    widgets::property_label(ui, "Next start");
                    ui.label(format_relative(next.max(SystemTime::now())));
                    ui.end_row();
                }
                if let Some(exit) = &status.last_exit {
                    widgets::property_label(ui, "Last exit");
                    ui.horizontal(|ui| {
                        ui.label(exit.reason.to_string())
                            .on_hover_text(format_time(exit.at));
                        if exit.crash_hint {
                            widgets::pill(ui, "looks like a crash", p.danger);
                        }
                    });
                    ui.end_row();
                }
                if status.failures > 0 {
                    widgets::property_label(ui, "Recent failures");
                    ui.label(status.failures.to_string());
                    ui.end_row();
                }
            });
        });
        ui.add_space(12.0);

        widgets::card(ui, |ui| {
            let capacity = snapshot.config.general.log_lines;
            let log = match &mut self.log {
                Some(log) if log.key() == id.0 => log,
                _ => self.log.insert(LogView::new(id.0.clone())),
            };
            log.pull(|from| core.logs_since(id, from), capacity);
            let file = snapshot
                .config
                .logging
                .app_log_files
                .then(|| snapshot.data_dir.join("logs").join(format!("{id}.log")));
            log.show(ui, file.as_deref());
        });
    }

    fn confirm_removal(&mut self, ui: &mut egui::Ui, core: &CoreHandle, snapshot: &Snapshot) {
        let Some(id) = self.confirm_remove.clone() else {
            return;
        };
        let name = snapshot.app(&id).map_or("?", |a| a.name.as_str());
        let modal = egui::Modal::new(egui::Id::unique("confirm_remove")).show(ui.ctx(), |ui| {
            ui.set_max_width(420.0);
            widgets::section_title(ui, "Remove application?");
            ui.label(format!(
                "\"{name}\" will be stopped if it is running and removed from the configuration."
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::danger_button(ui, true, icons::TRASH, "Remove").clicked() {
                    core.send(Command::RemoveApp(id.clone()));
                    self.confirm_remove = None;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_remove = None;
                }
            });
        });
        if modal.should_close() {
            self.confirm_remove = None;
        }
    }
}

/// The second line of a list row: what is happening now.
fn row_detail(status: &AppStatus) -> String {
    let now = SystemTime::now();
    match status.state {
        AppState::Running | AppState::Starting => format!(
            "{} · up {}",
            status.state.label(),
            format_duration(now.duration_since(status.since).unwrap_or_default())
        ),
        AppState::Waiting | AppState::Backoff => match status.next_attempt {
            Some(next) => format!(
                "{} {}",
                status.state.label(),
                format_relative(next.max(now))
            ),
            None => status.state.label().to_owned(),
        },
        _ => match &status.last_exit {
            Some(exit) => format!("{} · {}", status.state.label(), exit.reason),
            None => status.state.label().to_owned(),
        },
    }
}
