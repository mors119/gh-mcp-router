# AGENTS.md

This file defines repository-wide rules for coding agents working on `gh-mcp-router`.

## Project Purpose

`gh-mcp-router` is a local-first routing layer for using multiple GitHub identities through the official GitHub MCP Server.

Keep this project focused on:

* repository context detection,
* profile selection,
* credential isolation,
* MCP request routing,
* diagnostics for routing and authentication.

Do not turn it into a replacement GitHub MCP server or a general-purpose MCP gateway without an explicit architectural decision.

## Architecture Rules

Keep responsibilities separated.

* `config`: profiles and routing configuration
* `routing`: deterministic repository-to-profile selection
* `credentials`: credential resolution for a selected profile
* `context`: repository, owner, and host resolution
* `upstream`: lifecycle of profile-isolated GitHub MCP sessions
* `mcp`: MCP protocol handling and request forwarding
* `cli`: input parsing, command dispatch, and output formatting
* `security`: secret handling, redaction, and safe diagnostics

Keep reusable behavior out of CLI handlers.

Do not duplicate routing, credential, or repository-context logic across modules.

Use the official GitHub MCP Server for GitHub capabilities instead of reimplementing GitHub Issues, Pull Requests, Actions, Releases, or repository APIs.

## Routing Rules

Routing must be deterministic and explainable.

Prefer explicit repository context over ambient process state.

Do not silently guess an identity when routing is ambiguous.

Write or otherwise state-changing operations must fail closed when a safe profile cannot be determined.

Do not silently rewrite repository owners, names, or hosts to make a route succeed.

## Credential Safety

Never:

* commit or persist raw GitHub credentials,
* log tokens or Authorization headers,
* expose credentials through CLI or MCP responses,
* place credentials in visible process arguments,
* use a mutable global current account,
* route requests by calling `gh auth switch`.

Credentials must remain associated with the profile for which they were resolved.

Each upstream GitHub MCP session must belong to exactly one profile and must never be reused for another identity.

Secret-bearing values must be redacted from normal `Debug`, `Display`, error, and diagnostic output.

## Concurrency

Multiple profiles must be safe to use concurrently.

Do not introduce shared mutable state equivalent to:

```text
current_account
current_profile
current_token
```

A request must retain its routing decision for its complete lifecycle.

Failure, restart, or cancellation of one profile should not corrupt unrelated profile sessions.

## Change Rules

* Work from a GitHub Issue when one exists.
* Keep one clear purpose per branch and Pull Request.
* Implement the smallest complete change that satisfies the requirement.
* Add or update tests when behavior changes.
* Do not delete or weaken tests merely to make validation pass.
* Avoid unrelated refactoring.
* Do not prematurely build abstractions for GitLab, Bitbucket, cloud credential storage, or other future integrations.
* Record out-of-scope findings as follow-up Issue candidates.

Prefer extending existing abstractions over creating parallel implementations.

## Dependencies

Add dependencies only when they provide immediate value.

Avoid speculative infrastructure and unnecessary framework layers.

Keep the project simple enough that routing and security behavior can be understood and tested directly.

## Validation

Before considering a code change complete, run:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build
```

Run additional focused tests when the changed behavior requires them.

Tests should not depend on a developer's real GitHub credentials unless explicitly marked as opt-in integration tests.

Prefer fake credentials, fake GitHub CLI execution, fake MCP upstreams, and temporary repositories for automated tests.

## Git Safety

Do not perform destructive or history-rewriting actions without explicit user approval.

This includes:

```bash
git reset --hard
git clean -fd
git clean -fdx
git push --force
git push --force-with-lease
git branch -D
```

Do not commit:

* access tokens,
* credentials,
* secret environment files,
* local GitHub CLI configuration,
* build outputs,
* editor temporary files,
* unrelated generated files.

## Documentation

Documentation must distinguish clearly between:

* implemented behavior,
* experimental behavior,
* planned behavior,
* unsupported behavior.

Never include real credentials in examples.

## Guiding Principle

When trade-offs arise, prefer:

```text
credential isolation over convenience
deterministic routing over implicit guessing
explicit failure over unsafe fallback
small maintainable changes over speculative architecture
official GitHub MCP behavior over local reimplementation
```
