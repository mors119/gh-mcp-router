//! Credential-provider abstractions and the GitHub CLI implementation.
//!
//! This module owns credential lookup, but not profile or repository routing.
//! The GitHub CLI is invoked with an explicit host, user, and optional
//! `GH_CONFIG_DIR`; the process-global active account is never changed.

use std::{
    collections::BTreeMap,
    fmt,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use zeroize::Zeroize;

use crate::config::expand_path;
use crate::security::SecretString;

/// The host used when a credential reference does not specify one.
pub const DEFAULT_GITHUB_HOST: &str = "github.com";

const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// A non-secret reference to a credential provider and account/profile name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialRef {
    provider: String,
    name: String,
    host: Option<String>,
    gh_config_dir: Option<String>,
}

impl CredentialRef {
    /// Construct a provider-scoped credential reference.
    pub fn new(provider: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            name: name.into(),
            host: None,
            gh_config_dir: None,
        }
    }

    /// Attach the GitHub host for this provider reference.
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }

    /// Attach a profile-specific `GH_CONFIG_DIR` without expanding it yet.
    /// Expansion is performed at the provider boundary, after validation.
    pub fn with_gh_config_dir(mut self, path: impl Into<String>) -> Self {
        self.gh_config_dir = Some(path.into());
        self
    }

    /// Return the provider identifier.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Return the provider-specific credential name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the configured host, if one was supplied.
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// Return the configured, unexpanded `GH_CONFIG_DIR`, if one was supplied.
    pub fn gh_config_dir(&self) -> Option<&str> {
        self.gh_config_dir.as_deref()
    }
}

/// Stable categories for provider failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialErrorKind {
    /// The provider could not supply the requested credential.
    Unavailable,
    /// The reference was not valid for the provider.
    InvalidReference,
    /// The GitHub CLI executable could not be started.
    GhNotInstalled,
    /// The requested account is not authenticated for the host.
    AuthenticationMissing,
    /// The configured `GH_CONFIG_DIR` is not available.
    ConfigDirMissing,
    /// The requested host is not a valid GitHub host name.
    UnsupportedHost,
    /// The GitHub CLI returned data that could not be safely interpreted.
    MalformedOutput,
    /// The GitHub CLI did not finish within the configured timeout.
    Timeout,
    /// The GitHub CLI returned a failure status.
    CommandFailed,
}

/// Typed provider error that contains no credential value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialError {
    reference: CredentialRef,
    kind: CredentialErrorKind,
}

impl CredentialError {
    /// Construct an error for a provider-scoped reference.
    pub fn new(reference: CredentialRef, kind: CredentialErrorKind) -> Self {
        Self { reference, kind }
    }

    /// Return the non-secret reference associated with this error.
    pub fn reference(&self) -> &CredentialRef {
        &self.reference
    }

    /// Return the stable error category.
    pub fn kind(&self) -> CredentialErrorKind {
        self.kind
    }
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let description = match self.kind {
            CredentialErrorKind::Unavailable => "credential is unavailable",
            CredentialErrorKind::InvalidReference => "credential reference is invalid",
            CredentialErrorKind::GhNotInstalled => "the 'gh' CLI is not installed",
            CredentialErrorKind::AuthenticationMissing => "GitHub CLI authentication is missing",
            CredentialErrorKind::ConfigDirMissing => {
                "the configured GitHub CLI config directory is missing"
            }
            CredentialErrorKind::UnsupportedHost => "the GitHub host is unsupported",
            CredentialErrorKind::MalformedOutput => {
                "the GitHub CLI returned malformed authentication data"
            }
            CredentialErrorKind::Timeout => "the GitHub CLI command timed out",
            CredentialErrorKind::CommandFailed => "the GitHub CLI command failed",
        };

        write!(
            formatter,
            "{description} for provider '{}' and name '{}'",
            self.reference.provider(),
            self.reference.name()
        )
    }
}

impl std::error::Error for CredentialError {}

/// Boundary for resolving a non-secret reference into a secret value.
pub trait CredentialProvider {
    /// Resolve a reference without changing global authentication state.
    fn resolve(&self, reference: &CredentialRef) -> Result<SecretString, CredentialError>;
}

/// The source of a discovered authenticated account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    Gh,
}

impl fmt::Display for CredentialSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gh => formatter.write_str("gh"),
        }
    }
}

