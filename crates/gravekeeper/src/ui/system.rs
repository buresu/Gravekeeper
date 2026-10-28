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
    /// Read from the OS on first use, and again after each change. `None` means "re-read".
    automatic_updates: Option<Result<bool, String>>,
    /// The auto-logon account, or `None` when it is off.
    auto_login: Option<Result<Option<String>, String>>,
    /// Whether the computer sleeps on its own. Changed in-process (no elevation).
    automatic_sleep: Option<Result<bool, String>>,
    /// Whether the display turns off on its own. Changed in-process (no elevation).
    display_off: Option<Result<bool, String>>,
    /// Whether hibernation is enabled. Changed through `elevate`.
    hibernate: Option<Result<bool, String>>,
    /// Whether notifications are shown. Changed in-process (no elevation).
    notifications: Option<Result<bool, String>>,
    /// Whether Windows Security notifications are shown. The machine-wide part is changed
    /// through `elevate`; the per-user account-protection part is applied in-process on
    /// success, with the wanted value remembered here while the elevated change runs.
    security_notifications: Option<Result<bool, String>>,
    security_user_pending: Option<bool>,
    /// Whether the firewall is on. Changed through `elevate`.
    firewall: Option<Result<bool, String>>,
    /// A change waiting for administrator approval and the elevated helper. Only one runs at a
    /// time, so every control is disabled while it is set.
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
        let security_user = self.security_user_pending.take();
        // Re-read everything from the OS, whatever happened.
        self.automatic_updates = None;
        self.auto_login = None;
        self.automatic_sleep = None;
        self.display_off = None;
        self.hibernate = None;
        self.notifications = None;
        self.security_notifications = None;
        self.firewall = None;
        match result {
            Ok(()) => {
                self.error = None;
                // Apply the per-user part only once the elevated part succeeded.
                if let Some(enabled) = security_user
                    && let Err(err) = os_settings::set_security_notifications_user(enabled)
                {
                    self.error = Some(format!("Could not change the setting: {err}"));
                }
            }
            Err(elevate::Error::Cancelled) => self.error = None,
            Err(err) => self.error = Some(format!("Could not change the setting: {err}")),
        }
    }

    fn elevate(&mut self, ctx: &egui::Context, task: Task) {
        let ctx = ctx.clone();
        self.elevation = Some(elevate::spawn(vec![task], move || ctx.request_repaint()));
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
                        |ui| self.updates_control(ui, busy),
                    );
                });

                form::section(ui, "Sign-in", None, |ui| {
                    form::row(
                        ui,
                        "Automatic logon",
                        Some("Sign in to an account at startup without typing a password. You enter the password in a Windows prompt; it is stored by Windows, not by Gravekeeper."),
                        |ui| self.auto_login_control(ui, busy),
                    );
                });

                form::section(ui, "Power", None, |ui| {
                    form::row(
                        ui,
                        "Automatic sleep",
                        Some("When off, the computer never sleeps on its own. Needs no administrator rights."),
                        |ui| {
                            let cached = &mut self.automatic_sleep;
                            direct_toggle(
                                ui,
                                busy,
                                cached,
                                os_settings::automatic_sleep,
                                os_settings::set_automatic_sleep,
                                &mut self.error,
                            );
                        },
                    );
                    form::row(
                        ui,
                        "Turn off the display",
                        Some("When off, the display never turns off on its own. Needs no administrator rights."),
                        |ui| {
                            let cached = &mut self.display_off;
                            direct_toggle(
                                ui,
                                busy,
                                cached,
                                os_settings::display_off,
                                os_settings::set_display_off,
                                &mut self.error,
                            );
                        },
                    );
                    form::row(
                        ui,
                        "Hibernation",
                        Some("When off, the computer cannot hibernate and hiberfil.sys is freed. Asks for administrator rights."),
                        |ui| self.hibernate_control(ui, busy),
                    );
                });

                form::section(ui, "Notifications", None, |ui| {
                    form::row(
                        ui,
                        "Notifications",
                        Some("When off, apps show no notifications for this user, including Windows tips and setup suggestions. Needs no administrator rights."),
                        |ui| {
                            let cached = &mut self.notifications;
                            direct_toggle(
                                ui,
                                busy,
                                cached,
                                os_settings::notifications_enabled,
                                os_settings::set_notifications,
                                &mut self.error,
                            );
                        },
                    );
                    form::row(
                        ui,
                        "Windows Security notifications",
                        Some("When off, Microsoft Defender / Windows Security shows no notifications. Asks for administrator rights."),
                        |ui| self.security_notif_control(ui, busy),
                    );
                });

                form::section(ui, "Firewall", None, |ui| {
                    form::row(
                        ui,
                        "Microsoft Defender Firewall",
                        Some("Turns the firewall on or off for all network profiles. Off leaves the computer unprotected; asks for administrator rights."),
                        |ui| self.firewall_control(ui, busy),
                    );
                });
            });
    }

    fn firewall_control(&mut self, ui: &mut egui::Ui, busy: bool) {
        let state = self
            .firewall
            .get_or_insert_with(|| os_settings::firewall_enabled().map_err(|e| e.to_string()))
            .clone();
        match state {
            Ok(mut enabled) => {
                if ui
                    .add_enabled(!busy, egui::Checkbox::new(&mut enabled, "Enabled"))
                    .changed()
                {
                    self.elevate(ui.ctx(), Task::Firewall { enabled });
                }
            }
            Err(err) => {
                ui.label(format!("Unavailable: {err}"));
            }
        }
    }

    fn security_notif_control(&mut self, ui: &mut egui::Ui, busy: bool) {
        let state = self
            .security_notifications
            .get_or_insert_with(|| {
                os_settings::security_notifications_enabled().map_err(|e| e.to_string())
            })
            .clone();
        match state {
            Ok(mut enabled) => {
                if ui
                    .add_enabled(!busy, egui::Checkbox::new(&mut enabled, "Enabled"))
                    .changed()
                {
                    // The per-user part is applied in poll_elevation once the elevated
                    // (machine-wide) part succeeds.
                    self.security_user_pending = Some(enabled);
                    self.elevate(ui.ctx(), Task::SecurityNotifications { enabled });
                }
            }
            Err(err) => {
                ui.label(format!("Unavailable: {err}"));
            }
        }
    }

    fn hibernate_control(&mut self, ui: &mut egui::Ui, busy: bool) {
        let state = self
            .hibernate
            .get_or_insert_with(|| os_settings::hibernate_enabled().map_err(|e| e.to_string()))
            .clone();
        match state {
            Ok(mut enabled) => {
                if ui
                    .add_enabled(!busy, egui::Checkbox::new(&mut enabled, "Enabled"))
                    .changed()
                {
                    self.elevate(ui.ctx(), Task::Hibernate { enabled });
                }
            }
            Err(err) => {
                ui.label(format!("Unavailable: {err}"));
            }
        }
    }

    fn updates_control(&mut self, ui: &mut egui::Ui, busy: bool) {
        let state = self
            .automatic_updates
            .get_or_insert_with(|| os_settings::automatic_updates().map_err(|e| e.to_string()))
            .clone();
        match state {
            Ok(mut enabled) => {
                if ui
                    .add_enabled(!busy, egui::Checkbox::new(&mut enabled, "Enabled"))
                    .changed()
                {
                    self.elevate(ui.ctx(), Task::AutomaticUpdates { enabled });
                }
            }
            Err(err) => {
                ui.label(format!("Unavailable: {err}"));
            }
        }
    }

    fn auto_login_control(&mut self, ui: &mut egui::Ui, busy: bool) {
        let state = self
            .auto_login
            .get_or_insert_with(|| os_settings::auto_login().map_err(|e| e.to_string()))
            .clone();
        match state {
            Ok(account) => {
                let mut enabled = account.is_some();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!busy, egui::Checkbox::new(&mut enabled, "Enabled"))
                        .changed()
                    {
                        self.elevate(ui.ctx(), Task::AutoLogin { enabled });
                    }
                    if let Some(user) = account.as_deref().filter(|u| !u.is_empty()) {
                        widgets::hint(ui, &format!("as {user}"));
                    }
                });
            }
            Err(err) => {
                ui.label(format!("Unavailable: {err}"));
            }
        }
    }
}

/// A checkbox for a setting applied in-process (no elevation): reads once, applies on change
/// and re-reads. `cached` and `error` are distinct `SystemView` fields, passed separately.
fn direct_toggle(
    ui: &mut egui::Ui,
    busy: bool,
    cached: &mut Option<Result<bool, String>>,
    read: fn() -> std::io::Result<bool>,
    apply: fn(bool) -> std::io::Result<()>,
    error: &mut Option<String>,
) {
    let state = cached
        .get_or_insert_with(|| read().map_err(|e| e.to_string()))
        .clone();
    match state {
        Ok(mut enabled) => {
            // Disabled while an elevated change runs, to keep one change at a time.
            if ui
                .add_enabled(!busy, egui::Checkbox::new(&mut enabled, "Enabled"))
                .changed()
            {
                match apply(enabled) {
                    Ok(()) => *error = None,
                    Err(err) => *error = Some(format!("Could not change the setting: {err}")),
                }
                *cached = None;
            }
        }
        Err(err) => {
            ui.label(format!("Unavailable: {err}"));
        }
    }
}
