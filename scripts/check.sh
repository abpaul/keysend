#!/usr/bin/env bash
# Usage: scripts/check.sh
#
# The checks of CI, of the pre-push hook and of a release from this machine.
# The checks get no inherited credential or DATABASE_URL: only the variables
# below stay. The test database and every temporary file go to a new private
# directory. The database and temporary files remain as verification evidence.
set -euo pipefail
cd "$(dirname "$0")/.."

work=$(mktemp -d "${TMPDIR:-/tmp}/keysend-check.XXXXXX")
finish() {
  status=$?
  printf 'check: exit %s; the test database and temporary files are in %s\n' "$status" "$work" >&2
  exit "$status"
}
trap finish EXIT

for name in $(compgen -e); do
  case $name in
    PATH | HOME | USER | LOGNAME | SHELL | LANG | LC_* | TERM | CI | GITHUB_ACTIONS) ;;
    ASDF_* | RUSTUP_* | RUSTC_WRAPPER | CARGO_HOME | CARGO_TARGET_DIR | CARGO_INCREMENTAL | CARGO_TERM_* | CARGO_NET_* | CARGO_BUILD_JOBS) ;;
    DEVELOPER_DIR | SDKROOT | SSL_CERT_FILE | SSL_CERT_DIR | HTTP_PROXY | HTTPS_PROXY | NO_PROXY | http_proxy | https_proxy | no_proxy) ;;
    *) unset "$name" 2>/dev/null || true ;;
  esac
done
mkdir "$work/tmp"
export TMPDIR="$work/tmp"
export DATABASE_URL="sqlite://$work/keysend_test.sqlite?mode=rwc"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0

set -x
cargo fmt --all -- --check
python3 scripts/test-vendor-credseal.py
python3 scripts/vendor-credseal.py --verify
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
