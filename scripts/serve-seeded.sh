#!/bin/sh
# Usage: scripts/serve-seeded.sh DIRECTORY [TEST]
set -eu

directory=${1:?Usage: scripts/serve-seeded.sh DIRECTORY [TEST]}
test=${2:-requests::app::perf::serve_the_seeded_pages_to_a_browser}
mkdir -p "$directory"
rm -f "$directory/address" "$directory/cookie" "$directory/stop"

KEYSEND_BROWSER_CHECK_DIR="$directory" \
  cargo test --locked --test mod "$test" -- \
  --ignored --exact --nocapture > "$directory/server.log" 2>&1 &

for _ in $(seq 120); do
  if [ -s "$directory/address" ] && [ -s "$directory/cookie" ]; then
    address=$(cat "$directory/address")
    status=$(curl --silent --location --output /dev/null --write-out '%{http_code}' \
      --cookie "ks_session=$(cat "$directory/cookie")" "http://$address/ad-accounts" || true)
    if [ "$status" = 200 ]; then
      echo "Serving the seeded pages on http://$address"
      exit 0
    fi
  fi
  sleep 1
done

cat "$directory/server.log"
echo "The seeded server did not answer /ad-accounts with 200." >&2
exit 1
