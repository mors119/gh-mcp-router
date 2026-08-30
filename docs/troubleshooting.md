# Troubleshooting

Run diagnostics with the same configuration and environment that the MCP
client will use:

```bash
gh-mcp-router profiles --config /absolute/path/to/config.yaml
gh-mcp-router explain ExampleOrg/backend --config /absolute/path/to/config.yaml
gh-mcp-router doctor --config /absolute/path/to/config.yaml --json
```

## `gh` is not found

Install the GitHub CLI and make sure it is on the `PATH` inherited by the
client. Verify with:

```bash
command -v gh
gh --version
```

GUI-launched clients can have a different `PATH` from a terminal. Use absolute
paths for the router and upstream binary, or launch the client from an
environment with the required `PATH`.

## A GitHub account is missing

Check the host and username in the profile:

```bash
gh auth status --hostname github.com
gh-mcp-router profiles --config config.yaml
```

For an isolated profile, repeat the check with its configured directory:

```bash
GH_CONFIG_DIR="$HOME/.config/gh-work" gh auth status --hostname github.com
```

The profile's `user` and `host` must match an authenticated account. The router
does not switch accounts to make a mismatch pass.

## The upstream GitHub MCP Server is not found

Install the official `github-mcp-server` binary and check it directly:

```bash
command -v github-mcp-server
github-mcp-server --help
```

If it is not on `PATH`, pass its absolute path to both `doctor` and `serve`:

```bash
gh-mcp-router doctor --upstream-binary /absolute/path/to/github-mcp-server --config config.yaml
gh-mcp-router serve --upstream-binary /absolute/path/to/github-mcp-server --config config.yaml
```

## No matching route or an ambiguous route

Inspect the normalized context and rule evaluation:

```bash
gh-mcp-router explain OWNER/REPO --config config.yaml
```

Add an exact `repo` rule for an override, or remove conflicting rules at the
same specificity. A write with missing or ambiguous repository context is
expected to fail closed.

## The selected identity lacks permission

Routing can select only an identity; GitHub still enforces that account's
repository permissions. Confirm the selected username with `profiles` and test
the account's access through its own `gh_config_dir` if configured. Add a route
to an account that has the required permission; do not broaden credentials in
router configuration.

## The upstream process exits

Run `doctor` with the same `--upstream-binary` and configuration. Confirm the
upstream executable accepts the default `stdio` startup argument and that the
selected `gh` account is authenticated. A failed session is discarded and is
restarted on the next routed request; one profile's failure must not reassign
another profile's session.

## The client shows stale or duplicate GitHub tools

Keep only one router entry for this project. Remove or disable older direct
`github-mcp-server` entries and account-specific router entries, restart the
client, and run its MCP server list/refresh command. The router itself exposes
one validated upstream tool surface; it does not create per-account tool names.

## Configuration errors

Validate the exact file passed to the client:

```bash
gh-mcp-router validate --config /absolute/path/to/config.yaml
```

Use YAML or JSON matching the checked-in
[`examples/config.example.yaml`](../examples/config.example.yaml). Never add a
token field to silence a validation error; credentials are intentionally
resolved through `gh`.
