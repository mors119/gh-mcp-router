//! Command-line workflow and presentation.
//!
//! Commands call the domain boundaries rather than reimplementing routing or
//! credential selection. The parser is dependency-free so the binary remains
//! easy to install and embed in local MCP configurations.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::Serialize;

use crate::{
    config::{AmbiguityPolicyConfig, Config, ConfigError, ProfileConfig, RouteRule},
    context::{ContextError, RepositoryContext},
    credentials::{
        CredentialError, CredentialErrorKind, CredentialProvider, CredentialRef, GhAccount,
        GhCliCredentialProvider,
    },
    mcp::{McpError, McpRouter},
    routing::{
        explain as explain_route, route, RouteSpecificity, RoutingExplanation, RoutingResult,
    },
    upstream::{
        ProcessUpstreamLauncher, UpstreamConfig, UpstreamError, UpstreamLaunchError,
        UpstreamLauncher, UpstreamSessionManager,
    },
};

/// Read-only account operations used by setup and diagnostics. Keeping this
/// separate from CredentialProvider lets CLI tests use fake account data.
pub trait CredentialInspector: CredentialProvider {
    fn verify_gh_installed(&self) -> Result<(), CredentialError>;
    fn discover(&self, reference: &CredentialRef) -> Result<Vec<GhAccount>, CredentialError>;
    fn verify_account(&self, reference: &CredentialRef) -> Result<GhAccount, CredentialError>;
}

impl<R: crate::credentials::CommandRunner> CredentialInspector for GhCliCredentialProvider<R> {
    fn verify_gh_installed(&self) -> Result<(), CredentialError> {
        GhCliCredentialProvider::verify_gh_installed(self)
    }

    fn discover(&self, reference: &CredentialRef) -> Result<Vec<GhAccount>, CredentialError> {
        GhCliCredentialProvider::discover(self, reference)
    }

    fn verify_account(&self, reference: &CredentialRef) -> Result<GhAccount, CredentialError> {
        GhCliCredentialProvider::verify_account(self, reference)
    }
}

/// Run the command-line entry point with production dependencies.
pub fn run() {
    if let Err(error) = try_run(env::args().skip(1)) {
        eprintln!("gh-mcp-router: {error}");
    }
}

/// Parse and run commands with the production GitHub CLI credential provider.
pub fn try_run<I, S>(args: I) -> Result<(), CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    try_run_with(
        args,
        GhCliCredentialProvider::new(),
        ProcessUpstreamLauncher,
    )
}

/// Injectable command runner used by unit tests and hosts embedding the CLI.
pub fn try_run_with<I, S, C, L>(args: I, credentials: C, launcher: L) -> Result<(), CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
    C: CredentialInspector + Send + Sync + 'static,
    L: UpstreamLauncher + Send + Sync + 'static,
{
    let mut stdout = io::stdout();
    try_run_with_writer(args, credentials, launcher, &mut stdout)
}

/// Variant of try_run_with that captures command output in a caller-owned
/// writer. It is useful for tests that assert JSON validity and secret safety.
pub fn try_run_with_writer<I, S, C, L, W>(
    args: I,
    credentials: C,
    launcher: L,
    output: &mut W,
) -> Result<(), CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
    C: CredentialInspector + Send + Sync + 'static,
    L: UpstreamLauncher + Send + Sync + 'static,
    W: Write,
{
    let parsed = CliArgs::parse(args)?;
    if parsed.help {
        write_text(output, usage())?;
        return Ok(());
    }

    let config_path = parsed.config.clone().unwrap_or_else(Config::default_path);
    match parsed.command.as_str() {
        "init" => init(&parsed, &config_path, &credentials, output),
        "profiles" => profiles(&parsed, &config_path, &credentials, output),
        "route" => route_command(&parsed, &config_path, output),
        "explain" => explain_command(&parsed, &config_path, output),
        "doctor" => doctor(&parsed, &config_path, credentials, launcher, output),
        "serve" => serve(&parsed, &config_path, credentials, launcher),
        "validate" => validate(&config_path, output),
        _ => Err(CliError::UnknownCommand(parsed.command)),
    }
}

fn validate<W: Write>(path: &Path, output: &mut W) -> Result<(), CliError> {
    let config = Config::load(path)?;
    write_text(
        output,
        &format!(
            "configuration is valid ({} profile(s), {} route(s))\n",
            config.profiles.len(),
            config.routes.len()
        ),
    )
}

