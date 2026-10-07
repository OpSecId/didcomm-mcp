//! Receiving without a mediator: peers deliver to the server's own `/didcomm` endpoint
//! (`public_url`), and `fetch_messages` collects from the inbox. Run against the file
//! store, and against Postgres when `TEST_DATABASE_URL` is set (else that test is a
//! no-op that says so).

use std::path::PathBuf;
use std::sync::Arc;

use axum::{body::Bytes, extract::State, http::StatusCode, routing::post, Router};
use didcomm_agent::v1::normalize_type;
use didcomm_agent::{features, Agent, Features, Identity, Received};
use didcomm_mcp::{
    bridge::Bridge,
    config::{Config, HttpConfig},
    http,
    server::DidcommMcp,
    store::Store,
};
use rmcp::{
    model::{CallToolRequestParams, CallToolResult, ClientConfig},
    service::RunningService,
    ClientHandler, RoleClient, ServiceExt,
};
use serde_json::{json, Value};

const BASICMESSAGE: &str = "https://didcomm.org/basicmessage/2.0/message";
const BASICMESSAGE_V1: &str = "https://didcomm.org/basicmessage/1.0/message";

async fn listener() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (listener, url)
}

/// A directly reachable peer that answers trust-pings, completes DID Exchange (v1),
/// and acks v1 basicmessages.
async fn start_peer() -> Arc<Agent> {
    let (listener, url) = listener().await;
    let agent = Arc::new(
        Agent::with_endpoint(Identity::generate().unwrap(), &format!("{url}/")).unwrap().with_features(Features::standard().with_v1()),
    );
    let handler = |State(agent): State<Arc<Agent>>, body: Bytes| async move {
        let received = agent.receive(&body).await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        let reply = if Agent::is_connection_message(&received) {
            agent.handle_connection_message(&received).await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
        } else {
            agent.auto_reply(&received).or_else(|| ack_v1(&received))
        };
        let Some(reply) = reply else { return Ok(Vec::new()) };
        let packed = agent.respond(&received, &reply).await.map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
        Ok::<_, (StatusCode, String)>(packed.unwrap_or_default())
    };
    let app = Router::new().route("/", post(handler)).with_state(agent.clone());
    tokio::spawn(async move { axum::serve(listener, app).await });
    agent
}

fn ack_v1(received: &Received) -> Option<Value> {
    let content = received.message["content"].as_str().unwrap_or_default();
    (normalize_type(received.message_type()) == BASICMESSAGE_V1 && !content.starts_with("ack: "))
        .then(|| received.reply(BASICMESSAGE_V1, json!({"content": format!("ack: {content}"), "sent_time": "2026-10-07T00:00:00Z"})))
}

/// The server with a public URL and no mediator, registry or v1 mediator, its HTTP app
/// serving `/didcomm` on a real port; and an MCP client over an in-memory transport.
struct Server {
    client: RunningService<RoleClient, TestClient>,
    bridge: Arc<Bridge>,
    endpoint: String,
}

async fn start_server(listener: tokio::net::TcpListener, base: &str, store: Store, config: Config) -> Server {
    let identity = store.load_or_generate_identity().await.unwrap();
    let endpoint = format!("{base}{}", http::DIDCOMM_PATH);
    let agent = Agent::with_endpoint(identity, &endpoint).unwrap().with_features(Features::standard().with_v1());
    let bridge = Arc::new(Bridge::with_store(agent, config.clone(), store).await.unwrap());
    let app = http::router(bridge.clone(), &config.http);
    tokio::spawn(async move { axum::serve(listener, app).await });
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    let served = bridge.clone();
    tokio::spawn(async move { DidcommMcp::new(served).serve(server_io).await.unwrap().waiting().await });
    Server { client: TestClient.serve(client_io).await.unwrap(), bridge, endpoint }
}

fn config(base: &str, database_url: Option<String>) -> Config {
    let dir = std::env::temp_dir().join(format!("didcomm-mcp-inbound-{}", uuid::Uuid::new_v4()));
    Config {
        identity_path: dir.join("identity.json"),
        registry_did: None,
        mediator_did: None,
        v1_mediator: None,
        state_path: dir.join("connections.json"),
        allowed_targets: None,
        validate_messages: true,
        public_url: Some(base.to_string()),
        database_url,
        http: HttpConfig { bind: "127.0.0.1:0".into(), auth_token: Some("t".into()), allowed_hosts: None },
    }
}

