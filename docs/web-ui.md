# Web UI

`didcomm-mcp --http` serves a web UI at `/` (the same port as `/mcp`):

| Page | What it does |
|---|---|
| **Chats** | Conversations with peers (DIDComm v2 DIDs and v1 connections): basicmessages in a messaging view, unread counts, new messages every few seconds. Start one from a connection or a pasted DID. |
| **Connections** | DIDComm v1 connections with each peer's profile; create an invitation, accept one, ask a peer for their profile. |
| **Profile** | This agent's display name, picture URL and description, shared with [User Profile 1.0](https://didcomm.org/user-profile/1.0/). "Send to all peers" pushes it to every connection and chat. |
| **System** | Health, storage, reachability, the effective configuration (secrets redacted), the DIDs. |

## Signing in

With `DIDCOMM_MCP_HTTP_TOKEN` set, the UI asks for that token once and exchanges it for
a session cookie (HttpOnly, SameSite=Strict, Secure behind HTTPS, 12 hours). Sessions
live in memory: a restart signs everyone out. The API (`/api/*`) also accepts the token
as `Authorization: Bearer ...`. A server bound to loopback without a token needs no
sign-in.

## User Profile 1.0

- A `request-profile` is answered with this agent's profile (only the `query`'d
  fields, if given), as a new instance whose `pthid` is the request.
- A `profile` is stored as that peer's (absent fields kept, `null` or `""` removed),
  and answered with ours if it says `send_back_yours`.
- Pictures are sent as an attachment linking to the URL. A peer's picture is shown
  from its `https` link or embedded base64 (images only, up to 512 KiB); anything else
  is ignored.
- The display name is also the label of invitations and DID Exchange requests made
  from the UI.
- `user-profile/1.0` is listed in discover-features.

## Conversation history

Sent and received messages, except protocol plumbing (handshakes, mediation, pickup,
pings, feature discovery, profiles), are kept in the history (`didcomm_mcp_messages` in
Postgres; `<state>.messages.json`, last 5000, with files). Receiving still queues every
message for the MCP host's `fetch_messages` too. With a mediator, the server picks up
from it every 20 seconds so the UI sees new messages.

## Development

```
cd ui
npm ci
npm run dev        # http://localhost:5173, API proxied to a server on 127.0.0.1:8090
npm run build      # ui/dist, embedded by the next cargo build
```

Without `ui/dist`, the build embeds a placeholder page saying how to build it.
`cargo run --example ui_demo` starts a local server (token `demo`, port 8095) with two
peers that chat and exchange profiles.
