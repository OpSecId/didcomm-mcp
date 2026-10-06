//! `didcomm-mcp service install|uninstall`: run `didcomm-mcp --http` as a service that
//! starts with the machine (or, with `--user`, with the user's session), so MCP hosts
//! connect to it over HTTP instead of launching it.
//!
//! - Linux: a systemd unit (`/etc/systemd/system`, or `~/.config/systemd/user`).
//! - macOS: a launchd daemon (`/Library/LaunchDaemons`) or agent (`~/Library/LaunchAgents`).
//! - Windows: a Windows service (system-wide only).
//!
//! `install` writes a configuration file first if there is none, with a generated
//! bearer token, and is safe to repeat: it updates the definition and restarts the
//! service (e.g. after replacing the binary). `uninstall` leaves the configuration and
//! the identity in place.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use didcomm_mcp::config::{self, Config};

#[cfg(unix)]
mod launchd;
#[cfg(unix)]
mod systemd;
#[cfg(windows)]
pub mod windows;

/// The service's name (systemd unit, Windows service); launchd uses [`LAUNCHD_LABEL`].
pub const NAME: &str = "didcomm-mcp";

const USAGE: &str = "usage: didcomm-mcp service install [--user] [--config <path>] [--bind <address>]
       didcomm-mcp service uninstall [--user]

Runs `didcomm-mcp --http` in the background, started with the machine: a systemd
service on Linux, a launchd daemon on macOS, a Windows service on Windows. Needs
root (sudo) or an Administrator terminal, unless --user.

  --user             install for the current user only (Linux and macOS): it runs while
                     you're logged in, as you, with your usual config and identity
  --config <path>    the configuration file the service uses. Created if it doesn't exist
                     (default: /etc/didcomm-mcp/config.toml, /Library/Application
                     Support/didcomm-mcp/config.toml, %ProgramData%\\didcomm-mcp\\config.toml;
                     with --user, the per-user config file)
  --bind <address>   the HTTP address, when creating the configuration (default 127.0.0.1:8090)

install can be repeated, e.g. after replacing the binary: it updates and restarts the
service. uninstall keeps the configuration and the identity (the agent's keys and DID).";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Machine-wide, started at boot; needs root or Administrator.
    System,
    /// The current user's, started at login (Linux and macOS).
    User,
}

