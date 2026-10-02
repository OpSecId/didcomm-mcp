# didcomm-mcp

An [MCP](https://modelcontextprotocol.io) server that lets an AI agent discover, learn
and use [DIDComm v2](https://identity.foundation/didcomm-messaging/spec/v2.1/)
protocols with any DIDComm agent. All encryption, keys, DID resolution, mediation and
transport stay inside this server; the AI only ever sees plaintext JSON.

The tool set is fixed. New protocols never add tools. Instead the AI looks a protocol up
in a [documentation registry](https://github.com/wyvrn-cloud/documentation-server) when
it needs it, then sends that protocol's messages through `send_didcomm_message`.

## Tools

| Tool | What it does |
|---|---|
| `get_identity` | This agent's DID (give it to peers), its mediation status, and the configured registry. |
| `discover_features` | Asks a peer which protocols it supports (`discover-features/2.0`). |
| `search_protocols` | Searches the registry's protocol catalog by URI pattern, text, status or tag. |
| `lookup_protocol_documentation` | One protocol's definition: roles, the sections you ask for, message types with examples and JSON Schemas. |
| `lookup_spec` | The DIDComm Messaging spec: table of contents or one section. |
| `send_didcomm_message` | Sends a message (you give `type` and `body`; `id`, `from`, `to` and `created_time` are filled in). It's validated against the registry's schema first, when there is one. With `wait_for_reply`, it returns the reply received on the same connection. |
| `fetch_messages` | Collects messages queued at this agent's mediator, and answers trust-pings and feature queries among them. |

Everything that came from a peer or the registry is returned behind an
**UNTRUSTED CONTENT** marker. It's third-party text that lands in the model's context,
and could carry a prompt injection.

A typical exchange, the brief's six steps: `discover_features` on the peer, then
`search_protocols` / `lookup_protocol_documentation`, then `send_didcomm_message`, then
`fetch_messages` for the reply.

## Running it

```sh
cargo build --release
```

MCP host configuration (e.g. Claude Code's `.mcp.json` or Claude Desktop's config):

```json
{
  "mcpServers": {
    "didcomm": {
      "command": "/path/to/didcomm-mcp",
      "env": { "DIDCOMM_MCP_REGISTRY_DID": "did:web:docs.example" }
    }
  }
}
```

Or as a container (`docker build -t didcomm-mcp .`; see the Dockerfile for the build
secrets a private `didcomm` dependency or a TLS-intercepting proxy needs):

```json
{
  "mcpServers": {
    "didcomm": {
      "command": "docker",
      "args": ["run", "-i", "--rm", "-v", "didcomm-mcp:/data",
               "-e", "DIDCOMM_MCP_REGISTRY_DID=did:web:docs.example", "didcomm-mcp"]
    }
  }
}
```

It speaks MCP over stdio and logs to stderr (`RUST_LOG=debug` for more). On first start
it creates its identity (private keys, owner-only file permissions). It then mediates
with the configured mediator in the background, without holding up the MCP handshake.

## Configuration

Everything has a default, so no file is needed. To use one, pass `--config <path>`, set
`$DIDCOMM_MCP_CONFIG`, or put it at `~/.config/didcomm-mcp/config.toml`.
Environment variables override the file.

| Setting (`config.toml`) | Environment | Default | |
|---|---|---|---|
| `identity_path` | `DIDCOMM_MCP_IDENTITY` | `~/.local/share/didcomm-mcp/identity.json` | This agent's keys. Keep the file to keep the DID. |
| `registry_did` | `DIDCOMM_MCP_REGISTRY_DID` | none | The documentation registry. Without it, the lookup tools report that none is configured and sends skip validation. |
| `mediator_did` | `DIDCOMM_MCP_MEDIATOR_DID` | the Indicio public mediator | Receives messages for this agent. `""` disables mediation; replies then only arrive via `wait_for_reply`. Indicio's is for development and demos, not production. |
| `allowed_targets` | `DIDCOMM_MCP_ALLOWED_TARGETS` (comma-separated) | any | If set, only these DIDs can be messaged or queried. |
| `validate_messages` | `DIDCOMM_MCP_VALIDATE_MESSAGES` | `true` | Schema-check outgoing messages. |

## Tests

```sh
cargo test
```

`tests/mcp_end_to_end.rs` drives the server with an `rmcp` client over an in-memory
transport. Behind it, real DIDComm parties run over HTTP on localhost: a mediator, a
stand-in documentation registry, and a peer. It runs the brief's workflow end to end:
- discover and look up a protocol
- a schema-rejected send
- a send whose reply arrives through the mediator
- a request with its reply on the same connection

It also covers problem reports, `allowed_targets`, running without a registry or
mediator, and messaging the agent's own mediator.

It has also been verified by hand over stdio with raw MCP JSON-RPC, against the real
documentation server and the live Indicio mediator.

### End to end, in containers

```sh
e2e/run.py
```

It needs Docker with Compose, plus sibling checkouts of
[`didcomm`](https://github.com/wyvrn-cloud/didcomm) and
[`documentation-server`](https://github.com/wyvrn-cloud/documentation-server) (with its
submodules). Override their locations with `DIDCOMM_DIR` / `DOCSERVER_DIR`. The script:
1. Builds and starts `e2e/docker-compose.yml`: the real documentation server, a
   mediator, and a peer ("Bob").
2. Runs the MCP server's container with stdin/stdout attached, as an MCP host would.
3. Walks through the whole workflow over raw MCP JSON-RPC:
   - the handshake and tool list
   - mediation
   - discovering Bob
   - searching and looking up `basicmessage/2.0` in the real registry
   - a schema-rejected send, then a validated send with Bob's ack
   - Bob messaging us through the mediator, then `fetch_messages`
   - a spec section

`--no-build` reuses already-built `didcomm-e2e/*` images. `--keep` leaves the stack
running.

See [`PLAN.md`](PLAN.md) for the design of the whole system.
