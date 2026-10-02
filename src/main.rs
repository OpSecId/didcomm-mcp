use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use didcomm_agent::{Agent, Features, Identity};
use didcomm_mcp::{bridge::Bridge, config::Config, server::DidcommMcp};
use rmcp::ServiceExt;

const USAGE: &str = "usage: didcomm-mcp [--config <path>] [--http [<address>]]

  (default)            serve MCP over stdio, for an MCP host that launches this process
  --http [<address>]   serve MCP over Streamable HTTP at http://<address>/mcp instead
                       (default address: http.bind, 127.0.0.1:8090)
  --config <path>      configuration file (see README)";

struct Args {
    config: Option<PathBuf>,
    /// `Some(None)`: `--http` without an address.
    http: Option<Option<String>>,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut parsed = Args { config: None, http: None };
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => parsed.config = Some(PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!(USAGE))?)),
            "--http" => parsed.http = Some(args.next_if(|a| !a.starts_with("--"))),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => anyhow::bail!("unexpected argument {arg:?}\n{USAGE}"),
        }
    }
    Ok(parsed)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdout carries the MCP protocol in stdio mode; logs always go to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = parse_args()?;
    let mut config = Config::load(args.config)?;
    if let Some(Some(bind)) = &args.http {
        config.http.bind = bind.clone();
    }
    if args.http.is_some() {
        // Fail on unsafe HTTP settings before touching the identity or the network.
        didcomm_mcp::http::check(&config.http)?;
    }
    let identity = Identity::load_or_generate(&config.identity_path)
        .with_context(|| format!("identity file {}", config.identity_path.display()))?;
    let bridge = Arc::new(Bridge::new(Agent::new(identity)?.with_features(Features::standard().with_v1()), config));
    tracing::info!(did = %bridge.agent().base_did(), "didcomm-mcp starting");

    // Mediate in the background so the MCP handshake isn't held up by the network;
    // tools that need it wait for (or retry) it.
    let background = bridge.clone();
    tokio::spawn(async move {
        match background.ensure_mediation().await {
            Ok(m) => tracing::info!(did = %m.did, mediator = %m.mediator_did, "mediated"),
            Err(e) => tracing::warn!("{e}"),
        }
        match background.ensure_v1_mediation().await {
            Ok(m) => tracing::info!(did = %background.agent().v1_did(), endpoint = %m.endpoint, "mediated for DIDComm v1"),
            Err(didcomm_mcp::bridge::BridgeError::NoV1Mediator) => {}
            Err(e) => tracing::warn!("{e}"),
        }
    });

    if args.http.is_some() {
        let http = bridge.config().http.clone();
        return didcomm_mcp::http::serve(bridge, &http).await;
    }
    let service = DidcommMcp::new(bridge).serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
