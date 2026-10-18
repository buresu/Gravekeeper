use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use eframe::egui::{self, ViewportCommand};
use gravekeeper_core::CoreHandle;

use crate::shell::{CloseBehavior, Shell};

pub struct GravekeeperApp<'a> {
    shell: Arc<Shell>,
    core: &'a CoreHandle,
    close: CloseBehavior,
    renderer: &'static str,
    process_started: Instant,
}

/// The startup time is only meaningful for the first window of the process.
static FIRST_FRAME: Once = Once::new();

impl<'a> GravekeeperApp<'a> {
    pub fn new(
        shell: Arc<Shell>,
        core: &'a CoreHandle,
        close: CloseBehavior,
        renderer: &'static str,
        process_started: Instant,
    ) -> Self {
        Self {
            shell,
            core,
            close,
            renderer,
            process_started,
        }
    }
}

impl eframe::App for GravekeeperApp<'_> {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if !close_requested || self.shell.quit_requested() {
            return;
        }
        match self.close {
            CloseBehavior::Hide => {
                tracing::info!("window hidden to tray");
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            }
            CloseBehavior::Destroy => tracing::info!("window closed; staying in tray"),
            CloseBehavior::Quit => self.shell.request_quit(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        FIRST_FRAME.call_once(|| {
            tracing::info!(elapsed = ?self.process_started.elapsed(), "first frame");
        });

        let snapshot = self.core.snapshot();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Gravekeeper");
            ui.label("Phase 0 technology check");
            ui.separator();

            egui::Grid::new("status")
                .num_columns(2)
                .spacing([24.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Engine uptime");
                    ui.label(format_duration(snapshot.uptime));
                    ui.end_row();
                    ui.label("Platform");
                    ui.label(platform_description());
                    ui.end_row();
                    ui.label("Renderer");
                    ui.label(self.renderer);
                    ui.end_row();
                    ui.label("On window close");
                    ui.label(match self.close {
                        CloseBehavior::Hide => "Hide to tray",
                        CloseBehavior::Destroy => "Close window, keep running in tray",
                        CloseBehavior::Quit => "Quit (no tray available)",
                    });
                    ui.end_row();
                    ui.label("Font fallback");
                    ui.label("日本語 / 中文 / 한국어");
                    ui.end_row();
                });

            ui.separator();
            ui.horizontal(|ui| {
                if self.close != CloseBehavior::Quit && ui.button("Close to tray").clicked() {
                    ui.send_viewport_cmd(ViewportCommand::Close);
                }
                if ui.button("Quit").clicked() {
                    self.shell.request_quit();
                    ui.send_viewport_cmd(ViewportCommand::Close);
                }
            });
        });
    }
}

fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn platform_description() -> String {
    let mut text = format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH);
    if cfg!(target_os = "linux") {
        let session = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            "Wayland"
        } else if std::env::var_os("DISPLAY").is_some() {
            "X11"
        } else {
            "unknown display"
        };
        text.push_str(" / ");
        text.push_str(session);
    }
    text
}
