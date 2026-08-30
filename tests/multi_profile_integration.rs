use std::{
    collections::HashMap,
    fmt, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Barrier, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use gh_mcp_router::{
    config::{AmbiguityPolicyConfig, Config, ProfileConfig, RouteRule},
    context::{
        ContextSource, RepositoryContext, RepositoryContextRequest, RepositoryContextResolver,
    },
    credentials::{
        CommandOutput, CommandRequest, CommandRunner, CommandRunnerError, CredentialError,
        CredentialProvider, CredentialRef, GhCliCredentialProvider,
    },
    mcp::McpRouter,
    routing::{route_safely, OperationClass, RoutingResult, SafeRoutingError},
    security::{EventLogger, LogEvent, LogLevel, SecretString},
    upstream::{
        UpstreamConfig, UpstreamLaunchError, UpstreamLaunchRequest, UpstreamLauncher,
        UpstreamProcess, UpstreamProcessError,
    },
};
use serde_json::{json, Value};

const PERSONAL_SECRET: &str = "ghp_TEST_PERSONAL_PROFILE_A";
const WORK_SECRET: &str = "github_pat_TEST_WORK_PROFILE_B";
const OVERRIDE_SECRET: &str = "gho_TEST_OVERRIDE_PROFILE_C";

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "gh-mcp-router-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temporary directory should be creatable");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone)]
struct FakeGhRunner {
    calls: Arc<Mutex<Vec<CommandRequest>>>,
    active_account: Arc<Mutex<String>>,
}

impl FakeGhRunner {
    fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            active_account: Arc::new(Mutex::new("personal-user".to_owned())),
        }
    }
}

impl CommandRunner for FakeGhRunner {
    fn run(
        &self,
        request: CommandRequest,
    ) -> Result<gh_mcp_router::credentials::CommandOutput, CommandRunnerError> {
        self.calls.lock().unwrap().push(request.clone());
        let args = request.args();
        if args == ["--version"] {
            return Ok(gh_mcp_router::credentials::CommandOutput::success(
                "gh version 2.0.0",
                "",
            ));
        }

        let config_dir = request
            .gh_config_dir()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("personal");
        let (user, token) = if config_dir.contains("work") {
            ("work-user", WORK_SECRET)
        } else {
            ("personal-user", PERSONAL_SECRET)
        };
        if args.starts_with(&["auth".to_owned(), "status".to_owned()]) {
            return Ok(gh_mcp_router::credentials::CommandOutput::success(
                format!(
                    r#"{{"hosts":{{"github.com":[{{"login":"{user}","authenticated":true}}]}}}}"#
                ),
                "",
            ));
        }
        if args.starts_with(&["auth".to_owned(), "token".to_owned()]) {
            return Ok(gh_mcp_router::credentials::CommandOutput::success(
                token, "",
            ));
        }
        Err(CommandRunnerError::Io)
    }
}

#[derive(Clone)]
struct FakeCredentials {
    tokens: Arc<HashMap<String, &'static str>>,
}

impl FakeCredentials {
    fn new() -> Self {
        Self {
            tokens: Arc::new(
                [
                    ("personal-user".to_owned(), PERSONAL_SECRET),
                    ("work-user".to_owned(), WORK_SECRET),
                    ("override-user".to_owned(), OVERRIDE_SECRET),
                ]
                .into_iter()
                .collect(),
            ),
        }
    }
}

impl CredentialProvider for FakeCredentials {
    fn resolve(&self, reference: &CredentialRef) -> Result<SecretString, CredentialError> {
        self.tokens
            .get(reference.name())
            .map(|secret| SecretString::new(*secret))
            .ok_or_else(|| {
                CredentialError::new(
                    reference.clone(),
                    gh_mcp_router::credentials::CredentialErrorKind::Unavailable,
                )
            })
    }
}

#[derive(Clone, Default)]
struct FakeControl {
    launches: Arc<Mutex<Vec<String>>>,
    messages: Arc<Mutex<Vec<(String, Value)>>>,
    launch_debug: Arc<Mutex<Vec<String>>>,
    shutdowns: Arc<AtomicUsize>,
    slow_started: Arc<AtomicBool>,
    crash_once: Arc<AtomicBool>,
    mismatched_tools: Arc<AtomicBool>,
}

struct FakeLauncher {
    control: FakeControl,
}

struct FakeProcess {
    identity: String,
    secret: &'static str,
    control: FakeControl,
    alive: AtomicBool,
}

impl FakeLauncher {
    fn new(control: FakeControl) -> Self {
        Self { control }
    }
}

