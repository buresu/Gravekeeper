//! Form layout shared by the editors: labelled rows with help text, path inputs with native
//! dialogs, and duration inputs.

use std::path::Path;

use eframe::egui::{self, RichText};

use super::picker::{Kind, Picker};
use super::{icons, theme, widgets};

/// A titled card holding a two-column form.
pub fn section(
    ui: &mut egui::Ui,
    title: &str,
    description: Option<&str>,
    rows: impl FnOnce(&mut egui::Ui),
) {
    widgets::card(ui, |ui| {
        widgets::section_title(ui, title);
        if let Some(description) = description {
            widgets::hint(ui, description);
        }
        ui.add_space(6.0);
        egui::Grid::new(title)
            .num_columns(2)
            .spacing([20.0, 12.0])
            .min_col_width(170.0)
            .striped(false)
            .show(ui, rows);
    });
    ui.add_space(12.0);
}

/// One row: a label on the left, the control (and optional help below it) on the right.
pub fn row(
    ui: &mut egui::Ui,
    label: &str,
    help: Option<&str>,
    control: impl FnOnce(&mut egui::Ui),
) {
    labelled_row(ui, label, help, |ui, _| control(ui));
}

/// Like [`row`], passing the label's id so controls without their own text (combo boxes)
/// can be `labelled_by` it for screen readers.
pub fn labelled_row(
    ui: &mut egui::Ui,
    label: &str,
    help: Option<&str>,
    control: impl FnOnce(&mut egui::Ui, egui::Id),
) {
    let label = ui.label(label);
    ui.data_mut(|d| d.insert_temp(row_label_key(), label.id));
    ui.vertical(|ui| {
        control(ui, label.id);
        if let Some(help) = help {
            widgets::hint(ui, help);
        }
    });
    ui.data_mut(|d| d.remove::<egui::Id>(row_label_key()));
    ui.end_row();
}

fn row_label_key() -> egui::Id {
    egui::Id::unique("gravekeeper.form.row_label")
}

/// A numeric input, announced by screen readers with the label of the row it is in.
pub fn number(ui: &mut egui::Ui, drag: egui::DragValue<'_>) -> egui::Response {
    let response = ui.add(drag);
    match ui.data(|d| d.get_temp::<egui::Id>(row_label_key())) {
        Some(label) => response.labelled_by(label),
        None => response,
    }
}

pub fn text(ui: &mut egui::Ui, value: &mut String, hint: &str) {
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(f32::INFINITY),
    );
}

pub fn lines(ui: &mut egui::Ui, value: &mut String, hint: &str) {
    ui.add(
        egui::TextEdit::multiline(value)
            .hint_text(hint)
            .desired_rows(3)
            .code_editor()
            .desired_width(f32::INFINITY),
    );
}

/// A text field for a path with a "Browse…" button opening the native dialog. Returns true
/// when a path was picked this frame.
pub fn path<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    value: &mut String,
    hint: &str,
    picker: &mut Picker<T>,
    tag: T,
    kind: Kind,
) -> bool {
    let mut picked = false;
    if let Some(path) = picker.take(tag) {
        *value = path.to_string_lossy().into_owned();
        picked = true;
    }
    let (label, title) = match kind {
        Kind::File => ("Browse for a program", "Choose a program"),
        Kind::Folder => ("Choose a folder", "Choose a folder"),
    };
    ui.horizontal(|ui| {
        // Right to left: the icon button sits at the end, the field takes the rest.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let button = ui
                .add_enabled(!picker.is_open(), egui::Button::new(icons::FOLDER_OPEN))
                .on_hover_text(label);
            button.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, !picker.is_open(), label)
            });
            if button.clicked() {
                let current = value.trim();
                let start = (!current.is_empty()).then(|| Path::new(current));
                picker.open(ui.ctx(), tag, kind, title, start);
            }
            ui.add(
                egui::TextEdit::singleline(value)
                    .hint_text(hint)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(ui.available_width()),
            );
        });
    });
    if picker.is_open() {
        widgets::hint(ui, "Waiting for the file dialog…");
    }
    picked
}

/// Whole seconds, shown with a unit.
pub fn seconds(ui: &mut egui::Ui, value: &mut u64, range: std::ops::RangeInclusive<u64>) {
    ui.horizontal(|ui| {
        number(ui, egui::DragValue::new(value).range(range).speed(1.0));
        ui.label(RichText::new("seconds").color(theme::palette(ui).text_weak));
    });
}

/// Validation errors, shown above the form's buttons.
pub fn errors(ui: &mut egui::Ui, errors: &[String]) {
    if errors.is_empty() {
        return;
    }
    let color = theme::palette(ui).danger;
    widgets::banner(ui, color, |ui| {
        for error in errors {
            ui.label(RichText::new(widgets::icon_text(icons::WARNING_CIRCLE, error)).color(color));
        }
    });
    ui.add_space(8.0);
}
