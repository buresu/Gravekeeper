//! HTTP webhook notifications.
//!
//! The URL and header values are [`Secret`]s and never appear in logs or events; errors are
//! reported without the URL.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::{Event, EventKind};
use crate::config::{AppId, Secret};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WebhookId(pub String);

impl WebhookId {
    pub fn generate() -> Self {
        Self(crate::config::generate_id())
    }
}

impl fmt::Display for WebhookId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookSpec {
    pub id: WebhookId,
    pub name: String,
    #[serde(default = "crate::config::yes")]
    pub enabled: bool,
    pub url: Secret,
    /// Extra request headers, e.g. `Authorization`.
    #[serde(default)]
    pub headers: BTreeMap<String, Secret>,
    #[serde(default)]
    pub format: PayloadFormat,
    /// Which events are sent.
    #[serde(default = "default_events")]
    pub events: Vec<EventKind>,
    /// Only events of these applications (and events not about an application); all when
    /// empty.
    #[serde(default)]
    pub apps: Vec<AppId>,
}

impl WebhookSpec {
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: WebhookId::generate(),
            name: name.into(),
            enabled: true,
            url: Secret::new(url),
            headers: BTreeMap::new(),
            format: PayloadFormat::Json,
            events: default_events(),
            apps: Vec::new(),
        }
    }

    pub fn wants(&self, event: &Event) -> bool {
        self.enabled
            && self.events.contains(&event.kind)
            && (self.apps.is_empty()
                || event
                    .app_id
                    .as_ref()
                    .is_none_or(|id| self.apps.contains(id)))
    }
}

/// Problems and failures; routine starts and stops are opt-in.
pub fn default_events() -> Vec<EventKind> {
    vec![
        EventKind::AppExited,
        EventKind::AppStartFailed,
        EventKind::RestartGaveUp,
        EventKind::ScheduleFailed,
        EventKind::ScheduleMissed,
        EventKind::ResourceAlert,
    ]
}

