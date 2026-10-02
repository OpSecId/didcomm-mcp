//! The DIDComm side of every tool, independent of MCP: what each tool does, returning
//! plain JSON. `server.rs` turns these into MCP tool results.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use didcomm_agent::{features, Agent, AgentError, Mediation, Received};
use serde_json::{json, Value};

use crate::config::Config;

/// The documentation protocol this bridge speaks, and the version it falls back to for
/// registries that don't know it yet.
pub const DOCUMENTATION: &str = "https://wyvrn.app/documentation/1.1";
pub const DOCUMENTATION_1_0: &str = "https://wyvrn.app/documentation/1.0";

/// The DIDComm versions this bridge sends: v2 (as `semver` versions).
const SENDS: [(u64, u64); 2] = [(2, 0), (2, 1)];

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
        "{message_type} is a DIDComm v1 message type (the registry lists it for DIDComm {}), and this \
         server sends DIDComm v2 messages. Use the protocol's DIDComm v2 version if it has one \
         (search_protocols with didcomm_version 2.1).",
        didcomm_versions.join(", ")
    )]
    DidcommV1Only { message_type: String, didcomm_versions: Vec<String> },
    #[error("{0}")]
    BadArgument(String),
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
    /// Message type → what the registry says about it (`None`: it doesn't know it).
    known_types: Mutex<HashMap<String, Option<KnownType>>>,
    /// The registry only speaks documentation/1.0.
    registry_is_1_0: AtomicBool,
}

/// What the registry says about one message type.
#[derive(Debug, Clone, Default)]
struct KnownType {
    /// The schema for DIDComm v2 plaintext, the kind this bridge sends.
    schema: Option<Value>,
    /// Empty when the registry doesn't say.
    didcomm_versions: Vec<String>,
}

impl KnownType {
    fn from_message(message: &Value) -> Self {
        let didcomm_versions: Vec<String> = message["didcomm_versions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        let schema = match message["schemas"].as_array() {
            // documentation/1.1: one schema per envelope style; take the v2 one.
            Some(schemas) => schemas
                .iter()
                .find(|s| admits_v2(&strings(&s["didcomm_versions"])))
                .map(|s| s["schema"].clone()),
            // documentation/1.0: one schema, for DIDComm v2 plaintext unless it pins
            // v1's @type.
            None => message.get("schema").filter(|s| s["properties"].get("@type").is_none()).cloned(),
        };
        Self { schema, didcomm_versions }
    }

    /// Known, and known not to be usable with DIDComm v2.
    fn v1_only(&self) -> bool {
        !self.didcomm_versions.is_empty() && !admits_v2(&self.didcomm_versions)
    }
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect()
}

/// Whether any of `ranges` (semver requirements, e.g. `^2.0`) admits a DIDComm version
/// this bridge sends.
fn admits_v2(ranges: &[String]) -> bool {
    ranges.iter().filter_map(|r| semver::VersionReq::parse(r).ok()).any(|r| {
        SENDS.iter().any(|(major, minor)| r.matches(&semver::Version::new(*major, *minor, 0)))
    })
}

