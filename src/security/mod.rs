//! Security-sensitive helper types.

use std::fmt;

/// A secret value whose ordinary formatting paths always redact its contents.
///
/// Memory zeroization and provider-specific secret lifecycle rules are deferred
/// until the security-hardening work has a concrete requirement.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Wrap a sensitive value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the wrapped value for an explicitly authorized provider boundary.
    pub fn expose(&self) -> &str {
        &self.0
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

#[cfg(test)]
mod tests {
    use super::SecretString;

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
}
