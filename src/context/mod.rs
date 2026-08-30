//! Repository-context discovery and normalization.
//!
//! Context discovery is deliberately independent from credentials and routing.
//! It only answers which GitHub repository a request targets. The resolver
//! uses request-scoped inputs first and inspects Git read-only when an explicit
//! MCP/workspace root is supplied.

use std::{
    fmt,
    path::{Path, PathBuf},
    process::Command,
};

/// Where normalized repository context came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextSource {
    /// An owner/repository pair or full repository name supplied by a tool.
    ToolArguments,
    /// A GitHub repository URL supplied by a tool.
    RepositoryUrl,
    /// A repository discovered from an MCP root or workspace path.
    McpRoot,
    /// A repository discovered from the Git remote at an MCP root.
    GitRemote,
    /// An explicitly configured session/default context.
    Default,
}

impl ContextSource {
    // Keep the foundation names source-compatible for callers from #2/#3.
    #[allow(non_upper_case_globals)]
    pub const Explicit: Self = Self::ToolArguments;
    #[allow(non_upper_case_globals)]
    pub const McpWorkspace: Self = Self::McpRoot;
    #[allow(non_upper_case_globals)]
    pub const LocalGit: Self = Self::GitRemote;
    #[allow(non_upper_case_globals)]
    pub const ConfiguredFallback: Self = Self::Default;
}

impl fmt::Display for ContextSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ToolArguments => "tool-arguments",
            Self::RepositoryUrl => "repository-url",
            Self::McpRoot => "mcp-root",
            Self::GitRemote => "git-remote",
            Self::Default => "default",
        })
    }
}

/// Repository identity consumed by routing decisions.
///
/// The constructor trims values and normalizes the host. Authentication state
/// is intentionally absent, so this type can safely cross domain boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryContext {
    pub host: String,
    pub owner: String,
    pub repository: String,
    pub source: ContextSource,
}

impl RepositoryContext {
    pub fn new(
        host: impl Into<String>,
        owner: impl Into<String>,
        repository: impl Into<String>,
        source: ContextSource,
    ) -> Self {
        Self {
            host: host.into().trim().to_ascii_lowercase(),
            owner: owner.into().trim().to_owned(),
            repository: repository.into().trim().to_owned(),
            source,
        }
    }

    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repository)
    }

    pub fn from_owner_repo(owner: &str, repository: &str) -> Result<Self, ContextError> {
        context_from_owner_repo(owner, repository, ContextSource::ToolArguments)
    }

    pub fn from_full_name(value: &str) -> Result<Self, ContextError> {
        parse_full_name(value, ContextSource::ToolArguments)
    }

    pub fn from_repository_url(value: &str) -> Result<Self, ContextError> {
        parse_repository_reference(value, ContextSource::RepositoryUrl)
    }

    pub fn from_git_remote(value: &str) -> Result<Self, ContextError> {
        parse_repository_reference(value, ContextSource::GitRemote)
    }
}

/// Request-scoped inputs considered by [`RepositoryContextResolver`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepositoryContextRequest {
    /// Explicit owner and repository tool arguments. `repo` may also contain
    /// a full `owner/repository` name when `owner` is omitted.
    pub owner: Option<String>,
    pub repo: Option<String>,
    /// A full `owner/repository` tool argument.
    pub repository: Option<String>,
    /// A GitHub repository URL tool argument.
    pub repository_url: Option<String>,
    /// The request-scoped MCP root. This is preferred over workspace_root.
    pub mcp_root: Option<PathBuf>,
    /// A workspace root supplied by the client or host.
    pub workspace_root: Option<PathBuf>,
    /// An explicitly configured context used as the final fallback.
    pub configured_context: Option<RepositoryContext>,
}

impl RepositoryContextRequest {
    pub fn owner_repo(owner: impl Into<String>, repo: impl Into<String>) -> Self {
        Self {
            owner: Some(owner.into()),
            repo: Some(repo.into()),
            ..Self::default()
        }
    }

    pub fn repository(value: impl Into<String>) -> Self {
        Self {
            repository: Some(value.into()),
            ..Self::default()
        }
    }

