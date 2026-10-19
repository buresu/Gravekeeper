//! Form for adding or editing a supervised application.

use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui;
use gravekeeper_core::config::{self, AppId, AppSpec, RestartMode};

use super::picker::{Kind, Picker};
use super::{form, icons, widgets};

/// Text fields are kept as strings so half-typed input is not lost or rejected mid-edit.
pub struct Editor {
    id: AppId,
    pub is_new: bool,
    name: String,
    program: String,
    args: String,
    cwd: String,
    env: String,
    enabled: bool,
    autostart: bool,
    start_delay: u64,
    boot_delay: u64,
    startup_grace: u64,
    stop_timeout: u64,
    mode: RestartMode,
    backoff_initial: u64,
    backoff_max: u64,
    max_restarts: u32,
    window: u64,
    stable_after: u64,
    errors: Vec<String>,
    picker: Picker<Field>,
}

pub enum EditorAction {
    None,
    Save(Box<AppSpec>),
    Cancel,
}

impl Editor {
    pub fn new_app() -> Self {
        let mut editor = Self::from_spec(&AppSpec::new("", ""));
        editor.is_new = true;
        editor
    }

    pub fn from_spec(spec: &AppSpec) -> Self {
        let r = &spec.restart;
        Self {
            id: spec.id.clone(),
            is_new: false,
            name: spec.name.clone(),
            program: spec.program.to_string_lossy().into_owned(),
            args: spec.args.join("\n"),
            cwd: spec
                .cwd
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            env: spec
                .env
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("\n"),
            enabled: spec.enabled,
            autostart: spec.autostart,
            start_delay: spec.start_delay.as_secs(),
            boot_delay: spec.boot_delay.as_secs(),
            startup_grace: spec.startup_grace.as_secs(),
            stop_timeout: spec.stop_timeout.as_secs(),
            mode: r.mode,
            backoff_initial: r.backoff_initial.as_secs(),
            backoff_max: r.backoff_max.as_secs(),
            max_restarts: r.max_restarts,
            window: r.window.as_secs(),
            stable_after: r.stable_after.as_secs(),
            errors: Vec::new(),
            picker: Picker::default(),
        }
    }

    pub fn id(&self) -> &AppId {
        &self.id
    }

