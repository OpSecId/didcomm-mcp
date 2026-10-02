# The MCP server as a container. An MCP host runs it with stdin/stdout attached:
#
#   docker run -i --rm -v didcomm-mcp:/data didcomm-mcp
#
# Optional build secrets (neither ends up in an image layer):
#   github_token -- only if a git dependency is private (wyvrn-cloud/didcomm is public):
#                   --secret id=github_token,env=GITHUB_TOKEN
#   ca_bundle    -- extra CA certificates, for building behind a TLS-intercepting proxy:
#                   --secret id=ca_bundle,src=/path/to/ca.pem
FROM rust:1-slim-bookworm AS builder

# git: cargo fetches the didcomm git dependency with the git CLI (.cargo/config.toml).
# cmake/gcc/perl: aws-lc-rs (rustls' crypto provider) builds a C library.
RUN apt-get update && apt-get install -y --no-install-recommends git cmake gcc perl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY .cargo .cargo
COPY src src

# aws-lc-rs' cmake build ignores cargo's -j; cap it so constrained builders don't OOM.
ENV CMAKE_BUILD_PARALLEL_LEVEL=2
RUN --mount=type=secret,id=github_token,required=false \
    --mount=type=secret,id=ca_bundle,required=false \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    set -e; \
    if [ -s /run/secrets/ca_bundle ]; then \
        cat /etc/ssl/certs/ca-certificates.crt /run/secrets/ca_bundle > /tmp/ca.pem; \
        export SSL_CERT_FILE=/tmp/ca.pem GIT_SSL_CAINFO=/tmp/ca.pem CARGO_HTTP_CAINFO=/tmp/ca.pem; \
    fi; \
    if [ -s /run/secrets/github_token ]; then \
        export GIT_CONFIG_COUNT=1 \
            GIT_CONFIG_KEY_0="url.https://x-access-token:$(cat /run/secrets/github_token)@github.com/.insteadOf" \
            GIT_CONFIG_VALUE_0="https://github.com/"; \
    fi; \
    cargo build --release --locked -j 2 --bin didcomm-mcp

FROM debian:bookworm-slim
# ca-certificates: HTTPS to peers, mediators and did:web documents.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --home /data didcomm \
    && mkdir /data && chown didcomm /data
COPY --from=builder /build/target/release/didcomm-mcp /usr/local/bin/didcomm-mcp
# The identity lives in a volume so the agent keeps its DID across runs.
ENV DIDCOMM_MCP_IDENTITY=/data/identity.json
VOLUME /data
USER didcomm
ENTRYPOINT ["didcomm-mcp"]
