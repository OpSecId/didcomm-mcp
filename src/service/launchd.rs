//! macOS: a launchd daemon (system-wide, runs as root) or agent (`--user`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::Context;

use super::{run, Scope};

pub const LABEL: &str = "app.wyvrn.didcomm-mcp";

fn home() -> anyhow::Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).context("no home directory")
}

fn plist_path(scope: Scope) -> anyhow::Result<PathBuf> {
    let dir = match scope {
        Scope::System => PathBuf::from("/Library/LaunchDaemons"),
        Scope::User => home()?.join("Library/LaunchAgents"),
    };
    Ok(dir.join(format!("{LABEL}.plist")))
}

fn log_path(scope: Scope) -> anyhow::Result<PathBuf> {
    Ok(match scope {
        Scope::System => PathBuf::from("/Library/Logs/didcomm-mcp.log"),
        Scope::User => home()?.join("Library/Logs/didcomm-mcp.log"),
    })
}

/// The launchd domain: `system`, or the logged-in user's `gui/<uid>`.
fn domain(scope: Scope) -> anyhow::Result<String> {
    match scope {
        Scope::System => Ok("system".into()),
        Scope::User => {
            let output = Command::new("id").arg("-u").output().context("running id -u")?;
            Ok(format!("gui/{}", String::from_utf8_lossy(&output.stdout).trim()))
        }
    }
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn plist(scope: Scope, exe: &Path, config: &Path, log: &Path) -> String {
    let string = |s: &str| format!("<string>{}</string>", xml(s));
    let arguments: String = [&*exe.to_string_lossy(), "--http", "--config", &config.to_string_lossy()]
        .iter()
        .map(|a| format!("\n    {}", string(a)))
        .collect();
    // A daemon has no HOME: say where the identity goes if the configuration doesn't.
    let environment = match scope {
        Scope::System => format!(
            "\n  <key>EnvironmentVariables</key>\n  <dict>\n    <key>XDG_DATA_HOME</key>\n    {}\n  </dict>",
            string("/Library/Application Support")
        ),
        Scope::User => String::new(),
    };
    let log = string(&log.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  {label}
  <key>ProgramArguments</key>
  <array>{arguments}
  </array>{environment}
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>10</integer>
  <key>StandardOutPath</key>
  {log}
  <key>StandardErrorPath</key>
  {log}
</dict>
</plist>
"#,
        label = string(LABEL),
    )
}

/// Stop and unload the job if it's loaded, and wait until launchd has let go of it
/// (bootout returns before it has).
fn bootout(domain: &str) {
    let target = format!("{domain}/{LABEL}");
    if !Command::new("launchctl").args(["bootout", &target]).output().is_ok_and(|o| o.status.success()) {
        return; // not loaded
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline
        && Command::new("launchctl").args(["print", &target]).output().is_ok_and(|o| o.status.success())
    {
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Install (or update) and (re)start the job; returns how to manage it.
pub fn install(scope: Scope, exe: &Path, config: &Path) -> anyhow::Result<String> {
    let (path, log, domain) = (plist_path(scope)?, log_path(scope)?, domain(scope)?);
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::create_dir_all(log.parent().unwrap())?;
    std::fs::write(&path, plist(scope, exe, config, &log))
        .with_context(|| format!("writing {} (run with sudo, or use --user?)", path.display()))?;
    // Replace a running instance; it's fine if there is none.
    bootout(&domain);
    let _ = Command::new("launchctl").args(["enable", &format!("{domain}/{LABEL}")]).output();
    // Right after a bootout, bootstrap can still fail ("Input/output error") while
    // launchd finishes tearing the old job down.
    let mut attempts = 0;
    loop {
        match run(Command::new("launchctl").args(["bootstrap", &domain]).arg(&path)) {
            Err(_) if attempts < 10 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(500));
            }
            result => break result?,
        }
    }

    let sudo = if scope == Scope::System { "sudo " } else { "" };
    Ok(format!(
        "Manage it with launchctl:\n\n  {sudo}launchctl print {domain}/{LABEL}\n  {sudo}launchctl kickstart -k {domain}/{LABEL}    # restart, after editing the configuration\n  tail -f {}    # logs\n\nJob file: {}",
        log.display(),
        path.display()
    ))
}

pub fn uninstall(scope: Scope) -> anyhow::Result<()> {
    let path = plist_path(scope)?;
    if !path.exists() {
        anyhow::bail!("didcomm-mcp isn't installed ({} doesn't exist)", path.display());
    }
    bootout(&domain(scope)?);
    std::fs::remove_file(&path).with_context(|| format!("removing {} (run with sudo?)", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_plist() {
        let plist = plist(
            Scope::System,
            Path::new("/usr/local/bin/didcomm-mcp"),
            Path::new("/Library/Application Support/didcomm-mcp/config.toml"),
            Path::new("/Library/Logs/didcomm-mcp.log"),
        );
        assert!(plist.contains("<string>app.wyvrn.didcomm-mcp</string>"));
        assert!(plist.contains(
            "<string>/usr/local/bin/didcomm-mcp</string>\n    <string>--http</string>\n    <string>--config</string>\n    <string>/Library/Application Support/didcomm-mcp/config.toml</string>\n  </array>"
        ));
        assert!(plist.contains("<key>XDG_DATA_HOME</key>"));
    }

    #[test]
    fn agent_plist_escapes() {
        let plist = plist(Scope::User, Path::new("/Users/a&b/didcomm-mcp"), Path::new("/x.toml"), Path::new("/l.log"));
        assert!(plist.contains("<string>/Users/a&amp;b/didcomm-mcp</string>"));
        assert!(!plist.contains("EnvironmentVariables"));
    }
}
