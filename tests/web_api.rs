//! The web UI's API and user-profile/1.0, over real HTTP: sign-in with the token,
//! the session cookie, profiles exchanged with a peer (v2 and v1), the conversation
//! history, and the SPA fallback.

use std::sync::Arc;

use axum::{body::Bytes, extract::State, http::StatusCode, routing::post, Router};
use didcomm_agent::v1::normalize_type;
use didcomm_agent::{Agent, Features, Identity, Received};
use didcomm_mcp::{
    bridge::{agent_for, Bridge},
    config::{Config, HttpConfig},
    http,
    profile::{self, Profile},
    store::Store,
};
use serde_json::{json, Value};

const TOKEN: &str = "ui-test-token";
const BASICMESSAGE: &str = "https://didcomm.org/basicmessage/2.0/message";
const BASICMESSAGE_V1: &str = "https://didcomm.org/basicmessage/1.0/message";

async fn listener() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (listener, url)
}

/// A peer with its own profile: answers request-profile and send_back_yours, stores
/// profiles it gets, completes DID Exchange, acks basicmessages.
struct Peer {
    agent: Agent,
    got: std::sync::Mutex<Vec<Value>>,
}

async fn start_peer() -> Arc<Peer> {
    let (listener, url) = listener().await;
    let peer = Arc::new(Peer {
        agent: Agent::with_endpoint(Identity::generate().unwrap(), &format!("{url}/"))
            .unwrap()
            .with_features(Features::standard().with_v1().with_protocol(profile::USER_PROFILE, &["sender", "receiver"])),
        got: Default::default(),
    });
    let handler = |State(peer): State<Arc<Peer>>, body: Bytes| async move {
        let received = peer.agent.receive(&body).await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        peer.got.lock().unwrap().push(received.message.clone());
        let reply = react(&peer.agent, &received).await;
        let Some(reply) = reply else { return Ok(Vec::new()) };
        let packed = peer.agent.respond(&received, &reply).await.map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
        Ok::<_, (StatusCode, String)>(packed.unwrap_or_default())
    };
    let app = Router::new().route("/", post(handler)).with_state(peer.clone());
    tokio::spawn(async move { axum::serve(listener, app).await });
    peer
}

fn bob() -> Profile {
    Profile { display_name: Some("Bob".into()), display_picture: Some("https://example.com/bob.png".into()), description: Some("peer".into()), updated: None }
}

async fn react(agent: &Agent, received: &Received) -> Option<Value> {
    if Agent::is_connection_message(received) {
        return agent.handle_connection_message(received).await.unwrap();
    }
    if let Some(reply) = agent.auto_reply(received) {
        return Some(reply);
    }
    let t = normalize_type(received.message_type());
    let version = received.version;
    let wants = (t == profile::REQUEST_PROFILE) || (t == profile::PROFILE && profile::wants_ours_back(&received.message, version));
    if wants {
        let (body, attachments) = profile::profile_message(&bob(), None, false, version);
        let mut reply = received.reply(profile::PROFILE, body);
        if let Some(a) = attachments {
            reply[if version == didcomm_agent::DidcommVersion::V2 { "attachments" } else { "~attach" }] = a;
        }
        return Some(reply);
    }
    let content = received.message["body"]["content"].as_str().or(received.message["content"].as_str()).unwrap_or_default().to_string();
    if t == BASICMESSAGE && !content.starts_with("ack") {
        return Some(received.reply(BASICMESSAGE, json!({"content": format!("ack: {content}")})));
    }
    if t == BASICMESSAGE_V1 && !content.starts_with("ack") {
        return Some(received.reply(BASICMESSAGE_V1, json!({"content": format!("ack: {content}"), "sent_time": "2026-10-07T00:00:00Z"})));
    }
    None
}

struct Server {
    base: String,
    bridge: Arc<Bridge>,
    http: reqwest::Client,
}

