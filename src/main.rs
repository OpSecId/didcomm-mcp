use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use didcomm_agent::{Agent, Identity};
use didcomm_mcp::{bridge::Bridge, config::Config, server::DidcommMcp};
use rmcp::ServiceExt;

/// `--config <path>` is the only argument.
fn config_arg() -> anyhow::Result<Option<PathBuf>> {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (None, _) => Ok(None),
        (Some("--config"), Some(path)) => Ok(Some(PathBuf::from(path))),
        _ => anyhow::bail!("usage: didcomm-mcp [--config <path>]"),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdout carries the MCP protocol; logs go to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Config::load(config_arg()?)?;
    let identity = Identity::load_or_generate(&config.identity_path)
        .with_context(|| format!("identity file {}", config.identity_path.display()))?;
    let bridge = Arc::new(Bridge::new(Agent::new(identity)?, config));
    tracing::info!(did = %bridge.agent().base_did(), "didcomm-mcp starting");

    // Mediate in the background so the MCP handshake isn't held up by the network;
    // tools that need it wait for (or retry) it.
    let background = bridge.clone();
    tokio::spawn(async move {
        match background.ensure_mediation().await {
            Ok(m) => tracing::info!(did = %m.did, mediator = %m.mediator_did, "mediated"),
            Err(e) => tracing::warn!("{e}"),
        }
    });

    let service = DidcommMcp::new(bridge).serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
