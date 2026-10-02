//! The MCP server end to end: an rmcp client calls the tools over an in-memory
//! transport, and behind them real DIDComm parties run over HTTP on localhost -- a
//! mediator (`didcomm-mediator-core`), a documentation registry, and "Bob".
//!
//! The registry here is a small stand-in answering documentation/1.1 (or, in one test,
//! only 1.0) for two protocols, so these tests don't need the documentation-server and
//! its sources; documentation-server's own tests cover the real thing.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{body::Bytes, extract::State, http::StatusCode, routing::post, Router};
use base64::Engine as _;
use didcomm_agent::v1::normalize_type;
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
const BASICMESSAGE_V1: &str = "https://didcomm.org/basicmessage/1.0/message";
const DOCUMENTATION: &str = "https://wyvrn.app/documentation/1.1";

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

fn basicmessage_v1_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "required": ["@id", "@type", "content"],
        "properties": {"@type": {"const": BASICMESSAGE_V1}, "content": {"type": "string"}},
    })
}

/// A documentation registry that knows basicmessage 2.0 (DIDComm v2) and 1.0 (DIDComm v1
/// only), answering each request in the documentation version it was asked in.
fn registry_answers(received: &Received) -> Option<Value> {
    let body = &received.message["body"];
    let (piuri, name) = received.message_type().rsplit_once('/')?;
    let reply = |name: &str, body: Value| Some(received.reply(&format!("{piuri}/{name}"), body));
    match name {
        "query" => {
            let entries: Vec<Value> = [
                json!({"piuri": "https://didcomm.org/basicmessage/2.0", "title": "Basic Message", "status": "Production", "has_schemas": true, "didcomm_versions": ["^2.0"]}),
                json!({"piuri": "https://didcomm.org/basicmessage/1.0", "title": "Basic Message", "status": "Adopted", "has_schemas": true, "didcomm_versions": ["^1.0"]}),
            ]
            .into_iter()
            .filter(|e| match body["didcomm_version"].as_str() {
                Some("1.0") => e["didcomm_versions"][0] == "^1.0",
                Some(_) => e["didcomm_versions"][0] == "^2.0",
                None => true,
            })
            .collect();
            reply("catalog", json!({"total": entries.len(), "offset": 0, "entries": entries}))
        }
        "request" => {
            let requested = body["piuri"].as_str().unwrap_or_default();
            let (piuri, message) = if requested.starts_with("https://didcomm.org/basicmessage/2.0") {
                ("https://didcomm.org/basicmessage/2.0", json!({
                    "type": BASICMESSAGE, "didcomm_versions": ["^2.0"], "examples": [],
                    "schema": basicmessage_schema(),
                    "schemas": [{"didcomm_versions": ["^2.0"], "schema": basicmessage_schema()}],
                }))
            } else if requested.starts_with("https://didcomm.org/basicmessage/1.0") {
                ("https://didcomm.org/basicmessage/1.0", json!({
                    "type": BASICMESSAGE_V1, "didcomm_versions": ["^1.0"], "examples": [],
                    "schema": basicmessage_v1_schema(),
                    "schemas": [{"didcomm_versions": ["^1.0"], "schema": basicmessage_v1_schema()}],
                }))
            } else {
                return Some(received.problem_report("e.p.not-found.protocol", "No documentation for protocol {1}", &[requested]));
            };
            reply("response", json!({
                "piuri": piuri,
                "title": "Basic Message",
                "status": "Production",
                "didcomm_versions": message["didcomm_versions"],
                "available_sections": [{"id": "roles", "title": "Roles", "level": 2}],
                "sections": [],
                "messages": [message],
            }))
        }
        "spec-request" => reply("spec-response", json!({
            "document": body["document"].as_str().unwrap_or("spec"),
            "version": body["version"].as_str().unwrap_or("2.1"),
            "title": "Spec",
            "section": {"id": "x", "title": "X", "markdown": "text"},
        })),
        _ => None,
    }
}

/// A registry that only knows documentation/1.0, as one deployed before 1.1 would.
fn registry_1_0_answers(received: &Received) -> Option<Value> {
    if received.message_type().starts_with("https://wyvrn.app/documentation/1.1/") {
        return Some(received.problem_report("e.p.msg.unsupported", "Unsupported message type {1}", &[received.message_type()]));
    }
    let mut reply = registry_answers(received)?;
    // 1.0 has no per-version schemas.
    for message in reply["body"]["messages"].as_array_mut().into_iter().flatten() {
        message.as_object_mut().unwrap().remove("schemas");
        message.as_object_mut().unwrap().remove("didcomm_versions");
    }
    Some(reply)
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
    config: Config,
    identity: Identity,
}

async fn world(customize: impl FnOnce(&mut Config)) -> World {
    world_with(registry_answers, customize).await
}

