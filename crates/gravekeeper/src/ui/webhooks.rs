//! Webhook notifications: list, details, editor.

use std::collections::BTreeMap;

use eframe::egui::{self, Margin, RichText};
use gravekeeper_core::api::EventKind;
use gravekeeper_core::config::{AppId, Secret};
use gravekeeper_core::webhook::{self, PayloadFormat, WebhookId, WebhookSpec};
use gravekeeper_core::{Command, CoreHandle, Snapshot};

use super::widgets::{self, format_relative, format_time};
use super::{form, icons, theme};

#[derive(Default)]
pub struct WebhooksView {
    selected: Option<WebhookId>,
    editor: Option<WebhookEditor>,
    confirm_remove: Option<WebhookId>,
}

impl WebhooksView {
    pub fn with_selection(selected: Option<WebhookId>) -> Self {
        Self {
            selected,
            ..Self::default()
        }
    }

    pub fn selected(&self) -> Option<&WebhookId> {
        self.selected.as_ref()
    }

    fn start_adding(&mut self) {
        let editor = WebhookEditor::from_spec(&WebhookSpec::new("", ""), true);
        self.selected = Some(editor.id.clone());
        self.editor = Some(editor);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, core: &CoreHandle, snapshot: &Snapshot) {
        let editable = snapshot.config_error.is_none();
        if self.editor.is_none()
            && !self
                .selected
                .as_ref()
                .is_some_and(|id| snapshot.config.webhooks.iter().any(|w| &w.id == id))
        {
            self.selected = snapshot.config.webhooks.first().map(|w| w.id.clone());
        }

        widgets::page_header(
            ui,
            "Notifications",
            Some(
                "Send crashes, failed schedules and resource alerts to chat or any HTTP endpoint.",
            ),
            |ui| {
                if widgets::primary_button(ui, editable, icons::PLUS, "Add webhook").clicked() {
                    self.start_adding();
                }
            },
        );

        if snapshot.config.webhooks.is_empty() && self.editor.is_none() {
            if widgets::empty_state(
                ui,
                icons::BELL,
                "No webhooks yet",
                "Slack, Discord and plain JSON endpoints are supported.",
                editable.then_some((icons::PLUS, "Add webhook")),
            ) {
                self.start_adding();
            }
            self.confirm_removal(ui, core);
            return;
        }

        let p = theme::palette(ui);
        egui::Panel::left("webhook_list")
            .resizable(true)
            .default_size(260.0)
            .min_size(200.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().inner_margin(Margin {
                right: 16,
                ..Margin::ZERO
            }))
            .show(ui, |ui| {
                widgets::card(ui, |ui| {
                    for hook in &snapshot.config.webhooks {
                        let selected = self.selected.as_ref() == Some(&hook.id)
                            && !self.editor.as_ref().is_some_and(|e| e.is_new);
                        let failed = last_result(snapshot, hook).is_some_and(|(ok, _)| !ok);
                        let response = widgets::list_row(
                            ui,
                            ("webhook", &hook.id),
                            &hook.name,
                            selected,
                            |ui| {
                                ui.horizontal(|ui| {
                                    let (icon, color) = if failed {
                                        (icons::WARNING_CIRCLE, p.danger)
                                    } else if hook.enabled {
                                        (icons::BELL, p.accent)
                                    } else {
                                        (icons::BELL, p.neutral)
                                    };
                                    ui.label(RichText::new(icon).color(color));
                                    let name = RichText::new(&hook.name).strong();
                                    ui.label(if hook.enabled {
                                        name
                                    } else {
                                        name.color(p.text_weak)
                                    });
                                });
                                ui.label(
                                    RichText::new(format!(
                                        "{} · {} events",
                                        format_label(hook.format),
                                        hook.events.len()
                                    ))
                                    .small()
                                    .color(p.text_weak),
                                );
                            },
                        );
                        if response.clicked() {
                            self.selected = Some(hook.id.clone());
                            self.editor = None;
                        }
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| {
                if let Some(editor) = &mut self.editor {
                    match editor.show(ui, snapshot) {
                        EditorAction::None => {}
                        EditorAction::Cancel => self.editor = None,
                        EditorAction::Save(spec) => {
                            self.selected = Some(spec.id.clone());
                            core.send(Command::SaveWebhook(spec));
                            self.editor = None;
                        }
                    }
                } else if let Some(hook) = self
                    .selected
                    .as_ref()
                    .and_then(|id| snapshot.config.webhooks.iter().find(|w| &w.id == id))
                {
                    self.details(ui, core, snapshot, hook);
                }
            });

        self.confirm_removal(ui, core);
    }

    fn confirm_removal(&mut self, ui: &mut egui::Ui, core: &CoreHandle) {
        let Some(id) = self.confirm_remove.clone() else {
            return;
        };
        let modal =
            egui::Modal::new(egui::Id::unique("confirm_remove_webhook")).show(ui.ctx(), |ui| {
                ui.set_max_width(400.0);
                widgets::section_title(ui, "Remove webhook?");
                ui.label("No more notifications will be sent to it.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if widgets::danger_button(ui, true, icons::TRASH, "Remove").clicked() {
                        core.send(Command::RemoveWebhook(id.clone()));
                        self.confirm_remove = None;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_remove = None;
                    }
                });
            });
        if modal.should_close() {
            self.confirm_remove = None;
        }
    }

    fn details(
        &mut self,
        ui: &mut egui::Ui,
        core: &CoreHandle,
        snapshot: &Snapshot,
        hook: &WebhookSpec,
    ) {
        let p = theme::palette(ui);
        let editable = snapshot.config_error.is_none();
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&hook.name).text_style(egui::TextStyle::Heading));
                if !hook.enabled {
                    widgets::pill(ui, "Disabled", p.neutral);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::danger_button(ui, editable, icons::TRASH, "")
                        .on_hover_text("Remove")
                        .clicked()
                    {
                        self.confirm_remove = Some(hook.id.clone());
                    }
                    if widgets::button(ui, editable, icons::EDIT, "Edit").clicked() {
                        self.editor = Some(WebhookEditor::from_spec(hook, false));
                    }
                    if widgets::primary_button(ui, true, icons::SEND, "Send test")
                        .on_hover_text("The result appears below and in Events")
                        .clicked()
                    {
                        core.send(Command::TestWebhook(hook.id.clone()));
                    }
                });
            });
            ui.add_space(6.0);
            widgets::properties(ui, "webhook_status", |ui| {
                widgets::property_label(ui, "URL");
                ui.label(RichText::new(masked_url(hook.url.expose())).monospace())
                    .on_hover_text("The rest of the URL is hidden; edit to see it.");
                ui.end_row();
                widgets::property_label(ui, "Format");
                ui.label(format_label(hook.format));
                ui.end_row();
                widgets::property_label(ui, "Applications");
                ui.label(if hook.apps.is_empty() {
                    "all".to_owned()
                } else {
                    hook.apps
                        .iter()
                        .map(|id| snapshot.spec(id).map_or(id.0.as_str(), |a| a.name.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                });
                ui.end_row();
                if let Some((ok, event)) = last_result(snapshot, hook) {
                    widgets::property_label(ui, "Last result");
                    ui.horizontal(|ui| {
                        if ok {
                            widgets::pill(
                                ui,
                                &widgets::icon_text(icons::CHECK, "Delivered"),
                                p.success,
                            );
                        } else {
                            widgets::pill(
                                ui,
                                &widgets::icon_text(icons::ERROR, "Failed"),
                                p.danger,
                            );
                        }
                        let prefix = format!("{}: ", hook.name);
                        let message = event
                            .message
                            .strip_prefix(&prefix)
                            .unwrap_or(&event.message);
                        ui.label(RichText::new(format_relative(event.at)).color(p.text_weak))
                            .on_hover_text(format_time(event.at));
                        ui.add(egui::Label::new(message).truncate())
                            .on_hover_text(message);
                    });
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            widgets::property_label(ui, "Sends");
            ui.horizontal_wrapped(|ui| {
                for kind in &hook.events {
                    widgets::pill(ui, event_label(*kind), p.accent);
                }
            });
        });
    }
}

/// The latest test or failure result of a webhook (events do not carry the webhook id, so
/// they are matched by the name prefix).
fn last_result<'a>(
    snapshot: &'a Snapshot,
    hook: &WebhookSpec,
) -> Option<(bool, &'a gravekeeper_core::api::Event)> {
    let prefix = format!("{}: ", hook.name);
    snapshot
        .events
        .iter()
        .rev()
        .find(|e| {
            matches!(
                e.kind,
                EventKind::WebhookFailed | EventKind::WebhookDelivered
            ) && e.message.starts_with(&prefix)
        })
        .map(|e| (e.kind == EventKind::WebhookDelivered, e))
}

