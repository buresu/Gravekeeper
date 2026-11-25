//! Scheduled tasks: list, details, editor.

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui::{self, Margin, RichText};
use gravekeeper_core::api::ScheduleStatus;
use gravekeeper_core::config::AppId;
use gravekeeper_core::power::{self, Operation, SleepSupport};
use gravekeeper_core::schedule::{self, Action, Day, MissedRun, ScheduleId, ScheduleSpec, When};
use gravekeeper_core::{Command, CoreHandle, Snapshot};

use super::logview::LogView;
use super::picker::{Kind, Picker};
use super::widgets::{self, format_duration, format_relative, format_time};
use super::{form, icons, theme};
use crate::{autostart, os_settings};

#[derive(Default)]
pub struct SchedulesView {
    selected: Option<ScheduleId>,
    editor: Option<ScheduleEditor>,
    log: Option<LogView>,
    confirm_remove: Option<ScheduleId>,
    /// Running it now does this to the computer.
    confirm_power: Option<(ScheduleId, Operation)>,
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

    fn start_adding(&mut self, snapshot: &Snapshot) {
        let editor = ScheduleEditor::new_schedule(snapshot);
        self.selected = Some(editor.id.clone());
        self.editor = Some(editor);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, core: &CoreHandle, snapshot: &Snapshot) {
        let editable = snapshot.config_error.is_none();
        if self.editor.is_none()
            && !self
                .selected
                .as_ref()
                .is_some_and(|id| snapshot.schedules.iter().any(|s| &s.id == id))
        {
            self.selected = snapshot.schedules.first().map(|s| s.id.clone());
        }

        widgets::page_header(
            ui,
            "Schedules",
            Some(if power::SUPPORTED {
                "Start, stop or restart applications, run programs, or restart, shut down or sleep the computer, on a timetable."
            } else {
                "Start, stop or restart applications, or run programs, on a timetable."
            }),
            |ui| {
                if widgets::primary_button(ui, editable, icons::PLUS, "Add schedule").clicked() {
                    self.start_adding(snapshot);
                }
            },
        );

        let p = theme::palette(ui);
        egui::Panel::left("schedule_list")
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
                    if snapshot.schedules.is_empty() {
                        widgets::hint(ui, "No schedules yet.");
                    }
                    egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            for status in &snapshot.schedules {
                                let selected = self.selected.as_ref() == Some(&status.id)
                                    && !self.editor.as_ref().is_some_and(|e| e.is_new);
                                let response = widgets::list_row(
                                    ui,
                                    ("schedule", &status.id),
                                    &status.name,
                                    selected,
                                    |ui| {
                                        ui.horizontal(|ui| {
                                            let (icon, color) = match status
                                                .last_run
                                                .as_ref()
                                                .and_then(|r| r.success)
                                            {
                                                Some(false) => (icons::WARNING_CIRCLE, p.danger),
                                                _ if status.running => (icons::TIMER, p.warning),
                                                _ if status.enabled => (icons::CLOCK, p.accent),
                                                _ => (icons::PAUSE, p.neutral),
                                            };
                                            ui.label(RichText::new(icon).color(color));
                                            let name = RichText::new(&status.name).strong();
                                            ui.label(if status.enabled {
                                                name
                                            } else {
                                                name.color(p.text_weak)
                                            });
                                        });
                                        let detail = match (status.enabled, status.next_run) {
                                            (false, _) => "Disabled".to_owned(),
                                            (true, Some(next)) => {
                                                format!("Next {}", format_relative(next))
                                            }
                                            (true, None) => "No upcoming run".to_owned(),
                                        };
                                        ui.label(RichText::new(detail).small().color(p.text_weak));
                                    },
                                );
                                if response.clicked() {
                                    self.selected = Some(status.id.clone());
                                    self.editor = None;
                                }
                            }
                        });
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| {
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
                } else if snapshot.schedules.is_empty()
                    && widgets::empty_state(
                        ui,
                        icons::CALENDAR,
                        "No schedules yet",
                        "Restart something every night, run a cleanup every hour, or start a program after boot.",
                        editable.then_some((icons::PLUS, "Add schedule")),
                    )
                {
                    self.start_adding(snapshot);
                }
            });

        self.confirm_removal(ui, core);
        self.confirm_power(ui, core);
    }

    fn confirm_power(&mut self, ui: &mut egui::Ui, core: &CoreHandle) {
        let Some((id, operation)) = self.confirm_power.clone() else {
            return;
        };
        let (title, text, icon, confirm) = match operation {
            Operation::Restart => (
                "Restart the computer?",
                "It restarts in 1 minute, closing every program. Work that is not saved by then is lost.",
                icons::RESTART,
                "Restart",
            ),
            Operation::ShutDown => (
                "Shut down the computer?",
                "It shuts down in 1 minute, closing every program. Work that is not saved by then is lost.",
                icons::POWER,
                "Shut down",
            ),
            Operation::Sleep => (
                "Put the computer to sleep?",
                "It sleeps at once. Remote connections to it are lost.",
                icons::POWER,
                "Sleep",
            ),
        };
        let modal = egui::Modal::new(egui::Id::unique("confirm_power")).show(ui.ctx(), |ui| {
            ui.set_max_width(400.0);
            widgets::section_title(ui, title);
            ui.label(text);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::danger_button(ui, true, icon, confirm).clicked() {
                    core.send(Command::RunSchedule(id.clone()));
                    self.confirm_power = None;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_power = None;
                }
            });
        });
        if modal.should_close() {
            self.confirm_power = None;
        }
    }

    fn confirm_removal(&mut self, ui: &mut egui::Ui, core: &CoreHandle) {
        let Some(id) = self.confirm_remove.clone() else {
            return;
        };
        let modal =
            egui::Modal::new(egui::Id::unique("confirm_remove_schedule")).show(ui.ctx(), |ui| {
                ui.set_max_width(400.0);
                widgets::section_title(ui, "Remove schedule?");
                ui.label("It will no longer run. Its past runs stay in the event history.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if widgets::danger_button(ui, true, icons::TRASH, "Remove").clicked() {
                        core.send(Command::RemoveSchedule(id.clone()));
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
        let p = theme::palette(ui);
        let editable = snapshot.config_error.is_none();
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&spec.name).text_style(egui::TextStyle::Heading));
                if !spec.enabled {
                    widgets::pill(ui, "Disabled", p.neutral);
                } else if status.running {
                    widgets::pill(ui, &widgets::icon_text(icons::TIMER, "Running"), p.warning);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::danger_button(ui, editable, icons::TRASH, "")
                        .on_hover_text("Remove")
                        .clicked()
                    {
                        self.confirm_remove = Some(spec.id.clone());
                    }
                    if widgets::button(ui, editable, icons::EDIT, "Edit").clicked() {
                        self.editor = Some(ScheduleEditor::from_spec(spec));
                    }
                    if widgets::primary_button(ui, !status.running, icons::PLAY, "Run now")
                        .on_hover_text("Runs the action once without changing the timetable")
                        .clicked()
                    {
                        if let Some(operation) = spec.action.operation() {
                            self.confirm_power = Some((spec.id.clone(), operation));
                        } else {
                            core.send(Command::RunSchedule(spec.id.clone()));
                        }
                    }
                });
            });
            ui.add_space(6.0);
            widgets::properties(ui, "schedule_status", |ui| {
                widgets::property_label(ui, "When");
                ui.label(describe_when(spec));
                ui.end_row();
                widgets::property_label(ui, "Action");
                ui.label(describe_action(&spec.action, snapshot));
                ui.end_row();
                widgets::property_label(ui, "Next run");
                match (status.enabled, status.next_run) {
                    (false, _) => ui.label("disabled"),
                    (true, Some(t)) => ui.label(format_relative(t)).on_hover_text(format_time(t)),
                    (true, None) => ui.label("none"),
                };
                ui.end_row();
                if let Some(last) = &status.last_run {
                    widgets::property_label(ui, "Last run");
                    ui.horizontal(|ui| {
                        match last.success {
                            Some(true) => widgets::pill(
                                ui,
                                &widgets::icon_text(icons::CHECK, "Succeeded"),
                                p.success,
                            ),
                            Some(false) => widgets::pill(
                                ui,
                                &widgets::icon_text(icons::ERROR, "Failed"),
                                p.danger,
                            ),
                            None => widgets::pill(ui, "Earlier session", p.neutral),
                        };
                        ui.label(&last.summary).on_hover_text(format_time(last.at));
                    });
                    ui.end_row();
                }
                if let Some(problem) = &status.problem {
                    widgets::property_label(ui, "Problem");
                    ui.colored_label(p.danger, problem);
                    ui.end_row();
                }
            });
        });

        if matches!(spec.action, Action::Run { .. }) {
            ui.add_space(12.0);
            widgets::card(ui, |ui| {
                let log = match &mut self.log {
                    Some(log) if log.key() == spec.id.0 => log,
                    _ => self.log.insert(LogView::new(spec.id.0.clone())),
                };
                log.pull(|from| core.schedule_logs_since(&spec.id, from), 1000);
                let file = snapshot.config.logging.app_log_files.then(|| {
                    snapshot
                        .data_dir
                        .join("logs")
                        .join(format!("schedule-{}.log", spec.id))
                });
                log.show(ui, file.as_deref());
            });
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
    if spec.wake {
        text.push_str(", waking the computer");
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
        Action::RestartComputer {} | Action::ShutDownComputer {} | Action::SleepComputer {} => {
            action
                .operation()
                .map_or_else(String::new, |o| o.describe().to_owned())
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
    Computer(Operation),
}

/// In the order offered, in groups.
const ACTION_KINDS: [&[ActionKind]; 3] = [
    &[ActionKind::Start, ActionKind::Stop, ActionKind::Restart],
    &[ActionKind::Run],
    &[
        ActionKind::Computer(Operation::Restart),
        ActionKind::Computer(Operation::ShutDown),
        ActionKind::Computer(Operation::Sleep),
    ],
];

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
    wake: bool,
    /// What to know before choosing the operation; read when it is first shown.
    problems: Option<(Operation, Vec<&'static str>)>,
    errors: Vec<String>,
    picker: Picker<Field>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Program,
    Cwd,
}

impl ScheduleEditor {
    fn new_schedule(snapshot: &Snapshot) -> Self {
        let tomorrow = jiff::ZonedDateTime::now()
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
            wake: false,
            problems: None,
            errors: Vec::new(),
            picker: Picker::default(),
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
            wake: spec.wake,
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
            Action::RestartComputer {} => editor.action = ActionKind::Computer(Operation::Restart),
            Action::ShutDownComputer {} => {
                editor.action = ActionKind::Computer(Operation::ShutDown)
            }
            Action::SleepComputer {} => editor.action = ActionKind::Computer(Operation::Sleep),
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
            wake: false,
            problems: None,
            errors: Vec::new(),
            picker: Picker::default(),
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
            ActionKind::Computer(operation) => Ok(operation.into()),
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
            // Not offered for these.
            missed: match self.action {
                ActionKind::Computer(_) => MissedRun::Skip,
                _ => self.missed,
            },
            time_zone: (!tz.is_empty()).then(|| tz.to_owned()),
            // A run after boot has nothing to wake: the computer has just started.
            wake: self.wake && self.can_wake(),
        };
        errors.extend(schedule::validate(&spec));
        if errors.is_empty() {
            Ok(spec)
        } else {
            Err(errors)
        }
    }

    fn problems(&mut self, operation: Operation) -> &[&'static str] {
        if self
            .problems
            .as_ref()
            .is_none_or(|(of, _)| *of != operation)
        {
            self.problems = Some((operation, operation_problems(operation)));
        }
        self.problems.as_ref().map_or(&[], |(_, found)| found)
    }

    /// Whether waking the computer makes sense for this schedule.
    fn can_wake(&self) -> bool {
        self.when != WhenKind::AfterBoot && self.action != ActionKind::Computer(Operation::Sleep)
    }

    fn show(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) -> EditorAction {
        let mut action = EditorAction::None;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(if self.is_new {
                    "Add schedule"
                } else {
                    "Edit schedule"
                })
                .text_style(egui::TextStyle::Heading),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::primary_button(ui, true, icons::SAVE, "Save").clicked() {
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
        ui.add_space(8.0);
        form::errors(ui, &self.errors);

        egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            form::section(ui, "Schedule", None, |ui| {
                form::row(ui, "Name", None, |ui| form::text(ui, &mut self.name, "e.g. Nightly restart"));
                form::row(ui, "Enabled", None, |ui| {
                    ui.checkbox(&mut self.enabled, "Run on this timetable");
                });
            });

            form::section(ui, "When", None, |ui| {
                form::row(ui, "Repeat", None, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for kind in [WhenKind::Once, WhenKind::Daily, WhenKind::Weekly, WhenKind::Interval, WhenKind::AfterBoot] {
                            ui.selectable_value(&mut self.when, kind, when_label(kind));
                        }
                    });
                });
                match self.when {
                    WhenKind::Once => form::row(ui, "Date and time", Some("YYYY-MM-DD HH:MM"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.date_time).hint_text("2026-12-24 20:00").desired_width(180.0));
                    }),
                    WhenKind::Daily => form::row(ui, "Time", Some("24-hour clock, HH:MM"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.time).hint_text("09:30").desired_width(80.0));
                    }),
                    WhenKind::Weekly => {
                        form::row(ui, "Days", None, |ui| {
                            ui.horizontal(|ui| {
                                for (i, day) in Day::ALL.iter().enumerate() {
                                    ui.toggle_value(&mut self.days[i], day_label(*day));
                                }
                            });
                        });
                        form::row(ui, "Time", Some("24-hour clock, HH:MM"), |ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.time).hint_text("09:30").desired_width(80.0));
                        });
                    }
                    WhenKind::Interval => form::row(ui, "Every", Some("Counted from the previous run."), |ui| {
                        form::seconds(ui, &mut self.every, 1..=31_536_000)
                    }),
                    WhenKind::AfterBoot => form::row(ui, "Delay after boot", Some("Runs once per boot of the computer."), |ui| {
                        form::seconds(ui, &mut self.boot_delay, 0..=86_400)
                    }),
                }
                if !matches!(self.when, WhenKind::Interval | WhenKind::AfterBoot) {
                    form::row(ui, "Time zone", Some("IANA name such as Asia/Tokyo; empty for the system time zone."), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.time_zone).hint_text("system").desired_width(220.0));
                    });
                }
                if self.when != WhenKind::AfterBoot {
                    if !matches!(self.action, ActionKind::Computer(_)) {
                        form::row(ui, "Missed runs", Some("Runs due while Gravekeeper was not running or the computer was asleep."), |ui| {
                            ui.horizontal(|ui| {
                                for m in [MissedRun::Skip, MissedRun::RunOnce] {
                                    ui.selectable_value(&mut self.missed, m, missed_label(m));
                                }
                            });
                        });
                    }
                    if gravekeeper_core::wake::SUPPORTED && self.can_wake() {
                        form::row(ui, "Wake the computer", Some("Wakes the computer from sleep a minute before each run. Gravekeeper must be running, and wake timers allowed on the System page."), |ui| {
                            ui.checkbox(&mut self.wake, "Enabled");
                        });
                    }
                }
            });

            form::section(ui, "Action", None, |ui| {
                form::labelled_row(ui, "Do", None, |ui, label| {
                    egui::ComboBox::from_id_salt("schedule_action")
                        .width(240.0)
                        .selected_text(action_label(self.action))
                        .show_ui(ui, |ui| {
                            for (i, group) in ACTION_KINDS.into_iter().enumerate() {
                                let offered = |kind: &ActionKind| {
                                    power::SUPPORTED
                                        || !matches!(kind, ActionKind::Computer(_))
                                        || *kind == self.action
                                };
                                let kinds: Vec<_> = group.iter().copied().filter(offered).collect();
                                if i > 0 && !kinds.is_empty() {
                                    ui.separator();
                                }
                                for kind in kinds {
                                    ui.selectable_value(&mut self.action, kind, action_label(kind));
                                }
                            }
                        })
                        .response
                        .labelled_by(label);
                    if let ActionKind::Computer(operation) = self.action {
                        widgets::hint(ui, operation_hint(operation));
                        for problem in self.problems(operation) {
                            let p = theme::palette(ui);
                            ui.add(egui::Label::new(RichText::new(widgets::icon_text(icons::WARNING, problem)).color(p.warning)).wrap());
                        }
                    }
                });
                if self.action == ActionKind::Run {
                    form::row(ui, "Program", Some("Run directly, not through a shell."), |ui| {
                        form::path(ui, &mut self.program, "/path/to/program", &mut self.picker, Field::Program, Kind::File);
                    });
                    form::row(ui, "Arguments", Some("One argument per line."), |ui| {
                        form::lines(ui, &mut self.args, "--all")
                    });
                    form::row(ui, "Working directory", Some("Optional."), |ui| {
                        form::path(ui, &mut self.cwd, "optional", &mut self.picker, Field::Cwd, Kind::Folder);
                    });
                    form::row(ui, "Time limit", Some("Stopped if it runs longer."), |ui| {
                        form::seconds(ui, &mut self.timeout, 1..=86_400)
                    });
                } else if !matches!(self.action, ActionKind::Computer(_)) {
                    form::labelled_row(ui, "Application", None, |ui, label| {
                        let name = self
                            .app
                            .as_ref()
                            .and_then(|id| snapshot.spec(id))
                            .map_or("Choose…", |a| a.name.as_str());
                        egui::ComboBox::from_id_salt("schedule_app")
                            .width(240.0)
                            .selected_text(name)
                            .show_ui(ui, |ui| {
                                for app in &snapshot.apps {
                                    ui.selectable_value(&mut self.app, Some(app.id.clone()), &app.name);
                                }
                            })
                            .response
                            .labelled_by(label);
                    });
                }
            });
        });
        action
    }
}