async fn world_with(registry_answers: fn(&Received) -> Option<Value>, customize: impl FnOnce(&mut Config)) -> World {
    let mediator = start_mediator().await;
    let registry = start_agent(Features::standard().with_protocol(DOCUMENTATION, &["registry"]), registry_answers).await;
    let bob = start_agent(Features::standard().with_protocol("https://didcomm.org/basicmessage/2.0", &["receiver"]), bob_answers).await;

    let mut config = Config {
        identity_path: PathBuf::from("unused"),
        registry_did: Some(registry.did()),
        mediator_did: Some(mediator),
        v1_mediator: None,
        state_path: temp_state_path(),
        allowed_targets: None,
        validate_messages: true,
        http: Default::default(),
    };
    customize(&mut config);
    let identity = Identity::generate().unwrap();
    let client = serve(identity.clone(), config.clone()).await;
    World { client, bob, registry, config, identity }
}

/// An MCP client connected to a fresh server with `identity` and `config`.
async fn serve(identity: Identity, config: Config) -> RunningService<RoleClient, TestClient> {
    let bridge = Arc::new(Bridge::new(Agent::new(identity).unwrap(), config));
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move { DidcommMcp::new(bridge).serve(server_io).await.unwrap().waiting().await });
    TestClient.serve(client_io).await.unwrap()
}

fn temp_state_path() -> PathBuf {
    std::env::temp_dir().join(format!("didcomm-mcp-test-{}.json", uuid::Uuid::new_v4()))
}