fn init<C, W>(args: &CliArgs, path: &Path, credentials: &C, output: &mut W) -> Result<(), CliError>
where
    C: CredentialInspector,
    W: Write,
{
    if path.exists() && !args.force {
        return Err(CliError::ExistingConfig(path.to_owned()));
    }
    credentials.verify_gh_installed()?;
    let reference =
        CredentialRef::new("gh", "init").with_host(args.host.as_deref().unwrap_or("github.com"));
    let accounts = credentials
        .discover(&reference)?
        .into_iter()
        .filter(|account| account.authenticated)
        .collect::<Vec<_>>();
    if accounts.is_empty() {
        return Err(CliError::Credential(CredentialError::new(
            reference,
            CredentialErrorKind::AuthenticationMissing,
        )));
    }

    let mut assignments = BTreeMap::new();
    for (user, name) in &args.profile_assignments {
        assignments.insert(user.clone(), name.clone());
    }
    let mut used_names = BTreeSet::new();
    let mut profiles = BTreeMap::new();
    for account in accounts {
        let name = assignments
            .get(&account.user)
            .cloned()
            .unwrap_or_else(|| friendly_profile_name(&account.user));
        let name = unique_profile_name(name, &mut used_names);
        profiles.insert(
            name,
            ProfileConfig {
                provider: "gh".to_owned(),
                user: account.user,
                gh_config_dir: None,
                host: Some(account.host),
            },
        );
    }
    let default_profile = profiles.keys().next().cloned();
    let config = Config {
        profiles,
        routes: Vec::new(),
        default_profile,
        ambiguity_policy: AmbiguityPolicyConfig::default(),
    };
    config.validate()?;
    let contents = serialize_config(path, &config)?;
    write_config(path, &contents, args.force)?;

    if args.json {
        write_json(
            output,
            &InitReport {
                path: path.display().to_string(),
                profiles: config.profiles.keys().cloned().collect(),
                routes_suggested: false,
            },
        )
    } else {
        write_text(
            output,
            &format!(
                "created {} with {} profile(s); add routes to map repositories\n",
                path.display(),
                config.profiles.len()
            ),
        )
    }
}

fn profiles<C, W>(
    args: &CliArgs,
    path: &Path,
    credentials: &C,
    output: &mut W,
) -> Result<(), CliError>
where
    C: CredentialInspector,
    W: Write,
{
    let config = Config::load(path)?;
    let mut rows = Vec::new();
    for (name, profile) in &config.profiles {
        let reference = profile.credential_ref();
        let (user, auth) = match credentials.verify_account(&reference) {
            Ok(account) => (account.user, "ok".to_owned()),
            Err(error) => (profile.user.clone(), auth_status(&error).to_owned()),
        };
        rows.push(ProfileRow {
            profile: name.clone(),
            user,
            host: reference.host().unwrap_or("github.com").to_owned(),
            provider: reference.provider().to_owned(),
            auth,
        });
    }

    if args.json {
        write_json(output, &rows)
    } else {
        write_text(output, "PROFILE\tUSER\tHOST\tPROVIDER\tAUTH\n")?;
        for row in rows {
            write_text(
                output,
                &format!(
                    "{}\t{}\t{}\t{}\t{}\n",
                    row.profile, row.user, row.host, row.provider, row.auth
                ),
            )?;
        }
        Ok(())
    }
}

fn route_command<W: Write>(args: &CliArgs, path: &Path, output: &mut W) -> Result<(), CliError> {
    let config = Config::load(path)?;
    let context = parse_repository(args.repository.as_deref())?;
    let result = route(&config, &context);
    let report = route_report(&result);
    if args.json {
        write_json(output, &report)?;
    } else {
        write_text(output, &format_route(&report))?;
    }
    match result {
        RoutingResult::Selected(_) => Ok(()),
        RoutingResult::NoMatch(_) => Err(CliError::Routing(
            "no configured route matches this repository".to_owned(),
        )),
        RoutingResult::Ambiguous(_) => Err(CliError::Routing(
            "routing is ambiguous; see the route report and configure one profile".to_owned(),
        )),
    }
}

fn explain_command<W: Write>(args: &CliArgs, path: &Path, output: &mut W) -> Result<(), CliError> {
    let config = Config::load(path)?;
    let context = parse_repository(args.repository.as_deref())?;
    let explanation = explain_route(&config, &context);
    let report = explanation_report(&explanation);
    if args.json {
        write_json(output, &report)?;
    } else {
        write_text(output, &format_explanation(&report))?;
    }
    match explanation.result {
        RoutingResult::Selected(_) => Ok(()),
        RoutingResult::NoMatch(_) => Err(CliError::Routing(
            "no configured route matches this repository".to_owned(),
        )),
        RoutingResult::Ambiguous(_) => Err(CliError::Routing(
            "routing is ambiguous; configure one profile at the winning specificity".to_owned(),
        )),
    }
}

