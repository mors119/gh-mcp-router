//! Deterministic, side-effect-free repository-to-profile routing.
//!
//! Routing consumes only normalized repository context and typed configuration.
//! It does not retrieve credentials, execute subprocesses, call GitHub APIs,
//! or know about MCP transport. A route selects a credential *profile*; a
//! separate credential provider resolves that profile later.

use std::fmt;

use crate::{
    config::{Config, RouteRule},
    context::RepositoryContext,
};

/// Broad operation policy used by later request-routing features.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationClass {
    /// An operation that does not change GitHub state.
    Read,
    /// An operation that may change GitHub state.
    Write,
    /// An operation whose safety cannot be established from maintained
    /// upstream metadata.
    Unknown,
}

impl fmt::Display for OperationClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Unknown => "unknown",
        })
    }
}

impl OperationClass {
    /// Classify only names in maintained allowlists. New upstream tools remain
    /// conservative until explicitly reviewed.
    pub fn from_tool_name(name: &str) -> Self {
        match name {
            "get_file_contents"
            | "get_repository"
            | "get_issue"
            | "list_issues"
            | "search_issues"
            | "get_pull_request"
            | "list_pull_requests"
            | "search_pull_requests"
            | "search_code"
            | "search_repositories"
            | "get_commit"
            | "list_commits"
            | "list_branches"
            | "list_tags"
            | "get_release_by_tag"
            | "list_releases"
            | "get_workflow_run"
            | "list_workflow_runs"
            | "get_job_logs"
            | "get_me"
            | "get_project"
            | "list_projects"
            | "list_labels"
            | "get_discussion"
            | "list_discussions" => Self::Read,
            "create_issue"
            | "update_issue"
            | "add_issue_comment"
            | "create_pull_request"
            | "update_pull_request"
            | "merge_pull_request"
            | "close_pull_request"
            | "create_repository"
            | "update_repository"
            | "delete_repository"
            | "create_or_update_file"
            | "push_files"
            | "delete_file"
            | "create_branch"
            | "delete_branch"
            | "create_release"
            | "update_release"
            | "delete_release"
            | "rerun_workflow"
            | "cancel_workflow_run"
            | "create_fork"
            | "add_assignees"
            | "remove_assignees"
            | "add_labels"
            | "remove_labels"
            | "lock_issue"
            | "unlock_issue"
            | "update_pull_request_branch"
            | "request_copilot_review" => Self::Write,
            _ => Self::Unknown,
        }
    }
}

/// Specificity used to compare otherwise matching route rules.
///
/// The order is intentionally explicit and is part of the v0.1 routing
/// contract: exact repository, repository glob, owner, host, then default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteSpecificity {
    ExactRepository,
    RepositoryGlob,
    Owner,
    Host,
    Default,
}

impl fmt::Display for RouteSpecificity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::ExactRepository => "exact_repository",
            Self::RepositoryGlob => "repository_glob",
            Self::Owner => "owner",
            Self::Host => "host",
            Self::Default => "default",
        };
        formatter.write_str(value)
    }
}

/// Explainable metadata for a selected route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutingDecision {
    /// The repository identity rendered as `owner/repository` from context.
    pub repository: String,
    /// The profile name to pass to the credential/session layers.
    pub selected_profile: String,
    /// A human-readable description of the winning rule, or `None` for the
    /// configured default profile.
    pub matched_rule: Option<String>,
    pub specificity: RouteSpecificity,
    pub fallback_used: bool,
}

impl RoutingDecision {
    /// Return the selected profile without exposing or resolving credentials.
    pub fn profile(&self) -> &str {
        &self.selected_profile
    }
}

/// A route candidate involved in an ambiguous result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteCandidate {
    pub profile: String,
    pub matched_rule: String,
}

/// Returned when equally specific rules select different profiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmbiguousRouting {
    pub repository: String,
    pub specificity: RouteSpecificity,
    pub candidates: Vec<RouteCandidate>,
}

/// Returned when no route and no default profile apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoMatch {
    pub repository: String,
}

