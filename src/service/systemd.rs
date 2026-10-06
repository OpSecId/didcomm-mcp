//! Linux: a systemd unit.
//!
//! The system service runs as the unprivileged system user `didcomm-mcp` (created on
//! install), with its data in `/var/lib/didcomm-mcp` (`StateDirectory`). Its
//! configuration holds the bearer token, so only root and that user's group can read it.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;

use super::{run, Scope, NAME};

fn unit_path(scope: Scope) -> anyhow::Result<PathBuf> {
    Ok(match scope {
        Scope::System => PathBuf::from("/etc/systemd/system").join(format!("{NAME}.service")),
        Scope::User => std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .context("no home directory")?
            .join("systemd/user")
            .join(format!("{NAME}.service")),
    })
}

fn systemctl(scope: Scope) -> Command {
    let mut command = Command::new("systemctl");
    if scope == Scope::User {
        command.arg("--user");
    }
    command
}

/// One argument on an `ExecStart=` line: quoted, with systemd's specifiers (`%`) and
/// variable expansion (`$`) escaped.
fn exec_arg(arg: &str) -> String {
    let escaped = arg.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%").replace('$', "$$");
    format!("\"{escaped}\"")
}

pub fn unit(scope: Scope, exe: &Path, config: &Path) -> String {
    let exe = exec_arg(&exe.to_string_lossy());
    let common = "Restart=on-failure\nRestartSec=5\n";
    match scope {
        Scope::System => format!(
            "[Unit]\n\
             Description=didcomm-mcp: DIDComm MCP server (Streamable HTTP)\n\
             Documentation=https://github.com/wyvrn-cloud/mcp\n\
             Wants=network-online.target\n\
             After=network-online.target\n\
             \n\
             [Service]\n\
             ExecStart={exe} --http --config {config}\n\
             User={NAME}\n\
             Group={NAME}\n\
             StateDirectory={NAME}\n\
             StateDirectoryMode=0700\n\
             WorkingDirectory=/var/lib/{NAME}\n\
             # Where the identity goes if the configuration doesn't say.\n\
             Environment=XDG_DATA_HOME=/var/lib\n\
             {common}\
             NoNewPrivileges=yes\n\
             ProtectSystem=strict\n\
             ProtectHome=yes\n\
             PrivateTmp=yes\n\
             PrivateDevices=yes\n\
             ProtectKernelTunables=yes\n\
             ProtectKernelModules=yes\n\
             ProtectControlGroups=yes\n\
             RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX\n\
             LockPersonality=yes\n\
             \n\
             [Install]\n\
             WantedBy=multi-user.target\n",
            config = exec_arg(&config.to_string_lossy()),
        ),
        Scope::User => format!(
            "[Unit]\n\
             Description=didcomm-mcp: DIDComm MCP server (Streamable HTTP)\n\
             Documentation=https://github.com/wyvrn-cloud/mcp\n\
             \n\
             [Service]\n\
             ExecStart={exe} --http --config {config}\n\
             {common}\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            config = exec_arg(&config.to_string_lossy()),
        ),
    }
}

/// Install (or update) and (re)start the unit; returns how to manage it.
pub fn install(scope: Scope, exe: &Path, config: &Path) -> anyhow::Result<String> {
    if scope == Scope::System {
        // ProtectHome hides home directories (and PrivateTmp, /tmp) from the service.
        for path in [exe, config] {
            if let Some(hidden) = ["/home", "/root", "/tmp", "/var/tmp", "/run/user"].into_iter().find(|h| path.starts_with(h)) {
                anyhow::bail!(
                    "{} is under {hidden}, which the system service can't see. Put the binary in \
                     /usr/local/bin (`sudo install -m 755 didcomm-mcp /usr/local/bin/`) and the \
                     configuration under /etc, then run install again.",
                    path.display()
                );
            }
        }
        create_system_user()?;
        share_with_service(config)?;
    }
    let path = unit_path(scope)?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, unit(scope, exe, config))
        .with_context(|| format!("writing {} (run with sudo, or use --user?)", path.display()))?;
    run(systemctl(scope).arg("daemon-reload"))?;
    run(systemctl(scope).args(["enable", NAME]))?;
    run(systemctl(scope).args(["restart", NAME]))?;

    let (ctl, journal) = match scope {
        Scope::System => ("sudo systemctl", "sudo journalctl -u"),
        Scope::User => ("systemctl --user", "journalctl --user -u"),
    };
    let mut manage = format!(
        "Manage it with systemd:\n\n  {ctl} status {NAME}\n  {ctl} restart {NAME}    # after editing the configuration\n  {journal} {NAME} -f    # logs\n\nUnit file: {}",
        path.display()
    );
    if scope == Scope::User {
        manage.push_str(&format!(
            "\nIt runs while you're logged in. To keep it running after you log out: loginctl enable-linger {}",
            std::env::var("USER").unwrap_or_else(|_| "$USER".into())
        ));
    }
    Ok(manage)
}

/// The `didcomm-mcp` system user and group, unless they exist.
fn create_system_user() -> anyhow::Result<()> {
    if Command::new("id").arg("-u").arg(NAME).output().is_ok_and(|o| o.status.success()) {
        return Ok(());
    }
    let shell = ["/usr/sbin/nologin", "/sbin/nologin", "/usr/bin/nologin"]
        .into_iter()
        .find(|s| Path::new(s).exists())
        .unwrap_or("/bin/false");
    run(Command::new("useradd")
        .args(["--system", "--user-group", "--no-create-home", "--home-dir"])
        .arg(format!("/var/lib/{NAME}"))
        .args(["--shell", shell, NAME]))
    .context("creating the didcomm-mcp system user (run with sudo?)")
}

/// Let the service's group read the configuration (and no one else but root).
fn share_with_service(config: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    run(Command::new("chgrp").arg(NAME).arg(config))?;
    let mode = std::fs::metadata(config)?.permissions().mode();
    std::fs::set_permissions(config, std::fs::Permissions::from_mode((mode & 0o750) | 0o040))
        .with_context(|| format!("setting permissions on {}", config.display()))
}

pub fn uninstall(scope: Scope) -> anyhow::Result<()> {
    let path = unit_path(scope)?;
    if !path.exists() {
        anyhow::bail!("{NAME} isn't installed ({} doesn't exist)", path.display());
    }
    run(systemctl(scope).args(["disable", "--now", NAME]))?;
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    run(systemctl(scope).arg("daemon-reload"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_unit() {
        let unit = unit(Scope::System, Path::new("/usr/local/bin/didcomm-mcp"), Path::new("/etc/didcomm-mcp/config.toml"));
        assert!(unit.contains(
            "ExecStart=\"/usr/local/bin/didcomm-mcp\" --http --config \"/etc/didcomm-mcp/config.toml\"\n"
        ));
        assert!(unit.contains("User=didcomm-mcp\n") && unit.contains("StateDirectory=didcomm-mcp\n"));
        assert!(unit.contains("WantedBy=multi-user.target\n"));
    }

    #[test]
    fn user_unit_quotes_and_escapes() {
        let unit = unit(Scope::User, Path::new("/opt/my tools/didcomm-mcp"), Path::new("/home/a/100%/$x.toml"));
        assert!(unit.contains("ExecStart=\"/opt/my tools/didcomm-mcp\" --http --config \"/home/a/100%%/$$x.toml\"\n"));
        assert!(unit.contains("WantedBy=default.target\n"));
    }
}
