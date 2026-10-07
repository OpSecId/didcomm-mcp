//! The Streamable HTTP transport over real HTTP on localhost, driven by rmcp's own HTTP
//! client: bearer-token enforcement, the tool set, a real DIDComm exchange through it,
//! and one shared agent identity across sessions.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{body::Bytes, extract::State, http::StatusCode, routing::post, Router};
use didcomm_agent::{features, Agent, Identity};
use didcomm_mcp::{
    bridge::Bridge,
    config::{Config, HttpConfig},
    http,
};
use rmcp::{
    model::{CallToolRequestParams, CallToolResult, ClientConfig},
    service::RunningService,
    transport::{streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport},
    ClientHandler, RoleClient, ServiceExt,
};
use serde_json::{json, Value};

const TOKEN: &str = "test-token-0123456789";

async fn listener() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (listener, url)
}

/// A peer that answers trust-pings on the connection.
async fn start_peer() -> Arc<Agent> {
    let (listener, url) = listener().await;
    let agent = Arc::new(Agent::with_endpoint(Identity::generate().unwrap(), &format!("{url}/")).unwrap());
    let handler = |State(agent): State<Arc<Agent>>, body: Bytes| async move {
        let received = agent.receive(&body).await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        let Some(reply) = agent.auto_reply(&received) else { return Ok(Vec::new()) };
        let packed = agent.respond(&received, &reply).await.map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
        Ok::<_, (StatusCode, String)>(packed.unwrap_or_default())
    };
    let app = Router::new().route("/", post(handler)).with_state(agent.clone());
    tokio::spawn(async move { axum::serve(listener, app).await });
    agent
}

/// The MCP server over HTTP (no registry, no mediator); returns its base URL.
async fn start_server() -> String {
    let (listener, url) = listener().await;
    let http_config = HttpConfig { bind: listener.local_addr().unwrap().to_string(), auth_token: Some(TOKEN.into()), allowed_hosts: None };
    let config = Config {
        identity_path: PathBuf::from("unused"),
        registry_did: None,
        mediator_did: None,
        v1_mediator: None,
        state_path: std::env::temp_dir().join(format!("didcomm-mcp-test-{}.json", uuid::Uuid::new_v4())),
        allowed_targets: None,
        validate_messages: true,
        public_url: None,
        database_url: None,
        http: http_config.clone(),
    };
    let bridge = Arc::new(Bridge::new(Agent::new(Identity::generate().unwrap()).unwrap(), config).await);
    let app = http::router(bridge, &http_config);
    tokio::spawn(async move { axum::serve(listener, app).await });
    url
}

#[derive(Clone, Default)]
struct TestClient;
impl ClientHandler for TestClient {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }
}

async fn connect(url: &str, token: Option<&str>) -> Result<RunningService<RoleClient, TestClient>, String> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(format!("{url}{}", http::MCP_PATH));
    if let Some(token) = token {
        config = config.auth_header(token);
    }
    TestClient
        .serve(StreamableHttpClientTransport::from_config(config))
        .await
        .map_err(|e| e.to_string())
}

async fn call(client: &RunningService<RoleClient, TestClient>, tool: &str, arguments: Value) -> CallToolResult {
    client
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(arguments.as_object().unwrap().clone()))
        .await
        .unwrap()
}

fn data(result: &CallToolResult) -> Value {
    assert_ne!(result.is_error, Some(true), "tool failed: {:?}", result.content);
    serde_json::from_str(&result.content.last().unwrap().as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn the_endpoint_requires_the_bearer_token() {
    let url = start_server().await;
    let http = reqwest::Client::new();
    let initialize = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"},
    }});
    let post = |token: Option<&str>| {
        let mut request = http
            .post(format!("{url}{}", http::MCP_PATH))
            .header("accept", "application/json, text/event-stream")
            .json(&initialize);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        request.send()
    };

    assert_eq!(post(None).await.unwrap().status(), 401);
    assert_eq!(post(Some("wrong-token")).await.unwrap().status(), 401);
    assert!(post(Some(TOKEN)).await.unwrap().status().is_success());
    // Health checks need no token.
    assert_eq!(http.get(format!("{url}/healthz")).send().await.unwrap().text().await.unwrap(), "ok");

    assert!(connect(&url, None).await.is_err());
    assert!(connect(&url, Some("wrong-token")).await.is_err());
}

#[tokio::test]
async fn tools_work_over_http() {
    let url = start_server().await;
    let peer = start_peer().await;
    let client = connect(&url, Some(TOKEN)).await.unwrap();

    let tools = client.list_tools(None).await.unwrap().tools;
    assert_eq!(tools.len(), 10);

    let ping = data(&call(&client, "send_didcomm_message", json!({
        "target_did": peer.did(),
        "type": features::TRUST_PING_PING,
        "body": {},
        "wait_for_reply": true,
    })).await);
    assert_eq!(ping["reply"]["message"]["type"], features::TRUST_PING_RESPONSE);
    assert_eq!(ping["reply"]["from"], peer.did());
}

#[tokio::test]
async fn sessions_share_one_agent() {
    let url = start_server().await;
    let first = connect(&url, Some(TOKEN)).await.unwrap();
    let second = connect(&url, Some(TOKEN)).await.unwrap();

    let first_did = data(&call(&first, "get_identity", json!({})).await)["did"].clone();
    let second_did = data(&call(&second, "get_identity", json!({})).await)["did"].clone();

    assert!(first_did.is_string());
    assert_eq!(first_did, second_did);
}