pub fn cli(args: &[String]) -> anyhow::Result<()> {
    let mut args = args.iter();
    let command = args.next().map(String::as_str);
    let (mut scope, mut config, mut bind) = (Scope::System, None, None);
    while let Some(arg) = args.next() {
        let mut value = || args.next().cloned().ok_or_else(|| anyhow::anyhow!("{arg} needs a value\n\n{USAGE}"));
        match arg.as_str() {
            "--user" => scope = Scope::User,
            "--config" => config = Some(PathBuf::from(value()?)),
            "--bind" => bind = Some(value()?),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => anyhow::bail!("unexpected argument {arg:?}\n\n{USAGE}"),
        }
    }
    match command {
        Some("install") => install(scope, config, bind),
        Some("uninstall") if config.is_none() && bind.is_none() => uninstall(scope),
        Some("uninstall") => anyhow::bail!("uninstall only takes --user\n\n{USAGE}"),
        None | Some("-h" | "--help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => anyhow::bail!("unknown service command {other:?}\n\n{USAGE}"),
    }
}

/// Where a system-wide service keeps its configuration and data (identity, connections).
pub fn system_dirs() -> (PathBuf, PathBuf) {
    if cfg!(windows) {
        let dir = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join(NAME);
        (dir.clone(), dir)
    } else if cfg!(target_os = "macos") {
        let dir = PathBuf::from("/Library/Application Support").join(NAME);
        (dir.clone(), dir)
    } else {
        (PathBuf::from("/etc").join(NAME), PathBuf::from("/var/lib").join(NAME))
    }
}

fn install(scope: Scope, config_path: Option<PathBuf>, bind: Option<String>) -> anyhow::Result<()> {
    if cfg!(windows) && scope == Scope::User {
        anyhow::bail!(
            "Windows services run machine-wide: run `didcomm-mcp service install` (without --user) \
             from a terminal opened as Administrator. To run it only while you use it, let your MCP \
             host launch didcomm-mcp instead (see the README)."
        );
    }
    require_root(scope)?;
    let exe = std::env::current_exe().context("finding this executable")?;
    let (config_path, data_dir) = match (scope, config_path) {
        (_, Some(path)) => (std::path::absolute(&path)?, (scope == Scope::System).then(|| system_dirs().1)),
        (Scope::System, None) => (system_dirs().0.join("config.toml"), Some(system_dirs().1)),
        (Scope::User, None) => (config::user_config_path().context("no home directory to put the configuration in")?, None),
    };

    let created = write_config_if_missing(&config_path, scope, bind.as_deref(), data_dir.as_deref())?;
    // Check the configuration before the service tries it (and fails in a log).
    let config = Config::load(Some(config_path.clone()))?;
    didcomm_mcp::http::check(&config.http)?;

    #[cfg(unix)]
    let manage = if cfg!(target_os = "macos") {
        launchd::install(scope, &exe, &config_path)?
    } else {
        systemd::install(scope, &exe, &config_path)?
    };
    #[cfg(windows)]
    let manage = windows::install(&exe, &config_path)?;

    let url = format!("http://{}{}", config.http.bind, didcomm_mcp::http::MCP_PATH);
    let kind = match scope {
        Scope::System => "system service",
        Scope::User => "user service",
    };
    println!("didcomm-mcp {} is installed and running as a {kind}.\n", env!("CARGO_PKG_VERSION"));
    println!("  MCP endpoint   {url}");
    match (&config.http.auth_token, created) {
        (Some(token), true) => println!("  bearer token   {token}"),
        (Some(_), false) => println!("  bearer token   http.auth_token in the configuration"),
        (None, _) => println!("  bearer token   none (http.auth_token isn't set)"),
    }
    println!("  configuration  {}{}", config_path.display(), if created { " (new)" } else { "" });
    println!("  identity       {}", config.identity_path.display());
    println!("\nConnect Claude Code to it:\n");
    match &config.http.auth_token {
        Some(token) if created => println!(
            "  claude mcp add --transport http didcomm {url} --header \"Authorization: Bearer {token}\""
        ),
        Some(_) => println!(
            "  claude mcp add --transport http didcomm {url} --header \"Authorization: Bearer <http.auth_token>\""
        ),
        None => println!("  claude mcp add --transport http didcomm {url}"),
    }
    println!("\n{manage}");
    Ok(())
}

fn uninstall(scope: Scope) -> anyhow::Result<()> {
    require_root(scope)?;
    #[cfg(unix)]
    if cfg!(target_os = "macos") {
        launchd::uninstall(scope)?;
    } else {
        systemd::uninstall(scope)?;
    }
    #[cfg(windows)]
    {
        let _ = scope;
        windows::uninstall()?;
    }
    println!("didcomm-mcp's service is stopped and removed. Its configuration and identity are kept.");
    Ok(())
}

/// A system service on Linux or macOS needs root (Windows says "access denied" itself).
fn require_root(scope: Scope) -> anyhow::Result<()> {
    #[cfg(unix)]
    if scope == Scope::System {
        let uid = Command::new("id").arg("-u").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        if uid.as_deref().is_ok_and(|uid| uid != "0") {
            anyhow::bail!("a system service needs root: run this with sudo, or add --user for a service of your own");
        }
    }
    let _ = scope;
    Ok(())
}

/// Write a starting configuration to `path` unless one exists, with a fresh bearer
/// token, readable only by its owner. Returns whether it wrote one.
fn write_config_if_missing(path: &Path, scope: Scope, bind: Option<&str>, data_dir: Option<&Path>) -> anyhow::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    let dir = path.parent().context("configuration path has no directory")?;
    let new_dir = !dir.exists();
    std::fs::create_dir_all(dir).with_context(|| format!("creating {} (run as root/Administrator?)", dir.display()))?;
    // Don't leave it to the umask: no one else may write here.
    #[cfg(unix)]
    if new_dir {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))?;
    }
    let _ = new_dir;
    #[cfg(windows)]
    if scope == Scope::System {
        windows::restrict_to_administrators(dir)?;
    }
    let _ = scope;
    let text = config_text(bind.unwrap_or(config::DEFAULT_HTTP_BIND), &new_token(), data_dir);
    write_private(path, &text).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

