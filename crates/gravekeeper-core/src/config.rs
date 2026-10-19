//! The user's configuration: supervised applications and general settings, stored as TOML.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::monitor::MonitoringSettings;
use crate::paths::create_private_dir;
use crate::process::ProcessSpec;
use crate::schedule::{self, ScheduleSpec};
use crate::webhook::{self, WebhookSpec};

/// Bump when the file format changes incompatibly, and add a migration in [`migrate`].
///
/// - 1: applications and general settings
/// - 2: schedules, webhooks, monitoring and logging settings, `boot_delay`
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    #[serde(default)]
    pub general: GeneralSettings,
    #[serde(default)]
    pub monitoring: MonitoringSettings,
    #[serde(default)]
    pub logging: LoggingSettings,
    /// In start order: automatic starts happen top to bottom.
    #[serde(default, rename = "app")]
    pub apps: Vec<AppSpec>,
    #[serde(default, rename = "schedule")]
    pub schedules: Vec<ScheduleSpec>,
    #[serde(default, rename = "webhook")]
    pub webhooks: Vec<WebhookSpec>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            general: GeneralSettings::default(),
            monitoring: MonitoringSettings::default(),
            logging: LoggingSettings::default(),
            apps: Vec::new(),
            schedules: Vec::new(),
            webhooks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingSettings {
    /// Also write each application's output to `<data>/logs/<id>.log`.
    pub app_log_files: bool,
    /// Rotate a log file when it grows beyond this many MiB.
    pub max_file_mb: u64,
    /// Rotated files kept per log (`.1` is the newest).
    pub keep_files: u32,
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            app_log_files: true,
            max_file_mb: 10,
            keep_files: 5,
        }
    }
}

/// A value that must not appear in logs or debug output (webhook URLs, tokens).
///
/// It is still stored in plain text in the configuration file, which is readable only by
/// the user on Unix. Moving secrets to the OS credential store is planned.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// A new identifier that is unique in practice (time-based, with a process-wide counter).
pub(crate) fn generate_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}{n:x}")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralSettings {
    /// Start with only the tray icon, without opening the window.
    pub start_minimized: bool,
    /// Lines of stdout/stderr kept in memory per application.
    pub log_lines: usize,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            start_minimized: false,
            log_lines: 5000,
        }
    }
}

/// Stable identifier of a supervised application, independent of its (editable) name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AppId(pub String);

impl AppId {
    pub fn generate() -> Self {
        Self(generate_id())
    }
}

impl fmt::Display for AppId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSpec {
    pub id: AppId,
    pub name: String,
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Disabled applications are never started.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Start when Gravekeeper starts.
    #[serde(default)]
    pub autostart: bool,
    /// Wait this long after Gravekeeper starts before an automatic start.
    #[serde(default, with = "secs")]
    pub start_delay: Duration,
    /// Do not start automatically before the computer has been up this long.
    #[serde(default, with = "secs")]
    pub boot_delay: Duration,
    /// The process must stay up this long to count as started; exiting earlier is a
    /// start failure.
    #[serde(default = "default_startup_grace", with = "secs")]
    pub startup_grace: Duration,
    /// How long a stop request may take before the process tree is killed.
    #[serde(default = "default_stop_timeout", with = "secs")]
    pub stop_timeout: Duration,
    #[serde(default)]
    pub restart: RestartPolicy,
}

impl AppSpec {
    pub fn new(name: impl Into<String>, program: impl Into<PathBuf>) -> Self {
        Self {
            id: AppId::generate(),
            name: name.into(),
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            enabled: true,
            autostart: false,
            start_delay: Duration::ZERO,
            boot_delay: Duration::ZERO,
            startup_grace: default_startup_grace(),
            stop_timeout: default_stop_timeout(),
            restart: RestartPolicy::default(),
        }
    }

    /// The directory the application runs in: `cwd`, or else the folder of the program.
    pub fn working_dir(&self) -> Option<PathBuf> {
        self.cwd.clone().or_else(|| program_dir(&self.program))
    }

    pub fn process_spec(&self) -> ProcessSpec {
        ProcessSpec {
            program: self.program.clone(),
            args: self.args.iter().map(Into::into).collect(),
            cwd: self.working_dir(),
            env: self.env.iter().map(|(k, v)| (k.into(), v.into())).collect(),
        }
    }
}

