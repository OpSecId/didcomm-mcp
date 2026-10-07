#!/bin/sh
# Railway entrypoint: derive the bind address, public URL and allowed hosts from
# Railway's variables unless set explicitly, then serve MCP (and DIDComm) over HTTP.
set -eu

export DIDCOMM_MCP_HTTP_BIND="0.0.0.0:${PORT:-8090}"

if [ -z "${DIDCOMM_MCP_PUBLIC_URL:-}" ] && [ -n "${RAILWAY_PUBLIC_DOMAIN:-}" ]; then
    export DIDCOMM_MCP_PUBLIC_URL="https://${RAILWAY_PUBLIC_DOMAIN}"
fi

if [ -z "${DIDCOMM_MCP_HTTP_ALLOWED_HOSTS:-}" ] && [ -n "${DIDCOMM_MCP_PUBLIC_URL:-}" ]; then
    # The host part of the public URL.
    host="${DIDCOMM_MCP_PUBLIC_URL#*://}"
    export DIDCOMM_MCP_HTTP_ALLOWED_HOSTS="${host%%/*}"
fi

if [ -z "${DATABASE_URL:-}${DIDCOMM_MCP_DATABASE_URL:-}" ]; then
    echo "note: no DATABASE_URL; keeping the identity and state under /data (mount a volume there to keep them)" >&2
fi

exec didcomm-mcp --http
