//! A local demo for the web UI: the server (did:web off, own endpoint, file store,
//! token "demo") on 127.0.0.1:8095, and two peers -- "Alice" (DIDComm v2, by DID) and
//! "Bob" (DIDComm v1 connection) -- that answer profiles and chat back. Runs until
//! killed. `cargo run --example ui_demo` (after `npm run build` in ui/).

use std::sync::Arc;
use std::time::Duration;

use axum::{body::Bytes, extract::State, http::StatusCode, routing::post, Router};
use didcomm_agent::v1::normalize_type;
use didcomm_agent::{Agent, DidcommVersion, Features, Identity, Received};
use didcomm_mcp::{
    bridge::{agent_for, Bridge},
    config::{Config, HttpConfig},
    http,
    profile::{self, Profile},
    store::Store,
};
use serde_json::{json, Value};

const BM2: &str = "https://didcomm.org/basicmessage/2.0/message";
const BM1: &str = "https://didcomm.org/basicmessage/1.0/message";

struct Peer {
    agent: Agent,
    profile: Profile,
}

async fn start_peer(port: u16, profile: Profile) -> Arc<Peer> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let url = format!("http://127.0.0.1:{port}/");
    let peer = Arc::new(Peer {
        agent: Agent::with_endpoint(Identity::generate().unwrap(), &url)
            .unwrap()
            .with_features(Features::standard().with_v1().with_protocol(profile::USER_PROFILE, &["sender", "receiver"])),
        profile,
    });
    let handler = |State(peer): State<Arc<Peer>>, body: Bytes| async move {
        let received = peer.agent.receive(&body).await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        let Some(reply) = react(&peer, &received).await else { return Ok(Vec::new()) };
        let packed = peer.agent.respond(&received, &reply).await.map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
        Ok::<_, (StatusCode, String)>(packed.unwrap_or_default())
    };
    let app = Router::new().route("/", post(handler)).with_state(peer.clone());
    tokio::spawn(async move { axum::serve(listener, app).await });
    peer
}

async fn react(peer: &Peer, received: &Received) -> Option<Value> {
    if Agent::is_connection_message(received) {
        return peer.agent.handle_connection_message(received).await.ok().flatten();
    }
    if let Some(reply) = peer.agent.auto_reply(received) {
        return Some(reply);
    }
    let t = normalize_type(received.message_type());
    let v = received.version;
    if t == profile::REQUEST_PROFILE || (t == profile::PROFILE && profile::wants_ours_back(&received.message, v)) {
        let (body, attachments) = profile::profile_message(&peer.profile, None, false, v);
        let mut reply = received.reply(profile::PROFILE, body);
        if let Some(a) = attachments {
            reply[if v == DidcommVersion::V2 { "attachments" } else { "~attach" }] = a;
        }
        return Some(reply);
    }
    let content = received.message["body"]["content"].as_str().or(received.message["content"].as_str()).unwrap_or_default();
    let answer = format!("Got it: “{content}” 👍");
    match t.as_str() {
        BM2 => Some(received.reply(BM2, json!({"content": answer}))),
        BM1 => Some(received.reply(BM1, json!({"content": answer, "sent_time": "2026-10-07T12:00:00Z"}))),
        _ => None,
    }
}

#[tokio::main]
async fn main() {
    let base = "http://127.0.0.1:8095";
    let dir = std::env::temp_dir().join(format!("didcomm-mcp-ui-demo-{}", uuid::Uuid::new_v4()));
    let config = Config {
        identity_path: dir.join("identity.json"),
        registry_did: None,
        mediator_did: None,
        v1_mediator: None,
        state_path: dir.join("connections.json"),
        allowed_targets: None,
        validate_messages: true,
        public_url: Some(base.into()),
        database_url: None,
        did_method: Default::default(),
        http: HttpConfig { bind: "127.0.0.1:8095".into(), auth_token: Some("demo".into()), allowed_hosts: None },
    };
    let store = Store::files(&config.identity_path, &config.state_path);
    let identity = store.load_or_generate_identity().await.unwrap();
    let agent = agent_for(identity, &config).unwrap().with_features(Features::standard().with_v1().with_protocol(profile::USER_PROFILE, &["sender", "receiver"]));
    let bridge = Arc::new(Bridge::with_store(agent, config.clone(), store).await.unwrap());
    let bind = std::env::var("UI_DEMO_BIND").unwrap_or_else(|_| "127.0.0.1:8095".into());
    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap();
    let app = http::router(bridge.clone(), &config.http);
    tokio::spawn(async move { axum::serve(listener, app).await });

    bridge
        .set_profile(Profile {
            display_name: Some("OpSecId Main Agent".into()),
            display_picture: None,
            description: Some("The main DIDComm agent at agent.didcomm.link.".into()),
            updated: None,
        })
        .await
        .unwrap();

    let alice = start_peer(8096, Profile { display_name: Some("Alice Martin".into()), description: Some("Wallet · DIDComm v2".into()), ..Default::default() }).await;
    let bob = start_peer(8097, Profile { display_name: Some("Bob's Issuer".into()), description: Some("An Aries agent (DIDComm v1)".into()), ..Default::default() }).await;
    let me = bridge.agent().did();

    // Alice (v2): profiles both ways, then a conversation.
    bridge.share_profile(&alice.agent.did(), true).await.unwrap();
    for text in ["Hi! Are you the main agent?", "Can you verify my membership credential later today?"] {
        alice.agent.send(&me, &json!({"type": BM2, "body": {"content": text}})).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    bridge
        .send(didcomm_mcp::bridge::Outgoing { target_did: alice.agent.did(), message_type: BM2.into(), body: json!({"content": "Yes — send it over whenever you're ready."}), validate: true, ..Default::default() })
        .await
        .unwrap();

    // Bob (v1): connects through our invitation, profiles, a message.
    let invitation = bridge.create_invitation(Some("OpSecId Main Agent")).await.unwrap();
    let url = invitation["invitation_url"].as_str().unwrap();
    let conn = bob.agent.accept_invitation(&bob.agent.fetch_invitation(url).await.unwrap(), "Bob's Issuer").await.unwrap();
    bridge.request_profile(&conn.id).await.unwrap();
    bob.agent.send(&conn.id, &json!({"@type": BM1, "content": "Connection established. Ready to issue.", "sent_time": "2026-10-07T12:00:00Z"})).await.unwrap();

    println!("READY {base} token=demo alice={} bob_connection={}", alice.agent.did(), conn.id);
    // Keep chatting so the UI shows new messages arriving.
    let mut n = 0;
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        n += 1;
        let _ = alice.agent.send(&me, &json!({"type": BM2, "body": {"content": format!("ping #{n} from Alice")}})).await;
    }
}
