# ADR-0001: Local and installation identity targets

- Status: Accepted
- Scope: Identity and routing model introduced for Issue #37
- Date: 2026-09-13

## Context

The v0.1 router selects a named profile and resolves a GitHub CLI credential
after routing. GitHub App installations need to become routing targets without
turning an installation ID into a fake CLI username or putting app secrets in
configuration.

Several identities cross this boundary but are not interchangeable:

- an MCP client's principal,
- a router-local principal,
- a GitHub user or organization account,
- a GitHub App installation,
- a credential source, and
- the identity of an official GitHub MCP upstream session.

## Decision

The crate defines metadata-only identity values in `identity`:

- `ClientPrincipalRef` and `RouterPrincipalRef` are distinct opaque principal
  references;
- `GitHubAccountRef` identifies a GitHub login or organization on a host;
- `GitHubInstallationId` identifies an installation, not an account or client;
- `CredentialSource` describes where a selected target will obtain credentials;
- `UpstreamIdentity` describes the authentication identity of one upstream
  session; and
- `RoutingTarget` is the credential-independent target selected by routing.

`RoutingTarget` currently has two variants:

1. `local_gh`, carrying a provider ID, GitHub account, and optional isolated
   `GH_CONFIG_DIR` metadata;
2. `github_app_installation`, carrying an installation ID, its GitHub account,
   and optional repository-grant metadata.

`ProfileConfig` retains the v0.1 flat `provider`, `user`, `host`, and
`gh_config_dir` fields. When `target` is absent, those fields derive a
`local_gh` target. An explicit `target` is required for an installation and
cannot be mixed with the legacy fields. Routes continue to select profile
names, so manual routes work for both target kinds through the same routing
engine.

`RoutingDecision` carries the selected profile and its metadata-only target.
Routing never resolves credentials, mints tokens, calls GitHub, or rewrites
repository context. The current v0.1 credential and upstream layers accept
only local targets. An installation target is represented and selected, then
fails explicitly at the local credential boundary until the installation
provider is implemented by Issues #38 and #39.

Installation identity views retain the GitHub host alongside the installation
ID. This prevents an Enterprise installation from colliding with a same-ID
installation on another host and preserves the endpoint needed by a future
provider and upstream session.

## Source of truth

| Data | Source of truth | Local representation |
| --- | --- | --- |
| Installation existence, suspension, account association | GitHub | Installation ID and account metadata in config/cache |
| Repository grants and installation permissions | GitHub | `RepositoryGrant` metadata, treated as cache/control-plane input |
| Profile labels and manual route rules | Router configuration | `ProfileConfig` and `RouteRule` |
| Client/router principal authentication | The transport and router deployment | Typed principal references; no GitHub identity substitution |
| Installation access tokens, private keys, JWTs | Credential provider / secret boundary | Never serialized in normal routing config |

Future installation discovery must refresh GitHub-owned state and must not
silently expand grants or permissions based only on stale local metadata.

## Consequences

- Existing v0.1 configuration remains representable without migration.
- Local and future installation routes share specificity, ambiguity, and
  fail-closed write behavior.
- Routing diagnostics can identify an installation target without exposing a
  token or app secret.
- `serve` and `doctor` report installation targets as unsupported by the
  current local provider rather than attempting to resolve them through `gh`.
- Token minting, installation discovery, webhooks, permission checks, and
  remote MCP authentication remain later features.

## Rejected alternatives

- Encoding an installation ID in `ProfileConfig.user` would conflate a GitHub
  App installation with a GitHub account and route it through the wrong
  provider.
- Adding app tokens or private keys to `RoutingTarget` would cross the
  configuration/routing secret boundary.
- Creating a second installation-specific routing engine would duplicate the
  deterministic v0.1 policy and make local/manual routes diverge.