fn doctor<C, L, W>(
    args: &CliArgs,
    path: &Path,
    credentials: C,
    launcher: L,
    output: &mut W,
) -> Result<(), CliError>
where
    C: CredentialInspector + Send + Sync + 'static,
    L: UpstreamLauncher + Send + Sync + 'static,
    W: Write,
{
    let mut checks = Vec::new();
    let config = match Config::load(path) {
        Ok(config) => {
            checks.push(Check {
                name: "config".to_owned(),
                status: "ok".to_owned(),
                detail: path.display().to_string(),
            });
            Some(config)
        }
        Err(error) => {
            checks.push(Check {
                name: "config".to_owned(),
                status: "error".to_owned(),
                detail: error.to_string(),
            });
            None
        }
    };

    let gh_ok = credentials.verify_gh_installed().is_ok();
    checks.push(Check {
        name: "gh".to_owned(),
        status: if gh_ok { "ok" } else { "error" }.to_owned(),
        detail: if gh_ok {
            "available".to_owned()
        } else {
            "GitHub CLI is unavailable".to_owned()
        },
    });

    if let Some(config) = &config {
        let conflicts = route_conflicts(config);
        checks.push(Check {
            name: "routes".to_owned(),
            status: if conflicts.is_empty() { "ok" } else { "error" }.to_owned(),
            detail: if conflicts.is_empty() {
                "no obvious same-specificity conflicts".to_owned()
            } else {
                conflicts.join("; ")
            },
        });

        let mut startup_profiles = Vec::new();
        for (name, profile) in &config.profiles {
            let reference = profile.credential_ref();
            let config_dir_ok = match profile.expanded_gh_config_dir() {
                Ok(Some(dir)) => dir.is_dir(),
                Ok(None) => true,
                Err(_) => false,
            };
            if !config_dir_ok {
                checks.push(Check {
                    name: format!("profile:{name}:gh_config_dir"),
                    status: "error".to_owned(),
                    detail: "configured directory is unavailable".to_owned(),
                });
            }

            match credentials.verify_account(&reference) {
                Ok(account) => {
                    checks.push(Check {
                        name: format!("profile:{name}:account"),
                        status: "ok".to_owned(),
                        detail: format!("{}@{}", account.user, account.host),
                    });
                    if gh_ok && config_dir_ok {
                        match credentials.resolve(&reference) {
                            Ok(secret) => {
                                drop(secret);
                                startup_profiles.push(name.clone());
                                checks.push(Check {
                                    name: format!("profile:{name}:credential"),
                                    status: "ok".to_owned(),
                                    detail: "credential available".to_owned(),
                                });
                            }
                            Err(error) => checks.push(Check {
                                name: format!("profile:{name}:credential"),
                                status: "error".to_owned(),
                                detail: error.to_string(),
                            }),
                        }
                    }
                }
                Err(error) => checks.push(Check {
                    name: format!("profile:{name}:account"),
                    status: "error".to_owned(),
                    detail: error.to_string(),
                }),
            }
        }

        let upstream_config = upstream_config(args);
        let binary_ok = executable_available(upstream_config.binary());
        checks.push(Check {
            name: "github-mcp-server".to_owned(),
            status: if binary_ok { "ok" } else { "error" }.to_owned(),
            detail: if binary_ok {
                upstream_config.binary().display().to_string()
            } else {
                format!(
                    "executable '{}' was not found",
                    upstream_config.binary().display()
                )
            },
        });
        if binary_ok && gh_ok {
            let manager = UpstreamSessionManager::new(credentials, launcher, upstream_config);
            for name in startup_profiles {
                let reference = config.profiles[&name].credential_ref();
                match manager.start(name.clone(), &reference) {
                    Ok(_) => checks.push(Check {
                        name: format!("profile:{name}:upstream"),
                        status: "ok".to_owned(),
                        detail: "startup succeeded".to_owned(),
                    }),
                    Err(error) => checks.push(Check {
                        name: format!("profile:{name}:upstream"),
                        status: "error".to_owned(),
                        detail: upstream_error_detail(error),
                    }),
                }
            }
            manager.shutdown();
        }
    }

    let report = DoctorReport {
        healthy: checks.iter().all(|check| check.status == "ok"),
        checks,
    };
    if args.json {
        write_json(output, &report)?;
    } else {
        for check in &report.checks {
            let detail = if check.detail.is_empty() {
                String::new()
            } else {
                format!(" ({})", check.detail)
            };
            write_text(
                output,
                &format!("{}: {}{}\n", check.name, check.status, detail),
            )?;
        }
    }
    if report.healthy {
        Ok(())
    } else {
        Err(CliError::DoctorFailed)
    }
}

fn serve<C, L>(args: &CliArgs, path: &Path, credentials: C, launcher: L) -> Result<(), CliError>
where
    C: CredentialProvider + Send + Sync + 'static,
    L: UpstreamLauncher + Send + Sync + 'static,
{
    let config = Config::load(path)?;
    let router = McpRouter::new(config, credentials, launcher, upstream_config(args));
    Arc::new(router).serve_stdio().map_err(CliError::Mcp)
}

