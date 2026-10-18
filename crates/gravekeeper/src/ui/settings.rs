//! General settings.

use eframe::egui;
use gravekeeper_core::{Command, CoreHandle, Snapshot};

use crate::autostart;

#[derive(Default)]
pub struct SettingsView {
    /// Read from the OS once, then updated by our own changes.
    launch_at_login: Option<Result<bool, String>>,
    error: Option<String>,
}

impl SettingsView {
    pub fn show(&mut self, ui: &mut egui::Ui, core: &CoreHandle, snapshot: &Snapshot) {
        let editable = snapshot.config_error.is_none();
        let launch_at_login = self
            .launch_at_login
            .get_or_insert_with(|| autostart::is_enabled().map_err(|e| e.to_string()))
            .clone();

        ui.heading("Settings");
        ui.add_space(4.0);
        egui::Grid::new("settings")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(160.0)
            .show(ui, |ui| {
                ui.label("Launch at login");
                match launch_at_login {
                    Ok(mut enabled) => {
                        if ui
                            .checkbox(&mut enabled, "Start Gravekeeper in the tray when I log in")
                            .changed()
                        {
                            match autostart::set_enabled(enabled) {
                                Ok(()) => {
                                    self.launch_at_login = Some(Ok(enabled));
                                    self.error = None;
                                }
                                Err(err) => {
                                    self.error =
                                        Some(format!("Could not change login item: {err}"));
                                }
                            }
                        }
                    }
                    Err(err) => {
                        ui.label(format!("Unavailable: {err}"));
                    }
                }
                ui.end_row();

                let mut general = snapshot.config.general.clone();
                let mut changed = false;
                ui.label("Start minimized");
                changed |= ui
                    .add_enabled(
                        editable,
                        egui::Checkbox::new(
                            &mut general.start_minimized,
                            "Open only the tray icon at startup",
                        ),
                    )
                    .changed();
                ui.end_row();

                ui.label("Output lines kept");
                ui.horizontal(|ui| {
                    changed |= ui
                        .add_enabled(
                            editable,
                            egui::DragValue::new(&mut general.log_lines).range(100..=200_000),
                        )
                        .changed();
                    ui.label(
                        egui::RichText::new(
                            "per application; applies after restarting Gravekeeper",
                        )
                        .weak(),
                    );
                });
                ui.end_row();
                if changed {
                    core.send(Command::SaveGeneral(general));
                }

                ui.label("Configuration file");
                ui.horizontal(|ui| {
                    if ui
                        .button("Reload")
                        .on_hover_text("Re-read the file after editing it by hand")
                        .clicked()
                    {
                        core.send(Command::ReloadConfig);
                    }
                    let path = snapshot.config_path.display().to_string();
                    ui.add(egui::Label::new(egui::RichText::new(&path).monospace()).truncate())
                        .on_hover_text(&path);
                });
                ui.end_row();

                ui.label("Event history");
                ui.label(if snapshot.history_available {
                    "Saved"
                } else {
                    "Unavailable (kept in memory only)"
                });
                ui.end_row();

                ui.label("Version");
                ui.label(env!("CARGO_PKG_VERSION"));
                ui.end_row();
            });

        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }
}
