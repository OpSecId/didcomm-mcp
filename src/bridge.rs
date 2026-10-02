//! The DIDComm side of every tool, independent of MCP: what each tool does, returning
//! plain JSON. `server.rs` turns these into MCP tool results.

use std::collections::HashMap;
use std::sync::Mutex;

use didcomm_agent::{features, Agent, AgentError, Mediation, Received};
use serde_json::{json, Value};

use crate::config::Config;

pub const DOCUMENTATION_QUERY: &str = "https://wyvrn.app/documentation/1.0/query";
pub const DOCUMENTATION_REQUEST: &str = "https://wyvrn.app/documentation/1.0/request";
pub const DOCUMENTATION_SPEC_REQUEST: &str = "https://wyvrn.app/documentation/1.0/spec-request";

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
    /// Message type → the registry's schema for it (`None`: the registry has none).
    schemas: Mutex<HashMap<String, Option<Value>>>,
}

impl Bridge {
    pub fn new(agent: Agent, config: Config) -> Self {
        Self { agent, config, mediating: tokio::sync::Mutex::new(()), schemas: Mutex::new(HashMap::new()) }
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

    async fn ask_registry(&self, message_type: &str, body: Value) -> Result<Received, BridgeError> {
        let registry = self.config.registry_did.as_deref().ok_or(BridgeError::NoRegistry)?;
        Ok(self.agent.request(registry, &json!({"type": message_type, "body": body})).await?)
    }

    /// `search_protocols`: a documentation/1.0 `query`; `body` is passed through.
    pub async fn search_protocols(&self, body: Value) -> Result<Received, BridgeError> {
        self.ask_registry(DOCUMENTATION_QUERY, body).await
    }

    /// `lookup_protocol_documentation`: a documentation/1.0 `request`. Caches the schemas
    /// it returns for `send_didcomm_message`'s validation.
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
        let reply = self.ask_registry(DOCUMENTATION_REQUEST, body).await?;
        self.cache_schemas(&reply.message["body"]);
        Ok(reply)
    }

    /// `lookup_spec`: a documentation/1.0 `spec-request`.
    pub async fn lookup_spec(&self, version: Option<&str>, section: Option<&str>) -> Result<Received, BridgeError> {
        let mut body = json!({});
        if let Some(version) = version {
            body["version"] = json!(version);
        }
        if let Some(section) = section {
            body["section"] = json!(section);
        }
        self.ask_registry(DOCUMENTATION_SPEC_REQUEST, body).await
    }

    fn cache_schemas(&self, response_body: &Value) {
        let mut cache = self.schemas.lock().expect("schema cache lock poisoned");
        for message in response_body["messages"].as_array().into_iter().flatten() {
            if let Some(message_type) = message["type"].as_str() {
                cache.insert(message_type.to_string(), message.get("schema").cloned());
            }
        }
    }

    /// The registry's schema for `message_type`, if it has one -- only from a response
    /// for exactly that protocol version (a registry may answer with another minor
    /// version, whose schema wouldn't be the right one).
    async fn schema_for(&self, message_type: &str) -> Result<Option<Value>, BridgeError> {
        if let Some(cached) = self.schemas.lock().expect("schema cache lock poisoned").get(message_type) {
            return Ok(cached.clone());
        }
        let Some((piuri, _)) = message_type.rsplit_once('/') else {
            return Ok(None);
        };
        let reply = match self
            .ask_registry(DOCUMENTATION_REQUEST, json!({"piuri": piuri, "sections": [], "messages": true}))
            .await
        {
            Ok(reply) => reply,
            Err(BridgeError::Agent(AgentError::Problem { code, .. })) if code.starts_with("e.p.not-found") => {
                self.schemas.lock().expect("schema cache lock poisoned").insert(message_type.to_string(), None);
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let body = &reply.message["body"];
        if body["piuri"] == piuri {
            self.cache_schemas(body);
        }
        let mut cache = self.schemas.lock().expect("schema cache lock poisoned");
        Ok(cache.entry(message_type.to_string()).or_insert(None).clone())
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

        let validation = if !outgoing.validate || !self.config.validate_messages {
            "disabled".to_string()
        } else if self.config.registry_did.is_none() {
            "skipped: no documentation registry configured".to_string()
        } else {
            match self.schema_for(&outgoing.message_type).await {
                Ok(Some(schema)) => {
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
                Ok(None) => "skipped: the registry has no schema for this message type".to_string(),
                Err(e) => format!("skipped: couldn't get a schema from the registry ({e})"),
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