fn parse_repository(value: Option<&str>) -> Result<RepositoryContext, CliError> {
    let value = value.ok_or(CliError::MissingRepository)?;
    RepositoryContext::from_full_name(value).map_err(CliError::Context)
}

fn upstream_config(args: &CliArgs) -> UpstreamConfig {
    args.upstream_binary
        .as_ref()
        .map_or_else(UpstreamConfig::default, |binary| {
            UpstreamConfig::default().with_binary(binary)
        })
}

fn executable_available(binary: &Path) -> bool {
    if binary.components().count() > 1 {
        return binary.is_file();
    }
    env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join(binary))
        .any(|candidate| candidate.is_file())
}

fn serialize_config(path: &Path, config: &Config) -> Result<String, CliError> {
    if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
        serde_json::to_string_pretty(config)
            .map_err(|error| CliError::Serialization(error.to_string()))
    } else {
        serde_yaml::to_string(config).map_err(|error| CliError::Serialization(error.to_string()))
    }
}

fn write_config(path: &Path, contents: &str, force: bool) -> Result<(), CliError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| CliError::Io(error.to_string()))?;
    }
    if force {
        fs::write(path, contents).map_err(|error| CliError::Io(error.to_string()))
    } else {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    CliError::ExistingConfig(path.to_owned())
                } else {
                    CliError::Io(error.to_string())
                }
            })?;
        file.write_all(contents.as_bytes())
            .map_err(|error| CliError::Io(error.to_string()))
    }
}

fn friendly_profile_name(user: &str) -> String {
    let name = user
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if name.is_empty() {
        "profile".to_owned()
    } else {
        name
    }
}

fn unique_profile_name(mut name: String, used: &mut BTreeSet<String>) -> String {
    if used.insert(name.clone()) {
        return name;
    }
    let base = name.clone();
    let mut suffix = 2;
    loop {
        name = format!("{base}-{suffix}");
        if used.insert(name.clone()) {
            return name;
        }
        suffix += 1;
    }
}

fn route_conflicts(config: &Config) -> Vec<String> {
    let mut conflicts = Vec::new();
    for (left_index, left) in config.routes.iter().enumerate() {
        for (right_index, right) in config.routes.iter().enumerate().skip(left_index + 1) {
            if left.profile == right.profile || route_key(left) != route_key(right) {
                continue;
            }
            conflicts.push(format!(
                "routes[{left_index}] and routes[{right_index}] select different profiles"
            ));
        }
    }
    conflicts
}

fn route_key(rule: &RouteRule) -> Option<(RouteSpecificity, String)> {
    if let Some(repository) = &rule.repository {
        Some((
            if repository.contains('*') {
                RouteSpecificity::RepositoryGlob
            } else {
                RouteSpecificity::ExactRepository
            },
            format!(
                "{}|{}|{}",
                rule.host.as_deref().unwrap_or("").to_ascii_lowercase(),
                rule.owner.as_deref().unwrap_or("").to_ascii_lowercase(),
                repository.to_ascii_lowercase()
            ),
        ))
    } else if let Some(owner) = &rule.owner {
        Some((
            RouteSpecificity::Owner,
            format!(
                "{}|{}",
                rule.host.as_deref().unwrap_or("").to_ascii_lowercase(),
                owner.to_ascii_lowercase()
            ),
        ))
    } else {
        rule.host
            .as_ref()
            .map(|host| (RouteSpecificity::Host, host.to_ascii_lowercase()))
    }
}

fn auth_status(error: &CredentialError) -> &'static str {
    match error.kind() {
        CredentialErrorKind::AuthenticationMissing => "missing",
        CredentialErrorKind::GhNotInstalled => "gh-missing",
        _ => "error",
    }
}

fn upstream_error_detail(error: UpstreamError) -> String {
    match error {
        UpstreamError::Credential(error) => error.to_string(),
        UpstreamError::Launch(UpstreamLaunchError::BinaryNotFound { binary }) => {
            format!("executable '{}' was not found", binary.display())
        }
        UpstreamError::Launch(UpstreamLaunchError::Io) => "process could not be started".to_owned(),
        UpstreamError::Process(error) => error.to_string(),
        UpstreamError::ProfileIdentityMismatch { profile } => {
            format!("profile '{profile}' is bound to a different identity")
        }
    }
}

fn write_text<W: Write>(output: &mut W, text: &str) -> Result<(), CliError> {
    output
        .write_all(text.as_bytes())
        .map_err(|error| CliError::Io(error.to_string()))
}

fn write_json<W: Write, T: Serialize>(output: &mut W, value: &T) -> Result<(), CliError> {
    serde_json::to_writer(&mut *output, value)
        .map_err(|error| CliError::Serialization(error.to_string()))?;
    write_text(output, "\n")
}

#[derive(Debug, Serialize)]
struct InitReport {
    path: String,
    profiles: Vec<String>,
    routes_suggested: bool,
}

