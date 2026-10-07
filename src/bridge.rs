//! The DIDComm side of every tool, independent of MCP: what each tool does, returning
//! plain JSON. `server.rs` turns these into MCP tool results.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use anyhow::Context;

use didcomm_agent::{
    features, Agent, AgentError, Connection, ConnectionBook, DidcommVersion, Mediation, Received, V1Mediation,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::Config;
use crate::store::Store;

/// The documentation protocol this bridge speaks, and the version it falls back to for
/// registries that don't know it yet.
pub const DOCUMENTATION: &str = "https://wyvrn.app/documentation/1.1";
pub const DOCUMENTATION_1_0: &str = "https://wyvrn.app/documentation/1.0";

/// The DIDComm v2 versions this bridge sends (as `semver` versions).
const SENDS_V2: [(u64, u64); 2] = [(2, 0), (2, 1)];
/// The DIDComm v1 version this bridge sends.
const SENDS_V1: [(u64, u64); 1] = [(1, 0)];

/// How this agent introduces itself in DID Exchange unless told otherwise.
pub const DEFAULT_LABEL: &str = "didcomm-mcp";

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error("no documentation registry is configured (set registry_did in the config file, or DIDCOMM_MCP_REGISTRY_DID)")]
    NoRegistry,
    #[error("{0} is not in this server's allowed_targets")]
    NotAllowed(String),
    #[error("no mediator is configured, so this agent can't receive messages except as a direct reply (use wait_for_reply)")]
    NoMediator,
    #[error("mediation with {mediator} failed: {error}")]
    Mediation { mediator: String, error: String },
    #[error("the message doesn't match the registry's schema for {message_type}:\n{}", errors.join("\n"))]
    Invalid { message_type: String, errors: Vec<String> },
    #[error(
        "{message_type} is used with DIDComm {} (says the registry), but {target} is a DIDComm {version} \
         connection. Use the protocol version made for DIDComm {version}, or another connection.",
        didcomm_versions.join(", ")
    )]
    WrongDidcommVersion { message_type: String, didcomm_versions: Vec<String>, target: String, version: DidcommVersion },
    #[error("no DIDComm v1 mediator is configured, so this agent can't create invitations (set v1_mediator)")]
    NoV1Mediator,
    #[error("{0}")]
    BadArgument(String),
    #[error("storage: {0}")]
    Storage(String),
}

/// What `send_didcomm_message` was asked to send.
#[derive(Debug, Clone, Default)]
pub struct Outgoing {
    pub target_did: String,
    pub message_type: String,
    pub body: Value,
    pub thid: Option<String>,
    pub pthid: Option<String>,
    pub attachments: Option<Value>,
    pub wait_for_reply: bool,
    pub validate: bool,
}

pub struct Bridge {
    agent: Agent,
    config: Config,
    /// Serializes mediation attempts; the outcome itself lives in the agent.
    mediating: tokio::sync::Mutex<()>,
    mediating_v1: tokio::sync::Mutex<()>,
    /// Identity, state and inbox: files or Postgres.
    store: Store,
    /// Serializes writes of the state.
    saving: tokio::sync::Mutex<()>,
    /// Message type → what the registry says about it (`None`: it doesn't know it).
    known_types: Mutex<HashMap<String, Option<KnownType>>>,
    /// The registry only speaks documentation/1.0.
    registry_is_1_0: AtomicBool,
}

/// What the registry says about one message type.
#[derive(Debug, Clone, Default)]
struct KnownType {
    /// The schema for DIDComm v2 plaintext.
    schema_v2: Option<Value>,
    /// The schema for DIDComm v1 plaintext (`@type`, decorators).
    schema_v1: Option<Value>,
    /// Empty when the registry doesn't say.
    didcomm_versions: Vec<String>,
}

