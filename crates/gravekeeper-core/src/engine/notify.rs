//! Sending events to webhooks.
//!
//! Each delivery runs as its own task (with retries), so a slow or failing endpoint does
//! not hold up the others. Failures become `WebhookFailed` events, which are never sent to
//! webhooks themselves.

use std::sync::Arc;
use std::time::SystemTime;

use super::{Engine, Internal};
use crate::api::{Event, EventKind, EventLevel};
use crate::webhook::{self, DeliveryError, WebhookId, WebhookSpec};

/// Deliveries running at once; further ones are dropped (and logged).
const MAX_IN_FLIGHT: usize = 50;
/// Deliveries queued before the engine's runtime starts.
const MAX_PENDING: usize = 100;

#[derive(Default)]
pub(super) struct State {
    sender: Option<Arc<webhook::Sender>>,
    started: bool,
    pending: Vec<Delivery>,
    in_flight: usize,
    host: String,
}

struct Delivery {
    spec: WebhookSpec,
    body: serde_json::Value,
    kind: EventKind,
    test: bool,
}

pub(super) struct Outcome {
    webhook: String,
    kind: EventKind,
    test: bool,
    result: Result<u32, DeliveryError>,
}

impl Engine {
    /// Called once the runtime is running; sends what was queued during startup.
    pub(super) fn start_notifications(&mut self) {
        self.notify.host = sysinfo::System::host_name().unwrap_or_else(|| "unknown".into());
        match webhook::Sender::new() {
            Ok(sender) => self.notify.sender = Some(Arc::new(sender)),
            Err(err) => tracing::warn!(error = %err, "webhooks are unavailable"),
        }
        self.notify.started = true;
        for delivery in std::mem::take(&mut self.notify.pending) {
            self.dispatch(delivery);
        }
    }

    pub(super) fn notify(&mut self, event: &Event) {
        if !webhook::NOTIFIABLE.contains(&event.kind) {
            return;
        }
        let host = self.notify.host.clone();
        let deliveries: Vec<Delivery> = self
            .config
            .webhooks
            .iter()
            .filter(|spec| spec.wants(event))
            .map(|spec| Delivery {
                body: webhook::payload(spec.format, event, &host),
                spec: spec.clone(),
                kind: event.kind,
                test: false,
            })
            .collect();
        for delivery in deliveries {
            self.dispatch(delivery);
        }
    }

    pub(super) fn test_webhook(&mut self, id: &WebhookId) {
        let Some(spec) = self.config.webhooks.iter().find(|w| &w.id == id).cloned() else {
            self.reject(format!("unknown webhook {id}"));
            return;
        };
        let event = Event {
            at: SystemTime::now(),
            level: EventLevel::Info,
            kind: EventKind::WebhookDelivered,
            app_id: None,
            app_name: None,
            message: "This is a test message from Gravekeeper.".into(),
        };
        let body = webhook::payload(spec.format, &event, &self.notify.host);
        self.dispatch(Delivery {
            spec,
            body,
            kind: event.kind,
            test: true,
        });
    }

    fn dispatch(&mut self, delivery: Delivery) {
        let state = &mut self.notify;
        if !state.started {
            if state.pending.len() < MAX_PENDING {
                state.pending.push(delivery);
            }
            return;
        }
        let Some(sender) = state.sender.clone() else {
            return;
        };
        if state.in_flight >= MAX_IN_FLIGHT {
            tracing::warn!(webhook = %delivery.spec.name, "too many deliveries in progress; dropped");
            return;
        }
        state.in_flight += 1;
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let result = sender.deliver(&delivery.spec, &delivery.body).await;
            let _ = tx.send(Internal::Webhook(Outcome {
                webhook: delivery.spec.name,
                kind: delivery.kind,
                test: delivery.test,
                result,
            }));
        });
    }

    pub(super) fn on_webhook_outcome(&mut self, outcome: Outcome) {
        self.notify.in_flight = self.notify.in_flight.saturating_sub(1);
        match (outcome.result, outcome.test) {
            (Ok(attempts), true) => self.emit(
                EventLevel::Info,
                EventKind::WebhookDelivered,
                None,
                format!(
                    "{}: test message delivered (attempts: {attempts})",
                    outcome.webhook
                ),
            ),
            (Ok(_), false) => {}
            (Err(err), test) => {
                let what = if test {
                    "test message".to_owned()
                } else {
                    format!("{} notification", outcome.kind.as_str())
                };
                self.emit(
                    EventLevel::Warning,
                    EventKind::WebhookFailed,
                    None,
                    format!("{}: {what} not delivered: {}", outcome.webhook, err.message),
                );
            }
        }
    }
}
