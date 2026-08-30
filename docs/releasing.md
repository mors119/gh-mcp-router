# Release process

Releases use semantic-version tags and are created by GitHub Actions. The
workflow does not publish to crates.io and does not run on ordinary pushes.

Before creating `v0.1.0`, the v0.1 roadmap Features must be complete and
Issue #14 must pass. The release notes must retain the security and upstream
compatibility limitations described in `.github/release-notes.md`.

## Release candidate checklist

1. Confirm the release candidate is on `main` and that the required roadmap
   Issues, including #10 and #12, are merged.
2. Run the complete local checks:

   ```bash
   cargo fmt --check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test --all-features
   cargo build
   cargo package --locked
   ```

3. Verify the executable version:

   ```bash
   cargo run -- --version
   ```

4. Update `version` in `Cargo.toml` and `Cargo.lock` as one change when
   making a later release. The tag must match the package version exactly;
   the release workflow rejects mismatches.
5. Create and push an annotated tag without force-pushing:

   ```bash
   git tag -a v0.1.0 -m "gh-mcp-router v0.1.0"
   git push origin v0.1.0
   ```

The tag workflow reruns the release gates, builds the three supported platform
archives, writes a SHA-256 sidecar for each archive, verifies those checksums,
and creates a GitHub Release with generated notes plus the repository's
security/compatibility limitations.

## Installing a release artifact

Download the archive and its matching `.sha256` file from the GitHub Release,
verify the checksum, then place the `gh-mcp-router` binary on `PATH`:

```bash
sha256sum --check gh-mcp-router-linux-x86_64.tar.gz.sha256
tar -xzf gh-mcp-router-linux-x86_64.tar.gz
install -m 755 gh-mcp-router "$HOME/.local/bin/gh-mcp-router"
```

On macOS, use `shasum -a 256 --check` and the archive matching the Mac's CPU.
The source checkout path in [installation.md](installation.md) remains
available for development and for targets not covered by the release matrix.
