mod apps;
mod dashboard;
mod editor;
mod events;
mod logview;
mod schedules;
mod settings;
mod webhooks;
mod widgets;

use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use eframe::egui::{self, ViewportCommand};
use gravekeeper_core::api::AppState;
use gravekeeper_core::config::AppId;
use gravekeeper_core::schedule::ScheduleId;
use gravekeeper_core::webhook::WebhookId;
use gravekeeper_core::{Command, CoreHandle};
use serde::{Deserialize, Serialize};

use crate::shell::{CloseBehavior, Shell};

const STORAGE_KEY: &str = "gravekeeper.ui";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum Tab {
    #[default]
    Dashboard,
    Apps,
    Schedules,
    Notifications,
    Events,
    Settings,
}

/// UI state restored when the window is reopened (and across restarts).
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Persisted {
    tab: Tab,
    selected: Option<AppId>,
    schedule: Option<ScheduleId>,
    webhook: Option<WebhookId>,
}

pub struct GravekeeperApp<'a> {
    shell: Arc<Shell>,
    core: &'a CoreHandle,
    close: CloseBehavior,
    process_started: Instant,
    tab: Tab,
    apps: apps::AppsView,
    schedules: schedules::SchedulesView,
    webhooks: webhooks::WebhooksView,
    events: events::EventsView,
    settings: settings::SettingsView,
    confirm_quit: bool,
}

/// The startup time is only meaningful for the first window of the process.
static FIRST_FRAME: Once = Once::new();

impl<'a> GravekeeperApp<'a> {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        shell: Arc<Shell>,
        core: &'a CoreHandle,
        close: CloseBehavior,
        process_started: Instant,
    ) -> Self {
        let persisted: Persisted = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();
        let mut apps = apps::AppsView::default();
        apps.selected = persisted.selected;
        Self {
            shell,
            core,
            close,
            process_started,
            tab: persisted.tab,
            apps,
            schedules: schedules::SchedulesView::with_selection(persisted.schedule),
            webhooks: webhooks::WebhooksView::with_selection(persisted.webhook),
            events: events::EventsView::default(),
            settings: settings::SettingsView::default(),
            confirm_quit: false,
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.shell.request_quit();
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    fn confirm_quit(&mut self, ctx: &egui::Context, running: usize) {
        let modal = egui::Modal::new(egui::Id::unique("confirm_quit")).show(ctx, |ui| {
            ui.heading("Quit Gravekeeper?");
            ui.label(format!(
                "{running} running application(s) will be stopped. To keep them supervised, \
                 close the window instead; Gravekeeper keeps running in the tray."
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Quit and stop applications").clicked() {
                    self.confirm_quit = false;
                    self.quit(ui.ctx());
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_quit = false;
                }
            });
        });
        if modal.should_close() {
            self.confirm_quit = false;
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
        // Relative times and live output change without engine notifications.
        ui.ctx().request_repaint_after(Duration::from_millis(500));

        let snapshot = self.core.snapshot();
        let running = snapshot
            .apps
            .iter()
            .filter(|a| a.state.is_active() || a.state == AppState::Backoff)
            .count();

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Gravekeeper");
                ui.separator();
                for (tab, label) in [
                    (Tab::Dashboard, "Dashboard"),
                    (Tab::Apps, "Applications"),
                    (Tab::Schedules, "Schedules"),
                    (Tab::Notifications, "Notifications"),
                    (Tab::Events, "Events"),
                    (Tab::Settings, "Settings"),
                ] {
                    ui.selectable_value(&mut self.tab, tab, label);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Quit").clicked() {
                        if running > 0 {
                            self.confirm_quit = true;
                        } else {
                            self.quit(ui.ctx());
                        }
                    }
                    if self.close != CloseBehavior::Quit
                        && ui
                            .button("Close to tray")
                            .on_hover_text("Applications stay supervised")
                            .clicked()
                    {
                        ui.send_viewport_cmd(ViewportCommand::Close);
                    }
                });
            });
        });

        if let Some(error) = &snapshot.config_error {
            egui::Panel::top("config_error").show(ui, |ui| {
                let color = ui.visuals().error_fg_color;
                widgets::banner(ui, color, |ui| {
                    ui.strong("The configuration file could not be loaded. Changes are disabled until it is fixed.");
                    ui.label(error);
                    ui.horizontal(|ui| {
                        ui.monospace(snapshot.config_path.display().to_string());
                        if ui.button("Reload").clicked() {
                            self.core.send(Command::ReloadConfig);
                        }
                    });
                });
            });
        }

        match self.tab {
            Tab::Apps => self.apps.show(ui, self.core, &snapshot),
            Tab::Schedules => self.schedules.show(ui, self.core, &snapshot),
            Tab::Notifications => self.webhooks.show(ui, self.core, &snapshot),
            tab => {
                egui::CentralPanel::default().show(ui, |ui| match tab {
                    Tab::Dashboard => {
                        if let Some(id) = dashboard::show(ui, &snapshot) {
                            self.apps.select(id);
                            self.tab = Tab::Apps;
                        }
                    }
                    Tab::Events => self.events.show(ui, &snapshot),
                    Tab::Settings => self.settings.show(ui, self.core, &snapshot),
                    Tab::Apps | Tab::Schedules | Tab::Notifications => unreachable!(),
                });
            }
        }

        if self.confirm_quit {
            self.confirm_quit(ui.ctx(), running);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(
            storage,
            STORAGE_KEY,
            &Persisted {
                tab: self.tab,
                selected: self.apps.selected.clone(),
                schedule: self.schedules.selected().cloned(),
                webhook: self.webhooks.selected().cloned(),
            },
        );
    }
}

/// Shown instead of the main window when another instance is already supervising.
pub struct AlreadyRunning;

impl eframe::App for AlreadyRunning {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Gravekeeper is already running");
            ui.label("Open it from its icon in the system tray.");
            ui.add_space(8.0);
            if ui.button("OK").clicked() {
                ui.send_viewport_cmd(ViewportCommand::Close);
            }
        });
    }
}
