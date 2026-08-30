//! Profile-isolated sessions for the official GitHub MCP Server.
//!
//! The router starts one local stdio process per configured profile. A session
//! is permanently bound to the non-secret [`CredentialRef`] that created it;
//! it is never reassigned when routing decisions change. The upstream process
//! receives its credential through `GITHUB_PERSONAL_ACCESS_TOKEN` in its child
//! environment, never through command-line arguments or the router's global
//! environment.

use std::{
    collections::BTreeMap,
    fmt,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::{
    credentials::{CredentialError, CredentialProvider, CredentialRef},
    security::SecretString,
};

const DEFAULT_BINARY: &str = "github-mcp-server";
const DEFAULT_START_ARGS: &[&str] = &["stdio"];
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// Configuration for the official GitHub MCP stdio executable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpstreamConfig {
    binary: PathBuf,
    args: Vec<String>,
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            binary: PathBuf::from(DEFAULT_BINARY),
            args: DEFAULT_START_ARGS
                .iter()
                .map(|arg| (*arg).to_owned())
                .collect(),
        }
    }
}

impl UpstreamConfig {
    /// Use an explicit executable path, or a bare name resolved through PATH.
    pub fn with_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.binary = binary.into();
        self
    }

    /// Override the upstream startup arguments. The default is `stdio`.
    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }
}

/// Secret-bearing child-process environment values.
///
/// The launch request is consumed by a launcher. This makes it possible for
/// the real launcher to hand the credential to the child and then drop the
/// request, minimizing the time the token remains in router-owned memory.
#[derive(Clone, PartialEq, Eq)]
pub struct UpstreamEnvironment(BTreeMap<String, SecretString>);

impl fmt::Debug for UpstreamEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_map()
            .entries(self.0.keys().map(|key| (key, "[REDACTED]")))
            .finish()
    }
}

impl UpstreamEnvironment {
    fn for_credential(reference: &CredentialRef, credential: SecretString) -> Self {
        let mut values = BTreeMap::new();
        values.insert("GITHUB_PERSONAL_ACCESS_TOKEN".to_owned(), credential);
        if let Some(host) = reference.host() {
            let host = if host.eq_ignore_ascii_case("github.com") {
                "https://github.com".to_owned()
            } else {
                format!("https://{host}")
            };
            values.insert("GITHUB_HOST".to_owned(), SecretString::new(host));
        }
        if let Some(config_dir) = reference.gh_config_dir() {
            values.insert("GH_CONFIG_DIR".to_owned(), SecretString::new(config_dir));
        }
        Self(values)
    }

    /// Explicitly inspect an environment value at the child-process boundary.
    /// Ordinary formatting remains redacted.
    pub fn get(&self, key: &str) -> Option<&SecretString> {
        self.0.get(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

/// Metadata and environment passed to an upstream launcher.
#[derive(Clone, PartialEq, Eq)]
pub struct UpstreamLaunchRequest {
    binary: PathBuf,
    args: Vec<String>,
    environment: UpstreamEnvironment,
}

impl fmt::Debug for UpstreamLaunchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UpstreamLaunchRequest")
            .field("binary", &self.binary)
            .field("args", &self.args)
            .field("environment", &self.environment)
            .finish()
    }
}

impl UpstreamLaunchRequest {
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// Return the environment for an explicit launcher boundary inspection.
    pub fn environment(&self) -> &UpstreamEnvironment {
        &self.environment
    }
}

/// Stable categories for upstream process failures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpstreamProcessError {
    Io,
    Exited,
    InvalidResponse,
}

impl fmt::Display for UpstreamProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Io => "upstream process I/O failed",
            Self::Exited => "upstream process exited",
            Self::InvalidResponse => "upstream process returned an invalid response",
        })
    }
}

impl std::error::Error for UpstreamProcessError {}

/// A running upstream process. Implementations must not include payloads or
/// child stderr in errors, because upstream messages may contain sensitive
/// data in future protocol versions.
pub trait UpstreamProcess: Send + Sync {
    fn send(&self, message: &str) -> Result<String, UpstreamProcessError>;
    /// Send a JSON-RPC notification that does not have a response.
    fn notify(&self, message: &str) -> Result<(), UpstreamProcessError>;
    fn is_alive(&self) -> bool;
    fn shutdown(&self);
}

/// Injectable process-launch boundary used by tests and alternate runtimes.
pub trait UpstreamLauncher: Send + Sync {
    fn launch(
        &self,
        request: UpstreamLaunchRequest,
    ) -> Result<Box<dyn UpstreamProcess>, UpstreamLaunchError>;
}

