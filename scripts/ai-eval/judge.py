#!/usr/bin/env python3
"""Score each report of an evaluation run against the conclusions of its scenario.

The judge is a different model family from the one that writes the reports. It sees the
scenario's ground truth, the report as a client reads it, and the rubric; it never sees
which version of KeySend wrote the report.

Usage: scripts/ai-eval/judge.py RUN_DIRECTORY [MODEL]
"""

import concurrent.futures
import json
import subprocess
import sys
from pathlib import Path

DIRECTORY = Path(sys.argv[1])
MODEL = sys.argv[2] if len(sys.argv) > 2 else "opus"

RUBRIC = """You are a senior Google Ads consultant reviewing a report that software wrote for a client.

The Metrics and breakdown tables are computed by code and are correct. Judge the written
analysis: the assessment, the Update, the Recommendations and the Findings.

GROUND TRUTH is what the account data shows. ESSENTIALS are the conclusions a competent
consultant must reach. MISTAKES are errors a weak report makes. Judge whether the report
reaches each essential in substance; the wording does not matter. A conclusion counts as
"met" only when the report states it clearly enough for the client to act on it, "partly"
when it is hinted or incomplete, and "missed" otherwise.

Score each dimension from 1 to 5:
- accuracy: every statement agrees with the data (5 = no error, 1 = several false statements).
- relevance: recommendations address the real problems of this account (5 = all, 1 = none).
- evidence: each conclusion cites the specific numbers, campaigns or terms behind it.
- usefulness: the client learns what to do next and why it matters commercially.
- uncertainty: confident where the data is strong, explicit where it is weak, no speculation stated as fact.
- specificity: no generic advice that could apply to any advertiser, no repetition.
- client_ready: a consultant could use the written analysis in a client meeting with minimal editing.

List every unsupported claim: a number, cause, benchmark, market fact or promise that the
data does not support. List every generic item: advice that would fit any advertiser.

Answer only with JSON of this shape:
{"essentials":[{"verdict":"met|partly|missed","why":"..."}],
 "mistakes":[{"made":true,"why":"..."}],
 "unsupported_claims":["..."],
 "generic_items":["..."],
 "scores":{"accuracy":0,"relevance":0,"evidence":0,"usefulness":0,"uncertainty":0,"specificity":0,"client_ready":0},
 "summary":"one sentence"}
"""


def judge(path):
    result = json.loads(path.read_text())
    profile = {key: result.get(key) for key in ("business", "website", "goal", "target_cost", "note", "margin", "customer_value", "close_rate", "target_roas", "history")}
    prompt = (
        f"{RUBRIC}\nACCOUNT PROFILE:\n{json.dumps(profile)}\n\nGROUND TRUTH:\n{result['situation']}\n\n"
        + "ESSENTIALS:\n" + "\n".join(f"{i + 1}. {e}" for i, e in enumerate(result["essentials"]))
        + "\n\nMISTAKES:\n" + "\n".join(f"{i + 1}. {m}" for i, m in enumerate(result["mistakes"]))
        + f"\n\nREPORT:\n{result['report']}\n"
    )
    for _ in range(3):
        run = subprocess.run(
            ["claude", "-p", "--model", MODEL, "--output-format", "json", "--max-turns", "1"],
            input=prompt, capture_output=True, text=True, timeout=600,
        )
        try:
            text = json.loads(run.stdout)["result"]
            verdict = json.loads(text[text.index("{"): text.rindex("}") + 1])
            break
        except (json.JSONDecodeError, KeyError, ValueError):
            continue
    else:
        raise RuntimeError(f"judge failed for {path.name}: {run.stderr[-500:]}")
    path.with_suffix(".judged.json").write_text(json.dumps({"judge": MODEL, **verdict}, indent=2))
    return path.name


paths = [path for path in sorted(DIRECTORY.glob("*.json"))
         if not path.name.endswith(".judged.json") and path.name[0] != "_"
         and not path.with_suffix(".judged.json").exists()]
with concurrent.futures.ThreadPoolExecutor(6) as pool:
    for name in pool.map(judge, paths):
        print("judged", name)