/// A bearer token: 256 random bits, hex.
fn new_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn config_text(bind: &str, token: &str, data_dir: Option<&Path>) -> String {
    let identity = match data_dir {
        Some(dir) => format!(
            "# The agent's keys (its DID). Back this file up; delete it for a new identity.\nidentity_path = {}\n\n",
            toml_string(&dir.join("identity.json").to_string_lossy())
        ),
        None => String::new(),
    };
    format!(
        "# didcomm-mcp configuration, written by `didcomm-mcp service install`.\n\
         # Restart the service after changing it. Every setting:\n\
         # https://github.com/wyvrn-cloud/mcp#configuration\n\
         \n\
         {identity}\
         # registry_did = {registry}\n\
         # mediator_did = {mediator}\n\
         \n\
         [http]\n\
         # MCP is served at http://<bind>/mcp. To serve beyond this machine, bind a public\n\
         # address and list the hostnames clients use in allowed_hosts.\n\
         bind = {bind}\n\
         # Clients send `Authorization: Bearer <auth_token>`.\n\
         auth_token = {token}\n",
        registry = toml_string(config::DEFAULT_REGISTRY),
        mediator = toml_string(config::DEFAULT_MEDIATOR),
        bind = toml_string(bind),
        token = toml_string(token),
    )
}

/// Write `text` to a new file only its owner can read (on Windows the directory's
/// permissions apply).
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(text.as_bytes())
}

/// Run a command, failing with its output if it fails.
#[allow(dead_code)]
fn run(command: &mut Command) -> anyhow::Result<()> {
    let output = command.output().with_context(|| format!("running {command:?}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "{command:?} failed ({}): {}{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim(),
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generated_configuration_loads() {
        let dir = std::env::temp_dir().join(format!("didcomm-mcp-service-{}", uuid::Uuid::new_v4()));
        let path = dir.join("nested").join("config.toml");
        let data = Path::new(if cfg!(windows) { r"C:\ProgramData\didcomm-mcp" } else { "/var/lib/didcomm-mcp" });

        assert!(write_config_if_missing(&path, Scope::User, Some("127.0.0.1:9123"), Some(data)).unwrap());
        assert!(!write_config_if_missing(&path, Scope::User, None, None).unwrap(), "an existing file is kept");

        let file: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["identity_path"].as_str().unwrap(), data.join("identity.json").to_str().unwrap());
        assert_eq!(file["http"]["bind"].as_str(), Some("127.0.0.1:9123"));
        assert_eq!(file["http"]["auth_token"].as_str().unwrap().len(), 64);
        assert!(file.get("registry_did").is_none(), "defaults stay commented out");
        let config = Config::load(Some(path.clone())).unwrap();
        assert_eq!(config.identity_path, data.join("identity.json"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tokens_are_fresh() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert!(a.len() == 64 && a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn arguments() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(cli(&args(&["frobnicate"])).is_err());
        assert!(cli(&args(&["install", "--bind"])).is_err());
        assert!(cli(&args(&["uninstall", "--config", "x"])).is_err());
        assert!(cli(&args(&["--help"])).is_ok());
    }
}