/// Failures while creating an upstream process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpstreamLaunchError {
    BinaryNotFound { binary: PathBuf },
    Io,
}

impl fmt::Display for UpstreamLaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BinaryNotFound { binary } => {
                write!(
                    formatter,
                    "GitHub MCP upstream binary '{}' was not found",
                    binary.display()
                )
            }
            Self::Io => formatter.write_str("GitHub MCP upstream process could not be started"),
        }
    }
}

impl std::error::Error for UpstreamLaunchError {}

/// The production launcher for `github-mcp-server`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessUpstreamLauncher;

impl UpstreamLauncher for ProcessUpstreamLauncher {
    fn launch(
        &self,
        request: UpstreamLaunchRequest,
    ) -> Result<Box<dyn UpstreamProcess>, UpstreamLaunchError> {
        let mut command = Command::new(&request.binary);
        command.args(&request.args);
        for (key, value) in &request.environment.0 {
            command.env(key, value.expose());
        }
        // Never inherit stderr: the upstream process may print sensitive
        // diagnostics and the router must not accidentally expose them.
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = command.spawn().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                UpstreamLaunchError::BinaryNotFound {
                    binary: request.binary.clone(),
                }
            } else {
                UpstreamLaunchError::Io
            }
        })?;
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(UpstreamLaunchError::Io);
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(UpstreamLaunchError::Io);
        };

        Ok(Box::new(ProcessUpstreamSession {
            child: Mutex::new(child),
            stdin: Mutex::new(Some(BufWriter::new(stdin))),
            stdout: Mutex::new(BufReader::new(stdout)),
        }))
    }
}

struct ProcessUpstreamSession {
    child: Mutex<Child>,
    stdin: Mutex<Option<BufWriter<ChildStdin>>>,
    stdout: Mutex<BufReader<ChildStdout>>,
}

impl UpstreamProcess for ProcessUpstreamSession {
    fn send(&self, message: &str) -> Result<String, UpstreamProcessError> {
        if !self.is_alive() {
            return Err(UpstreamProcessError::Exited);
        }
        self.write_message(message)?;
        let expected_id = serde_json::from_str::<serde_json::Value>(message)
            .ok()
            .and_then(|value| value.get("id").cloned());
        let expected_id = expected_id.ok_or(UpstreamProcessError::InvalidResponse)?;

        let mut stdout = self.stdout.lock().map_err(|_| UpstreamProcessError::Io)?;
        loop {
            let mut response = String::new();
            let bytes = stdout
                .read_line(&mut response)
                .map_err(|_| UpstreamProcessError::Io)?;
            if bytes == 0 {
                return Err(UpstreamProcessError::Exited);
            }
            let response_text = response.trim_end_matches(['\r', '\n']);
            if response_matches_id(response_text, &expected_id)? {
                return Ok(response_text.to_owned());
            }
            // Notifications and server requests may be interleaved with a
            // response. They are consumed here so they cannot become the
            // response to the wrong request or corrupt the next exchange.
        }
    }

    fn notify(&self, message: &str) -> Result<(), UpstreamProcessError> {
        if !self.is_alive() {
            return Err(UpstreamProcessError::Exited);
        }
        self.write_message(message)
    }

    fn is_alive(&self) -> bool {
        self.child
            .lock()
            .ok()
            .and_then(|mut child| child.try_wait().map(|status| status.is_none()).ok())
            .unwrap_or(false)
    }