async fn start_server(token: Option<&str>) -> Server {
    let (listener, base) = listener().await;
    let dir = std::env::temp_dir().join(format!("didcomm-mcp-ui-{}", uuid::Uuid::new_v4()));
    let config = Config {
        identity_path: dir.join("identity.json"),
        registry_did: None,
        mediator_did: None,
        v1_mediator: None,
        state_path: dir.join("connections.json"),
        allowed_targets: None,
        validate_messages: true,
        public_url: Some(base.clone()),
        database_url: None,
        did_method: Default::default(),
        http: HttpConfig { bind: "127.0.0.1:0".into(), auth_token: token.map(str::to_string), allowed_hosts: None },
    };
    let store = Store::files(&config.identity_path, &config.state_path);
    let identity = store.load_or_generate_identity().await.unwrap();
    let agent = agent_for(identity, &config).unwrap().with_features(Features::standard().with_v1());
    let bridge = Arc::new(Bridge::with_store(agent, config.clone(), store).await.unwrap());
    let app = http::router(bridge.clone(), &config.http);
    tokio::spawn(async move { axum::serve(listener, app).await });
    Server { base, bridge, http: reqwest::Client::builder().cookie_store(true).build().unwrap() }
}

impl Server {
    async fn get(&self, path: &str) -> (u16, Value) {
        let r = self.http.get(format!("{}{path}", self.base)).send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }
    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let r = self.http.post(format!("{}{path}", self.base)).json(&body).send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }
    async fn put(&self, path: &str, body: Value) -> (u16, Value) {
        let r = self.http.put(format!("{}{path}", self.base)).json(&body).send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn sign_in_with_the_token() {
    let server = start_server(Some(TOKEN)).await;

    assert_eq!(server.get("/api/session").await.1, json!({"signed_in": false, "auth": true}));
    assert_eq!(server.get("/api/status").await.0, 401);
    assert_eq!(server.post("/api/login", json!({"token": "wrong"})).await.0, 401);
    assert_eq!(server.get("/api/status").await.0, 401);

    // The bearer token works without a session (scripts).
    let bearer = reqwest::Client::new().get(format!("{}/api/status", server.base)).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(bearer.status(), 200);

    let login = server.http.post(format!("{}/api/login", server.base)).json(&json!({"token": TOKEN})).send().await.unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"), "{cookie}");
    assert!(!cookie.contains(TOKEN), "the session id isn't the token");

    let (status, body) = server.get("/api/status").await;
    assert_eq!(status, 200);
    assert_eq!(body["storage"]["health"]["ok"], true);
    let (_, config) = server.get("/api/config").await;
    assert_eq!(config["http"]["auth_token"], "(set)");
    assert!(!config.to_string().contains(TOKEN), "no secrets in the config view");

    server.post("/api/logout", json!({})).await;
    assert_eq!(server.get("/api/status").await.0, 401);

    // The SPA: index.html for its routes, 404 for unknown API paths and files.
    let page = reqwest::get(format!("{}/chats/abc", server.base)).await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.headers()["content-type"].to_str().unwrap().starts_with("text/html"));
    assert_eq!(reqwest::get(format!("{}/api/nope", server.base)).await.unwrap().status(), 404);
    // MCP is still behind the bearer token.
    assert_eq!(reqwest::Client::new().post(format!("{}/mcp", server.base)).send().await.unwrap().status(), 401);
}

