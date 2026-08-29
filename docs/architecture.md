# Architecture

## Purpose

`gh-mcp-router` is an identity and routing layer in front of the official
[GitHub MCP Server](https://github.com/github/github-mcp-server). It is a
local-first component intended to let one MCP client work safely with multiple
GitHub identities.

The project does not replace the official server. GitHub capability behavior
remains upstream, while this project owns the identity-selection boundary.

## Planned flow

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

The configuration boundary now parses and validates profiles and ordered route
rules before later services start. Discovery, routing evaluation, credential
providers, upstream sessions, and MCP forwarding remain separate concerns.

## Module responsibilities

| Module | Responsibility | Explicit non-responsibilities |
| --- | --- | --- |
| `config` | Serializable profile/route models, parsing, path expansion, and validation | Credential retrieval or routing evaluation |
| `context` | Normalized repository identity | Git remote or MCP root discovery |
| `routing` | Pure routing decision domain | Credential retrieval, API calls, CLI state |
| `credentials` | Credential references and provider interface | GitHub CLI discovery or token retrieval |
| `security` | Secret-safe value formatting | Full lifecycle hardening and zeroization |
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
The upstream process/session boundary will forward or isolate those
capabilities rather than duplicate their definitions.

## Security principles

- Raw credentials do not belong in project configuration.
- Routing must not depend on global `gh auth switch` state.
- There is no global mutable current-account state.
- Credential retrieval and routing decisions remain separate concerns.
- Ordinary `Debug` and `Display` formatting of secret values must redact them.
- Ambiguous write operations will eventually fail closed rather than guess an
  identity.

These are design principles. This feature validates configuration and expands
safe path references, but does not perform credential discovery, repository
inference, routing evaluation, or MCP proxying.

## Planned feature ownership

The next features add behavior behind the boundaries established here:

- configuration parsing and validation (`#3`, implemented)
- GitHub CLI credential discovery (`#4`)
- deterministic routing evaluation (`#5`)
- repository context discovery and safe write policy (`#6`)
- profile-isolated upstream sessions (`#7`)
- MCP request forwarding (`#8`)
- complete CLI workflows (`#9`)
- deeper secret, concurrency, logging, and process hardening (`#10`)
