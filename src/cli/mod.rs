//! Command-line presentation and dispatch.

use std::{env, path::PathBuf};

use crate::config::{Config, ConfigError};

/// Run the command-line entry point, preserving the original library API.
pub fn run() {
    if let Err(error) = try_run(env::args().skip(1)) {
        eprintln!("gh-mcp-router: {error}");
    }
}

/// Parse CLI arguments and validate configuration before a service command.
///
/// `--config PATH` may appear before or after the command. The `validate`
/// command is useful for CI and `serve` deliberately loads configuration
/// before its future MCP startup path.
pub fn try_run<I, S>(args: I) -> Result<(), CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut command = None;
    let mut config_path = None;
    let mut arguments = args.into_iter().map(Into::into).peekable();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--config" => {
                config_path = Some(PathBuf::from(
                    arguments.next().ok_or(CliError::MissingConfigPath)?,
                ));
            }
            value if value.starts_with("--config=") => {
                let path = value.trim_start_matches("--config=");
                if path.is_empty() {
                    return Err(CliError::MissingConfigPath);
                }
                config_path = Some(PathBuf::from(path));
            }
            value if value.starts_with('-') => {
                return Err(CliError::UnknownArgument(value.to_owned()))
            }
            value if command.is_none() => command = Some(value.to_owned()),
            value => return Err(CliError::UnknownArgument(value.to_owned())),
        }
    }

    match command.as_deref().unwrap_or("status") {
        "validate" | "serve" => {
            let config = match config_path {
                Some(path) => Config::load(path)?,
                None => Config::load_default()?,
            };
            if command.as_deref() == Some("validate") {
                println!(
                    "configuration is valid ({} profile(s), {} route(s))",
                    config.profiles.len(),
                    config.routes.len()
                );
            } else {
                println!("configuration is valid; MCP server startup is not implemented yet");
            }
            Ok(())
        }
        "status" => {
            if config_path.is_some() {
                return Err(CliError::CommandNeedsConfig("status"));
            }
            println!("gh-mcp-router: command-line interface not implemented yet");
            Ok(())
        }
        value => Err(CliError::UnknownCommand(value.to_owned())),
    }
}

#[derive(Debug)]
pub enum CliError {
    MissingConfigPath,
    UnknownArgument(String),
    UnknownCommand(String),
    CommandNeedsConfig(&'static str),
    Config(ConfigError),
}

impl From<ConfigError> for CliError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingConfigPath => formatter.write_str("--config requires a path"),
            Self::UnknownArgument(argument) => write!(formatter, "unknown argument '{argument}'"),
            Self::UnknownCommand(command) => write!(formatter, "unknown command '{command}'"),
            Self::CommandNeedsConfig(command) => {
                write!(formatter, "--config is not used by '{command}'")
            }
            Self::Config(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CliError {}

#[cfg(test)]
mod tests {
    use super::try_run;

    #[test]
    fn invalid_explicit_config_fails_before_serve() {
        let result = try_run(["serve", "--config", "/definitely/missing/config.yaml"]);
        assert!(result.is_err());
    }
}
