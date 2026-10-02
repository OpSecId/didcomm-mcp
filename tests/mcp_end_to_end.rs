//! The MCP server end to end: an rmcp client calls the tools over an in-memory
//! transport, and behind them real DIDComm parties run over HTTP on localhost -- a
//! mediator (`didcomm-mediator-core`), a documentation registry, and "Bob".
//!
//! The registry here is a small stand-in answering documentation/1.0 for one protocol,
//! so these tests don't need the documentation-server and its sources;
//! documentation-server's own tests cover the real thing.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{body::Bytes, extract::State, http::StatusCode, routing::post, Router};
use didcomm_agent::{features, Agent, Features, Identity, Received};
use didcomm_mcp::{bridge::Bridge, config::Config, server::DidcommMcp};
use didcomm_mediator_core::MediatorService;
use didcomm_quickstart::{generate_did_with_endpoint, setup_default};
use rmcp::{
    model::{CallToolRequestParams, CallToolResult, ClientConfig},
    service::RunningService,
    ClientHandler, RoleClient, ServiceExt,
};
use serde_json::{json, Value};

const BASICMESSAGE: &str = "https://didcomm.org/basicmessage/2.0/message";
const DOCUMENTATION: &str = "https://wyvrn.app/documentation/1.0";

async fn listener() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    (listener, endpoint)
}

async fn start_mediator() -> String {
    let (listener, endpoint) = listener().await;
    let generated = generate_did_with_endpoint(&endpoint).unwrap();
    let did = generated.did.clone();
    let mediator = Arc::new(MediatorService::new(did.clone(), setup_default(&generated)));
    let app = Router::new().route(
        "/",
        post(|State(m): State<Arc<MediatorService<_, _>>>, body: Bytes| async move {
            match m.handle_message(&body).await {
                Ok(reply) => Ok(reply.unwrap_or_default()),
                Err(e) => Err((StatusCode::BAD_REQUEST, e.to_string())),
            }
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app.with_state(mediator)).await });
    did
}

/// An agent server: `answer` decides the reply to each message (`None`: no reply).
async fn start_agent(features: Features, answer: fn(&Received) -> Option<Value>) -> Arc<Agent> {
    let (listener, endpoint) = listener().await;
    let agent = Arc::new(Agent::with_endpoint(Identity::generate().unwrap(), &endpoint).unwrap().with_features(features));
    let handler = move |State(agent): State<Arc<Agent>>, body: Bytes| async move {
        let received = agent.receive(&body).await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        let Some(reply) = agent.auto_reply(&received).or_else(|| answer(&received)) else {
            return Ok(Vec::new());
        };
        let packed = agent.respond(&received, &reply).await.map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
        Ok::<_, (StatusCode, String)>(packed.unwrap_or_default())
    };
    let app = Router::new().route("/", post(handler)).with_state(agent.clone());
    tokio::spawn(async move { axum::serve(listener, app).await });
    agent
}

/// Bob acks basicmessages: on the connection if asked to, else to the sender's DID.
fn bob_answers(received: &Received) -> Option<Value> {
    (received.message_type() == BASICMESSAGE).then(|| {
        let content = received.message["body"]["content"].as_str().unwrap_or_default();
        json!({
            "type": BASICMESSAGE,
            "thid": received.thread_id(),
            "body": {"content": format!("ack: {content}")},
        })
    })
}

fn basicmessage_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "required": ["id", "type", "created_time", "body"],
        "properties": {
            "type": {"const": BASICMESSAGE},
            "body": {"type": "object", "required": ["content"], "properties": {"content": {"type": "string"}}},
        },
    })
}

/// A documentation/1.0 registry that knows basicmessage/2.0 only.
fn registry_answers(received: &Received) -> Option<Value> {
    let body = &received.message["body"];
    match received.message_type() {
        "https://wyvrn.app/documentation/1.0/query" => Some(received.reply(
            "https://wyvrn.app/documentation/1.0/catalog",
            json!({"total": 1, "offset": 0, "entries": [{
                "piuri": "https://didcomm.org/basicmessage/2.0",
                "title": "Basic Message",
                "status": "Production",
                "has_schemas": true,
            }]}),
        )),
        "https://wyvrn.app/documentation/1.0/request" => {
            let piuri = body["piuri"].as_str().unwrap_or_default();
            if !piuri.starts_with("https://didcomm.org/basicmessage/2.0") {
                return Some(received.problem_report("e.p.not-found.protocol", "No documentation for protocol {1}", &[piuri]));
            }
            Some(received.reply(
                "https://wyvrn.app/documentation/1.0/response",
                json!({
                    "piuri": "https://didcomm.org/basicmessage/2.0",
                    "title": "Basic Message",
                    "status": "Production",
                    "available_sections": [{"id": "roles", "title": "Roles", "level": 2}],
                    "sections": [],
                    "messages": [{"type": BASICMESSAGE, "examples": [], "schema": basicmessage_schema()}],
                }),
            ))
        }
        "https://wyvrn.app/documentation/1.0/spec-request" => Some(received.reply(
            "https://wyvrn.app/documentation/1.0/spec-response",
            json!({"version": "2.1", "title": "Spec", "section": {"id": "x", "title": "X", "markdown": "text"}}),
        )),
        _ => None,
    }
}

