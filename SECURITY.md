# Security policy

`gh-mcp-router` handles credentials for multiple GitHub identities. Please
treat suspected credential exposure or identity cross-talk as a security issue,
not as a normal bug report.

## Supported versions

Security fixes are developed against `main` and the latest published release.
Pre-release builds are for development and are not a production security
boundary. See [`docs/compatibility.md`](docs/compatibility.md) for the current
support and upstream compatibility baseline.

## Reporting a vulnerability

Use [GitHub's private vulnerability reporting](https://github.com/mors119/gh-mcp-router/security/advisories/new)
for this repository. If private reporting is unavailable, contact the
maintainer privately through GitHub. Please do not open a public issue for an
unfixed vulnerability.

Include the affected version or commit, a concise description, reproduction
steps or a minimal proof of concept, and the impact. Redact tokens, cookies,
authorization headers, private repository data, and personal configuration from
all reports. Synthetic credentials and disposable repositories are preferred.

Maintainers will acknowledge a report as soon as practical, investigate it,
and coordinate disclosure after a fix or mitigation is available. Please allow
time for validation before publishing details.

## Credential-safety notes

The router is a local developer process, not a public multi-tenant credential
broker. It must not persist raw credentials, call `gh auth switch`, expose
tokens in process arguments, or include secrets in logs and MCP responses. See
[`docs/security.md`](docs/security.md) for the trust model and the limitations
of best-effort in-memory secret protection.
