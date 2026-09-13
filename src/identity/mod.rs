//! Typed identity and execution-target values shared by configuration and
//! routing.
//!
//! These values intentionally contain identity metadata only. Credential
//! material, including GitHub App private keys and installation tokens, is
//! owned by credential providers and never belongs in a routing target.

use std::fmt;

use serde::{Deserialize, Serialize};

/// An identifier for a credential provider, not a credential itself.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialProviderId(String);

impl CredentialProviderId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CredentialProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Identity presented by an MCP client or another caller of the router.
///
/// This is deliberately not interchangeable with a GitHub account or an
/// installation ID. Authentication of this principal is introduced by a
/// later remote-transport feature.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientPrincipalRef(String);

impl ClientPrincipalRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Router-local principal used for policy and audit ownership.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RouterPrincipalRef(String);

impl RouterPrincipalRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A GitHub user or organization account reference.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubAccountRef {
    /// GitHub hostname without a scheme or path.
    pub host: String,
    /// GitHub login or organization name.
    pub login: String,
}

impl GitHubAccountRef {
    pub fn new(host: impl Into<String>, login: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            login: login.into(),
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn login(&self) -> &str {
        &self.login
    }
}

/// Stable GitHub App installation identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GitHubInstallationId(u64);

impl GitHubInstallationId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for GitHubInstallationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Repository access metadata associated with an installation.
///
/// Grants are routing metadata and may be cached locally. GitHub remains the
/// source of truth for whether a grant is current, suspended, or revoked.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryGrant {
    pub host: String,
    pub owner: String,
    pub repository: String,
}

impl RepositoryGrant {
    pub fn new(
        host: impl Into<String>,
        owner: impl Into<String>,
        repository: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            owner: owner.into(),
            repository: repository.into(),
        }
    }

    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repository)
    }
}

/// Where a selected target obtains its credential.
///
/// This is a provider-facing description. It is separate from
/// [`RoutingTarget`] so choosing a target does not imply that credential
/// resolution has happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialSource {
    LocalGh {
        provider: CredentialProviderId,
        account: GitHubAccountRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gh_config_dir: Option<String>,
    },
    #[serde(rename = "github_app_installation")]
    GitHubAppInstallation {
        /// GitHub host scopes installation IDs and selects the token endpoint.
        host: String,
        installation_id: GitHubInstallationId,
    },
}

/// Authentication identity used by one official GitHub MCP upstream session.
///
/// This remains metadata-only. The corresponding token is resolved by a
/// provider at the upstream process boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpstreamIdentity {
    LocalGhAccount {
        account: GitHubAccountRef,
    },
    #[serde(rename = "github_app_installation")]
    GitHubAppInstallation {
        /// GitHub host scopes installation IDs and the upstream endpoint.
        host: String,
        installation_id: GitHubInstallationId,
    },
}

/// Credential-independent execution target selected by routing.
///
/// Both target kinds use the same repository route engine. Installation
/// discovery, token minting, and upstream support for the app variant are
/// intentionally outside this type and outside routing evaluation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoutingTarget {
    LocalGh {
        provider: CredentialProviderId,
        account: GitHubAccountRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gh_config_dir: Option<String>,
    },
    #[serde(rename = "github_app_installation")]
    GitHubAppInstallation {
        installation_id: GitHubInstallationId,
        account: GitHubAccountRef,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        repository_grants: Vec<RepositoryGrant>,
    },
}

impl RoutingTarget {
    pub fn local_gh(
        provider: impl Into<String>,
        account: GitHubAccountRef,
        gh_config_dir: Option<String>,
    ) -> Self {
        Self::LocalGh {
            provider: CredentialProviderId::new(provider),
            account,
            gh_config_dir,
        }
    }

    pub fn github_app_installation(
        installation_id: GitHubInstallationId,
        account: GitHubAccountRef,
    ) -> Self {
        Self::GitHubAppInstallation {
            installation_id,
            account,
            repository_grants: Vec::new(),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::LocalGh { .. } => "local_gh",
            Self::GitHubAppInstallation { .. } => "github_app_installation",
        }
    }

    pub fn account(&self) -> &GitHubAccountRef {
        match self {
            Self::LocalGh { account, .. } | Self::GitHubAppInstallation { account, .. } => account,
        }
    }

    pub fn credential_source(&self) -> CredentialSource {
        match self {
            Self::LocalGh {
                provider,
                account,
                gh_config_dir,
            } => CredentialSource::LocalGh {
                provider: provider.clone(),
                account: account.clone(),
                gh_config_dir: gh_config_dir.clone(),
            },
            Self::GitHubAppInstallation {
                installation_id,
                account,
                ..
            } => CredentialSource::GitHubAppInstallation {
                host: account.host().to_owned(),
                installation_id: *installation_id,
            },
        }
    }

    pub fn upstream_identity(&self) -> UpstreamIdentity {
        match self {
            Self::LocalGh { account, .. } => UpstreamIdentity::LocalGhAccount {
                account: account.clone(),
            },
            Self::GitHubAppInstallation {
                installation_id,
                account,
                ..
            } => UpstreamIdentity::GitHubAppInstallation {
                host: account.host().to_owned(),
                installation_id: *installation_id,
            },
        }
    }

    pub fn repository_grants(&self) -> &[RepositoryGrant] {
        match self {
            Self::LocalGh { .. } => &[],
            Self::GitHubAppInstallation {
                repository_grants, ..
            } => repository_grants,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_principal_and_installation_id_are_distinct_domain_values() {
        let client = ClientPrincipalRef::new("agent:example");
        let installation = GitHubInstallationId::new(42);

        assert_eq!(client.as_str(), "agent:example");
        assert_eq!(installation.value(), 42);
    }

    #[test]
    fn target_derives_separate_credential_and_upstream_identity_views() {
        let target = RoutingTarget::github_app_installation(
            GitHubInstallationId::new(42),
            GitHubAccountRef::new("github.com", "ExampleOrg"),
        );

        assert_eq!(target.kind(), "github_app_installation");
        assert_eq!(
            target.credential_source(),
            CredentialSource::GitHubAppInstallation {
                host: "github.com".to_owned(),
                installation_id: GitHubInstallationId::new(42)
            }
        );
        assert_eq!(
            target.upstream_identity(),
            UpstreamIdentity::GitHubAppInstallation {
                host: "github.com".to_owned(),
                installation_id: GitHubInstallationId::new(42)
            }
        );
    }

    #[test]
    fn installation_identity_includes_host_in_provider_and_session_views() {
        let github = RoutingTarget::github_app_installation(
            GitHubInstallationId::new(42),
            GitHubAccountRef::new("github.com", "ExampleOrg"),
        );
        let enterprise = RoutingTarget::github_app_installation(
            GitHubInstallationId::new(42),
            GitHubAccountRef::new("ghe.example.com", "ExampleOrg"),
        );

        assert_ne!(github.credential_source(), enterprise.credential_source());
        assert_ne!(github.upstream_identity(), enterprise.upstream_identity());
        assert!(matches!(
            enterprise.credential_source(),
            CredentialSource::GitHubAppInstallation { host, .. }
                if host == "ghe.example.com"
        ));
    }
}
