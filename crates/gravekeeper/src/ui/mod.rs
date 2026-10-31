mod apps;
mod dashboard;
mod editor;
mod events;
mod form;
pub mod icons;
mod logview;
mod picker;
mod schedules;
#[cfg(test)]
mod screenshots;
mod settings;
mod system;
pub mod theme;
mod webhooks;
mod widgets;

use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use eframe::egui::{self, Margin, RichText, ViewportCommand};
use gravekeeper_core::api::AppState;
use gravekeeper_core::config::AppId;
use gravekeeper_core::schedule::ScheduleId;
use gravekeeper_core::webhook::WebhookId;
use gravekeeper_core::{Command, CoreHandle, Snapshot};
use serde::{Deserialize, Serialize};

use crate::shell::{CloseBehavior, Shell};
use crate::tray::WindowItem;

const STORAGE_KEY: &str = "gravekeeper.ui";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Tab {
    #[default]
    Dashboard,
    Apps,
    Schedules,
    Notifications,
    Events,
    System,
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
    shell: Option<Arc<Shell>>,
    core: &'a CoreHandle,
    close: CloseBehavior,
    process_started: Instant,
    tab: Tab,
    apps: apps::AppsView,
    schedules: schedules::SchedulesView,
    webhooks: webhooks::WebhooksView,
    events: events::EventsView,
    system: system::SystemView,
    settings: settings::SettingsView,
    confirm_quit: bool,
    logo: Option<egui::TextureHandle>,
    /// The tray's show/hide entry, relabeled when the window is shown or hidden.
    tray_window_item: Option<WindowItem>,
}

/// The startup time is only meaningful for the first window of the process.
static FIRST_FRAME: Once = Once::new();

