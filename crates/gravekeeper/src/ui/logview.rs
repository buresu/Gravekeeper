//! Output viewer for applications and schedules, fetched incrementally from the engine.

use std::collections::VecDeque;

use eframe::egui::{self, RichText};
use gravekeeper_core::api::AppState;
use gravekeeper_core::process::{LogLine, Stream};

use super::widgets;

/// The output of one application or schedule.
pub struct LogView {
    key: String,
    lines: VecDeque<LogLine>,
    next_seq: u64,
    stdout: bool,
    stderr: bool,
    meta: bool,
    filter: String,
}

impl LogView {
    pub fn new(key: String) -> Self {
        Self {
            key,
            lines: VecDeque::new(),
            next_seq: 0,
            stdout: true,
            stderr: true,
            meta: true,
            filter: String::new(),
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// Appends lines from `fetch(next_seq)`, keeping at most `capacity`.
    pub fn pull(&mut self, fetch: impl FnOnce(u64) -> Vec<LogLine>, capacity: usize) {
        for line in fetch(self.next_seq) {
            self.next_seq = line.seq + 1;
            if self.lines.len() >= capacity.max(1) {
                self.lines.pop_front();
            }
            self.lines.push_back(line);
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Output");
            ui.checkbox(&mut self.stdout, "stdout");
            ui.checkbox(&mut self.stderr, "stderr");
            ui.checkbox(&mut self.meta, "events");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Filter")
                    .desired_width(160.0),
            );
            if ui
                .button("Clear")
                .on_hover_text("Clears this view only")
                .clicked()
            {
                self.lines.clear();
            }
        });

        let filter = self.filter.to_lowercase();
        let visible: Vec<&LogLine> = self
            .lines
            .iter()
            .filter(|l| match l.stream {
                Stream::Stdout => self.stdout,
                Stream::Stderr => self.stderr,
                Stream::Meta => self.meta,
            })
            .filter(|l| filter.is_empty() || l.text.to_lowercase().contains(&filter))
            .collect();

        let dark = ui.visuals().dark_mode;
        let stderr_color = widgets::state_color(AppState::Backoff, dark);
        let meta_color = ui.visuals().weak_text_color();
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(4.0)
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    .show_rows(ui, row_height, visible.len(), |ui, range| {
                        for line in &visible[range] {
                            let text = RichText::new(&line.text).monospace();
                            let text = match line.stream {
                                Stream::Stdout => text,
                                Stream::Stderr => text.color(stderr_color),
                                Stream::Meta => text.color(meta_color).italics(),
                            };
                            ui.add(egui::Label::new(text).extend());
                        }
                    });
            });
    }
}
