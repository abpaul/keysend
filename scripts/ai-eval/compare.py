#!/usr/bin/env python3
"""Compare judged runs on the scenarios they share, with the length-normalized counts.

Usage: scripts/ai-eval/compare.py RUN_DIRECTORY... [--only scenario,scenario]
"""

import json
import statistics
import sys
from pathlib import Path

POINTS = {"met": 1.0, "partly": 0.5, "missed": 0.0}
args = [arg for arg in sys.argv[1:] if not arg.startswith("--only")]
only = next((arg.split("=", 1)[1].split(",") for arg in sys.argv[1:] if arg.startswith("--only=")), None)


def written(report):
    start = report.find("## Assessment")
    return len(report[start if start >= 0 else report.find("## Latest Update"):].split())


for directory in map(Path, args):
    rows = []
    for judged in sorted(directory.glob("*.judged.json")):
        result = json.loads(judged.with_name(judged.name.replace(".judged.json", ".json")).read_text())
        if only and result["scenario"] not in only:
            continue
        rows.append((result, json.loads(judged.read_text())))
    if not rows:
        continue
    essentials = [POINTS[e["verdict"]] for _, v in rows for e in v["essentials"]]
    words = sum(written(r["report"]) for r, _ in rows)
    ready = [v["scores"]["client_ready"] for _, v in rows]
    accuracy = [v["scores"]["accuracy"] for _, v in rows]
    print(json.dumps({
        "run": directory.name,
        "reports": len(rows),
        "essentials_met": round(sum(essentials) / len(essentials), 3),
        "client_ready": f"{statistics.mean(ready):.2f} ± {statistics.pstdev(ready):.2f}",
        "accuracy": f"{statistics.mean(accuracy):.2f}",
        "unsupported_per_1000_words": round(sum(len(v["unsupported_claims"]) for _, v in rows) * 1000 / words, 1),
        "generic_per_1000_words": round(sum(len(v["generic_items"]) for _, v in rows) * 1000 / words, 1),
        "mistakes_made": sum(m["made"] for _, v in rows for m in v["mistakes"]),
        "words_per_report": round(words / len(rows)),
    }))
