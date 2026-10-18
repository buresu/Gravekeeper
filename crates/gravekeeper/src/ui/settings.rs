//! General, monitoring and logging settings.

use eframe::egui::{self, RichText};
use gravekeeper_core::config::{GeneralSettings, LoggingSettings};
use gravekeeper_core::monitor::MonitoringSettings;
use gravekeeper_core::{Command, CoreHandle, Snapshot};

use crate::autostart;

#[derive(Default)]
pub struct SettingsView {
    /// Read from the OS once, then updated by our own changes.
    launch_at_login: Option<Result<bool, String>>,
    error: Option<String>,
    /// Edited copies, saved together with "Save changes", and the saved values they were
    /// made from.
    draft: Option<(Draft, Draft)>,
}

#[derive(Clone, PartialEq)]
struct Draft {
    general: GeneralSettings,
    monitoring: MonitoringSettings,
    logging: LoggingSettings,
}

impl Draft {
    fn of(snapshot: &Snapshot) -> Self {
        Self {
            general: snapshot.config.general.clone(),
            monitoring: snapshot.config.monitoring.clone(),
            logging: snapshot.config.logging.clone(),
        }
    }
}

impl SettingsView {
    pub fn show(&mut self, ui: &mut egui::Ui, core: &CoreHandle, snapshot: &Snapshot) {
        let editable = snapshot.config_error.is_none();
        let saved = Draft::of(snapshot);
        // An unedited draft follows changes made elsewhere (e.g. a reload).
        let mut draft = match self.draft.take() {
            Some((draft, base)) if draft != base || base == saved => draft,
            _ => saved.clone(),
        };
        let base = saved.clone();
        let mut reset = false;

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.add_enabled_ui(editable, |ui| {
                    section(ui, "General", |ui| {
                        ui.label("Launch at login");
                        let launch = self
                            .launch_at_login
                            .get_or_insert_with(|| {
                                autostart::is_enabled().map_err(|e| e.to_string())
                            })
                            .clone();
                        match launch {
                            Ok(mut enabled) => {
                                if ui
                                    .checkbox(
                                        &mut enabled,
                                        "Start Gravekeeper in the tray when I log in",
                                    )
                                    .changed()
                                {
                                    match autostart::set_enabled(enabled) {
                                        Ok(()) => {
                                            self.launch_at_login = Some(Ok(enabled));
                                            self.error = None;
                                        }
                                        Err(err) => {
                                            self.error =
                                                Some(format!("Could not change login item: {err}"))
                                        }
                                    }
                                }
                            }
                            Err(err) => {
                                ui.label(format!("Unavailable: {err}"));
                            }
                        }
                        ui.end_row();
                        ui.label("Start minimized");
                        ui.checkbox(
                            &mut draft.general.start_minimized,
                            "Open only the tray icon at startup",
                        );
                        ui.end_row();
                        ui.label("Output lines kept");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut draft.general.log_lines)
                                    .range(100..=200_000),
                            );
                            ui.label(
                                RichText::new("per application in memory (after restart)").weak(),
                            );
                        });
                        ui.end_row();
                    });

                    section(ui, "Resource monitoring", |ui| {
                        let m = &mut draft.monitoring;
                        ui.label("Enabled");
                        ui.checkbox(&mut m.enabled, "Measure CPU, memory and disks");
                        ui.end_row();
                        ui.label("Interval");
                        secs(ui, &mut m.interval, 1..=3600);
                        ui.end_row();
                        for (label, value) in [
                            ("CPU alert", &mut m.cpu_alert),
                            ("Memory alert", &mut m.memory_alert),
                            ("Disk alert", &mut m.disk_alert),
                        ] {
                            ui.label(label);
                            ui.horizontal(|ui| {
                                ui.add(egui::DragValue::new(value).range(0..=100).suffix(" %"));
                                ui.label(
                                    RichText::new(if *value == 0 { "off" } else { "at or above" })
                                        .weak(),
                                );
                            });
                            ui.end_row();
                        }
                        ui.label("Sustained for");
                        secs(ui, &mut m.sustain, 0..=86_400);
                        ui.end_row();
                    });

                    section(ui, "Log files", |ui| {
                        let l = &mut draft.logging;
                        ui.label("Application output");
                        ui.checkbox(&mut l.app_log_files, "Also write output to files");
                        ui.end_row();
                        ui.label("Rotate at");
                        ui.add(
                            egui::DragValue::new(&mut l.max_file_mb)
                                .range(1..=10_000)
                                .suffix(" MiB"),
                        );
                        ui.end_row();
                        ui.label("Keep");
                        ui.add(
                            egui::DragValue::new(&mut l.keep_files)
                                .range(0..=100)
                                .suffix(" old files"),
                        );
                        ui.end_row();
                        ui.label("");
                        ui.label(
                            RichText::new("Log file settings apply after restarting Gravekeeper.")
                                .weak(),
                        );
                        ui.end_row();
                    });
                });

                ui.add_space(8.0);
                let changed = draft != saved;
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(editable && changed, egui::Button::new("Save changes"))
                        .clicked()
                    {
                        if draft.general != saved.general {
                            core.send(Command::SaveGeneral(draft.general.clone()));
                        }
                        if draft.monitoring != saved.monitoring {
                            core.send(Command::SaveMonitoring(draft.monitoring.clone()));
                        }
                        if draft.logging != saved.logging {
                            core.send(Command::SaveLogging(draft.logging.clone()));
                        }
                        reset = true;
                    }
                    if ui
                        .add_enabled(changed, egui::Button::new("Revert"))
                        .clicked()
                    {
                        reset = true;
                    }
                });
                if let Some(error) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }

                section(ui, "Files", |ui| {
                    ui.label("Configuration");
                    ui.horizontal(|ui| {
                        if ui
                            .button("Reload")
                            .on_hover_text("Re-read the file after editing it by hand")
                            .clicked()
                        {
                            core.send(Command::ReloadConfig);
                        }
                        path(ui, &snapshot.config_path.display().to_string());
                    });
                    ui.end_row();
                    ui.label("Log files");
                    path(ui, &snapshot.data_dir.join("logs").display().to_string());
                    ui.end_row();
                    ui.label("Config backups");
                    path(ui, &snapshot.data_dir.join("backups").display().to_string());
                    ui.end_row();
                    ui.label("Event history");
                    ui.label(if snapshot.history_available {
                        "saved"
                    } else {
                        "unavailable (kept in memory only)"
                    });
                    ui.end_row();
                    ui.label("Version");
                    ui.label(env!("CARGO_PKG_VERSION"));
                    ui.end_row();
                });
            });
        if !reset {
            self.draft = Some((draft, base));
        }
    }
}

fn section(ui: &mut egui::Ui, title: &str, rows: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    ui.heading(title);
    egui::Grid::new(title)
        .num_columns(2)
        .spacing([16.0, 8.0])
        .min_col_width(160.0)
        .show(ui, rows);
}

fn secs(ui: &mut egui::Ui, value: &mut std::time::Duration, range: std::ops::RangeInclusive<u64>) {
    let mut s = value.as_secs();
    if ui
        .add(egui::DragValue::new(&mut s).range(range).suffix(" s"))
        .changed()
    {
        *value = std::time::Duration::from_secs(s);
    }
}

fn path(ui: &mut egui::Ui, path: &str) {
    ui.add(egui::Label::new(RichText::new(path).monospace()).truncate())
        .on_hover_text(path);
}