impl UpstreamLauncher for FakeLauncher {
    fn launch(
        &self,
        request: UpstreamLaunchRequest,
    ) -> Result<Box<dyn UpstreamProcess>, UpstreamLaunchError> {
        let token = request
            .environment()
            .get("GITHUB_PERSONAL_ACCESS_TOKEN")
            .expect("fake upstream must receive a token")
            .expose();
        let (identity, secret) = match token {
            PERSONAL_SECRET => ("personal", PERSONAL_SECRET),
            WORK_SECRET => ("work", WORK_SECRET),
            OVERRIDE_SECRET => ("override", OVERRIDE_SECRET),
            _ => return Err(UpstreamLaunchError::Io),
        };
        self.control
            .launches
            .lock()
            .unwrap()
            .push(identity.to_owned());
        self.control
            .launch_debug
            .lock()
            .unwrap()
            .push(format!("{request:?}"));
        Ok(Box::new(FakeProcess {
            identity: identity.to_owned(),
            secret,
            control: self.control.clone(),
            alive: AtomicBool::new(true),
        }))
    }
}

impl FakeProcess {
    fn record(&self, message: &str) -> Result<Value, UpstreamProcessError> {
        let value: Value =
            serde_json::from_str(message).map_err(|_| UpstreamProcessError::InvalidResponse)?;
        self.control
            .messages
            .lock()
            .unwrap()
            .push((self.identity.clone(), value.clone()));
        Ok(value)
    }
}

impl UpstreamProcess for FakeProcess {
    fn send(&self, message: &str) -> Result<String, UpstreamProcessError> {
        let value = self.record(message)?;
        let id = value.get("id").cloned().unwrap_or(Value::Null);
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let response = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "fake-github-mcp", "version": "test"}
                }
            }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": [{
                    "name": if self.control.mismatched_tools.load(Ordering::Acquire)
                        && self.identity == "work" { "different_tool" } else { "get_me" },
                    "description": "fake profile identity probe",
                    "inputSchema": {"type": "object"}
                }]}
            }),
            "tools/call" => {
                let tool = value["params"]["name"].as_str().unwrap_or_default();
                match tool {
                    "secret_error" => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32010, "message": format!("upstream rejected {}", self.secret)}
                    }),
                    "crash_once" if !self.control.crash_once.swap(true, Ordering::AcqRel) => {
                        return Err(UpstreamProcessError::Exited);
                    }
                    "slow_tool" => {
                        self.control.slow_started.store(true, Ordering::Release);
                        thread::sleep(Duration::from_millis(100));
                        json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": self.identity}]}})
                    }
                    _ => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"content": [{"type": "text", "text": self.identity}]}
                    }),
                }
            }
            "shutdown" => json!({"jsonrpc": "2.0", "id": id, "result": null}),
            _ => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        };
        Ok(response.to_string())
    }

    fn notify(&self, message: &str) -> Result<(), UpstreamProcessError> {
        self.record(message).map(|_| ())
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    fn shutdown(&self) {
        if self.alive.swap(false, Ordering::AcqRel) {
            self.control.shutdowns.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Default)]
struct TestLogger {
    events: Arc<Mutex<Vec<String>>>,
}

impl EventLogger for TestLogger {
    fn log(&self, _level: LogLevel, event: &LogEvent) {
        self.events.lock().unwrap().push(event.to_string());
    }
}

impl fmt::Debug for TestLogger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TestLogger(REDACTED_EVENTS)")
    }
}

fn profile(user: &str) -> ProfileConfig {
    ProfileConfig {
        provider: "gh".to_owned(),
        user: user.to_owned(),
        gh_config_dir: None,
        host: None,
    }
}

fn test_config() -> Config {
    let profiles = [
        ("personal".to_owned(), profile("personal-user")),
        ("work".to_owned(), profile("work-user")),
        ("override".to_owned(), profile("override-user")),
    ]
    .into_iter()
    .collect();
    let mut exact = RouteRule::for_profile("override");
    exact.repository = Some("WorkOrg/special".to_owned());
    let mut glob = RouteRule::for_profile("work");
    glob.repository = Some("WorkOrg/project-*".to_owned());
    let mut work_owner = RouteRule::for_profile("work");
    work_owner.owner = Some("WorkOrg".to_owned());
    let mut personal_owner = RouteRule::for_profile("personal");
    personal_owner.owner = Some("PersonalOrg".to_owned());
    let mut enterprise = RouteRule::for_profile("work");
    enterprise.host = Some("ghe.example".to_owned());
    Config {
        profiles,
        routes: vec![exact, glob, work_owner, personal_owner, enterprise],
        default_profile: Some("personal".to_owned()),
        ambiguity_policy: AmbiguityPolicyConfig::default(),
    }
}

