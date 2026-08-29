//! Pure routing decision domain.
//!
//! This module will eventually evaluate repository context and configuration.
//! It does not retrieve credentials, invoke external commands, call GitHub or
//! MCP APIs, or own CLI presentation state.

/// Broad operation policy used when a future router evaluates a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationClass {
    /// An operation that does not change GitHub state.
    Read,
    /// An operation that may change GitHub state.
    Write,
}

/// The profile selected by a future routing evaluation.
///
/// The decision carries a profile identifier only. Credential resolution is a
/// separate concern and is never represented by a plaintext token here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutingDecision {
    pub profile: String,
    pub operation: OperationClass,
    pub matched_rule: Option<String>,
}

impl RoutingDecision {
    /// Construct a decision without performing route evaluation.
    pub fn new(profile: impl Into<String>, operation: OperationClass) -> Self {
        Self {
            profile: profile.into(),
            operation,
            matched_rule: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{OperationClass, RoutingDecision};

    #[test]
    fn routing_decision_is_independent_of_credential_values() {
        let decision = RoutingDecision::new("work", OperationClass::Write);

        assert_eq!(decision.profile, "work");
        assert_eq!(decision.operation, OperationClass::Write);
        assert!(decision.matched_rule.is_none());
    }
}
