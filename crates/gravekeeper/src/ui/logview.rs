//! Output viewer for applications and schedules, fetched incrementally from the engine.

use std::collections::VecDeque;
use std::path::Path;

use eframe::egui::{self, CornerRadius, Margin, RichText};
use gravekeeper_core::process::{LogLine, Stream};

use super::{icons, picker, theme, widgets};

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

    /// `file` is the log file on disk, if output is also written there.
    pub fn show(&mut self, ui: &mut egui::Ui, file: Option<&Path>) {
        let p = theme::palette(ui);
        ui.horizontal(|ui| {
            widgets::section_title(ui, &widgets::icon_text(icons::TERMINAL, "Output"));
            ui.add_space(8.0);
            ui.toggle_value(&mut self.stdout, "stdout");
            ui.toggle_value(&mut self.stderr, RichText::new("stderr").color(p.warning));
            ui.toggle_value(&mut self.meta, "events")
                .on_hover_text("Starts, exits and restarts noted by Gravekeeper");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(file) = file
                    && widgets::button(ui, true, icons::FOLDER_OPEN, "")
                        .on_hover_text(format!("Open the log folder ({})", file.display()))
                        .clicked()
                {
                    picker::reveal(file);
                }
                if widgets::button(ui, true, icons::CLEAR, "")
                    .on_hover_text("Clear this view (the log file is kept)")
                    .clicked()
                {
                    self.lines.clear();
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text(widgets::icon_text(icons::SEARCH, "Filter"))
                        .desired_width(180.0),
                );
            });
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

        let meta_color = p.text_weak;
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::Frame::new()
            .fill(ui.visuals().code_bg_color)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                if visible.is_empty() {
                    // Take the space the lines will fill, so the frame does not grow later.
                    ui.set_min_size(ui.available_size());
                    ui.label(
                        RichText::new(if self.lines.is_empty() {
                            "No output yet."
                        } else {
                            "No lines match the filter."
                        })
                        .color(p.text_weak),
                    );
                    return;
                }
                egui::ScrollArea::both()
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    .show_rows(ui, row_height, visible.len(), |ui, range| {
                        for line in &visible[range] {
                            let text = RichText::new(&line.text).monospace();
                            let text = match line.stream {
                                Stream::Stdout => text,
                                Stream::Stderr => text.color(p.warning),
                                Stream::Meta => text.color(meta_color).italics(),
                            };
                            ui.add(egui::Label::new(text).extend());
                        }
                    });
            });
    }
}
