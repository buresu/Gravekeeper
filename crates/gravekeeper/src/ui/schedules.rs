//! Scheduled tasks: list, details, editor.

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui::{self, RichText};
use gravekeeper_core::api::{AppState, ScheduleStatus};
use gravekeeper_core::config::AppId;
use gravekeeper_core::schedule::{self, Action, Day, MissedRun, ScheduleId, ScheduleSpec, When};
use gravekeeper_core::{Command, CoreHandle, Snapshot};

use super::logview::LogView;
use super::widgets::{self, format_duration, format_relative, format_time};

#[derive(Default)]
pub struct SchedulesView {
    selected: Option<ScheduleId>,
    editor: Option<ScheduleEditor>,
    log: Option<LogView>,
    confirm_remove: Option<ScheduleId>,
}

impl SchedulesView {
    pub fn with_selection(selected: Option<ScheduleId>) -> Self {
        Self {
            selected,
            ..Self::default()
        }
    }

    pub fn selected(&self) -> Option<&ScheduleId> {
        self.selected.as_ref()
    }

    pub fn show(&mut self, ui: &mut egui::Ui, core: &CoreHandle, snapshot: &Snapshot) {
        let editable = snapshot.config_error.is_none();
        egui::Panel::left("schedule_list")
            .resizable(true)
            .default_size(220.0)
            .min_size(160.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                if ui
                    .add_enabled(editable, egui::Button::new("Add schedule"))
                    .clicked()
                {
                    let editor = ScheduleEditor::new_schedule(snapshot);
                    self.selected = Some(editor.id.clone());
                    self.editor = Some(editor);
                }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for status in &snapshot.schedules {
                        let selected = self.selected.as_ref() == Some(&status.id);
                        let text = if status.enabled {
                            RichText::new(&status.name)
                        } else {
                            RichText::new(&status.name).weak()
                        };
                        if ui.selectable_label(selected, text).clicked() {
                            self.selected = Some(status.id.clone());
                            self.editor = None;
                        }
                    }
                });
            });

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(editor) = &mut self.editor {
                match editor.show(ui, snapshot) {
                    EditorAction::None => {}
                    EditorAction::Cancel => self.editor = None,
                    EditorAction::Save(spec) => {
                        self.selected = Some(spec.id.clone());
                        core.send(Command::SaveSchedule(spec));
                        self.editor = None;
                    }
                }
            } else if let Some(status) = self
                .selected
                .as_ref()
                .and_then(|id| snapshot.schedules.iter().find(|s| &s.id == id))
            {
                self.details(ui, core, snapshot, status);
            } else if snapshot.schedules.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        "No schedules yet. Use \"Add schedule\" to run something on a timetable.",
                    );
                });
            } else {
                ui.centered_and_justified(|ui| ui.label("Select a schedule."));
            }
        });

        if let Some(id) = self.confirm_remove.clone() {
            let modal = egui::Modal::new(egui::Id::unique("confirm_remove_schedule")).show(
                ui.ctx(),
                |ui| {
                    ui.heading("Remove schedule?");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Remove").clicked() {
                            core.send(Command::RemoveSchedule(id.clone()));
                            self.confirm_remove = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_remove = None;
                        }
                    });
                },
            );
            if modal.should_close() {
                self.confirm_remove = None;
            }
        }
    }

    fn details(
        &mut self,
        ui: &mut egui::Ui,
        core: &CoreHandle,
        snapshot: &Snapshot,
        status: &ScheduleStatus,
    ) {
        let Some(spec) = snapshot.config.schedules.iter().find(|s| s.id == status.id) else {
            return;
        };
        let editable = snapshot.config_error.is_none();
        ui.heading(&spec.name);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!status.running, egui::Button::new("Run now"))
                .on_hover_text("Runs the action once without changing the timetable")
                .clicked()
            {
                core.send(Command::RunSchedule(spec.id.clone()));
            }
            if ui
                .add_enabled(editable, egui::Button::new("Edit"))
                .clicked()
            {
                self.editor = Some(ScheduleEditor::from_spec(spec));
            }
            if ui
                .add_enabled(editable, egui::Button::new("Remove"))
                .clicked()
            {
                self.confirm_remove = Some(spec.id.clone());
            }
        });
        ui.add_space(4.0);
        egui::Grid::new("schedule_status")
            .num_columns(2)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                ui.label("When");
                ui.label(describe_when(spec));
                ui.end_row();
                ui.label("Action");
                ui.label(describe_action(&spec.action, snapshot));
                ui.end_row();
                ui.label("Next run");
                ui.label(match (status.enabled, status.next_run) {
                    (false, _) => "disabled".into(),
                    (true, Some(t)) => format!("{} ({})", format_time(t), format_relative(t)),
                    (true, None) => "none".into(),
                });
                ui.end_row();
                if let Some(last) = &status.last_run {
                    ui.label("Last run");
                    let dark = ui.visuals().dark_mode;
                    let color = match last.success {
                        Some(true) => widgets::state_color(AppState::Running, dark),
                        Some(false) => widgets::state_color(AppState::GaveUp, dark),
                        None => ui.visuals().text_color(),
                    };
                    ui.colored_label(
                        color,
                        format!("{} — {}", format_time(last.at), last.summary),
                    );
                    ui.end_row();
                }
                if status.running {
                    ui.label("Status");
                    ui.label("running now");
                    ui.end_row();
                }
                if let Some(problem) = &status.problem {
                    ui.label("Problem");
                    ui.colored_label(ui.visuals().error_fg_color, problem);
                    ui.end_row();
                }
            });

        if matches!(spec.action, Action::Run { .. }) {
            ui.separator();
            let log = match &mut self.log {
                Some(log) if log.key() == spec.id.0 => log,
                _ => self.log.insert(LogView::new(spec.id.0.clone())),
            };
            log.pull(|from| core.schedule_logs_since(&spec.id, from), 1000);
            log.show(ui);
        }
    }
}

