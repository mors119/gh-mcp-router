use std::env;

use gh_mcp_router::{
    config::Config,
    credentials::GhCliCredentialProvider,
    mcp::McpRouter,
    upstream::{ProcessUpstreamLauncher, UpstreamConfig},
};
use serde_json::Value;

/// Opt-in live smoke test. It performs only initialize and tools/list, so it
/// does not write to a GitHub repository.
#[test]
#[ignore = "requires GH_MCP_ROUTER_LIVE_TEST=1 and an explicit live config"]
fn live_two_profile_mcp_handshake_is_available() {
    assert_eq!(
        env::var("GH_MCP_ROUTER_LIVE_TEST").as_deref(),
        Ok("1"),
        "set GH_MCP_ROUTER_LIVE_TEST=1 to opt in"
    );
    let config_path = env::var("GH_MCP_ROUTER_LIVE_CONFIG")
        .expect("GH_MCP_ROUTER_LIVE_CONFIG must point to an explicit config file");
    let config = Config::load(&config_path).expect("live config must parse and validate");
    assert!(
        config.profiles.len() >= 2,
        "live config must exercise at least two profiles"
    );

    let upstream = env::var_os("GH_MCP_ROUTER_LIVE_UPSTREAM")
        .map(|binary| UpstreamConfig::default().with_binary(binary))
        .unwrap_or_default();
    let router = McpRouter::new(
        config,
        GhCliCredentialProvider::new(),
        ProcessUpstreamLauncher,
        upstream,
    );
    let initialize = router
        .handle_message(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        )
        .expect("live initialize must return a response");
    let initialize: Value = serde_json::from_str(&initialize).unwrap();
    assert!(
        initialize["result"].is_object(),
        "live initialize failed: {initialize}"
    );
    assert!(router
        .handle_message(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .is_none());
    let tools = router
        .handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#)
        .expect("live tools/list must return a response");
    let tools: Value = serde_json::from_str(&tools).unwrap();
    assert!(
        tools["result"]["tools"].is_array(),
        "live tools/list failed: {tools}"
    );
    router.shutdown();
}