impl KnownType {
    fn from_message(message: &Value) -> Self {
        let didcomm_versions = strings(&message["didcomm_versions"]);
        let (schema_v2, schema_v1) = match message["schemas"].as_array() {
            // documentation/1.1: one schema per envelope style.
            Some(schemas) => {
                let find = |sends: &[(u64, u64)]| {
                    schemas.iter().find(|s| admits(&strings(&s["didcomm_versions"]), sends)).map(|s| s["schema"].clone())
                };
                (find(&SENDS_V2), find(&SENDS_V1))
            }
            // documentation/1.0: one schema, for DIDComm v1 plaintext if it pins @type.
            None => match message.get("schema") {
                Some(schema) if schema["properties"].get("@type").is_some() => (None, Some(schema.clone())),
                schema => (schema.cloned(), None),
            },
        };
        Self { schema_v2, schema_v1, didcomm_versions }
    }

    fn schema(&self, version: DidcommVersion) -> Option<&Value> {
        match version {
            DidcommVersion::V1 => self.schema_v1.as_ref(),
            DidcommVersion::V2 => self.schema_v2.as_ref(),
        }
    }

    /// Known, and known not to be usable with `version`.
    fn excludes(&self, version: DidcommVersion) -> bool {
        let sends: &[(u64, u64)] = match version {
            DidcommVersion::V1 => &SENDS_V1,
            DidcommVersion::V2 => &SENDS_V2,
        };
        !self.didcomm_versions.is_empty() && !admits(&self.didcomm_versions, sends)
    }
}

/// The agent for `identity` under `config`: reachable at its own endpoint when
/// `public_url` is set -- under a `did:web` (`did_method = "web"`) whose document
/// `--http` serves, or a `did:peer:4` naming the endpoint -- else with no endpoint
/// (mediation, or replies on the connection, only).
pub fn agent_for(identity: didcomm_agent::Identity, config: &Config) -> Result<Agent, AgentError> {
    Ok(match (config.inbound_endpoint(), config.web_did()) {
        (Some(endpoint), Some(did)) => Agent::with_did(identity, &did).with_v1_endpoint(&endpoint),
        (Some(endpoint), None) => Agent::with_endpoint(identity, &endpoint)?,
        (None, _) => Agent::new(identity)?,
    })
}

/// What the state file keeps: connections and the v1 mediation.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    connections: ConnectionBook,
    #[serde(default)]
    v1_mediation: Option<V1Mediation>,
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect()
}

/// Whether any of `ranges` (semver requirements, e.g. `^2.0`) admits one of `versions`.
fn admits(ranges: &[String], versions: &[(u64, u64)]) -> bool {
    ranges.iter().filter_map(|r| semver::VersionReq::parse(r).ok()).any(|r| {
        versions.iter().any(|(major, minor)| r.matches(&semver::Version::new(*major, *minor, 0)))
    })
}

impl Bridge {
    /// With the file store (`identity_path` / `state_path`): restores connections and
    /// the v1 mediation from the state file, if there is one.
    pub async fn new(agent: Agent, config: Config) -> Self {
        let store = Store::files(&config.identity_path, &config.state_path);
        Self::with_store(agent, config, store).await.expect("the file store logs read errors instead of failing")
    }

    /// With `store`: restores connections and the v1 mediation from it. Fails if a
    /// database can't be read, so a session never starts empty and then overwrites the
    /// saved state.
    pub async fn with_store(agent: Agent, config: Config, store: Store) -> anyhow::Result<Self> {
        let state = store.load_state().await.context("loading the saved state")?;
        let bridge = Self {
            agent,
            config,
            store,
            mediating: tokio::sync::Mutex::new(()),
            mediating_v1: tokio::sync::Mutex::new(()),
            saving: tokio::sync::Mutex::new(()),
            known_types: Mutex::new(HashMap::new()),
            registry_is_1_0: AtomicBool::new(false),
        };
        if let Some(text) = state {
            match serde_json::from_str::<State>(&text) {
                Ok(state) => {
                    bridge.agent.import_connections(state.connections);
                    bridge.agent.restore_v1_mediation(state.v1_mediation);
                }
                Err(e) => tracing::warn!("ignoring the saved state ({}): {e}", bridge.store.kind()),
            }
        }
        Ok(bridge)
    }