impl Bridge {
    pub fn new(agent: Agent, config: Config) -> Self {
        Self {
            agent,
            config,
            mediating: tokio::sync::Mutex::new(()),
            known_types: Mutex::new(HashMap::new()),
            registry_is_1_0: AtomicBool::new(false),
        }
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

    fn check_allowed(&self, target: &str) -> Result<(), BridgeError> {
        match &self.config.allowed_targets {
            Some(allowed) if !allowed.iter().any(|a| a == target) => Err(BridgeError::NotAllowed(target.to_string())),
            _ => Ok(()),
        }
    }

    /// `get_identity`.
    pub async fn identity(&self) -> Value {
        let mediation = match self.ensure_mediation().await {
            Ok(m) => json!({"mediator_did": m.mediator_did, "routing_did": m.routing_did}),
            Err(BridgeError::NoMediator) => Value::Null,
            Err(e) => json!({"error": e.to_string()}),
        };
        json!({
            "did": self.agent.did(),
            "base_did": self.agent.base_did(),
            "mediation": mediation,
            "registry_did": self.config.registry_did,
            "can_receive": self.agent.mediation().is_some(),
        })
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

    /// `send_didcomm_message`.
    pub async fn send(&self, outgoing: Outgoing) -> Result<Value, BridgeError> {
        self.check_allowed(&outgoing.target_did)?;
        if !outgoing.body.is_object() {
            return Err(BridgeError::BadArgument("body must be a JSON object".into()));
        }
        let mediation_problem = self.ensure_mediation_quietly().await;

        // Complete the headers here (pack would otherwise), so validation sees exactly
        // what will be sent.
        let mut message = json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "type": outgoing.message_type,
            "from": self.agent.did_for(&outgoing.target_did),
            "to": [outgoing.target_did],
            "created_time": now(),
            "body": outgoing.body,
        });
        for (key, value) in [("thid", outgoing.thid.map(Value::from)), ("pthid", outgoing.pthid.map(Value::from)), ("attachments", outgoing.attachments)] {
            if let Some(value) = value {
                message[key] = value;
            }
        }

        let known = match self.config.registry_did {
            Some(_) => Some(self.known_type(&outgoing.message_type).await),
            None => None,
        };
        if let Some(Ok(Some(known))) = &known {
            if known.v1_only() {
                return Err(BridgeError::DidcommV1Only {
                    message_type: outgoing.message_type,
                    didcomm_versions: known.didcomm_versions.clone(),
                });
            }
        }

        let validation = if !outgoing.validate || !self.config.validate_messages {
            "disabled".to_string()
        } else {
            match known {
                None => "skipped: no documentation registry configured".to_string(),
                Some(Ok(Some(KnownType { schema: Some(schema), .. }))) => {
                    let validator = jsonschema::validator_for(&schema).map_err(|e| BridgeError::Invalid {
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
                Some(Ok(_)) => "skipped: the registry has no schema for this message type".to_string(),
                Some(Err(e)) => format!("skipped: couldn't get a schema from the registry ({e})"),
            }
        };

        let reply = if outgoing.wait_for_reply {
            Some(self.agent.request(&outgoing.target_did, &message).await?)
        } else {
            self.agent.send(&outgoing.target_did, &message).await?
        };

        let mut result = json!({
            "sent": {
                "id": message["id"],
                "type": message["type"],
                "from": message["from"],
                "to": outgoing.target_did,
                "thid": message.get("thid").unwrap_or(&message["id"]),
            },
            "validation": validation,
            "reply": reply.as_ref().map(received_json),
        });
        if reply.is_none() {
            result["note"] = json!(match (self.agent.mediation(), mediation_problem) {
                (Some(_), _) => "Delivered. Any reply arrives through the mediator: call fetch_messages.".to_string(),
                (None, Some(problem)) => format!("Delivered, but this agent can't receive replies ({problem}). Use wait_for_reply to get a reply on the same connection."),
                (None, None) => "Delivered, but no mediator is configured, so replies can't reach this agent. Use wait_for_reply to get a reply on the same connection.".to_string(),
            });
        }
        Ok(result)
    }

    /// `fetch_messages`: pick up queued messages, answering trust-pings and
    /// discover-features queries among them automatically.
    pub async fn fetch(&self, limit: usize) -> Result<Value, BridgeError> {
        self.ensure_mediation().await?;
        let pickup = self.agent.pickup(limit).await?;
        let mut messages = Vec::new();
        for received in &pickup.messages {
            let mut entry = received_json(received);
            if let Some(reply) = self.agent.auto_reply(received) {
                let outcome = self.agent.respond(received, &reply).await;
                entry["auto_replied"] = json!(outcome.is_ok());
            }
            messages.push(entry);
        }
        let failed: Vec<Value> = pickup.failed.iter().map(|(id, error)| json!({"id": id, "error": error})).collect();
        Ok(json!({"messages": messages, "undecryptable": failed}))
    }
}

/// A received message as tool output: who authenticated it, and the message.
pub fn received_json(received: &Received) -> Value {
    json!({
        "from": received.sender.as_deref().unwrap_or("(anonymous)"),
        "authenticated": received.sender.is_some(),
        "message": received.message,
    })
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}