#[derive(Debug, Serialize)]
struct ProfileRow {
    profile: String,
    user: String,
    host: String,
    provider: String,
    auth: String,
}

#[derive(Debug, Serialize)]
struct RouteReport {
    repository: String,
    profile: Option<String>,
    rule: Option<String>,
    specificity: Option<String>,
    fallback: bool,
    result: String,
    ambiguity: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RuleReport {
    index: usize,
    rule: String,
    matched: bool,
    specificity: Option<String>,
}

#[derive(Debug, Serialize)]
struct ExplanationReport {
    context: ContextReport,
    precedence: Vec<&'static str>,
    rules: Vec<RuleReport>,
    result: RouteReport,
}

#[derive(Debug, Serialize)]
struct ContextReport {
    host: String,
    owner: String,
    repository: String,
    source: String,
}

#[derive(Debug, Serialize)]
struct Check {
    name: String,
    status: String,
    detail: String,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    healthy: bool,
    checks: Vec<Check>,
}

fn route_report(result: &RoutingResult) -> RouteReport {
    match result {
        RoutingResult::Selected(decision) => RouteReport {
            repository: decision.repository.clone(),
            profile: Some(decision.selected_profile.clone()),
            rule: decision.matched_rule.clone(),
            specificity: Some(decision.specificity.to_string()),
            fallback: decision.fallback_used,
            result: "selected".to_owned(),
            ambiguity: Vec::new(),
        },
        RoutingResult::NoMatch(no_match) => RouteReport {
            repository: no_match.repository.clone(),
            profile: None,
            rule: None,
            specificity: None,
            fallback: false,
            result: "no_match".to_owned(),
            ambiguity: Vec::new(),
        },
        RoutingResult::Ambiguous(ambiguous) => RouteReport {
            repository: ambiguous.repository.clone(),
            profile: None,
            rule: None,
            specificity: Some(ambiguous.specificity.to_string()),
            fallback: false,
            result: "ambiguous".to_owned(),
            ambiguity: ambiguous
                .candidates
                .iter()
                .map(|candidate| format!("{} ({})", candidate.profile, candidate.matched_rule))
                .collect(),
        },
    }
}

fn explanation_report(explanation: &RoutingExplanation) -> ExplanationReport {
    ExplanationReport {
        context: ContextReport {
            host: explanation.context.host.clone(),
            owner: explanation.context.owner.clone(),
            repository: explanation.context.repository.clone(),
            source: explanation.context.source.to_string(),
        },
        precedence: vec![
            "exact_repository",
            "repository_glob",
            "owner",
            "host",
            "default",
        ],
        rules: explanation
            .rules
            .iter()
            .map(|rule| RuleReport {
                index: rule.index,
                rule: rule.rule.clone(),
                matched: rule.matched,
                specificity: rule.specificity.map(|specificity| specificity.to_string()),
            })
            .collect(),
        result: route_report(&explanation.result),
    }
}

fn format_route(report: &RouteReport) -> String {
    let mut text = format!("Repository: {}\n", report.repository);
    match report.profile.as_deref() {
        Some(profile) => text.push_str(&format!("Profile:    {profile}\n")),
        None => text.push_str("Profile:    <rejected>\n"),
    }
    text.push_str(&format!(
        "Rule:       {}\nFallback:   {}\nResult:     {}\n",
        report.rule.as_deref().unwrap_or("none"),
        if report.fallback { "yes" } else { "no" },
        report.result
    ));
    if !report.ambiguity.is_empty() {
        text.push_str("Candidates: ");
        text.push_str(&report.ambiguity.join(", "));
        text.push('\n');
    }
    text
}

fn format_explanation(report: &ExplanationReport) -> String {
    let mut text = format!(
        "Context:    {}/{} (host {}, source {})\n",
        report.context.owner, report.context.repository, report.context.host, report.context.source
    );
    text.push_str(&format!("Precedence: {}\n", report.precedence.join(" > ")));
    text.push_str("Rules:\n");
    for rule in &report.rules {
        text.push_str(&format!(
            "  [{}] {} — {}{}\n",
            rule.index,
            rule.rule,
            if rule.matched {
                "matched"
            } else {
                "not matched"
            },
            rule.specificity
                .as_deref()
                .map(|specificity| format!(" ({specificity})"))
                .unwrap_or_default()
        ));
    }
    text.push_str("Final:\n");
    text.push_str(&format_route(&report.result));
    text
}

fn usage() -> &'static str {
    "gh-mcp-router — route GitHub MCP requests by profile\n\nUsage:\n  gh-mcp-router init [--config PATH] [--profile USER=NAME] [--force]\n  gh-mcp-router profiles [--config PATH] [--json]\n  gh-mcp-router route OWNER/REPO [--config PATH] [--json]\n  gh-mcp-router explain OWNER/REPO [--config PATH] [--json]\n  gh-mcp-router doctor [--config PATH] [--json] [--upstream-binary PATH]\n  gh-mcp-router serve [--config PATH] [--upstream-binary PATH]\n\ninit creates identity-only configuration; it never stores credentials.\n"
}