#[tokio::test]
async fn profiles_and_conversations() {
    let server = start_server(None).await;
    let peer = start_peer().await;
    let bob_did = peer.agent.did();

    // Our profile.
    let (status, _) = server.put("/api/profile", json!({"displayName": "  "})).await;
    assert_eq!(status, 200);
    let (status, err) = server.put("/api/profile", json!({"displayPicture": "javascript:alert(1)"})).await;
    assert_eq!(status, 400, "{err}");
    let (status, saved) = server.put("/api/profile", json!({"displayName": "Main agent", "displayPicture": "https://didcomm.link/me.png", "description": "hello"})).await;
    assert_eq!(status, 200);
    assert_eq!(saved["displayName"], "Main agent");
    assert_eq!(server.get("/api/profile").await.1["description"], "hello");

    // Share it, asking for Bob's back: Bob receives ours, his comes back.
    let (status, sent) = server.post("/api/profile/share", json!({"target": bob_did, "send_back_yours": true})).await;
    assert_eq!(status, 200, "{sent}");
    let ours = peer.got.lock().unwrap().iter().find(|m| m["type"] == profile::PROFILE).cloned().unwrap();
    assert_eq!(ours["body"]["profile"]["displayName"], "Main agent");
    assert_eq!(ours["body"]["send_back_yours"], true);
    assert_eq!(ours["attachments"][0]["data"]["links"][0], "https://didcomm.link/me.png");
    let applied = profile::apply_profile(None, &ours, didcomm_agent::DidcommVersion::V2, 0);
    assert_eq!(applied.display_picture.as_deref(), Some("https://didcomm.link/me.png"));

    // Bob's answer arrives at our endpoint (he sends to our DID).
    let profiles = server.bridge.peer_profiles().await.unwrap();
    let got = profiles.get(&bob_did).unwrap_or_else(|| panic!("no profile for bob: {profiles:?}"));
    assert_eq!(got.display_name.as_deref(), Some("Bob"));
    assert_eq!(got.display_picture.as_deref(), Some("https://example.com/bob.png"));

    // Bob asks for ours: answered on its own.
    let ask = json!({"id": "ask-1", "type": profile::REQUEST_PROFILE, "body": {"query": ["displayName"]}, "return_route": "all"});
    let answer = peer.agent.request(&server.bridge.agent().did(), &ask).await.unwrap();
    assert_eq!(answer.message_type(), profile::PROFILE);
    assert_eq!(answer.message["body"]["profile"], json!({"displayName": "Main agent"}));
    assert_eq!(answer.message["pthid"], "ask-1", "a new instance, its parent the request");

    // A chat: we send, Bob acks to our DID; Bob writes on his own.
    let (status, sent) = server.post("/api/messages", json!({"target": bob_did, "content": "hi bob"})).await;
    assert_eq!(status, 200, "{sent}");
    peer.agent.send(&server.bridge.agent().did(), &json!({"type": BASICMESSAGE, "body": {"content": "ack from bob"}})).await.unwrap();

    let (_, conversations) = server.get("/api/conversations").await;
    let list = conversations.as_array().unwrap();
    assert_eq!(list.len(), 1, "profiles and pings aren't conversations: {conversations}");
    let c = &list[0];
    assert_eq!(c["peer"], bob_did.as_str());
    assert_eq!(c["profile"]["displayName"], "Bob");
    assert!(c["unread"].as_i64().unwrap() >= 1, "{c}");

    let (_, history) = server.get(&format!("/api/messages?peer={}", urlencode(&bob_did))).await;
    let contents: Vec<(String, String)> = history
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["direction"].as_str().unwrap().to_string(), e["entry"]["message"]["body"]["content"].as_str().unwrap().to_string()))
        .collect();
    assert!(contents.contains(&("out".into(), "hi bob".into())), "{contents:?}");
    assert!(contents.contains(&("in".into(), "ack: hi bob".into())), "{contents:?}");
    assert!(contents.contains(&("in".into(), "ack from bob".into())), "{contents:?}");
    let last = history.as_array().unwrap().last().unwrap()["id"].as_i64().unwrap();

    let (_, newer) = server.get(&format!("/api/messages?peer={}&after={last}", urlencode(&bob_did))).await;
    assert_eq!(newer, json!([]));
    server.post("/api/read", json!({"peer": bob_did, "upto": last})).await;
    assert_eq!(server.get("/api/conversations").await.1[0]["unread"], 0);

    // Messages also wait for fetch_messages (the MCP side).
    let fetched = server.bridge.fetch(50).await.unwrap();
    assert!(fetched["messages"].as_array().unwrap().iter().any(|m| m["message"]["body"]["content"] == "ack from bob"), "{fetched}");
}

#[tokio::test]
async fn v1_connection_with_profile_and_chat() {
    let server = start_server(None).await;
    let peer = start_peer().await;
    server.put("/api/profile", json!({"displayName": "Main agent"})).await;

    // Bob accepts our invitation: the label is our display name.
    let (_, invitation) = server.post("/api/invitations", json!({})).await;
    assert_eq!(invitation["invitation"]["label"], "Main agent");
    let url = invitation["invitation_url"].as_str().unwrap();
    let theirs = peer.agent.accept_invitation(&peer.agent.fetch_invitation(url).await.unwrap(), "Bob").await.unwrap();

    let (_, connections) = server.get("/api/connections").await;
    let id = connections[0]["id"].as_str().unwrap().to_string();
    assert_eq!(id, theirs.id);

    // Ask Bob for his profile over the v1 connection.
    let (status, sent) = server.post("/api/profile/request", json!({"target": id})).await;
    assert_eq!(status, 200, "{sent}");
    assert_eq!(sent["sent"]["didcomm_version"], "v1");
    let (_, connections) = server.get("/api/connections").await;
    assert_eq!(connections[0]["profile"]["displayName"], "Bob", "{connections}");

    // A v1 chat on the connection.
    let (status, sent) = server.post("/api/messages", json!({"target": id, "content": "hi v1"})).await;
    assert_eq!(status, 200, "{sent}");
    assert_eq!(sent["sent"]["type"], BASICMESSAGE_V1);
    let (_, history) = server.get(&format!("/api/messages?peer={id}")).await;
    let contents: Vec<&str> = history.as_array().unwrap().iter().filter_map(|e| e["entry"]["message"]["content"].as_str()).collect();
    assert_eq!(contents, ["hi v1", "ack: hi v1"], "{history}");

    // A send that fails leaves nothing behind in the history.
    let (status, _) = server.post("/api/messages", json!({"target": "did:peer:2.Ez6LSbogus", "content": "lost"})).await;
    assert_ne!(status, 200);
    assert_eq!(server.get("/api/messages?peer=did%3Apeer%3A2.Ez6LSbogus").await.1, json!([]));
}

