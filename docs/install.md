# Installing and running didcomm-mcp

didcomm-mcp is a single executable with no runtime dependencies. There are two ways to run it:

- **Launched by your MCP host (stdio).** The host (Claude Code, Claude Desktop, ...)
  starts `didcomm-mcp` when it needs it and talks to it over stdin/stdout. This is the
  simplest setup: nothing runs in the background.
- **As a service (HTTP).** didcomm-mcp runs all the time, started with the machine, and
  hosts connect to `http://127.0.0.1:8090/mcp` with a bearer token. Use this to share one
  agent between several hosts or sessions, or so its mediator picks up messages for it
  even while no host is open.

Each running copy acts as its own DIDComm agent with an identity (keys and DID) kept in a
file. Pick one way per identity: a stdio launch and a service both using the same
identity file would compete for the same messages. (The service and stdio launches use
different identity files by default, so this only happens if you point them at the same
file.)

- [1. Download](#1-download)
- [2. Connect an MCP host](#2-connect-an-mcp-host)
- [3. Configure it](#3-configure-it)
- [4. Run it as a service](#4-run-it-as-a-service)
- [5. Upgrade](#5-upgrade)
- [6. Uninstall](#6-uninstall)
- [Troubleshooting](#troubleshooting)

## 1. Download

Every [release](https://github.com/wyvrn-cloud/mcp/releases) has an archive per platform,
and a `SHA256SUMS` file:

| Platform | Archive |
|---|---|
| Linux x86_64 | `didcomm-mcp-x86_64-unknown-linux-musl.tar.gz` |
| Linux ARM64 (e.g. Raspberry Pi 4/5, Graviton) | `didcomm-mcp-aarch64-unknown-linux-musl.tar.gz` |
| macOS, Apple silicon (M1 and later) | `didcomm-mcp-aarch64-apple-darwin.tar.gz` |
| macOS, Intel | `didcomm-mcp-x86_64-apple-darwin.tar.gz` |
| Windows x64 | `didcomm-mcp-x86_64-pc-windows-msvc.zip` |
| Windows ARM64 | `didcomm-mcp-aarch64-pc-windows-msvc.zip` |

The Linux builds are static, so they run on any distribution (glibc or musl). Each
archive holds `didcomm-mcp` (or `didcomm-mcp.exe`), this guide, the README and the license.

### Linux and macOS

Pick your platform's archive name from the table, then download it and put the binary
in `/usr/local/bin`:

```sh
TARGET=x86_64-unknown-linux-musl     # or aarch64-unknown-linux-musl, aarch64-apple-darwin, x86_64-apple-darwin
curl -fLO https://github.com/wyvrn-cloud/mcp/releases/latest/download/didcomm-mcp-$TARGET.tar.gz
tar xzf didcomm-mcp-$TARGET.tar.gz
sudo install -m 755 didcomm-mcp-$TARGET/didcomm-mcp /usr/local/bin/
didcomm-mcp --version
```

To check the download first:

```sh
curl -fLO https://github.com/wyvrn-cloud/mcp/releases/latest/download/SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS            # Linux
shasum -a 256 --check --ignore-missing SHA256SUMS        # macOS
```

Without `sudo`, put it anywhere on your `PATH` instead, e.g. `~/.local/bin` (but a
*system* service on Linux needs it outside your home directory; see section 4).

> **macOS:** the binaries aren't notarized by Apple. Downloads made with `curl` as above
> run as they are. If you downloaded the archive with a browser, macOS will refuse to
> open it ("cannot be verified"). Clear the quarantine flag once:
> `xattr -d com.apple.quarantine /usr/local/bin/didcomm-mcp`

### Windows

In PowerShell **opened as Administrator** (to write to Program Files):

```powershell
$target = "x86_64-pc-windows-msvc"   # or aarch64-pc-windows-msvc
$dir = "$env:ProgramFiles\didcomm-mcp"
Invoke-WebRequest "https://github.com/wyvrn-cloud/mcp/releases/latest/download/didcomm-mcp-$target.zip" -OutFile "$env:TEMP\didcomm-mcp.zip"
Expand-Archive "$env:TEMP\didcomm-mcp.zip" -DestinationPath "$env:TEMP\didcomm-mcp" -Force
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item "$env:TEMP\didcomm-mcp\didcomm-mcp-$target\didcomm-mcp.exe" $dir
& "$dir\didcomm-mcp.exe" --version
```

Optionally, add it to the system `PATH` (new terminals will then find `didcomm-mcp`):

```powershell
$path = [Environment]::GetEnvironmentVariable("Path", "Machine")
[Environment]::SetEnvironmentVariable("Path", "$path;$dir", "Machine")
```

To check the download: `Get-FileHash "$env:TEMP\didcomm-mcp.zip"` and compare it with
the archive's line in `SHA256SUMS`.

The executable isn't code-signed, so Windows SmartScreen may warn the first time it's
run from Explorer. Running it from a terminal, or as a service, isn't affected.

### From source

With [Rust](https://rustup.rs) installed (a C compiler and CMake are needed too, for
the cryptography library):

```sh
cargo install --locked --git https://github.com/wyvrn-cloud/mcp didcomm-mcp
```

## 2. Connect an MCP host

### Launched by the host (stdio)

**Claude Code:**

```sh
claude mcp add didcomm -- /usr/local/bin/didcomm-mcp
```

On Windows: `claude mcp add didcomm -- "C:\Program Files\didcomm-mcp\didcomm-mcp.exe"`.
Add `--scope user` to have it in every project, and pass settings with `-e`, e.g.
`-e DIDCOMM_MCP_MEDIATOR_DID=did:web:mediator.example.com`.

**Claude Desktop**, and other hosts configured with JSON: add it to the host's
configuration file. For Claude Desktop that's `claude_desktop_config.json`
(Settings → Developer → Edit Config), which lives in
`~/Library/Application Support/Claude/` on macOS and `%APPDATA%\Claude\` on Windows.

```json
{
  "mcpServers": {
    "didcomm": {
      "command": "/usr/local/bin/didcomm-mcp",
      "env": { "RUST_LOG": "info" }
    }
  }
}
```

On Windows, write the path with doubled backslashes:
`"command": "C:\\Program Files\\didcomm-mcp\\didcomm-mcp.exe"`.

Restart the host. On first start didcomm-mcp creates its identity; ask the AI to call
`get_identity` to see its DID.

### A running server (HTTP)

If didcomm-mcp runs as a service (section 4) or you start it yourself with
`didcomm-mcp --http`, connect to its URL with its bearer token:

```sh
claude mcp add --transport http didcomm http://127.0.0.1:8090/mcp \
  --header "Authorization: Bearer <token>"
```

or in a host's JSON configuration (Claude Code's `.mcp.json`, for example):

```json
{
  "mcpServers": {
    "didcomm": {
      "type": "http",
      "url": "http://127.0.0.1:8090/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    }
  }
}
```

`service install` prints the exact command, with its token, when it creates the
configuration.

## 3. Configure it

Everything has a default, so no configuration is needed to start. By default
didcomm-mcp:
- looks protocols up in the registry at `did:web:docs.wyvrn.app`;
- receives messages through the Indicio public mediator. That mediator is free and fine
  for trying things out, but it's meant for development and demos: use your own for
  anything real.

Settings come from a TOML file, overridden by environment variables. The file is the
one given with `--config <path>`, else `$DIDCOMM_MCP_CONFIG`, else the per-user file if
it exists:

| | Per-user configuration | Identity (keys) |
|---|---|---|
| Linux, macOS | `~/.config/didcomm-mcp/config.toml` | `~/.local/share/didcomm-mcp/identity.json` |
| Windows | `%APPDATA%\didcomm-mcp\config.toml` | `%LOCALAPPDATA%\didcomm-mcp\identity.json` |

An example with the common settings:

```toml
# The documentation registry ("" disables it; sends are then not schema-checked).
registry_did = "did:web:docs.wyvrn.app"
# The mediator that receives messages for this agent ("" disables mediation).
mediator_did = "did:web:mediator.example.com"
# Only let the AI talk to these DIDs (and connections with them).
allowed_targets = ["did:web:partner.example.com"]

[http]                    # only used with --http, i.e. as a service
bind = "127.0.0.1:8090"
auth_token = "a long random string"
```

Every setting and its environment variable is listed in the
[README](../README.md#configuration). After changing the configuration, restart the
host (stdio) or the service.

**The identity file is the agent's private key.** Keep it to keep the same DID (back it
up if peers know this agent); delete it to get a new identity on the next start. It's
created readable by its owner only.

## 4. Run it as a service

`didcomm-mcp service install` sets didcomm-mcp up to run `--http` in the background,
started with the machine:

| | System service (default) | User service (`--user`) |
|---|---|---|
| Linux | systemd system unit, runs as the `didcomm-mcp` system user | systemd user unit, runs as you |
| macOS | launchd daemon, runs as root | launchd agent, runs as you while you're logged in |
| Windows | Windows service, runs as LocalSystem | not available |
| Needs | `sudo` / an Administrator terminal | nothing |

It:
1. writes a configuration file, unless the one it's given already exists, with a new
   random bearer token, readable only by its owner (and by the service);
2. registers the service and starts it;
3. prints the MCP URL, the token and the command to connect Claude Code.

```sh
sudo didcomm-mcp service install            # Linux and macOS, system-wide
didcomm-mcp service install --user          # Linux and macOS, just for you
```

```powershell
# Windows, in PowerShell opened as Administrator
& "$env:ProgramFiles\didcomm-mcp\didcomm-mcp.exe" service install
```

Options:
- `--config <path>`: use (or create) this configuration file instead of the default
  location below.
- `--bind <address>`: the address to listen on, when creating the configuration
  (default `127.0.0.1:8090`).

Install where the binary will stay: the service runs the executable you ran `install`
with. On Linux the system service can't see home directories or `/tmp`, so put the
binary in `/usr/local/bin` first; `install` refuses otherwise.

### Where things are

| | Configuration | Identity, connections | Logs |
|---|---|---|---|
| Linux, system | `/etc/didcomm-mcp/config.toml` | `/var/lib/didcomm-mcp/` | `journalctl -u didcomm-mcp` |
| Linux, `--user` | `~/.config/didcomm-mcp/config.toml` | `~/.local/share/didcomm-mcp/` | `journalctl --user -u didcomm-mcp` |
| macOS, system | `/Library/Application Support/didcomm-mcp/config.toml` | same folder | `/Library/Logs/didcomm-mcp.log` |
| macOS, `--user` | `~/.config/didcomm-mcp/config.toml` | `~/.local/share/didcomm-mcp/` | `~/Library/Logs/didcomm-mcp.log` |
| Windows | `%ProgramData%\didcomm-mcp\config.toml` | same folder | `%ProgramData%\didcomm-mcp\didcomm-mcp.log` |

A `--user` service shares your per-user configuration and identity with stdio launches,
so connect your hosts to it over HTTP rather than also launching didcomm-mcp from them.

On Windows the `%ProgramData%\didcomm-mcp` folder is restricted to SYSTEM and
Administrators, because it holds the token and the keys; read the token from an
Administrator terminal.

### Managing it

**Linux:**

```sh
sudo systemctl status didcomm-mcp
sudo systemctl restart didcomm-mcp          # after editing the configuration
sudo journalctl -u didcomm-mcp -f           # follow the logs
```

With `--user`, use `systemctl --user` and `journalctl --user` instead, without `sudo`.
A user service runs while you're logged in; to keep it running after you log out (on a
server, say): `sudo loginctl enable-linger $USER`.

**macOS:**

```sh
sudo launchctl print system/app.wyvrn.didcomm-mcp
sudo launchctl kickstart -k system/app.wyvrn.didcomm-mcp     # restart
tail -f /Library/Logs/didcomm-mcp.log
```

With `--user`: `launchctl print gui/$(id -u)/app.wyvrn.didcomm-mcp`, and so on, without
`sudo`, and the log is in `~/Library/Logs/`.

**Windows** (PowerShell as Administrator; or the Services app, `services.msc`, where it's
listed as "DIDComm MCP server"):

```powershell
Get-Service didcomm-mcp
Restart-Service didcomm-mcp
Get-Content -Wait "$env:ProgramData\didcomm-mcp\didcomm-mcp.log"
```

The service restarts by itself if it fails.

### Serving beyond this machine

By default the service only listens on `127.0.0.1`. To let other machines connect:

1. Set `bind` to a reachable address (e.g. `"0.0.0.0:8090"`). A bearer token is then
   mandatory; didcomm-mcp refuses to start without one.
2. List the hostnames clients will use in `allowed_hosts` (e.g.
   `["mcp.example.com"]`). Requests with other `Host` headers are refused, which
   protects against DNS rebinding.
3. Put TLS in front of it with a reverse proxy (Caddy, nginx, ...). didcomm-mcp itself
   only speaks plain HTTP, and the token must not cross a network unencrypted.

Anyone with the token acts with this agent's keys, so treat it like a password.

## 5. Upgrade

Replace the binary with the new release's, then, if it runs as a service, run
`service install` again: it keeps the configuration and identity, updates the service
and restarts it.

```sh
sudo install -m 755 didcomm-mcp-$TARGET/didcomm-mcp /usr/local/bin/
sudo didcomm-mcp service install
```

On Windows, stop the service first so the executable can be replaced:

```powershell
Stop-Service didcomm-mcp
Copy-Item "$env:TEMP\didcomm-mcp\didcomm-mcp-$target\didcomm-mcp.exe" "$env:ProgramFiles\didcomm-mcp"
& "$env:ProgramFiles\didcomm-mcp\didcomm-mcp.exe" service install
```

Hosts that launch it over stdio pick up the new binary the next time they start it.

## 6. Uninstall

```sh
sudo didcomm-mcp service uninstall          # or: didcomm-mcp service uninstall --user
sudo rm /usr/local/bin/didcomm-mcp
```

```powershell
& "$env:ProgramFiles\didcomm-mcp\didcomm-mcp.exe" service uninstall
Remove-Item -Recurse "$env:ProgramFiles\didcomm-mcp"
```

`service uninstall` stops and removes the service, but keeps the configuration and the
identity, in case you reinstall. Delete those folders (section 4's table) to remove
them too; the identity's DID is gone for good once its file is deleted.

## Troubleshooting

- **The host says the server failed to start.** Run the same command in a terminal: it
  logs to stderr, and reports a bad configuration file or identity path. `RUST_LOG=debug`
  logs more.
- **`refusing to serve on 0.0.0.0:8090 without an auth token`.** Set `http.auth_token`,
  or bind to `127.0.0.1`.
- **HTTP clients get `401`.** The `Authorization: Bearer <token>` header is missing or
  doesn't match `http.auth_token`.
- **HTTP clients get `403`.** The host name in the URL isn't in `allowed_hosts` (only
  loopback names are allowed by default).
- **"mediation ... failed" in the logs.** The mediator couldn't be reached. Everything
  else still works, but messages to this agent can only arrive as direct replies until
  it can. didcomm-mcp retries when a tool needs the mediator.
- **Linux: `service install` says the binary is under /home.** Copy it to
  `/usr/local/bin` and run `sudo /usr/local/bin/didcomm-mcp service install`.
- **Windows: "access denied".** Open PowerShell with "Run as administrator".
