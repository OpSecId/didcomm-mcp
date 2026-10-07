//! The web UI's JSON API, under `/api`. Signing in exchanges the HTTP bearer token
//! (`DIDCOMM_MCP_HTTP_TOKEN`) for a session cookie (HttpOnly, SameSite=Strict); every
//! other endpoint needs that session, or the bearer token itself. Without a token
//! configured (a loopback-only server) no sign-in is needed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    extract::{Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::bridge::{Bridge, BridgeError, Outgoing};
use crate::profile::Profile;

const COOKIE: &str = "dmcp_session";
const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const BASICMESSAGE_V2: &str = "https://didcomm.org/basicmessage/2.0/message";
const BASICMESSAGE_V1: &str = "https://didcomm.org/basicmessage/1.0/message";

#[derive(Clone)]
struct Api {
    bridge: Arc<Bridge>,
    token: Option<Arc<str>>,
    sessions: Arc<Mutex<HashMap<String, Instant>>>,
    /// Mark the cookie Secure (the public URL is HTTPS).
    secure: bool,
}

/// The `/api` routes.
pub fn router(bridge: Arc<Bridge>) -> Router {
    let api = Api {
        token: bridge.config().http.auth_token.as_deref().map(Arc::from),
        secure: bridge.config().public_url.as_deref().is_some_and(|u| u.starts_with("https://")),
        sessions: Arc::new(Mutex::new(HashMap::new())),
        bridge,
    };
    let protected = Router::new()
        .route("/api/identity", get(identity))
        .route("/api/profile", get(get_profile).put(put_profile))
        .route("/api/profile/share", post(share_profile))
        .route("/api/profile/request", post(request_profile))
        .route("/api/connections", get(connections))
        .route("/api/connections/accept", post(accept_invitation))
        .route("/api/invitations", get(list_invitations).post(create_invitation))
        .route("/api/invitations/revoke", post(revoke_invitation))
        .route("/api/conversations", get(conversations))
        .route("/api/messages", get(messages).post(send_message))
        .route("/api/read", post(mark_read))
        .route("/api/refresh", post(refresh))
        .route("/api/status", get(status))
        .route("/api/config", get(config))
        .route_layer(middleware::from_fn_with_state(api.clone(), require_session));
    Router::new()
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/session", get(session))
        .merge(protected)
        .with_state(api)
}

impl Api {
    fn session_of(&self, headers: &HeaderMap) -> bool {
        let Some(token) = &self.token else { return true };
        if let Some(bearer) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) {
            if constant_time_eq(bearer.as_bytes(), token.as_bytes()) {
                return true;
            }
        }
        let Some(id) = cookie(headers, COOKIE) else { return false };
        let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
        sessions.retain(|_, created| created.elapsed() < SESSION_TTL);
        sessions.contains_key(&id)
    }

    fn cookie_header(&self, value: &str, max_age: u64) -> String {
        let secure = if self.secure { "; Secure" } else { "" };
        format!("{COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}")
    }
}

async fn require_session(State(api): State<Api>, request: Request, next: Next) -> Response {
    if api.session_of(request.headers()) {
        next.run(request).await
    } else {
        (StatusCode::UNAUTHORIZED, Json(json!({"error": "sign in first"}))).into_response()
    }
}

#[derive(Deserialize)]
struct Login {
    token: String,
}

async fn login(State(api): State<Api>, Json(login): Json<Login>) -> Response {
    let Some(token) = &api.token else {
        return Json(json!({"ok": true, "auth": false})).into_response();
    };
    if !constant_time_eq(login.token.trim().as_bytes(), token.as_bytes()) {
        // Slow down guessing.
        tokio::time::sleep(Duration::from_millis(750)).await;
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "wrong token"}))).into_response();
    }
    let id = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    api.sessions.lock().expect("sessions lock poisoned").insert(id.clone(), Instant::now());
    ([(header::SET_COOKIE, api.cookie_header(&id, SESSION_TTL.as_secs()))], Json(json!({"ok": true, "auth": true}))).into_response()
}

async fn logout(State(api): State<Api>, headers: HeaderMap) -> Response {
    if let Some(id) = cookie(&headers, COOKIE) {
        api.sessions.lock().expect("sessions lock poisoned").remove(&id);
    }
    ([(header::SET_COOKIE, api.cookie_header("", 0))], Json(json!({"ok": true}))).into_response()
}

async fn session(State(api): State<Api>, headers: HeaderMap) -> Json<Value> {
    Json(json!({"signed_in": api.session_of(&headers), "auth": api.token.is_some()}))
}

/// A bridge error as an HTTP response: the caller's mistakes as 400, the rest 502.
fn error(e: BridgeError) -> Response {
    let status = match e {
        BridgeError::BadArgument(_) | BridgeError::NotAllowed(_) | BridgeError::Invalid { .. } | BridgeError::WrongDidcommVersion { .. } => {
            StatusCode::BAD_REQUEST
        }
        BridgeError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_GATEWAY,
    };
    (status, Json(json!({"error": e.to_string()}))).into_response()
}

fn ok(result: Result<Value, BridgeError>) -> Response {
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => error(e),
    }
}

async fn identity(State(api): State<Api>) -> Json<Value> {
    Json(api.bridge.identity().await)
}

async fn get_profile(State(api): State<Api>) -> Response {
    ok(api.bridge.profile().await.map(|p| json!(p)))
}

