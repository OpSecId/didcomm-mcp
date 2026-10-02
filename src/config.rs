//! Configuration: an optional TOML file, then environment-variable overrides.
//!
//! File location: the `--config <path>` argument, else `$DIDCOMM_MCP_CONFIG`, else
//! `$XDG_CONFIG_HOME/didcomm-mcp/config.toml` (`~/.config/...`) if it exists. Every
//! setting has a default, so no file is needed at all.

use std::path::PathBuf;

use serde::Deserialize;

/// The documentation/1.0 registry the lookup tools ask by default.
pub const DEFAULT_REGISTRY: &str = "did:web:docs.wyvrn.app";

/// The Indicio public mediator: a free DIDComm v2 mediator meant for development and
/// demos, not production.
pub const DEFAULT_MEDIATOR: &str = "did:web:us-east2.public.mediator.indiciotech.io";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// This agent's private keys; created on first start.
    pub identity_path: PathBuf,
    /// The documentation/1.0 registry the lookup tools ask (default
    /// [`DEFAULT_REGISTRY`]). `None` -- configured as `""` -- disables them: they then
    /// report that none is configured, and sends skip schema validation.
    pub registry_did: Option<String>,
    /// The mediator that receives messages for this agent. `None` disables mediation:
    /// replies then only arrive on the same connection (`wait_for_reply`).
    pub mediator_did: Option<String>,
    /// The DIDComm v1 mediator: a DID to connect to through DID Exchange (an implicit
    /// invitation), or an out-of-band invitation (URL or JSON). Defaults to
    /// `mediator_did`; `None` -- configured as `""` -- leaves this agent without a v1
    /// address, so v1 peers can only answer on the same connection.
    pub v1_mediator: Option<String>,
    /// Connections, created invitations and the v1 mediation, kept across restarts.
    /// Defaults to `connections.json` next to the identity file.
    pub state_path: PathBuf,
    /// If set, `send_didcomm_message` and `discover_features` only talk to these DIDs
    /// (and connections with them).
    pub allowed_targets: Option<Vec<String>>,
    /// Check outgoing messages against the registry's schema for their type (when it
    /// has one) before sending.
    pub validate_messages: bool,
    /// The Streamable HTTP transport (`--http`).
    pub http: HttpConfig,
}

/// Default `http.bind`: loopback only.
pub const DEFAULT_HTTP_BIND: &str = "127.0.0.1:8090";

/// Settings for serving MCP over Streamable HTTP (`didcomm-mcp --http`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpConfig {
    /// Address to listen on (default [`DEFAULT_HTTP_BIND`]).
    pub bind: String,
    /// Clients must send `Authorization: Bearer <token>`. Required unless `bind` is a
    /// loopback address: anyone who can reach the endpoint acts with this agent's keys.
    pub auth_token: Option<String>,
    /// `Host` header values to accept, e.g. `["mcp.example.com"]`. Defaults to
    /// loopback names only, which protects local servers against DNS rebinding; list
    /// the server's public hostnames when serving beyond localhost.
    pub allowed_hosts: Option<Vec<String>>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self { bind: DEFAULT_HTTP_BIND.to_string(), auth_token: None, allowed_hosts: None }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpFile {
    bind: Option<String>,
    auth_token: Option<String>,
    allowed_hosts: Option<Vec<String>>,
}

/// The file's shape: everything optional.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    identity_path: Option<PathBuf>,
    /// `""` disables the registry.
    registry_did: Option<String>,
    /// `""` disables mediation.
    mediator_did: Option<String>,
    /// `""` disables v1 mediation.
    v1_mediator: Option<String>,
    state_path: Option<PathBuf>,
    allowed_targets: Option<Vec<String>>,
    validate_messages: Option<bool>,
    #[serde(default)]
    http: HttpFile,
}

impl Config {
    /// Load from the file (if any) and the environment.
    pub fn load(explicit_path: Option<PathBuf>) -> anyhow::Result<Self> {
        let path = explicit_path
            .or_else(|| std::env::var_os("DIDCOMM_MCP_CONFIG").map(PathBuf::from))
            .or_else(|| Some(config_dir()?.join("config.toml")).filter(|p| p.exists()));
        let file = match &path {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
                toml::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?
            }
            None => ConfigFile::default(),
        };
        Ok(Self::resolve(file, |name| std::env::var(name).ok()))
    }

    /// Apply defaults and the environment (`env`) to `file`.
    fn resolve(file: ConfigFile, env: impl Fn(&str) -> Option<String>) -> Self {
        let identity_path = env("DIDCOMM_MCP_IDENTITY")
            .map(PathBuf::from)
            .or(file.identity_path)
            .unwrap_or_else(default_identity_path);
        let registry_did = env("DIDCOMM_MCP_REGISTRY_DID")
            .or(file.registry_did)
            .unwrap_or_else(|| DEFAULT_REGISTRY.to_string());
        let mediator_did = env("DIDCOMM_MCP_MEDIATOR_DID")
            .or(file.mediator_did)
            .unwrap_or_else(|| DEFAULT_MEDIATOR.to_string());
        let v1_mediator = env("DIDCOMM_MCP_V1_MEDIATOR").or(file.v1_mediator).unwrap_or_else(|| mediator_did.clone());
        let state_path = env("DIDCOMM_MCP_STATE")
            .map(PathBuf::from)
            .or(file.state_path)
            .unwrap_or_else(|| identity_path.with_file_name("connections.json"));
        // Comma-separated environment lists.
        let list = |v: String| v.split(',').map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect();
        let allowed_targets = env("DIDCOMM_MCP_ALLOWED_TARGETS").map(list).or(file.allowed_targets);
        let validate_messages = env("DIDCOMM_MCP_VALIDATE_MESSAGES")
            .map(|v| !matches!(v.trim(), "0" | "false" | "no" | "off"))
            .or(file.validate_messages)
            .unwrap_or(true);
        let http = HttpConfig {
            bind: env("DIDCOMM_MCP_HTTP_BIND")
                .or(file.http.bind)
                .unwrap_or_else(|| DEFAULT_HTTP_BIND.to_string()),
            auth_token: env("DIDCOMM_MCP_HTTP_TOKEN").or(file.http.auth_token).filter(|t| !t.is_empty()),
            allowed_hosts: env("DIDCOMM_MCP_HTTP_ALLOWED_HOSTS").map(list).or(file.http.allowed_hosts),
        };
        Self {
            http,
            identity_path,
            registry_did: Some(registry_did).filter(|d| !d.trim().is_empty()),
            mediator_did: Some(mediator_did).filter(|d| !d.trim().is_empty()),
            v1_mediator: Some(v1_mediator).filter(|d| !d.trim().is_empty()),
            state_path,
            allowed_targets,
            validate_messages,
        }
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))
        .map(|d| d.join("didcomm-mcp"))
}

