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

The initial credential provider integrates with the GitHub CLI.

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

### Repository context and write safety

The router resolves repository context in this order, stopping at the first
valid source:

1. explicit `owner` and `repo` tool arguments;
2. a full `owner/repository` argument;
3. a GitHub repository URL;
4. the request-scoped MCP root or workspace root's Git `origin` remote; and
5. an explicitly configured default context.

Common HTTPS, SCP-style SSH, and `ssh://` remotes are normalized to
`host`, `owner`, `repository`, and a recorded source. Context discovery is
read-only and never calls GitHub APIs. An explicit request wins over conflicting
ambient context.

Known read and write tools use separate operation classes. Unknown tools are
treated conservatively. Ambiguous routes always fail closed; writes cannot use
the default profile unless `ambiguity_policy.write: default_profile` is
explicitly configured. Read fallback decisions record that the default profile
was used. Missing or ambiguous repository context produces an actionable error
instead of guessing an identity.

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

The v0.1 local architecture uses one isolated official GitHub MCP stdio
session per credential profile. The router starts sessions lazily and caches a
healthy session for reuse. The default executable is `github-mcp-server stdio`;
an explicit executable path or startup arguments can be supplied by the caller,
and a missing executable is reported clearly rather than downloaded.

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
Each session is permanently bound to its profile's non-secret credential
reference. The resolved credential is passed only as
`GITHUB_PERSONAL_ACCESS_TOKEN` in that child process's environment, alongside
profile-specific host/config values when configured. It is never put in
arguments or the router's global environment. Failed sessions are discarded
without retrying the request and restarted on the next request; shutdown closes
and terminates all child sessions.

The upstream boundary forwards newline-delimited messages and does not define
GitHub tools itself.

## MCP request routing

Issue #8 adds the client-facing MCP proxy. v0.1 supports JSON-RPC 2.0 over
newline-delimited stdio and forwards the upstream `initialize` response,
capabilities, tool definitions, tool arguments, results, and errors. The
client sends one `tools/list` surface; profile-specific tool names are never
created. During initialization the router obtains tool metadata from every
configured profile and rejects incompatible upstream tool schemas.

`tools/call` requests are classified and routed from their original arguments.
Arguments are not rewritten. Explicit repository context is routed through the
configured precedence and safe write policy. A repository-free read may use the
explicitly configured `default_profile`; writes and unknown operations without
deterministic context fail closed. `resources/*`, `prompts/*`, `ping`, and
other supported protocol metadata requests are forwarded through the primary
validated session. When the client advertises roots support, the proxy
requests `roots/list` after initialization and uses a single returned file
root for workspace/Git-remote context; multiple roots remain intentionally
ambiguous. The proxy completes each upstream handshake before metadata
discovery, then consumes the client's `initialized` notification. Cancellation,
shutdown, and exit lifecycle messages are handled without exposing credentials.

The proxy can be used as a library through `McpRouter::handle_message` or its
newline-delimited `serve_stdio` entry point. The command-line `serve` wiring
remains part of the later CLI workflow feature.

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

## CLI

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

All commands above are available in the v0.1 CLI. `validate` remains
available for checking configuration files in automation.

### `init`

Create an initial profile/routing configuration using locally authenticated GitHub accounts.

```bash
gh-mcp-router init
```

The command never copies raw GitHub credentials into the generated
configuration. It refuses to replace an existing file unless `--force`
is supplied, and repeated `--profile USER=NAME` options assign friendly
names.

### `profiles`

Inspect configured identities without exposing credentials.

Human-readable output:

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

Human-readable output:

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

Checks include:

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

MCP clients can point to this process instead of launching the GitHub MCP Server
directly.

## CLI workflow

The binary provides setup, inspection, diagnostics, and serving commands:

    gh-mcp-router init
    gh-mcp-router profiles
    gh-mcp-router route OWNER/REPO
    gh-mcp-router explain OWNER/REPO
    gh-mcp-router doctor
    gh-mcp-router serve

Use --config PATH with any command to select a configuration file. The
default is ~/.config/gh-mcp-router/config.yaml on Unix-like systems. init
discovers authenticated accounts through gh auth status, writes only profile
references, and refuses to replace an existing file unless --force is
provided. Friendly names can be assigned with repeated --profile USER=NAME
options:

    gh-mcp-router init --profile mors119=personal --profile work-account=work

The generated file contains no tokens. Add route rules to it, then inspect a
decision before connecting an MCP client:

    gh-mcp-router route ExampleOrg/backend
    gh-mcp-router explain ExampleOrg/backend --json
    gh-mcp-router profiles --json
    gh-mcp-router doctor --json

route and explain do not retrieve credentials. profiles verifies account
status without printing credentials. doctor checks configuration, gh,
configured accounts and config directories, obvious route conflicts, the
github-mcp-server executable, credential availability, and upstream process
startup. Diagnostic failures use non-zero exit codes and never include token
values.

For an MCP client, configure one stdio server command:

    {
      "mcpServers": {
        "github": {
          "command": "gh-mcp-router",
          "args": ["serve", "--config", "/absolute/path/to/config.yaml"]
        }
      }
    }

The router uses newline-delimited JSON-RPC 2.0 over stdio and preserves the
official GitHub MCP tool surface.

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

### v0.1 trust and process boundaries

The local user and configured MCP client are trusted. Profile credentials are
retrieved locally through `gh`; the official GitHub MCP child process receives
only the credential assigned to that profile. Router configuration contains
identity and routing metadata, never raw PATs, OAuth tokens, or GitHub App
private keys.

Child environments are built from an explicit small allowlist of benign
runtime variables. `GITHUB_PERSONAL_ACCESS_TOKEN`, `GITHUB_HOST`, and an
optional profile-specific `GH_CONFIG_DIR` are added only to the corresponding
child. The parent environment is never changed and `gh auth switch` is never
used.

`SecretString` uses shared ownership to avoid unnecessary byte copies, formats
as `[REDACTED]`, and zeroizes its owned buffer when the last reference is
dropped. Rust and the operating system may make unavoidable copies during
allocation, subprocess creation, paging, or crash handling; v0.1 therefore
documents this as best-effort in-memory protection rather than a guarantee of
instant memory erasure.

The router logs only typed metadata: `request_id`, `operation_class`,
`repository`, `profile`, `matched_rule`, `upstream_session_id`, and
`result_status`. Set `GH_MCP_ROUTER_LOG_LEVEL` to `error`, `warn`, `info`,
`debug`, or `trace`; higher verbosity changes metadata volume only and never
weakens redaction. Raw MCP messages, subprocess output, authorization headers,
and credentials are not log fields. Upstream responses and JSON-RPC error text
are scrubbed for known and token-shaped values before they reach the client.

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

The test suite includes concurrent mixed-profile requests specifically to catch these failures.

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
