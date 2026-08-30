//! Security-sensitive helper types.

use std::{
    env, fmt,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use zeroize::Zeroize;

/// A secret value whose ordinary formatting paths always redact its contents.
///
/// The backing string is zeroized when the value is dropped. Provider-specific
/// secret lifecycle rules remain separate concerns.
#[derive(PartialEq, Eq)]
struct SecretValue(String);

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// A cheaply shareable secret value whose ordinary formatting paths always
/// redact its contents. Cloning this wrapper only clones an `Arc`; it does not
/// copy the secret bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(Arc<SecretValue>);

impl SecretString {
    /// Wrap a sensitive value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(Arc::new(SecretValue(value.into())))
    }

    /// Borrow the wrapped value for an explicitly authorized provider boundary.
    pub fn expose(&self) -> &str {
        self.0.as_ref().0.as_str()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString(REDACTED)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// Cooperative cancellation shared by request, credential, and session
/// acquisition. Cancelling a request never shuts down a profile session.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Replace a known secret and token-shaped values in text before it crosses
/// a user-visible boundary. This intentionally errs toward redaction: the
/// router must never make debug logging or an upstream error a credential
/// exfiltration path.
pub fn redact_sensitive_text(input: &str, known_secrets: &[&SecretString]) -> String {
    let mut output = input.to_owned();
    for secret in known_secrets {
        if !secret.expose().is_empty() {
            output = output.replace(secret.expose(), "[REDACTED]");
        }
    }
    for marker in ["ghp_", "gho_", "ghs_", "ghu_", "github_pat_"] {
        output = redact_marker(&output, marker);
    }
    output = redact_bearer_values(&output);
    output
}

fn redact_marker(input: &str, marker: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(relative) = input[cursor..].find(marker) {
        let start = cursor + relative;
        output.push_str(&input[cursor..start]);
        let end = input[start..]
            .char_indices()
            .skip(marker.len())
            .find(|(_, character)| {
                !character.is_ascii_alphanumeric() && *character != '_' && *character != '-'
            })
            .map_or(input.len(), |(index, _)| start + index);
        output.push_str("[REDACTED]");
        cursor = end;
        if cursor >= input.len() {
            break;
        }
    }
    output.push_str(&input[cursor..]);
    output
}

fn redact_bearer_values(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(relative) = input[cursor..].find("Bearer ") {
        let start = cursor + relative;
        output.push_str(&input[cursor..start]);
        let value_start = start + "Bearer ".len();
        let end = input[value_start..]
            .char_indices()
            .find(|(_, character)| {
                character.is_whitespace() || [',', '"', '\''].contains(character)
            })
            .map_or(input.len(), |(index, _)| value_start + index);
        output.push_str("Bearer [REDACTED]");
        cursor = end;
        if cursor >= input.len() {
            break;
        }
    }
    output.push_str(&input[cursor..]);
    output
}

/// Keep the child environment intentionally small. In particular, arbitrary
/// `GH_*`, cloud credential, and provider-specific variables are not inherited.
/// `PATH` is retained for bare executable names and locale/temp/home values are
/// retained because common CLIs need them for normal operation.
pub fn apply_minimal_child_environment(command: &mut Command) {
    const ALLOWED_EXACT: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TMPDIR",
        "TMP",
        "TEMP",
        "APPDATA",
        "LOCALAPPDATA",
        "SYSTEMROOT",
        "USERPROFILE",
        "XDG_CONFIG_HOME",
    ];

    command.env_clear();
    for (key, value) in env::vars_os() {
        let allowed = key
            .to_str()
            .is_some_and(|key| ALLOWED_EXACT.contains(&key) || key.starts_with("LC_"));
        if allowed {
            command.env(key, value);
        }
    }
}

/// Structured, secret-free routing metadata for diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogEvent {
    pub request_id: Option<String>,
    pub operation_class: Option<String>,
    pub repository: Option<String>,
    pub profile: Option<String>,
    pub matched_rule: Option<String>,
    pub upstream_session_id: Option<String>,
    pub result_status: Option<String>,
}

