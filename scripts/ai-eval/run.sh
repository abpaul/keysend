#!/bin/sh
# Usage: scripts/ai-eval/run.sh DIRECTORY [TRIALS] [SCENARIO,...]
# Writes one report per scenario and trial into DIRECTORY, then judges each one.
set -eu

directory=${1:?Usage: scripts/ai-eval/run.sh DIRECTORY [TRIALS] [SCENARIO,...]}
here=$(dirname "$0")
mkdir -p "$directory"
rm -f "$directory/address"
python3 "$here/proxy.py" "$directory" &
proxy=$!
trap 'kill $proxy' EXIT
for _ in $(seq 50); do [ -s "$directory/address" ] && break; sleep 0.2; done

KEYSEND_EVAL_AI_URL=$(cat "$directory/address") \
KEYSEND_EVAL_DIR=$(cd "$directory" && pwd) \
KEYSEND_EVAL_TRIALS=${2:-1} \
KEYSEND_EVAL_ONLY=${3:-} \
  cargo test --locked --lib monitoring::eval::writes_the_reports_of_the_scenarios \
  -- --ignored --exact --nocapture

python3 "$here/judge.py" "$directory"
python3 "$here/summarize.py" "$directory"
