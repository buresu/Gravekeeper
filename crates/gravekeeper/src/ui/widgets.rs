//! Shared building blocks: cards, page headers, status pills, buttons, formatting.

use std::time::{Duration, SystemTime};

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, TextStyle};
use gravekeeper_core::api::{AppState, AppStatus, EventLevel};

use super::icons;
use super::theme::{self, Palette};

pub fn state_color(state: AppState, p: &Palette) -> Color32 {
    match state {
        AppState::Running => p.success,
        AppState::Starting | AppState::Waiting | AppState::Stopping | AppState::Backoff => {
            p.warning
        }
        AppState::GaveUp => p.danger,
        AppState::Stopped | AppState::Disabled => p.neutral,
    }
}

pub fn level_color(level: EventLevel, p: &Palette) -> Color32 {
    match level {
        EventLevel::Info => p.neutral,
        EventLevel::Warning => p.warning,
        EventLevel::Error => p.danger,
    }
}

pub fn level_label(level: EventLevel) -> &'static str {
    match level {
        EventLevel::Info => "Info",
        EventLevel::Warning => "Warning",
        EventLevel::Error => "Error",
    }
}

/// `icon  text`, for buttons and labels.
pub fn icon_text(icon: &str, text: &str) -> String {
    if text.is_empty() {
        icon.to_owned()
    } else {
        format!("{icon}  {text}")
    }
}

/// A rounded, bordered surface for grouping content.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let p = theme::palette(ui);
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Cards side by side with equal heights: the contents are laid out first, then every
/// card's background is painted down to the bottom of the tallest one.
pub fn card_row(ui: &mut egui::Ui, cards: &mut [&mut dyn FnMut(&mut egui::Ui)]) {
    let p = theme::palette(ui);
    ui.columns(cards.len(), |cols| {
        let mut placed = Vec::new();
        for (col, add) in cols.iter_mut().zip(cards.iter_mut()) {
            let background = col.painter().add(egui::Shape::Noop);
            let rect = egui::Frame::new()
                .inner_margin(Margin::same(16))
                .show(col, |ui| {
                    ui.set_width(ui.available_width());
                    add(ui);
                })
                .response
                .rect;
            placed.push((col.painter().clone(), background, rect));
        }
        let bottom = placed
            .iter()
            .map(|(_, _, rect)| rect.bottom())
            .fold(f32::MIN, f32::max);
        for (painter, background, rect) in placed {
            painter.set(
                background,
                egui::epaint::RectShape::new(
                    rect.with_max_y(bottom),
                    CornerRadius::same(10),
                    p.card,
                    Stroke::new(1.0, p.border),
                    egui::StrokeKind::Inside,
                ),
            );
        }
    });
}

/// A card that fills the remaining height, so short or empty lists keep their shape.
pub fn card_fill<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    card(ui, |ui| {
        ui.set_min_height(ui.available_height());
        add(ui)
    })
}

/// Page title with an optional subtitle, and actions aligned to the right.
pub fn page_header(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: Option<&str>,
    actions: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(title).text_style(TextStyle::Name(theme::TITLE.into())));
            if let Some(subtitle) = subtitle {
                ui.label(RichText::new(subtitle).color(theme::palette(ui).text_weak));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), actions);
    });
    ui.add_space(8.0);
}

pub fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).text_style(TextStyle::Name(theme::SECTION.into())));
}

/// Weak, small explanatory text.
pub fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .small()
            .color(theme::palette(ui).text_weak),
    );
}

/// A small rounded label with a tinted background. Measured before it is placed, so it
/// wraps correctly in `horizontal_wrapped` layouts.
pub fn pill(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    let font = egui::FontId::new(11.5, egui::FontFamily::Name(theme::SEMIBOLD.into()));
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let padding = egui::vec2(8.0, 3.0);
    let (rect, response) =
        ui.allocate_exact_size(galley.size() + 2.0 * padding, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(10), color.gamma_multiply(0.16));
        ui.painter().galley(rect.min + padding, galley, color);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
    response
}

/// A small filled circle, e.g. for status (drawn, so it needs no font glyph).
pub fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

pub fn state_pill(ui: &mut egui::Ui, status: &AppStatus) {
    let p = theme::palette(ui);
    let color = state_color(status.state, &p);
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.horizontal(|ui| {
                dot(ui, color);
                ui.label(
                    RichText::new(status.state.label())
                        .small()
                        .strong()
                        .color(color),
                );
            });
        });
    if status.paused {
        pill(ui, &icon_text(icons::PAUSE, "Paused"), p.neutral);
    }
}

/// A plain button with an icon.
pub fn button(ui: &mut egui::Ui, enabled: bool, icon: &str, text: &str) -> egui::Response {
    ui.add_enabled(enabled, egui::Button::new(icon_text(icon, text)))
}

/// The main action of a page, filled with the accent color.
pub fn primary_button(ui: &mut egui::Ui, enabled: bool, icon: &str, text: &str) -> egui::Response {
    let p = theme::palette(ui);
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(icon_text(icon, text)).color(Color32::WHITE))
            .fill(p.accent),
    )
}

/// A destructive action, with red text.
pub fn danger_button(ui: &mut egui::Ui, enabled: bool, icon: &str, text: &str) -> egui::Response {
    let p = theme::palette(ui);
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(icon_text(icon, text)).color(p.danger)),
    )
}