fn reference(user: &str) -> CredentialRef {
    CredentialRef::new("gh", user).with_host("github.com")
}

fn initialize<C, L>(router: &McpRouter<C, L>)
where
    C: CredentialProvider + Send + Sync + 'static,
    L: UpstreamLauncher + 'static,
{
    let response = router
        .handle_message(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"roots":{"listChanged":true}}}}"#,
        )
        .expect("initialize must return a response");
    assert_eq!(serde_json::from_str::<Value>(&response).unwrap()["id"], 1);
    let roots = router
        .handle_message(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .expect("roots-capable clients receive a roots/list request");
    let roots: Value = serde_json::from_str(&roots).unwrap();
    assert_eq!(roots["method"], "roots/list");
}

fn call(
    router: &McpRouter<FakeCredentials, FakeLauncher>,
    id: usize,
    tool: &str,
    args: Value,
) -> Value {
    let message = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": tool, "arguments": args}
    });
    serde_json::from_str(
        &router
            .handle_message(&message.to_string())
            .expect("tool call must return a response"),
    )
    .unwrap()
}

#[test]
fn credential_matrix_uses_isolated_gh_accounts_without_switching() {
    let temp = TempDir::new("gh-config");
    let personal_dir = temp.path().join("personal");
    let work_dir = temp.path().join("work");
    fs::create_dir_all(&personal_dir).unwrap();
    fs::create_dir_all(&work_dir).unwrap();
    let runner = FakeGhRunner::new();
    let provider = GhCliCredentialProvider::with_runner(runner.clone());
    let personal = reference("personal-user").with_gh_config_dir(personal_dir.to_string_lossy());
    let work = reference("work-user").with_gh_config_dir(work_dir.to_string_lossy());

    let personal_secret = provider.resolve(&personal).unwrap();
    let work_secret = provider.resolve(&work).unwrap();
    assert!(personal_secret.expose() == PERSONAL_SECRET);
    assert!(work_secret.expose() == WORK_SECRET);
    assert_eq!(*runner.active_account.lock().unwrap(), "personal-user");
    let command_output = CommandOutput::success(PERSONAL_SECRET, WORK_SECRET);
    let debug = format!("{command_output:?}");
    assert!(!debug.contains(PERSONAL_SECRET));
    assert!(!debug.contains(WORK_SECRET));

    let calls = runner.calls.lock().unwrap();
    assert!(calls.iter().all(|request| {
        !request
            .args()
            .iter()
            .any(|arg| arg == PERSONAL_SECRET || arg == WORK_SECRET)
            && !request
                .args()
                .windows(2)
                .any(|window| window[0] == "auth" && window[1] == "switch")
    }));
    assert!(calls.iter().any(|request| request
        .args()
        .starts_with(&["auth".to_owned(), "status".to_owned(),])));
}

#[test]
fn routing_matrix_covers_precedence_and_fail_closed_writes() {
    let config = test_config();
    let exact = RepositoryContext::from_full_name("WorkOrg/special").unwrap();
    let glob = RepositoryContext::from_full_name("WorkOrg/project-one").unwrap();
    let owner = RepositoryContext::from_full_name("WorkOrg/other").unwrap();
    let host = RepositoryContext::from_repository_url("https://ghe.example/Other/repo").unwrap();
    let unmatched = RepositoryContext::from_full_name("Other/repo").unwrap();

    assert!(
        matches!(gh_mcp_router::routing::route(&config, &exact), RoutingResult::Selected(decision) if decision.selected_profile == "override")
    );
    assert!(
        matches!(gh_mcp_router::routing::route(&config, &glob), RoutingResult::Selected(decision) if decision.selected_profile == "work")
    );
    assert!(
        matches!(gh_mcp_router::routing::route(&config, &owner), RoutingResult::Selected(decision) if decision.selected_profile == "work")
    );
    assert!(
        matches!(gh_mcp_router::routing::route(&config, &host), RoutingResult::Selected(decision) if decision.selected_profile == "work")
    );
    assert!(
        matches!(gh_mcp_router::routing::route(&config, &unmatched), RoutingResult::Selected(decision) if decision.fallback_used)
    );
    assert!(matches!(
        route_safely(&config, Some(&unmatched), OperationClass::Write),
        Err(SafeRoutingError::FallbackNotAllowed { .. })
    ));

    let mut no_default = config.clone();
    no_default.default_profile = None;
    assert!(matches!(
        route_safely(&no_default, Some(&unmatched), OperationClass::Read),
        Err(SafeRoutingError::NoMatch { .. })
    ));

    let mut ambiguous = no_default.clone();
    let mut first = RouteRule::for_profile("personal");
    first.owner = Some("AmbiguousOrg".to_owned());
    let mut second = RouteRule::for_profile("work");
    second.owner = Some("AmbiguousOrg".to_owned());
    ambiguous.routes.extend([first, second]);
    let context = RepositoryContext::from_full_name("AmbiguousOrg/repo").unwrap();
    assert!(matches!(
        route_safely(&ambiguous, Some(&context), OperationClass::Write),
        Err(SafeRoutingError::Ambiguous { .. })
    ));
}