impl World {
    async fn call(&self, tool: &str, arguments: Value) -> CallToolResult {
        call(&self.client, tool, arguments).await
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
        "accept_invitation",
        "create_invitation",
        "discover_features",
        "fetch_messages",
        "get_identity",
        "list_connections",
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
    let found = data(&world.call("search_protocols", json!({"text": "basic", "didcomm_version": "2.1"})).await);
    assert_eq!(found["message"]["body"]["total"], 1);
    assert_eq!(found["message"]["type"], "https://wyvrn.app/documentation/1.1/catalog");
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

#[tokio::test]
async fn a_v1_type_to_a_did_without_a_v1_service_fails() {
    let world = world(|_| {}).await;

    let refused = world
        .call("send_didcomm_message", json!({"target_did": world.bob.did(), "type": BASICMESSAGE_V1, "body": {"content": "x"}}))
        .await;

    assert_eq!(refused.is_error, Some(true));
    let text = &texts(&refused)[0];
    assert!(text.contains("no DIDComm v1 service"), "{text}");
}

#[tokio::test]
async fn documents_and_versions_reach_the_registry() {
    let world = world(|_| {}).await;

    let v1 = data(&world.call("search_protocols", json!({"didcomm_version": "1.0"})).await);
    assert_eq!(v1["message"]["body"]["entries"][0]["piuri"], "https://didcomm.org/basicmessage/1.0");

    let extension = data(&world.call("lookup_spec", json!({"document": "extension/l10n", "section": "x"})).await);
    assert_eq!(extension["message"]["body"]["document"], "extension/l10n");
    let v1_spec = data(&world.call("lookup_spec", json!({"version": "1.0"})).await);
    assert_eq!(v1_spec["message"]["body"]["version"], "1.0");
}

#[tokio::test]
async fn a_documentation_1_0_registry_still_works() {
    let world = world_with(registry_1_0_answers, |_| {}).await;
    let bob = world.bob.did();

    let found = data(&world.call("search_protocols", json!({"text": "basic"})).await);
    assert_eq!(found["message"]["type"], "https://wyvrn.app/documentation/1.0/catalog");

    let invalid = world.call("send_didcomm_message", json!({"target_did": bob, "type": BASICMESSAGE, "body": {}})).await;
    assert_eq!(invalid.is_error, Some(true), "validated against the 1.0 response's schema");
    let sent = data(&world.call(
        "send_didcomm_message",
        json!({"target_did": bob, "type": BASICMESSAGE, "body": {"content": "hi"}, "wait_for_reply": true}),
    ).await);
    assert_eq!(sent["validation"], "passed");

    // 1.0 doesn't say which DIDComm version a type is for; a v1 schema (pinning @type)
    // isn't used to validate a v2 message, and the send isn't refused.
    let v1 = data(&world.call(
        "send_didcomm_message",
        json!({"target_did": bob, "type": BASICMESSAGE_V1, "body": {}, "wait_for_reply": false}),
    ).await);
    assert_eq!(v1["validation"], "skipped: the registry has no DIDComm v2 schema for this message type");
}

// DIDComm v1: connections through out-of-band invitations and DID Exchange.

/// What a v1 agent does with a message: DID Exchange, auto-replies, and acking
/// basicmessage/1.0 (`ack: <content>`, unless it is an ack).
async fn v1_react(agent: &Agent, received: &Received) -> Option<Value> {
    if Agent::is_connection_message(received) {
        return agent.handle_connection_message(received).await.unwrap();
    }
    if let Some(reply) = agent.auto_reply(received) {
        return Some(reply);
    }
    let content = received.message["content"].as_str().unwrap_or_default();
    (normalize_type(received.message_type()) == BASICMESSAGE_V1 && !content.starts_with("ack: "))
        .then(|| received.reply(BASICMESSAGE_V1, json!({"content": format!("ack: {content}"), "sent_time": "2026-10-02T00:00:00Z"})))
}

/// A v1 agent ("Carol"), directly reachable over HTTP.
async fn start_carol() -> Arc<Agent> {
    let (listener, endpoint) = listener().await;
    let carol = Arc::new(
        Agent::with_endpoint(Identity::generate().unwrap(), &endpoint).unwrap().with_features(Features::standard().with_v1()),
    );
    let handler = |State(agent): State<Arc<Agent>>, body: Bytes| async move {
        let received = agent.receive(&body).await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        let Some(reply) = v1_react(&agent, &received).await else {
            return Ok(Vec::new());
        };
        let packed = agent.respond(&received, &reply).await.map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
        Ok::<_, (StatusCode, String)>(packed.unwrap_or_default())
    };
    let app = Router::new().route("/", post(handler)).with_state(carol.clone());
    tokio::spawn(async move { axum::serve(listener, app).await });
    carol
}

#[tokio::test]
async fn a_v1_connection_from_an_invitation_url() {
    let world = world(|_| {}).await;
    let carol = start_carol().await;
    let url = Agent::invitation_url("https://carol.example/", &carol.create_invitation("Carol").unwrap());

    let accepted = world.call("accept_invitation", json!({"invitation": url, "label": "Claude"})).await;
    assert!(is_untrusted(&accepted), "the inviter's label is third-party content");
    let connection = data(&accepted);
    assert_eq!(connection["state"], "completed");
    assert_eq!(connection["didcomm_version"], "v1");
    assert_eq!(connection["their_label"], "Carol");
    assert_eq!(carol.connections()[0].their_label.as_deref(), Some("Claude"));
    let id = connection["id"].as_str().unwrap().to_string();

    let listed = data(&world.call("list_connections", json!({})).await);
    assert_eq!(listed["connections"][0]["id"], id.as_str());

    // A v1 message to the connection, validated against the v1 schema.
    let sent = data(&world.call(
        "send_didcomm_message",
        json!({"target_did": id, "type": BASICMESSAGE_V1, "body": {"content": "hi", "sent_time": "2026-10-02T00:00:00Z"}, "wait_for_reply": true}),
    ).await);
    assert_eq!(sent["sent"]["didcomm_version"], "v1");
    assert_eq!(sent["validation"], "passed");
    assert_eq!(sent["reply"]["didcomm_version"], "v1");
    assert_eq!(sent["reply"]["connection"], id.as_str());
    assert_eq!(sent["reply"]["message"]["content"], "ack: hi");

    let invalid = world.call("send_didcomm_message", json!({"target_did": id, "type": BASICMESSAGE_V1, "body": {}})).await;
    assert_eq!(invalid.is_error, Some(true));
    assert!(texts(&invalid)[0].contains("content"), "{:?}", texts(&invalid));

    // A v2-only type doesn't go over a v1 connection.
    let wrong = world.call("send_didcomm_message", json!({"target_did": id, "type": BASICMESSAGE, "body": {"content": "x"}})).await;
    assert_eq!(wrong.is_error, Some(true));
    assert!(texts(&wrong)[0].contains("DIDComm v1 connection"), "{:?}", texts(&wrong));
}

/// A v1 mediator: an agent that connects, grants mediation, queues forwarded messages
/// and delivers them through messagepickup/2.0.
struct V1Mediator {
    agent: Agent,
    queue: std::sync::Mutex<Vec<(String, Value)>>,
}

async fn start_v1_mediator() -> Arc<V1Mediator> {
    let (listener, endpoint) = listener().await;
    let mediator = Arc::new(V1Mediator {
        agent: Agent::with_endpoint(Identity::generate().unwrap(), &endpoint).unwrap(),
        queue: Default::default(),
    });
    let handler = |State(m): State<Arc<V1Mediator>>, body: Bytes| async move {
        let received = m.agent.receive(&body).await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        let reply = match normalize_type(received.message_type()).as_str() {
            "https://didcomm.org/routing/1.0/forward" => {
                m.queue.lock().unwrap().push((uuid::Uuid::new_v4().to_string(), received.message["msg"].clone()));
                None
            }
            "https://didcomm.org/coordinate-mediation/1.0/mediate-request" => Some(received.reply(
                "https://didcomm.org/coordinate-mediation/1.0/mediate-grant",
                json!({"endpoint": m.agent.endpoint(), "routing_keys": [m.agent.v1_did_key()]}),
            )),
            "https://didcomm.org/coordinate-mediation/1.0/keylist-update" => {
                let updated: Vec<Value> = received.message["updates"].as_array().unwrap().iter()
                    .map(|u| json!({"recipient_key": u["recipient_key"], "action": u["action"], "result": "success"}))
                    .collect();
                Some(received.reply("https://didcomm.org/coordinate-mediation/1.0/keylist-update-response", json!({"updated": updated})))
            }
            "https://didcomm.org/messagepickup/2.0/delivery-request" => {
                let queue = m.queue.lock().unwrap();
                Some(if queue.is_empty() {
                    received.reply("https://didcomm.org/messagepickup/2.0/status", json!({"message_count": 0}))
                } else {
                    let attachments: Vec<Value> = queue.iter()
                        .map(|(id, msg)| json!({"@id": id, "data": {"base64": base64::engine::general_purpose::STANDARD.encode(msg.to_string())}}))
                        .collect();
                    received.reply("https://didcomm.org/messagepickup/2.0/delivery", json!({"~attach": attachments}))
                })
            }
            "https://didcomm.org/messagepickup/2.0/messages-received" => {
                let ids: Vec<String> = received.message["message_id_list"].as_array().unwrap().iter().filter_map(|i| i.as_str().map(str::to_string)).collect();
                let mut queue = m.queue.lock().unwrap();
                queue.retain(|(id, _)| !ids.contains(id));
                Some(received.reply("https://didcomm.org/messagepickup/2.0/status", json!({"message_count": queue.len()})))
            }
            _ => v1_react(&m.agent, &received).await,
        };
        let Some(reply) = reply else { return Ok(Vec::new()) };
        let packed = m.agent.respond(&received, &reply).await.map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
        Ok::<_, (StatusCode, String)>(packed.unwrap_or_default())
    };
    let app = Router::new().route("/", post(handler)).with_state(mediator.clone());
    tokio::spawn(async move { axum::serve(listener, app).await });
    mediator
}

#[tokio::test]
async fn a_v1_mediated_server_invites_and_keeps_its_connections() {
    let v1_mediator = start_v1_mediator().await;
    let mediator_invitation = Agent::invitation_url("https://m.example/", &v1_mediator.agent.create_invitation("Mediator").unwrap());
    let world = world(|c| c.v1_mediator = Some(mediator_invitation)).await;
    let carol = start_carol().await;

    let identity = data(&world.call("get_identity", json!({})).await);
    assert_eq!(identity["didcomm_v1"]["can_receive"], true, "{identity}");
    assert_eq!(identity["didcomm_v1"]["mediation"]["endpoint"], v1_mediator.agent.endpoint());

    // Carol accepts our invitation; her request waits at the mediator.
    let created = data(&world.call("create_invitation", json!({"label": "Claude"})).await);
    let url = created["invitation_url"].as_str().unwrap();
    let invitation = carol.fetch_invitation(url).await.unwrap();
    let carols = carol.accept_invitation(&invitation, "Carol").await.unwrap();

    // Fetching answers it; Carol completes, which the next fetch records.
    let fetched = data(&world.call("fetch_messages", json!({})).await);
    assert_eq!(fetched["messages"][0]["handshake"], "handled; see list_connections", "{fetched}");
    assert_eq!(carol.connection(&carols.id).unwrap().state, didcomm_agent::ConnectionState::Completed);
    data(&world.call("fetch_messages", json!({})).await);
    let listed = data(&world.call("list_connections", json!({})).await);
    let ours = listed["connections"].as_array().unwrap().iter().find(|c| c["id"] == carols.id.as_str()).unwrap().clone();
    assert_eq!(ours["state"], "completed");
    assert_eq!(ours["their_label"], "Carol");

    // Carol writes unprompted; it arrives through the mediator, and our ack reaches her.
    carol.send(&carols.id, &json!({"@type": BASICMESSAGE_V1, "content": "hello", "sent_time": "2026-10-02T00:00:00Z"})).await.unwrap();
    let fetched = data(&world.call("fetch_messages", json!({})).await);
    assert_eq!(fetched["messages"][0]["message"]["content"], "hello");
    assert_eq!(fetched["messages"][0]["connection"], carols.id.as_str());

    // A restarted server (same identity and state file) still has both.
    let restarted = serve(world.identity.clone(), world.config.clone()).await;
    let listed = data(&call(&restarted, "list_connections", json!({})).await);
    assert!(listed["connections"].as_array().unwrap().iter().any(|c| c["id"] == carols.id.as_str()));
    let identity = data(&call(&restarted, "get_identity", json!({})).await);
    assert_eq!(identity["didcomm_v1"]["mediation"]["endpoint"], v1_mediator.agent.endpoint());
}
