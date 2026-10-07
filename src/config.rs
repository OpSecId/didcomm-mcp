//! Configuration: an optional TOML file, then environment-variable overrides.
//!
//! File location: the `--config <path>` argument, else `$DIDCOMM_MCP_CONFIG`, else
//! [`user_config_path`] if it exists: `$XDG_CONFIG_HOME/didcomm-mcp/config.toml`
//! (`~/.config/...`) on Linux and macOS, `%APPDATA%\didcomm-mcp\config.toml` on Windows.
//! Every setting has a default, so no file is needed at all.

use std::path::PathBuf;

use serde::Deserialize;

/// How the agent's DID is made (`did_method`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DidMethod {
    /// `did:peer:4`, naming the endpoint (or the mediator's routing DID).
    #[default]
    Peer,
    /// `did:web` for `public_url`, whose document this server publishes.
    Web,
}

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
    /// The public base URL this server is reached at (e.g. `https://agent.example`). If
    /// set, `--http` also accepts DIDComm messages at `<public_url>/didcomm`, and the
    /// agent's DID names that endpoint, so peers can deliver straight to it without a
    /// mediator.
    pub public_url: Option<String>,
    /// A Postgres URL. If set, the identity, the state and the inbox live in the
    /// database instead of `identity_path` / `state_path`.
    pub database_url: Option<String>,
    /// How the agent's DID is made when `public_url` is set: a `did:peer:4` naming the
    /// endpoint (default), or a `did:web` for the public URL's host (and path), whose
    /// document `--http` serves itself.
    pub did_method: DidMethod,
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
    public_url: Option<String>,
    database_url: Option<String>,
    did_method: Option<DidMethod>,
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
            public_url: env("DIDCOMM_MCP_PUBLIC_URL")
                .or(file.public_url)
                .map(|u| u.trim().trim_end_matches('/').to_string())
                .filter(|u| !u.is_empty()),
            // DATABASE_URL too: what Railway, Heroku and most hosts set for a database.
            database_url: env("DIDCOMM_MCP_DATABASE_URL")
                .or_else(|| env("DATABASE_URL"))
                .or(file.database_url)
                .filter(|u| !u.trim().is_empty()),
            did_method: env("DIDCOMM_MCP_DID_METHOD")
                .and_then(|m| match m.trim().to_ascii_lowercase().as_str() {
                    "web" => Some(DidMethod::Web),
                    "peer" | "" => Some(DidMethod::Peer),
                    other => {
                        tracing::warn!("ignoring DIDCOMM_MCP_DID_METHOD={other:?} (expected peer or web)");
                        None
                    }
                })
                .or(file.did_method)
                .unwrap_or_default(),
        }
    }

    /// The agent's `did:web`, if `did_method` is `web` and `public_url` is set:
    /// `https://host[:port][/path]` becomes `did:web:host[%3Aport][:path...]`.
    pub fn web_did(&self) -> Option<String> {
        if self.did_method != DidMethod::Web {
            return None;
        }
        let url = self.public_url.as_deref()?;
        let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let mut did = format!("did:web:{}", authority.replace(':', "%3A"));
        for segment in path.split('/').filter(|s| !s.is_empty()) {
            did.push(':');
            did.push_str(segment);
        }
        Some(did)
    }

    /// Where `--http` serves the `did:web` document, relative to `public_url`:
    /// `/.well-known/did.json`, or `/did.json` under a path (did:web's mapping).
    pub fn did_document_path(&self) -> Option<String> {
        self.web_did()?;
        let url = self.public_url.as_deref()?;
        let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
        let has_path = rest.split_once('/').is_some_and(|(_, p)| !p.trim_matches('/').is_empty());
        Some(if has_path { "/did.json".into() } else { "/.well-known/did.json".into() })
    }

    /// Where peers deliver DIDComm messages, if [`public_url`](Self::public_url) is set.
    pub fn inbound_endpoint(&self) -> Option<String> {
        self.public_url.as_ref().map(|u| format!("{u}{}", crate::http::DIDCOMM_PATH))
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home() -> Option<PathBuf> {
    env_path("HOME").or_else(|| env_path("USERPROFILE"))
}

/// The per-user configuration file, read when no other is named:
/// `$XDG_CONFIG_HOME/didcomm-mcp/config.toml` (`~/.config/...`), or on Windows
/// `%APPDATA%\didcomm-mcp\config.toml`.
pub fn user_config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.toml"))
}

fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        env_path("APPDATA")
    } else {
        env_path("XDG_CONFIG_HOME").or_else(|| home().map(|h| h.join(".config")))
    };
    base.map(|d| d.join("didcomm-mcp"))
}

/// `$XDG_DATA_HOME/didcomm-mcp/identity.json` (`~/.local/share/...`), or on Windows
/// `%LOCALAPPDATA%\didcomm-mcp\identity.json`: keys stay on this machine.
fn default_identity_path() -> PathBuf {
    let base = if cfg!(windows) {
        env_path("LOCALAPPDATA").or_else(|| env_path("APPDATA"))
    } else {
        env_path("XDG_DATA_HOME").or_else(|| home().map(|h| h.join(".local").join("share")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("didcomm-mcp").join("identity.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
        assert!(config.identity_path.ends_with(Path::new("didcomm-mcp").join("identity.json")));
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
    fn public_url_and_database() {
        let config = resolve("", &[]);
        assert_eq!((config.inbound_endpoint(), config.public_url, config.database_url), (None, None, None));

        let config = resolve("", &[("DIDCOMM_MCP_PUBLIC_URL", "https://a.example/"), ("DATABASE_URL", "postgres://h/db")]);
        assert_eq!(config.public_url.as_deref(), Some("https://a.example"));
        assert_eq!(config.inbound_endpoint().as_deref(), Some("https://a.example/didcomm"));
        assert_eq!(config.database_url.as_deref(), Some("postgres://h/db"));

        let config = resolve("", &[("DIDCOMM_MCP_DATABASE_URL", "postgres://mine/db"), ("DATABASE_URL", "postgres://h/db")]);
        assert_eq!(config.database_url.as_deref(), Some("postgres://mine/db"));
        assert_eq!(resolve("", &[("DIDCOMM_MCP_PUBLIC_URL", " ")]).public_url, None);
    }

    #[test]
    fn did_web_from_the_public_url() {
        assert_eq!(resolve("", &[("DIDCOMM_MCP_PUBLIC_URL", "https://agent.didcomm.link")]).web_did(), None, "peer by default");
        let web = |url: &str| {
            let c = resolve("", &[("DIDCOMM_MCP_PUBLIC_URL", url), ("DIDCOMM_MCP_DID_METHOD", "web")]);
            (c.web_did().unwrap(), c.did_document_path().unwrap())
        };
        assert_eq!(web("https://agent.didcomm.link/"), ("did:web:agent.didcomm.link".into(), "/.well-known/did.json".into()));
        assert_eq!(web("http://127.0.0.1:8090"), ("did:web:127.0.0.1%3A8090".into(), "/.well-known/did.json".into()));
        assert_eq!(web("https://didcomm.link/agents/alice"), ("did:web:didcomm.link:agents:alice".into(), "/did.json".into()));
        assert_eq!(resolve("did_method = \"web\"", &[]).web_did(), None, "needs a public_url");
        assert_eq!(resolve("did_method = \"web\"", &[]).did_method, DidMethod::Web);
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