impl fmt::Display for LogEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fields = [
            ("request_id", self.request_id.as_deref()),
            ("operation_class", self.operation_class.as_deref()),
            ("repository", self.repository.as_deref()),
            ("profile", self.profile.as_deref()),
            ("matched_rule", self.matched_rule.as_deref()),
            ("upstream_session_id", self.upstream_session_id.as_deref()),
            ("result_status", self.result_status.as_deref()),
        ];
        let mut first = true;
        for (key, value) in fields
            .into_iter()
            .filter_map(|(key, value)| value.map(|value| (key, value)))
        {
            if !first {
                formatter.write_str(" ")?;
            }
            first = false;
            write!(formatter, "{key}={}", redact_sensitive_text(value, &[]))?;
        }
        Ok(())
    }
}

/// Documented log levels. `debug` and `trace` affect metadata volume only;
/// they never disable redaction. Set `GH_MCP_ROUTER_LOG_LEVEL` to one of
/// `error`, `warn`, `info`, `debug`, or `trace`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn from_env() -> Self {
        match env::var("GH_MCP_ROUTER_LOG_LEVEL")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "error" => Self::Error,
            "warn" | "warning" => Self::Warn,
            "debug" => Self::Debug,
            "trace" => Self::Trace,
            _ => Self::Info,
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        })
    }
}

/// Logger boundary used by the router. Implementations receive only typed,
/// non-secret metadata and must not be given raw MCP messages or credentials.
pub trait EventLogger: Send + Sync {
    fn log(&self, level: LogLevel, event: &LogEvent);
}

#[derive(Clone, Copy, Debug)]
pub struct StderrLogger {
    minimum: LogLevel,
}

impl StderrLogger {
    pub fn new(minimum: LogLevel) -> Self {
        Self { minimum }
    }
}

impl Default for StderrLogger {
    fn default() -> Self {
        Self::new(LogLevel::from_env())
    }
}

impl EventLogger for StderrLogger {
    fn log(&self, level: LogLevel, event: &LogEvent) {
        if level <= self.minimum {
            eprintln!("gh-mcp-router {level}: {event}");
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopLogger;

impl EventLogger for NoopLogger {
    fn log(&self, _level: LogLevel, _event: &LogEvent) {}
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::{redact_sensitive_text, LogEvent, SecretString};

    #[test]
    fn debug_format_redacts_secret_contents() {
        let secret = "ghp_TEST_SECRET_123456";

        let formatted = format!("{:?}", SecretString::new(secret));

        assert!(!formatted.contains(secret));
        assert_eq!(formatted, "SecretString(REDACTED)");
    }

    #[test]
    fn display_format_redacts_secret_contents() {
        let secret = "ghp_TEST_SECRET_123456";

        let formatted = SecretString::new(secret).to_string();

        assert!(!formatted.contains(secret));
        assert_eq!(formatted, "[REDACTED]");
    }

    #[test]
    fn diagnostic_redaction_covers_known_and_token_shaped_values() {
        let known = SecretString::new("opaque-secret");
        let input =
            "opaque-secret ghp_FAKE_TOKEN github_pat_FAKE_TOKEN Authorization: Bearer oauth-value";
        let formatted = redact_sensitive_text(input, &[&known]);

        assert!(!formatted.contains("opaque-secret"));
        assert!(!formatted.contains("ghp_FAKE_TOKEN"));
        assert!(!formatted.contains("github_pat_FAKE_TOKEN"));
        assert!(!formatted.contains("Bearer oauth-value"));
    }

    #[test]
    fn log_events_render_only_the_documented_metadata_shape() {
        let event = LogEvent {
            request_id: Some("request-1".to_owned()),
            operation_class: Some("write".to_owned()),
            repository: Some("ExampleOrg/project".to_owned()),
            profile: Some("work".to_owned()),
            matched_rule: Some("owner: ExampleOrg".to_owned()),
            upstream_session_id: Some("profile:work".to_owned()),
            result_status: Some("ok".to_owned()),
        };
        let formatted = event.to_string();

        assert!(formatted.contains("request_id=request-1"));
        assert!(formatted.contains("operation_class=write"));
        assert!(formatted.contains("upstream_session_id=profile:work"));
        assert!(!formatted.contains("token"));
    }

    #[test]
    fn child_environment_does_not_inherit_unapproved_variables() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "test -z \"${GH_MCP_ROUTER_TEST_SECRET-}\""])
            .env("GH_MCP_ROUTER_TEST_SECRET", "ghp_SYNTHETIC");
        super::apply_minimal_child_environment(&mut command);

        assert!(command.status().unwrap().success());
    }
}
