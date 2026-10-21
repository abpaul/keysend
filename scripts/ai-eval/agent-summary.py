#!/usr/bin/env python3
"""Summarize paired Agent trials; optional USD prices per million input/output tokens."""
import argparse
from collections import defaultdict
import csv
import json
from pathlib import Path
import statistics

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("directory", type=Path)
parser.add_argument("--input-usd", type=float)
parser.add_argument("--output-usd", type=float)
args = parser.parse_args()
calls = defaultdict(list)
if (args.directory / "calls.jsonl").exists():
    for line in (args.directory / "calls.jsonl").read_text().splitlines():
        call = json.loads(line)
        calls[call.get("run_id")].append(call)
points = {"met": 1.0, "partly": 0.5, "missed": 0.0}
rows = []
essentials = []
for path in sorted(args.directory.glob("*.*.*.json")):
    if path.name.endswith(".judged.json"):
        continue
    result = json.loads(path.read_text())
    judge_path = path.with_suffix(".judged.json")
    judge = json.loads(judge_path.read_text()) if judge_path.exists() else None
    rows.append((result, judge))
    for i, essential in enumerate(result["essentials"]):
        verdict = judge["essentials"][i] if judge else {"verdict": "unjudged", "why": ""}
        essentials.append([result["scenario"], result["held_out"], result["trial"], result["arm"], i+1, essential, verdict["verdict"], verdict["why"]])
with (args.directory / "essentials.tsv").open("w") as out:
    writer = csv.writer(out, delimiter="\t")
    writer.writerow(["scenario", "held_out", "trial", "arm", "essential", "conclusion", "verdict", "why"])
    writer.writerows(essentials)


def removed_texts(result):
    if result["arm"] == "unchecked" or not result["draft"]:
        return 0
    report = result["report"]
    kept = int(report.get("summary") is not None)
    kept += sum(len(report[key]) for key in ("observations", "hypotheses", "recommendations"))
    kept += sum(item.get("kind") == "stated" for item in report["limitations"])
    return result["drafted_texts"] - kept


def summarize(items):
    runs = [r for r, _ in items]
    judged = [j for _, j in items if j]
    average = lambda values: round(statistics.mean(values), 3) if values else None
    total_texts = sum(r["drafted_texts"] for r in runs)
    return {
        "runs": len(runs), "judged": len(judged), "failures": sum(r["failure"] is not None for r in runs),
        "scores": {key: average([j["scores"][key] for j in judged]) for key in ("factual_correctness", "recommendation_quality", "evidence_quality", "uncertainty_handling", "user_experience")},
        "essentials": {key: sum(e["verdict"] == key for j in judged for e in j["essentials"]) for key in points},
        "essentials_credit": average([points[e["verdict"]] for j in judged for e in j["essentials"]]),
        "judge_flag_entries": sum(len(j["unsupported_claims"]) for j in judged),
        "input_tokens": average([r["usage"]["input_tokens"] for r in runs]),
        "keysend_input_characters": average([sum(c["payload_characters"] for c in calls[f"{r['scenario']}.{r['trial']}.{'agent' if r['arm']=='unchecked' else r['arm']}"]) for r in runs]),
        "output_tokens": average([r["usage"]["output_tokens"] for r in runs]),
        "seconds": average([r["seconds"] for r in runs]),
        "seconds_sd": round(statistics.pstdev(r["seconds"] for r in runs), 3),
        "seconds_range": [round(min(r["seconds"] for r in runs),3), round(max(r["seconds"] for r in runs),3)],
        "model_calls": average([r["usage"]["model_calls"] for r in runs]),
        "estimated_usd": round(statistics.mean([(r["usage"]["input_tokens"]*args.input_usd+r["usage"]["output_tokens"]*args.output_usd)/1e6 for r in runs]), 6) if args.input_usd is not None and args.output_usd is not None else None,
        "expected_recall": average([r["metrics"]["expected_recall"] for r in runs]),
        "unneeded_operations": average([len(r["metrics"]["unneeded"]) for r in runs]),
        "repeated_operations": sum(r["metrics"]["repeated"] for r in runs),
        "operations": average([len(r["steps"]) for r in runs]),
        "operation_refusals": sum(s["outcome"] == "refused" for r in runs for s in r["steps"]),
        "rounds_with_operations": average([r["metrics"]["rounds_with_operations"] for r in runs]),
        "refused": sum(r["refused"] for r in runs),
        "reported_refused_share": round(sum(r["refused"] for r in runs)/total_texts, 3) if total_texts else 0,
        "removed_texts": sum(removed_texts(r) for r in runs),
        "refused_share": round(sum(removed_texts(r) for r in runs)/total_texts, 3) if total_texts else 0,
        "empty_answers": sum(r["failure"] is None and not r["clarification"] and not any((r["report"] or {}).get(key) for key in ("summary", "observations", "hypotheses", "recommendations")) for r in runs),
        "unsupported_numbers": sum(len(r["metrics"]["unsupported_numbers"]) for r in runs),
        "weak_actions": sum(r["metrics"]["weak_actions"] for r in runs),
        "K_clarified": sum(r["clarification"] for r in runs if r["scenario"] == "K_ambiguous"),
        "J_pause_all": sum(r["metrics"]["pause_all_campaigns"] for r in runs if r["scenario"] == "J_injection"),
    }

by_arm = defaultdict(list)
by_case = defaultdict(list)
by_partition = defaultdict(list)
by_trial = defaultdict(list)
for r, j in rows:
    by_arm[r["arm"]].append((r, j))
    by_trial[f"{r['trial']}.{r['arm']}"].append((r,j))
    by_case[r["scenario"] + "." + r["arm"]].append((r, j))
    by_partition[("held_out" if r["held_out"] else "development") + "." + r["arm"]].append((r,j))
result = {"prices_per_million": {"input": args.input_usd, "output": args.output_usd},
          "arms": {key: summarize(items) for key, items in by_arm.items()},
          "trials": {key: summarize(items) for key, items in by_trial.items()},
          "partitions": {key: summarize(items) for key, items in by_partition.items()},
          "scenarios": {key: summarize(items) for key, items in by_case.items()}}
(args.directory / "summary.json").write_text(json.dumps(result, indent=2))
print(json.dumps(result["arms"], indent=2))
print("Per-essential verdicts and explanations:", args.directory / "essentials.tsv")
for key, value in result["scenarios"].items():
    print(key, value["essentials"], "failures", value["failures"], "unsupported", value["unsupported_numbers"])

print("scenario\ttrial\tarm\tessential\tverdict\twhy")
for row in essentials:
    print("\t".join(str(row[i]) for i in (0,2,3,4,6,7)))
