//! Webhook notifications: list, details, editor.

use std::collections::BTreeMap;

use eframe::egui::{self, RichText};
use gravekeeper_core::api::EventKind;
use gravekeeper_core::config::{AppId, Secret};
use gravekeeper_core::webhook::{self, PayloadFormat, WebhookId, WebhookSpec};
use gravekeeper_core::{Command, CoreHandle, Snapshot};

use super::widgets::format_time;

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

    pub fn show(&mut self, ui: &mut egui::Ui, core: &CoreHandle, snapshot: &Snapshot) {
        let editable = snapshot.config_error.is_none();
        egui::Panel::left("webhook_list")
            .resizable(true)
            .default_size(220.0)
            .min_size(160.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                if ui
                    .add_enabled(editable, egui::Button::new("Add webhook"))
                    .clicked()
                {
                    let editor = WebhookEditor::from_spec(&WebhookSpec::new("", ""), true);
                    self.selected = Some(editor.id.clone());
                    self.editor = Some(editor);
                }
                ui.separator();
                for hook in &snapshot.config.webhooks {
                    let selected = self.selected.as_ref() == Some(&hook.id);
                    let text = if hook.enabled {
                        RichText::new(&hook.name)
                    } else {
                        RichText::new(&hook.name).weak()
                    };
                    if ui.selectable_label(selected, text).clicked() {
                        self.selected = Some(hook.id.clone());
                        self.editor = None;
                    }
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
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
            } else if snapshot.config.webhooks.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label("No webhooks yet. Add one to be notified of crashes, failed schedules and resource alerts.");
                });
            } else {
                ui.centered_and_justified(|ui| ui.label("Select a webhook."));
            }
        });

        if let Some(id) = self.confirm_remove.clone() {
            let modal =
                egui::Modal::new(egui::Id::unique("confirm_remove_webhook")).show(ui.ctx(), |ui| {
                    ui.heading("Remove webhook?");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Remove").clicked() {
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
    }

    fn details(
        &mut self,
        ui: &mut egui::Ui,
        core: &CoreHandle,
        snapshot: &Snapshot,
        hook: &WebhookSpec,
    ) {
        let editable = snapshot.config_error.is_none();
        ui.heading(&hook.name);
        ui.horizontal(|ui| {
            if ui
                .button("Send test")
                .on_hover_text("The result appears below and in Events")
                .clicked()
            {
                core.send(Command::TestWebhook(hook.id.clone()));
            }
            if ui
                .add_enabled(editable, egui::Button::new("Edit"))
                .clicked()
            {
                self.editor = Some(WebhookEditor::from_spec(hook, false));
            }
            if ui
                .add_enabled(editable, egui::Button::new("Remove"))
                .clicked()
            {
                self.confirm_remove = Some(hook.id.clone());
            }
        });
        ui.add_space(4.0);
        egui::Grid::new("webhook_status")
            .num_columns(2)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                ui.label("Status");
                ui.label(if hook.enabled { "enabled" } else { "disabled" });
                ui.end_row();
                ui.label("URL");
                ui.label(masked_url(hook.url.expose()));
                ui.end_row();
                ui.label("Format");
                ui.label(format_label(hook.format));
                ui.end_row();
                ui.label("Events");
                let events: Vec<_> = hook.events.iter().map(|k| event_label(*k)).collect();
                ui.label(events.join(", "));
                ui.end_row();
                ui.label("Applications");
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
            });

        // Recent delivery problems for this webhook (events do not carry the webhook id).
        let prefix = format!("{}: ", hook.name);
        let recent: Vec<_> = snapshot
            .events
            .iter()
            .rev()
            .filter(|e| {
                matches!(
                    e.kind,
                    EventKind::WebhookFailed | EventKind::WebhookDelivered
                ) && e.message.starts_with(&prefix)
            })
            .take(5)
            .collect();
        if !recent.is_empty() {
            ui.separator();
            ui.strong("Recent results");
            for event in recent {
                ui.label(format!(
                    "{} — {}",
                    format_time(event.at),
                    &event.message[prefix.len()..]
                ));
            }
        }
    }
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
        ui.heading(if self.is_new {
            "Add webhook"
        } else {
            "Edit webhook"
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("webhook_editor")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .min_col_width(130.0)
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut self.name).hint_text("e.g. Ops channel"));
                    ui.end_row();
                    ui.label("Enabled");
                    ui.checkbox(&mut self.enabled, "Send notifications");
                    ui.end_row();
                    ui.label("URL");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.url)
                                .password(!self.show_secrets)
                                .hint_text("https://…")
                                .desired_width(360.0),
                        );
                        ui.checkbox(&mut self.show_secrets, "Show");
                    });
                    ui.end_row();
                    ui.label("Headers");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.headers)
                            .password(!self.show_secrets)
                            .hint_text("Authorization: Bearer …")
                            .desired_rows(2)
                            .desired_width(360.0),
                    );
                    ui.end_row();
                    ui.label("Format");
                    egui::ComboBox::from_id_salt("webhook_format")
                        .selected_text(format_label(self.format))
                        .show_ui(ui, |ui| {
                            for f in [PayloadFormat::Json, PayloadFormat::Slack, PayloadFormat::Discord] {
                                ui.selectable_value(&mut self.format, f, format_label(f));
                            }
                        });
                    ui.end_row();
                    ui.label("Events");
                    ui.vertical(|ui| {
                        for kind in webhook::NOTIFIABLE {
                            let mut on = self.events.contains(kind);
                            if ui.checkbox(&mut on, event_label(*kind)).changed() {
                                if on {
                                    self.events.push(*kind);
                                } else {
                                    self.events.retain(|k| k != kind);
                                }
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("Applications");
                    ui.vertical(|ui| {
                        ui.label(RichText::new("None checked means all").weak());
                        for app in &snapshot.apps {
                            let mut on = self.apps.contains(&app.id);
                            if ui.checkbox(&mut on, &app.name).changed() {
                                if on {
                                    self.apps.push(app.id.clone());
                                } else {
                                    self.apps.retain(|id| id != &app.id);
                                }
                            }
                        }
                    });
                    ui.end_row();
                });
            ui.label(
                RichText::new(
                    "The URL and headers are stored in the configuration file (readable only by you on Linux and macOS) and never written to logs.",
                )
                .weak(),
            );
            for error in &self.errors {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
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
