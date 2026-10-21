#!/bin/sh
# Usage: scripts/ai-eval/agent-run.sh DIRECTORY [TRIALS] [SCENARIO,...]
set -eu
directory=${1:?Usage: scripts/ai-eval/agent-run.sh DIRECTORY [TRIALS] [SCENARIO,...]}
here=$(dirname "$0")
mkdir -p "$directory"
rm -f "$directory/address"
python3 "$here/proxy.py" "$directory" &
proxy=$!
trap 'kill $proxy' EXIT
for _ in $(seq 50); do [ -s "$directory/address" ] && break; sleep 0.2; done
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
KEYSEND_EVAL_AI_URL=$(cat "$directory/address") \
KEYSEND_EVAL_DIR=$(cd "$directory" && pwd) \
KEYSEND_EVAL_TRIALS=${2:-2} KEYSEND_EVAL_ONLY=${3:-} \
 cargo test --locked --lib agent::eval::the_agent_answers_the_scenarios -- --ignored --exact --nocapture
python3 "$here/agent-judge.py" "$directory"
python3 "$here/agent-summary.py" "$directory"