async fn put_profile(State(api): State<Api>, Json(profile): Json<Profile>) -> Response {
    ok(api.bridge.set_profile(profile).await.map(|p| json!(p)))
}

#[derive(Deserialize)]
struct Target {
    target: String,
    #[serde(default)]
    send_back_yours: bool,
}

async fn share_profile(State(api): State<Api>, Json(t): Json<Target>) -> Response {
    ok(api.bridge.share_profile(&t.target, t.send_back_yours).await)
}

async fn request_profile(State(api): State<Api>, Json(t): Json<Target>) -> Response {
    ok(api.bridge.request_profile(&t.target).await)
}

async fn connections(State(api): State<Api>) -> Response {
    let profiles = match api.bridge.peer_profiles().await {
        Ok(p) => p,
        Err(e) => return error(e),
    };
    let mut list = api.bridge.connections();
    for c in list.as_array_mut().into_iter().flatten() {
        let profile = c["id"].as_str().and_then(|id| profiles.get(id)).or_else(|| c["their_did"].as_str().and_then(|d| profiles.get(d)));
        c["profile"] = json!(profile);
    }
    Json(list).into_response()
}

/// The label this agent introduces itself with: its display name, if it has one.
async fn label(api: &Api) -> Option<String> {
    api.bridge.profile().await.ok().and_then(|p| p.display_name)
}

#[derive(Deserialize)]
struct Accept {
    invitation: String,
}

async fn accept_invitation(State(api): State<Api>, Json(a): Json<Accept>) -> Response {
    let label = label(&api).await;
    ok(api.bridge.accept_invitation(&a.invitation, label.as_deref()).await.map(|c| crate::bridge::connection_json(&c)))
}

#[derive(Deserialize, Default)]
struct NewInvitation {
    /// `"v1"` (default) or `"v2"`.
    #[serde(default)]
    didcomm_version: Option<String>,
    /// How long the short URL lives (default 7 days; 0: until revoked).
    #[serde(default)]
    validity_seconds: Option<u64>,
}

async fn create_invitation(State(api): State<Api>, body: Option<Json<NewInvitation>>) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let version = match body.didcomm_version.as_deref() {
        None | Some("v1") => didcomm_agent::DidcommVersion::V1,
        Some("v2") => didcomm_agent::DidcommVersion::V2,
        Some(other) => return error(BridgeError::BadArgument(format!("didcomm_version must be v1 or v2, not {other}"))),
    };
    let label = label(&api).await;
    ok(api.bridge.create_invitation_with(label.as_deref(), version, body.validity_seconds).await)
}

async fn list_invitations(State(api): State<Api>) -> Response {
    ok(api.bridge.short_urls().await)
}

#[derive(Deserialize)]
struct Revoke {
    id: String,
}

async fn revoke_invitation(State(api): State<Api>, Json(r): Json<Revoke>) -> Response {
    ok(api.bridge.revoke_short_url(&r.id).await.map(|_| json!({"ok": true})))
}

async fn conversations(State(api): State<Api>) -> Response {
    ok(api.bridge.conversations().await)
}

#[derive(Deserialize)]
struct MessagesQuery {
    peer: String,
    before: Option<i64>,
    after: Option<i64>,
    limit: Option<i64>,
}

async fn messages(State(api): State<Api>, Query(q): Query<MessagesQuery>) -> Response {
    ok(api.bridge.history(&q.peer, q.before, q.after, q.limit.unwrap_or(100)).await)
}

#[derive(Deserialize)]
struct Send {
    target: String,
    content: String,
}

/// A basicmessage: 2.0 to a DID or a v2 connection, 1.0 to a v1 connection.
async fn send_message(State(api): State<Api>, Json(s): Json<Send>) -> Response {
    let content = s.content.trim();
    if content.is_empty() {
        return error(BridgeError::BadArgument("the message is empty".into()));
    }
    let v1 = api.bridge.agent().connection(&s.target).is_some_and(|c| c.didcomm_version == didcomm_agent::DidcommVersion::V1);
    let (message_type, body) = if v1 {
        (BASICMESSAGE_V1, json!({"content": content, "sent_time": rfc3339_now()}))
    } else {
        (BASICMESSAGE_V2, json!({"content": content}))
    };
    let outgoing = Outgoing {
        target_did: s.target,
        message_type: message_type.into(),
        body,
        validate: true,
        ..Default::default()
    };
    ok(api.bridge.send(outgoing).await)
}

#[derive(Deserialize)]
struct Read {
    peer: String,
    upto: i64,
}

async fn mark_read(State(api): State<Api>, Json(r): Json<Read>) -> Response {
    ok(api.bridge.mark_read(&r.peer, r.upto).await.map(|_| json!({"ok": true})))
}

/// Pick up from the mediators now.
async fn refresh(State(api): State<Api>) -> Response {
    ok(api.bridge.collect(50).await.map(|problems| json!({"problems": problems})))
}

async fn status(State(api): State<Api>) -> Json<Value> {
    Json(api.bridge.status().await)
}

async fn config(State(api): State<Api>) -> Json<Value> {
    Json(api.bridge.config_view())
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// Now as RFC 3339 (UTC, seconds), for basicmessage/1.0's `sent_time`.
fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_and_dates() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "a=1; dmcp_session=abc; b=2".parse().unwrap());
        assert_eq!(cookie(&headers, COOKIE).as_deref(), Some("abc"));
        let now = rfc3339_now();
        assert_eq!(now.len(), 20, "{now}");
        assert!(now.starts_with("20") && now.ends_with('Z'));
    }
}
