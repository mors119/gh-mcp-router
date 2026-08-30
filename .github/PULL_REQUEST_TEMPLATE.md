## Summary

<!-- What changed and why? Link the related issue when one exists. -->

## Validation

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test --all-features`
- [ ] `cargo build`

## Review checklist

- [ ] The change is focused and does not duplicate official GitHub MCP behavior.
- [ ] Tests and documentation were added or updated where behavior changed.
- [ ] No credentials, private configuration, or generated build artifacts are included.
- [ ] Routing remains deterministic and unsafe writes fail closed.
- [ ] Security, compatibility, and migration impact are called out above if applicable.
