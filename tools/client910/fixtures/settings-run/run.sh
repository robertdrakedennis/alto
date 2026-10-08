#!/bin/sh
# One scripted online run through the real settings interface, with
# screenshots: graphics controls (checkboxes and dropdowns) and their effect
# on the world, real exclusive fullscreen with the keep-this-setting dialog,
# a key rebind on the Controls tab, and the way back to the window.
#
#   tools/client910/fixtures/settings-run/run.sh [OUT_DIR]
#
# Starts its own dev lobby and world (LOBBY_PORT 40610, WORLD_PORT 40611 by
# default; never the owner's 43594/43595), runs the release client with the
# CLIENT910_* input injectors below (they enter the same input queues the
# window events do) and stops the servers by PID when the client exits.
# The client writes `OUT_DIR/settings_<cycle>.png` and exits after the last.
# It needs a display: the fullscreen step really changes the video mode.
# The mouse coordinates are canvas pixels of the 800x600 window and, after
# fullscreen, of the 1920x1200 frame (the first listed video mode).
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../../.." && pwd)
OUT=${1:-${TMPDIR:-/tmp}/settings-run}
LOBBY_PORT=${LOBBY_PORT:-40610}
WORLD_PORT=${WORLD_PORT:-40611}
MAX_SECONDS=${MAX_SECONDS:-180}
WORK=$(mktemp -d)
mkdir -p "$OUT" "$WORK/players" "$WORK/client"
LOBBY= WORLD= CLIENT=
cleanup() {
  for pid in $CLIENT $LOBBY $WORLD; do kill -TERM "$pid" 2>/dev/null || true; done
  rm -rf "$WORK"
}
trap cleanup EXIT
(cd "$ROOT/tools/client910" && cargo build --release)
BIN=${CARGO_TARGET_DIR:-$ROOT/tools/target}/release/client910
cd "$ROOT/server"
export ALTO_LOBBY_PORT=$LOBBY_PORT ALTO_WORLD_PORT=$WORLD_PORT ALTO_PLAYER_DATA_DIR=$WORK/players
node src/lostcity/lobby.ts > "$WORK/lobby.log" 2>&1 & LOBBY=$!
node src/lostcity/world.ts > "$WORK/world.log" 2>&1 & WORLD=$!
until grep -q "Listening on port $WORLD_PORT" "$WORK/world.log"; do sleep 1; done
cd "$ROOT/tools/client910"
# Component ids: 1513:155 is the graphics panel's control row (children 4,
# 8, 10, 13, 17 are the scenery-shadow and particle dropdowns and the ground
# decoration, fog and skybox checkboxes), 1477:829 the open dropdown's
# entries (child 2n+1 is entry n), 1477:656 the settings tabs (children 7
# graphics, 11 controls), 1477:659 the window's close button, 1444:15 the
# camera-up "Change Keybind" button, 883:14 the dialog's YES.
OPS="440,93913102,-1,1"                  # Options -> Settings
OPS="$OPS;480,96797328,7,1"              # Graphics tab
OPS="$OPS;520,99156123,13,1;525,99156123,17,1;530,99156123,10,1"
OPS="$OPS;540,99156123,8,1;545,96797501,1,1"     # particles: first entry
OPS="$OPS;555,99156123,4,1;560,96797501,1,1"     # scenery shadows: first entry
OPS="$OPS;640,96797331,1,1"              # close the settings window
OPS="$OPS;880,93913102,-1,1;920,96797328,7,1"    # reopen on the graphics tab
OPS="$OPS;1080,96797328,11,1;1150,94633999,-1,1" # Controls tab, camera-up keybind
OPS="$OPS;1260,96797328,7,1"             # back to the graphics tab
KEYS="400,27,0;402,27,1;840,27,0;842,27,1;1170,89,0;1174,89,1"
CLICKS="l,665,400,960;l,893,707,1020;l,1235,625,1300"
CLIENT910_PREFERENCES_FILE="$WORK/client/preferences.dat" \
CLIENT910_VARC_FILE="$WORK/client/client-vars.dat" CLIENT910_UID192_FILE="$WORK/client/random.dat" \
CLIENT910_KEY_INPUT="$KEYS" CLIENT910_UI_OPERATIONS="$OPS" CLIENT910_UI_CLICKS="$CLICKS" \
CLIENT910_SCREENSHOT_SERIES=300,600,780,1000,1060,1240,1360 CLIENT910_LOG=info \
"$BIN" --direct-login --lobby-port "$LOBBY_PORT" --world-port "$WORLD_PORT" \
  --username settingsrun --password password --pack-root "$ROOT/server/data/pack" \
  --cache-dir "$WORK/cache" --screenshot "$OUT/settings.png" > "$WORK/client.log" 2>&1 & CLIENT=$!
waited=0
while kill -0 "$CLIENT" 2>/dev/null && [ $waited -lt "$MAX_SECONDS" ]; do sleep 1; waited=$((waited + 1)); done
grep -E "screenshot written|failures" "$WORK/client.log" || true
ls "$OUT"
