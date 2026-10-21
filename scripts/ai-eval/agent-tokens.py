#!/usr/bin/env python3
"""Count the tokens of KeySend's own investigation requests in a saved evaluation.

The evaluation proxy answers through `codex exec`, whose usage includes the Codex agent
prompt. This script counts only what KeySend sends to the Responses API: the instructions,
the input and the JSON schema of each `agent_investigation` request in `payloads.jsonl`,
with the o200k_base tokenizer. It needs the `tiktoken` package; it reads and prints only.

Usage: scripts/ai-eval/agent-tokens.py DIRECTORY
"""

import json
import statistics
import sys
from pathlib import Path

try:
    import tiktoken
except ImportError:
    sys.exit("agent-tokens: install the tiktoken package to count tokens")

encoding = tiktoken.get_encoding("o200k_base")


def tokens(text):
    return len(encoding.encode(text))


rows = [json.loads(line) for line in (Path(sys.argv[1]) / "payloads.jsonl").open()]
calls = [
    row for row in rows
    if row.get("run_id", "").endswith(".agent")
    and row["payload"]["text"]["format"]["name"] == "agent_investigation"
]
parts = {"instructions": [], "schema": [], "input": [], "evidence": [], "request": []}
investigations = {}
for row in calls:
    payload = row["payload"]
    instructions = tokens(payload["instructions"])
    schema = tokens(json.dumps(payload["text"]["format"]["schema"], separators=(",", ":")))
    given = tokens(payload["input"])
    evidence = tokens(json.dumps(json.loads(payload["input"]).get("evidence", []), separators=(",", ":")))
    for name, value in (("instructions", instructions), ("schema", schema), ("input", given), ("evidence", evidence), ("request", instructions + schema + given)):
        parts[name].append(value)
    investigations[row["run_id"]] = investigations.get(row["run_id"], 0) + instructions + schema + given
print(f"{len(calls)} requests, {len(investigations)} investigations")
for name, values in parts.items():
    print(f"{name:13s} mean {statistics.mean(values):7.0f}  median {statistics.median(values):7.0f}  max {max(values):6d}")
values = list(investigations.values())
print(f"per investigation  mean {statistics.mean(values):7.0f}  median {statistics.median(values):7.0f}  max {max(values):6d}")