    fn shutdown(&self) {
        // Closing stdin gives a well-behaved stdio server a chance to exit.
        if let Ok(mut stdin) = self.stdin.lock() {
            stdin.take();
        }
        let started = Instant::now();
        while started.elapsed() < SHUTDOWN_TIMEOUT {
            match self
                .child
                .lock()
                .ok()
                .and_then(|mut child| child.try_wait().ok())
            {
                Some(Some(_)) => return,
                Some(None) => thread::sleep(Duration::from_millis(10)),
                None => break,
            }
        }
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl ProcessUpstreamSession {
    fn write_message(&self, message: &str) -> Result<(), UpstreamProcessError> {
        let mut stdin_guard = self.stdin.lock().map_err(|_| UpstreamProcessError::Io)?;
        let stdin = stdin_guard.as_mut().ok_or(UpstreamProcessError::Exited)?;
        stdin
            .write_all(message.as_bytes())
            .map_err(|_| UpstreamProcessError::Io)?;
        if !message.ends_with('\n') {
            stdin
                .write_all(b"\n")
                .map_err(|_| UpstreamProcessError::Io)?;
        }
        stdin.flush().map_err(|_| UpstreamProcessError::Io)
    }
}

fn response_matches_id(
    response: &str,
    expected_id: &serde_json::Value,
) -> Result<bool, UpstreamProcessError> {
    let value = serde_json::from_str::<serde_json::Value>(response)
        .map_err(|_| UpstreamProcessError::InvalidResponse)?;
    Ok(value.get("id") == Some(expected_id))
}

impl Drop for ProcessUpstreamSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A public snapshot of the manager's lifecycle state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub profile: String,
    pub active: bool,
}

/// Errors returned by the profile session manager.
#[derive(Debug)]
pub enum UpstreamError {
    Credential(CredentialError),
    Launch(UpstreamLaunchError),
    Process(UpstreamProcessError),
    ProfileIdentityMismatch { profile: String },
}

impl fmt::Display for UpstreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credential(error) => error.fmt(formatter),
            Self::Launch(error) => error.fmt(formatter),
            Self::Process(error) => error.fmt(formatter),
            Self::ProfileIdentityMismatch { profile } => write!(
                formatter,
                "profile '{profile}' is already bound to a different credential reference"
            ),
        }
    }
}

impl std::error::Error for UpstreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Credential(error) => Some(error),
            Self::Launch(error) => Some(error),
            Self::Process(error) => Some(error),
            Self::ProfileIdentityMismatch { .. } => None,
        }
    }
}

struct ManagedSession {
    credential: CredentialRef,
    process: Option<Arc<dyn UpstreamProcess>>,
    request_lock: Arc<Mutex<()>>,
}

/// Lazily creates and caches one upstream session per configured profile.
///
/// The map lock only protects session lookup. Each profile has its own lock,
/// so different profiles may start and serve concurrently while same-profile
/// startup is serialized and cannot spawn duplicate children.
pub struct UpstreamSessionManager<C, L = ProcessUpstreamLauncher> {
    credential_provider: C,
    launcher: L,
    config: UpstreamConfig,
    sessions: Mutex<BTreeMap<String, Arc<Mutex<ManagedSession>>>>,
}

