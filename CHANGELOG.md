# Changelog

## Unreleased

- Receiving without a mediator: with `public_url` set, `--http` accepts DIDComm
  messages at `/didcomm` and the agent's DID names that endpoint. Messages are queued
  for `fetch_messages`; trust-pings and DID Exchange are answered when they arrive;
  `create_invitation` uses the endpoint when there's no v1 mediator.
- Postgres storage: with `database_url` (or `DATABASE_URL`), the identity, state and
  inbox are kept in the database instead of files.
- `did_method = "web"`: the agent is `did:web:<public_url host[:path]>` and serves its
  own DID document (`/.well-known/did.json`, or `/did.json` under a path). Default
  stays `peer`.
- `get_identity` reports the `endpoint`, the `storage` and the `did_method` in use.
- Railway: `Dockerfile.railway`, `railway-start.sh`, `railway.toml`; see
  [docs/railway.md](docs/railway.md).

## 0.1.1 - 2026-10-07

- Fixed: a build against the current `didcomm` failed every registry call against
  `did:web:docs.wyvrn.app` with `invalid COSE structure: COSE_Encrypt is not an array`.
  The registry, on an earlier `didcomm`, answered in that version's CBOR layout. The
  release binaries of 0.1.0 weren't affected; builds without `--locked` were.
  ([wyvrn-cloud/didcomm#10](https://github.com/wyvrn-cloud/didcomm/pull/10): the
  older layout is read again, and the encoding follows the order of a peer's `accept`
  list.)
- This agent now prefers `didcomm/v2+cbor` (COSE) in its DID document; peers without
  it keep using JSON.
- `didcomm` is pinned to a commit in `Cargo.toml`, so a build gets the tested version
  with or without `--locked`.
- Failed tool calls are logged (a warning; a peer's problem report at info), so they
  show up in the service's log, not only in the MCP host.
- [docs/building.md](docs/building.md): building from source, `cargo install`, static
  builds.

## 0.1.0 - 2026-10-06

The first release.

- An MCP server with a fixed set of ten tools for DIDComm: discover a peer's protocols,
  look protocols and the spec up in a documentation registry (`documentation/1.1`,
  falling back to 1.0), and send and receive their messages. Outgoing messages are
  checked against the registry's JSON Schemas.
- DIDComm v2, and DIDComm v1 (Aries) over connections made from out-of-band
  invitations with DID Exchange (`accept_invitation`, `create_invitation`,
  `list_connections`).
- Mediation for both: messages to this agent wait at a mediator until
  `fetch_messages` picks them up. By default it uses Indicio's public mediator and the
  registry at `did:web:docs.wyvrn.app`.
- For credential protocols, the registry's attachment formats (AnonCreds, Indy, JSON-LD,
  Data Integrity, SD-JWT, DIF Presentation Exchange, ...) with schemas.
- MCP over stdio, or Streamable HTTP (`--http`) with a bearer token and `Host` checks.
- `didcomm-mcp service install`: runs it as a systemd, launchd or Windows service,
  with a generated configuration and token.
- Release builds for Linux (x86_64, ARM64; static), macOS (Apple silicon, Intel) and
  Windows (x64, ARM64).
