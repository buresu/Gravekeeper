//! Small display helpers shared by the views.

use std::time::{Duration, SystemTime};

use eframe::egui::{self, Color32, RichText};
use gravekeeper_core::api::{AppState, AppStatus, EventLevel};

pub fn state_color(state: AppState, dark: bool) -> Color32 {
    let (light, dark_c) = match state {
        AppState::Running => (
            Color32::from_rgb(0x1e, 0x8e, 0x3e),
            Color32::from_rgb(0x5c, 0xd0, 0x7a),
        ),
        AppState::Starting | AppState::Waiting | AppState::Stopping => (
            Color32::from_rgb(0x9a, 0x6b, 0x00),
            Color32::from_rgb(0xe8, 0xc0, 0x4a),
        ),
        AppState::Backoff => (
            Color32::from_rgb(0xc2, 0x5a, 0x00),
            Color32::from_rgb(0xff, 0x9d, 0x4d),
        ),
        AppState::GaveUp => (
            Color32::from_rgb(0xc5, 0x22, 0x1f),
            Color32::from_rgb(0xff, 0x6b, 0x66),
        ),
        AppState::Stopped | AppState::Disabled => (Color32::GRAY, Color32::GRAY),
    };
    if dark { dark_c } else { light }
}

pub fn level_color(level: EventLevel, dark: bool) -> Color32 {
    match level {
        EventLevel::Info => {
            if dark {
                Color32::LIGHT_GRAY
            } else {
                Color32::DARK_GRAY
            }
        }
        EventLevel::Warning => state_color(AppState::Backoff, dark),
        EventLevel::Error => state_color(AppState::GaveUp, dark),
    }
}

/// A colored dot followed by the state name.
pub fn state_badge(ui: &mut egui::Ui, status: &AppStatus) {
    let color = state_color(status.state, ui.visuals().dark_mode);
    ui.label(RichText::new("●").color(color));
    let mut label = status.state.label().to_owned();
    if status.paused {
        label.push_str(" (paused)");
    }
    ui.label(RichText::new(label).color(color));
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

/// A full-width colored box for problems that need the user's attention.
pub fn banner(ui: &mut egui::Ui, color: Color32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.15))
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(4.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
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
    fn relative_times_have_a_direction() {
        let now = SystemTime::now();
        assert!(format_relative(now - Duration::from_secs(30)).ends_with("ago"));
        assert!(format_relative(now + Duration::from_secs(30)).starts_with("in "));
    }
}