    /// Save connections and the v1 mediation. Failure is logged, not fatal: the session
    /// goes on in memory.
    async fn save_state(&self) {
        let _guard = self.saving.lock().await;
        let state = State { connections: self.agent.export_connections(), v1_mediation: self.agent.v1_mediation() };
        let saved = match serde_json::to_string_pretty(&state) {
            Ok(json) => self.store.save_state(json).await,
            Err(e) => Err(e.into()),
        };
        if let Err(e) = saved {
            tracing::warn!("can't save the state ({}): {e:#}", self.store.kind());
        }
    }

    /// The `did:web` document `--http` publishes, when the agent has one.
    pub fn did_document(&self) -> Option<Value> {
        let did = self.config.web_did()?;
        let endpoint = self.config.inbound_endpoint()?;
        Some(self.agent.identity().did_document(&did, &endpoint))
    }

    /// Whether peers can deliver to this agent's own endpoint (`public_url`).
    fn has_inbound(&self) -> bool {
        self.config.inbound_endpoint().is_some()
    }

    /// A message POSTed to this agent's own endpoint (`/didcomm`): unpack it, take DID
    /// Exchange a step further and answer trust-pings and feature queries, as
    /// `fetch_messages` does for picked-up messages, and queue it for
    /// `fetch_messages`. Returns the packed reply to write back on the connection, when
    /// the sender asked for one (`return_route`).
    pub async fn deliver(&self, packed: &[u8]) -> Result<Option<Vec<u8>>, BridgeError> {
        let received = self.agent.receive(packed).await?;
        let mut entry = self.received_json(&received);
        entry["via"] = json!("endpoint");
        let mut on_connection = None;
        if Agent::is_connection_message(&received) {
            let outcome = match self.agent.handle_connection_message(&received).await {
                Ok(Some(reply)) => self.agent.respond(&received, &reply).await.map(|packed| on_connection = packed),
                Ok(None) => Ok(()),
                Err(e) => Err(e),
            };
            self.save_state().await;
            entry["handshake"] = match outcome {
                Ok(()) => json!("handled; see list_connections"),
                Err(e) => json!(format!("failed: {e}")),
            };
        } else if let Some(reply) = self.agent.auto_reply(&received) {
            let outcome = self.agent.respond(&received, &reply).await;
            entry["auto_replied"] = json!(outcome.is_ok());
            on_connection = outcome.ok().flatten();
        }
        self.store.push_inbox(entry).await.map_err(|e| BridgeError::Storage(format!("{e:#}")))?;
        Ok(on_connection)
    }

    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Mediate with the configured mediator unless already mediated. A failed attempt
    /// is retried on the next call.
    pub async fn ensure_mediation(&self) -> Result<Mediation, BridgeError> {
        let mediator = self.config.mediator_did.as_deref().ok_or(BridgeError::NoMediator)?;
        let _guard = self.mediating.lock().await;
        if let Some(mediation) = self.agent.mediation() {
            return Ok(mediation);
        }
        self.agent.mediate(mediator).await.map_err(|e| BridgeError::Mediation {
            mediator: mediator.to_string(),
            error: e.to_string(),
        })
    }

