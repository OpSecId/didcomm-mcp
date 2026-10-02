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

### Phase 2: shared agent crate (`didcomm` repo) -- done except WebSocket

`crates/didcomm-agent` in the `didcomm` repo. It's the agent runtime that
`didcomm-peer-service` used to hand-roll, as a reusable crate, and the peer service is
now built on it:

- `Identity`: Ed25519 + X25519 keys in a `0600` JWK file. The agent's DIDs are derived
  from the keys and an endpoint, so they survive restarts without storing anything else.
- `Agent::send` / `Agent::request` over HTTP(S). `request` asks for `return_route: all`,
  checks the reply's thread, and returns a problem report as an `AgentError::Problem`.
- `Agent::mediate` (`coordinate-mediation/3.0`) and `Agent::pickup`
  (`messagepickup/3.0`, acknowledging what it collects).
- `Features` plus `Agent::auto_reply` for `discover-features/2.0` and `trust-ping/2.0`,
  and `respond` / `pack_reply` to reply on the connection or to the sender's endpoint.
- Header completion lives in `didcomm-core`'s `pack` (see the findings below).

Verified live against the Indicio public mediator: mediation, a message forwarded
through it to our mediated DID, and pickup.

Not done: a WebSocket transport. The MCP server polls with `fetch_messages`, so it
isn't needed yet; it's needed only for live delivery.

### Phase 3: `documentation-server` -- done

Built as planned; see that repo's README and PLAN.md. It indexes all 50 didcomm.org
definitions and three spec versions. It answers `query`, `request` and `spec-request`,
plus discover-features and trust-ping, as a `did:peer:4` (default) or a `did:web`. It
ships as a ~150 MB container image. Its tests validate every reply against the
published `documentation/1.0` schemas.

### Phase 4: `mcp` (this repo) -- done

Built as planned: Rust and `rmcp` 3.5, over stdio, on `didcomm-agent`. The seven tools
are `get_identity`, `discover_features`, `search_protocols`,
`lookup_protocol_documentation`, `lookup_spec`, `send_didcomm_message` and
`fetch_messages`. See README.md for configuration. Details:

- **Validation.** Outgoing messages get their headers filled in *before* validation, so
  the registry's schema checks exactly what is sent. A schema is only used when the
  registry returns that exact protocol version. If no schema can be had, the result
  says so, and the message isn't blocked.
- **Untrusted content.** Peer and registry content comes back after an UNTRUSTED
  CONTENT marker naming its source. Problem reports become tool errors (`isError`),
  with the report itself marked the same way.
- **Mediation.** It starts in the background so it doesn't hold up the MCP handshake.
  A failed attempt is retried by the next tool that needs it.
- **`fetch_messages`** auto-answers trust-pings and discover-features queries among the
  messages it collects.

Found while testing against Indicio, and fixed in `didcomm-agent`: messages to the
agent's *own* mediator must come from its base DID (`Agent::did_for`). Otherwise the
mediator routes its reply back into itself. Also added: a 30 s default HTTP timeout
(`Agent::with_http_client` overrides it).

### Phase 5: end-to-end test -- done

`e2e/docker-compose.yml` runs the real documentation server, a mediator and a peer
("Bob"); the mediator and Bob are both `didcomm-peer-service`. `e2e/run.py` (standard
library Python) runs the MCP server's own container with stdin/stdout attached, as an
MCP host would, and walks through the brief's workflow over raw MCP JSON-RPC: the
handshake, mediation, discover, search and look up, a schema rejection, a validated
send and its reply, a message delivered through the mediator and fetched, and a spec
section. The MCP server also ships as a container image (`Dockerfile`).

The live Indicio path is covered by hand (README) and by `didcomm-agent`'s ignored live
test.

## Findings from probing the Indicio mediator (2026-10-01)

Probed live with this workspace's Rust stack (`did:peer:4` sender, authcrypt, HTTP,
`return_route: all`):

- trust-ping, discover-features, `coordinate-mediation/3.0` mediate-request, and
  `messagepickup/3.0` status-request all work. It discloses `discover-features/2.0`,
  `trust-ping/2.0`, `coordinate-mediation/3.0`, `messagepickup/3.0` and **`routing/3.0`**.
- **The plaintext must carry `from` and `to`** (plus `id` and `created_time`). Without
  them the mediator returns HTTP 500 with an `e.m.me` problem report.
  `didcomm-messaging-python`'s `pack` never adds them. **Fixed in `didcomm-core`:**
  `pack` now fills in any that are missing and rejects contradictory ones by default
  (`HeaderPolicy::Complete`; `HeaderPolicy::Verbatim` opts out), and `unpack` rejects a
  `from` that doesn't own the sender key. Verified live against this mediator with
  messages carrying only `type` and `body`.
- **It discloses `routing/3.0`, not `routing/2.0`, but accepts `routing/2.0` forwards**,
  which is what `didcomm-core` sends. Confirmed by `didcomm-agent`'s live test: a
  message forwarded through Indicio to our mediated DID was picked up intact. No
  `routing/3.0` support is needed for it.
