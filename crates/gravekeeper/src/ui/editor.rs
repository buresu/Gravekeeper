//! Form for adding or editing a supervised application.

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use gravekeeper_core::config::{self, AppId, AppSpec, RestartMode};

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
    startup_grace: u64,
    stop_timeout: u64,
    mode: RestartMode,
    backoff_initial: u64,
    backoff_max: u64,
    max_restarts: u32,
    window: u64,
    stable_after: u64,
    errors: Vec<String>,
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
            startup_grace: spec.startup_grace.as_secs(),
            stop_timeout: spec.stop_timeout.as_secs(),
            mode: r.mode,
            backoff_initial: r.backoff_initial.as_secs(),
            backoff_max: r.backoff_max.as_secs(),
            max_restarts: r.max_restarts,
            window: r.window.as_secs(),
            stable_after: r.stable_after.as_secs(),
            errors: Vec::new(),
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
        ui.heading(if self.is_new {
            "Add application"
        } else {
            "Edit application"
        });
        ui.add_space(4.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            section(ui, "Command", |ui| {
                text_row(ui, "Name", &mut self.name, "e.g. Web server");
                text_row(
                    ui,
                    "Program",
                    &mut self.program,
                    "Full path to the executable",
                );
                multiline_row(ui, "Arguments", &mut self.args, "One argument per line");
                text_row(ui, "Working directory", &mut self.cwd, "Optional");
                multiline_row(ui, "Environment", &mut self.env, "KEY=VALUE, one per line");
            });
            ui.label(
                egui::RichText::new("The program is run directly, not through a shell.").weak(),
            );

            section(ui, "Startup", |ui| {
                ui.label("Enabled");
                ui.checkbox(&mut self.enabled, "Allow this application to run");
                ui.end_row();
                ui.label("Start automatically");
                ui.checkbox(&mut self.autostart, "When Gravekeeper starts");
                ui.end_row();
                secs_row(ui, "Start delay", &mut self.start_delay);
                secs_row(ui, "Startup grace", &mut self.startup_grace);
                secs_row(ui, "Stop timeout", &mut self.stop_timeout);
            });

            section(ui, "Restart policy", |ui| {
                ui.label("Restart");
                egui::ComboBox::from_id_salt("restart_mode")
                    .selected_text(mode_label(self.mode))
                    .show_ui(ui, |ui| {
                        for mode in [
                            RestartMode::OnFailure,
                            RestartMode::Always,
                            RestartMode::Never,
                        ] {
                            ui.selectable_value(&mut self.mode, mode, mode_label(mode));
                        }
                    });
                ui.end_row();
                if self.mode != RestartMode::Never {
                    secs_row(ui, "First delay", &mut self.backoff_initial);
                    secs_row(ui, "Maximum delay", &mut self.backoff_max);
                    ui.label("Give up after");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.max_restarts).range(0..=1000));
                        ui.label("restarts within");
                        ui.add(
                            egui::DragValue::new(&mut self.window)
                                .range(1..=86_400)
                                .suffix(" s"),
                        );
                    });
                    ui.end_row();
                    secs_row(ui, "Reset after running", &mut self.stable_after);
                }
            });

            for error in &self.errors {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    match self.build(other_names) {
                        Ok(spec) => action = EditorAction::Save(Box::new(spec)),
                        Err(errors) => self.errors = errors,
                    }
                }
                if ui.button("Cancel").clicked() {
                    action = EditorAction::Cancel;
                }
            });
            if !self.is_new {
                ui.label(
                    egui::RichText::new("Changes apply the next time the application starts.")
                        .weak(),
                );
            }
        });
        action
    }
}

fn mode_label(mode: RestartMode) -> &'static str {
    match mode {
        RestartMode::OnFailure => "On failure",
        RestartMode::Always => "Always",
        RestartMode::Never => "Never",
    }
}

fn section(ui: &mut egui::Ui, title: &str, rows: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(6.0);
    ui.strong(title);
    egui::Grid::new(title)
        .num_columns(2)
        .spacing([12.0, 6.0])
        .min_col_width(130.0)
        .show(ui, rows);
}

fn text_row(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.label(label);
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(f32::INFINITY),
    );
    ui.end_row();
}

fn multiline_row(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.label(label);
    ui.add(
        egui::TextEdit::multiline(value)
            .hint_text(hint)
            .desired_rows(3)
            .code_editor()
            .desired_width(f32::INFINITY),
    );
    ui.end_row();
}

fn secs_row(ui: &mut egui::Ui, label: &str, value: &mut u64) {
    ui.label(label);
    ui.add(egui::DragValue::new(value).range(0..=86_400).suffix(" s"));
    ui.end_row();
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
