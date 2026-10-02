#!/usr/bin/env python3
"""End-to-end test of didcomm-mcp against a real stack (docker-compose.yml).

Starts the registry, mediator and Bob, then runs the MCP server container with
stdin/stdout attached -- exactly how an MCP host runs it -- and walks through the
architecture brief's workflow with raw MCP JSON-RPC:

  discover Bob's features -> search and look up a protocol in the registry ->
  a schema-rejected send -> a valid send (Bob's reply comes back) -> Bob messages us
  through the mediator -> fetch_messages picks it up -> read a spec section.

Usage: e2e/run.py [--no-build] [--keep]
  --no-build  use already-built images (didcomm-e2e/*)
  --keep      leave the stack running afterwards

Needs Docker with Compose, and sibling checkouts of didcomm and documentation-server
(see docker-compose.yml). Standard library only.
"""

import json
import os
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
COMPOSE = ["docker", "compose", "-f", str(HERE / "docker-compose.yml"), "-p", "didcomm-e2e"]
PORTS = {name: int(os.environ.get(f"{name.upper()}_PORT", port))
         for name, port in [("registry", 18180), ("mediator", 18181), ("bob", 18182)]}
BASICMESSAGE = "https://didcomm.org/basicmessage/2.0/message"


def step(name):
    print(f"--- {name}", flush=True)


def check(condition, what, detail=None):
    if not condition:
        print(f"FAIL: {what}" + (f"\n{json.dumps(detail, indent=2)[:3000]}" if detail is not None else ""))
        raise SystemExit(1)
    print(f"  ok: {what}", flush=True)


