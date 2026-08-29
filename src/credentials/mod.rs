//! Credential-provider abstractions.
//!
//! This module identifies credentials without retrieving them. Provider
//! implementations, including GitHub CLI integration, are deferred.

use std::fmt;

use crate::security::SecretString;

/// A non-secret reference to a credential provider and account/profile name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialRef {
    provider: String,
    name: String,
}

impl CredentialRef {
    /// Construct a provider-scoped credential reference.
    pub fn new(provider: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            name: name.into(),
        }
    }

    /// Return the provider identifier.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Return the provider-specific credential name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Stable categories for provider failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialErrorKind {
    /// The provider could not supply the requested credential.
    Unavailable,
    /// The reference was not valid for the provider.
    InvalidReference,
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

#[cfg(test)]
mod tests {
    use super::{CredentialError, CredentialErrorKind, CredentialRef};

    #[test]
    fn credential_reference_contains_identity_metadata_only() {
        let reference = CredentialRef::new("gh", "work-account");
        let error = CredentialError::new(reference.clone(), CredentialErrorKind::Unavailable);

        assert_eq!(reference.provider(), "gh");
        assert_eq!(reference.name(), "work-account");
        assert_eq!(error.reference(), &reference);
        assert!(!error.to_string().contains("token"));
    }
}
