# Security and trust model

The v0.1 router is a local developer process. The local user and configured MCP
client are trusted; the router is not a public multi-tenant credential broker.

## Credential flow

```text
profile reference in config
        ↓
gh auth status / gh auth token for that profile
        ↓
selected profile's official github-mcp-server child
```

Configuration stores provider, username, host, route rules, and optional
`GH_CONFIG_DIR` references. It never stores raw PATs, OAuth tokens, GitHub App
private keys, or authorization headers. The parent process is not changed and
the router never calls `gh auth switch`.

Each upstream child receives only its selected profile's credential through
`GITHUB_PERSONAL_ACCESS_TOKEN`. A child session is permanently associated with
one profile and cannot be reused for another profile. Profile-specific host and
`GH_CONFIG_DIR` values are also scoped to that child.

## Routing and writes

Repository context and routing are request-scoped. The router prefers explicit
owner/repository arguments, then other documented repository sources. A missing
or ambiguous context is not silently repaired. Writes fail closed unless the
configured policy explicitly permits a default profile.

Use these commands before enabling a client:

```bash
gh-mcp-router route OWNER/REPO --config config.yaml
gh-mcp-router explain OWNER/REPO --config config.yaml
gh-mcp-router doctor --config config.yaml --json
```

## Best-effort secret lifetime

Secret values use a redacting wrapper and zeroize owned buffers when their last
reference is dropped. Rust and the operating system can still make unavoidable
copies during allocation, subprocess creation, paging, or crash handling. v0.1
therefore provides best-effort in-memory protection, not an instant-erasure
guarantee.

Logs and diagnostics contain routing metadata such as profile, repository,
operation class, and session status. They must not contain credentials,
authorization headers, raw credential-provider output, or unredacted upstream
errors. Set `GH_MCP_ROUTER_LOG_LEVEL` to `error`, `warn`, `info`, `debug`, or
`trace`; increasing verbosity changes metadata volume only.
