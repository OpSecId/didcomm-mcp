//! Short invitation URLs, public (no auth):
//! - `GET /invitations?_oobid=<id>`: a DIDComm v2 OOB invitation ("Short URL Message
//!   Retrieval", DIDComm Messaging v2.1).
//! - `GET /invitations/<id>`: a DIDComm v1 OOB invitation (Aries RFC 0434's short URLs).
//!
//! Asked for JSON (`Accept: application/json`), they answer with the invitation itself;
//! otherwise they redirect (302) to the long invitation URL, which a wallet that opens
//! links understands too. Unknown, expired or revoked: 404. `GET /invitations?_oob=...`
//! (a long v2 URL opened in a browser) gets a small page saying to use a wallet.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};

use crate::bridge::Bridge;

pub fn router(bridge: Arc<Bridge>) -> Router {
    Router::new()
        .route("/invitations", get(by_query))
        .route("/invitations/{id}", get(by_path))
        .with_state(bridge)
}

async fn by_query(State(bridge): State<Arc<Bridge>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    if let Some(id) = q.get("_oobid") {
        return serve(&bridge, &headers, id, "v2").await;
    }
    if q.contains_key("_oob") {
        return (
            [(header::CACHE_CONTROL, "no-store")],
            Html("<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>DIDComm invitation</title>\
                  <p style=\"font-family:sans-serif;max-width:32rem;margin:3rem auto;padding:0 1rem\">This is a DIDComm invitation. \
                  Open it with a DIDComm wallet or agent, or paste this link into one.</p>"),
        )
            .into_response();
    }
    (StatusCode::NOT_FOUND, "no invitation").into_response()
}

async fn by_path(State(bridge): State<Arc<Bridge>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    serve(&bridge, &headers, &id, "v1").await
}

async fn serve(bridge: &Bridge, headers: &HeaderMap, id: &str, version: &str) -> Response {
    let short = match bridge.short_url(id).await {
        Ok(Some(short)) if short.didcomm_version == version => short,
        Ok(_) => return (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, "no-store")], "no such invitation, or it expired").into_response(),
        Err(e) => {
            tracing::warn!("short url lookup: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let wants_json = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("application/json"));
    if wants_json {
        (
            [(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")],
            short.invitation.to_string(),
        )
            .into_response()
    } else {
        (StatusCode::FOUND, [(header::LOCATION, short.long_url.as_str()), (header::CACHE_CONTROL, "no-store")]).into_response()
    }
}