/// A centered message for empty lists, with an optional call to action. Returns whether
/// the action was clicked.
pub fn empty_state(
    ui: &mut egui::Ui,
    icon: &str,
    title: &str,
    body: &str,
    action: Option<(&str, &str)>,
) -> bool {
    let mut clicked = false;
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.25).max(24.0));
        let p = theme::palette(ui);
        ui.label(RichText::new(icon).size(40.0).color(p.text_weak));
        ui.add_space(6.0);
        ui.label(RichText::new(title).text_style(TextStyle::Name(theme::SECTION.into())));
        ui.label(RichText::new(body).color(p.text_weak));
        if let Some((icon, label)) = action {
            ui.add_space(8.0);
            clicked = primary_button(ui, true, icon, label).clicked();
        }
    });
    clicked
}

/// A number with a label, for the dashboard.
pub fn stat_tile(ui: &mut egui::Ui, icon: &str, label: &str, value: usize, color: Color32) {
    let p = theme::palette(ui);
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).size(22.0).color(color));
                ui.vertical(|ui| {
                    ui.label(RichText::new(value.to_string()).size(24.0).strong());
                    ui.label(RichText::new(label).color(p.text_weak));
                });
            });
        });
}

/// A sidebar navigation entry.
pub fn nav_item(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    selected: bool,
    badge: usize,
) -> egui::Response {
    let p = theme::palette(ui);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(6), p.accent.gamma_multiply(0.18));
    } else if response.hovered() {
        painter.rect_filled(
            rect,
            CornerRadius::same(6),
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
    }
    let color = if selected {
        p.accent
    } else {
        ui.visuals().text_color()
    };
    painter.text(
        rect.left_center() + egui::vec2(12.0, 0.0),
        egui::Align2::LEFT_CENTER,
        icon,
        egui::FontId::proportional(17.0),
        color,
    );
    painter.text(
        rect.left_center() + egui::vec2(38.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        color,
    );
    if badge > 0 {
        let center = rect.right_center() - egui::vec2(18.0, 0.0);
        painter.circle_filled(center, 9.0, p.danger);
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            badge.min(99).to_string(),
            egui::FontId::proportional(11.0),
            Color32::WHITE,
        );
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, label));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A selectable row in a list (applications, schedules, webhooks). `label` is what screen
/// readers announce.
pub fn list_row(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    selected: bool,
    add: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let p = theme::palette(ui);
    let fill = if selected {
        p.accent.gamma_multiply(0.16)
    } else {
        Color32::TRANSPARENT
    };
    let inner = egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
    let rect = inner.response.rect;
    let response = ui.interact(
        rect,
        egui::Id::unique(("list_row", id)),
        egui::Sense::click(),
    );
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::Role::ListItem, true, selected, label));
    if response.hovered() && !selected {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(1.0, p.border),
            egui::StrokeKind::Inside,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A label/value grid.
pub fn properties(ui: &mut egui::Ui, id: &str, rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([20.0, 8.0])
        .min_col_width(120.0)
        .striped(false)
        .show(ui, rows);
}

pub fn property_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::palette(ui).text_weak));
}

/// A full-width colored box for problems that need the user's attention.
pub fn banner(ui: &mut egui::Ui, color: Color32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// Local date and time, e.g. `2026-10-19 03:12:45`.
pub fn format_time(t: SystemTime) -> String {
    jiff::Timestamp::try_from(t)
        .map(|ts| {
            ts.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| "?".into())
}

/// `5s ago` for the past, `in 5s` for the future.
pub fn format_relative(t: SystemTime) -> String {
    match t.duration_since(SystemTime::now()) {
        Ok(ahead) => format!("in {}", format_duration(ahead)),
        Err(err) => format!("{} ago", format_duration(err.duration())),
    }
}

pub fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 if s.is_multiple_of(60) => format!("{}m", s / 60),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        3600..86400 => format!("{}h {}m", s / 3600, s / 60 % 60),
        _ => format!("{}d {}h", s / 86400, s / 3600 % 24),
    }
}

/// `1.5 GiB`
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A small area chart of percentages (0-100), oldest value on the left.
pub fn sparkline(ui: &mut egui::Ui, values: &[f32], color: Color32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 40.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(4), color.gamma_multiply(0.06));
    if values.len() < 2 {
        return;
    }
    let step = rect.width() / (values.len() - 1) as f32;
    let points: Vec<egui::Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let y = rect.bottom() - rect.height() * (v.clamp(0.0, 100.0) / 100.0);
            egui::pos2(rect.left() + i as f32 * step, y)
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(1.5, color)));
}

/// A thin horizontal usage bar (0-100%).
pub fn usage_bar(ui: &mut egui::Ui, percent: f32, color: Color32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 8.0), egui::Sense::hover());
    let p = theme::palette(ui);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(4), p.border);
    let mut filled = rect;
    filled.set_width(rect.width() * (percent / 100.0).clamp(0.0, 1.0));
    ui.painter()
        .rect_filled(filled, CornerRadius::same(4), color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_compact() {
        assert_eq!(format_duration(Duration::from_secs(5)), "5s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 5s");
        assert_eq!(format_duration(Duration::from_secs(7260)), "2h 1m");
        assert_eq!(format_duration(Duration::from_secs(90000)), "1d 1h");
    }

    #[test]
    fn bytes_are_readable() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn relative_times_have_a_direction() {
        let now = SystemTime::now();
        assert!(format_relative(now - Duration::from_secs(30)).ends_with("ago"));
        assert!(format_relative(now + Duration::from_secs(30)).starts_with("in "));
    }
}
