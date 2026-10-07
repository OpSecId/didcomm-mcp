use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use didcomm_agent::Features;
use didcomm_mcp::{bridge::Bridge, config::Config, server::DidcommMcp, store::Store};
use rmcp::ServiceExt;

mod service;

const USAGE: &str = "usage: didcomm-mcp [--config <path>] [--http [<address>]]
       didcomm-mcp service install [--user] [--config <path>] [--bind <address>]
       didcomm-mcp service uninstall [--user]

  (default)            serve MCP over stdio, for an MCP host that launches this process
  --http [<address>]   serve MCP over Streamable HTTP at http://<address>/mcp instead
                       (default address: http.bind, 127.0.0.1:8090)
  --config <path>      configuration file (see README)
  service install      run `--http` as a system service (or with --user, a user service)
                       that starts with the machine; see `didcomm-mcp service --help`
  -V, --version        print the version";

pub struct Args {
    pub config: Option<PathBuf>,
    /// `Some(None)`: `--http` without an address.
    pub http: Option<Option<String>>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> anyhow::Result<Args> {
    let mut parsed = Args { config: None, http: None };
    let mut args = args.into_iter().peekable();
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

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("service") => return service::cli(&args[1..]),
        Some("-V" | "--version") => {
            println!("didcomm-mcp {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        #[cfg(windows)]
        Some(service::windows::RUN_AS_SERVICE) => return service::windows::run(&args[1..]),
        _ => {}
    }
    init_logging(std::io::stderr);
    let args = parse_args(args)?;
    tokio::runtime::Runtime::new()?.block_on(run(args, shutdown_signal()))
}

/// Log to `writer` (stdout carries the MCP protocol in stdio mode, so never stdout).
fn init_logging<W>(writer: W)
where
    W: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + Send + Sync + 'static,
{
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
}

/// Ctrl-C, or on Unix also SIGTERM (what systemd and launchd stop a service with).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// Serve MCP over stdio, or with `--http` over HTTP until `shutdown` completes.
pub async fn run(args: Args, shutdown: impl Future<Output = ()> + Send + 'static) -> anyhow::Result<()> {
    let mut config = Config::load(args.config)?;
    if let Some(Some(bind)) = &args.http {
        config.http.bind = bind.clone();
    }
    if args.http.is_some() {
        // Fail on unsafe HTTP settings before touching the identity or the network.
        didcomm_mcp::http::check(&config.http)?;
    }
    let store = match &config.database_url {
        Some(url) => Store::postgres(url).await?,
        None => Store::files(&config.identity_path, &config.state_path),
    };
    let identity = store.load_or_generate_identity().await?;
    // With a public URL, the DID names this server's own /didcomm endpoint.
    let agent = didcomm_mcp::bridge::agent_for(identity, &config)?;
    if config.inbound_endpoint().is_some() && args.http.is_none() {
        tracing::warn!("public_url is set, but DIDComm messages are only received with --http");
    }
    tracing::info!(storage = store.kind(), endpoint = ?config.inbound_endpoint(), "configured");
    let bridge = Arc::new(Bridge::with_store(agent.with_features(Features::standard().with_v1()), config, store).await?);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), did = %bridge.agent().base_did(), "didcomm-mcp starting");

    // Mediate in the background so the MCP handshake isn't held up by the network;
    // tools that need it wait for (or retry) it.
    let background = bridge.clone();
    tokio::spawn(async move {
        match background.ensure_mediation().await {
            Ok(m) => tracing::info!(did = %m.did, mediator = %m.mediator_did, "mediated"),
            // Reachable at its own endpoint instead: nothing to warn about.
            Err(didcomm_mcp::bridge::BridgeError::NoMediator) if background.config().inbound_endpoint().is_some() => {}
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
        return didcomm_mcp::http::serve(bridge, &http, shutdown).await;
    }
    let service = DidcommMcp::new(bridge).serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