impl<'a> GravekeeperApp<'a> {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        shell: Arc<Shell>,
        core: &'a CoreHandle,
        close: CloseBehavior,
        start_hidden: bool,
        tray_window_item: Option<WindowItem>,
        process_started: Instant,
    ) -> Self {
        theme::install(&cc.egui_ctx);
        let persisted: Persisted = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();
        let mut app = Self::detached(core, close, process_started);
        shell.set_window_shown(!start_hidden);
        app.shell = Some(shell);
        app.tray_window_item = tray_window_item;
        app.restore(persisted);
        if start_hidden {
            // eframe shows the window once its first frame is painted, whatever the viewport
            // builder says, so hide it again right after. This command is processed after
            // that show.
            cc.egui_ctx
                .send_viewport_cmd(ViewportCommand::Visible(false));
        }
        app
    }

    /// Without a window shell (no tray, no viewport commands); also used by rendering tests.
    fn detached(core: &'a CoreHandle, close: CloseBehavior, process_started: Instant) -> Self {
        Self {
            shell: None,
            core,
            close,
            process_started,
            tab: Tab::default(),
            apps: apps::AppsView::default(),
            schedules: schedules::SchedulesView::default(),
            webhooks: webhooks::WebhooksView::default(),
            events: events::EventsView::default(),
            system: system::SystemView::default(),
            settings: settings::SettingsView::default(),
            confirm_quit: false,
            logo: None,
            tray_window_item: None,
        }
    }

    fn restore(&mut self, persisted: Persisted) {
        self.tab = persisted.tab;
        self.apps.selected = persisted.selected;
        self.schedules = schedules::SchedulesView::with_selection(persisted.schedule);
        self.webhooks = webhooks::WebhooksView::with_selection(persisted.webhook);
    }

    #[cfg(test)]
    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
    }

    fn quit_requested(&self) -> bool {
        self.shell.as_ref().is_some_and(|s| s.quit_requested())
    }

    fn handle_close_request(&self, ctx: &egui::Context) {
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if !close_requested || self.quit_requested() {
            return;
        }
        match self.close {
            CloseBehavior::Hide => {
                tracing::info!("window hidden to tray");
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                if let Some(shell) = &self.shell {
                    shell.set_window_shown(false);
                }
            }
            CloseBehavior::Destroy => tracing::info!("window closed; staying in tray"),
            CloseBehavior::Quit => {
                if let Some(shell) = &self.shell {
                    shell.request_quit();
                }
            }
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        if let Some(shell) = &self.shell {
            shell.request_quit();
        }
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    fn confirm_quit(&mut self, ctx: &egui::Context, running: usize) {
        let modal = egui::Modal::new(egui::Id::unique("confirm_quit")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            widgets::section_title(ui, "Quit Gravekeeper?");
            ui.label(format!(
                "{running} running application(s) will be stopped. To keep them supervised, \
                 close the window instead; Gravekeeper keeps running in the tray."
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::danger_button(ui, true, icons::POWER, "Quit and stop applications")
                    .clicked()
                {
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

    fn sidebar(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot, running: usize) {
        let p = theme::palette(ui);
        let logo = self
            .logo
            .get_or_insert_with(|| {
                ui.ctx().load_texture(
                    "logo",
                    egui::ColorImage::from_rgba_unmultiplied([64, 64], &crate::icon::rgba(64)),
                    egui::TextureOptions::LINEAR,
                )
            })
            .clone();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(28.0, 28.0)));
            ui.vertical(|ui| {
                ui.label(RichText::new("Gravekeeper").text_style(egui::TextStyle::Heading));
                ui.label(
                    RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .small()
                        .color(p.text_weak),
                );
            });
        });
        ui.add_space(14.0);

        let gave_up = snapshot
            .apps
            .iter()
            .filter(|a| a.state == AppState::GaveUp)
            .count();
        let failed_schedules = snapshot
            .schedules
            .iter()
            .filter(|s| {
                s.last_run
                    .as_ref()
                    .is_some_and(|r| r.success == Some(false))
            })
            .count();
        for (tab, icon, label, badge) in [
            (Tab::Dashboard, icons::DASHBOARD, "Dashboard", 0),
            (Tab::Apps, icons::APPS, "Applications", gave_up),
            (
                Tab::Schedules,
                icons::CALENDAR,
                "Schedules",
                failed_schedules,
            ),
            (Tab::Notifications, icons::BELL, "Notifications", 0),
            (Tab::Events, icons::LIST, "Events", 0),
            (Tab::System, icons::CPU, "System", 0),
            (Tab::Settings, icons::GEAR, "Settings", 0),
        ] {
            if widgets::nav_item(ui, icon, label, self.tab == tab, badge).clicked() {
                self.tab = tab;
            }
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.add_space(2.0);
            // Two equal buttons spanning the sidebar; Quit alone when there is no tray.
            let can_hide = self.close != CloseBehavior::Quit;
            let gap = ui.spacing().item_spacing.x;
            let width = if can_hide {
                (ui.available_width() - gap) / 2.0
            } else {
                ui.available_width()
            };
            let size = egui::vec2(width, 30.0);
            ui.horizontal(|ui| {
                if can_hide
                    && ui
                        .add_sized(
                            size,
                            egui::Button::new(widgets::icon_text(icons::TRAY, "Hide")),
                        )
                        .on_hover_text("Close the window; applications stay supervised")
                        .clicked()
                {
                    ui.send_viewport_cmd(ViewportCommand::Close);
                }
                let quit = egui::Button::new(
                    RichText::new(widgets::icon_text(icons::POWER, "Quit")).color(p.danger),
                );
                if ui
                    .add_sized(size, quit)
                    .on_hover_text("Stop all applications and exit")
                    .clicked()
                {
                    if running > 0 {
                        self.confirm_quit = true;
                    } else {
                        self.quit(ui.ctx());
                    }
                }
            });
            ui.add_space(4.0);
            let (color, text) = if running > 0 {
                (p.success, format!("Supervising {running} application(s)"))
            } else {
                (p.neutral, "Nothing running".to_owned())
            };
            ui.horizontal(|ui| {
                widgets::dot(ui, color);
                ui.label(RichText::new(text).small().color(color));
            });
            ui.add_space(2.0);
            ui.separator();
        });
    }
}

impl eframe::App for GravekeeperApp<'_> {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_close_request(ctx);
        if let Some(shell) = &self.shell {
            let shown = shell.window_shown();
            if let Some(item) = &self.tray_window_item {
                item.set_shown(shown);
            }
            crate::dock::set_window_shown(shown);
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

        let p = theme::palette(ui);
        egui::Panel::left("nav")
            .exact_size(210.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(p.sidebar)
                    .inner_margin(Margin::symmetric(12, 14)),
            )
            .show(ui, |ui| self.sidebar(ui, &snapshot, running));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(Margin::symmetric(24, 20)),
            )
            .show(ui, |ui| {
                if let Some(error) = &snapshot.config_error {
                    widgets::banner(ui, p.danger, |ui| {
                        ui.label(
                            RichText::new(widgets::icon_text(
                                icons::WARNING_CIRCLE,
                                "The configuration file could not be loaded. Changes are disabled until it is fixed.",
                            ))
                            .strong()
                            .color(p.danger),
                        );
                        ui.label(error);
                        ui.horizontal(|ui| {
                            if widgets::button(ui, true, icons::RELOAD, "Reload").clicked() {
                                self.core.send(Command::ReloadConfig);
                            }
                            if widgets::button(ui, true, icons::FOLDER_OPEN, "Show file").clicked() {
                                picker::reveal(&snapshot.config_path);
                            }
                        });
                    });
                    ui.add_space(12.0);
                }
                match self.tab {
                    Tab::Dashboard => {
                        if let Some(id) = dashboard::show(ui, &snapshot) {
                            self.apps.select(id);
                            self.tab = Tab::Apps;
                        }
                    }
                    Tab::Apps => self.apps.show(ui, self.core, &snapshot),
                    Tab::Schedules => self.schedules.show(ui, self.core, &snapshot),
                    Tab::Notifications => self.webhooks.show(ui, self.core, &snapshot),
                    Tab::Events => self.events.show(ui, &snapshot),
                    Tab::System => self.system.show(ui),
                    Tab::Settings => self.settings.show(ui, self.core, &snapshot),
                }
            });

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

impl AlreadyRunning {
    pub const WINDOW_SIZE: [f32; 2] = [380.0, 170.0];

    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        Self
    }
}

impl eframe::App for AlreadyRunning {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(Margin::same(20)),
            )
            .show(ui, |ui| {
                widgets::section_title(ui, "Gravekeeper is already running");
                ui.label(
                    "Open it from its icon in the system tray. If it runs as a background service \
                     (gravekeeper service status), stop the service to use the window.",
                );
                ui.add_space(8.0);
                if ui.button("OK").clicked() {
                    ui.send_viewport_cmd(ViewportCommand::Close);
                }
            });
    }
}
