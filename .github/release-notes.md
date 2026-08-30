This release contains the local-first router and its profile-isolated stdio
integration with the official GitHub MCP Server.

Security and compatibility limitations:

- The router is a local developer process, not a public multi-tenant secret
  broker.
- Credentials remain in the user's GitHub CLI authentication context and are
  passed only to the selected upstream child process.
- The compatibility baseline is `github-mcp-server` v1.11.0. Newer upstream
  versions should be validated before use if their MCP surface changes.
- v0.1 release artifacts support Linux x86_64 and macOS x86_64/arm64. Windows
  is not a supported platform in this release.