/// Non-secret account information returned by `gh auth status`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhAccount {
    pub host: String,
    pub user: String,
    pub authenticated: bool,
    pub source: CredentialSource,
}

/// A command request understood by the GitHub CLI runner.
///
/// Arguments are deliberately limited to command metadata. Credential values
/// are returned by stdout and are never placed in this structure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRequest {
    args: Vec<String>,
    gh_config_dir: Option<PathBuf>,
    timeout: Duration,
}

impl CommandRequest {
    fn new<I, S>(args: I, gh_config_dir: Option<PathBuf>, timeout: Duration) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            gh_config_dir,
            timeout,
        }
    }

    /// Return the command arguments for fake runners and diagnostics.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// Return the profile-specific config directory, if present.
    pub fn gh_config_dir(&self) -> Option<&Path> {
        self.gh_config_dir.as_deref()
    }

    /// Return the command timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// Output from a command runner. Its debug representation never contains
/// stdout or stderr, and both buffers are zeroized when dropped.
pub struct CommandOutput {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

impl CommandOutput {
    /// Construct successful output for a fake command runner.
    pub fn success(stdout: impl Into<String>, stderr: impl Into<String>) -> Self {
        Self {
            status: Some(0),
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    /// Construct failed output for a fake command runner.
    pub fn failure(status: i32, stdout: impl Into<String>, stderr: impl Into<String>) -> Self {
        Self {
            status: Some(status),
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    fn is_success(&self) -> bool {
        self.status == Some(0)
    }

    fn into_stdout(mut self) -> String {
        std::mem::take(&mut self.stdout)
    }
}

impl fmt::Debug for CommandOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandOutput")
            .field("status", &self.status)
            .field("stdout", &"[REDACTED]")
            .field("stderr", &"[REDACTED]")
            .finish()
    }
}

impl Drop for CommandOutput {
    fn drop(&mut self) {
        self.stdout.zeroize();
        self.stderr.zeroize();
    }
}

/// Errors raised while starting or supervising a subprocess.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandRunnerError {
    NotFound,
    Io,
    Timeout,
}

/// Injectable boundary for command execution. Tests use this instead of a
/// contributor's real GitHub login.
pub trait CommandRunner: Send + Sync {
    fn run(&self, request: CommandRequest) -> Result<CommandOutput, CommandRunnerError>;
}

/// Real subprocess runner for the `gh` executable.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessCommandRunner;

impl CommandRunner for ProcessCommandRunner {
    fn run(&self, request: CommandRequest) -> Result<CommandOutput, CommandRunnerError> {
        let mut command = Command::new("gh");
        command.args(&request.args);
        if let Some(config_dir) = &request.gh_config_dir {
            command.env("GH_CONFIG_DIR", config_dir);
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = command.spawn().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CommandRunnerError::NotFound
            } else {
                CommandRunnerError::Io
            }
        })?;

        let mut stdout = child.stdout.take().ok_or(CommandRunnerError::Io)?;
        let mut stderr = child.stderr.take().ok_or(CommandRunnerError::Io)?;
        let stdout_reader = thread::spawn(move || {
            let mut output = String::new();
            let result = stdout.read_to_string(&mut output);
            (result, output)
        });
        let stderr_reader = thread::spawn(move || {
            let mut output = String::new();
            let result = stderr.read_to_string(&mut output);
            (result, output)
        });

        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() >= request.timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(CommandRunnerError::Timeout);
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(CommandRunnerError::Io);
                }
            }
        };

        let (stdout_result, stdout) = stdout_reader.join().map_err(|_| CommandRunnerError::Io)?;
        let (stderr_result, stderr) = stderr_reader.join().map_err(|_| CommandRunnerError::Io)?;
        if stdout_result.is_err() || stderr_result.is_err() {
            return Err(CommandRunnerError::Io);
        }

        Ok(CommandOutput {
            status: status.code(),
            stdout,
            stderr,
        })
    }
}

/// GitHub CLI credential provider with injectable process execution.
pub struct GhCliCredentialProvider<R = ProcessCommandRunner> {
    runner: R,
    timeout: Duration,
}

impl GhCliCredentialProvider<ProcessCommandRunner> {
    /// Construct a provider that invokes the local `gh` executable.
    pub fn new() -> Self {
        Self {
            runner: ProcessCommandRunner,
            timeout: DEFAULT_COMMAND_TIMEOUT,
        }
    }
}

