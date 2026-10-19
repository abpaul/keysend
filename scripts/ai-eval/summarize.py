#!/usr/bin/env python3
"""Summarize one or more judged evaluation runs side by side.

Usage: scripts/ai-eval/summarize.py RUN_DIRECTORY...
"""

import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

DIMENSIONS = ["accuracy", "relevance", "evidence", "usefulness", "uncertainty", "specificity", "client_ready"]
POINTS = {"met": 1.0, "partly": 0.5, "missed": 0.0}


def load(directory):
    reports = []
    for judged in sorted(directory.glob("*.judged.json")):
        verdict = json.loads(judged.read_text())
        result = json.loads(judged.with_name(judged.name.replace(".judged.json", ".json")).read_text())
        reports.append((result, verdict))
    calls = [json.loads(line) for line in (directory / "calls.jsonl").read_text().splitlines()] \
        if (directory / "calls.jsonl").exists() else []
    return reports, calls


def spread(values):
    return f"{statistics.mean(values):.2f} ± {statistics.pstdev(values):.2f}" if values else "-"


def summary(directory):
    reports, calls = load(directory)
    essentials = [POINTS[e["verdict"]] for _, v in reports for e in v["essentials"]]
    by_scenario = defaultdict(list)
    for result, verdict in reports:
        by_scenario[result["scenario"]].append(
            statistics.mean(POINTS[e["verdict"]] for e in verdict["essentials"]))
    words = [len(result["report"].split("## Latest Update", 1)[-1].split()) for result, _ in reports]
    by_schema = defaultdict(list)
    for call in calls:
        by_schema[call["schema"]].append(call)
    return {
        "reports": len(reports),
        "essentials_met": sum(essentials) / len(essentials) if essentials else 0,
        "mistakes_made": sum(m["made"] for _, v in reports for m in v["mistakes"]),
        "unsupported_claims": sum(len(v["unsupported_claims"]) for _, v in reports),
        "generic_items": sum(len(v["generic_items"]) for _, v in reports),
        "scores": {d: spread([v["scores"][d] for _, v in reports]) for d in DIMENSIONS},
        "by_scenario": {key: round(statistics.mean(values), 2) for key, values in sorted(by_scenario.items())},
        "written_words": spread(words),
        "update_by_ai": sum(r["ai"].get("update_written_by_ai", False) for r, _ in reports),
        "warnings": sum(len(r["warnings"]) for r, _ in reports),
        "calls": {
            schema: {
                "count": len(items),
                "seconds": spread([c["seconds"] for c in items]),
                "payload_tokens_estimate": spread([c["payload_characters"] / 4 for c in items]),
                "output_tokens": spread([c["codex_usage"].get("output_tokens", 0) for c in items]),
            }
            for schema, items in sorted(by_schema.items())
        },
    }


runs = {Path(arg).name: summary(Path(arg)) for arg in sys.argv[1:]}
print(json.dumps(runs, indent=2))
