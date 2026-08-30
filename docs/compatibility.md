# Compatibility and supported platforms

## Router and Rust

The package version is the semantic version shown by:

```bash
gh-mcp-router --version
```

The repository does not currently declare an MSRV. CI uses the stable Rust
toolchain and `Cargo.lock` is checked with `--locked` for reproducible
dependency resolution.

## Official GitHub MCP Server

The CI and release workflows use `github-mcp-server` **v1.11.0** as the
compatibility baseline. CI downloads that exact Linux x86_64 release, verifies
the upstream-provided SHA-256 checksum, and runs its executable smoke check.
The credential-free fake-upstream integration matrix covers the MCP initialize,
tool discovery, forwarding, routing, concurrency, error, and shutdown paths.

This is a reproducible baseline, not a claim that every future upstream
release is compatible. When updating the baseline, change the version, asset,
and checksum together in `.github/workflows/ci.yml`, rerun the full suite, and
review upstream MCP protocol/tool-surface changes before updating this page.

The router uses the upstream MCP protocol and tool surface; it does not copy
GitHub capability definitions into this repository.

## Supported release platforms

Release artifacts are built and tested on:

| Platform | Rust target | Artifact |
| --- | --- | --- |
| Linux x86_64 (glibc) | `x86_64-unknown-linux-gnu` | `gh-mcp-router-linux-x86_64.tar.gz` |
| macOS x86_64 | `x86_64-apple-darwin` | `gh-mcp-router-macos-x86_64.tar.gz` |
| macOS arm64 | `aarch64-apple-darwin` | `gh-mcp-router-macos-arm64.tar.gz` |

Windows is not claimed as supported by v0.1 because its process and stdio
behavior has not been included in the tested release matrix. Other targets
may compile from source but are not release-supported unless added to both the
CI and release matrices.