    /// Get DIDComm v1 mediation from the configured v1 mediator unless already
    /// mediated: connect (DID Exchange, through an implicit invitation when it's a DID),
    /// then coordinate-mediation/1.0. A failed attempt is retried on the next call.
    pub async fn ensure_v1_mediation(&self) -> Result<V1Mediation, BridgeError> {
        let mediator = self.config.v1_mediator.as_deref().ok_or(BridgeError::NoV1Mediator)?;
        let _guard = self.mediating_v1.lock().await;
        if let Some(mediation) = self.agent.v1_mediation() {
            return Ok(mediation);
        }
        let failed = |e: AgentError| BridgeError::Mediation { mediator: mediator.to_string(), error: e.to_string() };
        let invitation = if mediator.starts_with("did:") {
            json!({
                "@type": "https://didcomm.org/out-of-band/1.1/invitation",
                "@id": mediator,
                "handshake_protocols": ["https://didcomm.org/didexchange/1.1"],
                "services": [mediator],
            })
        } else {
            self.agent.fetch_invitation(mediator).await.map_err(failed)?
        };
        let connection = self.agent.accept_invitation(&invitation, DEFAULT_LABEL).await.map_err(failed)?;
        let mediation = self.agent.mediate_v1(&connection.id).await.map_err(failed);
        self.save_state().await;
        mediation
    }

    async fn ensure_v1_mediation_quietly(&self) -> Option<String> {
        match self.ensure_v1_mediation().await {
            Ok(_) | Err(BridgeError::NoV1Mediator) => None,
            Err(e) => Some(e.to_string()),
        }
    }

    /// `target` (a DID, or a connection by id) is allowed if it, or the connection's
    /// peer DID, is in `allowed_targets`.
    fn check_allowed(&self, target: &str) -> Result<(), BridgeError> {
        let Some(allowed) = &self.config.allowed_targets else { return Ok(()) };
        let peer = self.agent.connection(target).and_then(|c| c.their_did);
        if allowed.iter().any(|a| a == target || Some(a) == peer.as_ref()) {
            Ok(())
        } else {
            Err(BridgeError::NotAllowed(target.to_string()))
        }
    }

    /// `get_identity`.
    pub async fn identity(&self) -> Value {
        let mediation = match self.ensure_mediation().await {
            Ok(m) => json!({"mediator_did": m.mediator_did, "routing_did": m.routing_did}),
            Err(BridgeError::NoMediator) => Value::Null,
            Err(e) => json!({"error": e.to_string()}),
        };
        let v1_mediation = match self.ensure_v1_mediation().await {
            Ok(m) => json!({"endpoint": m.endpoint, "connection": m.connection_id}),
            Err(BridgeError::NoV1Mediator) => Value::Null,
            Err(e) => json!({"error": e.to_string()}),
        };
        json!({
            "did": self.agent.did(),
            "base_did": self.agent.base_did(),
            "mediation": mediation,
            "registry_did": self.config.registry_did,
            "can_receive": self.agent.mediation().is_some() || self.has_inbound(),
            "endpoint": self.config.inbound_endpoint(),
            "storage": self.store.kind(),
            "did_method": if self.config.web_did().is_some() { "web" } else { "peer" },
            "didcomm_v1": {
                "did": self.agent.v1_did(),
                "verkey": self.agent.v1_verkey(),
                "mediation": v1_mediation,
                "can_receive": self.agent.v1_reachable(),
            },
            "connections": self.agent.connections().len(),
        })
    }

    /// `accept_invitation`: connect through an out-of-band invitation (URL or JSON).
    pub async fn accept_invitation(&self, invitation: &str, label: Option<&str>) -> Result<Connection, BridgeError> {
        // Mediated first, so the connection gets the DID peers can reach.
        let mediation_problem = self.ensure_v1_mediation_quietly().await;
        if let Some(problem) = mediation_problem {
            tracing::warn!("accepting an invitation without v1 mediation: {problem}");
        }
        let invitation = self.agent.fetch_invitation(invitation).await?;
        if let Some(allowed) = &self.config.allowed_targets {
            let named: Vec<&str> = [&invitation["from"], &invitation["services"][0], &invitation["@id"]]
                .into_iter()
                .filter_map(Value::as_str)
                .collect();
            if !named.iter().any(|n| allowed.iter().any(|a| a == n)) {
                return Err(BridgeError::NotAllowed(format!("the inviter ({})", named.join(", "))));
            }
        }
        let connection = self.agent.accept_invitation(&invitation, label.unwrap_or(DEFAULT_LABEL)).await;
        self.save_state().await;
        Ok(connection?)
    }

