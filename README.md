# gh-mcp-router

Route GitHub MCP requests to the right GitHub identity automatically.

`gh-mcp-router` is a local-first multi-account routing layer for the official [GitHub MCP Server](https://github.com/github/github-mcp-server).

It is designed for developers who work across multiple GitHub identities — personal accounts, work accounts, organizations, client accounts, bot accounts, or permission-scoped credentials — but want to expose **one GitHub MCP tool surface** to their AI tools.

> Project status: early development.
> The v0.1 architecture and roadmap are defined, but the router is not yet ready for production use.

## The problem

The GitHub MCP Server normally runs with one authentication context.

That works well until your development environment looks like this:

```text
Personal repositories
    → personal GitHub account

Company repositories
    → work GitHub account

Client repositories
    → client GitHub account
```

Without an account-aware routing layer, users generally have to:

- manually switch authentication,
- restart the MCP server,
- configure multiple GitHub MCP servers,
- expose duplicated tool namespaces,
- or use one broadly privileged token.

For example:

```text
github-personal.create_issue
github-work.create_issue
github-client.create_issue
```

`gh-mcp-router` aims to make this unnecessary.

Instead:

```text
                    ┌── personal identity
                    │
MCP Client ──> gh-mcp-router ──┼── work identity
                    │
                    └── client identity
                             │
                             v
                    Official GitHub MCP
```

The client continues to see one GitHub tool surface.

The router determines which identity should handle each request.

## Why this exists

Multi-account support is also being discussed upstream in the official GitHub MCP project:

- [github/github-mcp-server#1940 — Add multi-account support for client-side account switching](https://github.com/github/github-mcp-server/issues/1940)
- [github/github-mcp-server#2050 — Auto-select token based on repository owner](https://github.com/github/github-mcp-server/issues/2050)

`gh-mcp-router` explores this problem as a separate local routing layer rather than reimplementing GitHub MCP functionality.

## Design goals

### One GitHub tool surface

The MCP client should not need to know which account is being used.

```text
create_issue(owner="PersonalUser", repo="project")
```

and:

```text
create_issue(owner="CompanyOrg", repo="backend")
```

should use the appropriate identities automatically.

### Repository-aware routing

Routing is based on repository context.

Planned matching levels are:

```text
exact repository
    ↓
repository pattern
    ↓
owner / organization
    ↓
GitHub host
    ↓
explicit default policy
```

For example:

```yaml
profiles:
  personal:
    provider: gh
    user: personal-user

  work:
    provider: gh
    user: work-user

routes:
  - match:
      repo: ExampleOrg/security-*
    profile: work

  - match:
      owner: ExampleOrg
    profile: work

  - match:
      owner: personal-user
    profile: personal
```

No GitHub access token is stored in this configuration.

### No global account switching

The router must not depend on:

```bash
gh auth switch
```

for request routing.

Global account switching becomes dangerous when several agents or concurrent requests are running:

```text
Request A
    → switch to personal

Request B
    → switch to work

Request A
    → accidentally continues as work
```

Instead, each profile should resolve its own credential without changing global GitHub CLI state.

### Local-first credential handling

The initial credential provider will integrate with the GitHub CLI.

Conceptually:

```text
gh-mcp-router
      │
      ├── profile: personal
      │      └── GitHub CLI credential
      │
      └── profile: work
             └── GitHub CLI credential
```

The router configuration stores only identity references and routing rules.

Raw credentials must not be persisted by `gh-mcp-router`.

### Fail closed for unsafe writes

A router handling multiple identities must not guess when performing destructive or state-changing operations.

For example, if a write request cannot be safely mapped to one identity:

```text
Cannot safely select a GitHub identity.

Repository context is missing or ambiguous.
```

The operation should fail instead of silently using a default account.

Read operations may support explicitly configured fallback policies, but routing decisions must remain observable.

### Keep GitHub MCP upstream

`gh-mcp-router` is not intended to implement GitHub Issues, Pull Requests, Actions, repositories, releases, or other GitHub APIs itself.

The architecture is:

```text
MCP Client
    │
    v
gh-mcp-router
    │
    ├── routing
    ├── repository context
    ├── credential selection
    └── upstream session management
              │
              v
     Official GitHub MCP Server
              │
              v
            GitHub
```

GitHub behavior remains owned by the official GitHub MCP Server.

## Planned v0.1 architecture

The initial implementation is being developed in Rust.

The intended module boundaries are approximately:

```text
src/
├── main.rs
├── config/
├── routing/
├── credentials/
├── context/
├── mcp/
├── upstream/
├── cli/
└── security/
```

The project will begin as a single crate.

It should only be split into multiple crates when real architectural boundaries justify the additional complexity.

## Profile-isolated upstream sessions

The preferred v0.1 local architecture is one isolated official GitHub MCP session per credential profile.

```text
                       ┌─────────────────────┐
                       │ personal credential │
                       └──────────┬──────────┘
                                  v
                           GitHub MCP A
                          /
MCP Client → Router -----+
                          \
                                  v
                           GitHub MCP B
                       ┌──────────┴──────────┐
                       │   work credential   │
                       └─────────────────────┘
```

A session created for one profile must never be reused for another profile.

HTTP upstream routing with request-scoped authorization may be explored later where appropriate, but it is not required for the first implementation.

## Repository context

Not every MCP call represents a repository in exactly the same way.

The router will attempt to normalize repository context from the most explicit available source.

Planned priority:

```text
explicit owner + repo
        ↓
repository full name
        ↓
GitHub repository URL
        ↓
MCP workspace/root
        ↓
local Git remote
        ↓
explicit configured fallback
```

For example:

```text
git@github.com:ExampleOrg/backend.git
```

becomes:

```text
host:  github.com
owner: ExampleOrg
repo:  backend
```

This normalized context is evaluated by the pure routing engine. It returns the
selected profile, the winning rule and specificity, or an explicit no-match or
ambiguous result.

## Planned CLI

The v0.1 CLI is expected to provide:

```bash
gh-mcp-router init
gh-mcp-router profiles
gh-mcp-router route <owner/repo>
gh-mcp-router explain <owner/repo>
gh-mcp-router doctor
gh-mcp-router validate --config PATH
gh-mcp-router serve
```

## Configuration

The default configuration is `$XDG_CONFIG_HOME/gh-mcp-router/config.yaml` on
Unix-like systems, or `~/.config/gh-mcp-router/config.yaml` when
`XDG_CONFIG_HOME` is not set. Windows uses `%APPDATA%/gh-mcp-router/config.yaml`.
Use `--config PATH` to select another YAML or JSON file.

The complete v0.1 schema is checked in at
[`examples/config.example.yaml`](examples/config.example.yaml). It supports
named credential references, optional per-profile `GH_CONFIG_DIR` and host
isolation, exact/glob repository, owner, and host routes, a default profile,
and separate read/write ambiguity policies. Routes are evaluated by
`exact repository > repository glob > owner > host > default`; conflicting
equally specific profiles produce an explicit ambiguous result. Unknown fields
are rejected, including token/PAT fields. GitHub host, owner, and repository
matching is case-insensitive; explanations retain the configured spelling. If
equally specific matching rules select the same profile, the first configured
rule is reported as the stable tie-breaker.

The remaining commands are part of the v0.1 roadmap and are not available yet;
`validate` is available now for checking configuration files.

### `init`

Create an initial profile/routing configuration using locally authenticated GitHub accounts.

```bash
gh-mcp-router init
```

The command must never copy raw GitHub credentials into the generated configuration.

### `profiles`

Inspect configured identities without exposing credentials.

Expected style:

```text
PROFILE    USER          HOST        PROVIDER   AUTH
personal   personal-user github.com  gh         ok
work       work-user     github.com  gh         ok
```

### `route`

Test routing before connecting an MCP client.

```bash
gh-mcp-router route ExampleOrg/backend
```

Expected style:

```text
Repository: ExampleOrg/backend
Profile:    work
Rule:       owner:ExampleOrg
Fallback:   no
```

### `explain`

Show why a routing rule was selected.

```bash
gh-mcp-router explain ExampleOrg/backend
```

This is intended to make account routing deterministic and debuggable instead of becoming hidden authentication magic.

### `doctor`

Diagnose common configuration problems.

Planned checks include:

```text
configuration
GitHub CLI
GitHub identities
credential availability
route conflicts
GitHub MCP availability
upstream startup
```

Credential values must never appear in diagnostic output.

### `serve`

Start the MCP router.

```bash
gh-mcp-router serve
```

MCP clients will eventually point to this process instead of launching the GitHub MCP Server directly.

## Security model

`gh-mcp-router` handles access to credentials and therefore treats credential isolation as a core correctness requirement.

The following rules apply to the v0.1 design:

```text
Secrets never belong in router configuration.

Secrets must not appear in logs.

Secrets must not appear in CLI diagnostics.

Secrets must not appear in MCP errors.

Secrets must not appear in test snapshots.

One profile must never receive another profile's credential.

Routing must never rely on a mutable global current account.
```

The router is intended to be a **local developer tool**.

It is not designed as a public multi-tenant credential broker.

## Concurrency

Supporting multiple identities is only useful if concurrent agent workloads remain safe.

This must work:

```text
Agent A
    → PersonalOrg/project
    → personal profile

Agent B
    → CompanyOrg/backend
    → work profile

Agent C
    → PersonalOrg/another-project
    → personal profile
```

without:

```text
auth switch
restart
shared current account
credential cross-talk
```

The test suite will include concurrent mixed-profile requests specifically to catch these failures.

## Testing strategy

Most automated tests should not require real GitHub credentials.

The project will use:

```text
fake gh command runner
fake credentials
fake GitHub MCP upstream
temporary Git repositories
temporary GH_CONFIG_DIR fixtures
captured logs/output
```

This allows pull-request CI to verify multi-account routing safely.

Real GitHub testing will be opt-in and should use disposable repositories for write operations.

The final v0.1 release gate will verify:

- two real GitHub identities,
- repository-based routing,
- read and write operations,
- concurrent requests,
- fail-closed behavior,
- credential isolation,
- MCP client compatibility,
- release artifacts,
- and documentation reproducibility.

## Roadmap

The current implementation roadmap is tracked by:

[EPIC #1 — v0.1 Multi-account GitHub MCP routing](https://github.com/mors119/gh-mcp-router/issues/1)

Major phases:

```text
Foundation
    ↓
Configuration
    ↓
Credential provider + Routing engine
    ↓
Repository context
    ↓
Profile-isolated GitHub MCP sessions
    ↓
MCP request routing
    ↓
CLI + Security hardening
    ↓
Integration testing + Documentation
    ↓
Packaging / Release
    ↓
Final E2E validation
```

The final release gate is:

[Issue #14 — v0.1 end-to-end multi-account routing verification](https://github.com/mors119/gh-mcp-router/issues/14)

## Non-goals for v0.1

The first release intentionally does not aim to provide:

- a replacement GitHub MCP Server,
- GitLab or Bitbucket routing,
- a general-purpose MCP gateway,
- cloud credential synchronization,
- a public multi-tenant service,
- automatic Git commit identity switching,
- automatic SSH key switching,
- automatic GPG signing identity switching,
- or a graphical interface.

The goal is narrower:

> Make multiple GitHub identities feel like one safe GitHub MCP connection.

## Related projects and references

Official GitHub MCP Server:

https://github.com/github/github-mcp-server

Relevant upstream feature discussions:

https://github.com/github/github-mcp-server/issues/1940

https://github.com/github/github-mcp-server/issues/2050

GitHub CLI:

https://cli.github.com/

Model Context Protocol:

https://modelcontextprotocol.io/

## Contributing

The project is currently in its initial implementation phase.

Before implementing a feature, check the v0.1 Epic and the Feature issue that owns the behavior.

Changes should:

- keep the router focused on identity and request routing,
- avoid duplicating official GitHub MCP functionality,
- preserve deterministic routing,
- keep credentials out of persistent configuration,
- add tests for changed behavior,
- and avoid unrelated architectural expansion.

Early feedback on multi-account workflows, organization routing, credential isolation, and MCP client compatibility is welcome.
