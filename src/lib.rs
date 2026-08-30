//! Architectural foundation for `gh-mcp-router`.
//!
//! The crate is intentionally small. These modules define ownership boundaries
//! while keeping repository routing separate from discovery, credentials, and
//! MCP work.

pub mod cli;
pub mod config;
pub mod context;
pub mod credentials;
pub mod mcp;
pub mod routing;
pub mod security;
pub mod upstream;