fn day_label(day: Day) -> &'static str {
    match day {
        Day::Mon => "Mon",
        Day::Tue => "Tue",
        Day::Wed => "Wed",
        Day::Thu => "Thu",
        Day::Fri => "Fri",
        Day::Sat => "Sat",
        Day::Sun => "Sun",
    }
}

fn hhmm(time: jiff::civil::Time) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

pub fn describe_when(spec: &ScheduleSpec) -> String {
    let mut text = match &spec.when {
        When::Once { at } => format!("once at {} {}", at.date(), hhmm(at.time())),
        When::Daily { time } => format!("daily at {}", hhmm(*time)),
        When::Weekly { days, time } => {
            let days: Vec<_> = Day::ALL
                .iter()
                .filter(|d| days.contains(d))
                .map(|d| day_label(*d))
                .collect();
            format!("{} at {}", days.join(", "), hhmm(*time))
        }
        When::Interval { every } => format!("every {}", format_duration(*every)),
        When::AfterBoot { delay } => {
            format!("{} after the computer starts", format_duration(*delay))
        }
    };
    if let Some(tz) = &spec.time_zone {
        text.push_str(&format!(" ({tz})"));
    }
    if !spec.enabled {
        text.push_str(" — disabled");
    }
    text
}

fn describe_action(action: &Action, snapshot: &Snapshot) -> String {
    let app_name = |id: &AppId| {
        snapshot
            .spec(id)
            .map_or_else(|| format!("(missing application {id})"), |a| a.name.clone())
    };
    match action {
        Action::Start { app } => format!("start {}", app_name(app)),
        Action::Stop { app } => format!("stop {}", app_name(app)),
        Action::Restart { app } => format!("restart {}", app_name(app)),
        Action::Run {
            program,
            args,
            timeout,
            ..
        } => {
            let mut text = format!("run {}", program.display());
            for arg in args {
                text.push(' ');
                text.push_str(arg);
            }
            text.push_str(&format!(" (time limit {})", format_duration(*timeout)));
            text
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WhenKind {
    Once,
    Daily,
    Weekly,
    Interval,
    AfterBoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionKind {
    Start,
    Stop,
    Restart,
    Run,
}

enum EditorAction {
    None,
    Save(Box<ScheduleSpec>),
    Cancel,
}

struct ScheduleEditor {
    id: ScheduleId,
    is_new: bool,
    name: String,
    enabled: bool,
    when: WhenKind,
    date_time: String,
    time: String,
    days: [bool; 7],
    every: u64,
    boot_delay: u64,
    action: ActionKind,
    app: Option<AppId>,
    program: String,
    args: String,
    cwd: String,
    timeout: u64,
    missed: MissedRun,
    time_zone: String,
    errors: Vec<String>,
}

impl ScheduleEditor {
    fn new_schedule(snapshot: &Snapshot) -> Self {
        let tomorrow = jiff::Zoned::now()
            .date()
            .tomorrow()
            .map(|d| format!("{d} 09:00"))
            .unwrap_or_default();
        Self {
            id: ScheduleId::generate(),
            is_new: true,
            name: String::new(),
            enabled: true,
            when: WhenKind::Daily,
            date_time: tomorrow,
            time: "03:00".into(),
            days: [true, false, false, false, false, false, false],
            every: 3600,
            boot_delay: 300,
            action: ActionKind::Restart,
            app: snapshot.apps.first().map(|a| a.id.clone()),
            program: String::new(),
            args: String::new(),
            cwd: String::new(),
            timeout: schedule::default_run_timeout().as_secs(),
            missed: MissedRun::Skip,
            time_zone: String::new(),
            errors: Vec::new(),
        }
    }

    fn from_spec(spec: &ScheduleSpec) -> Self {
        let mut editor = Self {
            id: spec.id.clone(),
            is_new: false,
            name: spec.name.clone(),
            enabled: spec.enabled,
            missed: spec.missed,
            time_zone: spec.time_zone.clone().unwrap_or_default(),
            ..Self::blank()
        };
        match &spec.when {
            When::Once { at } => {
                editor.when = WhenKind::Once;
                editor.date_time = format!("{} {}", at.date(), hhmm(at.time()));
            }
            When::Daily { time } => {
                editor.when = WhenKind::Daily;
                editor.time = hhmm(*time);
            }
            When::Weekly { days, time } => {
                editor.when = WhenKind::Weekly;
                editor.time = hhmm(*time);
                for (i, day) in Day::ALL.iter().enumerate() {
                    editor.days[i] = days.contains(day);
                }
            }
            When::Interval { every } => {
                editor.when = WhenKind::Interval;
                editor.every = every.as_secs();
            }
            When::AfterBoot { delay } => {
                editor.when = WhenKind::AfterBoot;
                editor.boot_delay = delay.as_secs();
            }
        }
        match &spec.action {
            Action::Start { app } => {
                (editor.action, editor.app) = (ActionKind::Start, Some(app.clone()))
            }
            Action::Stop { app } => {
                (editor.action, editor.app) = (ActionKind::Stop, Some(app.clone()))
            }
            Action::Restart { app } => {
                (editor.action, editor.app) = (ActionKind::Restart, Some(app.clone()))
            }
            Action::Run {
                program,
                args,
                cwd,
                timeout,
            } => {
                editor.action = ActionKind::Run;
                editor.program = program.to_string_lossy().into_owned();
                editor.args = args.join("\n");
                editor.cwd = cwd
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                editor.timeout = timeout.as_secs();
            }
        }
        editor
    }

    fn blank() -> Self {
        Self {
            id: ScheduleId(String::new()),
            is_new: false,
            name: String::new(),
            enabled: true,
            when: WhenKind::Daily,
            date_time: String::new(),
            time: "03:00".into(),
            days: [false; 7],
            every: 3600,
            boot_delay: 300,
            action: ActionKind::Restart,
            app: None,
            program: String::new(),
            args: String::new(),
            cwd: String::new(),
            timeout: schedule::default_run_timeout().as_secs(),
            missed: MissedRun::Skip,
            time_zone: String::new(),
            errors: Vec::new(),
        }
    }

    fn build(&self) -> Result<ScheduleSpec, Vec<String>> {
        let mut errors = Vec::new();
        let time = || {
            self.time
                .trim()
                .parse::<jiff::civil::Time>()
                .map_err(|_| "time must look like 09:30".to_owned())
        };
        let when = match self.when {
            WhenKind::Once => self
                .date_time
                .trim()
                .replace(' ', "T")
                .parse::<jiff::civil::DateTime>()
                .map(|at| When::Once { at })
                .map_err(|_| "date and time must look like 2026-12-24 20:00".to_owned()),
            WhenKind::Daily => time().map(|time| When::Daily { time }),
            WhenKind::Weekly => time().map(|time| When::Weekly {
                days: Day::ALL
                    .iter()
                    .zip(self.days)
                    .filter(|(_, on)| *on)
                    .map(|(d, _)| *d)
                    .collect(),
                time,
            }),
            WhenKind::Interval => Ok(When::Interval {
                every: Duration::from_secs(self.every),
            }),
            WhenKind::AfterBoot => Ok(When::AfterBoot {
                delay: Duration::from_secs(self.boot_delay),
            }),
        };
        let app = || {
            self.app
                .clone()
                .ok_or_else(|| "choose an application".to_owned())
        };
        let action = match self.action {
            ActionKind::Start => app().map(|app| Action::Start { app }),
            ActionKind::Stop => app().map(|app| Action::Stop { app }),
            ActionKind::Restart => app().map(|app| Action::Restart { app }),
            ActionKind::Run => {
                let cwd = self.cwd.trim();
                Ok(Action::Run {
                    program: PathBuf::from(self.program.trim()),
                    args: self
                        .args
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(str::to_owned)
                        .collect(),
                    cwd: (!cwd.is_empty()).then(|| PathBuf::from(cwd)),
                    timeout: Duration::from_secs(self.timeout),
                })
            }
        };
        let (when, action) = match (when, action) {
            (Ok(w), Ok(a)) => (w, a),
            (w, a) => {
                errors.extend(w.err());
                errors.extend(a.err());
                return Err(errors);
            }
        };
        let tz = self.time_zone.trim();
        let spec = ScheduleSpec {
            id: self.id.clone(),
            name: self.name.trim().to_owned(),
            enabled: self.enabled,
            when,
            action,
            missed: self.missed,
            time_zone: (!tz.is_empty()).then(|| tz.to_owned()),
        };
        errors.extend(schedule::validate(&spec));
        if errors.is_empty() {
            Ok(spec)
        } else {
            Err(errors)
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) -> EditorAction {
        let mut action = EditorAction::None;
        ui.heading(if self.is_new {
            "Add schedule"
        } else {
            "Edit schedule"
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("schedule_editor")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .min_col_width(130.0)
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut self.name).hint_text("e.g. Nightly restart"));
                    ui.end_row();
                    ui.label("Enabled");
                    ui.checkbox(&mut self.enabled, "Run on this timetable");
                    ui.end_row();

                    ui.label("When");
                    egui::ComboBox::from_id_salt("when_kind")
                        .selected_text(when_label(self.when))
                        .show_ui(ui, |ui| {
                            for kind in [
                                WhenKind::Once,
                                WhenKind::Daily,
                                WhenKind::Weekly,
                                WhenKind::Interval,
                                WhenKind::AfterBoot,
                            ] {
                                ui.selectable_value(&mut self.when, kind, when_label(kind));
                            }
                        });
                    ui.end_row();
                    match self.when {
                        WhenKind::Once => {
                            ui.label("Date and time");
                            ui.add(egui::TextEdit::singleline(&mut self.date_time).hint_text("2026-12-24 20:00"));
                            ui.end_row();
                        }
                        WhenKind::Daily => {
                            ui.label("Time");
                            ui.add(egui::TextEdit::singleline(&mut self.time).hint_text("09:30").desired_width(80.0));
                            ui.end_row();
                        }
                        WhenKind::Weekly => {
                            ui.label("Days");
                            ui.horizontal(|ui| {
                                for (i, day) in Day::ALL.iter().enumerate() {
                                    ui.checkbox(&mut self.days[i], day_label(*day));
                                }
                            });
                            ui.end_row();
                            ui.label("Time");
                            ui.add(egui::TextEdit::singleline(&mut self.time).hint_text("09:30").desired_width(80.0));
                            ui.end_row();
                        }
                        WhenKind::Interval => {
                            ui.label("Every");
                            ui.add(egui::DragValue::new(&mut self.every).range(1..=31_536_000).suffix(" s"));
                            ui.end_row();
                        }
                        WhenKind::AfterBoot => {
                            ui.label("Delay after boot");
                            ui.add(egui::DragValue::new(&mut self.boot_delay).range(0..=86_400).suffix(" s"));
                            ui.end_row();
                        }
                    }
                    if !matches!(self.when, WhenKind::Interval | WhenKind::AfterBoot) {
                        ui.label("Time zone");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.time_zone)
                                .hint_text("system time zone (or e.g. Asia/Tokyo)"),
                        );
                        ui.end_row();
                    }
                    if self.when != WhenKind::AfterBoot {
                        ui.label("Missed runs").on_hover_text(
                            "Runs that were due while Gravekeeper was not running or the computer was asleep",
                        );
                        egui::ComboBox::from_id_salt("missed")
                            .selected_text(missed_label(self.missed))
                            .show_ui(ui, |ui| {
                                for m in [MissedRun::Skip, MissedRun::RunOnce] {
                                    ui.selectable_value(&mut self.missed, m, missed_label(m));
                                }
                            });
                        ui.end_row();
                    }

                    ui.label("Action");
                    egui::ComboBox::from_id_salt("action_kind")
                        .selected_text(action_label(self.action))
                        .show_ui(ui, |ui| {
                            for kind in [ActionKind::Start, ActionKind::Stop, ActionKind::Restart, ActionKind::Run] {
                                ui.selectable_value(&mut self.action, kind, action_label(kind));
                            }
                        });
                    ui.end_row();
                    if self.action == ActionKind::Run {
                        ui.label("Program");
                        ui.add(egui::TextEdit::singleline(&mut self.program).hint_text("Full path to the executable").desired_width(f32::INFINITY));
                        ui.end_row();
                        ui.label("Arguments");
                        ui.add(egui::TextEdit::multiline(&mut self.args).hint_text("One argument per line").desired_rows(2).code_editor().desired_width(f32::INFINITY));
                        ui.end_row();
                        ui.label("Working directory");
                        ui.add(egui::TextEdit::singleline(&mut self.cwd).hint_text("Optional").desired_width(f32::INFINITY));
                        ui.end_row();
                        ui.label("Time limit");
                        ui.add(egui::DragValue::new(&mut self.timeout).range(1..=86_400).suffix(" s"));
                        ui.end_row();
                    } else {
                        ui.label("Application");
                        let name = self
                            .app
                            .as_ref()
                            .and_then(|id| snapshot.spec(id))
                            .map_or("choose…", |a| a.name.as_str());
                        egui::ComboBox::from_id_salt("schedule_app")
                            .selected_text(name)
                            .show_ui(ui, |ui| {
                                for app in &snapshot.apps {
                                    ui.selectable_value(&mut self.app, Some(app.id.clone()), &app.name);
                                }
                            });
                        ui.end_row();
                    }
                });

            for error in &self.errors {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    match self.build() {
                        Ok(spec) => action = EditorAction::Save(Box::new(spec)),
                        Err(errors) => self.errors = errors,
                    }
                }
                if ui.button("Cancel").clicked() {
                    action = EditorAction::Cancel;
                }
            });
        });
        action
    }
}