    pub fn url(value: impl Into<String>) -> Self {
        Self {
            repository_url: Some(value.into()),
            ..Self::default()
        }
    }

    pub fn with_mcp_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.mcp_root = Some(root.into());
        self
    }

    pub fn with_workspace_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.workspace_root = Some(root.into());
        self
    }

    pub fn with_configured_context(mut self, context: RepositoryContext) -> Self {
        self.configured_context = Some(context);
        self
    }
}

/// Errors produced while resolving request context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextError {
    MissingRepositoryContext,
    IncompleteExplicitArguments,
    InvalidRepository {
        source: ContextSource,
        message: String,
    },
    GitInspection {
        root: PathBuf,
        message: String,
    },
}

impl fmt::Display for ContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRepositoryContext => formatter.write_str(
                "repository context is missing; provide owner/repository or configure a matching route",
            ),
            Self::IncompleteExplicitArguments => formatter.write_str(
                "owner and repo must be provided together, or repo must be a full owner/repository name",
            ),
            Self::InvalidRepository { source, message } => {
                write!(formatter, "invalid repository context from {source}: {message}")
            }
            Self::GitInspection { root, message } => {
                write!(formatter, "cannot inspect Git repository '{}': {message}", root.display())
            }
        }
    }
}

impl std::error::Error for ContextError {}

/// Read-only provider for the origin URL of a workspace.
pub trait GitRemoteProvider {
    fn origin_url(&self, root: &Path) -> Result<Option<String>, ContextError>;
}

/// Git implementation used by the application.
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandGitRemoteProvider;

impl GitRemoteProvider for CommandGitRemoteProvider {
    fn origin_url(&self, root: &Path) -> Result<Option<String>, ContextError> {
        let root_arg = root.to_string_lossy().into_owned();
        let output = Command::new("git")
            .args(["-C", root_arg.as_str(), "remote", "get-url", "origin"])
            .output()
            .map_err(|error| ContextError::GitInspection {
                root: root.to_owned(),
                message: error.to_string(),
            })?;

        if !output.status.success() {
            // A workspace without an origin is a valid discovery miss. Other
            // Git failures remain visible so malformed/unsupported state cannot
            // silently become an identity guess.
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("No such remote")
                || stderr.contains("not a git repository")
                || stderr.contains("No such file or directory")
            {
                return Ok(None);
            }
            return Err(ContextError::GitInspection {
                root: root.to_owned(),
                message: "git could not read the origin remote".to_owned(),
            });
        }

        let url = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Ok((!url.is_empty()).then_some(url))
    }
}

/// Resolves a repository using explicit request data before ambient context.
pub struct RepositoryContextResolver<P = CommandGitRemoteProvider> {
    git: P,
}

impl Default for RepositoryContextResolver<CommandGitRemoteProvider> {
    fn default() -> Self {
        Self::new(CommandGitRemoteProvider)
    }
}

impl<P: GitRemoteProvider> RepositoryContextResolver<P> {
    pub fn new(git: P) -> Self {
        Self { git }
    }

    /// Apply the Issue #6 precedence order:
    /// owner+repo, full name, URL, MCP/workspace root, Git remote, default.
    pub fn resolve(
        &self,
        request: &RepositoryContextRequest,
    ) -> Result<RepositoryContext, ContextError> {
        if request.owner.is_some() || request.repo.is_some() {
            return match (request.owner.as_deref(), request.repo.as_deref()) {
                (Some(owner), Some(repo)) => {
                    context_from_owner_repo(owner, repo, ContextSource::ToolArguments)
                }
                (None, Some(repo)) if repo.contains('/') => {
                    parse_full_name(repo, ContextSource::ToolArguments)
                }
                _ => Err(ContextError::IncompleteExplicitArguments),
            };
        }

        if let Some(repository) = request.repository.as_deref() {
            return parse_full_name(repository, ContextSource::ToolArguments);
        }

        if let Some(url) = request.repository_url.as_deref() {
            return parse_repository_reference(url, ContextSource::RepositoryUrl);
        }

        if let Some(root) = request
            .mcp_root
            .as_deref()
            .or(request.workspace_root.as_deref())
        {
            if let Some(remote) = self.git.origin_url(root)? {
                return parse_repository_reference(&remote, ContextSource::GitRemote);
            }
        }

        if let Some(context) = &request.configured_context {
            return Ok(RepositoryContext::new(
                &context.host,
                &context.owner,
                &context.repository,
                ContextSource::Default,
            ));
        }

        Err(ContextError::MissingRepositoryContext)
    }
}