impl<C, L> UpstreamSessionManager<C, L> {
    pub fn new(credential_provider: C, launcher: L, config: UpstreamConfig) -> Self {
        Self {
            credential_provider,
            launcher,
            config,
            sessions: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn session_count(&self) -> usize {
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .len()
    }

    pub fn session_info(&self) -> Vec<SessionInfo> {
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .iter()
            .map(|(profile, session)| SessionInfo {
                profile: profile.clone(),
                active: session
                    .lock()
                    .expect("session lock poisoned")
                    .process
                    .as_ref()
                    .is_some_and(|process| process.is_alive()),
            })
            .collect()
    }
}

impl<C: CredentialProvider + Send + Sync, L: UpstreamLauncher> UpstreamSessionManager<C, L> {
    /// Start or reuse the session bound to `profile` and `credential`.
    pub fn start(
        &self,
        profile: impl Into<String>,
        credential: &CredentialRef,
    ) -> Result<SessionInfo, UpstreamError> {
        let profile = profile.into();
        let session = self.session_for(&profile, credential)?;
        let mut session = session.lock().expect("session lock poisoned");
        self.ensure_process(&mut session)?;
        Ok(SessionInfo {
            profile,
            active: true,
        })
    }

    /// Send one newline-delimited MCP message through a profile's session.
    ///
    /// A failed request is not retried: a write could have reached the
    /// upstream process before its response failed. The process is discarded,
    /// and the next request starts a fresh session safely.
    pub fn send(
        &self,
        profile: impl Into<String>,
        credential: &CredentialRef,
        message: &str,
    ) -> Result<String, UpstreamError> {
        self.send_with_startup(profile, credential, message, |_| Ok(()))
    }

    /// Send a request, running `startup` only when a new child process had to
    /// be created. This lets protocol owners re-run their handshake after a
    /// process restart without retrying the original request implicitly.
    pub fn send_with_startup<F>(
        &self,
        profile: impl Into<String>,
        credential: &CredentialRef,
        message: &str,
        startup: F,
    ) -> Result<String, UpstreamError>
    where
        F: FnOnce(&dyn UpstreamProcess) -> Result<(), UpstreamProcessError>,
    {
        let profile = profile.into();
        let session = self.session_for(&profile, credential)?;
        let request_lock = session
            .lock()
            .expect("session lock poisoned")
            .request_lock
            .clone();
        let _request_guard = request_lock.lock().expect("request lock poisoned");

        let (session, started) = {
            let mut session = session.lock().expect("session lock poisoned");
            let started = self.ensure_process(&mut session)?;
            let process = session
                .process
                .as_ref()
                .expect("ensure_process installs a process")
                .clone();
            (process, started)
        };
        if started {
            if let Err(error) = startup(session.as_ref()) {
                let sessions = self.sessions.lock().expect("session map lock poisoned");
                if let Some(managed) = sessions.get(&profile) {
                    managed
                        .lock()
                        .expect("session lock poisoned")
                        .process
                        .take();
                }
                return Err(UpstreamError::Process(error));
            }
        }
        let result = session.send(message);
        match result {
            Ok(response) => Ok(response),
            Err(error) => {
                let managed = self
                    .sessions
                    .lock()
                    .expect("session map lock poisoned")
                    .get(&profile)
                    .cloned();
                if let Some(managed) = managed {
                    managed
                        .lock()
                        .expect("session lock poisoned")
                        .process
                        .take();
                }
                Err(UpstreamError::Process(error))
            }
        }
    }

    /// Forward a JSON-RPC notification through a profile's session.
    pub fn notify(
        &self,
        profile: impl Into<String>,
        credential: &CredentialRef,
        message: &str,
    ) -> Result<(), UpstreamError> {
        let profile = profile.into();
        let session = self.session_for(&profile, credential)?;
        let process = {
            let mut session = session.lock().expect("session lock poisoned");
            self.ensure_process(&mut session)?;
            session
                .process
                .as_ref()
                .expect("ensure_process installs a process")
                .clone()
        };
        let result = process.notify(message);
        if let Err(error) = result {
            let sessions = self.sessions.lock().expect("session map lock poisoned");
            if let Some(session) = sessions.get(&profile) {
                session
                    .lock()
                    .expect("session lock poisoned")
                    .process
                    .take();
            }
            return Err(UpstreamError::Process(error));
        }
        Ok(())
    }

    /// Shut down every child and remove all cached sessions.
    pub fn shutdown(&self) {
        let sessions =
            std::mem::take(&mut *self.sessions.lock().expect("session map lock poisoned"));
        for session in sessions.into_values() {
            if let Ok(mut session) = session.lock() {
                if let Some(process) = session.process.as_mut() {
                    process.shutdown();
                }
                session.process.take();
            }
        }
    }

    fn session_for(
        &self,
        profile: &str,
        credential: &CredentialRef,
    ) -> Result<Arc<Mutex<ManagedSession>>, UpstreamError> {
        let mut sessions = self.sessions.lock().expect("session map lock poisoned");
        if let Some(session) = sessions.get(profile) {
            if session.lock().expect("session lock poisoned").credential != *credential {
                return Err(UpstreamError::ProfileIdentityMismatch {
                    profile: profile.to_owned(),
                });
            }
            return Ok(Arc::clone(session));
        }
        let session = Arc::new(Mutex::new(ManagedSession {
            credential: credential.clone(),
            process: None,
            request_lock: Arc::new(Mutex::new(())),
        }));
        sessions.insert(profile.to_owned(), Arc::clone(&session));
        Ok(session)
    }

    fn ensure_process(&self, session: &mut ManagedSession) -> Result<bool, UpstreamError> {
        if session
            .process
            .as_ref()
            .is_some_and(|process| process.is_alive())
        {
            return Ok(false);
        }
        session.process.take();

        let secret = self
            .credential_provider
            .resolve(&session.credential)
            .map_err(UpstreamError::Credential)?;
        let request = UpstreamLaunchRequest {
            binary: self.config.binary.clone(),
            args: self.config.args.clone(),
            environment: UpstreamEnvironment::for_credential(&session.credential, secret),
        };
        session.process = Some(Arc::from(
            self.launcher
                .launch(request)
                .map_err(UpstreamError::Launch)?,
        ));
        Ok(true)
    }
}

impl<C, L> Drop for UpstreamSessionManager<C, L> {
    fn drop(&mut self) {
        let sessions = std::mem::take(self.sessions.get_mut().expect("session map lock poisoned"));
        for session in sessions.into_values() {
            if let Ok(mut session) = session.lock() {
                if let Some(process) = session.process.as_mut() {
                    process.shutdown();
                }
                session.process.take();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Mutex,
        },
        thread,
    };

    use super::*;

    #[derive(Clone)]
    struct FakeCredentials {
        values: Arc<HashMap<String, String>>,
    }

    impl FakeCredentials {
        fn new(values: [(&str, &str); 2]) -> Self {
            Self {
                values: Arc::new(
                    values
                        .into_iter()
                        .map(|(profile, token)| (profile.to_owned(), token.to_owned()))
                        .collect(),
                ),
            }
        }
    }

    impl CredentialProvider for FakeCredentials {
        fn resolve(&self, reference: &CredentialRef) -> Result<SecretString, CredentialError> {
            self.values
                .get(reference.name())
                .cloned()
                .map(SecretString::new)
                .ok_or_else(|| {
                    CredentialError::new(
                        reference.clone(),
                        crate::credentials::CredentialErrorKind::Unavailable,
                    )
                })
        }
    }

    struct FakeProcess {
        alive: AtomicBool,
        fail_next_send: Arc<AtomicBool>,
        shutdowns: Arc<AtomicUsize>,
    }

    impl UpstreamProcess for FakeProcess {
        fn send(&self, message: &str) -> Result<String, UpstreamProcessError> {
            if self.fail_next_send.swap(false, Ordering::SeqCst) {
                self.alive.store(false, Ordering::SeqCst);
                return Err(UpstreamProcessError::Io);
            }
            Ok(format!(r#"{{"jsonrpc":"2.0","result":"{message}"}}"#))
        }

        fn notify(&self, _message: &str) -> Result<(), UpstreamProcessError> {
            Ok(())
        }

        fn is_alive(&self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }

        fn shutdown(&self) {
            self.alive.store(false, Ordering::SeqCst);
            self.shutdowns.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Clone)]
    struct FakeLauncher {
        requests: Arc<Mutex<Vec<UpstreamLaunchRequest>>>,
        launches: Arc<AtomicUsize>,
        shutdowns: Arc<AtomicUsize>,
        fail_next_send: Arc<AtomicBool>,
    }

    impl FakeLauncher {
        fn new() -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                launches: Arc::new(AtomicUsize::new(0)),
                shutdowns: Arc::new(AtomicUsize::new(0)),
                fail_next_send: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl UpstreamLauncher for FakeLauncher {
        fn launch(
            &self,
            request: UpstreamLaunchRequest,
        ) -> Result<Box<dyn UpstreamProcess>, UpstreamLaunchError> {
            self.requests.lock().unwrap().push(request);
            self.launches.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeProcess {
                alive: AtomicBool::new(true),
                fail_next_send: Arc::clone(&self.fail_next_send),
                shutdowns: Arc::clone(&self.shutdowns),
            }))
        }
    }

    fn manager() -> (
        UpstreamSessionManager<FakeCredentials, FakeLauncher>,
        FakeLauncher,
    ) {
        let credentials = FakeCredentials::new([
            ("personal", "ghp_PERSONAL_SYNTHETIC"),
            ("work", "github_pat_WORK_SYNTHETIC"),
        ]);
        let launcher = FakeLauncher::new();
        let manager = UpstreamSessionManager::new(
            credentials,
            launcher.clone(),
            UpstreamConfig::default().with_binary("fake-github-mcp-server"),
        );
        (manager, launcher)
    }

    fn reference(profile: &str) -> CredentialRef {
        CredentialRef::new("gh", profile).with_host("github.com")
    }

    #[test]
    fn starts_two_profile_isolated_sessions_without_token_arguments() {
        let (manager, launcher) = manager();

        manager.start("personal", &reference("personal")).unwrap();
        manager.start("personal", &reference("personal")).unwrap();
        manager.start("work", &reference("work")).unwrap();

        assert_eq!(manager.session_count(), 2);
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 2);
        let requests = launcher.requests.lock().unwrap();
        let personal = requests[0]
            .environment()
            .get("GITHUB_PERSONAL_ACCESS_TOKEN")
            .unwrap();
        let work = requests[1]
            .environment()
            .get("GITHUB_PERSONAL_ACCESS_TOKEN")
            .unwrap();
        assert_eq!(personal.expose(), "ghp_PERSONAL_SYNTHETIC");
        assert_eq!(work.expose(), "github_pat_WORK_SYNTHETIC");
        assert_eq!(
            requests[0]
                .environment()
                .get("GITHUB_HOST")
                .unwrap()
                .expose(),
            "https://github.com"
        );
        assert!(!format!("{requests:?}").contains("ghp_PERSONAL_SYNTHETIC"));
        assert!(requests.iter().all(|request| request
            .args()
            .iter()
            .all(|arg| { !arg.contains("ghp_") && !arg.contains("github_pat_") })));
    }

    #[test]
    fn same_profile_concurrency_reuses_one_session() {
        let (manager, launcher) = manager();
        let manager = Arc::new(manager);
        let credential = reference("personal");
        let handles = (0..8)
            .map(|index| {
                let manager = Arc::clone(&manager);
                let credential = credential.clone();
                thread::spawn(move || {
                    manager.send("personal", &credential, &format!("message-{index}"))
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            assert!(handle.join().unwrap().is_ok());
        }
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
        assert!(manager.session_info()[0].active);
    }

    #[test]
    fn failed_process_is_restarted_without_retrying_the_failed_message() {
        let (manager, launcher) = manager();
        launcher.fail_next_send.store(true, Ordering::SeqCst);
        let credential = reference("personal");

        assert!(matches!(
            manager.send("personal", &credential, "write-once"),
            Err(UpstreamError::Process(UpstreamProcessError::Io))
        ));
        assert!(manager
            .send("personal", &credential, "retry-after-restart")
            .is_ok());
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn startup_hook_reinitializes_a_restarted_process_before_the_next_request() {
        let (manager, launcher) = manager();
        let credential = reference("personal");
        manager.send("personal", &credential, "initial").unwrap();
        launcher.fail_next_send.store(true, Ordering::SeqCst);
        assert!(manager.send("personal", &credential, "failed").is_err());

        let startup_runs = Arc::new(AtomicUsize::new(0));
        let startup_runs_for_hook = Arc::clone(&startup_runs);
        manager
            .send_with_startup("personal", &credential, "after-restart", move |process| {
                startup_runs_for_hook.fetch_add(1, Ordering::SeqCst);
                process.notify(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            })
            .unwrap();
        assert_eq!(startup_runs.load(Ordering::SeqCst), 1);
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn profile_cannot_be_rebound_to_a_different_credential_reference() {
        let (manager, _launcher) = manager();
        manager.start("personal", &reference("personal")).unwrap();

        let error = manager.start("personal", &reference("work")).unwrap_err();
        assert!(
            matches!(error, UpstreamError::ProfileIdentityMismatch { profile } if profile == "personal")
        );
    }

    #[test]
    fn shutdown_terminates_all_sessions_and_clears_cache() {
        let (manager, launcher) = manager();
        manager.start("personal", &reference("personal")).unwrap();
        manager.start("work", &reference("work")).unwrap();

        manager.shutdown();

        assert_eq!(manager.session_count(), 0);
        assert_eq!(launcher.shutdowns.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn launch_request_debug_redacts_all_environment_values() {
        let (manager, launcher) = manager();
        manager.start("personal", &reference("personal")).unwrap();
        let requests = launcher.requests.lock().unwrap();
        let debug = format!("{requests:?}");
        assert!(debug.contains("GITHUB_PERSONAL_ACCESS_TOKEN"));
        assert!(!debug.contains("SYNTHETIC"));
    }

    #[test]
    fn process_launcher_reports_missing_explicit_binary() {
        let launcher = ProcessUpstreamLauncher;
        let request = UpstreamLaunchRequest {
            binary: PathBuf::from("/definitely/missing/github-mcp-server"),
            args: vec!["stdio".to_owned()],
            environment: UpstreamEnvironment::for_credential(
                &reference("personal"),
                SecretString::new("ghp_SYNTHETIC"),
            ),
        };

        let error = match launcher.launch(request) {
            Ok(_) => panic!("missing binary unexpectedly launched"),
            Err(error) => error,
        };
        assert!(matches!(error, UpstreamLaunchError::BinaryNotFound { .. }));
        assert!(!error.to_string().contains("ghp_SYNTHETIC"));
    }

    #[test]
    fn response_demultiplexing_skips_notifications_until_the_expected_id() {
        let expected_id = serde_json::json!(7);
        assert!(!response_matches_id(
            r#"{"jsonrpc":"2.0","method":"notifications/progress","params":{}}"#,
            &expected_id
        )
        .unwrap());
        assert!(
            response_matches_id(r#"{"jsonrpc":"2.0","id":7,"result":{}}"#, &expected_id).unwrap()
        );
    }
}
