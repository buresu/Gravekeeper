//! Settings of the computer itself (as opposed to Gravekeeper's own, in `settings.rs`).
//! Values are read from the OS, never stored in the configuration; machine-wide changes ask
//! for administrator rights through `elevate`.

use std::sync::mpsc::{self, TryRecvError};

use eframe::egui::{self, RichText};

use super::{form, theme, widgets};
use crate::elevate::{self, Task};
use crate::os_settings;

#[derive(Default)]
pub struct SystemView {
    /// Read from the OS once, and again after each change.
    automatic_updates: Option<Result<bool, String>>,
    /// A change waiting for administrator approval and the elevated helper.
    elevation: Option<mpsc::Receiver<Result<(), elevate::Error>>>,
    error: Option<String>,
}

impl SystemView {
    /// Picks up the result of a finished elevated change.
    fn poll_elevation(&mut self) {
        let Some(rx) = &self.elevation else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(elevate::Error::Failed(
                "the change stopped unexpectedly".into(),
            )),
        };
        self.elevation = None;
        // Show what the OS has now, whatever happened.
        self.automatic_updates = None;
        match result {
            Ok(()) | Err(elevate::Error::Cancelled) => self.error = None,
            Err(err) => self.error = Some(format!("Could not change the setting: {err}")),
        }
    }

    fn elevate(&mut self, ctx: &egui::Context, tasks: Vec<Task>) {
        let ctx = ctx.clone();
        self.elevation = Some(elevate::spawn(tasks, move || ctx.request_repaint()));
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.poll_elevation();
        let busy = self.elevation.is_some();

        widgets::page_header(
            ui,
            "System",
            Some(
                "Settings of this computer. Changes take effect at once and ask for administrator rights.",
            ),
            |ui| {
                if busy {
                    ui.label(
                        RichText::new("Waiting for administrator approval…")
                            .color(theme::palette(ui).warning),
                    );
                }
            },
        );
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
                form::section(ui, "Updates", None, |ui| {
                    form::row(
                        ui,
                        "Automatic updates",
                        Some("When off, the OS no longer installs updates on its own; you can still update by hand."),
                        |ui| {
                            let state = self
                                .automatic_updates
                                .get_or_insert_with(|| {
                                    os_settings::automatic_updates().map_err(|e| e.to_string())
                                })
                                .clone();
                            match state {
                                Ok(mut enabled) => {
                                    if ui
                                        .add_enabled(
                                            !busy,
                                            egui::Checkbox::new(&mut enabled, "Enabled"),
                                        )
                                        .changed()
                                    {
                                        self.elevate(
                                            ui.ctx(),
                                            vec![Task::AutomaticUpdates { enabled }],
                                        );
                                    }
                                }
                                Err(err) => {
                                    ui.label(format!("Unavailable: {err}"));
                                }
                            }
                        },
                    );
                });
            });
    }
}