impl Default for GhCliCredentialProvider<ProcessCommandRunner> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R> GhCliCredentialProvider<R> {
    /// Construct a provider using an injectable command runner.
    pub fn with_runner(runner: R) -> Self {
        Self {
            runner,
            timeout: DEFAULT_COMMAND_TIMEOUT,
        }
    }

    /// Set the maximum duration of each `gh` subprocess.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl<R: CommandRunner> GhCliCredentialProvider<R> {
    /// Verify that the `gh` executable can be started without exposing output.
    pub fn verify_gh_installed(&self) -> Result<(), CredentialError> {
        let reference = CredentialRef::new("gh", "cli");
        match self
            .runner
            .run(CommandRequest::new(["--version"], None, self.timeout))
        {
            Ok(output) if output.is_success() => Ok(()),
            Ok(_output) => Err(CredentialError::new(
                reference,
                CredentialErrorKind::CommandFailed,
            )),
            Err(CommandRunnerError::NotFound) => Err(CredentialError::new(
                reference,
                CredentialErrorKind::GhNotInstalled,
            )),
            Err(CommandRunnerError::Timeout) => Err(CredentialError::new(
                reference,
                CredentialErrorKind::Timeout,
            )),
            Err(CommandRunnerError::Io) => Err(CredentialError::new(
                reference,
                CredentialErrorKind::CommandFailed,
            )),
        }
    }

    /// Discover all accounts known to `gh` for a profile's host and config.
    pub fn discover(&self, reference: &CredentialRef) -> Result<Vec<GhAccount>, CredentialError> {
        self.validate_reference(reference)?;
        let host = reference.host().unwrap_or(DEFAULT_GITHUB_HOST);
        let config_dir = self.config_dir(reference)?;
        let output = self.run(
            reference,
            CommandRequest::new(
                ["auth", "status", "--hostname", host, "--json", "hosts"],
                config_dir,
                self.timeout,
            ),
        )?;
        if !output.is_success() {
            return Err(CredentialError::new(
                reference.clone(),
                CredentialErrorKind::CommandFailed,
            ));
        }
        let mut stdout = output.into_stdout();
        let result = parse_accounts(&stdout, host, reference);
        stdout.zeroize();
        result
    }

    /// Verify that the exact configured account is authenticated without
    /// retrieving its token.
    pub fn verify_account(&self, reference: &CredentialRef) -> Result<GhAccount, CredentialError> {
        let host = reference.host().unwrap_or(DEFAULT_GITHUB_HOST);
        let account = self
            .discover(reference)?
            .into_iter()
            .find(|account| account.host == host && account.user == reference.name());
        match account {
            Some(account) if account.authenticated => Ok(account),
            _ => Err(CredentialError::new(
                reference.clone(),
                CredentialErrorKind::AuthenticationMissing,
            )),
        }
    }

    /// Return the selected account's credential only after exact account
    /// discovery succeeds.
    pub fn resolve_reference(
        &self,
        reference: &CredentialRef,
    ) -> Result<SecretString, CredentialError> {
        self.verify_account(reference)?;
        let host = reference.host().unwrap_or(DEFAULT_GITHUB_HOST);

        let output = self.run(
            reference,
            CommandRequest::new(
                [
                    "auth",
                    "token",
                    "--hostname",
                    host,
                    "--user",
                    reference.name(),
                ],
                self.config_dir(reference)?,
                self.timeout,
            ),
        )?;
        if !output.is_success() {
            return Err(CredentialError::new(
                reference.clone(),
                CredentialErrorKind::CommandFailed,
            ));
        }

        let mut token_output = output.into_stdout();
        let token = token_output.trim();
        if token.is_empty() || token.chars().any(char::is_whitespace) {
            token_output.zeroize();
            return Err(CredentialError::new(
                reference.clone(),
                CredentialErrorKind::MalformedOutput,
            ));
        }
        let secret = SecretString::new(token);
        token_output.zeroize();
        Ok(secret)
    }

    fn validate_reference(&self, reference: &CredentialRef) -> Result<(), CredentialError> {
        if reference.provider() != "gh" || reference.name().trim().is_empty() {
            return Err(CredentialError::new(
                reference.clone(),
                CredentialErrorKind::InvalidReference,
            ));
        }
        let host = reference.host().unwrap_or(DEFAULT_GITHUB_HOST);
        if !is_valid_host(host) {
            return Err(CredentialError::new(
                reference.clone(),
                CredentialErrorKind::UnsupportedHost,
            ));
        }
        Ok(())
    }