fn operation_hint(operation: Operation) -> &'static str {
    match operation {
        Operation::Restart => {
            "Warns 1 minute ahead, then closes every program; unsaved work is lost. Skipped if the computer started less than 10 minutes ago, or if Gravekeeper is not running at the time."
        }
        Operation::ShutDown => {
            "Warns 1 minute ahead, then closes every program and turns the computer off; unsaved work is lost. Gravekeeper cannot turn it on again. Skipped if the computer started less than 10 minutes ago, or if Gravekeeper is not running at the time."
        }
        Operation::Sleep => {
            "Sleeps at once, without a warning. Schedules that wake the computer wake it again. Skipped if the computer started less than 10 minutes ago, or if Gravekeeper is not running at the time."
        }
    }
}

fn operation_problems(operation: Operation) -> Vec<&'static str> {
    let mut problems = Vec::new();
    match operation {
        // What keeps Gravekeeper, and so its schedules, from running once the computer is back.
        Operation::Restart | Operation::ShutDown => {
            if matches!(os_settings::auto_login(), Ok(None)) {
                problems.push(
                    "Automatic logon is off (System page): Gravekeeper does not run again until someone signs in.",
                );
            }
            if matches!(autostart::is_enabled(), Ok(false)) {
                problems.push(
                    "Launch at login is off (Settings page): Gravekeeper does not start again by itself.",
                );
            }
        }
        Operation::Sleep => match power::sleep_support() {
            SleepSupport::Sleeps => {}
            SleepSupport::ModernStandby => problems.push(
                "This computer uses Modern Standby: its display is turned off, which lets it sleep. Programs that keep it awake can prevent that.",
            ),
            SleepSupport::Unsupported => problems.push("This computer does not support sleep."),
        },
    }
    problems
}

