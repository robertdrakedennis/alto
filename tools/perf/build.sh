#!/usr/bin/env bash
# Build an instrumented (perf_probe) release client from a tree, without
# touching the tree: tools/ is staged into $CARGO_TARGET_DIR/perf-src/NAME,
# instrumented there (instrument.py) and built into its own target
# directory $CARGO_TARGET_DIR/perf-target/NAME (cargo hashes workspace
# members by their workspace-relative path, so two copies sharing a target
# directory would reuse each other's artifacts).
#
#   CARGO_TARGET_DIR=... tools/perf/build.sh NAME SOURCE
#
# SOURCE is a worktree path (its current files, committed or not) or
# `git:REV` (that revision of the repository this script lives in). Prints
# the binary's path: $CARGO_TARGET_DIR/perf-bin/client910-NAME. Unchanged
# files keep their mtimes in the build copy, so a rebuild is incremental.
# PERF_TESTS=1 also builds the instrumented client910 lib test binary
# (perf-bin/client910-tests-NAME): the GPU capture replays
# (ui_equipment_replay, ui_settings_replay) paint recorded UI sessions through
# the real GPU painter deterministically, so their content traces compare
# the UI paths the offline views do not draw. The tests read the pack from
# the build copy's ../../server/data/pack: a symlink to SOURCE's is made.
# PERF_FEATURES=profile builds client910 with those cargo features (the
# engine profiler, `rs910_core::profile`; lane E-A4's overhead A/B).
set -euo pipefail
NAME=$1
SOURCE=$2
: "${CARGO_TARGET_DIR:?set CARGO_TARGET_DIR}"
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
STAGE="$CARGO_TARGET_DIR/perf-src/$NAME.stage"
DEST="$CARGO_TARGET_DIR/perf-src/$NAME"
mkdir -p "$STAGE" "$DEST" "$CARGO_TARGET_DIR/perf-bin"
touch "$STAGE/.perf-copy" "$DEST/.perf-copy"
if [[ "$SOURCE" == git:* ]]; then
  rm -rf "$STAGE/tools"
  git -C "$REPO" archive "${SOURCE#git:}" tools | tar -x -C "$STAGE"
else
  rsync -a --delete --exclude target --exclude '.git' "$SOURCE/tools/" "$STAGE/tools/"
fi
python3 "$HERE/instrument.py" "$STAGE" >&2
# Content-compared copy without times: unchanged files keep their mtime.
rsync -rlc --delete --exclude target "$STAGE/tools/" "$DEST/tools/"
BIN_ROOT="$CARGO_TARGET_DIR/perf-bin"
export CARGO_TARGET_DIR="$CARGO_TARGET_DIR/perf-target/$NAME"
FEATURES=()
[ -n "${PERF_FEATURES:-}" ] && FEATURES=(--features "$PERF_FEATURES")
(cd "$DEST/tools" && cargo build --release --offline -q -p client910 --bin client910 ${FEATURES[@]+"${FEATURES[@]}"}) >&2
cp "$CARGO_TARGET_DIR/release/client910" "$BIN_ROOT/client910-$NAME"
echo "$BIN_ROOT/client910-$NAME"
if [ "${PERF_TESTS:-0}" = 1 ]; then
  mkdir -p "$DEST/server/data"
  ln -sfn "$REPO/server/data/pack" "$DEST/server/data/pack"
  ln -sfn "$REPO/docs" "$DEST/docs"
  EXE=$(cd "$DEST/tools" && cargo test --offline -q -p client910 --lib --no-run --message-format=json 2>/dev/null \
    | python3 -c "import sys,json; print([m['executable'] for m in map(json.loads, sys.stdin) if m.get('reason')=='compiler-artifact' and m.get('executable') and m['target']['name']=='client910'][-1])")
  cp "$EXE" "$BIN_ROOT/client910-tests-$NAME"
  echo "$BIN_ROOT/client910-tests-$NAME"
fi