async fn call(client: &RunningService<RoleClient, TestClient>, tool: &str, arguments: Value) -> CallToolResult {
    client
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(arguments.as_object().unwrap().clone()))
        .await
        .unwrap()
}

fn texts(result: &CallToolResult) -> Vec<String> {
    result.content.iter().filter_map(|c| c.as_text()).map(|t| t.text.clone()).collect()
}

fn data(result: &CallToolResult) -> Value {
    assert_ne!(result.is_error, Some(true), "tool failed: {:?}", texts(result));
    serde_json::from_str(texts(result).last().unwrap()).unwrap()
}

/// The whole workflow against `store_for()` (called again for the restart).
async fn workflow(store_for: impl Fn() -> futures_store::StoreFuture, database_url: Option<String>) {
    let (first, base) = listener().await;
    let config = config(&base, database_url);
    let server = start_server(first, &base, store_for().await, config.clone()).await;
    let peer = start_peer().await;

    // Reachable at its own endpoint, with no mediator.
    let identity = data(&call(&server.client, "get_identity", json!({})).await);
    assert_eq!(identity["can_receive"], true, "{identity}");
    assert_eq!(identity["endpoint"], server.endpoint.as_str());
    assert_eq!(identity["mediation"], Value::Null);
    let our_did = identity["did"].as_str().unwrap().to_string();

    // Nothing queued yet: an empty fetch, not a "no mediator" error.
    let fetched = data(&call(&server.client, "fetch_messages", json!({})).await);
    assert_eq!(fetched["messages"], json!([]), "{fetched}");

    // A v2 peer writes unprompted, straight to our DID's endpoint.
    peer.send(&our_did, &json!({"type": BASICMESSAGE, "body": {"content": "hello direct"}})).await.unwrap();
    // A trust-ping that wants its answer on the connection gets it there.
    let ping = json!({"type": features::TRUST_PING_PING, "body": {"response_requested": true}, "return_route": "all"});
    let pong = peer.request(&our_did, &ping).await.unwrap();
    assert_eq!(pong.message_type(), features::TRUST_PING_RESPONSE);

    let fetched = data(&call(&server.client, "fetch_messages", json!({})).await);
    let messages = fetched["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2, "{fetched}");
    assert_eq!(messages[0]["from"], peer.did());
    assert_eq!(messages[0]["via"], "endpoint");
    assert_eq!(messages[0]["message"]["body"]["content"], "hello direct");
    assert_eq!(messages[1]["auto_replied"], true);
    // Taken: the next fetch is empty.
    assert_eq!(data(&call(&server.client, "fetch_messages", json!({})).await)["messages"], json!([]));

    // Garbage is refused, and not queued.
    let status = reqwest::Client::new().post(&server.endpoint).body("not didcomm").send().await.unwrap().status();
    assert_eq!(status, 400);

    // DIDComm v1: an invitation at our own endpoint (no v1 mediator), accepted by the peer.
    let created = data(&call(&server.client, "create_invitation", json!({"label": "MCP"})).await);
    let url = created["invitation_url"].as_str().unwrap();
    assert!(url.starts_with(&server.endpoint), "{url}");
    let invitation = peer.fetch_invitation(url).await.unwrap();
    let theirs = peer.accept_invitation(&invitation, "Peer").await.unwrap();
    assert_eq!(theirs.state, didcomm_agent::ConnectionState::Completed, "the handshake completes on delivery");
    let listed = data(&call(&server.client, "list_connections", json!({})).await);
    let ours = listed["connections"].as_array().unwrap().iter().find(|c| c["id"] == theirs.id.as_str()).cloned().unwrap();
    assert_eq!(ours["their_label"], "Peer");

    // The peer writes over the connection; it's queued, and we can answer it.
    peer.send(&theirs.id, &json!({"@type": BASICMESSAGE_V1, "content": "v1 hi", "sent_time": "2026-10-07T00:00:00Z"})).await.unwrap();
    let fetched = data(&call(&server.client, "fetch_messages", json!({})).await);
    let v1 = fetched["messages"].as_array().unwrap().iter().find(|m| m["didcomm_version"] == "v1" && m["message"]["content"] == "v1 hi").cloned();
    assert!(v1.is_some(), "{fetched}");
    assert_eq!(v1.unwrap()["connection"], theirs.id.as_str());
    let sent = data(&call(&server.client, "send_didcomm_message", json!({
        "target_did": theirs.id, "type": BASICMESSAGE_V1, "body": {"content": "back", "sent_time": "2026-10-07T00:00:00Z"},
    })).await);
    assert_eq!(sent["sent"]["didcomm_version"], "v1");

    // Something arrives while we're "down", then a restart on the same store: same DID,
    // connections kept, the message still waiting.
    peer.send(&our_did, &json!({"type": BASICMESSAGE, "body": {"content": "while you were out"}})).await.unwrap();
    drop(server);
    let (second, base2) = listener().await;
    let mut config2 = config.clone();
    config2.public_url = Some(base2.clone());
    let restarted = start_server(second, &base2, store_for().await, config2).await;
    // Same keys: the same v1 verkey (the v2 DID changes with the endpoint URL here).
    assert_eq!(restarted.bridge.agent().v1_verkey(), identity["didcomm_v1"]["verkey"].as_str().unwrap());
    let listed = data(&call(&restarted.client, "list_connections", json!({})).await);
    assert!(listed["connections"].as_array().unwrap().iter().any(|c| c["id"] == theirs.id.as_str()), "{listed}");
    let fetched = data(&call(&restarted.client, "fetch_messages", json!({})).await);
    let contents: Vec<&str> = fetched["messages"].as_array().unwrap().iter()
        .filter_map(|m| m["message"]["body"]["content"].as_str().or(m["message"]["content"].as_str()))
        .collect();
    // Our v1 "back" was acked over the connection; then the message sent while down.
    assert_eq!(contents, ["ack: back", "while you were out"], "{fetched}");
}

/// `Store` constructors as boxed futures, so `workflow` can make a fresh one per start.
mod futures_store {
    pub type StoreFuture = std::pin::Pin<Box<dyn std::future::Future<Output = didcomm_mcp::store::Store>>>;
}

#[tokio::test]
async fn receives_directly_with_the_file_store() {
    let dir: PathBuf = std::env::temp_dir().join(format!("didcomm-mcp-files-{}", uuid::Uuid::new_v4()));
    let (identity, state) = (dir.join("identity.json"), dir.join("connections.json"));
    workflow(move || {
        let (identity, state) = (identity.clone(), state.clone());
        Box::pin(async move { Store::files(&identity, &state) })
    }, None)
    .await;
}

#[tokio::test]
async fn receives_directly_with_postgres() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("TEST_DATABASE_URL not set: skipping the Postgres test");
        return;
    };
    // A fresh schema per run, so runs don't see each other's rows.
    let schema = format!("t{}", uuid::Uuid::new_v4().simple());
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}")).execute(&admin).await.unwrap();
    let separator = if url.contains('?') { '&' } else { '?' };
    let scoped = format!("{url}{separator}options=-c%20search_path%3D{schema}");
    let for_store = scoped.clone();
    workflow(move || {
        let url = for_store.clone();
        Box::pin(async move { Store::postgres(&url).await.unwrap() })
    }, Some(scoped))
    .await;

    // It's all in the database: the identity, the state, and an empty inbox.
    let keys: Vec<String> = sqlx::query_scalar(&format!("SELECT key FROM {schema}.didcomm_mcp_kv ORDER BY key")).fetch_all(&admin).await.unwrap();
    assert_eq!(keys, ["identity", "state"]);
    let queued: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {schema}.didcomm_mcp_inbox")).fetch_one(&admin).await.unwrap();
    assert_eq!(queued, 0);
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE")).execute(&admin).await.unwrap();
}

#[derive(Clone, Default)]
struct TestClient;
impl ClientHandler for TestClient {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }
}
