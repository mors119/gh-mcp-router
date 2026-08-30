# Testing

The normal test suite is credential-free. It uses fake credential providers,
fake `gh` command runners, fake official-GitHub-MCP-compatible upstream
processes, and disposable Git repositories.

Run the complete local matrix with:

```bash
cargo test --all-features
```

The test organization is:

- unit tests live beside their owning module under `src/`;
- `tests/multi_profile_integration.rs` exercises routing, context resolution,
  credential isolation, MCP lifecycle forwarding, restart and shutdown,
  concurrency, cancellation, upstream errors, schema-compatible tool
  discovery, and secret-leak regression checks;
- `tests/live_multi_profile.rs` is an ignored, opt-in smoke test for real
  GitHub CLI credentials and the official upstream server.

The fake integration matrix is also runnable by itself:

```bash
cargo test --all-features --test multi_profile_integration
```

## Opt-in live smoke test

Live testing is never required by CI. To run the non-destructive live
initialize and tool-discovery check, provide an explicit config containing at
least two authenticated profiles and set:

```bash
GH_MCP_ROUTER_LIVE_TEST=1 \
GH_MCP_ROUTER_LIVE_CONFIG=/absolute/path/to/live-config.yaml \
cargo test --all-features --test live_multi_profile -- --ignored
```

Use `GH_MCP_ROUTER_LIVE_UPSTREAM=/absolute/path/to/github-mcp-server` when
the official upstream executable is not on `PATH`. The live config contains
profile and route metadata only; credentials continue to come from each
profile's local `gh` authentication context. The live test performs no issue,
pull-request, or repository write.

Do not put tokens in the live config or in environment variables used by the
router. Any destructive live fixture would need a separate, disposable
repository-specific test before it could be added.