#[test]
fn context_matrix_resolves_explicit_inputs_and_temporary_git_remotes() {
    let git_repo = TempDir::new("git-context");
    let init = Command::new("git")
        .args(["-C", git_repo.path().to_str().unwrap(), "init", "-q"])
        .status()
        .expect("git should be installed for integration tests");
    assert!(init.success());
    let remote = Command::new("git")
        .args([
            "-C",
            git_repo.path().to_str().unwrap(),
            "remote",
            "add",
            "origin",
            "git@github.com:PersonalOrg/from-root.git",
        ])
        .status()
        .expect("git remote should be writable");
    assert!(remote.success());

    let resolver = RepositoryContextResolver::default();
    let explicit = resolver
        .resolve(&RepositoryContextRequest::owner_repo(
            "PersonalOrg",
            "explicit",
        ))
        .unwrap();
    assert_eq!(explicit.source, ContextSource::ToolArguments);
    let full_name = resolver
        .resolve(&RepositoryContextRequest::repository("WorkOrg/full-name"))
        .unwrap();
    assert_eq!(full_name.source, ContextSource::ToolArguments);
    let https = resolver
        .resolve(&RepositoryContextRequest::url(
            "https://github.com/WorkOrg/https.git",
        ))
        .unwrap();
    assert_eq!(https.source, ContextSource::RepositoryUrl);
    let ssh = resolver
        .resolve(&RepositoryContextRequest::url(
            "ssh://git@ghe.example/WorkOrg/ssh.git",
        ))
        .unwrap();
    assert_eq!(ssh.host, "ghe.example");
    assert_eq!(ssh.source, ContextSource::RepositoryUrl);

    let from_root = resolver
        .resolve(&RepositoryContextRequest::default().with_mcp_root(git_repo.path()))
        .unwrap();
    assert_eq!(from_root.full_name(), "PersonalOrg/from-root");
    assert_eq!(from_root.source, ContextSource::GitRemote);
    let explicit_wins = resolver
        .resolve(
            &RepositoryContextRequest::owner_repo("WorkOrg", "explicit")
                .with_mcp_root(git_repo.path()),
        )
        .unwrap();
    assert_eq!(explicit_wins.full_name(), "WorkOrg/explicit");
    assert_eq!(explicit_wins.source, ContextSource::ToolArguments);
}

