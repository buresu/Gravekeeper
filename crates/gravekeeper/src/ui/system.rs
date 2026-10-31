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
    /// Whether the computer sleeps on its own. Changed in-process (no elevation), except on
    /// macOS, where it goes through `elevate` (see `os_settings::POWER_NEEDS_ADMIN`).
    automatic_sleep: Option<Result<bool, String>>,
    /// Whether the display turns off on its own. Changed like `automatic_sleep`.
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
    /// Whether macOS App Nap is on. Changed in-process (no elevation).
    app_nap: Option<Result<bool, String>>,
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
        self.app_nap = None;
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
                    let auto_login_help = per_os(
                        "Sign in to an account at startup without typing a password. You enter the password in a Windows prompt; it is stored by Windows, not by Gravekeeper.",
                        "Sign in to your account at startup without typing a password. Asks for administrator rights. KWallet or the GNOME keyring is then not unlocked at sign-in.",
                        "Sign in to your account at startup without typing a password. You enter it in a macOS prompt, and macOS keeps it in /etc/kcpassword, as when set in System Settings. Not available while FileVault is on.",
                    );
                    form::row(
                        ui,
                        "Automatic logon",
                        Some(auto_login_help),
                        |ui| self.auto_login_control(ui, busy),
                    );
                });

                form::section(ui, "Power", None, |ui| {
                    let rights = if os_settings::POWER_NEEDS_ADMIN {
                        "Asks for administrator rights."
                    } else {
                        "Needs no administrator rights."
                    };
                    form::row(
                        ui,
                        "Automatic sleep",
                        Some(&format!("When off, the computer never sleeps on its own. {rights}")),
                        |ui| {
                            if os_settings::POWER_NEEDS_ADMIN {
                                let state = read_once(&mut self.automatic_sleep, os_settings::automatic_sleep);
                                self.elevated_toggle(ui, busy, state, |enabled| Task::AutomaticSleep { enabled });
                            } else {
                                let cached = &mut self.automatic_sleep;
                                direct_toggle(
                                    ui,
                                    busy,
                                    cached,
                                    os_settings::automatic_sleep,
                                    os_settings::set_automatic_sleep,
                                    &mut self.error,
                                );
                            }
                        },
                    );
                    form::row(
                        ui,
                        "Turn off the display",
                        Some(&format!("When off, the display never turns off on its own. {rights}")),
                        |ui| {
                            if os_settings::POWER_NEEDS_ADMIN {
                                let state = read_once(&mut self.display_off, os_settings::display_off);
                                self.elevated_toggle(ui, busy, state, |enabled| Task::DisplayOff { enabled });
                            } else {
                                let cached = &mut self.display_off;
                                direct_toggle(
                                    ui,
                                    busy,
                                    cached,
                                    os_settings::display_off,
                                    os_settings::set_display_off,
                                    &mut self.error,
                                );
                            }
                        },
                    );
                    let hibernation_help = per_os(
                        "When off, the computer cannot hibernate and hiberfil.sys is freed. Asks for administrator rights.",
                        "When off, the computer cannot hibernate, including suspend-then-hibernate and hybrid sleep. Asks for administrator rights.",
                        "When off, the Mac only sleeps, without saving memory to disk, and the sleep image is deleted. Asks for administrator rights.",
                    );
                    form::row(
                        ui,
                        "Hibernation",
                        Some(hibernation_help),
                        |ui| self.hibernate_control(ui, busy),
                    );
                });

                form::section(ui, "Notifications", None, |ui| {
                    let notifications_help = per_os(
                        "When off, apps show no notifications for this user, including Windows tips and setup suggestions. Needs no administrator rights.",
                        "When off, the desktop stays in Do not disturb: notifications are kept in its history but not shown. Needs no administrator rights.",
                        "Use Focus in Control Center or System Settings instead.",
                    );
                    form::row(
                        ui,
                        "Notifications",
                        Some(notifications_help),
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
                    if cfg!(windows) {
                        form::row(
                            ui,
                            "Windows Security notifications",
                            Some("When off, Microsoft Defender / Windows Security shows no notifications. Asks for administrator rights."),
                            |ui| self.security_notif_control(ui, busy),
                        );
                    }
                });

                form::section(ui, "Firewall", None, |ui| {
                    let firewall_label = if cfg!(windows) {
                        "Microsoft Defender Firewall"
                    } else {
                        "Firewall"
                    };
                    let firewall_help = per_os(
                        "Turns the firewall on or off for all network profiles. Off leaves the computer unprotected; asks for administrator rights.",
                        "Turns ufw or firewalld on or off. Off leaves the computer unprotected; asks for administrator rights.",
                        "Turns the macOS application firewall on or off. Off leaves the computer unprotected; asks for administrator rights.",
                    );
                    form::row(
                        ui,
                        firewall_label,
                        Some(firewall_help),
                        |ui| self.firewall_control(ui, busy),
                    );
                });

                if cfg!(target_os = "macos") {
                    form::section(ui, "Apps", None, |ui| {
                        form::row(
                            ui,
                            "App Nap",
                            Some("When off, macOS no longer slows down apps whose windows are hidden or covered. Applies to apps started afterwards. Needs no administrator rights."),
                            |ui| {
                                let cached = &mut self.app_nap;
                                direct_toggle(
                                    ui,
                                    busy,
                                    cached,
                                    os_settings::app_nap_enabled,
                                    os_settings::set_app_nap,
                                    &mut self.error,
                                );
                            },
                        );
                    });
                }
            });
    }

    /// A checkbox for a setting changed through `elevate`.
    fn elevated_toggle(
        &mut self,
        ui: &mut egui::Ui,
        busy: bool,
        state: Result<bool, String>,
        task: fn(bool) -> Task,
    ) {
        match state {
            Ok(mut enabled) => {
                if ui
                    .add_enabled(!busy, egui::Checkbox::new(&mut enabled, "Enabled"))
                    .changed()
                {
                    self.elevate(ui.ctx(), task(enabled));
                }
            }
            Err(err) => {
                ui.label(format!("Unavailable: {err}"));
            }
        }
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

/// The help text for this OS.
fn per_os(windows: &'static str, linux: &'static str, macos: &'static str) -> &'static str {
    if cfg!(windows) {
        windows
    } else if cfg!(target_os = "macos") {
        macos
    } else {
        linux
    }
}

/// The cached state, read from the OS if it is not known yet.
fn read_once(
    cached: &mut Option<Result<bool, String>>,
    read: fn() -> std::io::Result<bool>,
) -> Result<bool, String> {
    cached
        .get_or_insert_with(|| read().map_err(|e| e.to_string()))
        .clone()
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
    match read_once(cached, read) {
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
