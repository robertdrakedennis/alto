#!/usr/bin/env bash
# The merge gate for the Rust client and native tools. See tools/refactor/README.md ("Gate").
#
#   tools/refactor/gate.sh [--base REV] [--full] [--verify-record]
#                          [--fn-hash <before.tsv | git-rev>] [--fn-hash-allow FILE]
#                          [--fn-hash-renames] [--fail-fast] [--only step,step]
#
# Rust steps: fmt, clippy, dag, provenance, magic-ids, tests, replay, inventory, fn-hash.
# Always: generated protocol tables.
#
# --base REV (default: the --fn-hash revision): run only what the diff against
#   REV needs: no Rust steps when nothing under tools/ or revisions/ changed;
#   the tests of the
#   changed crates plus their dependents (every crate when a shared foundation
#   changed), and the render tier's replay tests only when the change touches
#   render, presentation, profiler or app-shell code. Without a base every
#   step and every crate runs.
# --full: every step and every crate (nextest profile `full`), plus a real
#   no-pack build that cross-checks the static no-pack scan.
# --verify-record: pass at once when a successful run recorded the identical
#   tree with the same options; otherwise run.
#
# All steps run (unless --fail-fast); the exit status is non-zero when any
# failed. One gate runs at a time per machine (others wait for the lock).
# Honours CARGO_TARGET_DIR; runs cargo offline unless GATE_ONLINE=1.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT" || exit 2
[ "${GATE_ONLINE:-0}" = 1 ] || export CARGO_NET_OFFLINE=true
ARGS=("$@")

FNHASH=""
FNHASH_ALLOW=""
FNHASH_RENAMES=0
FAIL_FAST=0
ONLY=""
BASE=""
FULL=0
VERIFY=0
while [ $# -gt 0 ]; do
  case "$1" in
    --base) BASE="$2"; shift 2 ;;
    --full) FULL=1; shift ;;
    --verify-record) VERIFY=1; shift ;;
    --fn-hash) FNHASH="$2"; shift 2 ;;
    --fn-hash-allow)
      FNHASH_ALLOW="$2"
      [ -f "$FNHASH_ALLOW" ] || { echo "--fn-hash-allow: no such file: $FNHASH_ALLOW" >&2; exit 2; }
      shift 2 ;;
    --fn-hash-renames) FNHASH_RENAMES=1; shift ;;
    --skip-nopack) shift ;;  # accepted for old scripts: the gate no longer runs a no-pack build
    --fail-fast) FAIL_FAST=1; shift ;;
    --only) ONLY=",$2,"; shift 2 ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [ -z "$BASE" ] && [ -n "$FNHASH" ] && [ ! -f "$FNHASH" ]; then BASE="$FNHASH"; fi
[ "$FULL" -eq 1 ] && BASE=""

KIT=tools/refactor/gatekit.py
sha_of() { if [ -f "$1" ]; then shasum -a 256 "$1" | cut -c1-16; else printf '%s' "$1"; fi; }
# What a run checks, for the gate record: a record only stands for a run
# with the same options.
OPTKEY="full=$FULL only=${ONLY//,/ } base=$BASE fn-hash=$(sha_of "$FNHASH") allow=$(sha_of "$FNHASH_ALLOW") renames=$FNHASH_RENAMES"
key_now() { python3 "$KIT" key "$OPTKEY" ${BASE:+--base "$BASE"} ${FNHASH:+--fn-hash "$FNHASH"}; }
key_field() { python3 -c 'import json,sys; print(json.loads(sys.stdin.read())[sys.argv[1]])' "$1"; }

if [ "$VERIFY" -eq 1 ] && [ -z "${GATE_LOCK_HELD:-}" ]; then
  KEYJSON=$(key_now) || exit 2
  if python3 "$KIT" record find "$(printf '%s' "$KEYJSON" | key_field key)"; then
    echo "gate: ok (this tree passed with these options; not re-run)"
    exit 0
  fi
  echo "gate: no passing record for tree $(printf '%s' "$KEYJSON" | key_field tree) with these options; running the gate"
fi

# One gate at a time on this machine: re-run this script under the lock.
if [ -z "${GATE_LOCK_HELD:-}" ]; then
  GATE_LOCK_HELD=1 exec python3 "$KIT" lock -- "$ROOT/tools/refactor/gate.sh" ${ARGS[@]+"${ARGS[@]}"}
fi