/// Complete result of a routing evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoutingResult {
    Selected(RoutingDecision),
    NoMatch(NoMatch),
    Ambiguous(AmbiguousRouting),
}

/// A route selected after applying operation-specific safety policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafeRoutingDecision {
    pub operation: OperationClass,
    pub context: RepositoryContext,
    pub routing: RoutingDecision,
}

/// Failure to safely associate an operation with one identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SafeRoutingError {
    MissingRepositoryContext {
        operation: OperationClass,
    },
    NoMatch {
        operation: OperationClass,
        repository: String,
    },
    Ambiguous {
        operation: OperationClass,
        route: AmbiguousRouting,
    },
    FallbackNotAllowed {
        operation: OperationClass,
        repository: String,
    },
}

impl fmt::Display for SafeRoutingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRepositoryContext { operation } => write!(
                formatter,
                "Cannot safely select a GitHub identity for this {operation} operation. Repository context is missing or ambiguous. Provide owner/repository or configure a matching route."
            ),
            Self::NoMatch {
                operation,
                repository,
            } => write!(
                formatter,
                "Cannot safely select a GitHub identity for this {operation} operation. No route matches {repository}; provide a matching route."
            ),
            Self::Ambiguous { operation, route } => write!(
                formatter,
                "Cannot safely select a GitHub identity for this {operation} operation. Routing for {} is ambiguous at {} specificity; configure one matching profile.",
                route.repository, route.specificity
            ),
            Self::FallbackNotAllowed {
                operation,
                repository,
            } => write!(
                formatter,
                "Cannot safely select a GitHub identity for this {operation} operation. The route for {repository} relies on a default profile, which is not permitted by the configured safety policy."
            ),
        }
    }
}

impl std::error::Error for SafeRoutingError {}

impl RoutingResult {
    pub fn selected_profile(&self) -> Option<&str> {
        match self {
            Self::Selected(decision) => Some(decision.profile()),
            Self::NoMatch(_) | Self::Ambiguous(_) => None,
        }
    }
}

/// Pure evaluator for a validated configuration.
pub struct RoutingEngine<'config> {
    config: &'config Config,
}

/// One entry in a routing evaluation trace. This contains only route metadata
/// and is safe to expose in diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleEvaluation {
    pub index: usize,
    pub rule: String,
    pub matched: bool,
    pub specificity: Option<RouteSpecificity>,
}

/// Complete, side-effect-free routing trace used by CLI diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutingExplanation {
    pub context: RepositoryContext,
    pub rules: Vec<RuleEvaluation>,
    pub result: RoutingResult,
}

impl<'config> RoutingEngine<'config> {
    pub fn new(config: &'config Config) -> Self {
        Self { config }
    }

    /// Evaluate all matching rules and select the highest-specificity level.
    ///
    /// Multiple rules at that level are safe to resolve only when they select
    /// the same profile. Conflicting profiles return `Ambiguous`, even when
    /// one appeared earlier in the configuration, so a write cannot silently
    /// use an unintended identity.
    pub fn evaluate(&self, context: &RepositoryContext) -> RoutingResult {
        evaluate(self.config, context)
    }

    pub fn route(&self, context: &RepositoryContext) -> RoutingResult {
        self.evaluate(context)
    }
}