/// The folder of an absolute program path. A bare name (found through `PATH`) or a relative
/// path has none: those are resolved against Gravekeeper's own working directory, so the
/// application keeps that one.
pub fn program_dir(program: &Path) -> Option<PathBuf> {
    program
        .is_absolute()
        .then(|| program.parent())
        .flatten()
        .map(Path::to_path_buf)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartMode {
    Never,
    /// Restart unless the process exited with code 0.
    OnFailure,
    Always,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RestartPolicy {
    pub mode: RestartMode,
    /// Delay before the first restart; doubled after each consecutive failure.
    #[serde(with = "secs")]
    pub backoff_initial: Duration,
    #[serde(with = "secs")]
    pub backoff_max: Duration,
    /// Give up after more than this many restarts within `window`.
    pub max_restarts: u32,
    #[serde(with = "secs")]
    pub window: Duration,
    /// Running at least this long resets the backoff.
    #[serde(with = "secs")]
    pub stable_after: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            mode: RestartMode::OnFailure,
            backoff_initial: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            max_restarts: 5,
            window: Duration::from_secs(300),
            stable_after: Duration::from_secs(60),
        }
    }
}

pub(crate) fn yes() -> bool {
    true
}

fn default_startup_grace() -> Duration {
    Duration::from_secs(1)
}

fn default_stop_timeout() -> Duration {
    Duration::from_secs(10)
}

/// Durations are written as whole seconds in the file.
pub(crate) mod secs {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        u64::deserialize(d).map(Duration::from_secs)
    }
}