#[tokio::test]
async fn short_invitation_urls() {
    let server = start_server(None).await;
    let peer = start_peer().await;
    server.put("/api/profile", json!({"displayName": "Main agent"})).await;
    let no_redirects = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    // DIDComm v2: OOB 2.0 from our DID, short URL with _oobid (DIDComm v2.1 spec).
    let (status, v2) = server.post("/api/invitations", json!({"didcomm_version": "v2"})).await;
    assert_eq!(status, 200, "{v2}");
    let id = v2["id"].as_str().unwrap();
    let short = v2["short_url"].as_str().unwrap();
    assert_eq!(short, format!("{}/invitations?_oobid={id}", server.base));
    assert!(v2["invitation_url"].as_str().unwrap().starts_with(&format!("{}/invitations?_oob=", server.base)));
    assert_eq!(v2["invitation"]["type"], "https://didcomm.org/out-of-band/2.0/invitation");
    assert_eq!(v2["invitation"]["from"], server.bridge.agent().did().as_str());
    assert_eq!(v2["invitation"]["body"]["label"], "Main agent");
    let expires = v2["expires_time"].as_i64().unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    assert!((expires - now - 7 * 24 * 3600).abs() < 60, "default 7 days");
    assert!(short.len() < 100 && v2["invitation_url"].as_str().unwrap().len() > short.len() * 3);

    // GET with Accept: application/json → the invitation; otherwise 302 → the long URL.
    let json_answer: Value = no_redirects.get(short).header("accept", "application/json").send().await.unwrap().json().await.unwrap();
    assert_eq!(json_answer, v2["invitation"]);
    let redirect = no_redirects.get(short).send().await.unwrap();
    assert_eq!(redirect.status(), 302);
    assert_eq!(redirect.headers()["location"], v2["invitation_url"].as_str().unwrap());
    // No auth needed, and the long v2 URL in a browser gets a page, not a 404.
    assert_eq!(reqwest::get(v2["invitation_url"].as_str().unwrap()).await.unwrap().status(), 200);

    // A peer accepts through the short URL alone (the library follows it), then messages us.
    let invitation = peer.agent.fetch_invitation(short).await.unwrap();
    let connection = peer.agent.accept_invitation(&invitation, "Bob").await.unwrap();
    assert_eq!(connection.their_did.as_deref(), Some(server.bridge.agent().did().as_str()));
    peer.agent.send(&server.bridge.agent().did(), &json!({"type": BASICMESSAGE, "body": {"content": "via short url"}})).await.unwrap();
    let (_, conversations) = server.get("/api/conversations").await;
    assert!(conversations.to_string().contains("via short url"), "{conversations}");

    // DIDComm v1: OOB 1.1 for DID Exchange, short URL as a path (RFC 0434).
    let (_, v1) = server.post("/api/invitations", json!({"validity_seconds": 0})).await;
    let short1 = v1["short_url"].as_str().unwrap();
    assert_eq!(short1, format!("{}/invitations/{}", server.base, v1["id"].as_str().unwrap()));
    assert_eq!(v1["expires_time"], Value::Null, "0: until revoked");
    assert!(v1["invitation_url"].as_str().unwrap().contains("/didcomm?oob="));
    let theirs = peer.agent.accept_invitation(&peer.agent.fetch_invitation(short1).await.unwrap(), "Bob").await.unwrap();
    assert_eq!(theirs.didcomm_version, didcomm_agent::DidcommVersion::V1);
    // The forms don't cross: a v1 id isn't served as an _oobid.
    let crossed = format!("{}/invitations?_oobid={}", server.base, v1["id"].as_str().unwrap());
    assert_eq!(reqwest::get(&crossed).await.unwrap().status(), 404);

    // Listed; revoking stops it resolving at once.
    let (_, list) = server.get("/api/invitations").await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert!(list.as_array().unwrap().iter().all(|s| s["live"] == true));
    let (status, _) = server.post("/api/invitations/revoke", json!({"id": id})).await;
    assert_eq!(status, 200);
    assert_eq!(no_redirects.get(short).send().await.unwrap().status(), 404);
    assert_eq!(server.post("/api/invitations/revoke", json!({"id": id})).await.0, 400, "already revoked");
    let (_, list) = server.get("/api/invitations").await;
    assert_eq!(list.as_array().unwrap().iter().find(|s| s["id"] == id).unwrap()["live"], false);

    // Limits and bad input.
    assert_eq!(server.post("/api/invitations", json!({"validity_seconds": 91 * 24 * 3600})).await.0, 400);
    assert_eq!(server.post("/api/invitations", json!({"didcomm_version": "v3"})).await.0, 400);
    assert_eq!(reqwest::get(format!("{}/invitations?_oobid=nope", server.base)).await.unwrap().status(), 404);
    assert_eq!(reqwest::get(format!("{}/invitations/nope", server.base)).await.unwrap().status(), 404);
}