fn when_label(kind: WhenKind) -> &'static str {
    match kind {
        WhenKind::Once => "Once",
        WhenKind::Daily => "Daily",
        WhenKind::Weekly => "Weekly",
        WhenKind::Interval => "Interval",
        WhenKind::AfterBoot => "After boot",
    }
}

fn action_label(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::Start => "Start app",
        ActionKind::Stop => "Stop app",
        ActionKind::Restart => "Restart app",
        ActionKind::Run => "Run program",
        ActionKind::Computer(Operation::Restart) => "Restart computer",
        ActionKind::Computer(Operation::ShutDown) => "Shut down computer",
        ActionKind::Computer(Operation::Sleep) => "Sleep computer",
    }
}

fn missed_label(missed: MissedRun) -> &'static str {
    match missed {
        MissedRun::Skip => "Skip",
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
    fn waking_is_kept_except_after_boot() {
        let mut spec: ScheduleSpec = toml::from_str(
            "id = \"s\"\nname = \"n\"\nwhen = { kind = \"daily\", time = \"09:30:00\" }\naction = { kind = \"run\", program = \"/bin/x\", timeout = 60 }\nwake = true",
        )
        .unwrap();
        assert!(spec.wake);
        assert!(ScheduleEditor::from_spec(&spec).build().unwrap().wake);
        spec.when = When::AfterBoot {
            delay: Duration::from_secs(60),
        };
        assert!(!ScheduleEditor::from_spec(&spec).build().unwrap().wake);
    }

    #[test]
    fn computer_operations_are_never_made_up_for() {
        for operation in [Operation::Restart, Operation::ShutDown, Operation::Sleep] {
            let mut spec: ScheduleSpec = toml::from_str(
                "id = \"s\"\nname = \"n\"\nwhen = { kind = \"daily\", time = \"04:00:00\" }\naction = { kind = \"restart-computer\" }",
            )
            .unwrap();
            spec.action = operation.into();
            let mut editor = ScheduleEditor::from_spec(&spec);
            assert_eq!(editor.build().unwrap(), spec);
            // Chosen for another action before.
            editor.missed = MissedRun::RunOnce;
            editor.wake = true;
            let built = editor.build().unwrap();
            assert_eq!(built.missed, MissedRun::Skip);
            // Waking the computer to put it to sleep is pointless.
            assert_eq!(built.wake, operation != Operation::Sleep);
            editor.when = WhenKind::AfterBoot;
            assert!(editor.build().is_err());
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