    /// `create_invitation`: an out-of-band invitation to connect with this agent over
    /// DIDComm v1 (DID Exchange), as JSON and as a URL.
    pub async fn create_invitation(&self, label: Option<&str>) -> Result<Value, BridgeError> {
        // Through the v1 mediator if there is one; else at this agent's own endpoint.
        let base = match (self.ensure_v1_mediation().await, self.config.inbound_endpoint()) {
            (Ok(mediation), _) => mediation.endpoint,
            (Err(BridgeError::NoV1Mediator), Some(endpoint)) => endpoint,
            (Err(e), _) => return Err(e),
        };
        let invitation = self.agent.create_invitation(label.unwrap_or(DEFAULT_LABEL))?;
        self.save_state().await;
        Ok(json!({
            "invitation": invitation,
            "invitation_url": Agent::invitation_url(&base, &invitation),
            "note": "Any number of peers can accept it. Their requests arrive at this agent: call fetch_messages, which completes the connections.",
        }))
    }

    /// `list_connections`.
    pub fn connections(&self) -> Value {
        Value::Array(self.agent.connections().iter().map(connection_json).collect())
    }

    /// `discover_features`: the peer's disclosures.
    pub async fn discover_features(&self, target: &str, pattern: Option<&str>) -> Result<Received, BridgeError> {
        self.check_allowed(target)?;
        self.ensure_mediation_quietly().await;
        let query = json!({
            "type": features::DISCOVER_FEATURES_QUERIES,
            "body": {"queries": [{"feature-type": "protocol", "match": pattern.unwrap_or("*")}]},
        });
        Ok(self.agent.request(target, &query).await?)
    }

    /// Ask the registry `name` (`query`, `request`, `spec-request`) in documentation/1.1,
    /// or 1.0 once it has said it doesn't support 1.1.
    async fn ask_registry(&self, name: &str, body: Value) -> Result<Received, BridgeError> {
        let registry = self.config.registry_did.as_deref().ok_or(BridgeError::NoRegistry)?;
        if !self.registry_is_1_0.load(Ordering::Relaxed) {
            let message = json!({"type": format!("{DOCUMENTATION}/{name}"), "body": body.clone()});
            match self.agent.request(registry, &message).await {
                Err(AgentError::Problem { code, .. }) if code == "e.p.msg.unsupported" => {
                    tracing::info!("the registry doesn't speak documentation/1.1; using 1.0");
                    self.registry_is_1_0.store(true, Ordering::Relaxed);
                }
                result => return Ok(result?),
            }
        }
        let message = json!({"type": format!("{DOCUMENTATION_1_0}/{name}"), "body": body});
        Ok(self.agent.request(registry, &message).await?)
    }

    /// `search_protocols`: a documentation `query`; `body` is passed through.
    pub async fn search_protocols(&self, body: Value) -> Result<Received, BridgeError> {
        self.ask_registry("query", body).await
    }

    /// `lookup_protocol_documentation`: a documentation `request`. Remembers what it
    /// says about each message type for `send_didcomm_message`.
    pub async fn lookup_protocol(
        &self,
        protocol_uri: &str,
        sections: Option<Vec<String>>,
        include_messages: bool,
    ) -> Result<Received, BridgeError> {
        let mut body = json!({"piuri": protocol_uri, "messages": include_messages});
        if let Some(sections) = sections {
            body["sections"] = json!(sections);
        }
        let reply = self.ask_registry("request", body).await?;
        self.remember_types(&reply.message["body"]);
        Ok(reply)
    }

    /// `lookup_spec`: a documentation `spec-request` for `document` (default: the
    /// spec itself).
    pub async fn lookup_spec(
        &self,
        document: Option<&str>,
        version: Option<&str>,
        section: Option<&str>,
    ) -> Result<Received, BridgeError> {
        let mut body = json!({});
        for (key, value) in [("document", document), ("version", version), ("section", section)] {
            if let Some(value) = value {
                body[key] = json!(value);
            }
        }
        self.ask_registry("spec-request", body).await
    }