def http(service, path, body=None):
    url = f"http://127.0.0.1:{PORTS[service]}{path}"
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request(url, data=data, headers={"content-type": "application/json"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return response.read().decode()


def wait_for_did(service, timeout=180):
    deadline = time.time() + timeout
    while True:
        try:
            return http(service, "/did").strip()
        except OSError:
            if time.time() > deadline:
                subprocess.run(COMPOSE + ["logs", service])
                raise SystemExit(f"{service} didn't come up")
            time.sleep(1)


class Mcp:
    """The MCP server container, spoken to over its stdin/stdout."""

    def __init__(self, env):
        args = COMPOSE + ["--profile", "mcp", "run", "--rm", "--no-deps", "-T"]
        for key, value in env.items():
            args += ["-e", f"{key}={value}"]
        self.process = subprocess.Popen(args + ["mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        self.next_id = 0

    def rpc(self, method, params=None, notify=False):
        message = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            message["params"] = params
        if not notify:
            self.next_id += 1
            message["id"] = self.next_id
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()
        if notify:
            return None
        while True:
            line = self.process.stdout.readline()
            if not line:
                raise SystemExit("the MCP server exited")
            response = json.loads(line)
            if response.get("id") == self.next_id:
                if "error" in response:
                    raise SystemExit(f"{method} failed: {response['error']}")
                return response["result"]

    def tool(self, name, arguments):
        """(is_error, text blocks, parsed last block or None)."""
        result = self.rpc("tools/call", {"name": name, "arguments": arguments})
        texts = [c["text"] for c in result["content"] if c.get("type") == "text"]
        try:
            data = json.loads(texts[-1])
        except (ValueError, IndexError):
            data = None
        return result.get("isError", False), texts, data

    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=30)


def main():
    build = "--no-build" not in sys.argv
    keep = "--keep" in sys.argv
    if build:
        step("build images")
        subprocess.run(COMPOSE + ["--profile", "mcp", "build"], check=True)
    step("start registry, mediator, bob")
    subprocess.run(COMPOSE + ["up", "-d", "--no-build", "registry", "mediator", "bob"], check=True)
    mcp = None
    try:
        registry, mediator, bob = (wait_for_did(s) for s in ("registry", "mediator", "bob"))
        print(f"  registry {registry[:40]}...\n  mediator {mediator[:40]}...\n  bob      {bob[:40]}...")

        step("MCP handshake")
        mcp = Mcp({
            "DIDCOMM_MCP_REGISTRY_DID": registry,
            "DIDCOMM_MCP_MEDIATOR_DID": mediator,
            # The server logs to stderr, which run passes through; keep it to problems.
            "RUST_LOG": os.environ.get("RUST_LOG", "warn"),
        })
        init = mcp.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                      "clientInfo": {"name": "e2e", "version": "0"}})
        mcp.rpc("notifications/initialized", notify=True)
        check(init["serverInfo"]["name"] == "didcomm-mcp", "server identifies as didcomm-mcp")
        tools = sorted(t["name"] for t in mcp.rpc("tools/list")["tools"])
        check(len(tools) == 7, f"7 tools: {tools}")

        step("get_identity")
        _, _, identity = mcp.tool("get_identity", {})
        check(identity["can_receive"] and identity["mediation"]["mediator_did"] == mediator,
              "mediated with the stack's mediator", identity)
        me = identity["did"]

        step("1-2. discover_features on Bob")
        err, texts, reply = mcp.tool("discover_features", {"target_did": bob})
        check(not err and texts[0].startswith("UNTRUSTED CONTENT"), "answered, marked untrusted", texts)
        ids = [d["id"] for d in reply["message"]["body"]["disclosures"]]
        check("https://didcomm.org/trust-ping/2.0" in ids, f"Bob discloses {ids}")

        step("3-4. learn basicmessage from the registry")
        err, _, found = mcp.tool("search_protocols", {"text": "basic", "status": ["Production"]})
        piuris = [e["piuri"] for e in found["message"]["body"]["entries"]]
        check("https://didcomm.org/basicmessage/2.0" in piuris, f"search finds it: {piuris}")
        err, _, docs = mcp.tool("lookup_protocol_documentation",
                                {"protocol_uri": "https://didcomm.org/basicmessage/2.0", "sections": ["roles"]})
        body = docs["message"]["body"]
        check(body["roles"] == ["sender", "receiver"], "roles from the real didcomm.org definition", body.get("roles"))
        check(any(m["type"] == BASICMESSAGE and "schema" in m for m in body["messages"]), "message schema included")

        step("a message that breaks the schema is refused")
        err, texts, _ = mcp.tool("send_didcomm_message",
                                 {"target_did": bob, "type": BASICMESSAGE, "body": {"text": "wrong field"}})
        check(err and "content" in texts[0], "refused, naming the missing field", texts)

        step("5-6. send it properly")
        err, texts, sent = mcp.tool("send_didcomm_message",
                                    {"target_did": bob, "type": BASICMESSAGE, "body": {"content": "hello Bob"}})
        check(not err and sent["validation"] == "passed", "validated and sent", texts)
        check(sent["reply"]["message"]["body"]["content"] == "ack: hello Bob", "Bob's ack came back", sent)

        step("Bob messages us through the mediator")
        http("bob", "/send", {"to": me, "content": "hi from Bob"})
        err, texts, fetched = mcp.tool("fetch_messages", {})
        contents = [(m["from"], m["message"]["body"].get("content")) for m in fetched["messages"]]
        check((bob, "hi from Bob") in contents, "fetch_messages picked it up, from Bob", contents)
        _, _, again = mcp.tool("fetch_messages", {})
        check(again["messages"] == [], "and it was removed from the queue")

        step("the spec")
        err, _, spec = mcp.tool("lookup_spec", {"version": "2.1", "section": "message-headers"})
        check("`thid`" in spec["message"]["body"]["section"]["markdown"], "spec v2.1 message-headers section")

        print("\nPASS: the full workflow works end to end")
    finally:
        if mcp:
            mcp.close()
        if not keep:
            subprocess.run(COMPOSE + ["down", "-v"], capture_output=True)


if __name__ == "__main__":
    main()