    fn config_dir(&self, reference: &CredentialRef) -> Result<Option<PathBuf>, CredentialError> {
        let Some(raw_path) = reference.gh_config_dir() else {
            return Ok(None);
        };
        let path = expand_path(raw_path).map_err(|_| {
            CredentialError::new(reference.clone(), CredentialErrorKind::InvalidReference)
        })?;
        if !path.is_dir() {
            return Err(CredentialError::new(
                reference.clone(),
                CredentialErrorKind::ConfigDirMissing,
            ));
        }
        Ok(Some(path))
    }

    fn run(
        &self,
        reference: &CredentialRef,
        request: CommandRequest,
    ) -> Result<CommandOutput, CredentialError> {
        self.runner.run(request).map_err(|error| {
            let kind = match error {
                CommandRunnerError::NotFound => CredentialErrorKind::GhNotInstalled,
                CommandRunnerError::Timeout => CredentialErrorKind::Timeout,
                CommandRunnerError::Io => CredentialErrorKind::CommandFailed,
            };
            CredentialError::new(reference.clone(), kind)
        })
    }
}

impl<R: CommandRunner> CredentialProvider for GhCliCredentialProvider<R> {
    fn resolve(&self, reference: &CredentialRef) -> Result<SecretString, CredentialError> {
        self.resolve_reference(reference)
    }
}

#[derive(Deserialize)]
struct AuthStatus {
    hosts: BTreeMap<String, Vec<AuthAccount>>,
}

#[derive(Deserialize)]
struct AuthAccount {
    #[serde(default)]
    login: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    authenticated: Option<bool>,
    #[serde(default)]
    state: Option<String>,
}

fn parse_accounts(
    stdout: &str,
    requested_host: &str,
    reference: &CredentialRef,
) -> Result<Vec<GhAccount>, CredentialError> {
    let parsed: AuthStatus = serde_json::from_str(stdout).map_err(|_| {
        CredentialError::new(reference.clone(), CredentialErrorKind::MalformedOutput)
    })?;
    let entries = parsed.hosts.get(requested_host).ok_or_else(|| {
        CredentialError::new(reference.clone(), CredentialErrorKind::MalformedOutput)
    })?;
    entries
        .iter()
        .map(|entry| {
            let user = entry
                .login
                .as_deref()
                .or(entry.user.as_deref())
                .filter(|user| !user.trim().is_empty())
                .ok_or_else(|| {
                    CredentialError::new(reference.clone(), CredentialErrorKind::MalformedOutput)
                })?;
            let authenticated = entry
                .authenticated
                .or_else(|| {
                    entry.state.as_deref().map(|state| {
                        matches!(
                            state.to_ascii_lowercase().as_str(),
                            "success" | "authenticated"
                        )
                    })
                })
                .unwrap_or(true);
            Ok(GhAccount {
                host: requested_host.to_owned(),
                user: user.to_owned(),
                authenticated,
                source: CredentialSource::Gh,
            })
        })
        .collect()
}

