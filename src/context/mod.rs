//! Normalized repository context.
//!
//! This boundary describes repository identity after a future discovery step.
//! Git remote parsing, MCP root discovery, and fallback selection are deferred.

/// Where normalized repository context came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextSource {
    /// Context supplied explicitly by a caller.
    Explicit,
    /// Context discovered from an MCP workspace or root.
    McpWorkspace,
    /// Context discovered from a local Git remote.
    LocalGit,
    /// Context supplied by an explicit configured fallback.
    ConfiguredFallback,
}

/// Repository identity used by future routing decisions.
///
/// This type contains no authentication state and can therefore be created and
/// passed through the application independently of credential handling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryContext {
    pub host: String,
    pub owner: String,
    pub repository: String,
    pub source: ContextSource,
}

impl RepositoryContext {
    /// Construct normalized context without performing discovery.
    pub fn new(
        host: impl Into<String>,
        owner: impl Into<String>,
        repository: impl Into<String>,
        source: ContextSource,
    ) -> Self {
        Self {
            host: host.into(),
            owner: owner.into(),
            repository: repository.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextSource, RepositoryContext};

    #[test]
    fn repository_context_is_constructible_without_authentication() {
        let context = RepositoryContext::new(
            "github.com",
            "ExampleOrg",
            "backend",
            ContextSource::Explicit,
        );

        assert_eq!(context.host, "github.com");
        assert_eq!(context.owner, "ExampleOrg");
        assert_eq!(context.repository, "backend");
        assert_eq!(context.source, ContextSource::Explicit);
    }
}
