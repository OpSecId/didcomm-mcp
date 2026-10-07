//! Serving MCP over Streamable HTTP (`didcomm-mcp --http`), as an alternative to stdio.
//!
//! Every HTTP session shares the one [`Bridge`] -- the same agent, DID and mediator --
//! exactly as a stdio server has one. Since anyone who can reach the endpoint acts with
//! that agent's keys, a bearer token is required unless the server only listens on
//! loopback.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    body::Bytes,
    routing::{get, post},
    Router,
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};

use crate::bridge::Bridge;
use crate::config::HttpConfig;
use crate::server::DidcommMcp;

/// Where MCP is served.
pub const MCP_PATH: &str = "/mcp";

/// Where peers deliver DIDComm messages, when `public_url` is set. Not behind the
/// bearer token: peers don't have it, and DIDComm messages are encrypted to this
/// agent and authenticated on their own.
pub const DIDCOMM_PATH: &str = "/didcomm";

/// The largest DIDComm message accepted at [`DIDCOMM_PATH`].
const MAX_DIDCOMM_BYTES: usize = 1 << 20;

/// Refuse settings that would expose the agent: a non-loopback `bind` without a token.
pub fn check(http: &HttpConfig) -> anyhow::Result<SocketAddr> {
    let addr: SocketAddr = http
        .bind
        .parse()
        .map_err(|e| anyhow::anyhow!("http bind address {:?}: {e}", http.bind))?;
    if !addr.ip().is_loopback() && http.auth_token.is_none() {
        anyhow::bail!(
            "refusing to serve on {addr} without an auth token: anyone who can reach it would act \
             with this agent's keys. Set DIDCOMM_MCP_HTTP_TOKEN (or http.auth_token), or bind to \
             a loopback address."
        );
    }
    Ok(addr)
}

/// The HTTP app: MCP at [`MCP_PATH`] (behind the bearer token, if configured) and an
/// unauthenticated `GET /healthz`.
pub fn router(bridge: Arc<Bridge>, http: &HttpConfig) -> Router {
    let inbound = bridge.config().inbound_endpoint().is_some().then(|| bridge.clone());
    let mut config = StreamableHttpServerConfig::default();
    if let Some(hosts) = &http.allowed_hosts {
        config = config.with_allowed_hosts(hosts.clone());
    }
    let service: StreamableHttpService<DidcommMcp, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(DidcommMcp::new(bridge.clone())),
        Default::default(),
        config,
    );
    let mut mcp = Router::new().nest_service(MCP_PATH, service);
    if let Some(token) = &http.auth_token {
        mcp = mcp.layer(middleware::from_fn_with_state(Arc::<str>::from(token.as_str()), require_token));
    }
    if let Some(bridge) = inbound {
        let didcomm = Router::new()
            .route(DIDCOMM_PATH, post(receive_didcomm))
            .layer(axum::extract::DefaultBodyLimit::max(MAX_DIDCOMM_BYTES))
            .with_state(bridge);
        mcp = mcp.merge(didcomm);
    }
    mcp.route("/healthz", get(|| async { "ok" }))
}

/// `POST /didcomm`: a packed DIDComm message (v1 or v2) for this agent.
async fn receive_didcomm(State(bridge): State<Arc<Bridge>>, body: Bytes) -> Response {
    match bridge.deliver(&body).await {
        Ok(Some(reply)) => reply.into_response(),
        Ok(None) => StatusCode::ACCEPTED.into_response(),
        Err(crate::bridge::BridgeError::Storage(e)) => {
            tracing::error!("couldn't queue a delivered message: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "couldn't store the message").into_response()
        }
        Err(e) => {
            tracing::info!("rejected a delivered message: {e}");
            (StatusCode::BAD_REQUEST, "not a DIDComm message for this agent").into_response()
        }
    }
}

/// Serve until `shutdown` completes.
pub async fn serve(
    bridge: Arc<Bridge>,
    http: &HttpConfig,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let addr = check(http)?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(
        url = %format!("http://{}{MCP_PATH}", listener.local_addr()?),
        auth = http.auth_token.is_some(),
        "serving MCP over Streamable HTTP"
    );
    axum::serve(listener, router(bridge, http))
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

async fn require_token(State(token): State<Arc<str>>, request: Request, next: Next) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match presented {
        Some(presented) if constant_time_eq(presented.as_bytes(), token.as_bytes()) => next.run(request).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "missing or wrong bearer token",
        )
            .into_response(),
    }
}

/// Compare without leaking, through timing, how much of the token matched.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(bind: &str, token: Option<&str>) -> HttpConfig {
        HttpConfig { bind: bind.into(), auth_token: token.map(str::to_string), allowed_hosts: None }
    }

    #[test]
    fn a_token_is_required_beyond_loopback() {
        assert!(check(&http("127.0.0.1:8090", None)).is_ok());
        assert!(check(&http("[::1]:8090", None)).is_ok());
        assert!(check(&http("0.0.0.0:8090", None)).is_err());
        assert!(check(&http("0.0.0.0:8090", Some("secret"))).is_ok());
        assert!(check(&http("not an address", None)).is_err());
    }

    #[test]
    fn token_comparison() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secret-longer"));
    }
}
