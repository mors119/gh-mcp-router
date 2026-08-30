# Architecture

## Purpose

`gh-mcp-router` is an identity and routing layer in front of the official
[GitHub MCP Server](https://github.com/github/github-mcp-server). It is a
local-first component intended to let one MCP client work safely with multiple
GitHub identities.

The project does not replace the official server. GitHub capability behavior
remains upstream, while this project owns the identity-selection boundary.

## Planned flow

The v0.1 client transport is JSON-RPC 2.0 over newline-delimited stdio. The
router forwards the official upstream MCP protocol version and capabilities;
it does not advertise a separate GitHub tool schema. Profile sessions are
validated during initialization before the client receives the public tool
surface.

```text
MCP Client
    |
    v
gh-mcp-router
    |
    +-- repository context
    +-- route selection
    +-- credential selection
    +-- profile-isolated upstream session
    |
    v
Official GitHub MCP Server
    |
    v
GitHub
```

The configuration boundary parses and validates profiles and ordered route rules
before later services start. Credential providers resolve account references,
while discovery, routing evaluation, upstream sessions, and MCP forwarding
remain separate concerns. The MCP proxy forwards upstream lifecycle and
capability messages, derives one public tool surface after validating every
profile's tool schema, and routes only repository-scoped `tools/call` requests.

## Module responsibilities

| Module | Responsibility | Explicit non-responsibilities |
| --- | --- | --- |
| `config` | Serializable profile/route models, parsing, path expansion, and validation | Credential retrieval or routing evaluation |
| `context` | Request-scoped repository resolution, normalization, and source metadata | Credential retrieval, routing policy, GitHub API calls |
| `routing` | Pure routing decisions, operation classification, and safe fallback policy | Credential retrieval, API calls, CLI state |
| `credentials` | Credential references, GitHub CLI account discovery, and token retrieval | Repository/profile routing or GitHub API calls |
| `security` | Secret-safe formatting and `SecretString` cleanup | Full lifecycle hardening beyond value cleanup |
| `mcp` | Client-facing protocol/proxy boundary | Repository routing policy |
| `upstream` | Official GitHub MCP session boundary | GitHub API/tool implementation |
| `cli` | Presentation and command dispatch | The routing engine itself |

The project remains a single crate until a real boundary justifies splitting
it. Domain values are intentionally small and can be constructed independently
for unit tests. Profiles refer to `CredentialRef` values; no domain type needs
a plaintext token.

## Upstream boundary and non-goals

The router intentionally does **not** implement or replace the following:

- GitHub Issues API behavior
- Pull Request API behavior
- Actions behavior
- Releases behavior
- repository business behavior
- a replacement GitHub MCP toolset

Those capabilities remain responsibilities of the official GitHub MCP Server.
The upstream process/session boundary forwards or isolates those capabilities
rather than duplicate their definitions. It maintains one lazily started stdio
child per profile, binds each child permanently to its credential reference,
passes the resolved credential only in the child environment, and restarts a
failed child on the next request without retrying the failed message.

## Security principles

- Raw credentials do not belong in project configuration.
- Routing must not depend on global `gh auth switch` state.
- There is no global mutable current-account state.
- Credential retrieval and routing decisions remain separate concerns.
- Ordinary `Debug` and `Display` formatting of secret values must redact them;
  subprocess output is never included in provider errors.
- Ambiguous write operations fail closed rather than guess an identity.
- Repository context prefers explicit tool arguments, then repository URLs,
  request-scoped MCP/workspace roots, read-only Git remotes, and finally an
  explicit configured context.
- Default-profile fallback is allowed for reads by default, while write
  fallback requires `ambiguity_policy.write: default_profile`; unknown tools
  never use a default fallback.

These are design principles. The GitHub CLI provider performs account discovery
and on-demand token retrieval through explicit subprocess arguments and
profile-specific `GH_CONFIG_DIR` values. The upstream launcher uses the
configured `github-mcp-server` executable or PATH lookup and never inherits
upstream stderr. Repository inference, routing evaluation, and MCP proxying
remain separate concerns.

## Planned feature ownership

The next features add behavior behind the boundaries established here:

- configuration parsing and validation (`#3`, implemented)
- GitHub CLI credential discovery (`#4`)
- deterministic routing evaluation (`#5`, implemented)
- repository context discovery and safe write policy (`#6`, implemented)
- profile-isolated upstream sessions (`#7`, implemented)
- MCP request forwarding (`#8`, implemented)
- complete CLI workflows (`#9`)
- deeper secret, concurrency, logging, and process hardening (`#10`)