    /// Builds the spec, or lists what is wrong with the input.
    fn build(&self, other_names: &[&str]) -> Result<AppSpec, Vec<String>> {
        let mut errors = Vec::new();
        let mut env = std::collections::BTreeMap::new();
        for (i, line) in self.env.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match line.split_once('=') {
                Some((k, v)) => {
                    env.insert(k.trim().to_owned(), v.to_owned());
                }
                None => errors.push(format!("environment line {} must be KEY=VALUE", i + 1)),
            }
        }
        let secs = Duration::from_secs;
        let cwd = self.cwd.trim();
        let mut spec = AppSpec::new(self.name.trim(), self.program.trim());
        spec.id = self.id.clone();
        spec.args = self
            .args
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned)
            .collect();
        spec.cwd = (!cwd.is_empty()).then(|| PathBuf::from(cwd));
        spec.env = env;
        spec.enabled = self.enabled;
        spec.autostart = self.autostart;
        spec.start_delay = secs(self.start_delay);
        spec.boot_delay = secs(self.boot_delay);
        spec.startup_grace = secs(self.startup_grace);
        spec.stop_timeout = secs(self.stop_timeout);
        spec.restart.mode = self.mode;
        spec.restart.backoff_initial = secs(self.backoff_initial);
        spec.restart.backoff_max = secs(self.backoff_max);
        spec.restart.max_restarts = self.max_restarts;
        spec.restart.window = secs(self.window);
        spec.restart.stable_after = secs(self.stable_after);

        errors.extend(config::validate_app(&spec).into_iter().map(|e| e.message));
        if other_names.contains(&spec.name.as_str()) {
            errors.push("another application has the same name".into());
        }
        if errors.is_empty() {
            Ok(spec)
        } else {
            Err(errors)
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, other_names: &[&str]) -> EditorAction {
        let mut action = EditorAction::None;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(if self.is_new {
                    "Add application"
                } else {
                    "Edit application"
                })
                .text_style(egui::TextStyle::Heading),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::primary_button(ui, true, icons::SAVE, "Save").clicked() {
                    match self.build(other_names) {
                        Ok(spec) => action = EditorAction::Save(Box::new(spec)),
                        Err(errors) => self.errors = errors,
                    }
                }
                if ui.button("Cancel").clicked() {
                    action = EditorAction::Cancel;
                }
            });
        });
        if !self.is_new {
            widgets::hint(ui, "Changes apply the next time the application starts.");
        }
        ui.add_space(8.0);
        form::errors(ui, &self.errors);

        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                form::section(
                    ui,
                    "Program",
                    Some("Run directly, not through a shell."),
                    |ui| {
                        form::row(ui, "Name", None, |ui| {
                            form::text(ui, &mut self.name, "e.g. Web server")
                        });
                        form::row(ui, "Executable", None, |ui| {
                            let picked = form::path(
                                ui,
                                &mut self.program,
                                "/path/to/program",
                                &mut self.picker,
                                Field::Program,
                                Kind::File,
                            );
                            if picked && self.name.trim().is_empty() {
                                self.name = name_from_path(&self.program);
                                // Name is drawn above, so show the new value next frame.
                                ui.ctx().request_repaint();
                            }
                        });
                        form::row(ui, "Arguments", Some("One argument per line."), |ui| {
                            form::lines(ui, &mut self.args, "--port\n8080")
                        });
                        // The placeholder shows what an empty field means.
                        let default_cwd = config::program_dir(Path::new(self.program.trim()))
                            .map_or_else(
                                || "Gravekeeper's own".to_owned(),
                                |dir| dir.display().to_string(),
                            );
                        form::row(
                            ui,
                            "Working directory",
                            Some("Optional; defaults to the executable's folder."),
                            |ui| {
                                form::path(
                                    ui,
                                    &mut self.cwd,
                                    &default_cwd,
                                    &mut self.picker,
                                    Field::Cwd,
                                    Kind::Folder,
                                );
                            },
                        );
                        form::row(
                            ui,
                            "Environment",
                            Some("KEY=VALUE, one per line; added to the inherited environment."),
                            |ui| form::lines(ui, &mut self.env, "RUST_LOG=info"),
                        );
                    },
                );

                form::section(ui, "Startup", None, |ui| {
                    form::row(ui, "Enabled", None, |ui| {
                        ui.checkbox(&mut self.enabled, "Allow this application to run");
                    });
                    form::row(ui, "Start automatically", None, |ui| {
                        ui.checkbox(&mut self.autostart, "When Gravekeeper starts");
                    });
                    if self.autostart {
                        form::row(
                            ui,
                            "Start delay",
                            Some("Wait after Gravekeeper starts."),
                            |ui| form::seconds(ui, &mut self.start_delay, 0..=86_400),
                        );
                        form::row(
                            ui,
                            "Wait for system uptime",
                            Some("Not before the computer has been up this long."),
                            |ui| form::seconds(ui, &mut self.boot_delay, 0..=86_400),
                        );
                    }
                    form::row(
                        ui,
                        "Startup grace",
                        Some("Exiting sooner counts as a failed start."),
                        |ui| form::seconds(ui, &mut self.startup_grace, 0..=86_400),
                    );
                    form::row(
                        ui,
                        "Stop timeout",
                        Some("Killed if still running this long after a stop request."),
                        |ui| form::seconds(ui, &mut self.stop_timeout, 0..=86_400),
                    );
                });

                form::section(ui, "When it exits", None, |ui| {
                    form::labelled_row(ui, "Restart", None, |ui, label| {
                        egui::ComboBox::from_id_salt("restart_mode")
                            .width(220.0)
                            .selected_text(mode_label(self.mode))
                            .show_ui(ui, |ui| {
                                for mode in [
                                    RestartMode::OnFailure,
                                    RestartMode::Always,
                                    RestartMode::Never,
                                ] {
                                    ui.selectable_value(&mut self.mode, mode, mode_label(mode));
                                }
                            })
                            .response
                            .labelled_by(label);
                    });
                    if self.mode != RestartMode::Never {
                        form::row(
                            ui,
                            "First delay",
                            Some("Doubled after each consecutive failure."),
                            |ui| form::seconds(ui, &mut self.backoff_initial, 1..=86_400),
                        );
                        form::row(ui, "Maximum delay", None, |ui| {
                            form::seconds(ui, &mut self.backoff_max, 1..=86_400)
                        });
                        form::row(ui, "Give up after", None, |ui| {
                            ui.horizontal(|ui| {
                                form::number(
                                    ui,
                                    egui::DragValue::new(&mut self.max_restarts).range(0..=1000),
                                );
                                ui.label("restarts within");
                                form::number(
                                    ui,
                                    egui::DragValue::new(&mut self.window).range(1..=86_400),
                                );
                                ui.label("seconds");
                            });
                        });
                        form::row(
                            ui,
                            "Reset after running",
                            Some("A run this long clears the failure count."),
                            |ui| form::seconds(ui, &mut self.stable_after, 0..=86_400),
                        );
                    }
                });
            });
        action
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Program,
    Cwd,
}

/// A readable name from an executable path: `/usr/bin/my-server` → `my-server`.
fn name_from_path(path: &str) -> String {
    std::path::Path::new(path.trim())
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn mode_label(mode: RestartMode) -> &'static str {
    match mode {
        RestartMode::OnFailure => "On failure (non-zero exit)",
        RestartMode::Always => "Always",
        RestartMode::Never => "Never",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_spec() {
        let mut spec = AppSpec::new("web", "/usr/bin/web");
        spec.args = vec!["--port".into(), "8080".into()];
        spec.env.insert("A".into(), "1=2".into());
        spec.cwd = Some("/srv".into());
        let built = Editor::from_spec(&spec).build(&[]).unwrap();
        assert_eq!(built, spec);
    }

    #[test]
    fn names_come_from_the_executable() {
        assert_eq!(name_from_path("/usr/bin/my-server"), "my-server");
        assert_eq!(name_from_path("  ./run.sh  "), "run");
    }

    #[test]
    fn reports_input_problems() {
        let mut editor = Editor::new_app();
        editor.env = "NOEQUALS".into();
        let errors = editor.build(&[]).unwrap_err();
        assert!(errors.iter().any(|e| e.contains("KEY=VALUE")));
        assert!(errors.iter().any(|e| e.contains("name must not be empty")));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("program must not be empty"))
        );

        editor.name = "dup".into();
        assert!(
            editor
                .build(&["dup"])
                .unwrap_err()
                .iter()
                .any(|e| e.contains("same name"))
        );
    }
}