fn context_from_owner_repo(
    owner: &str,
    repo: &str,
    source: ContextSource,
) -> Result<RepositoryContext, ContextError> {
    if repo.contains('/') {
        return Err(invalid(
            source,
            "repo must be a repository name when owner is supplied",
        ));
    }
    context_from_parts("github.com", owner, repo, source)
}

fn parse_full_name(value: &str, source: ContextSource) -> Result<RepositoryContext, ContextError> {
    let Some((owner, repository)) = value.trim().split_once('/') else {
        return Err(invalid(source, "expected owner/repository"));
    };
    if repository.contains('/') {
        return Err(invalid(source, "expected exactly one path separator"));
    }
    context_from_parts("github.com", owner, repository, source)
}

fn parse_repository_reference(
    value: &str,
    source: ContextSource,
) -> Result<RepositoryContext, ContextError> {
    let value = value.trim();
    if value.starts_with("git@") {
        let Some((user, remainder)) = value.split_once('@') else {
            return Err(invalid(source, "malformed SSH remote"));
        };
        if user.is_empty() {
            return Err(invalid(source, "malformed SSH remote"));
        }
        let Some((host, path)) = remainder.split_once(':') else {
            return Err(invalid(source, "malformed SSH remote"));
        };
        return parse_host_path(host, path, source);
    }

    if let Some(remainder) = value.strip_prefix("ssh://") {
        let Some((authority, path)) = remainder.split_once('/') else {
            return Err(invalid(source, "malformed SSH URL"));
        };
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        return parse_host_path(host, path, source);
    }

    for scheme in ["https://", "http://"] {
        if let Some(remainder) = value.strip_prefix(scheme) {
            if remainder.contains('?') || remainder.contains('#') {
                return Err(invalid(
                    source,
                    "repository URL must not contain a query or fragment",
                ));
            }
            let Some((authority, path)) = remainder.split_once('/') else {
                return Err(invalid(
                    source,
                    "repository URL is missing owner/repository",
                ));
            };
            let host = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            return parse_host_path(host, path, source);
        }
    }

    parse_full_name(value, source)
}

fn parse_host_path(
    host: &str,
    path: &str,
    source: ContextSource,
) -> Result<RepositoryContext, ContextError> {
    let path = path.trim_start_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let Some((owner, repository)) = path.split_once('/') else {
        return Err(invalid(source, "expected owner/repository"));
    };
    if repository.contains('/') {
        return Err(invalid(source, "expected exactly one repository path"));
    }
    context_from_parts(host, owner, repository, source)
}

fn context_from_parts(
    host: &str,
    owner: &str,
    repository: &str,
    source: ContextSource,
) -> Result<RepositoryContext, ContextError> {
    let host = host.trim();
    let owner = owner.trim();
    let repository = repository.trim();
    if !valid_host(host) {
        return Err(invalid(source, "host is malformed"));
    }
    if !valid_component(owner) || !valid_component(repository) {
        return Err(invalid(
            source,
            "owner and repository must be non-empty path components",
        ));
    }
    Ok(RepositoryContext::new(host, owner, repository, source))
}

