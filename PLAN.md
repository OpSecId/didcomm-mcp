# Plan: MCP ⇄ DIDComm bridge

This is the plan for the whole system, not just this repo. It has three parts:

- **`mcp`** (this repo): a local MCP server. It lets an AI host discover, look up and
  run DIDComm v2 protocols with any peer through a small, fixed set of tools.
- **[`documentation-server`](https://github.com/wyvrn-cloud/documentation-server)**: a
  DIDComm agent that serves protocol and spec documentation, plus hand-written JSON
  Schemas, over the `documentation/1.0` protocol.
- **[`protocols`](https://github.com/wyvrn-cloud/protocols)**: the specification for
  `https://wyvrn.app/documentation/1.0` (and other wyvrn-authored protocols).

All three build on [`didcomm`](https://github.com/wyvrn-cloud/didcomm), the Rust
DIDComm v1/v2 workspace. That repo is going public soon, so everything here is built as
if it already were: no private-repo assumptions in shared crates, and nothing in CI that
needs a token to fetch it.

## Decisions

| Topic | Decision |
|---|---|
| MCP server language | Rust, using the official [`rmcp`](https://crates.io/crates/rmcp) SDK, over stdio. It uses the `didcomm` crates directly, so no binding layer can be missing features. |
| Documentation protocol URI (PIURI) | `https://wyvrn.app/documentation/1.0`. It moves to `didcomm.org` only if accepted upstream (per `protocols`' README). |
| JSON Schemas | didcomm.org has none; protocols are prose plus example messages. We write schemas by hand in a `schemas/` folder that the documentation server merges into its responses. First pass covers the core protocols (below). Longer term, we'll propose to the DIDComm users group that new protocols ship with schemas. |
| Protocol identity | Protocols are keyed by **PIURI** (the frontmatter `piuri:` field), never by folder path. A checked-in PIURI→path mapping file takes precedence over folder names until the upstream naming is fixed. |
| Documentation sources | A configurable list of folders. Defaults: the `didcomm.org` and `didcomm-messaging` git submodules. Anyone hosting their own server can add more folders (e.g. a checkout of `wyvrn-cloud/protocols`); off by default. |
| Mediator | Configurable. The default config points at the Indicio public mediator, `did:web:us-east2.public.mediator.indiciotech.io`, which supports DIDComm v1 and v2. CI and end-to-end tests run a local `didcomm-mediator-core` instead, so they don't depend on a public service. |

## Things that differ from the original architecture brief

1. **didcomm.org has no JSON Schemas.** Each protocol is a `readme.md`: YAML
   frontmatter, prose, and examples in fenced code blocks, and some have no examples at
   all. Schemas therefore come from our own `schemas/` overlay. Protocols without one
   still get structured output: metadata, sections, and examples grouped by message type.
2. **Three tools aren't enough.** A local MCP server has no public address, so replies
   to asynchronous protocols land at its mediator. It needs `fetch_messages` (pickup)
   and `get_identity` (so you can hand peers your DID) on top of the brief's three.
3. **Folder names on didcomm.org don't always match PIURIs.** For example,
   `question-answer/` holds `.../questionanswer/1.0`, and `messagepickup/4.0` holds
   `.../message-pickup/4.0`. Some protocols are DIDComm v1 (they use `@type` instead of
   `type`). Hence the PIURI keying and the mapping file.

## Phases

### Phase 1: `documentation/1.0` spec (`protocols` repo)

`protocols/documentation/1.0/readme.md`, in didcomm.org's own format. Roles are
`requester` and `registry`. Messages:

| Message | Direction | Purpose |
|---|---|---|
| `query` | requester → registry | List or search protocols by PIURI match pattern (same matching rules as discover-features 2.0), optionally filtered by status or tag. |
| `catalog` | registry → requester | Matching entries: PIURI, title, status, summary, whether a schema exists. |
| `request` | requester → registry | Fetch one protocol by PIURI. Optional `sections` filter to keep the AI's context small. |
| `response` | registry → requester | Frontmatter metadata, roles, states, sections by heading, example messages grouped by message type, and JSON Schemas per message type (where available). |
| `spec-request` / `spec-response` | both | Fetch a DIDComm spec section by spec version (`2.0`, `2.1`, `editors-draft`) and section id, or the table of contents. |

Errors use `report-problem/2.0` with codes such as `e.p.msg.not-found` and
`e.p.msg.unsupported-version`. Replies thread on `thid`. The spec includes JSON
Schemas for its own messages, practising what we preach.

Core protocols that get JSON Schemas in the first pass: discover-features 2.0,
trust-ping 2.0, basicmessage 2.0, report-problem 2.0, coordinate-mediation 3.0,
messagepickup 3.0, routing 2.0, out-of-band 2.0, documentation 1.0.

### Phase 2: shared agent crate (`didcomm` repo)

Pull the agent-runtime pieces that `didcomm-peer-service` currently hand-rolls into a
reusable, publishable crate (working name `didcomm-agent`):

- `send`: pack, POST, and unpack the synchronous reply (`return_route: all`)
- a WebSocket transport (the Indicio mediator advertises one)
- mediation setup (`coordinate-mediation/3.0`) and the pickup cycle
  (`messagepickup/3.0`); add a `message-pickup/4.0` client if the target mediator needs it
- a responder for `discover-features/2.0` queries, and one for `trust-ping/2.0`
- saving and loading key material (JWK file to start; the storage layer is pluggable)

`didcomm-peer-service` then becomes a thin user of this crate.

### Phase 3: `documentation-server`

See [documentation-server's PLAN.md](https://github.com/wyvrn-cloud/documentation-server/blob/master/PLAN.md).
In short: submodules plus configurable source folders, a startup indexer keyed by PIURI,
the `schemas/` overlay, and an axum DIDComm endpoint with its own `did:web` that answers
`documentation/1.0`, `discover-features/2.0` and `trust-ping/2.0`.

### Phase 4: `mcp` (this repo)

Rust and `rmcp`, over stdio. Tools (fixed; new protocols never add tools):

| Tool | Arguments | Notes |
|---|---|---|
| `get_identity` | — | Our DID and the mediator in use, to share with peers. |
| `discover_features` | `target_did`, `match?` | Sends discover-features/2.0 `queries`; returns the `disclose` results. |
| `lookup_protocol_documentation` | `protocol_uri`, `sections?` | Sends documentation/1.0 `request` to the configured registry. |
| `search_protocols` | `match?`, `status?`, `tag?` | Sends documentation/1.0 `query`. |
| `lookup_spec` | `version?`, `section?` | Sends documentation/1.0 `spec-request`. |
| `send_didcomm_message` | `target_did`, `type`, `body`, `thid?`, `pthid?`, `wait_for_reply?` | The server fills in `id`, `from`, `created_time` and threading. Optionally checks the body against a schema fetched from the registry before sending. |
| `fetch_messages` | `limit?` | Pickup from the mediator. Returns decrypted messages with sender DID and thread ids. |

Configuration (TOML file plus environment-variable overrides): key file path (created
on first run, permissions `0600`), registry DID, mediator DID (default: the Indicio
public mediator), optional allow-list of DIDs we may send to.

Security posture:
- Keys never leave the server.
- Everything that came from a peer or the registry is returned clearly marked as
  untrusted content, because it lands in the AI's context and could carry a prompt
  injection.
- Sending is a separate, explicit tool, so the MCP host's approval prompt covers it.

### Phase 5: end-to-end test

A `docker-compose.yml` with the documentation server, `didcomm-mediator-core` (via
`didcomm-peer-service ROLE=mediator`), and a peer acting as "Bob". An `rmcp` client
test drives the brief's six-step flow: discover, look up, basicmessage, protocol
message, fetch the reply. A separate manual, non-CI smoke test runs against the
Indicio mediator.

## Open issues

- **Indicio mediator returns 500 on DIDComm v2 messages (2026-10-01).** We probed it
  with valid authcrypt and anoncrypt envelopes (sender `did:peer:2` and `did:peer:4`;
  trust-ping and discover-features). Every one got HTTP 500 with an unencrypted
  `report-problem/2.0` (`e.m.me`, "Internal server error"). Junk input gets a clean 400,
  so the mediator parses the envelope and fails afterwards. It still needs to be found
  out, with Indicio's help, which protocol versions it speaks and whether the fault is
  on their side or ours.