#[derive(Clone, Default)]
struct TestClient;
impl ClientHandler for TestClient {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }
}

struct World {
    client: RunningService<RoleClient, TestClient>,
    bob: Arc<Agent>,
    registry: Arc<Agent>,
}

async fn world(customize: impl FnOnce(&mut Config)) -> World {
    let mediator = start_mediator().await;
    let registry = start_agent(Features::standard().with_protocol(DOCUMENTATION, &["registry"]), registry_answers).await;
    let bob = start_agent(Features::standard().with_protocol("https://didcomm.org/basicmessage/2.0", &["receiver"]), bob_answers).await;

    let mut config = Config {
        identity_path: PathBuf::from("unused"),
        registry_did: Some(registry.did()),
        mediator_did: Some(mediator),
        allowed_targets: None,
        validate_messages: true,
    };
    customize(&mut config);
    let bridge = Arc::new(Bridge::new(Agent::new(Identity::generate().unwrap()).unwrap(), config));

    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move { DidcommMcp::new(bridge).serve(server_io).await.unwrap().waiting().await });
    let client = TestClient.serve(client_io).await.unwrap();
    World { client, bob, registry }
}

impl World {
    async fn call(&self, tool: &str, arguments: Value) -> CallToolResult {
        self.client
            .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(arguments.as_object().unwrap().clone()))
            .await
            .unwrap()
    }
}

fn texts(result: &CallToolResult) -> Vec<String> {
    result.content.iter().filter_map(|c| c.as_text()).map(|t| t.text.clone()).collect()
}

/// The JSON block of a result (its last text block).
fn data(result: &CallToolResult) -> Value {
    assert_ne!(result.is_error, Some(true), "tool failed: {:?}", texts(result));
    serde_json::from_str(texts(result).last().unwrap()).unwrap()
}

fn is_untrusted(result: &CallToolResult) -> bool {
    texts(result).first().is_some_and(|t| t.starts_with("UNTRUSTED CONTENT"))
}

#[tokio::test]
async fn exposes_the_fixed_tool_set() {
    let world = world(|_| {}).await;
    let mut names: Vec<String> = world.client.list_tools(None).await.unwrap().tools.into_iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(names, [
        "discover_features",
        "fetch_messages",
        "get_identity",
        "lookup_protocol_documentation",
        "lookup_spec",
        "search_protocols",
        "send_didcomm_message",
    ]);
}