/// Events that may be sent at all. Webhook failures are excluded so a broken webhook cannot
/// trigger notifications about itself.
pub const NOTIFIABLE: &[EventKind] = &[
    EventKind::AppStarted,
    EventKind::AppStopped,
    EventKind::AppExited,
    EventKind::AppStartFailed,
    EventKind::RestartScheduled,
    EventKind::RestartGaveUp,
    EventKind::ScheduleSucceeded,
    EventKind::ScheduleFailed,
    EventKind::ScheduleMissed,
    EventKind::ResourceAlert,
    EventKind::ResourceRecovered,
    EventKind::EngineStarted,
    EventKind::EngineStopped,
    EventKind::ConfigError,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PayloadFormat {
    /// `{"source", "host", "time", "level", "kind", "app", "message"}`
    #[default]
    Json,
    /// `{"text": "..."}` for Slack incoming webhooks (and compatible services).
    Slack,
    /// `{"content": "..."}` for Discord webhooks.
    Discord,
}

pub fn validate(spec: &WebhookSpec) -> Vec<String> {
    let mut errors = Vec::new();
    if spec.name.trim().is_empty() {
        errors.push("name must not be empty".into());
    }
    let url = spec.url.expose();
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() < 10 {
        errors.push("URL must start with https:// (or http://)".into());
    }
    for name in spec.headers.keys() {
        let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
        if name.is_empty() || !name.chars().all(token) {
            errors.push(format!("invalid header name {name:?}"));
        }
    }
    if let Some(kind) = spec.events.iter().find(|k| !NOTIFIABLE.contains(k)) {
        errors.push(format!("{} events cannot be sent", kind.as_str()));
    }
    errors
}

/// The request body for `event`.
pub fn payload(format: PayloadFormat, event: &Event, host: &str) -> serde_json::Value {
    let app = event.app_name.as_deref();
    let level = event.level.as_str();
    let line = match app {
        Some(app) => format!("[Gravekeeper@{host}] {level}: {app}: {}", event.message),
        None => format!("[Gravekeeper@{host}] {level}: {}", event.message),
    };
    match format {
        PayloadFormat::Json => json!({
            "source": "gravekeeper",
            "host": host,
            "time": rfc3339(event.at),
            "level": level,
            "kind": event.kind.as_str(),
            "app": app,
            "message": event.message,
        }),
        PayloadFormat::Slack => json!({ "text": line }),
        PayloadFormat::Discord => json!({ "content": line }),
    }
}

fn rfc3339(t: SystemTime) -> String {
    jiff::Timestamp::try_from(t).map_or_else(|_| String::new(), |ts| ts.to_string())
}

/// Attempts per message, and the delay before the first retry (doubled each time).
pub const ATTEMPTS: u32 = 4;
pub const FIRST_RETRY: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a delivery failed, safe to show and log (contains no URL or header values).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryError {
    pub message: String,
    /// Worth trying again (network problems, 5xx, 429).
    pub retryable: bool,
}

pub struct Sender {
    #[cfg(feature = "http")]
    client: reqwest::Client,
}

impl Sender {
    pub fn new() -> Result<Self, String> {
        #[cfg(feature = "http")]
        {
            let client = reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .user_agent(concat!("Gravekeeper/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|e| e.without_url().to_string())?;
            Ok(Self { client })
        }
        #[cfg(not(feature = "http"))]
        Ok(Self {})
    }

    /// One attempt.
    pub async fn send(
        &self,
        spec: &WebhookSpec,
        body: &serde_json::Value,
    ) -> Result<(), DeliveryError> {
        #[cfg(feature = "http")]
        {
            let mut request = self
                .client
                .post(spec.url.expose())
                .header("content-type", "application/json")
                .body(body.to_string());
            for (name, value) in &spec.headers {
                request = request.header(name, value.expose());
            }
            match request.send().await {
                Ok(response) if response.status().is_success() => Ok(()),
                Ok(response) => {
                    let status = response.status();
                    Err(DeliveryError {
                        message: format!("server answered {status}"),
                        retryable: status.is_server_error() || status.as_u16() == 429,
                    })
                }
                Err(err) => {
                    let retryable = !err.is_builder();
                    Err(DeliveryError {
                        message: err.without_url().to_string(),
                        retryable,
                    })
                }
            }
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = (spec, body, REQUEST_TIMEOUT);
            Err(DeliveryError {
                message: "this build has no HTTP support".into(),
                retryable: false,
            })
        }
    }

    /// Sends with retries and exponential backoff. Returns the number of attempts made on
    /// success.
    pub async fn deliver(
        &self,
        spec: &WebhookSpec,
        body: &serde_json::Value,
    ) -> Result<u32, DeliveryError> {
        let mut delay = FIRST_RETRY;
        for attempt in 1..=ATTEMPTS {
            match self.send(spec, body).await {
                Ok(()) => return Ok(attempt),
                Err(err) if err.retryable && attempt < ATTEMPTS => {
                    tracing::debug!(webhook = %spec.name, attempt, error = %err.message, "retrying");
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
                Err(err) => return Err(err),
            }
        }
        unreachable!("the last attempt returns")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::EventLevel;

    fn event(kind: EventKind, app: Option<&str>) -> Event {
        Event {
            at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            level: EventLevel::Warning,
            kind,
            app_id: app.map(|a| AppId(a.into())),
            app_name: app.map(|a| a.to_uppercase()),
            message: "exited with code 1".into(),
        }
    }

    #[test]
    fn filters_by_kind_and_app() {
        let mut spec = WebhookSpec::new("ops", "https://example.com/hook");
        assert!(spec.wants(&event(EventKind::AppExited, Some("web"))));
        assert!(!spec.wants(&event(EventKind::AppStarted, Some("web"))));

        spec.apps = vec![AppId("db".into())];
        assert!(!spec.wants(&event(EventKind::AppExited, Some("web"))));
        assert!(spec.wants(&event(EventKind::AppExited, Some("db"))));
        assert!(spec.wants(&event(EventKind::ResourceAlert, None)));

        spec.enabled = false;
        assert!(!spec.wants(&event(EventKind::AppExited, Some("db"))));
    }

    #[test]
    fn payload_formats() {
        let e = event(EventKind::AppExited, Some("web"));
        let json = payload(PayloadFormat::Json, &e, "box");
        assert_eq!(json["kind"], "app_exited");
        assert_eq!(json["app"], "WEB");
        assert_eq!(json["time"], "2027-01-15T08:00:00Z");
        let slack = payload(PayloadFormat::Slack, &e, "box");
        assert_eq!(
            slack["text"],
            "[Gravekeeper@box] warning: WEB: exited with code 1"
        );
        let discord = payload(PayloadFormat::Discord, &e, "box");
        assert!(discord["content"].as_str().unwrap().contains("WEB"));
    }

    #[test]
    fn validation() {
        let mut spec = WebhookSpec::new("", "ftp://x");
        spec.headers.insert("Bad Header".into(), Secret::new("v"));
        spec.events.push(EventKind::WebhookFailed);
        assert_eq!(validate(&spec).len(), 4, "{:?}", validate(&spec));
        assert!(validate(&WebhookSpec::new("ok", "https://example.com/x")).is_empty());
    }

    #[test]
    fn debug_output_hides_secrets() {
        let mut spec = WebhookSpec::new("ops", "https://example.com/secret-token");
        spec.headers
            .insert("Authorization".into(), Secret::new("Bearer abc"));
        let debug = format!("{spec:?}");
        assert!(
            !debug.contains("secret-token") && !debug.contains("abc"),
            "{debug}"
        );
    }
}
