#!/usr/bin/env bash
# Golden-session cache overlay (code-quality programme Phase 5).
#
# The recorded RTR1 sessions replay through ClientCore + ReplayIo from a
# small overlay of exactly the cache files they read (~13.5 MB). The overlay
# is revision-910 cache data, so it is NEVER committed: it lives under
# tools/target (gitignored).
#
#   tools/refactor/replay-overlay.sh export   # needs server/data/pack
#   tools/refactor/replay-overlay.sh pack     # -> tools/target/replay-overlay.tar.gz
#   tools/refactor/replay-overlay.sh run [dir]  # replays both sessions from it
#
# CI does not run this (no cache, no secrets); it is a local step.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DIR="${CLIENT910_REPLAY_OVERLAY:-$ROOT/tools/target/replay-overlay}"
MANIFEST="$ROOT/tools/client910/Cargo.toml"

case "${1:-}" in
  export)
    rm -rf "$DIR" && mkdir -p "$DIR"
    CLIENT910_REPLAY_OVERLAY_OUT="$DIR" cargo test --offline --manifest-path "$MANIFEST" --lib \
      app::session_replay::export_the_recorded_sessions_pack_overlay -- --ignored --exact
    echo "overlay written to $DIR ($(du -sh "$DIR" | cut -f1))"
    ;;
  pack)
    tar -C "$DIR" -czf "$ROOT/tools/target/replay-overlay.tar.gz" .
    echo "wrote tools/target/replay-overlay.tar.gz ($(du -h "$ROOT/tools/target/replay-overlay.tar.gz" | cut -f1))"
    ;;
  run)
    OVERLAY="${2:-$DIR}"
    CLIENT910_REPLAY_OVERLAY="$OVERLAY" cargo test --manifest-path "$MANIFEST" --lib --features no-pack \
      app::session_replay::recorded_sessions_replay_from_the_pack_free_overlay -- --ignored --exact
    ;;
  *)
    echo "usage: $0 export|pack|run [dir]" >&2
    exit 2
    ;;
esac
