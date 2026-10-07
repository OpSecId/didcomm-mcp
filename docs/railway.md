# Deploying on Railway

One service, plus a Postgres database. The server serves MCP at
`https://<domain>/mcp` and receives DIDComm at `https://<domain>/didcomm`, so it needs
no mediator, and keeps everything in Postgres, so it needs no volume.

## Setup

1. **New project → Deploy from GitHub repo**, this repository. `railway.toml` points the
   build at `Dockerfile.railway`.
2. **+ New → Database → PostgreSQL** in the same project.
3. On the didcomm-mcp service, **Settings → Networking → Generate Domain**. The public
   URL is derived from it.
4. **Variables** on the didcomm-mcp service:

   | Variable | Value |
   |---|---|
   | `DIDCOMM_MCP_HTTP_TOKEN` | a long random secret (`openssl rand -hex 32`) |
   | `DATABASE_URL` | `${{Postgres.DATABASE_URL}}` (a reference to the database's variable) |

   Optional:

   | Variable | Default | |
   |---|---|---|
   | `DIDCOMM_MCP_PUBLIC_URL` | `https://$RAILWAY_PUBLIC_DOMAIN` | Set for a custom domain. |
   | `DIDCOMM_MCP_DID_METHOD` | `peer` | `web`: the agent is `did:web:<host>` and serves `/.well-known/did.json`. |
   | `DIDCOMM_MCP_HTTP_ALLOWED_HOSTS` | the public URL's host | Comma-separated. |
   | `DIDCOMM_MCP_MEDIATOR_DID` | Indicio's public mediator | `""` for none: the server is reachable at its own endpoint. |
   | `DIDCOMM_MCP_V1_MEDIATOR` | `DIDCOMM_MCP_MEDIATOR_DID` | `""` for none: invitations use the server's own endpoint. |
   | `DIDCOMM_MCP_REGISTRY_DID` | `did:web:docs.wyvrn.app` | `""` disables lookups and schema checks. |

   With a mediator configured, the agent's DID is the mediated one and peers deliver
   through the mediator; the `/didcomm` endpoint still accepts messages. For a fully
   self-contained deployment, set both mediator variables to `""`.

5. Deploy. Check `https://<domain>/healthz` (→ `ok`), then connect an MCP client to
   `https://<domain>/mcp` with `Authorization: Bearer <token>`.

## Notes

- **The DID.** By default a `did:peer` naming `<public_url>/didcomm`: changing the
  domain changes it. With `DIDCOMM_MCP_DID_METHOD=web` it's `did:web:<host>`, resolved
  from `https://<host>/.well-known/did.json`, which this service serves: it survives
  endpoint changes, but it's only as trustworthy as the domain's DNS and TLS. Either
  way, pick before handing the DID out: switching changes it.
- **The database holds the private keys** (`didcomm_mcp_kv`, key `identity`), in the
  clear, as the identity file does. Don't share the database; back it up to keep the
  DID.
- **Messages sent while the service is down are lost** (the peer's delivery fails);
  a mediator would queue them.
- **`/didcomm` is unauthenticated** by design: peers can't hold the MCP token. What
  doesn't unpack as a DIDComm message for this agent is refused (400); bodies are
  capped at 1 MiB.
- **One replica.** Several replicas would share the database, but each answers
  handshakes from its own in-memory copy of the connections.