/// Shows only the scheme and host, e.g. `https://hooks.slack.com/…`.
fn masked_url(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split('/').next().unwrap_or_default();
    let scheme = url.split_once("://").map_or("", |(s, _)| s);
    format!("{scheme}://{host}/…")
}

fn format_label(format: PayloadFormat) -> &'static str {
    match format {
        PayloadFormat::Json => "JSON",
        PayloadFormat::Slack => "Slack",
        PayloadFormat::Discord => "Discord",
    }
}

fn event_label(kind: EventKind) -> &'static str {
    match kind {
        EventKind::AppStarted => "application started",
        EventKind::AppStopped => "application stopped",
        EventKind::AppExited => "application exited unexpectedly",
        EventKind::AppStartFailed => "application failed to start",
        EventKind::RestartScheduled => "restart scheduled",
        EventKind::RestartGaveUp => "gave up restarting",
        EventKind::ScheduleSucceeded => "schedule succeeded",
        EventKind::ScheduleFailed => "schedule failed",
        EventKind::ScheduleMissed => "schedule missed",
        EventKind::ResourceAlert => "resource alert",
        EventKind::ResourceRecovered => "resource recovered",
        EventKind::EngineStarted => "Gravekeeper started",
        EventKind::EngineStopped => "Gravekeeper stopped",
        EventKind::ConfigError => "configuration error",
        other => other.as_str(),
    }
}