/// Evaluate a repository context against a configuration.
pub fn evaluate(config: &Config, context: &RepositoryContext) -> RoutingResult {
    let repository = repository_name(context);
    let mut best_specificity = None;
    let mut matches = Vec::new();

    for rule in &config.routes {
        let Some(specificity) = matching_specificity(rule, context) else {
            continue;
        };

        match best_specificity {
            None => {
                best_specificity = Some(specificity);
                matches.push((rule, specificity));
            }
            Some(best) if specificity_rank(specificity) < specificity_rank(best) => {}
            Some(best) if specificity_rank(specificity) > specificity_rank(best) => {
                best_specificity = Some(specificity);
                matches.clear();
                matches.push((rule, specificity));
            }
            Some(_) => matches.push((rule, specificity)),
        }
    }

    if let Some(specificity) = best_specificity {
        let candidates = matches
            .iter()
            .map(|(rule, _)| RouteCandidate {
                profile: rule.profile.clone(),
                matched_rule: describe_rule(rule),
            })
            .collect::<Vec<_>>();
        let first = &candidates[0];

        if candidates
            .iter()
            .any(|candidate| candidate.profile != first.profile)
        {
            return RoutingResult::Ambiguous(AmbiguousRouting {
                repository,
                specificity,
                candidates,
            });
        }

        return RoutingResult::Selected(RoutingDecision {
            repository,
            selected_profile: first.profile.clone(),
            matched_rule: Some(first.matched_rule.clone()),
            specificity,
            fallback_used: false,
        });
    }

    match &config.default_profile {
        Some(profile) => RoutingResult::Selected(RoutingDecision {
            repository,
            selected_profile: profile.clone(),
            matched_rule: None,
            specificity: RouteSpecificity::Default,
            fallback_used: true,
        }),
        None => RoutingResult::NoMatch(NoMatch { repository }),
    }
}

/// Evaluate a route and retain every rule considered along the way.
pub fn explain(config: &Config, context: &RepositoryContext) -> RoutingExplanation {
    let rules = config
        .routes
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            let specificity = matching_specificity(rule, context);
            RuleEvaluation {
                index,
                rule: describe_rule(rule),
                matched: specificity.is_some(),
                specificity,
            }
        })
        .collect();

    RoutingExplanation {
        context: context.clone(),
        rules,
        result: evaluate(config, context),
    }
}

/// Alias emphasizing that this function performs no external work.
pub fn route(config: &Config, context: &RepositoryContext) -> RoutingResult {
    evaluate(config, context)
}

/// Apply routing and the conservative read/write/unknown fallback policy.
///
/// An explicit matching rule is sufficient for a known operation. A default
/// profile is usable only when the corresponding policy explicitly permits it;
/// unknown operations never use a default fallback. Ambiguous routes always
/// fail closed.
pub fn route_safely(
    config: &Config,
    context: Option<&RepositoryContext>,
    operation: OperationClass,
) -> Result<SafeRoutingDecision, SafeRoutingError> {
    let context = context.ok_or(SafeRoutingError::MissingRepositoryContext { operation })?;
    let result = evaluate(config, context);
    let routing = match result {
        RoutingResult::Selected(decision) => {
            if decision.fallback_used && !fallback_allowed(config, operation) {
                return Err(SafeRoutingError::FallbackNotAllowed {
                    operation,
                    repository: decision.repository,
                });
            }
            decision
        }
        RoutingResult::NoMatch(no_match) => {
            return Err(SafeRoutingError::NoMatch {
                operation,
                repository: no_match.repository,
            })
        }
        RoutingResult::Ambiguous(route) => {
            return Err(SafeRoutingError::Ambiguous { operation, route })
        }
    };

    Ok(SafeRoutingDecision {
        operation,
        context: context.clone(),
        routing,
    })
}

fn fallback_allowed(config: &Config, operation: OperationClass) -> bool {
    match operation {
        OperationClass::Read => {
            config.ambiguity_policy.read == crate::config::AmbiguityPolicy::DefaultProfile
        }
        OperationClass::Write => {
            config.ambiguity_policy.write == crate::config::AmbiguityPolicy::DefaultProfile
        }
        OperationClass::Unknown => false,
    }
}

fn repository_name(context: &RepositoryContext) -> String {
    format!("{}/{}", context.owner.trim(), context.repository.trim())
}

fn normalized_repository_name(context: &RepositoryContext) -> String {
    normalize(&repository_name(context))
}

fn matching_specificity(rule: &RouteRule, context: &RepositoryContext) -> Option<RouteSpecificity> {
    if rule
        .host
        .as_deref()
        .is_some_and(|host| normalize(host) != normalize(&context.host))
    {
        return None;
    }
    if rule
        .owner
        .as_deref()
        .is_some_and(|owner| normalize(owner) != normalize(&context.owner))
    {
        return None;
    }

    if let Some(pattern) = &rule.repository {
        let repository = normalized_repository_name(context);
        let pattern = normalize(pattern);
        if pattern.contains('*') {
            glob_matches(&pattern, &repository).then_some(RouteSpecificity::RepositoryGlob)
        } else {
            (pattern == repository).then_some(RouteSpecificity::ExactRepository)
        }
    } else if rule.owner.is_some() {
        Some(RouteSpecificity::Owner)
    } else if rule.host.is_some() {
        Some(RouteSpecificity::Host)
    } else {
        None
    }
}