fn default_identity_path() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".local").join("share")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("didcomm-mcp")
        .join("identity.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(toml: &str, env: &[(&str, &str)]) -> Config {
        let file: ConfigFile = toml::from_str(toml).unwrap();
        Config::resolve(file, |name| env.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string()))
    }

    #[test]
    fn defaults() {
        let config = resolve("", &[]);
        assert_eq!(config.mediator_did.as_deref(), Some(DEFAULT_MEDIATOR));
        assert_eq!(config.registry_did.as_deref(), Some(DEFAULT_REGISTRY));
        assert!(config.validate_messages);
        assert!(config.identity_path.ends_with("didcomm-mcp/identity.json"));
    }

    #[test]
    fn environment_overrides_the_file() {
        let config = resolve(
            "registry_did = \"did:example:file\"\nvalidate_messages = true",
            &[("DIDCOMM_MCP_REGISTRY_DID", "did:example:env"), ("DIDCOMM_MCP_VALIDATE_MESSAGES", "false")],
        );
        assert_eq!(config.registry_did.as_deref(), Some("did:example:env"));
        assert!(!config.validate_messages);
    }

    #[test]
    fn empty_registry_disables_the_registry() {
        assert_eq!(resolve("registry_did = \"\"", &[]).registry_did, None);
        assert_eq!(resolve("", &[("DIDCOMM_MCP_REGISTRY_DID", "")]).registry_did, None);
    }

    #[test]
    fn v1_mediation_follows_the_mediator_unless_set() {
        let config = resolve("", &[]);
        assert_eq!(config.v1_mediator.as_deref(), Some(DEFAULT_MEDIATOR));
        assert_eq!(config.state_path, config.identity_path.with_file_name("connections.json"));
        assert_eq!(resolve("mediator_did = \"did:example:m\"", &[]).v1_mediator.as_deref(), Some("did:example:m"));
        assert_eq!(resolve("v1_mediator = \"\"", &[]).v1_mediator, None);
        assert_eq!(resolve("", &[("DIDCOMM_MCP_V1_MEDIATOR", "https://m.example/?oob=x")]).v1_mediator.as_deref(), Some("https://m.example/?oob=x"));
        assert_eq!(resolve("state_path = \"/tmp/s.json\"", &[]).state_path, PathBuf::from("/tmp/s.json"));
    }

    #[test]
    fn empty_mediator_disables_mediation() {
        assert_eq!(resolve("mediator_did = \"\"", &[]).mediator_did, None);
        assert_eq!(resolve("", &[("DIDCOMM_MCP_MEDIATOR_DID", "")]).mediator_did, None);
    }

    #[test]
    fn allowed_targets_from_the_environment() {
        let config = resolve("", &[("DIDCOMM_MCP_ALLOWED_TARGETS", "did:example:a, did:example:b,")]);
        assert_eq!(config.allowed_targets, Some(vec!["did:example:a".to_string(), "did:example:b".to_string()]));
    }

    #[test]
    fn http_settings() {
        let defaults = resolve("", &[]).http;
        assert_eq!(defaults, HttpConfig { bind: DEFAULT_HTTP_BIND.into(), auth_token: None, allowed_hosts: None });

        let configured = resolve(
            "[http]\nbind = \"0.0.0.0:9000\"\nauth_token = \"from-file\"\nallowed_hosts = [\"a.example\"]",
            &[("DIDCOMM_MCP_HTTP_TOKEN", "from-env"), ("DIDCOMM_MCP_HTTP_ALLOWED_HOSTS", "b.example, c.example")],
        )
        .http;
        assert_eq!(configured.bind, "0.0.0.0:9000");
        assert_eq!(configured.auth_token.as_deref(), Some("from-env"));
        assert_eq!(configured.allowed_hosts, Some(vec!["b.example".to_string(), "c.example".to_string()]));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<ConfigFile>("registry = \"typo\"").is_err());
    }
}
