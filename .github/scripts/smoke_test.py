#!/usr/bin/env python3
"""Smoke-test a didcomm-mcp release binary on the machine it was built for.

  smoke_test.py <binary> [--version X.Y.Z] [--service]

- `--version` prints the expected version.
- Over stdio, as an MCP host runs it: initialize and list the tools (offline: no
  registry, no mediators).
- With --service (needs root, or an Administrator shell on Windows): `service install`
  starts it as a system service; /healthz answers, MCP needs the printed bearer token;
  installing again restarts it with the same configuration; `service uninstall` stops it.
  Run it on the binary at its installed location (not under a home directory on Linux).
"""

import argparse
import json
import os
import platform
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

BASE = "http://127.0.0.1:8090"


def step(text):
    print(f"--- {text}", flush=True)


def fail(text, detail=""):
    print(f"FAIL: {text}\n{detail}", flush=True)
    show_service_logs()
    sys.exit(1)


def check(condition, text, detail=""):
    if not condition:
        fail(text, detail)
    print(f"  ok: {text}", flush=True)


def stdio(binary):
    with tempfile.TemporaryDirectory() as tmp:
        env = dict(os.environ,
                   DIDCOMM_MCP_IDENTITY=os.path.join(tmp, "identity.json"),
                   DIDCOMM_MCP_REGISTRY_DID="", DIDCOMM_MCP_MEDIATOR_DID="", DIDCOMM_MCP_V1_MEDIATOR="",
                   RUST_LOG="warn")
        messages = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize",
             "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "0"}}},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
        ]
        proc = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, text=True)
        replies = {}
        for message in messages:
            proc.stdin.write(json.dumps(message) + "\n")
            proc.stdin.flush()
            if "id" in message:
                reply = json.loads(proc.stdout.readline())
                replies[reply["id"]] = reply
        proc.stdin.close()
        proc.wait(timeout=20)
        check(replies[1]["result"]["serverInfo"]["name"] == "didcomm-mcp", "stdio: initialize", replies[1])
        tools = [t["name"] for t in replies[2]["result"]["tools"]]
        check("send_didcomm_message" in tools and len(tools) >= 10, f"stdio: {len(tools)} tools", tools)
        check(os.path.exists(os.path.join(tmp, "identity.json")), "stdio: identity created")


def http(path, body=None, token=None, timeout=5):
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request(BASE + path, data=data, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, response.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def wait_until(predicate, seconds):
    deadline = time.time() + seconds
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(0.5)
    return False


def healthy():
    try:
        return http("/healthz", timeout=2)[0] == 200
    except OSError:
        return False


def install(binary):
    out = subprocess.run([binary, "service", "install"], capture_output=True, text=True)
    print(out.stdout + out.stderr, flush=True)
    if out.returncode != 0:
        fail("service install exited with an error")
    return out.stdout


def service(binary):
    step("service install")
    output = install(binary)
    token = re.search(r"bearer token\s+([0-9a-f]{64})", output)
    check(token, "a bearer token was generated", output)
    token = token.group(1)
    check(wait_until(healthy, 60), "the service answers /healthz")

    initialize = {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                  "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "0"}}}
    status, _ = http("/mcp", initialize)
    check(status == 401, f"MCP without the token is refused ({status})")
    status, body = http("/mcp", initialize, token=token)
    check(status == 200 and "didcomm-mcp" in body, f"MCP with the token works ({status})", body)

    step("service install again (an upgrade)")
    output = install(binary)
    check("in the configuration" in output, "the existing configuration is kept", output)
    check(wait_until(healthy, 60), "the service is back")
    status, _ = http("/mcp", initialize, token=token)
    check(status == 200, "the same token still works")

    step("service uninstall")
    out = subprocess.run([binary, "service", "uninstall"], capture_output=True, text=True)
    print(out.stdout + out.stderr, flush=True)
    check(out.returncode == 0, "uninstalled")
    check(wait_until(lambda: not healthy(), 30), "the service has stopped")
    config = re.search(r"^ *configuration +(.+?)(?: \(new\))?$", output, re.M).group(1)
    check(os.path.exists(config), "the configuration is kept")


def show_service_logs():
    system = platform.system()
    commands = {
        "Linux": ["journalctl", "-u", "didcomm-mcp", "--no-pager", "-n", "100"],
        "Darwin": ["tail", "-n", "100", "/Library/Logs/didcomm-mcp.log"],
        "Windows": ["powershell", "-Command",
                    r"Get-Content -Tail 100 $env:ProgramData\didcomm-mcp\didcomm-mcp.log"],
    }
    if system in commands:
        print("--- service logs", flush=True)
        subprocess.run(commands[system])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("--version")
    parser.add_argument("--service", action="store_true")
    args = parser.parse_args()
    binary = os.path.abspath(args.binary)

    step("--version")
    out = subprocess.run([binary, "--version"], capture_output=True, text=True, check=True).stdout.strip()
    expected = f"didcomm-mcp {args.version}" if args.version else "didcomm-mcp "
    check(out.startswith(expected), out)

    step("stdio")
    stdio(binary)
    if args.service:
        service(binary)
    print("\nPASS", flush=True)


if __name__ == "__main__":
    main()
