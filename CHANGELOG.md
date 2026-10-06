# Changelog

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
