# Contributing to gh-mcp-router

Thank you for helping improve `gh-mcp-router`. The project is an Apache-2.0
licensed local routing layer for the official GitHub MCP Server.

## Before you start

For a planned feature or behavior change, check the [open issues](https://github.com/mors119/gh-mcp-router/issues)
and the v0.1 roadmap. Please open an issue first for changes that affect the
architecture, routing policy, credential handling, or supported platforms.

Keep contributions focused on:

- repository context detection and deterministic profile selection;
- credential isolation and safe diagnostics;
- profile-isolated official GitHub MCP sessions; and
- MCP forwarding and CLI behavior around those boundaries.

The project does not reimplement GitHub APIs or the official GitHub MCP
Server's tools.

## Development setup

Install the stable Rust toolchain. The normal test suite does not require real
GitHub credentials, `gh` authentication, or network access:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build
```

The live multi-profile test is opt-in and documented in
[`docs/testing.md`](docs/testing.md). Never put real tokens in configuration,
fixtures, logs, or pull requests.

## Pull requests

Create a focused branch from `main`. Explain the user-visible behavior and
include tests and documentation when behavior changes. Before requesting
review, confirm that the local validation commands above pass and that no
credentials or generated build artifacts are included.

Changes that touch routing, context, credentials, upstream sessions, MCP
forwarding, or security should preserve the module boundaries described in
[`docs/architecture.md`](docs/architecture.md). In particular:

- routing decisions must be deterministic and explainable;
- unsafe writes must fail closed;
- credentials must remain profile-scoped and redacted from diagnostics; and
- concurrent profiles must not share mutable account or token state.

Use the pull request template to call out security or compatibility impact.
Maintainers may ask for an issue or design discussion before larger changes.

## Commit and review expectations

Use a concise imperative commit subject, optionally prefixed by the affected
area (for example, `docs: clarify release support`). Keep unrelated cleanup in
a separate change. Reviews prioritize correctness, credential safety,
backward compatibility, and tests over style preferences.

By submitting a contribution, you agree that it is provided under the
Apache License 2.0, as described in [`LICENSE`](LICENSE).
