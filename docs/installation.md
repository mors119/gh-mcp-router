# Installation and GitHub authentication

This document describes the supported pre-release v0.1 installation path.

## Availability

There is currently no published `gh-mcp-router` crate on crates.io and no
downloadable release binary. `cargo install gh-mcp-router` is therefore not a
supported command yet. Build from a checked-out repository instead:

```bash
git clone https://github.com/mors119/gh-mcp-router.git
cd gh-mcp-router
cargo build --release
```

The binary is `target/release/gh-mcp-router`. To install it in a user-level
directory on Unix-like systems:

```bash
mkdir -p "$HOME/.local/bin"
install -m 755 target/release/gh-mcp-router "$HOME/.local/bin/gh-mcp-router"
```

Ensure `$HOME/.local/bin` is on `PATH`, or use the absolute binary path in the
MCP client configuration. A local Cargo install is also possible while
developing from the checkout:

```bash
cargo install --path .
```

That command installs the checked-out source; it does not download a release
from crates.io.

## Prerequisites

The router relies on two external executables:

1. [GitHub CLI (`gh`)](https://cli.github.com/) for account discovery and
   profile-specific credential lookup.
2. The official
   [GitHub MCP Server](https://github.com/github/github-mcp-server) binary,
   named `github-mcp-server` and available on `PATH`.

The router does not download, replace, or reimplement the upstream GitHub MCP
Server. Verify the tools before starting a client:

```bash
gh --version
github-mcp-server --help
gh-mcp-router --help
```

If the upstream binary is outside `PATH`, use an absolute path for diagnostics
and serving:

```bash
gh-mcp-router doctor --upstream-binary /absolute/path/to/github-mcp-server
gh-mcp-router serve --upstream-binary /absolute/path/to/github-mcp-server
```

The `--upstream-binary` option selects the executable only. It does not change
the credential or routing configuration.

## Prepare multiple GitHub CLI accounts

Authenticate each account with `gh`, then inspect the accounts known to the
host:

```bash
gh auth status --hostname github.com
```

The output should identify the usernames that will be referenced by the
router, for example `personal-user` and `work-user`. The router uses the
non-secret username and host as a lookup reference; it asks `gh` for the
selected account's credential only after routing has selected a profile.

For accounts that should be isolated completely, create one GitHub CLI config
directory per account and authenticate within that directory:

```bash
mkdir -p "$HOME/.config/gh-personal" "$HOME/.config/gh-work"
GH_CONFIG_DIR="$HOME/.config/gh-personal" gh auth login --hostname github.com
GH_CONFIG_DIR="$HOME/.config/gh-work" gh auth login --hostname github.com
GH_CONFIG_DIR="$HOME/.config/gh-personal" gh auth status --hostname github.com
GH_CONFIG_DIR="$HOME/.config/gh-work" gh auth status --hostname github.com
```

Reference those existing directories with `gh_config_dir` in the profile
configuration. The directory must exist when `profiles`, `doctor`, or `serve`
uses that profile. This is an alternative to the shared `gh` configuration
workflow below: the current `init` command discovers accounts from the default
GitHub CLI configuration and has no option to discover accounts from multiple
`GH_CONFIG_DIR` values. For this isolated setup, skip `init` and create the
profiles manually using the examples in
[`configuration.md`](configuration.md), including each `gh_config_dir`.

`gh-mcp-router` never runs `gh auth switch`. It does not change the parent
process's active account and it does not require a globally selected account to
be changed while requests are in flight.

## Create a starter configuration

Use `init` when the accounts are available in the default GitHub CLI
configuration:

```bash
gh-mcp-router init \
  --config "$HOME/.config/gh-mcp-router/config.yaml" \
  --profile personal-user=personal \
  --profile work-user=work
```

`init` discovers authenticated accounts and writes only profile references. It
does not write tokens. It refuses to overwrite an existing configuration;
pass `--force` only when replacing it is intentional. Add routes to the
generated file, then validate and inspect it:

```bash
gh-mcp-router validate --config "$HOME/.config/gh-mcp-router/config.yaml"
gh-mcp-router profiles --config "$HOME/.config/gh-mcp-router/config.yaml"
gh-mcp-router doctor --config "$HOME/.config/gh-mcp-router/config.yaml"
```

The default path is `$XDG_CONFIG_HOME/gh-mcp-router/config.yaml` when
`XDG_CONFIG_HOME` is set, otherwise `~/.config/gh-mcp-router/config.yaml` on
Unix-like systems. Windows uses `%APPDATA%/gh-mcp-router/config.yaml`.
Use `--config PATH` when the file is elsewhere.

## Security expectations

Router configuration contains usernames, hosts, route rules, and optional
`GH_CONFIG_DIR` references. It must not contain a PAT, OAuth token, GitHub App
private key, or an `Authorization` header. The resolved credential is passed
only to the selected official GitHub MCP child process through its environment.

For the trust model, best-effort in-memory protections, process boundaries,
and logging policy, see [Security and trust](security.md).