fn when_label(kind: WhenKind) -> &'static str {
    match kind {
        WhenKind::Once => "Once",
        WhenKind::Daily => "Every day",
        WhenKind::Weekly => "On days of the week",
        WhenKind::Interval => "At an interval",
        WhenKind::AfterBoot => "After the computer starts",
    }
}

fn action_label(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::Start => "Start an application",
        ActionKind::Stop => "Stop an application",
        ActionKind::Restart => "Restart an application",
        ActionKind::Run => "Run a program",
    }
}

fn missed_label(missed: MissedRun) -> &'static str {
    match missed {
        MissedRun::Skip => "Skip them",
        MissedRun::RunOnce => "Run once when possible",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_kind() {
        let specs = [
            r#"{ kind = "once", at = "2026-12-24T20:00:00" }"#,
            r#"{ kind = "daily", time = "09:30:00" }"#,
            r#"{ kind = "weekly", days = ["mon", "sun"], time = "18:00:00" }"#,
            r#"{ kind = "interval", every = 90 }"#,
            r#"{ kind = "after-boot", delay = 300 }"#,
        ];
        for when in specs {
            let text = format!(
                "id = \"s\"\nname = \"n\"\nwhen = {when}\naction = {{ kind = \"run\", program = \"/bin/x\", args = [\"-a\"], timeout = 60 }}\ntime_zone = \"UTC\""
            );
            let spec: ScheduleSpec = toml::from_str(&text).unwrap();
            assert_eq!(
                ScheduleEditor::from_spec(&spec).build().unwrap(),
                spec,
                "{when}"
            );
        }
    }

    #[test]
    fn reports_bad_input() {
        let mut editor = ScheduleEditor::blank();
        editor.time = "25:99".into();
        assert!(
            editor
                .build()
                .unwrap_err()
                .iter()
                .any(|e| e.contains("09:30"))
        );
        editor.when = WhenKind::Once;
        editor.date_time = "tomorrow".into();
        assert!(
            editor
                .build()
                .unwrap_err()
                .iter()
                .any(|e| e.contains("2026-12-24"))
        );
        editor.when = WhenKind::Interval;
        editor.action = ActionKind::Start;
        assert!(
            editor
                .build()
                .unwrap_err()
                .iter()
                .any(|e| e.contains("choose an application"))
        );
    }
}