#[derive(Debug)]
struct CliArgs {
    command: String,
    config: Option<PathBuf>,
    repository: Option<String>,
    json: bool,
    force: bool,
    help: bool,
    host: Option<String>,
    upstream_binary: Option<PathBuf>,
    profile_assignments: Vec<(String, String)>,
}

impl CliArgs {
    fn parse<I, S>(args: I) -> Result<Self, CliError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut command = None;
        let mut config = None;
        let mut repository = None;
        let mut json = false;
        let mut force = false;
        let mut help = false;
        let mut host = None;
        let mut upstream_binary = None;
        let mut profile_assignments = Vec::new();
        let mut arguments = args.into_iter().map(Into::into).peekable();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--help" | "-h" => help = true,
                "--json" => json = true,
                "--force" => force = true,
                "--config" => config = Some(PathBuf::from(next_value(&mut arguments, "--config")?)),
                value if value.starts_with("--config=") => {
                    let value = &value["--config=".len()..];
                    if value.is_empty() {
                        return Err(CliError::MissingArgument("--config".to_owned()));
                    }
                    config = Some(PathBuf::from(value));
                }
                "--profile" => profile_assignments
                    .push(parse_assignment(next_value(&mut arguments, "--profile")?)?),
                value if value.starts_with("--profile=") => profile_assignments
                    .push(parse_assignment(value["--profile=".len()..].to_owned())?),
                "--host" => host = Some(next_value(&mut arguments, "--host")?),
                value if value.starts_with("--host=") => {
                    host = Some(value["--host=".len()..].to_owned())
                }
                "--upstream-binary" => {
                    upstream_binary = Some(PathBuf::from(next_value(
                        &mut arguments,
                        "--upstream-binary",
                    )?))
                }
                value if value.starts_with("--upstream-binary=") => {
                    upstream_binary = Some(PathBuf::from(
                        value["--upstream-binary=".len()..].to_owned(),
                    ))
                }
                value if value.starts_with('-') => {
                    return Err(CliError::UnknownArgument(value.to_owned()))
                }
                value if command.is_none() => command = Some(value.to_owned()),
                value if repository.is_none() => repository = Some(value.to_owned()),
                value => return Err(CliError::UnknownArgument(value.to_owned())),
            }
        }
        let command = command.unwrap_or_else(|| "help".to_owned());
        if command == "help" || help {
            return Ok(Self {
                command,
                config,
                repository,
                json,
                force,
                help: true,
                host,
                upstream_binary,
                profile_assignments,
            });
        }
        if repository.is_some() && !matches!(command.as_str(), "route" | "explain") {
            return Err(CliError::UnknownArgument(repository.unwrap_or_default()));
        }
        if force && command != "init" {
            return Err(CliError::UnknownArgument("--force".to_owned()));
        }
        if !profile_assignments.is_empty() && command != "init" {
            return Err(CliError::UnknownArgument("--profile".to_owned()));
        }
        Ok(Self {
            command,
            config,
            repository,
            json,
            force,
            help,
            host,
            upstream_binary,
            profile_assignments,
        })
    }
}

fn next_value<I>(arguments: &mut std::iter::Peekable<I>, flag: &str) -> Result<String, CliError>
where
    I: Iterator<Item = String>,
{
    arguments
        .next()
        .ok_or_else(|| CliError::MissingArgument(flag.to_owned()))
}

fn parse_assignment(value: String) -> Result<(String, String), CliError> {
    let Some((user, name)) = value.split_once('=') else {
        return Err(CliError::InvalidAssignment(value));
    };
    if user.is_empty() || name.is_empty() {
        return Err(CliError::InvalidAssignment(value));
    }
    Ok((user.to_owned(), name.to_owned()))
}

#[derive(Debug)]
pub enum CliError {
    MissingArgument(String),
    MissingRepository,
    InvalidAssignment(String),
    UnknownArgument(String),
    UnknownCommand(String),
    ExistingConfig(PathBuf),
    Config(ConfigError),
    Context(ContextError),
    Credential(CredentialError),
    Routing(String),
    DoctorFailed,
    Mcp(McpError),
    Serialization(String),
    Io(String),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::MissingArgument(_)
            | Self::MissingRepository
            | Self::InvalidAssignment(_)
            | Self::UnknownArgument(_)
            | Self::UnknownCommand(_) => 64,
            Self::Config(_) | Self::ExistingConfig(_) | Self::Serialization(_) | Self::Io(_) => 2,
            Self::Credential(_) => 3,
            Self::Routing(_) | Self::Context(_) => 4,
            Self::Mcp(_) => 5,
            Self::DoctorFailed => 6,
        }
    }
}