#[tokio::test]
async fn expired_short_urls_stop_resolving() {
    use didcomm_mcp::store::{ShortUrl, Store};
    let dir = std::env::temp_dir().join(format!("didcomm-mcp-short-{}", uuid::Uuid::new_v4()));
    let store = Store::files(&dir.join("identity.json"), &dir.join("state.json"));
    let short = |id: &str, expires_at| ShortUrl {
        id: id.into(),
        didcomm_version: "v2".into(),
        invitation: json!({}),
        long_url: "https://x/invitations?_oob=e30".into(),
        created_at: 100,
        expires_at,
        revoked: false,
    };
    store.put_short_url(&short("old", Some(150)), 120).await.unwrap();
    store.put_short_url(&short("forever", None), 120).await.unwrap();
    assert!(store.get_short_url("old").await.unwrap().unwrap().is_live(149));
    assert!(!store.get_short_url("old").await.unwrap().unwrap().is_live(150));
    // Storing another after it expired drops it.
    store.put_short_url(&short("new", Some(1000)), 200).await.unwrap();
    assert_eq!(store.get_short_url("old").await.unwrap(), None);
    assert!(store.get_short_url("forever").await.unwrap().unwrap().is_live(i64::MAX - 1));
    // Kept across a restart.
    let reopened = Store::files(&dir.join("identity.json"), &dir.join("state.json"));
    assert_eq!(reopened.list_short_urls().await.unwrap().len(), 2);
}

/// The same lifecycle on Postgres (skipped without TEST_DATABASE_URL).
#[tokio::test]
async fn short_urls_on_postgres() {
    use didcomm_mcp::store::{ShortUrl, Store};
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL isn't set");
        return;
    };
    let store = Store::postgres(&url).await.unwrap();
    let tag = uuid::Uuid::new_v4().to_string();
    let short = |id: String, expires_at| ShortUrl {
        id,
        didcomm_version: "v2".into(),
        invitation: json!({"type": "https://didcomm.org/out-of-band/2.0/invitation", "body": {"label": "pg"}}),
        long_url: "https://x/invitations?_oob=e30".into(),
        created_at: 100,
        expires_at,
        revoked: false,
    };
    let (old, live) = (format!("old-{tag}"), format!("live-{tag}"));
    store.put_short_url(&short(old.clone(), Some(150)), 120).await.unwrap();
    store.put_short_url(&short(live.clone(), None), 120).await.unwrap();
    let got = store.get_short_url(&live).await.unwrap().unwrap();
    assert_eq!(got.invitation["body"]["label"], "pg");
    assert!(store.list_short_urls().await.unwrap().iter().any(|s| s.id == live));
    assert!(store.revoke_short_url(&live).await.unwrap());
    assert!(!store.revoke_short_url(&live).await.unwrap(), "already revoked");
    assert!(!store.get_short_url(&live).await.unwrap().unwrap().is_live(0));
    // A later insert sweeps the expired one.
    store.put_short_url(&short(format!("new-{tag}"), None), 200).await.unwrap();
    assert_eq!(store.get_short_url(&old).await.unwrap(), None);
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}
