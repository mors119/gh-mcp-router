//! Serializable profile and routing configuration.
//!
//! Configuration is a data-only boundary: it contains credential references,
//! never credential values, and is validated before services are started.

use std::{
    collections::BTreeMap,
    env, fmt, fs,
    path::{Path, PathBuf},
};

use serde::{
    de::{MapAccess, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};

use crate::credentials::CredentialRef;
use crate::security::redact_sensitive_text;

/// A named GitHub identity configuration used by domain callers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub credential: CredentialRef,
}

impl Profile {
    pub fn new(name: impl Into<String>, credential: CredentialRef) -> Self {
        Self {
            name: name.into(),
            credential,
        }
    }
}

/// A profile as represented in the configuration file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    /// Credential provider identifier, for example `gh`.
    pub provider: String,
    /// Provider account or username reference.
    pub user: String,
    /// Optional isolated GitHub CLI configuration directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gh_config_dir: Option<String>,
    /// GitHub host, defaulting to `github.com`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

impl ProfileConfig {
    pub fn credential_ref(&self) -> CredentialRef {
        let mut reference = CredentialRef::new(self.provider.clone(), self.user.clone());
        if let Some(host) = &self.host {
            reference = reference.with_host(host.clone());
        }
        if let Some(config_dir) = &self.gh_config_dir {
            reference = reference.with_gh_config_dir(config_dir.clone());
        }
        reference
    }

    pub fn expanded_gh_config_dir(&self) -> Result<Option<PathBuf>, ConfigError> {
        self.gh_config_dir.as_deref().map(expand_path).transpose()
    }
}

/// A route match. At least one field must be set.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteMatch {
    /// Exact `owner/repo` or a repository glob containing one `*`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Exact GitHub owner or organization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// GitHub host, such as `github.com` or an Enterprise hostname.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// A route rule. The routing engine compares matching rules by specificity;
/// configuration order only makes same-profile ties stable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteRule {
    pub profile: String,
    pub host: Option<String>,
    pub owner: Option<String>,
    pub repository: Option<String>,
}

impl RouteRule {
    pub fn for_profile(profile: impl Into<String>) -> Self {
        Self {
            profile: profile.into(),
            ..Self::default()
        }
    }

    pub fn route_match(&self) -> RouteMatch {
        RouteMatch {
            repo: self.repository.clone(),
            owner: self.owner.clone(),
            host: self.host.clone(),
        }
    }
}

impl Serialize for RouteRule {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct FileRule<'a> {
            #[serde(rename = "match")]
            route_match: RouteMatch,
            profile: &'a str,
        }
        FileRule {
            route_match: self.route_match(),
            profile: &self.profile,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RouteRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FileRule {
            #[serde(rename = "match")]
            route_match: RouteMatch,
            profile: String,
        }
        let value = FileRule::deserialize(deserializer)?;
        Ok(Self {
            profile: value.profile,
            host: value.route_match.host,
            owner: value.route_match.owner,
            repository: value.route_match.repo,
        })
    }
}

/// Policy used when no route gives an unambiguous profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbiguityPolicy {
    Error,
    DefaultProfile,
}

/// Explicit read/write ambiguity behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmbiguityPolicyConfig {
    #[serde(default = "default_read_policy")]
    pub read: AmbiguityPolicy,
    #[serde(default = "default_write_policy")]
    pub write: AmbiguityPolicy,
}

impl Default for AmbiguityPolicyConfig {
    fn default() -> Self {
        Self {
            read: default_read_policy(),
            write: default_write_policy(),
        }
    }
}

fn default_read_policy() -> AmbiguityPolicy {
    AmbiguityPolicy::DefaultProfile
}
fn default_write_policy() -> AmbiguityPolicy {
    AmbiguityPolicy::Error
}

/// Complete v0.1 configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(deserialize_with = "deserialize_unique_profiles")]
    pub profiles: BTreeMap<String, ProfileConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<RouteRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default, skip_serializing_if = "is_default_policy")]
    pub ambiguity_policy: AmbiguityPolicyConfig,
}

fn deserialize_unique_profiles<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, ProfileConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ProfilesVisitor;

    impl<'de> Visitor<'de> for ProfilesVisitor {
        type Value = BTreeMap<String, ProfileConfig>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map of unique profile names")
        }

        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut profiles = BTreeMap::new();
            while let Some((name, profile)) = map.next_entry::<String, ProfileConfig>()? {
                if profiles.insert(name.clone(), profile).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate profile '{name}'"
                    )));
                }
            }
            Ok(profiles)
        }
    }

    deserializer.deserialize_map(ProfilesVisitor)
}

fn is_default_policy(policy: &AmbiguityPolicyConfig) -> bool {
    *policy == AmbiguityPolicyConfig::default()
}

