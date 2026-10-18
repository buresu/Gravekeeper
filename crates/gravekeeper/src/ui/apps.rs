//! Application list, details with controls, and the output viewer.

use std::time::SystemTime;

use eframe::egui::{self, RichText};
use gravekeeper_core::api::{AppState, AppStatus};
use gravekeeper_core::config::AppId;
use gravekeeper_core::{Command, CoreHandle, Snapshot};

use super::editor::{Editor, EditorAction};
use super::logview::LogView;
use super::widgets::{self, format_relative, format_time};

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

    pub fn show(&mut self, ui: &mut egui::Ui, core: &CoreHandle, snapshot: &Snapshot) {
        if self
            .selected
            .as_ref()
            .is_some_and(|id| snapshot.app(id).is_none())
            && !self.editor.as_ref().is_some_and(|e| e.is_new)
        {
            self.selected = None;
        }

        egui::Panel::left("app_list")
            .resizable(true)
            .default_size(220.0)
            .min_size(160.0)
            .show(ui, |ui| self.list(ui, snapshot));

        egui::CentralPanel::default().show(ui, |ui| {
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
                    EditorAction::Cancel => self.editor = None,
                    EditorAction::Save(spec) => {
                        self.selected = Some(spec.id.clone());
                        core.send(Command::SaveApp(spec));
                        self.editor = None;
                    }
                }
            } else if let Some(status) = self.selected.as_ref().and_then(|id| snapshot.app(id)) {
                self.details(ui, core, snapshot, status);
            } else if snapshot.apps.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label("No applications yet. Use \"Add application\" to supervise one.");
                });
            } else {
                ui.centered_and_justified(|ui| ui.label("Select an application."));
            }
        });

        self.confirm_removal(ui, core, snapshot);
    }

    fn list(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        ui.add_space(4.0);
        let editable = snapshot.config_error.is_none();
        if ui
            .add_enabled(editable, egui::Button::new("Add application"))
            .clicked()
        {
            let editor = Editor::new_app();
            self.selected = Some(editor.id().clone());
            self.editor = Some(editor);
        }
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for status in &snapshot.apps {
                let selected = self.selected.as_ref() == Some(&status.id);
                let dot = RichText::new("● ")
                    .color(widgets::state_color(status.state, ui.visuals().dark_mode));
                let response = ui
                    .horizontal(|ui| {
                        ui.label(dot);
                        ui.selectable_label(selected, &status.name)
                    })
                    .inner
                    .on_hover_text(status.state.label());
                if response.clicked() {
                    self.select(status.id.clone());
                }
            }
        });
    }

    fn details(
        &mut self,
        ui: &mut egui::Ui,
        core: &CoreHandle,
        snapshot: &Snapshot,
        status: &AppStatus,
    ) {
        let id = &status.id;
        let editable = snapshot.config_error.is_none();
        ui.horizontal(|ui| {
            ui.heading(&status.name);
            ui.add_space(8.0);
            widgets::state_badge(ui, status);
        });

        ui.horizontal(|ui| {
            let state = status.state;
            let can_start = !state.is_active() && state != AppState::Disabled;
            if ui
                .add_enabled(can_start, egui::Button::new("Start"))
                .clicked()
            {
                core.send(Command::Start(id.clone()));
            }
            let can_stop = state.is_active() && state != AppState::Stopping
                || matches!(
                    state,
                    AppState::Waiting | AppState::Backoff | AppState::GaveUp
                );
            if ui
                .add_enabled(can_stop, egui::Button::new("Stop"))
                .clicked()
            {
                core.send(Command::Stop(id.clone()));
            }
            let can_restart = matches!(state, AppState::Starting | AppState::Running);
            if ui
                .add_enabled(can_restart, egui::Button::new("Restart"))
                .clicked()
            {
                core.send(Command::Restart(id.clone()));
            }
            let pause_label = if status.paused {
                "Resume supervision"
            } else {
                "Pause supervision"
            };
            if ui
                .button(pause_label)
                .on_hover_text("While paused, exits are recorded but never restarted.")
                .clicked()
            {
                core.send(Command::SetPaused(id.clone(), !status.paused));
            }
            ui.separator();
            if ui
                .add_enabled(editable, egui::Button::new("Edit"))
                .clicked()
                && let Some(spec) = snapshot.spec(id)
            {
                self.editor = Some(Editor::from_spec(spec));
            }
            if ui
                .add_enabled(editable, egui::Button::new("Remove"))
                .clicked()
            {
                self.confirm_remove = Some(id.clone());
            }
            ui.separator();
            let position = snapshot.apps.iter().position(|a| &a.id == id).unwrap_or(0);
            if ui
                .add_enabled(editable && position > 0, egui::Button::new("⬆"))
                .on_hover_text("Start earlier (automatic starts go top to bottom)")
                .clicked()
            {
                core.send(Command::MoveApp(id.clone(), -1));
            }
            if ui
                .add_enabled(
                    editable && position + 1 < snapshot.apps.len(),
                    egui::Button::new("⬇"),
                )
                .on_hover_text("Start later")
                .clicked()
            {
                core.send(Command::MoveApp(id.clone(), 1));
            }
        });
        ui.add_space(4.0);

        egui::Grid::new("app_status")
            .num_columns(2)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                if let Some(spec) = snapshot.spec(id) {
                    ui.label("Command");
                    let mut command = spec.program.to_string_lossy().into_owned();
                    for arg in &spec.args {
                        command.push(' ');
                        command.push_str(arg);
                    }
                    ui.add(egui::Label::new(RichText::new(&command).monospace()).truncate())
                        .on_hover_text(&command);
                    ui.end_row();
                }
                if let Some(pid) = status.pid {
                    ui.label("PID");
                    ui.label(pid.to_string());
                    ui.end_row();
                    ui.label("Resources");
                    ui.label(match status.usage {
                        Some(u) => format!(
                            "CPU {:.1}% · memory {}",
                            u.cpu_percent,
                            widgets::format_bytes(u.memory_bytes)
                        ),
                        None if snapshot.config.monitoring.enabled => "measuring…".into(),
                        None => "monitoring is off".into(),
                    })
                    .on_hover_text("Main process only. CPU: all cores together are 100%.");
                    ui.end_row();
                }
                ui.label(if status.state.is_active() {
                    "Started"
                } else {
                    "Since"
                });
                ui.label(format!(
                    "{} ({})",
                    format_time(status.since),
                    format_relative(status.since)
                ));
                ui.end_row();
                if let Some(next) = status.next_attempt {
                    ui.label("Next start");
                    ui.label(format_relative(next.max(SystemTime::now())));
                    ui.end_row();
                }
                if let Some(exit) = &status.last_exit {
                    ui.label("Last exit");
                    let mut text = format!("{} at {}", exit.reason, format_time(exit.at));
                    if exit.crash_hint {
                        text.push_str(" — looks like a crash");
                    }
                    ui.label(text);
                    ui.end_row();
                }
                if status.failures > 0 {
                    ui.label("Recent failures");
                    ui.label(status.failures.to_string());
                    ui.end_row();
                }
            });

        ui.separator();
        let capacity = snapshot.config.general.log_lines;
        let log = match &mut self.log {
            Some(log) if log.key() == id.0 => log,
            _ => self.log.insert(LogView::new(id.0.clone())),
        };
        log.pull(|from| core.logs_since(id, from), capacity);
        log.show(ui);
    }

    fn confirm_removal(&mut self, ui: &mut egui::Ui, core: &CoreHandle, snapshot: &Snapshot) {
        let Some(id) = self.confirm_remove.clone() else {
            return;
        };
        let name = snapshot.app(&id).map_or("?", |a| a.name.as_str());
        let modal = egui::Modal::new(egui::Id::unique("confirm_remove")).show(ui.ctx(), |ui| {
            ui.heading("Remove application?");
            ui.label(format!(
                "\"{name}\" will be stopped if it is running and removed from the configuration."
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Remove").clicked() {
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
