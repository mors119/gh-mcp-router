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
surface. If the client advertises the roots capability, the proxy requests
`roots/list` after initialization and uses exactly one returned file root for
workspace context; multiple roots are treated as ambiguous.

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
remain separate concerns. The MCP proxy completes each upstream handshake
before discovery, forwards lifecycle and capability messages, derives one
public tool surface after validating every profile's tool schema, and routes
only repository-scoped `tools/call` requests.

## Module responsibilities

| Module | Responsibility | Explicit non-responsibilities |
| --- | --- | --- |
| `config` | Serializable profile/route models, parsing, path expansion, and validation | Credential retrieval or routing evaluation |
| `context` | Request-scoped repository resolution, normalization, and source metadata | Credential retrieval, routing policy, GitHub API calls |
| `routing` | Pure routing decisions, operation classification, and safe fallback policy | Credential retrieval, API calls, CLI state |
| `credentials` | Credential references, GitHub CLI account discovery, and token retrieval | Repository/profile routing or GitHub API calls |
| `security` | Secret-safe formatting, cancellation, child-environment allowlisting, and redacting observability | GitHub credential retrieval or public multi-tenant secret brokering |
| `mcp` | Client-facing protocol/proxy boundary | Repository routing policy |
| `upstream` | Official GitHub MCP session boundary | GitHub API/tool implementation |
| `cli` | CLI setup, inspection, diagnostics, serving, and command dispatch | The routing engine itself |

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
configured `github-mcp-server` executable or PATH lookup, builds an explicit
minimal child environment, and never inherits upstream stderr. Resolved
credentials are retained only by their profile-owned session guard so known
values can be scrubbed from upstream responses before they cross the MCP
boundary. Repository inference, routing evaluation, and MCP proxying remain
separate concerns.

## Security hardening

`SecretString` is an `Arc`-backed redacted wrapper. Cloning it shares the
existing allocation rather than copying token bytes, and the allocation is
zeroized when released. This is best-effort protection: Rust allocators,
subprocess creation, and crash handling can create copies outside the wrapper's
control, so the router does not promise a universal in-memory lifetime
guarantee.

The process boundary starts with a minimal allowlist (`PATH`, home, locale,
temporary-directory, and platform runtime values), then adds only the
profile-specific GitHub token, host, and `GH_CONFIG_DIR`. No parent environment
mutation or command-line token injection is used. Upstream stderr is discarded
at the process boundary; `gh` output is held in zeroized buffers and provider
errors contain categories and non-secret profile references only.

Concurrent requests carry their selected profile and cancellation token for
their full lifecycle. A per-profile request lock serializes writes to one
stdio session, while different profiles remain concurrent. Session startup is
serialized per profile, session identity is immutable, and a failed child is
discarded without retrying the request. Router shutdown waits for active
request handlers before closing children, and cancellation never shuts down a
different profile.

The default log policy is `info`; `debug` and `trace` add only the typed route
metadata fields `request_id`, `operation_class`, `repository`, `profile`,
`matched_rule`, `upstream_session_id`, and `result_status`. Every level passes
through token-pattern redaction. Raw MCP payloads, subprocess output,
authorization headers, and credentials are not logged.

## Planned feature ownership

The next features add behavior behind the boundaries established here:

- configuration parsing and validation (`#3`, implemented)
- GitHub CLI credential discovery (`#4`)
- deterministic routing evaluation (`#5`, implemented)
- repository context discovery and safe write policy (`#6`, implemented)
- profile-isolated upstream sessions (`#7`, implemented)
- MCP request forwarding (`#8`, implemented)
- complete CLI workflows (`#9`, implemented)
- deeper secret, concurrency, logging, and process hardening (`#10`)

## CLI boundary

The CLI loads and validates configuration before invoking application
behavior. route and explain construct a repository context and call the pure
routing evaluator; they do not resolve credentials. profiles and doctor use
the credential provider's non-secret account checks. serve constructs the MCP
router with the configured upstream executable and starts the client-facing
stdio transport. JSON output is available for inspection commands, and
diagnostic failures use stable command-line exit categories.
