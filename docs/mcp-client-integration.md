# MCP client integration

The v0.1 client-facing transport is local stdio. The client starts one
`gh-mcp-router serve` process and exchanges newline-delimited JSON-RPC 2.0
messages with it. The router performs profile selection behind that one
connection and exposes one upstream GitHub tool surface.

Do not configure one server per GitHub account. That would expose duplicated
account-specific tools and bypass the router's request routing.

## VS Code

VS Code uses a `servers` object in `.vscode/mcp.json`. Add one server entry and
use absolute paths for both the binary and configuration file when the client
will start from an unexpected working directory:

```json
{
  "servers": {
    "github-router": {
      "type": "stdio",
      "command": "/absolute/path/to/gh-mcp-router",
      "args": [
        "serve",
        "--config",
        "/absolute/path/to/gh-mcp-router.yaml"
      ]
    }
  }
}
```

The same entry can be added to the VS Code user profile with its command-line
helper:

```bash
code --add-mcp '{"name":"github-router","type":"stdio","command":"/absolute/path/to/gh-mcp-router","args":["serve","--config","/absolute/path/to/gh-mcp-router.yaml"]}'
```

In VS Code, run `MCP: List Servers`, start `github-router`, and approve the
server trust prompt if shown. Use `Configure Tools` in Chat to confirm that the
server presents one GitHub tool list. If the server was previously configured
under another command or name, stop/remove the stale entry before comparing
tool lists.

VS Code's configuration format and lifecycle are documented in the
[VS Code MCP configuration reference](https://code.visualstudio.com/docs/agents/reference/mcp-configuration).

## MCP Inspector CLI verification

The MCP Inspector CLI is a scriptable stdio MCP client. It is the reproducible
client used to verify the router's documented handshake and tool discovery
path:

```bash
npx @modelcontextprotocol/inspector --cli \
  /absolute/path/to/gh-mcp-router serve \
  --method tools/list \
  --format json
```

This ad-hoc form uses the router's default configuration path. The positional
arguments after `--cli` are the command and arguments for the stdio server. For
a non-default router config path, put the complete launch command in an
Inspector config file as shown below; this avoids the Inspector's own
`--config` option being confused with the router's `--config` option.

For a reusable Inspector configuration file, use its `mcpServers` format:

```json
{
  "mcpServers": {
    "github-router": {
      "type": "stdio",
      "command": "/absolute/path/to/gh-mcp-router",
      "args": [
        "serve",
        "--config",
        "/absolute/path/to/gh-mcp-router.yaml"
      ]
    }
  }
}
```

Then query the configured server:

```bash
npx @modelcontextprotocol/inspector --cli \
  --config /absolute/path/to/mcp.json \
  --server github-router \
  --method initialize \
  --format json

npx @modelcontextprotocol/inspector --cli \
  --config /absolute/path/to/mcp.json \
  --server github-router \
  --method tools/list \
  --format json
```

The repository's validation uses the ad-hoc form so the client command is
visible in the test output. The Inspector is a development verification tool;
it does not add a runtime dependency to `gh-mcp-router`.

## Other clients and unsupported claims

Any MCP host that can launch local stdio servers may be able to use the same
command, but configuration keys and lifecycle behavior vary by host. Claude
Desktop, Claude Code, Cursor, ChatGPT, and other clients are not claimed as
verified v0.1 integrations by this repository unless a client-specific setup
has been exercised and added here. In particular, this project does not claim
ChatGPT local MCP support.

## Client-side safety notes

Do not add `GITHUB_PERSONAL_ACCESS_TOKEN`, `GH_TOKEN`, or an `Authorization`
header to the client configuration. Authentication is resolved by the router
from the configured `gh` profile. Client logs and MCP error panes should still
be treated as local diagnostic output; use `doctor` and the security guidance
when investigating failures.
