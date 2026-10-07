# didcomm-mcp

An [MCP](https://modelcontextprotocol.io) server that lets an AI agent discover, learn
and use [DIDComm v2](https://identity.foundation/didcomm-messaging/spec/v2.1/)
protocols with any DIDComm agent, and DIDComm v1 (Aries) protocols over connections
made from out-of-band invitations and DID Exchange. All encryption, keys, DID resolution, mediation and
transport stay inside this server; the AI only ever sees plaintext JSON.

The registry is asked in [`documentation/1.1`](https://github.com/wyvrn-cloud/protocols/blob/master/protocols/documentation/1.1/readme.md),
falling back to 1.0 for a registry that doesn't speak it yet.

The tool set is fixed. New protocols never add tools. Instead the AI looks a protocol up
in a [documentation registry](https://github.com/wyvrn-cloud/documentation-server) when
it needs it, then sends that protocol's messages through `send_didcomm_message`.

## Tools

| Tool | What it does |
|---|---|
| `get_identity` | This agent's DID (give it to peers), its mediation status, the configured registry, and its DIDComm v1 DID and mediation. |
| `discover_features` | Asks a peer which protocols it supports (`discover-features/2.0`). |
| `search_protocols` | Searches the registry's protocol catalog by URI pattern, text, status, tag or DIDComm version (`didcomm_version`: `2.1`, `1.0`). |
| `lookup_protocol_documentation` | One protocol's definition: roles, the DIDComm versions it's used with, the sections you ask for, message types with examples and JSON Schemas (one per DIDComm version), and for credential protocols the attachment formats their messages carry, with schemas for the attachments' contents. |
| `lookup_spec` | A document's table of contents or one section: the DIDComm Messaging spec (`2.1`, `2.0`, `editors-draft`, and `1.0`, DIDComm v1 as the Aries RFCs define it), or another `document` the registry lists, such as `extension/l10n`. |
| `send_didcomm_message` | Sends a message to a DID or a connection (you give `type` and `body`; the headers are filled in). DIDComm v1 to a v1 connection, or when the registry lists the type for DIDComm v1 alone (then the `body` holds the message's own fields, next to `@type`, `@id`, `~thread`); DIDComm v2 otherwise. It's validated against the registry's schema for that DIDComm version first, when there is one. With `wait_for_reply`, it returns the reply received on the same connection. |
| `fetch_messages` | Collects messages queued at this agent's mediators (v2 and v1), takes DID Exchange handshakes among them a step further, and answers trust-pings and feature queries. |
| `accept_invitation` | Connects through an out-of-band invitation (URL or JSON): DID Exchange 1.1/1.0 for a v1 one; an OOB 2.0 one becomes a connection to the inviter's DID. |
| `create_invitation` | An out-of-band invitation (and URL) for other agents to connect to this one over DIDComm v1. Needs the v1 mediator. |
| `list_connections` | The connections: id (usable as `target_did`), state, role, DIDComm version, the peer's label and DID. |

Everything that came from a peer or the registry is returned behind an
**UNTRUSTED CONTENT** marker. It's third-party text that lands in the model's context,
and could carry a prompt injection.

A typical exchange, the brief's six steps: `discover_features` on the peer, then
`search_protocols` / `lookup_protocol_documentation`, then `send_didcomm_message`, then
`fetch_messages` for the reply.

## Install

Download the archive for your platform from the
[latest release](https://github.com/wyvrn-cloud/mcp/releases/latest): Linux (x86_64,
ARM64; static, any distribution), macOS (Apple silicon, Intel) or Windows (x64, ARM64).
For example, on Linux:

```sh
curl -fL https://github.com/wyvrn-cloud/mcp/releases/latest/download/didcomm-mcp-x86_64-unknown-linux-musl.tar.gz | tar xz
sudo install -m 755 didcomm-mcp-x86_64-unknown-linux-musl/didcomm-mcp /usr/local/bin/
```

**[docs/install.md](docs/install.md)** walks through every platform: downloading and
verifying, connecting Claude Code, Claude Desktop or another MCP host, configuring it,
running it as a service, upgrading and uninstalling.

To build it yourself: `cargo install --locked --git https://github.com/wyvrn-cloud/mcp --tag v0.1.1 didcomm-mcp`
([docs/building.md](docs/building.md): requirements, building from a clone, static
builds).

## Running it

An MCP host launches it and talks to it over stdio. In Claude Code:
`claude mcp add didcomm -- /usr/local/bin/didcomm-mcp`. In a host's JSON configuration
(e.g. Claude Code's `.mcp.json` or Claude Desktop's config):

```json
{
  "mcpServers": {
    "didcomm": {
      "command": "/usr/local/bin/didcomm-mcp"
    }
  }
}
```

Or as a container (`docker build -t didcomm-mcp .`; see the Dockerfile for the build
secret a TLS-intercepting proxy needs):

```json
{
  "mcpServers": {
    "didcomm": {
      "command": "docker",
      "args": ["run", "-i", "--rm", "-v", "didcomm-mcp:/data", "didcomm-mcp"]
    }
  }
}
```

### Over HTTP

`didcomm-mcp --http [<address>]` serves MCP over
[Streamable HTTP](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#streamable-http)
at `http://<address>/mcp` instead of stdio. The default address is `127.0.0.1:8090`
(`http.bind`). All sessions share one agent: the same keys, DID and mediator.

- **A bearer token** (`DIDCOMM_MCP_HTTP_TOKEN`, or `http.auth_token`) is required in
  every request's `Authorization: Bearer` header when set. It's *mandatory* for any
  non-loopback address, and the server refuses to start without one, because anyone
  who can reach the endpoint acts with this agent's keys.
- **`Host` checking** accepts loopback names only by default, to stop DNS rebinding
  against a local server. To serve under a real hostname, list it in
  `DIDCOMM_MCP_HTTP_ALLOWED_HOSTS` (comma-separated) or `http.allowed_hosts`. Put TLS in
  front (a reverse proxy) for anything beyond localhost.
- **`GET /healthz`** answers without a token.

```sh
DIDCOMM_MCP_HTTP_TOKEN=$(openssl rand -hex 32) didcomm-mcp --http
docker run --rm -p 127.0.0.1:8090:8090 -v didcomm-mcp:/data \
  -e DIDCOMM_MCP_HTTP_TOKEN=... didcomm-mcp --http 0.0.0.0:8090
```

MCP host configuration for an HTTP server:

```json
{
  "mcpServers": {
    "didcomm": {
      "type": "http",
      "url": "http://localhost:8090/mcp",
      "headers": { "Authorization": "Bearer ..." }
    }
  }
}
```

### As a service

`didcomm-mcp service install` runs `--http` as a service that starts with the machine:
a systemd unit on Linux, a launchd daemon on macOS, a Windows service on Windows (or,
with `--user` on Linux and macOS, a service of your own). It writes a configuration
with a new bearer token if there's none, starts the service, and prints the URL and the
command to connect Claude Code. Running it again after an upgrade restarts the service
with the new binary; `didcomm-mcp service uninstall` removes it.

```sh
sudo didcomm-mcp service install
```

See [docs/install.md](docs/install.md#4-run-it-as-a-service) for where each platform
keeps the configuration, identity and logs, and how to manage the service.

### Logging

It speaks MCP over stdio and logs to stderr (`RUST_LOG=debug` for more). On first start
it creates its identity (private keys, owner-only file permissions). It then mediates
with the configured mediator in the background, without holding up the MCP handshake,
and then with the DIDComm v1 mediator: it connects to it with DID Exchange and asks for
coordinate-mediation/1.0, so v1 peers can reach it too. Connections and the v1 mediation
are kept in a state file next to the identity.

## Configuration

Everything has a default, so no file is needed. To use one, pass `--config <path>`, set
`$DIDCOMM_MCP_CONFIG`, or put it at `~/.config/didcomm-mcp/config.toml`
(`%APPDATA%\didcomm-mcp\config.toml` on Windows). Environment variables override the
file. A service uses the file `service install` wrote (see
[docs/install.md](docs/install.md#where-things-are)).

| Setting (`config.toml`) | Environment | Default | |
|---|---|---|---|
| `identity_path` | `DIDCOMM_MCP_IDENTITY` | `~/.local/share/didcomm-mcp/identity.json` (Windows: `%LOCALAPPDATA%\didcomm-mcp\identity.json`) | This agent's keys. Keep the file to keep the DID. |
| `registry_did` | `DIDCOMM_MCP_REGISTRY_DID` | `did:web:docs.wyvrn.app` | The [documentation registry](https://github.com/wyvrn-cloud/documentation-server). `""` disables it: the lookup tools then report that none is configured, and sends skip validation. |
| `mediator_did` | `DIDCOMM_MCP_MEDIATOR_DID` | the Indicio public mediator | Receives messages for this agent. `""` disables mediation; replies then only arrive via `wait_for_reply`. Indicio's is for development and demos, not production. |
| `v1_mediator` | `DIDCOMM_MCP_V1_MEDIATOR` | `mediator_did` | The DIDComm v1 mediator: a DID (connected to through an implicit invitation, as the Indicio public mediator accepts) or an out-of-band invitation URL. `""` disables it; v1 peers can then only answer on the same connection, and `create_invitation` is unavailable. |
| `state_path` | `DIDCOMM_MCP_STATE` | `connections.json` next to the identity | Connections, created invitations and the v1 mediation. |
| `allowed_targets` | `DIDCOMM_MCP_ALLOWED_TARGETS` (comma-separated) | any | If set, only these DIDs (and connections with them) can be messaged or queried, and only invitations naming one can be accepted. |
| `validate_messages` | `DIDCOMM_MCP_VALIDATE_MESSAGES` | `true` | Schema-check outgoing messages. |
| `public_url` | `DIDCOMM_MCP_PUBLIC_URL` | none | The public base URL of this server. With `--http`, DIDComm messages (v1 and v2) are then accepted at `<public_url>/didcomm`, and the agent's DID names that endpoint: peers deliver straight to it, no mediator needed. Delivered messages are queued for `fetch_messages`; trust-pings and DID Exchange are answered on arrival. `create_invitation` works without a v1 mediator. The DID changes with the URL. |
| `database_url` | `DIDCOMM_MCP_DATABASE_URL`, else `DATABASE_URL` | none | A Postgres URL. The identity (private keys, as in the identity file), the state and the inbox then live in two tables (`didcomm_mcp_kv`, `didcomm_mcp_inbox`, created on startup) instead of files. |
| `[http] bind` | `DIDCOMM_MCP_HTTP_BIND` | `127.0.0.1:8090` | Address for `--http` (`--http <address>` overrides it). |
| `[http] auth_token` | `DIDCOMM_MCP_HTTP_TOKEN` | none | Bearer token for `--http`; required for non-loopback addresses. |
| `[http] allowed_hosts` | `DIDCOMM_MCP_HTTP_ALLOWED_HOSTS` | loopback names | `Host` header values `--http` accepts. |

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
mediator, and messaging the agent's own mediator. For DIDComm v1, a v1 peer and a v1
mediator join them:
- accepting an invitation URL (DID Exchange), then v1 sends validated against the v1
  schema, with the reply on the connection
- creating an invitation behind the v1 mediator; the peer's request, our response and
  its completion through `fetch_messages`; a v1 message through the mediator
- connections and the v1 mediation surviving a restart

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

### CI

`.github/workflows/ci.yml` runs `cargo test`, a `docker build`, and `e2e/run.py` on every
pull request. The end-to-end job needs a `WYVRN_READ_TOKEN` repository secret, a token
that can read the private `wyvrn-cloud/documentation-server`; without it, the job skips.

`.github/workflows/release.yml` builds the release binaries for six targets and
smoke-tests each on its own OS (`.github/scripts/smoke_test.py`): `--version`, MCP over
stdio, and, on every platform but Intel macOS, a real `service install`, an MCP request
with the generated token, a reinstall, and `service uninstall`. It runs on pull requests
that touch the release path, and on manual runs.

## Releasing

1. Set the new version in `Cargo.toml` (and `Cargo.lock`, with `cargo update -p didcomm-mcp`)
   and add its section to [`CHANGELOG.md`](CHANGELOG.md); merge that.
2. Either push a tag on the merge commit (`git tag v0.2.0 && git push origin v0.2.0`), or
   run the **Release** workflow on `master` from the Actions tab with **publish** ticked,
   which creates the tag itself.

The release workflow checks that the tag matches `Cargo.toml`, builds and tests every
platform, and publishes the GitHub release with the archives, `SHA256SUMS`, and the
changelog section as its notes.

See [`PLAN.md`](PLAN.md) for the design of the whole system.