fn is_valid_host(host: &str) -> bool {
    !host.is_empty()
        && host != "."
        && !host.contains('/')
        && !host.contains(':')
        && !host.chars().any(char::is_whitespace)
        && !host.starts_with('.')
        && !host.ends_with('.')
        && host.split('.').all(|part| {
            !part.is_empty()
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        path::PathBuf,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use super::{
        CommandOutput, CommandRequest, CommandRunner, CommandRunnerError, CredentialError,
        CredentialErrorKind, CredentialProvider, CredentialRef, CredentialSource,
        GhCliCredentialProvider,
    };

    #[test]
    fn credential_reference_contains_identity_metadata_only() {
        let reference = CredentialRef::new("gh", "work-account");
        let error = CredentialError::new(reference.clone(), CredentialErrorKind::Unavailable);

        assert_eq!(reference.provider(), "gh");
        assert_eq!(reference.name(), "work-account");
        assert_eq!(error.reference(), &reference);
        assert!(!error.to_string().contains("token"));
    }

    #[derive(Clone)]
    struct FakeRunner {
        responses: Arc<Mutex<VecDeque<Result<CommandOutput, CommandRunnerError>>>>,
        requests: Arc<Mutex<Vec<CommandRequest>>>,
        active_account: Arc<Mutex<String>>,
    }

    impl FakeRunner {
        fn new(
            responses: impl IntoIterator<Item = Result<CommandOutput, CommandRunnerError>>,
        ) -> Self {
            Self {
                responses: Arc::new(Mutex::new(responses.into_iter().collect())),
                requests: Arc::new(Mutex::new(Vec::new())),
                active_account: Arc::new(Mutex::new("personal".to_owned())),
            }
        }

        fn requests(&self) -> Vec<CommandRequest> {
            self.requests.lock().unwrap().clone()
        }

        fn active_account(&self) -> String {
            self.active_account.lock().unwrap().clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, request: CommandRequest) -> Result<CommandOutput, CommandRunnerError> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("fake command response")
        }
    }

    fn status(accounts: &str) -> Result<CommandOutput, CommandRunnerError> {
        status_for("github.com", accounts)
    }

    fn status_for(host: &str, accounts: &str) -> Result<CommandOutput, CommandRunnerError> {
        Ok(CommandOutput::success(
            format!(r#"{{"hosts":{{"{host}":[{accounts}]}}}}"#),
            "",
        ))
    }

    fn reference(user: &str) -> CredentialRef {
        CredentialRef::new("gh", user).with_host("github.com")
    }

    #[test]
    fn discovers_multiple_accounts_without_switching_the_active_account() {
        let runner = FakeRunner::new([status(
            r#"{"login":"personal","active":true,"state":"success"},{"login":"work","active":false,"state":"success"}"#,
        )]);
        let provider = GhCliCredentialProvider::with_runner(runner.clone());
        let active_before = runner.active_account();

        let accounts = provider.discover(&reference("personal")).unwrap();

        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].user, "personal");
        assert_eq!(accounts[1].user, "work");
        assert!(accounts.iter().all(|account| account.authenticated));
        assert!(accounts
            .iter()
            .all(|account| account.source == CredentialSource::Gh));
        let requests = runner.requests();
        assert_eq!(requests[0].args()[..3], ["auth", "status", "--hostname"]);
        assert!(!requests
            .iter()
            .any(|request| request.args().contains(&"switch".to_owned())));
        assert_eq!(runner.active_account(), active_before);
    }

    #[test]
    fn resolves_two_profiles_to_two_accounts_and_keeps_tokens_redacted() {
        let runner = FakeRunner::new([
            status(r#"{"login":"personal","state":"success"}"#),
            Ok(CommandOutput::success("ghp_PERSONAL_FAKE\n", "")),
            status(r#"{"login":"work","state":"success"}"#),
            Ok(CommandOutput::success("github_pat_WORK_FAKE\n", "")),
        ]);
        let provider = GhCliCredentialProvider::with_runner(runner.clone());

        let personal = provider.resolve(&reference("personal")).unwrap();
        let work = provider.resolve(&reference("work")).unwrap();

        assert_eq!(personal.expose(), "ghp_PERSONAL_FAKE");
        assert_eq!(work.expose(), "github_pat_WORK_FAKE");
        assert!(!format!("{personal:?}").contains("ghp_PERSONAL_FAKE"));
        assert!(!work.to_string().contains("github_pat_WORK_FAKE"));
        let requests = runner.requests();
        assert_eq!(
            requests[1].args().last().map(String::as_str),
            Some("personal")
        );
        assert_eq!(requests[3].args().last().map(String::as_str), Some("work"));
        assert!(!requests.iter().any(|request| {
            request
                .args()
                .iter()
                .any(|argument| argument.contains("ghp_") || argument.contains("github_pat_"))
        }));
    }

    #[test]
    fn honors_profile_specific_config_directories() {
        let path = std::env::temp_dir();
        let runner = FakeRunner::new([status(r#"{"login":"work","state":"success"}"#)]);
        let reference = reference("work").with_gh_config_dir(path.to_string_lossy());
        let provider = GhCliCredentialProvider::with_runner(runner.clone());

        provider.discover(&reference).unwrap();

        assert_eq!(runner.requests()[0].gh_config_dir(), Some(path.as_path()));
    }

    #[test]
    fn passes_custom_github_enterprise_hosts_without_special_case_routing() {
        let runner = FakeRunner::new([status_for(
            "github.example.com",
            r#"{"login":"enterprise","state":"success"}"#,
        )]);
        let provider = GhCliCredentialProvider::with_runner(runner.clone());
        let reference = CredentialRef::new("gh", "enterprise").with_host("github.example.com");

        let accounts = provider.discover(&reference).unwrap();

        assert_eq!(accounts[0].host, "github.example.com");
        assert_eq!(runner.requests()[0].args()[3], "github.example.com");
    }

    #[test]
    fn missing_account_is_reported_without_revealing_command_output() {
        let runner = FakeRunner::new([status(r#"{"login":"other","state":"success"}"#)]);
        let provider = GhCliCredentialProvider::with_runner(runner);

        let error = provider.resolve(&reference("missing")).unwrap_err();

        assert_eq!(error.kind(), CredentialErrorKind::AuthenticationMissing);
        assert!(error.to_string().contains("missing"));
        assert!(!format!("{error:?}").contains("ghp_"));
    }

    #[test]
    fn expired_or_failed_authentication_is_not_usable() {
        let runner = FakeRunner::new([status(r#"{"login":"work","state":"failure"}"#)]);
        let provider = GhCliCredentialProvider::with_runner(runner);

        let error = provider.verify_account(&reference("work")).unwrap_err();

        assert_eq!(error.kind(), CredentialErrorKind::AuthenticationMissing);
    }

    #[test]
    fn command_failures_and_subprocess_text_are_redacted() {
        let runner = FakeRunner::new([
            status(r#"{"login":"work","state":"success"}"#),
            Ok(CommandOutput::failure(
                1,
                "github_pat_FAKE_TOKEN",
                "Authorization: Bearer github_pat_FAKE_TOKEN",
            )),
        ]);
        let provider = GhCliCredentialProvider::with_runner(runner);

        let error = provider.resolve(&reference("work")).unwrap_err();

        assert_eq!(error.kind(), CredentialErrorKind::CommandFailed);
        assert!(!error.to_string().contains("github_pat_FAKE_TOKEN"));
        assert!(!format!("{error:?}").contains("Authorization"));
    }

    #[test]
    fn handles_missing_gh_timeout_bad_host_and_malformed_output() {
        let not_installed = GhCliCredentialProvider::with_runner(FakeRunner::new([Err(
            CommandRunnerError::NotFound,
        )]));
        assert_eq!(
            not_installed.verify_gh_installed().unwrap_err().kind(),
            CredentialErrorKind::GhNotInstalled
        );

        let timed_out = GhCliCredentialProvider::with_runner(FakeRunner::new([Err(
            CommandRunnerError::Timeout,
        )]));
        assert_eq!(
            timed_out.verify_gh_installed().unwrap_err().kind(),
            CredentialErrorKind::Timeout
        );

        let bad_host = GhCliCredentialProvider::with_runner(FakeRunner::new([]));
        let error = bad_host
            .discover(&CredentialRef::new("gh", "user").with_host("https://github.com"))
            .unwrap_err();
        assert_eq!(error.kind(), CredentialErrorKind::UnsupportedHost);

        let malformed = GhCliCredentialProvider::with_runner(FakeRunner::new([Ok(
            CommandOutput::success("ghp_FAKE_TOKEN is not JSON", "Bearer ghp_FAKE_TOKEN"),
        )]));
        let error = malformed.discover(&reference("user")).unwrap_err();
        assert_eq!(error.kind(), CredentialErrorKind::MalformedOutput);
    }

    #[test]
    fn missing_config_directory_fails_before_running_gh() {
        let runner = FakeRunner::new([]);
        let provider = GhCliCredentialProvider::with_runner(runner.clone());
        let reference = reference("work").with_gh_config_dir(
            PathBuf::from("/definitely/missing/gh-config-dir").to_string_lossy(),
        );

        let error = provider.discover(&reference).unwrap_err();

        assert_eq!(error.kind(), CredentialErrorKind::ConfigDirMissing);
        assert!(runner.requests().is_empty());
    }

    #[test]
    fn provider_timeout_configuration_is_passed_to_fake_runner() {
        let runner = FakeRunner::new([status(r#"{"login":"user","state":"success"}"#)]);
        let provider = GhCliCredentialProvider::with_runner(runner.clone())
            .with_timeout(Duration::from_millis(5));

        provider.discover(&reference("user")).unwrap();

        assert_eq!(runner.requests()[0].timeout(), Duration::from_millis(5));
    }
}