#[tokio::test]
async fn the_architecture_brief_workflow() {
    let world = world(|_| {}).await;
    let bob = world.bob.did();

    // Our identity: mediated, so peers can reach us.
    let identity = data(&world.call("get_identity", json!({})).await);
    assert_eq!(identity["can_receive"], true);
    assert_ne!(identity["did"], identity["base_did"]);

    // 1-2. What does Bob support?
    let disclosed = world.call("discover_features", json!({"target_did": bob})).await;
    assert!(is_untrusted(&disclosed));
    let disclosures = data(&disclosed)["message"]["body"]["disclosures"].clone();
    assert!(disclosures.as_array().unwrap().iter().any(|d| d["id"] == "https://didcomm.org/basicmessage/2.0"));

    // 3-4. Learn the protocol from the registry.
    let found = data(&world.call("search_protocols", json!({"text": "basic"})).await);
    assert_eq!(found["message"]["body"]["total"], 1);
    let docs = data(&world.call(
        "lookup_protocol_documentation",
        json!({"protocol_uri": "https://didcomm.org/basicmessage/2.0", "sections": []}),
    ).await);
    assert_eq!(docs["message"]["body"]["messages"][0]["schema"], basicmessage_schema());

    // A message that breaks the schema is refused before it leaves.
    let invalid = world.call("send_didcomm_message", json!({"target_did": bob, "type": BASICMESSAGE, "body": {}})).await;
    assert_eq!(invalid.is_error, Some(true));
    assert!(texts(&invalid)[0].contains("content"), "{:?}", texts(&invalid));

    // 5-6. Send it properly; Bob's reply comes back through the mediator.
    let sent = data(&world.call(
        "send_didcomm_message",
        json!({"target_did": bob, "type": BASICMESSAGE, "body": {"content": "hello"}}),
    ).await);
    assert_eq!(sent["validation"], "passed");
    assert!(sent["note"].as_str().unwrap().contains("fetch_messages"));

    let fetched = world.call("fetch_messages", json!({})).await;
    assert!(is_untrusted(&fetched));
    let messages = data(&fetched)["messages"].clone();
    assert_eq!(messages.as_array().unwrap().len(), 1, "{messages}");
    assert_eq!(messages[0]["from"], bob);
    assert_eq!(messages[0]["message"]["body"]["content"], "ack: hello");
    assert_eq!(messages[0]["message"]["thid"], sent["sent"]["id"]);

    // Or wait for the reply on the same connection.
    let answered = world.call(
        "send_didcomm_message",
        json!({"target_did": bob, "type": BASICMESSAGE, "body": {"content": "now"}, "wait_for_reply": true}),
    ).await;
    assert!(is_untrusted(&answered));
    assert_eq!(data(&answered)["reply"]["message"]["body"]["content"], "ack: now");
}

#[tokio::test]
async fn the_own_mediator_can_be_messaged_too() {
    let mediator = start_mediator().await;
    let world = world(|c| c.mediator_did = Some(mediator.clone())).await;
    assert_eq!(data(&world.call("get_identity", json!({})).await)["can_receive"], true);

    let status = data(&world.call("send_didcomm_message", json!({
        "target_did": mediator,
        "type": "https://didcomm.org/messagepickup/3.0/status-request",
        "body": {},
        "wait_for_reply": true,
    })).await);

    assert_eq!(status["reply"]["message"]["type"], "https://didcomm.org/messagepickup/3.0/status");
}

#[tokio::test]
async fn problem_reports_are_tool_errors() {
    let world = world(|_| {}).await;

    let missing = world.call("lookup_protocol_documentation", json!({"protocol_uri": "https://didcomm.org/escrow/1.0"})).await;

    assert_eq!(missing.is_error, Some(true));
    let texts = texts(&missing);
    assert!(texts[0].contains("e.p.not-found.protocol"), "{texts:?}");
    assert!(texts[1].starts_with("UNTRUSTED CONTENT"), "the report is third-party content: {texts:?}");
    assert!(texts[1].contains(&world.registry.did()));
}

#[tokio::test]
async fn allowed_targets_are_enforced() {
    let world = world(|c| c.allowed_targets = Some(vec!["did:example:someone-else".into()])).await;

    let refused = world
        .call("send_didcomm_message", json!({"target_did": world.bob.did(), "type": BASICMESSAGE, "body": {"content": "x"}}))
        .await;

    assert_eq!(refused.is_error, Some(true));
    assert!(texts(&refused)[0].contains("allowed_targets"));
}

#[tokio::test]
async fn works_without_a_registry_or_mediator() {
    let world = world(|c| {
        c.registry_did = None;
        c.mediator_did = None;
    })
    .await;
    let bob = world.bob.did();

    let lookup = world.call("lookup_spec", json!({})).await;
    assert_eq!(lookup.is_error, Some(true));
    assert!(texts(&lookup)[0].contains("no documentation registry"));

    let sent = data(&world.call(
        "send_didcomm_message",
        json!({"target_did": bob, "type": BASICMESSAGE, "body": {"content": "hi"}, "wait_for_reply": true}),
    ).await);
    assert_eq!(sent["validation"], "skipped: no documentation registry configured");
    assert_eq!(sent["reply"]["message"]["body"]["content"], "ack: hi");

    let fetch = world.call("fetch_messages", json!({})).await;
    assert_eq!(fetch.is_error, Some(true));
    assert!(texts(&fetch)[0].contains("no mediator"));

    let ping = world.call("send_didcomm_message", json!({
        "target_did": bob, "type": features::TRUST_PING_PING, "body": {}, "wait_for_reply": true,
    })).await;
    assert_eq!(data(&ping)["reply"]["message"]["type"], features::TRUST_PING_RESPONSE);
}
