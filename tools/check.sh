#!/usr/bin/env bash
# Pre-merge check for the Rust client and native tools: generated protocol
# tables, formatting, clippy and the provenance count. No tests: run the
# tests of the code you change, and prove a feature on the real client.
#
#   tools/check.sh
#
# Honours CARGO_TARGET_DIR; runs cargo offline unless CHECK_ONLINE=1.
set -u
cd "$(dirname "$0")/.." || exit 2
[ "${CHECK_ONLINE:-0}" = 1 ] || export CARGO_NET_OFFLINE=true

FAILED=0
run() { # name, command...
  local name="$1"; shift
  echo "=== $name"
  "$@" || { echo "check: $name FAILED"; FAILED=1; }
}

run generated python3 tools/revision/gen_protocol.py --check
run fmt cargo fmt --manifest-path tools/Cargo.toml --all --check
run clippy cargo clippy --manifest-path tools/Cargo.toml --workspace --all-targets -q -- -D warnings
run provenance python3 tools/provenance/provenance.py check

[ "$FAILED" -eq 0 ] || { echo "check: FAILED"; exit 1; }
echo "check: ok"
