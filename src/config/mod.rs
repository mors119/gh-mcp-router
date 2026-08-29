//! Configuration models.
//!
//! This module owns the shape of profiles and route rules. Configuration file
//! parsing and validation are deliberately deferred to a later feature.

use crate::credentials::CredentialRef;

/// A named GitHub identity configuration.
///
/// A profile stores a reference to a credential provider. It never stores a
/// plaintext credential value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    /// Stable name used by route rules and routing decisions.
    pub name: String,
    /// Provider-scoped reference used to obtain credentials later.
    pub credential: CredentialRef,
}

impl Profile {
    /// Construct a profile from a name and a provider reference.
    pub fn new(name: impl Into<String>, credential: CredentialRef) -> Self {
        Self {
            name: name.into(),
            credential,
        }
    }
}

/// A declarative route target and its optional repository match fields.
///
/// Matching and precedence are intentionally not implemented here. The
/// fields establish the configuration boundary that the routing engine will
/// consume later.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteRule {
    /// Profile name selected when this rule matches.
    pub profile: String,
    /// Optional GitHub host constraint.
    pub host: Option<String>,
    /// Optional owner or organization constraint.
    pub owner: Option<String>,
    /// Optional repository or repository-pattern constraint.
    pub repository: Option<String>,
}

impl RouteRule {
    /// Construct a route rule that targets a profile.
    pub fn for_profile(profile: impl Into<String>) -> Self {
        Self {
            profile: profile.into(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Profile, RouteRule};
    use crate::credentials::CredentialRef;

    #[test]
    fn profile_references_credentials_without_containing_a_token() {
        let profile = Profile::new("work", CredentialRef::new("gh", "work-account"));

        assert_eq!(profile.name, "work");
        assert_eq!(profile.credential.provider(), "gh");
        assert_eq!(profile.credential.name(), "work-account");
    }

    #[test]
    fn route_rule_can_be_constructed_independently() {
        let rule = RouteRule {
            profile: "work".to_owned(),
            host: Some("github.com".to_owned()),
            owner: Some("ExampleOrg".to_owned()),
            repository: Some("backend".to_owned()),
        };

        assert_eq!(rule.profile, "work");
        assert_eq!(rule.owner.as_deref(), Some("ExampleOrg"));
    }
}