impl From<ConfigError> for CliError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<ContextError> for CliError {
    fn from(error: ContextError) -> Self {
        Self::Context(error)
    }
}

impl From<CredentialError> for CliError {
    fn from(error: CredentialError) -> Self {
        Self::Credential(error)
    }
}

impl From<McpError> for CliError {
    fn from(error: McpError) -> Self {
        Self::Mcp(error)
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingArgument(flag) => write!(formatter, "{flag} requires a value"),
            Self::MissingRepository => formatter.write_str("route and explain require OWNER/REPO"),
            Self::InvalidAssignment(value) => {
                write!(formatter, "invalid --profile '{value}'; use USER=NAME")
            }
            Self::UnknownArgument(argument) => write!(formatter, "unknown argument '{argument}'"),
            Self::UnknownCommand(command) => write!(formatter, "unknown command '{command}'"),
            Self::ExistingConfig(path) => write!(
                formatter,
                "config '{}' already exists; use --force to replace it",
                path.display()
            ),
            Self::Config(error) => error.fmt(formatter),
            Self::Context(error) => error.fmt(formatter),
            Self::Credential(error) => error.fmt(formatter),
            Self::Routing(message) => formatter.write_str(message),
            Self::DoctorFailed => formatter.write_str("doctor found one or more setup problems"),
            Self::Mcp(error) => error.fmt(formatter),
            Self::Serialization(message) => {
                write!(formatter, "cannot serialize output: {message}")
            }
            Self::Io(message) => write!(formatter, "I/O failed: {message}"),
        }
    }
}