impl Config {
    pub fn from_yaml_str(input: &str) -> Result<Self, ConfigError> {
        let config: Self =
            serde_yaml::from_str(input).map_err(|error| ConfigError::parse(error.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn from_json_str(input: &str) -> Result<Self, ConfigError> {
        let config: Self =
            serde_json::from_str(input).map_err(|error| ConfigError::parse(error.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(input: &str) -> Result<Self, ConfigError> {
        Self::from_yaml_str(input)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let input =
            fs::read_to_string(path).map_err(|error| ConfigError::io(path, error.to_string()))?;
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            Self::from_json_str(&input)
        } else {
            Self::from_yaml_str(&input)
        }
    }

    pub fn load_default() -> Result<Self, ConfigError> {
        Self::load(Self::default_path())
    }

    /// XDG on Unix, APPDATA on Windows, with a conventional HOME fallback.
    pub fn default_path() -> PathBuf {
        if cfg!(windows) {
            if let Some(path) = env::var_os("APPDATA") {
                return PathBuf::from(path).join("gh-mcp-router/config.yaml");
            }
            if let Some(home) = env::var_os("USERPROFILE") {
                return PathBuf::from(home).join("AppData/Roaming/gh-mcp-router/config.yaml");
            }
        } else {
            if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
                return PathBuf::from(path).join("gh-mcp-router/config.yaml");
            }
            if let Some(home) = env::var_os("HOME") {
                return PathBuf::from(home).join(".config/gh-mcp-router/config.yaml");
            }
        }
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home).join(".config/gh-mcp-router/config.yaml");
        }
        PathBuf::from(".config/gh-mcp-router/config.yaml")
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.profiles.is_empty() {
            return Err(ConfigError::validation(
                "profiles",
                "must contain at least one profile",
            ));
        }
        for (name, profile) in &self.profiles {
            validate_name(name, &format!("profiles.{name}"))?;
            if profile.provider.trim().is_empty() {
                return Err(ConfigError::validation(
                    format!("profiles.{name}.provider"),
                    "must not be empty",
                ));
            }
            if profile.user.trim().is_empty() {
                return Err(ConfigError::validation(
                    format!("profiles.{name}.user"),
                    "must not be empty",
                ));
            }
            if let Some(host) = &profile.host {
                validate_host(host, &format!("profiles.{name}.host"))?;
            }
            if profile.gh_config_dir.is_some() {
                if let Err(error) = profile.expanded_gh_config_dir() {
                    return Err(error.at_path(format!("profiles.{name}.gh_config_dir")));
                }
            }
        }
        if let Some(default) = &self.default_profile {
            if !self.profiles.contains_key(default) {
                return Err(ConfigError::validation(
                    "default_profile",
                    format!("references missing profile '{default}'"),
                ));
            }
        }
        for (index, route) in self.routes.iter().enumerate() {
            let path = format!("routes[{index}]");
            if !self.profiles.contains_key(&route.profile) {
                return Err(ConfigError::validation(
                    format!("{path}.profile"),
                    format!("references missing profile '{}'", route.profile),
                ));
            }
            if route.host.is_none() && route.owner.is_none() && route.repository.is_none() {
                return Err(ConfigError::validation(
                    format!("{path}.match"),
                    "must contain repo, owner, or host",
                ));
            }
            if let Some(host) = &route.host {
                validate_host(host, &format!("{path}.match.host"))?;
            }
            if let Some(owner) = &route.owner {
                validate_name(owner, &format!("{path}.match.owner"))?;
            }
            if let Some(repo) = &route.repository {
                validate_repo_pattern(repo, &format!("{path}.match.repo"))?;
                if let Some(owner) = &route.owner {
                    let repo_owner = repo.split_once('/').map(|parts| parts.0);
                    if repo_owner.is_none_or(|repo_owner| !repo_owner.eq_ignore_ascii_case(owner)) {
                        return Err(ConfigError::validation(
                            format!("{path}.match"),
                            "owner conflicts with the owner in repo",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

fn validate_name(value: &str, path: &str) -> Result<(), ConfigError> {
    if value.is_empty()
        || value.len() > 128
        || value
            .chars()
            .any(|character| !(character.is_ascii_alphanumeric() || "-_".contains(character)))
    {
        return Err(ConfigError::validation(
            path,
            "must contain only ASCII letters, numbers, '-' or '_'",
        ));
    }
    Ok(())
}

fn validate_host(value: &str, path: &str) -> Result<(), ConfigError> {
    if value.is_empty()
        || value.contains('/')
        || value.contains(':')
        || value.chars().any(char::is_whitespace)
        || value.starts_with('.')
        || value.ends_with('.')
    {
        return Err(ConfigError::validation(
            path,
            "must be a hostname such as github.com",
        ));
    }
    if value != "github.com" && value.parse::<std::net::IpAddr>().is_ok() {
        return Err(ConfigError::validation(
            path,
            "must be a hostname, not an IP address",
        ));
    }
    if value.split('.').any(|part| {
        part.is_empty()
            || part.starts_with('-')
            || part.ends_with('-')
            || part
                .chars()
                .any(|character| !(character.is_ascii_alphanumeric() || character == '-'))
    }) {
        return Err(ConfigError::validation(
            path,
            "contains an invalid hostname label",
        ));
    }
    Ok(())
}

fn validate_repo_pattern(value: &str, path: &str) -> Result<(), ConfigError> {
    if value.matches('/').count() != 1
        || value.contains("**")
        || value.starts_with('/')
        || value.ends_with('/')
        || value.chars().any(char::is_whitespace)
        || value.matches('*').count() > 1
    {
        return Err(ConfigError::validation(
            path,
            "must be owner/repo, with at most one '*' glob wildcard",
        ));
    }
    if value.split('/').any(|part| {
        part.is_empty()
            || part
                .chars()
                .any(|character| !(character.is_ascii_alphanumeric() || "-_.*".contains(character)))
    }) {
        return Err(ConfigError::validation(
            path,
            "contains invalid owner or repository characters",
        ));
    }
    Ok(())
}

/// Expand only `~` and `$NAME`/`${NAME}` path references. Shell commands are
/// never evaluated.
pub fn expand_path(value: &str) -> Result<PathBuf, ConfigError> {
    if value.contains("$(") || value.contains('`') {
        return Err(ConfigError::validation(
            "gh_config_dir",
            "shell command expansion is not allowed",
        ));
    }
    let mut input = value.to_owned();
    if input == "~" || input.starts_with("~/") || input.starts_with("~\\") {
        let home = env::var_os("HOME").ok_or_else(|| {
            ConfigError::validation("gh_config_dir", "'~' requires HOME to be set")
        })?;
        input = if value == "~" {
            home.to_string_lossy().into_owned()
        } else {
            PathBuf::from(home)
                .join(&input[2..])
                .to_string_lossy()
                .into_owned()
        };
    } else if input.starts_with('~') {
        return Err(ConfigError::validation(
            "gh_config_dir",
            "only '~' or '~/...' expansion is supported",
        ));
    }
    let mut output = String::new();
    let characters: Vec<char> = input.chars().collect();
    let mut index = 0;
    while index < characters.len() {
        if characters[index] != '$' {
            output.push(characters[index]);
            index += 1;
            continue;
        }
        let (name, next) = if characters.get(index + 1) == Some(&'{') {
            let end = characters[index + 2..]
                .iter()
                .position(|character| *character == '}')
                .ok_or_else(|| {
                    ConfigError::validation("gh_config_dir", "unterminated '${...}' expansion")
                })?
                + index
                + 2;
            (
                characters[index + 2..end].iter().collect::<String>(),
                end + 1,
            )
        } else {
            let end = (index + 1..characters.len())
                .find(|position| {
                    !(characters[*position].is_ascii_alphanumeric() || characters[*position] == '_')
                })
                .unwrap_or(characters.len());
            (characters[index + 1..end].iter().collect::<String>(), end)
        };
        if name.is_empty()
            || !name
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
            || name
                .chars()
                .any(|character| !(character.is_ascii_alphanumeric() || character == '_'))
        {
            return Err(ConfigError::validation(
                "gh_config_dir",
                "invalid environment variable expansion",
            ));
        }
        let replacement = env::var_os(&name).ok_or_else(|| {
            ConfigError::validation(
                "gh_config_dir",
                format!("environment variable '{name}' is not set"),
            )
        })?;
        output.push_str(&replacement.to_string_lossy());
        index = next;
    }
    if output.contains("$(") {
        return Err(ConfigError::validation(
            "gh_config_dir",
            "shell command expansion is not allowed",
        ));
    }
    Ok(PathBuf::from(output))
}

/// Actionable configuration failure with a field/path location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    Io { path: PathBuf, message: String },
    Parse { message: String },
    Validation { path: String, message: String },
}

impl ConfigError {
    fn io(path: &Path, message: String) -> Self {
        Self::Io {
            path: path.to_owned(),
            message,
        }
    }
    fn parse(message: String) -> Self {
        Self::Parse {
            message: redact_sensitive_text(&message, &[]),
        }
    }
    fn validation(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            path: path.into(),
            message: message.into(),
        }
    }

    fn at_path(self, path: impl Into<String>) -> Self {
        match self {
            Self::Validation { message, .. } => Self::Validation {
                path: path.into(),
                message,
            },
            other => other,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(
                formatter,
                "cannot read config '{}': {message}",
                path.display()
            ),
            Self::Parse { message } => write!(formatter, "invalid configuration syntax: {message}"),
            Self::Validation { path, message } => {
                write!(formatter, "invalid configuration at '{path}': {message}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> &'static str {
        "profiles:\n  personal:\n    provider: gh\n    user: mors119\n"
    }

    #[test]
    fn profile_references_credentials_without_containing_a_token() {
        let profile = Profile::new("work", CredentialRef::new("gh", "work-account"));
        assert_eq!(profile.name, "work");
        assert_eq!(profile.credential.provider(), "gh");
    }

    #[test]
    fn valid_minimal_config_uses_safe_policy_defaults() {
        let config = Config::from_yaml_str(minimal()).unwrap();
        assert_eq!(
            config.profiles["personal"].credential_ref().name(),
            "mors119"
        );
        assert_eq!(
            config.ambiguity_policy.read,
            AmbiguityPolicy::DefaultProfile
        );
        assert_eq!(config.ambiguity_policy.write, AmbiguityPolicy::Error);
    }

    #[test]
    fn profile_credential_reference_preserves_non_secret_host_and_config_metadata() {
        let config = Config::from_yaml_str(
            "profiles:\n  work:\n    provider: gh\n    user: work-account\n    host: github.example.com\n    gh_config_dir: ~/.config/gh-work\n",
        )
        .unwrap();

        let reference = config.profiles["work"].credential_ref();

        assert_eq!(reference.provider(), "gh");
        assert_eq!(reference.name(), "work-account");
        assert_eq!(reference.host(), Some("github.example.com"));
        assert_eq!(reference.gh_config_dir(), Some("~/.config/gh-work"));
    }

    #[test]
    fn parses_profiles_and_ordered_routes() {
        let input = "profiles:\n  personal:\n    provider: gh\n    user: mors119\n  work:\n    provider: gh\n    user: work-account\n    gh_config_dir: ~/.config/gh-work\nroutes:\n  - match:\n      repo: ExampleOrg/security-*\n    profile: work\n  - match:\n      owner: ExampleOrg\n    profile: work\ndefault_profile: personal\n";
        let config = Config::from_yaml_str(input).unwrap();
        assert_eq!(
            config.routes[0].repository.as_deref(),
            Some("ExampleOrg/security-*")
        );
        assert_eq!(config.default_profile.as_deref(), Some("personal"));
    }

    #[test]
    fn rejects_duplicate_profiles() {
        let error = Config::from_yaml_str(
            "profiles:\n  work: { provider: gh, user: one }\n  work: { provider: gh, user: two }\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("profiles") || error.to_string().contains("duplicate"));
    }

    #[test]
    fn rejects_missing_profile_and_malformed_repo() {
        let error = Config::from_yaml_str("profiles:\n  work: { provider: gh, user: work }\nroutes:\n  - match: { repo: not-a-repo }\n    profile: missing\n").unwrap_err();
        assert!(error.to_string().contains("routes[0].profile"));
    }

    #[test]
    fn rejects_secret_fields() {
        let error = Config::from_yaml_str(
            "profiles:\n  work: { provider: gh, user: work, token: plaintext }\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field") || error.to_string().contains("token"));
    }

    #[test]
    fn allows_broad_rules_before_narrow_rules_for_specificity_routing() {
        let config = Config::from_yaml_str("profiles:\n  work: { provider: gh, user: work }\nroutes:\n  - match: { owner: ExampleOrg }\n    profile: work\n  - match: { repo: ExampleOrg/security }\n    profile: work\n").unwrap();
        assert_eq!(config.routes.len(), 2);
    }

    #[test]
    fn serialization_is_deterministic_and_uses_nested_match() {
        let config = Config::from_yaml_str(minimal()).unwrap();
        let first = serde_yaml::to_string(&config).unwrap();
        let second = serde_yaml::to_string(&config).unwrap();
        assert_eq!(first, second);
        assert!(first.contains("profiles:"));
    }

    #[test]
    fn expands_home_and_environment_paths_without_shell_evaluation() {
        let home = env::var("HOME").unwrap();
        let expanded = expand_path("~/gh-work/$HOME").unwrap();
        assert_eq!(
            expanded,
            PathBuf::from(&home)
                .join("gh-work")
                .join(home.trim_start_matches('/'))
        );
        let error = expand_path("$(touch /tmp/should-not-run)").unwrap_err();
        assert!(error.to_string().contains("shell command"));
    }

    #[test]
    fn parses_json_and_reports_profile_path_for_expansion_errors() {
        let config = Config::from_json_str(
            r#"{"profiles":{"work":{"provider":"gh","user":"work"}},"routes":[]}"#,
        )
        .unwrap();
        assert!(config.profiles.contains_key("work"));
        let error = Config::from_yaml_str("profiles:\n  work:\n    provider: gh\n    user: work\n    gh_config_dir: ~other/.config/gh-work\n").unwrap_err();
        assert!(error.to_string().contains("profiles.work.gh_config_dir"));
    }
}