    fn remember_types(&self, response_body: &Value) {
        let mut known = self.known_types.lock().expect("known types lock poisoned");
        for message in response_body["messages"].as_array().into_iter().flatten() {
            if let Some(message_type) = message["type"].as_str() {
                known.insert(message_type.to_string(), Some(KnownType::from_message(message)));
            }
        }
    }

    /// What the registry says about `message_type` -- only from a response for exactly
    /// that protocol version (a registry may answer with another minor version, whose
    /// schema wouldn't be the right one).
    async fn known_type(&self, message_type: &str) -> Result<Option<KnownType>, BridgeError> {
        if let Some(cached) = self.known_types.lock().expect("known types lock poisoned").get(message_type) {
            return Ok(cached.clone());
        }
        let Some((piuri, _)) = message_type.rsplit_once('/') else {
            return Ok(None);
        };
        let reply = match self.ask_registry("request", json!({"piuri": piuri, "sections": [], "messages": true})).await {
            Ok(reply) => reply,
            Err(BridgeError::Agent(AgentError::Problem { code, .. })) if code.starts_with("e.p.not-found") => {
                self.known_types.lock().expect("known types lock poisoned").insert(message_type.to_string(), None);
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let body = &reply.message["body"];
        if body["piuri"] == piuri {
            self.remember_types(body);
        }
        let mut known = self.known_types.lock().expect("known types lock poisoned");
        Ok(known.entry(message_type.to_string()).or_insert(None).clone())
    }

    /// Mediate if configured, ignoring failure -- for sends, whose `from` should be the
    /// mediated DID when there is one, but which can still work without it.
    async fn ensure_mediation_quietly(&self) -> Option<String> {
        match self.ensure_mediation().await {
            Ok(_) | Err(BridgeError::NoMediator) => None,
            Err(e) => Some(e.to_string()),
        }
    }

    /// `send_didcomm_message`. DIDComm v1 to a v1 connection, or with a message type
    /// the registry lists for v1 alone; v2 otherwise.
    pub async fn send(&self, outgoing: Outgoing) -> Result<Value, BridgeError> {
        self.check_allowed(&outgoing.target_did)?;
        if !outgoing.body.is_object() {
            return Err(BridgeError::BadArgument("body must be a JSON object".into()));
        }
        let connection = self.agent.connection(&outgoing.target_did);
        let known = match self.config.registry_did {
            Some(_) => Some(self.known_type(&outgoing.message_type).await),
            None => None,
        };
        let known_excludes = |version| matches!(&known, Some(Ok(Some(k))) if k.excludes(version));
        let version = match &connection {
            Some(c) => c.didcomm_version,
            None if known_excludes(DidcommVersion::V2) => DidcommVersion::V1,
            None => DidcommVersion::V2,
        };
        if known_excludes(version) {
            let Some(Ok(Some(known))) = &known else { unreachable!("excludes() needs a known type") };
            return Err(BridgeError::WrongDidcommVersion {
                message_type: outgoing.message_type,
                didcomm_versions: known.didcomm_versions.clone(),
                target: outgoing.target_did,
                version,
            });
        }
        let mediation_problem = match version {
            DidcommVersion::V1 => self.ensure_v1_mediation_quietly().await,
            DidcommVersion::V2 => self.ensure_mediation_quietly().await,
        };

        // Complete the headers here (pack would otherwise), so validation sees exactly
        // what will be sent.
        let message = match version {
            DidcommVersion::V2 => {
                let peer = connection.as_ref().and_then(|c| c.their_did.clone()).unwrap_or_else(|| outgoing.target_did.clone());
                let mut message = json!({
                    "id": uuid::Uuid::new_v4().to_string(),
                    "type": outgoing.message_type,
                    "from": self.agent.did_for(&peer),
                    "to": [peer],
                    "created_time": now(),
                    "body": outgoing.body,
                });
                for (key, value) in [("thid", outgoing.thid.map(Value::from)), ("pthid", outgoing.pthid.map(Value::from)), ("attachments", outgoing.attachments)] {
                    if let Some(value) = value {
                        message[key] = value;
                    }
                }
                message
            }
            // v1: the body's fields are the message's own, next to @type, @id and the
            // ~thread and ~attach decorators.
            DidcommVersion::V1 => {
                let mut message = outgoing.body.clone();
                message["@type"] = json!(outgoing.message_type);
                message["@id"] = json!(uuid::Uuid::new_v4().to_string());
                let mut thread = json!({});
                for (key, value) in [("thid", &outgoing.thid), ("pthid", &outgoing.pthid)] {
                    if let Some(value) = value {
                        thread[key] = json!(value);
                    }
                }
                if thread.as_object().is_some_and(|t| !t.is_empty()) {
                    message["~thread"] = thread;
                }
                if let Some(attachments) = outgoing.attachments {
                    message["~attach"] = attachments;
                }
                message
            }
        };

        let validation = if !outgoing.validate || !self.config.validate_messages {
            "disabled".to_string()
        } else {
            match known {
                None => "skipped: no documentation registry configured".to_string(),
                Some(Ok(Some(known))) if known.schema(version).is_some() => {
                    let schema = known.schema(version).expect("checked");
                    let validator = jsonschema::validator_for(schema).map_err(|e| BridgeError::Invalid {
                        message_type: outgoing.message_type.clone(),
                        errors: vec![format!("the registry's schema itself is invalid: {e}")],
                    })?;
                    let errors: Vec<String> = validator
                        .iter_errors(&message)
                        .map(|e| {
                            let path = e.instance_path.to_string();
                            format!("at {}: {e}", if path.is_empty() { "/" } else { &path })
                        })
                        .collect();
                    if !errors.is_empty() {
                        return Err(BridgeError::Invalid { message_type: outgoing.message_type, errors });
                    }
                    "passed".to_string()
                }
                Some(Ok(_)) => format!("skipped: the registry has no DIDComm {version} schema for this message type"),
                Some(Err(e)) => format!("skipped: couldn't get a schema from the registry ({e})"),
            }
        };

        let reply = if outgoing.wait_for_reply {
            Some(self.agent.request(&outgoing.target_did, &message).await?)
        } else {
            self.agent.send(&outgoing.target_did, &message).await?
        };

        let sent = match version {
            DidcommVersion::V2 => json!({
                "didcomm_version": version,
                "id": message["id"],
                "type": message["type"],
                "from": message["from"],
                "to": outgoing.target_did,
                "thid": message.get("thid").unwrap_or(&message["id"]),
            }),
            DidcommVersion::V1 => json!({
                "didcomm_version": version,
                "id": message["@id"],
                "type": message["@type"],
                "from": self.agent.v1_did(),
                "to": outgoing.target_did,
                "thid": message["~thread"].get("thid").unwrap_or(&message["@id"]),
            }),
        };
        let mut result = json!({
            "sent": sent,
            "validation": validation,
            "reply": reply.as_ref().map(|r| self.received_json(r)),
        });
        if reply.is_none() {
            let can_receive = match version {
                DidcommVersion::V1 => self.agent.v1_reachable(),
                DidcommVersion::V2 => self.agent.mediation().is_some() || self.has_inbound(),
            };
            result["note"] = json!(match (can_receive, mediation_problem) {
                (true, _) => "Delivered. Any reply arrives later: call fetch_messages.".to_string(),
                (false, Some(problem)) => format!("Delivered, but this agent can't receive replies ({problem}). Use wait_for_reply to get a reply on the same connection."),
                (false, None) => "Delivered, but no mediator is configured, so replies can't reach this agent. Use wait_for_reply to get a reply on the same connection.".to_string(),
            });
        }
        Ok(result)
    }

    /// `fetch_messages`: pick up queued messages from the v2 mediator and the v1 one,
    /// taking DID Exchange messages among them a step further and answering
    /// trust-pings and discover-features queries automatically.
    pub async fn fetch(&self, limit: usize) -> Result<Value, BridgeError> {
        // Messages delivered straight to this agent's endpoint were already handled
        // (handshakes, auto-replies) when they arrived; they're only collected here.
        let mut messages = if self.has_inbound() {
            self.store.take_inbox(limit).await.map_err(|e| BridgeError::Storage(format!("{e:#}")))?
        } else {
            Vec::new()
        };
        let v2 = self.ensure_mediation().await;
        let v1 = self.ensure_v1_mediation().await;
        if !self.has_inbound() {
            if let (Err(e), Err(BridgeError::NoV1Mediator)) = (&v2, &v1) {
                return Err(match e {
                    BridgeError::NoMediator => BridgeError::NoMediator,
                    e => BridgeError::Mediation { mediator: "the mediator".into(), error: e.to_string() },
                });
            }
        }
        let mut pickups = Vec::new();
        let mut problems = Vec::new();
        match &v2 {
            Ok(_) => pickups.push(self.agent.pickup(limit).await?),
            Err(BridgeError::NoMediator) => {}
            Err(e) => problems.push(e.to_string()),
        }
        match &v1 {
            Ok(_) => pickups.push(self.agent.pickup_v1(limit).await?),
            Err(BridgeError::NoV1Mediator) => {}
            Err(e) => problems.push(e.to_string()),
        }

        let mut failed = Vec::new();
        for pickup in pickups {
            for received in &pickup.messages {
                let mut entry = self.received_json(received);
                if Agent::is_connection_message(received) {
                    let outcome = match self.agent.handle_connection_message(received).await {
                        Ok(Some(reply)) => self.agent.respond(received, &reply).await.map(|_| ()),
                        Ok(None) => Ok(()),
                        Err(e) => Err(e),
                    };
                    entry["handshake"] = match outcome {
                        Ok(()) => json!("handled; see list_connections"),
                        Err(e) => json!(format!("failed: {e}")),
                    };
                } else if let Some(reply) = self.agent.auto_reply(received) {
                    let outcome = self.agent.respond(received, &reply).await;
                    entry["auto_replied"] = json!(outcome.is_ok());
                }
                messages.push(entry);
            }
            failed.extend(pickup.failed.iter().map(|(id, error)| json!({"id": id, "error": error})));
        }
        self.save_state().await;
        let mut result = json!({"messages": messages, "undecryptable": failed});
        if !problems.is_empty() {
            result["mediation_problems"] = json!(problems);
        }
        Ok(result)
    }

    /// A received message as tool output: who authenticated it, the DIDComm version,
    /// for v1 the connection it came on, and the message.
    pub fn received_json(&self, received: &Received) -> Value {
        let mut entry = received_json(received);
        if let Some(connection) = received.sender_key.as_deref().and_then(|k| self.agent.connection_by_key(k)) {
            entry["connection"] = json!(connection.id);
        }
        entry
    }
}

/// A received message as tool output: who authenticated it, and the message.
pub fn received_json(received: &Received) -> Value {
    json!({
        "from": received.sender.as_deref().or(received.sender_key.as_deref()).unwrap_or("(anonymous)"),
        "authenticated": received.sender.is_some() || received.sender_key.is_some(),
        "didcomm_version": received.version,
        "message": received.message,
    })
}

/// A connection as tool output.
pub fn connection_json(connection: &Connection) -> Value {
    json!({
        "id": connection.id,
        "state": connection.state,
        "role": connection.role,
        "didcomm_version": connection.didcomm_version,
        "protocol": connection.protocol,
        "their_label": connection.their_label,
        "their_did": connection.their_did,
        "invitation_id": connection.invitation_id,
    })
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}
