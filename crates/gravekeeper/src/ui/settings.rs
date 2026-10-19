//! General, appearance, monitoring and logging settings.

use std::path::Path;

use eframe::egui::{self, RichText, ThemePreference};
use gravekeeper_core::config::{GeneralSettings, LoggingSettings};
use gravekeeper_core::monitor::MonitoringSettings;
use gravekeeper_core::{Command, CoreHandle, Snapshot};

use super::{form, icons, picker, theme, widgets};
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
        let changed = draft != saved;
        let mut reset = false;

        widgets::page_header(ui, "Settings", None, |ui| {
            if widgets::primary_button(ui, editable && changed, icons::SAVE, "Save changes")
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
            if widgets::button(ui, changed, icons::UNDO, "Revert").clicked() {
                reset = true;
            }
            if changed {
                ui.label(RichText::new("Unsaved changes").color(theme::palette(ui).warning));
            }
        });
        if let Some(error) = &self.error {
            let color = theme::palette(ui).danger;
            widgets::banner(ui, color, |ui| {
                ui.label(RichText::new(error).color(color));
            });
            ui.add_space(8.0);
        }

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                form::section(ui, "Startup", None, |ui| {
                    form::row(
                        ui,
                        "Launch at login",
                        Some("Starts Gravekeeper in the tray when you log in."),
                        |ui| {
                            let launch = self
                                .launch_at_login
                                .get_or_insert_with(|| {
                                    autostart::is_enabled().map_err(|e| e.to_string())
                                })
                                .clone();
                            match launch {
                                Ok(mut enabled) => {
                                    if ui.checkbox(&mut enabled, "Enabled").changed() {
                                        match autostart::set_enabled(enabled) {
                                            Ok(()) => {
                                                self.launch_at_login = Some(Ok(enabled));
                                                self.error = None;
                                            }
                                            Err(err) => {
                                                self.error = Some(format!(
                                                    "Could not change the login item: {err}"
                                                ))
                                            }
                                        }
                                    }
                                }
                                Err(err) => {
                                    ui.label(format!("Unavailable: {err}"));
                                }
                            }
                        },
                    );
                    form::row(
                        ui,
                        "Start minimized",
                        Some("Open only the tray icon at startup."),
                        |ui| {
                            ui.add_enabled(
                                editable,
                                egui::Checkbox::new(&mut draft.general.start_minimized, "Enabled"),
                            );
                        },
                    );
                });

                form::section(ui, "Appearance", None, |ui| {
                    form::row(ui, "Theme", None, |ui| {
                        let mut preference = ui.ctx().options(|o| o.theme_preference);
                        let before = preference;
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut preference, ThemePreference::System, "System");
                            ui.selectable_value(&mut preference, ThemePreference::Light, "Light");
                            ui.selectable_value(&mut preference, ThemePreference::Dark, "Dark");
                        });
                        if preference != before {
                            ui.ctx().set_theme(preference);
                        }
                    });
                });

                ui.add_enabled_ui(editable, |ui| {
                    form::section(ui, "Resource monitoring", None, |ui| {
                        let m = &mut draft.monitoring;
                        form::row(ui, "Enabled", None, |ui| {
                            ui.checkbox(&mut m.enabled, "Measure CPU, memory and disks");
                        });
                        form::row(ui, "Interval", None, |ui| {
                            let mut secs = m.interval.as_secs();
                            form::seconds(ui, &mut secs, 1..=3600);
                            m.interval = std::time::Duration::from_secs(secs);
                        });
                        for (label, value) in [
                            ("CPU alert", &mut m.cpu_alert),
                            ("Memory alert", &mut m.memory_alert),
                            ("Disk alert", &mut m.disk_alert),
                        ] {
                            form::row(ui, label, None, |ui| {
                                ui.horizontal(|ui| {
                                    form::number(
                                        ui,
                                        egui::DragValue::new(value).range(0..=100).suffix(" %"),
                                    );
                                    widgets::hint(
                                        ui,
                                        if *value == 0 {
                                            "off"
                                        } else {
                                            "alert at or above"
                                        },
                                    );
                                });
                            });
                        }
                        form::row(
                            ui,
                            "Sustained for",
                            Some("A value must stay high this long before an alert."),
                            |ui| {
                                let mut secs = m.sustain.as_secs();
                                form::seconds(ui, &mut secs, 0..=86_400);
                                m.sustain = std::time::Duration::from_secs(secs);
                            },
                        );
                    });

                    form::section(
                        ui,
                        "Logs",
                        Some("Log file settings apply after restarting Gravekeeper."),
                        |ui| {
                            form::row(ui, "Output kept in memory", None, |ui| {
                                ui.horizontal(|ui| {
                                    form::number(
                                        ui,
                                        egui::DragValue::new(&mut draft.general.log_lines)
                                            .range(100..=200_000),
                                    );
                                    widgets::hint(ui, "lines per application");
                                });
                            });
                            let l = &mut draft.logging;
                            form::row(ui, "Log files", None, |ui| {
                                ui.checkbox(
                                    &mut l.app_log_files,
                                    "Also write application output to files",
                                );
                            });
                            form::row(ui, "Rotate at", None, |ui| {
                                form::number(
                                    ui,
                                    egui::DragValue::new(&mut l.max_file_mb)
                                        .range(1..=10_000)
                                        .suffix(" MiB"),
                                );
                            });
                            form::row(ui, "Keep", None, |ui| {
                                form::number(
                                    ui,
                                    egui::DragValue::new(&mut l.keep_files)
                                        .range(0..=100)
                                        .suffix(" old files"),
                                );
                            });
                        },
                    );
                });

                form::section(ui, "Files", None, |ui| {
                    file_row(ui, "Configuration", &snapshot.config_path, |ui| {
                        if widgets::button(ui, true, icons::RELOAD, "Reload")
                            .on_hover_text("Re-read the file after editing it by hand")
                            .clicked()
                        {
                            core.send(Command::ReloadConfig);
                        }
                    });
                    file_row(ui, "Log files", &snapshot.data_dir.join("logs"), |_| {});
                    file_row(
                        ui,
                        "Configuration backups",
                        &snapshot.data_dir.join("backups"),
                        |_| {},
                    );
                    form::row(ui, "Event history", None, |ui| {
                        ui.label(if snapshot.history_available {
                            "saved in history.sqlite3"
                        } else {
                            "unavailable (kept in memory only)"
                        });
                    });
                    form::row(ui, "Version", None, |ui| {
                        ui.label(env!("CARGO_PKG_VERSION"));
                    });
                });
            });

        if !reset {
            self.draft = Some((draft, base));
        }
    }
}

fn file_row(ui: &mut egui::Ui, label: &str, path: &Path, extra: impl FnOnce(&mut egui::Ui)) {
    form::row(ui, label, None, |ui| {
        ui.horizontal(|ui| {
            if widgets::button(ui, true, icons::FOLDER_OPEN, "Open")
                .on_hover_text("Show in the file manager")
                .clicked()
            {
                picker::reveal(path);
            }
            extra(ui);
            let text = path.display().to_string();
            ui.add(
                egui::Label::new(
                    RichText::new(&text)
                        .monospace()
                        .color(theme::palette(ui).text_weak),
                )
                .truncate(),
            )
            .on_hover_text(&text);
        });
    });
}