impl std::error::Error for CliError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecretString;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };

    #[derive(Clone)]
    struct FakeCredentials {
        accounts: Vec<GhAccount>,
        missing_gh: bool,
        secret: String,
    }

    impl CredentialProvider for FakeCredentials {
        fn resolve(&self, reference: &CredentialRef) -> Result<SecretString, CredentialError> {
            self.accounts
                .iter()
                .find(|account| account.user == reference.name())
                .map(|_| SecretString::new(self.secret.clone()))
                .ok_or_else(|| {
                    CredentialError::new(
                        reference.clone(),
                        CredentialErrorKind::AuthenticationMissing,
                    )
                })
        }
    }

    impl CredentialInspector for FakeCredentials {
        fn verify_gh_installed(&self) -> Result<(), CredentialError> {
            if self.missing_gh {
                Err(CredentialError::new(
                    CredentialRef::new("gh", "cli"),
                    CredentialErrorKind::GhNotInstalled,
                ))
            } else {
                Ok(())
            }
        }

        fn discover(&self, _reference: &CredentialRef) -> Result<Vec<GhAccount>, CredentialError> {
            Ok(self.accounts.clone())
        }

        fn verify_account(&self, reference: &CredentialRef) -> Result<GhAccount, CredentialError> {
            self.accounts
                .iter()
                .find(|account| account.user == reference.name())
                .cloned()
                .ok_or_else(|| {
                    CredentialError::new(
                        reference.clone(),
                        CredentialErrorKind::AuthenticationMissing,
                    )
                })
        }
    }

    #[derive(Clone, Default)]
    struct FakeLauncher(Arc<Mutex<Vec<String>>>);

    impl UpstreamLauncher for FakeLauncher {
        fn launch(
            &self,
            request: crate::upstream::UpstreamLaunchRequest,
        ) -> Result<Box<dyn crate::upstream::UpstreamProcess>, UpstreamLaunchError> {
            self.0
                .lock()
                .unwrap()
                .push(request.binary().display().to_string());
            Err(UpstreamLaunchError::BinaryNotFound {
                binary: request.binary().to_owned(),
            })
        }
    }

    fn credentials() -> FakeCredentials {
        FakeCredentials {
            accounts: vec![GhAccount {
                host: "github.com".to_owned(),
                user: "personal".to_owned(),
                authenticated: true,
                source: crate::credentials::CredentialSource::Gh,
            }],
            missing_gh: false,
            secret: "ghp_SYNTHETIC_TOKEN_MUST_NOT_LEAK".to_owned(),
        }
    }

    fn two_credentials() -> FakeCredentials {
        let mut credentials = credentials();
        credentials.accounts.push(GhAccount {
            host: "github.com".to_owned(),
            user: "work".to_owned(),
            authenticated: true,
            source: crate::credentials::CredentialSource::Gh,
        });
        credentials
    }

    fn config(path: &Path) {
        fs::write(
            path,
            "profiles:\n  personal: { provider: gh, user: personal }\nroutes:\n  - match: { owner: ExampleOrg }\n    profile: personal\n",
        )
        .unwrap();
    }

    #[test]
    fn parser_accepts_global_options_before_and_after_command() {
        let args = CliArgs::parse([
            "--json",
            "route",
            "ExampleOrg/repo",
            "--config",
            "config.yaml",
        ])
        .unwrap();
        assert!(args.json);
        assert_eq!(args.command, "route");
        assert_eq!(args.repository.as_deref(), Some("ExampleOrg/repo"));
    }

    #[test]
    fn first_init_writes_identity_only_config_and_protects_existing_config() {
        let directory = tempfile_dir();
        let path = directory.join("config.yaml");
        let mut output = Vec::new();
        try_run_with_writer(
            [
                "init",
                "--config",
                path.to_str().unwrap(),
                "--profile",
                "personal=home",
            ],
            credentials(),
            FakeLauncher::default(),
            &mut output,
        )
        .unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("home:"));
        assert!(!content.contains("SYNTHETIC_TOKEN"));
        let error = try_run_with_writer(
            ["init", "--config", path.to_str().unwrap()],
            credentials(),
            FakeLauncher::default(),
            &mut output,
        )
        .unwrap_err();
        assert!(matches!(error, CliError::ExistingConfig(_)));
    }

    #[test]
    fn json_route_output_is_valid_and_does_not_need_credentials() {
        let directory = tempfile_dir();
        let path = directory.join("config.yaml");
        config(&path);
        let mut output = Vec::new();
        try_run_with_writer(
            [
                "route",
                "ExampleOrg/repo",
                "--config",
                path.to_str().unwrap(),
                "--json",
            ],
            credentials(),
            FakeLauncher::default(),
            &mut output,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["profile"], "personal");
        assert!(!String::from_utf8_lossy(&output).contains("SYNTHETIC_TOKEN"));
    }

    #[test]
    fn profiles_lists_two_profiles_with_non_secret_auth_status() {
        let directory = tempfile_dir();
        let path = directory.join("config.yaml");
        fs::write(
            &path,
            "profiles:\n  personal: { provider: gh, user: personal }\n  work: { provider: gh, user: work }\n",
        )
        .unwrap();
        let mut output = Vec::new();
        try_run_with_writer(
            ["profiles", "--config", path.to_str().unwrap(), "--json"],
            two_credentials(),
            FakeLauncher::default(),
            &mut output,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 2);
        assert!(String::from_utf8_lossy(&output).contains("\"auth\":\"ok\""));
        assert!(!String::from_utf8_lossy(&output).contains("SYNTHETIC_TOKEN"));
    }

    #[test]
    fn explain_reports_ambiguous_routes_without_secrets() {
        let directory = tempfile_dir();
        let path = directory.join("config.yaml");
        fs::write(
            path.as_path(),
            "profiles:\n  a: { provider: gh, user: a }\n  b: { provider: gh, user: b }\nroutes:\n  - match: { owner: ExampleOrg }\n    profile: a\n  - match: { owner: ExampleOrg }\n    profile: b\n",
        )
        .unwrap();
        let mut output = Vec::new();
        let error = try_run_with_writer(
            [
                "explain",
                "ExampleOrg/repo",
                "--config",
                path.to_str().unwrap(),
                "--json",
            ],
            credentials(),
            FakeLauncher::default(),
            &mut output,
        )
        .unwrap_err();
        assert!(matches!(error, CliError::Routing(_)));
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["result"]["result"], "ambiguous");
    }

    #[test]
    fn doctor_reports_missing_gh_in_machine_readable_output() {
        let directory = tempfile_dir();
        let path = directory.join("config.yaml");
        config(&path);
        let mut credentials = credentials();
        credentials.missing_gh = true;
        let mut output = Vec::new();
        let error = try_run_with_writer(
            ["doctor", "--config", path.to_str().unwrap(), "--json"],
            credentials,
            FakeLauncher::default(),
            &mut output,
        )
        .unwrap_err();
        assert!(matches!(error, CliError::DoctorFailed));
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["healthy"], false);
        assert!(!String::from_utf8_lossy(&output).contains("SYNTHETIC_TOKEN"));
    }

    #[test]
    fn doctor_reports_a_missing_configured_account() {
        let directory = tempfile_dir();
        let path = directory.join("config.yaml");
        fs::write(
            &path,
            "profiles:\n  personal: { provider: gh, user: personal }\n  work: { provider: gh, user: work }\n",
        )
        .unwrap();
        let mut output = Vec::new();
        let error = try_run_with_writer(
            ["doctor", "--config", path.to_str().unwrap(), "--json"],
            credentials(),
            FakeLauncher::default(),
            &mut output,
        )
        .unwrap_err();
        assert!(matches!(error, CliError::DoctorFailed));
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert!(value["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["name"] == "profile:work:account" && check["status"] == "error"));
    }

    fn tempfile_dir() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = env::temp_dir().join(format!(
            "gh-mcp-router-cli-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