fn describe_rule(rule: &RouteRule) -> String {
    let mut fields = Vec::new();
    if let Some(repository) = &rule.repository {
        fields.push(format!("repo: {}", repository.trim()));
    }
    if let Some(owner) = &rule.owner {
        fields.push(format!("owner: {}", owner.trim()));
    }
    if let Some(host) = &rule.host {
        fields.push(format!("host: {}", host.trim()));
    }
    fields.join(", ")
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn specificity_rank(specificity: RouteSpecificity) -> u8 {
    match specificity {
        RouteSpecificity::ExactRepository => 5,
        RouteSpecificity::RepositoryGlob => 4,
        RouteSpecificity::Owner => 3,
        RouteSpecificity::Host => 2,
        RouteSpecificity::Default => 1,
    }
}

/// Match one `*` without allowing it to cross the repository-name separator.
fn glob_matches(pattern: &str, value: &str) -> bool {
    let Some((pattern_owner, pattern_repository)) = pattern.split_once('/') else {
        return false;
    };
    let Some((owner, repository)) = value.split_once('/') else {
        return false;
    };
    segment_glob_matches(pattern_owner, owner)
        && segment_glob_matches(pattern_repository, repository)
}

fn segment_glob_matches(pattern: &str, value: &str) -> bool {
    if value.contains('/') {
        return false;
    }
    match pattern.split_once('*') {
        Some((prefix, suffix)) => {
            value.starts_with(prefix)
                && value.ends_with(suffix)
                && value.len() >= prefix.len() + suffix.len()
        }
        None => pattern == value,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{
        config::{ProfileConfig, RouteRule},
        context::ContextSource,
    };

    fn config(routes: Vec<RouteRule>, default_profile: Option<&str>) -> Config {
        let profiles = ["personal", "work", "other"]
            .into_iter()
            .map(|name| {
                (
                    name.to_owned(),
                    ProfileConfig {
                        provider: "gh".to_owned(),
                        user: name.to_owned(),
                        gh_config_dir: None,
                        host: None,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        Config {
            profiles,
            routes,
            default_profile: default_profile.map(str::to_owned),
            ambiguity_policy: Default::default(),
        }
    }

    fn rule(
        profile: &str,
        repository: Option<&str>,
        owner: Option<&str>,
        host: Option<&str>,
    ) -> RouteRule {
        RouteRule {
            profile: profile.to_owned(),
            repository: repository.map(str::to_owned),
            owner: owner.map(str::to_owned),
            host: host.map(str::to_owned),
        }
    }

    fn context(host: &str, owner: &str, repository: &str) -> RepositoryContext {
        RepositoryContext::new(host, owner, repository, ContextSource::Explicit)
    }

    fn selected(result: RoutingResult) -> RoutingDecision {
        match result {
            RoutingResult::Selected(decision) => decision,
            other => panic!("expected selected result, got {other:?}"),
        }
    }

    #[test]
    fn exact_repository_overrides_owner() {
        let config = config(
            vec![
                rule("personal", None, Some("ExampleOrg"), None),
                rule("work", Some("ExampleOrg/private-api"), None, None),
            ],
            None,
        );
        let decision = selected(evaluate(
            &config,
            &context("github.com", "ExampleOrg", "private-api"),
        ));

        assert_eq!(decision.selected_profile, "work");
        assert_eq!(decision.specificity, RouteSpecificity::ExactRepository);
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("repo: ExampleOrg/private-api")
        );
        assert!(!decision.fallback_used);
    }

    #[test]
    fn repository_glob_overrides_owner_and_does_not_cross_separator() {
        let config = config(
            vec![
                rule("personal", None, Some("ExampleOrg"), None),
                rule("work", Some("ExampleOrg/security-*"), None, None),
            ],
            None,
        );
        let decision = selected(evaluate(
            &config,
            &context("github.com", "exampleorg", "security-api"),
        ));

        assert_eq!(decision.selected_profile, "work");
        assert_eq!(decision.specificity, RouteSpecificity::RepositoryGlob);
        assert!(!glob_matches(
            "exampleorg/security-*",
            "exampleorg/security-api/x"
        ));
    }

    #[test]
    fn owner_overrides_host() {
        let config = config(
            vec![
                rule("personal", None, None, Some("github.com")),
                rule("work", None, Some("ExampleOrg"), None),
            ],
            None,
        );
        let decision = selected(evaluate(
            &config,
            &context("github.com", "EXAMPLEORG", "backend"),
        ));

        assert_eq!(decision.selected_profile, "work");
        assert_eq!(decision.specificity, RouteSpecificity::Owner);
    }

    #[test]
    fn host_routes_are_separated_by_normalized_host() {
        let config = config(
            vec![
                rule("personal", None, None, Some("github.com")),
                rule("work", None, None, Some("github.example.com")),
            ],
            None,
        );
        let github = selected(evaluate(
            &config,
            &context("GITHUB.COM", "ExampleOrg", "repo"),
        ));
        let enterprise = selected(evaluate(
            &config,
            &context("github.example.com", "ExampleOrg", "repo"),
        ));
        assert_eq!(github.selected_profile, "personal");
        assert_eq!(enterprise.selected_profile, "work");

        let result = evaluate(&config, &context("other.example.com", "ExampleOrg", "repo"));
        assert!(matches!(result, RoutingResult::NoMatch(_)));
    }

    #[test]
    fn default_is_used_only_when_nothing_matches() {
        let config = config(
            vec![rule("work", None, Some("ExampleOrg"), None)],
            Some("personal"),
        );
        let decision = selected(evaluate(
            &config,
            &context("github.com", "OtherOrg", "repo"),
        ));

        assert_eq!(decision.selected_profile, "personal");
        assert_eq!(decision.specificity, RouteSpecificity::Default);
        assert!(decision.matched_rule.is_none());
        assert!(decision.fallback_used);
    }

    #[test]
    fn no_default_returns_no_match() {
        let result = evaluate(
            &config(Vec::new(), None),
            &context("github.com", "ExampleOrg", "repo"),
        );
        assert_eq!(
            result,
            RoutingResult::NoMatch(NoMatch {
                repository: "ExampleOrg/repo".to_owned()
            })
        );
    }

    #[test]
    fn conflicting_equally_specific_routes_are_ambiguous() {
        let config = config(
            vec![
                rule("personal", Some("ExampleOrg/repo"), None, None),
                rule("work", Some("exampleorg/REPO"), None, None),
            ],
            Some("other"),
        );
        let result = evaluate(&config, &context("github.com", "exampleorg", "repo"));

        match result {
            RoutingResult::Ambiguous(ambiguous) => {
                assert_eq!(ambiguous.specificity, RouteSpecificity::ExactRepository);
                assert_eq!(ambiguous.candidates.len(), 2);
                assert_eq!(ambiguous.candidates[0].profile, "personal");
                assert_eq!(ambiguous.candidates[1].profile, "work");
            }
            other => panic!("expected ambiguous result, got {other:?}"),
        }
    }

    #[test]
    fn same_specificity_same_profile_is_deterministic() {
        let config = config(
            vec![
                rule("work", None, Some("ExampleOrg"), None),
                rule("work", None, Some("exampleorg"), None),
            ],
            None,
        );
        let decision = selected(evaluate(
            &config,
            &context("github.com", "ExampleOrg", "repo"),
        ));
        assert_eq!(decision.selected_profile, "work");
        assert_eq!(decision.matched_rule.as_deref(), Some("owner: ExampleOrg"));
    }

    #[test]
    fn unrelated_configuration_does_not_change_explanation() {
        let base = config(vec![rule("work", None, Some("ExampleOrg"), None)], None);
        let extended = config(
            vec![
                rule("other", None, Some("Unrelated"), None),
                rule("work", None, Some("ExampleOrg"), None),
            ],
            None,
        );
        let context = context("github.com", "ExampleOrg", "repo");
        assert_eq!(evaluate(&base, &context), evaluate(&extended, &context));
    }

    #[test]
    fn routing_types_contain_no_credential_values() {
        let config = config(
            vec![rule("work", Some("ExampleOrg/repo"), None, None)],
            None,
        );
        let result = evaluate(&config, &context("github.com", "ExampleOrg", "repo"));
        let formatted = format!("{result:?}");
        assert!(!formatted.contains("ghp_"));
        assert!(!formatted.contains("github_pat_"));
    }

    #[test]
    fn safe_routing_allows_explicit_write_route() {
        let config = config(
            vec![rule("work", Some("ExampleOrg/repo"), None, None)],
            Some("personal"),
        );
        let decision = route_safely(
            &config,
            Some(&context("github.com", "ExampleOrg", "repo")),
            OperationClass::Write,
        )
        .unwrap();
        assert_eq!(decision.routing.selected_profile, "work");
        assert!(!decision.routing.fallback_used);
    }

    #[test]
    fn safe_routing_rejects_unmatched_write_and_ambiguous_write() {
        let no_route = config(Vec::new(), Some("personal"));
        let error = route_safely(
            &no_route,
            Some(&context("github.com", "ExampleOrg", "repo")),
            OperationClass::Write,
        )
        .unwrap_err();
        assert!(matches!(error, SafeRoutingError::FallbackNotAllowed { .. }));

        let ambiguous = config(
            vec![
                rule("personal", Some("ExampleOrg/repo"), None, None),
                rule("work", Some("ExampleOrg/repo"), None, None),
            ],
            None,
        );
        let error = route_safely(
            &ambiguous,
            Some(&context("github.com", "ExampleOrg", "repo")),
            OperationClass::Write,
        )
        .unwrap_err();
        assert!(matches!(error, SafeRoutingError::Ambiguous { .. }));
    }

    #[test]
    fn safe_routing_allows_explicit_read_fallback_and_marks_it() {
        let config = config(Vec::new(), Some("personal"));
        let decision = route_safely(
            &config,
            Some(&context("github.com", "ExampleOrg", "repo")),
            OperationClass::Read,
        )
        .unwrap();
        assert_eq!(decision.routing.selected_profile, "personal");
        assert!(decision.routing.fallback_used);
    }

    #[test]
    fn write_fallback_requires_and_honors_explicit_policy() {
        let mut config = config(Vec::new(), Some("personal"));
        config.ambiguity_policy.write = crate::config::AmbiguityPolicy::DefaultProfile;
        let decision = route_safely(
            &config,
            Some(&context("github.com", "ExampleOrg", "repo")),
            OperationClass::Write,
        )
        .unwrap();
        assert_eq!(decision.routing.selected_profile, "personal");
        assert!(decision.routing.fallback_used);
    }

    #[test]
    fn unknown_operations_never_use_default_fallback() {
        let config = config(Vec::new(), Some("personal"));
        let error = route_safely(
            &config,
            Some(&context("github.com", "ExampleOrg", "repo")),
            OperationClass::Unknown,
        )
        .unwrap_err();
        assert!(matches!(error, SafeRoutingError::FallbackNotAllowed { .. }));
    }

    #[test]
    fn operation_classification_is_allowlist_based() {
        assert_eq!(
            OperationClass::from_tool_name("get_repository"),
            OperationClass::Read
        );
        assert_eq!(
            OperationClass::from_tool_name("create_issue"),
            OperationClass::Write
        );
        assert_eq!(
            OperationClass::from_tool_name("future_tool_with_update_in_name"),
            OperationClass::Unknown
        );
    }

    #[test]
    fn missing_context_is_an_actionable_safe_routing_error() {
        let error =
            route_safely(&config(Vec::new(), None), None, OperationClass::Write).unwrap_err();
        assert!(error.to_string().contains("owner/repository"));
        assert!(!error.to_string().contains("ghp_"));
    }
}