/// A problem that makes a configuration unusable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// Name of the application, schedule or webhook concerned, if any.
    pub subject: Option<String>,
    pub message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.subject {
            Some(subject) => write!(f, "{subject}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// Checks a single application on its own.
pub fn validate_app(app: &AppSpec) -> Vec<ValidationError> {
    let label = if app.name.trim().is_empty() {
        app.id.to_string()
    } else {
        app.name.clone()
    };
    let error = |message: &str| ValidationError {
        subject: Some(label.clone()),
        message: message.to_owned(),
    };

    let mut errors = Vec::new();
    if app.id.0.trim().is_empty() {
        errors.push(error("id must not be empty"));
    }
    if app.name.trim().is_empty() {
        errors.push(error("name must not be empty"));
    }
    if app.program.as_os_str().is_empty() {
        errors.push(error("program must not be empty"));
    }
    if app.env.keys().any(|k| k.is_empty() || k.contains('=')) {
        errors.push(error(
            "environment variable names must be non-empty and contain no '='",
        ));
    }
    let r = &app.restart;
    if r.backoff_initial.is_zero() {
        errors.push(error("initial backoff must be at least 1 second"));
    }
    if r.backoff_initial > r.backoff_max {
        errors.push(error("initial backoff must not exceed maximum backoff"));
    }
    if r.window.is_zero() {
        errors.push(error("restart window must be at least 1 second"));
    }
    errors
}

/// Checks the whole configuration, including cross-application rules.
pub fn validate(config: &Config) -> Vec<ValidationError> {
    let mut errors: Vec<_> = config.apps.iter().flat_map(validate_app).collect();
    if config.general.log_lines == 0 {
        errors.push(ValidationError {
            subject: None,
            message: "log lines must be at least 1".into(),
        });
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for app in &config.apps {
        if !ids.insert(&app.id) {
            errors.push(ValidationError {
                subject: Some(app.name.clone()),
                message: format!("duplicate id {}", app.id),
            });
        }
        if !names.insert(app.name.trim()) {
            errors.push(ValidationError {
                subject: Some(app.name.clone()),
                message: "another application has the same name".into(),
            });
        }
    }

    let general = |message: &str| ValidationError {
        subject: None,
        message: message.to_owned(),
    };
    if config.monitoring.interval.is_zero() {
        errors.push(general("monitoring interval must be at least 1 second"));
    }
    let m = &config.monitoring;
    if [m.cpu_alert, m.memory_alert, m.disk_alert]
        .iter()
        .any(|p| *p > 100)
    {
        errors.push(general(
            "alert thresholds are percentages (0 to turn off, up to 100)",
        ));
    }
    if config.logging.max_file_mb == 0 {
        errors.push(general("log file size must be at least 1 MiB"));
    }

    let mut schedule_ids = HashSet::new();
    for spec in &config.schedules {
        let error = |message: String| ValidationError {
            subject: Some(spec.name.clone()),
            message,
        };
        errors.extend(schedule::validate(spec).into_iter().map(error));
        if !schedule_ids.insert(&spec.id) {
            errors.push(error(format!("duplicate id {}", spec.id)));
        }
        if let Some(app) = spec.action.app()
            && !ids.contains(app)
        {
            errors.push(error(format!("refers to an unknown application ({app})")));
        }
    }

    let mut webhook_ids = HashSet::new();
    for spec in &config.webhooks {
        let error = |message: String| ValidationError {
            subject: Some(spec.name.clone()),
            message,
        };
        errors.extend(webhook::validate(spec).into_iter().map(error));
        if !webhook_ids.insert(&spec.id) {
            errors.push(error(format!("duplicate id {}", spec.id)));
        }
    }
    errors
}

#[derive(Debug)]
pub enum LoadError {
    Io(io::Error),
    Parse(String),
    UnsupportedVersion(u32),
    Invalid(Vec<ValidationError>),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "cannot read the configuration: {err}"),
            Self::Parse(err) => write!(f, "the configuration is not valid TOML: {err}"),
            Self::UnsupportedVersion(v) => write!(
                f,
                "the configuration has schema version {v}, but this build supports up to {SCHEMA_VERSION}; \
                 it was probably written by a newer Gravekeeper"
            ),
            Self::Invalid(errors) => {
                f.write_str("the configuration is invalid:")?;
                for error in errors {
                    write!(f, "\n- {error}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// Reads the configuration. A missing file yields the default configuration.
pub fn load(path: &Path) -> Result<Config, LoadError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(err) => return Err(LoadError::Io(err)),
    };
    parse(&text)
}

pub fn parse(text: &str) -> Result<Config, LoadError> {
    #[derive(Deserialize)]
    struct Version {
        schema_version: Option<u32>,
    }
    let version: Version = toml::from_str(text).map_err(|e| LoadError::Parse(e.to_string()))?;
    let version = version
        .schema_version
        .ok_or_else(|| LoadError::Parse("missing schema_version".into()))?;
    if version > SCHEMA_VERSION {
        return Err(LoadError::UnsupportedVersion(version));
    }
    let config = migrate(version, text)?;
    let errors = validate(&config);
    if errors.is_empty() {
        Ok(config)
    } else {
        Err(LoadError::Invalid(errors))
    }
}

/// Converts a file of an older schema version to the current [`Config`].
fn migrate(version: u32, text: &str) -> Result<Config, LoadError> {
    let parse_error = |e: toml::de::Error| LoadError::Parse(e.to_string());
    match version {
        SCHEMA_VERSION => toml::from_str(text).map_err(parse_error),
        // Version 2 only added optional sections and fields.
        1 => {
            let mut table: toml::Table = toml::from_str(text).map_err(parse_error)?;
            table.insert("schema_version".into(), i64::from(SCHEMA_VERSION).into());
            toml::Value::Table(table).try_into().map_err(parse_error)
        }
        other => Err(LoadError::UnsupportedVersion(other)),
    }
}

/// Validates and writes the configuration atomically, keeping the previous file as `.bak`.
pub fn save(path: &Path, config: &Config) -> io::Result<()> {
    let errors = validate(config);
    if !errors.is_empty() {
        let message = errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
    }
    let text = toml::to_string_pretty(config).map_err(io::Error::other)?;

    let dir = path.parent().unwrap_or(Path::new("."));
    create_private_dir(dir)?;
    let tmp = path.with_extension("toml.tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    if path.exists() {
        std::fs::copy(path, path.with_extension("toml.bak"))?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Config {
        let mut app = AppSpec::new("web", "/usr/bin/web");
        app.args = vec!["--port".into(), "8080".into()];
        app.env.insert("RUST_LOG".into(), "info".into());
        app.autostart = true;
        Config {
            apps: vec![app],
            ..Config::default()
        }
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/config.toml");
        let config = sample();
        save(&path, &config).unwrap();
        assert_eq!(load(&path).unwrap(), config);

        // Saving again keeps the previous version as a backup.
        save(&path, &Config::default()).unwrap();
        assert_eq!(load(&path).unwrap(), Config::default());
        let backup = path.with_extension("toml.bak");
        assert_eq!(load(&backup).unwrap(), config);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save(&path, &sample()).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn working_dir_defaults_to_the_program_folder() {
        let (program, folder) = if cfg!(windows) {
            (r"C:\Tools\web\web.exe", r"C:\Tools\web")
        } else {
            ("/opt/web/bin/web", "/opt/web/bin")
        };
        let mut app = AppSpec::new("web", program);
        assert_eq!(app.process_spec().cwd, Some(PathBuf::from(folder)));
        app.cwd = Some("elsewhere".into());
        assert_eq!(app.process_spec().cwd, Some(PathBuf::from("elsewhere")));

        // Found through PATH, or relative to Gravekeeper's own directory: left alone.
        for program in ["python", "bin/web"] {
            assert_eq!(AppSpec::new("x", program).process_spec().cwd, None);
        }
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load(&dir.path().join("none.toml")).unwrap(),
            Config::default()
        );
    }

    #[test]
    fn minimal_app_uses_defaults() {
        let config = parse(
            r#"
            schema_version = 2
            [[app]]
            id = "a1"
            name = "worker"
            program = "/bin/worker"
            "#,
        )
        .unwrap();
        let app = &config.apps[0];
        assert!(app.enabled);
        assert!(!app.autostart);
        assert_eq!(app.stop_timeout, Duration::from_secs(10));
        assert_eq!(app.restart, RestartPolicy::default());
    }

    #[test]
    fn rejects_broken_and_unknown_input() {
        assert!(matches!(parse("not = [toml"), Err(LoadError::Parse(_))));
        assert!(matches!(parse("x = 1"), Err(LoadError::Parse(_))));
        assert!(matches!(
            parse("schema_version = 99"),
            Err(LoadError::UnsupportedVersion(99))
        ));
        // Typos are reported instead of silently ignored.
        assert!(matches!(
            parse("schema_version = 2\n[general]\nstart_minimised = true"),
            Err(LoadError::Parse(_))
        ));
    }

    #[test]
    fn validation_catches_mistakes() {
        let mut config = sample();
        let mut twin = config.apps[0].clone();
        twin.restart.backoff_initial = Duration::from_secs(100);
        config.apps.push(twin);
        config.apps[0].name = " ".into();

        let messages: Vec<_> = validate(&config).into_iter().map(|e| e.message).collect();
        assert!(
            messages
                .iter()
                .any(|m| m.contains("name must not be empty"))
        );
        assert!(messages.iter().any(|m| m.contains("duplicate id")));
        assert!(messages.iter().any(|m| m.contains("must not exceed")));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(save(&path, &config).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn migrates_version_1() {
        let config = parse(
            r#"
            schema_version = 1
            [general]
            start_minimized = true
            [[app]]
            id = "a1"
            name = "worker"
            program = "/bin/worker"
            "#,
        )
        .unwrap();
        assert_eq!(config.schema_version, SCHEMA_VERSION);
        assert!(config.general.start_minimized);
        assert_eq!(config.apps[0].boot_delay, Duration::ZERO);
        assert_eq!(config.monitoring, MonitoringSettings::default());
    }

    #[test]
    fn schedules_must_refer_to_existing_apps() {
        let config = parse(
            r#"
            schema_version = 2
            [[schedule]]
            id = "s1"
            name = "Nightly"
            when = { kind = "daily", time = "03:00" }
            action = { kind = "restart", app = "missing" }
            "#,
        );
        match config {
            Err(LoadError::Invalid(errors)) => {
                assert!(
                    errors[0].message.contains("unknown application"),
                    "{errors:?}"
                )
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn secrets_are_hidden_from_debug_output() {
        let secret = Secret::new("https://hooks.example.com/T000/B000/XXXX");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        assert_eq!(secret.expose(), "https://hooks.example.com/T000/B000/XXXX");
    }

    #[test]
    fn generated_ids_are_unique() {
        let ids: HashSet<_> = (0..1000).map(|_| AppId::generate()).collect();
        assert_eq!(ids.len(), 1000);
    }
}