fn valid_host(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn invalid(source: ContextSource, message: impl Into<String>) -> ContextError {
    ContextError::InvalidRepository {
        source,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug)]
    struct FakeGit {
        url: Option<String>,
    }

    impl GitRemoteProvider for FakeGit {
        fn origin_url(&self, _root: &Path) -> Result<Option<String>, ContextError> {
            Ok(self.url.clone())
        }
    }

    fn resolver(url: Option<&str>) -> RepositoryContextResolver<FakeGit> {
        RepositoryContextResolver::new(FakeGit {
            url: url.map(str::to_owned),
        })
    }

    #[test]
    fn resolves_owner_and_repo_arguments() {
        let context = resolver(None)
            .resolve(&RepositoryContextRequest::owner_repo(
                "ExampleOrg",
                "project",
            ))
            .unwrap();
        assert_eq!(context.host, "github.com");
        assert_eq!(context.owner, "ExampleOrg");
        assert_eq!(context.repository, "project");
        assert_eq!(context.source, ContextSource::ToolArguments);
    }

    #[test]
    fn resolves_full_name_and_repository_url() {
        let full = resolver(None)
            .resolve(&RepositoryContextRequest::repository("ExampleOrg/project"))
            .unwrap();
        assert_eq!(full.source, ContextSource::ToolArguments);

        let url = resolver(None)
            .resolve(&RepositoryContextRequest::url(
                "https://github.example.com/ExampleOrg/project.git",
            ))
            .unwrap();
        assert_eq!(url.host, "github.example.com");
        assert_eq!(url.source, ContextSource::RepositoryUrl);
    }

    #[test]
    fn resolves_https_and_ssh_git_remotes() {
        for remote in [
            "https://github.com/ExampleOrg/project.git",
            "git@github.com:ExampleOrg/project.git",
            "ssh://git@github.example.com/ExampleOrg/project.git",
        ] {
            let context = resolver(Some(remote))
                .resolve(&RepositoryContextRequest::default().with_mcp_root("/workspace"))
                .unwrap();
            assert_eq!(context.owner, "ExampleOrg");
            assert_eq!(context.repository, "project");
            assert_eq!(context.source, ContextSource::GitRemote);
        }
    }

    #[test]
    fn explicit_context_wins_over_conflicting_url_and_remote() {
        let request = RepositoryContextRequest::owner_repo("ExplicitOrg", "explicit")
            .with_mcp_root("/workspace");
        let mut request = request;
        request.repository_url = Some("https://github.com/OtherOrg/other".to_owned());
        let context = resolver(Some("git@github.com:RemoteOrg/remote.git"))
            .resolve(&request)
            .unwrap();
        assert_eq!(context.full_name(), "ExplicitOrg/explicit");
        assert_eq!(context.source, ContextSource::ToolArguments);
    }

    #[test]
    fn mcp_root_precedes_workspace_root_and_default() {
        let request = RepositoryContextRequest::default()
            .with_mcp_root("/mcp")
            .with_workspace_root("/workspace")
            .with_configured_context(RepositoryContext::new(
                "github.com",
                "DefaultOrg",
                "default",
                ContextSource::Explicit,
            ));
        let context = resolver(Some("git@github.com:McpOrg/mcp.git"))
            .resolve(&request)
            .unwrap();
        assert_eq!(context.full_name(), "McpOrg/mcp");
        assert_eq!(context.source, ContextSource::GitRemote);
    }

    #[test]
    fn configured_context_is_the_final_fallback() {
        let request =
            RepositoryContextRequest::default().with_configured_context(RepositoryContext::new(
                "github.com",
                "ExampleOrg",
                "project",
                ContextSource::Explicit,
            ));
        let context = resolver(None).resolve(&request).unwrap();
        assert_eq!(context.source, ContextSource::Default);
    }

    #[test]
    fn incomplete_or_malformed_context_is_rejected() {
        let error = resolver(None)
            .resolve(&RepositoryContextRequest {
                owner: Some("ExampleOrg".to_owned()),
                ..RepositoryContextRequest::default()
            })
            .unwrap_err();
        assert_eq!(error, ContextError::IncompleteExplicitArguments);

        let error = resolver(None)
            .resolve(&RepositoryContextRequest::url("git@github.com:bad"))
            .unwrap_err();
        assert!(error.to_string().contains("invalid repository context"));
    }

    #[test]
    fn context_errors_do_not_echo_remote_values() {
        let error = resolver(None)
            .resolve(&RepositoryContextRequest::url(
                "https://github.com/owner/invalid/repo?token=synthetic",
            ))
            .unwrap_err();
        assert!(!error.to_string().contains("synthetic"));
    }
}
