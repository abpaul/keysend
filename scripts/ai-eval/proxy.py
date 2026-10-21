#!/usr/bin/env python3
"""Serve the OpenAI Responses API on localhost and answer each request with `codex exec`.

The evaluation runs KeySend's own request code against this server, so the production
instructions, schemas and checks are the ones that are measured. Codex adds its own agent
prompt, so latency and token counts are an upper bound; `calls.jsonl` records both the
Codex usage and the size of the KeySend payload.

Usage: scripts/ai-eval/proxy.py DIRECTORY [PORT]
"""

import copy
import json
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

DIRECTORY = Path(sys.argv[1])
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 0
LOCK = threading.Lock()

PREAMBLE = """You are answering one call to a JSON API. Do not run commands or read files.
Follow the INSTRUCTIONS exactly and answer only with JSON that matches the schema.
When TOOLS lists web_search, you may search the web; otherwise do not.
"""


def compatible_schema(value):
    """Preserve constraints as descriptions if the CLI rejects validation keywords."""
    if isinstance(value, list):
        return [compatible_schema(item) for item in value]
    if not isinstance(value, dict):
        return value
    result = {}
    constraints = {}
    for key, item in value.items():
        if key in {"pattern", "minimum", "maximum", "minItems", "maxItems", "minLength", "maxLength"}:
            constraints[key] = item
        else:
            result[key] = compatible_schema(item)
    if constraints:
        result["description"] = result.get("description", "") + " Required constraints: " + json.dumps(constraints)
    return result


def answer(payload, run_id=None):
    schema = copy.deepcopy(payload["text"]["format"]["schema"])
    schema_changes = []
    tools = [tool.get("type") for tool in payload.get("tools", [])]
    prompt = f"{PREAMBLE}\nTOOLS: {json.dumps(tools)}\n\nINSTRUCTIONS:\n{payload.get('instructions', '')}\n\nINPUT:\n{payload['input']}\n"
    with tempfile.TemporaryDirectory() as work:
        work = Path(work)
        (work / "schema.json").write_text(json.dumps(schema))
        command = [
            "codex", "exec", "--skip-git-repo-check", "--ephemeral", "--ignore-rules",
            "-s", "read-only", "-C", str(work), "-m", payload["model"],
            "--output-schema", str(work / "schema.json"),
            "-o", str(work / "answer.json"), "--json", "-",
        ]
        if "web_search" in tools:
            command[2:2] = ["-c", 'web_search="live"']
        started = time.monotonic()
        run = subprocess.run(command, input=prompt, capture_output=True, text=True, timeout=600)
        if run.returncode and any(word in (run.stdout + run.stderr).lower() for word in ("invalid schema", "unsupported schema")):
            schema = compatible_schema(schema)
            schema_changes.append("validation keywords moved into description; production schema unchanged")
            (work / "schema.json").write_text(json.dumps(schema))
            run = subprocess.run(command, input=prompt, capture_output=True, text=True, timeout=600)
        seconds = time.monotonic() - started
        usage = {}
        sources = []
        for line in run.stdout.splitlines():
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if event.get("type") == "turn.completed":
                usage = event.get("usage", {})
            item = event.get("item") or {}
            if item.get("type") == "web_search":
                sources.extend(result["url"] for result in item.get("results") or [] if result.get("url"))
        text = (work / "answer.json").read_text() if (work / "answer.json").exists() else ""
    record = {
        "schema": payload["text"]["format"].get("name"),
        "schema_changes": schema_changes,
        "run_id": run_id,
        "error": (run.stderr + run.stdout)[-2000:] if run.returncode else None,
        "seconds": round(seconds, 1),
        "payload_characters": len(payload.get("instructions", "")) + len(payload["input"]),
        "codex_usage": usage,
        "sources": len(sources),
        "exit": run.returncode,
        "answer_characters": len(text),
    }
    with LOCK, open(DIRECTORY / "calls.jsonl", "a") as log:
        log.write(json.dumps(record) + "\n")
        with open(DIRECTORY / "payloads.jsonl", "a") as raw:
            raw.write(json.dumps({"run_id": run_id, "payload": payload, "answer": text, "schema_changes": schema_changes}) + "\n")
    return text, usage, sources


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        try:
            text, usage, sources = answer(payload, self.headers.get("x-keysend-eval-run"))
        except subprocess.TimeoutExpired:
            self.send_response(504)
            self.end_headers()
            return
        searched = [{"type": "web_search_call", "action": {"type": "search", "sources": [{"type": "url", "url": url} for url in sources]}}] if sources else []
        body = json.dumps({
            "output": searched + [{"type": "message", "content": [{"type": "output_text", "text": text}]}],
            "usage": {"input_tokens": usage.get("input_tokens", 0), "output_tokens": usage.get("output_tokens", 0)},
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


DIRECTORY.mkdir(parents=True, exist_ok=True)
server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
(DIRECTORY / "address").write_text(f"http://127.0.0.1:{server.server_address[1]}/v1/responses")
server.serve_forever()