KEYJSON_START=$(key_now) || exit 2
PLAN=$(python3 "$KIT" plan ${BASE:+--base "$BASE"} $([ "$FULL" -eq 1 ] && echo --full)) || exit 2
eval "$PLAN"
echo "gate: $GATE_PLAN"
echo "gate: rust=$GATE_RUST crates=$([ "$GATE_RUST_ALL" -eq 1 ] && echo all || echo "$GATE_PACKAGES")"

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/tools/target}"
case "$TARGET_DIR" in /*) ;; *) TARGET_DIR="$ROOT/$TARGET_DIR" ;; esac
PROFILE=gate
[ "$FULL" -eq 1 ] && PROFILE=full
OUT="$TARGET_DIR/gate"
mkdir -p "$OUT"
LISTING="$OUT/nextest-list.json"
# nextest keeps its store under the workspace root (tools/target/nextest),
# whatever CARGO_TARGET_DIR says.
JUNIT="$ROOT/tools/target/nextest/$PROFILE/junit.xml"
if [ "$GATE_RUST_ALL" -eq 1 ]; then PKG_ARGS=(--workspace); else
  PKG_ARGS=(); for p in $GATE_PACKAGES; do PKG_ARGS+=(-p "$p"); done
fi
NATIVE=tools/native910/Cargo.toml
RESULTS=()
STEPS_JSON=()
FAILED=0

now() { python3 -c 'import time; print(time.time())'; }

step() { # name, command...
  local name="$1"; shift
  if [ -n "$ONLY" ] && [[ "$ONLY" != *",$name,"* ]]; then return 0; fi
  if [ "$FAILED" -ne 0 ] && [ "$FAIL_FAST" -eq 1 ]; then
    RESULTS+=("$(printf '%-16s %-7s %8s' "$name" skipped -)"); return 0
  fi
  echo
  echo "=== gate: $name"
  local t0 t1 rc
  t0=$(now)
  "$@"
  rc=$?
  t1=$(now)
  local secs status=ok
  secs=$(python3 -c "print(f'{$t1 - $t0:.1f}s')")
  [ $rc -eq 0 ] || { status=FAILED; FAILED=1; }
  RESULTS+=("$(printf '%-16s %-7s %8s' "$name" "$status" "$secs")")
  STEPS_JSON+=("{\"name\": \"$name\", \"status\": \"$status\", \"secs\": \"$secs\"}")
}

skip() { # name, reason
  if [ -n "$ONLY" ] && [[ "$ONLY" != *",$1,"* ]]; then return 0; fi
  RESULTS+=("$(printf '%-16s %-7s %8s  %s' "$1" skipped - "($2)")")
  STEPS_JSON+=("{\"name\": \"$1\", \"status\": \"skipped\", \"secs\": \"-\"}")
}

gate_fmt() {
  local rc=0
  cargo fmt --manifest-path "$NATIVE" --check || rc=1
  cargo fmt --manifest-path tools/cs2/Cargo.toml --package cs2 --check || rc=1
  python3 tools/refactor/fmt-ratchet.py || rc=1
  cargo fmt --manifest-path tools/refactor/rsscan/Cargo.toml --check || rc=1
  return $rc
}

# Committed files generated from the revision registries (revisions/<rev>/):
# regenerate in memory and fail on any difference.
gate_generated() {
  python3 tools/revision/gen_protocol.py --check
}

gate_clippy() {
  local rc=0
  python3 tools/refactor/clippy-ratchet.py || rc=1
  cargo clippy --manifest-path "$NATIVE" --all-targets -q -- -D warnings || rc=1
  cargo clippy --manifest-path tools/cs2/Cargo.toml --all-targets -q -- -D warnings || rc=1
  return $rc
}

# One nextest run over the selected crates (default features, the test
# profile): every test binary is built once and the tests run in parallel
# across binaries. The replay and inventory steps read its results and its
# listing instead of building or running anything again.
gate_tests() {
  local rc=0
  rm -f "$LISTING" "$JUNIT"
  # The render tier (gatekit.py RENDER_TIER_TESTS) runs only when the plan
  # says the change needs it; the listing below is unfiltered.
  local filter=()
  if [ -n "$GATE_TEST_FILTER" ]; then
    echo "tests: render tier left out (${GATE_TEST_FILTER})"
    filter=(-E "$GATE_TEST_FILTER")
  fi
  cargo nextest run --manifest-path tools/Cargo.toml --profile "$PROFILE" "${PKG_ARGS[@]}" \
    --cargo-quiet --no-fail-fast --no-tests=warn --hide-progress-bar \
    --status-level fail --final-status-level slow ${filter[@]+"${filter[@]}"} || rc=1
  cargo nextest list --manifest-path tools/Cargo.toml "${PKG_ARGS[@]}" --cargo-quiet \
    --message-format json > "$LISTING" || { rm -f "$LISTING"; rc=1; }
  if [ "$GATE_RSSCAN" -eq 1 ]; then
    echo "rsscan (the gate's own scanner):"
    cargo test --manifest-path tools/refactor/rsscan/Cargo.toml -q 2>&1 | grep -E '^test result|FAILED|panicked'
    [ "${PIPESTATUS[0]}" -eq 0 ] || rc=1
  fi
  return $rc
}

# Every test in replay-gate.txt must have run and passed in the tests step
# (a missing, renamed, ignored or failing entry fails). With --only replay
# it runs just those tests first.
gate_replay() {
  if [ ! -f "$JUNIT" ]; then
    local filter
    filter=$(grep -v '^[[:space:]]*#' tools/refactor/replay-gate.txt | grep -v '^[[:space:]]*$' \
      | sed 's/^/test(=/; s/$/)/' | paste -sd'|' -)
    [ -n "$GATE_TEST_FILTER" ] && filter="($filter) and ($GATE_TEST_FILTER)"
    cargo nextest run --manifest-path tools/Cargo.toml --profile "$PROFILE" -p client910 --lib \
      --cargo-quiet --no-fail-fast --hide-progress-bar --status-level fail --final-status-level fail \
      -E "$filter"
  fi
  if [ "$GATE_RENDER_TIER" -eq 1 ]; then
    python3 "$KIT" replay-check "$JUNIT"
  else
    python3 "$KIT" replay-check --without-render-tier "$JUNIT"
  fi
}

gate_inventory() {
  local args=(--quiet --native-profile dev --nopack static)
  [ "$FULL" -eq 1 ] && args=(--quiet --native-profile dev --nopack check)
  [ "$GATE_RUST_ALL" -eq 1 ] || args+=(--packages "$(echo "$GATE_PACKAGES" | tr ' ' ',')")
  args+=(--no-server)
  if [ ! -f "$LISTING" ]; then
    cargo nextest list --manifest-path tools/Cargo.toml "${PKG_ARGS[@]}" --cargo-quiet \
      --message-format json > "$LISTING" || return 1
  fi
  python3 tools/refactor/test-inventory.py --nextest-list "$LISTING" "${args[@]}"
}

gate_fnhash() {
  # --fn-hash-allow FILE: one `fn-hash.py --allow` glob per line (# comments),
  # the differences a change makes on purpose.
  local allow=() g
  if [ -n "$FNHASH_ALLOW" ]; then
    while IFS= read -r g; do
      g="${g%%#*}"; g="$(printf '%s' "$g" | sed 's/[[:space:]]*$//')"
      [ -n "$g" ] && allow+=(--allow "$g")
    done < "$FNHASH_ALLOW"
  fi
  [ "$FNHASH_RENAMES" -eq 1 ] && allow+=(--renames)
  if [ -f "$FNHASH" ]; then
    local after
    after=$(mktemp -t fnhash-after.XXXXXX)
    python3 tools/refactor/fn-hash.py snapshot "$after" && python3 tools/refactor/fn-hash.py compare "$FNHASH" "$after" ${allow[@]+"${allow[@]}"}
  else
    python3 tools/refactor/fn-hash.py check "$FNHASH" ${allow[@]+"${allow[@]}"}
  fi
}

step generated gate_generated
if [ "$GATE_RUST" -eq 1 ]; then
  step fmt gate_fmt
  step clippy gate_clippy
  step dag python3 tools/refactor/dag-check.py
  step provenance python3 tools/refactor/provenance.py check
  step magic-ids python3 tools/refactor/magic-id-ratchet.py
  step tests gate_tests
  step replay gate_replay
  step inventory gate_inventory
  [ -n "$FNHASH" ] && step fn-hash gate_fnhash
else
  for s in fmt clippy dag provenance magic-ids tests replay; do skip "$s" "no Rust change"; done
fi

echo
echo "=== gate summary: $GATE_PLAN"
for r in ${RESULTS[@]+"${RESULTS[@]}"}; do echo "  $r"; done
if [ "$FAILED" -ne 0 ]; then echo "gate: FAILED"; exit 1; fi
# Record the pass, keyed by the tree and the options (not for --only runs,
# and not when the tree changed while the gate ran).
if [ -z "$ONLY" ]; then
  KEYJSON_END=$(key_now)
  KEY=$(printf '%s' "$KEYJSON_START" | key_field key)
  if [ "$KEY" = "$(printf '%s' "$KEYJSON_END" | key_field key)" ]; then
    RESULT=$(mktemp -t gate-result.XXXXXX)
    python3 - "$RESULT" "$KEYJSON_START" "$(IFS=,; echo "${STEPS_JSON[*]-}")" "$GATE_PLAN" <<'PY'
import json, sys
out, key, steps, plan = sys.argv[1:]
rec = json.loads(key)
rec.update(exit=0, plan=plan, steps=json.loads(f"[{steps}]"))
open(out, "w").write(json.dumps(rec))
PY
    python3 "$KIT" record write "$KEY" "$RESULT"
    rm -f "$RESULT"
  else
    echo "gate: the tree changed while the gate ran; no record written"
  fi
fi
echo "gate: ok"