#[test]
fn mcp_matrix_exercises_one_surface_profiles_concurrency_errors_and_shutdown() {
    let git_repo = TempDir::new("mcp-root");
    let init = Command::new("git")
        .args(["-C", git_repo.path().to_str().unwrap(), "init", "-q"])
        .status()
        .unwrap();
    assert!(init.success());
    let remote = Command::new("git")
        .args([
            "-C",
            git_repo.path().to_str().unwrap(),
            "remote",
            "add",
            "origin",
            "git@github.com:PersonalOrg/from-root.git",
        ])
        .status()
        .unwrap();
    assert!(remote.success());

    let control = FakeControl::default();
    let logger = TestLogger::default();
    let router = Arc::new(
        McpRouter::new(
            test_config(),
            FakeCredentials::new(),
            FakeLauncher::new(control.clone()),
            UpstreamConfig::default(),
        )
        .with_logger(Arc::new(logger.clone())),
    );
    initialize(&router);
    assert!(router
        .handle_message(
            &json!({
                "jsonrpc": "2.0",
                "id": "gh-mcp-router-roots",
                "result": {"roots": [{"uri": format!("file://{}", git_repo.path().display())}]}
            })
            .to_string()
        )
        .is_none());

    let tools = router
        .handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#)
        .unwrap();
    let tools: Value = serde_json::from_str(&tools).unwrap();
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(tools["result"]["tools"][0]["name"], "get_me");

    assert_eq!(
        call(
            &router,
            3,
            "get_me",
            json!({"owner": "PersonalOrg", "repo": "repo"})
        )["result"]["content"][0]["text"],
        "personal"
    );
    assert_eq!(
        call(
            &router,
            4,
            "get_me",
            json!({"repository": "WorkOrg/project-one"})
        )["result"]["content"][0]["text"],
        "work"
    );
    assert_eq!(
        call(
            &router,
            5,
            "get_me",
            json!({"repository_url": "https://github.com/WorkOrg/special.git"})
        )["result"]["content"][0]["text"],
        "override"
    );
    assert_eq!(
        call(&router, 6, "get_me", json!({}))["result"]["content"][0]["text"],
        "personal"
    );
    assert_eq!(
        call(
            &router,
            7,
            "get_me",
            json!({"repository_url": "https://ghe.example/Other/repo.git"})
        )["result"]["content"][0]["text"],
        "work"
    );

    let unmatched_write = call(
        &router,
        8,
        "create_issue",
        json!({"repository_url": "https://unknown.example/Other/repo.git"}),
    );
    assert!(unmatched_write["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Cannot safely"));

    let secret_error = call(
        &router,
        9,
        "secret_error",
        json!({"owner": "PersonalOrg", "repo": "repo"}),
    );
    let secret_error = secret_error.to_string();
    assert!(!secret_error.contains(PERSONAL_SECRET));
    assert!(secret_error.contains("[REDACTED]"));

    let crash = call(
        &router,
        10,
        "crash_once",
        json!({"owner": "PersonalOrg", "repo": "repo"}),
    );
    assert!(crash["error"].is_object());
    assert_eq!(
        call(
            &router,
            11,
            "get_me",
            json!({"owner": "PersonalOrg", "repo": "repo"})
        )["result"]["content"][0]["text"],
        "personal"
    );

    let barrier = Arc::new(Barrier::new(25));
    let workers = (0..24)
        .map(|index| {
            let router = Arc::clone(&router);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let (owner, expected) = if index % 2 == 0 {
                    ("PersonalOrg", "personal")
                } else {
                    ("WorkOrg", "work")
                };
                let response = call(
                    &router,
                    100 + index,
                    "get_me",
                    json!({"owner": owner, "repo": format!("repo-{index}")}),
                );
                assert_eq!(response["result"]["content"][0]["text"], expected);
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    for worker in workers {
        worker.join().unwrap();
    }

    let slow_router = Arc::clone(&router);
    let slow = thread::spawn(move || {
        slow_router
            .handle_message(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 2000,
                    "method": "tools/call",
                    "params": {"name": "slow_tool", "arguments": {"owner": "PersonalOrg", "repo": "repo"}}
                })
                .to_string(),
            )
            .unwrap()
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    while !control.slow_started.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::yield_now();
    }
    assert!(control.slow_started.load(Ordering::Acquire));
    assert!(router
        .handle_message(
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2000}}"#,
        )
        .is_none());
    let cancelled = slow.join().unwrap();
    assert!(cancelled.contains("cancelled"));

    let messages = control.messages.lock().unwrap();
    assert!(messages.iter().any(|(identity, message)| {
        identity == "personal" && message["method"] == "notifications/cancelled"
    }));
    drop(messages);

    let launches = control.launches.lock().unwrap().clone();
    assert!(launches.iter().any(|identity| identity == "personal"));
    assert!(launches.iter().any(|identity| identity == "work"));
    assert!(launches.iter().any(|identity| identity == "override"));
    assert!(
        launches
            .iter()
            .filter(|identity| *identity == "personal")
            .count()
            >= 2
    );

    let debug = control.launch_debug.lock().unwrap().join("\n");
    assert!(!debug.contains(PERSONAL_SECRET));
    assert!(!debug.contains(WORK_SECRET));
    assert!(!debug.contains(OVERRIDE_SECRET));
    let logs = logger.events.lock().unwrap().join("\n");
    assert!(!logs.contains(PERSONAL_SECRET));
    assert!(!logs.contains(WORK_SECRET));
    assert!(!logs.contains(OVERRIDE_SECRET));

    router.shutdown();
    assert!(control.shutdowns.load(Ordering::Relaxed) >= 3);
}

#[test]
fn mcp_matrix_rejects_incompatible_profile_tool_schemas() {
    let control = FakeControl::default();
    control.mismatched_tools.store(true, Ordering::Release);
    let router = McpRouter::new(
        test_config(),
        FakeCredentials::new(),
        FakeLauncher::new(control.clone()),
        UpstreamConfig::default(),
    );
    let response = router
        .handle_message(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        )
        .unwrap();
    assert!(response.contains("incompatible"));
    assert!(control
        .launch_debug
        .lock()
        .unwrap()
        .iter()
        .all(|debug| !debug.contains(PERSONAL_SECRET) && !debug.contains(WORK_SECRET)));
}