enum EditorAction {
    None,
    Save(Box<WebhookSpec>),
    Cancel,
}

struct WebhookEditor {
    id: WebhookId,
    is_new: bool,
    name: String,
    enabled: bool,
    url: String,
    show_secrets: bool,
    headers: String,
    format: PayloadFormat,
    events: Vec<EventKind>,
    apps: Vec<AppId>,
    errors: Vec<String>,
}

impl WebhookEditor {
    fn from_spec(spec: &WebhookSpec, is_new: bool) -> Self {
        Self {
            id: spec.id.clone(),
            is_new,
            name: spec.name.clone(),
            enabled: spec.enabled,
            url: spec.url.expose().to_owned(),
            show_secrets: is_new,
            headers: spec
                .headers
                .iter()
                .map(|(k, v)| format!("{k}: {}", v.expose()))
                .collect::<Vec<_>>()
                .join("\n"),
            format: spec.format,
            events: spec.events.clone(),
            apps: spec.apps.clone(),
            errors: Vec::new(),
        }
    }

    fn build(&self) -> Result<WebhookSpec, Vec<String>> {
        let mut errors = Vec::new();
        let mut headers = BTreeMap::new();
        for (i, line) in self.headers.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match line.split_once(':') {
                Some((k, v)) => {
                    headers.insert(k.trim().to_owned(), Secret::new(v.trim()));
                }
                None => errors.push(format!("header line {} must be Name: value", i + 1)),
            }
        }
        let spec = WebhookSpec {
            id: self.id.clone(),
            name: self.name.trim().to_owned(),
            enabled: self.enabled,
            url: Secret::new(self.url.trim()),
            headers,
            format: self.format,
            events: webhook::NOTIFIABLE
                .iter()
                .copied()
                .filter(|k| self.events.contains(k))
                .collect(),
            apps: self.apps.clone(),
        };
        errors.extend(webhook::validate(&spec));
        if spec.events.is_empty() {
            errors.push("choose at least one event".into());
        }
        if errors.is_empty() {
            Ok(spec)
        } else {
            Err(errors)
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) -> EditorAction {
        let mut action = EditorAction::None;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(if self.is_new {
                    "Add webhook"
                } else {
                    "Edit webhook"
                })
                .text_style(egui::TextStyle::Heading),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::primary_button(ui, true, icons::SAVE, "Save").clicked() {
                    match self.build() {
                        Ok(spec) => action = EditorAction::Save(Box::new(spec)),
                        Err(errors) => self.errors = errors,
                    }
                }
                if ui.button("Cancel").clicked() {
                    action = EditorAction::Cancel;
                }
            });
        });
        ui.add_space(8.0);
        form::errors(ui, &self.errors);

        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                form::section(
                    ui,
                    "Endpoint",
                    Some("The URL and headers are kept out of logs and the event history."),
                    |ui| {
                        form::row(ui, "Name", None, |ui| {
                            form::text(ui, &mut self.name, "e.g. Ops channel")
                        });
                        form::row(ui, "Enabled", None, |ui| {
                            ui.checkbox(&mut self.enabled, "Send notifications");
                        });
                        form::row(ui, "URL", None, |ui| {
                            ui.horizontal(|ui| {
                                let icon = if self.show_secrets {
                                    icons::EYE_SLASH
                                } else {
                                    icons::EYE
                                };
                                if ui
                                    .button(icon)
                                    .on_hover_text(if self.show_secrets { "Hide" } else { "Show" })
                                    .clicked()
                                {
                                    self.show_secrets = !self.show_secrets;
                                }
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.url)
                                        .password(!self.show_secrets)
                                        .hint_text("https://…")
                                        .desired_width(ui.available_width()),
                                );
                            });
                        });
                        form::row(ui, "Format", None, |ui| {
                            ui.horizontal(|ui| {
                                for f in [
                                    PayloadFormat::Json,
                                    PayloadFormat::Slack,
                                    PayloadFormat::Discord,
                                ] {
                                    ui.selectable_value(&mut self.format, f, format_label(f));
                                }
                            });
                        });
                        form::row(
                            ui,
                            "Headers",
                            Some("Name: value, one per line (e.g. Authorization)."),
                            |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.headers)
                                        .password(!self.show_secrets)
                                        .hint_text("Authorization: Bearer …")
                                        .desired_rows(2)
                                        .desired_width(f32::INFINITY),
                                );
                            },
                        );
                    },
                );

                form::section(ui, "What to send", None, |ui| {
                    form::row(ui, "Events", None, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for kind in webhook::NOTIFIABLE {
                                let mut on = self.events.contains(kind);
                                if ui.toggle_value(&mut on, event_label(*kind)).changed() {
                                    if on {
                                        self.events.push(*kind);
                                    } else {
                                        self.events.retain(|k| k != kind);
                                    }
                                }
                            }
                        });
                    });
                    form::row(
                        ui,
                        "Applications",
                        Some("None selected means all applications."),
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                for app in &snapshot.apps {
                                    let mut on = self.apps.contains(&app.id);
                                    if ui.toggle_value(&mut on, &app.name).changed() {
                                        if on {
                                            self.apps.push(app.id.clone());
                                        } else {
                                            self.apps.retain(|id| id != &app.id);
                                        }
                                    }
                                }
                                if snapshot.apps.is_empty() {
                                    widgets::hint(ui, "No applications yet.");
                                }
                            });
                        },
                    );
                });
            });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_masked_to_the_host() {
        assert_eq!(
            masked_url("https://hooks.slack.com/services/T/B/secret"),
            "https://hooks.slack.com/…"
        );
    }

    #[test]
    fn round_trips_a_spec() {
        let mut spec = WebhookSpec::new("ops", "https://example.com/hook");
        spec.headers
            .insert("Authorization".into(), Secret::new("Bearer x"));
        spec.format = PayloadFormat::Discord;
        assert_eq!(
            WebhookEditor::from_spec(&spec, false).build().unwrap(),
            spec
        );
    }

    #[test]
    fn reports_bad_input() {
        let mut editor = WebhookEditor::from_spec(&WebhookSpec::new("x", "nope"), true);
        editor.headers = "no colon".into();
        editor.events.clear();
        let errors = editor.build().unwrap_err();
        assert_eq!(errors.len(), 3, "{errors:?}");
    }
}
