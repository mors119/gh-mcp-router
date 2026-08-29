//! Architectural foundation for `gh-mcp-router`.
//!
//! The crate is intentionally small. These modules define ownership boundaries
//! for later features without performing discovery, routing, or MCP work yet.

pub mod cli;
pub mod config;
pub mod context;
pub mod credentials;
pub mod mcp;
pub mod routing;
pub mod security;
pub mod upstream;
