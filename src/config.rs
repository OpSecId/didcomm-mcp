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
    /// If set, `send_didcomm_message` and `discover_features` only talk to these DIDs.
    pub allowed_targets: Option<Vec<String>>,
    /// Check outgoing messages against the registry's schema for their type (when it
    /// has one) before sending.
    pub validate_messages: bool,
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
    allowed_targets: Option<Vec<String>>,
    validate_messages: Option<bool>,
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
        let allowed_targets = env("DIDCOMM_MCP_ALLOWED_TARGETS")
            .map(|list| list.split(',').map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect())
            .or(file.allowed_targets);
        let validate_messages = env("DIDCOMM_MCP_VALIDATE_MESSAGES")
            .map(|v| !matches!(v.trim(), "0" | "false" | "no" | "off"))
            .or(file.validate_messages)
            .unwrap_or(true);
        Self {
            identity_path,
            registry_did: Some(registry_did).filter(|d| !d.trim().is_empty()),
            mediator_did: Some(mediator_did).filter(|d| !d.trim().is_empty()),
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
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<ConfigFile>("registry = \"typo\"").is_err());
    }
}
