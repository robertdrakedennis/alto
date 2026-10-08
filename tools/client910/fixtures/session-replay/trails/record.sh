#!/bin/sh
# Regenerate the Treasure Trails session-replay fixture (scenario_trails::recorded_trails_*):
#   tools/client910/fixtures/session-replay/trails/record.sh
# One real client reads clues in the clue scroll window, digs at a coordinate, does an emote clue and opens a casket:
# - a medium trail of one step starts on the coordinate clue 2805 (`trail medium 2805 1`); the client reads it (its
#   backpack slot's Read: the scroll window 345 with the two bearings) and digs with the clue's own Dig (the sextant,
#   watch and chart are in the backpack): the casket takes the clue's slot;
# - the player steps onto the Fishing Trawler's pier (`warp`, inside the build area), an easy trail of one step starts
#   on the emote clue 10224 (panic on the pier, nothing worn); the client reads it, panics (the emotes window's list,
#   590:8, child 18), Uri comes (`talkto 5141`: the replay cannot pick an NPC from the scene) and hands over the easy
#   casket with one of his phrases, answered with the chatbox's Continue;
# - the client opens the medium casket: the reward window (364) draws the rewards from inventory 141.
# The character is placed by a written save on the coordinate's tile (2697, 3207) with a spade, sextant, watch and chart.
# Server commands (one per 100 cycles from cycle 101): notimeout, `trail medium 2805 1`, notimeout x2, `warp 2676 3170`,
# `trail easy 10224 1`, notimeout x2, `talkto 5141`, notimeout to the end.
# The client's own input: the operations below, a left click on the chat window's Emotes tab and one on the chatbox's Continue (canvas pixels of the
# 1024x768 layout, as the quest recording found them). SCOUT=1 answers with `dialogue continue` instead.
# Env: SCOUT, SHOTS_DIR (screenshots at the SHOTS cycles), KEEP_WORK, CARGO_TARGET_DIR, LOBBY_PORT, WORLD_PORT.
LOBBY_PORT=${LOBBY_PORT:-47540}
WORLD_PORT=${WORLD_PORT:-47541}
export LOBBY_PORT WORLD_PORT
. "$(dirname "$0")/../flow-lib.sh"
export MAX_SECONDS=${MAX_SECONDS:-300}
SCOUT=${SCOUT:-}
CONTINUE_CLICK=${CONTINUE_CLICK:-261,750}
# The chat window's Emotes tab (its last tab icon; the default layout shows All Chat): showing the emotes window lets
# its list draw (the list's hook runs when the window is drawn after the unlock variables it watches were sent).
EMOTES_TAB_CLICK=${EMOTES_TAB_CLICK:-331,580}
flow_start
python3 - "$WORK/players" <<'PY'
import hashlib, json, os, sys
key = 'trails'
wear = [-1] * 19
for slot, kit in [(4, 19), (6, 27), (7, 37), (8, 1), (9, 34), (10, 43), (11, 11)]:
    wear[slot] = -2147483648 | kit
# A spade, a sextant, a watch and a chart.
backpack = [[952, 1], [2574, 1], [2575, 1], [2576, 1]] + [[-1, 0]] * 24
save = {'version': 2, 'accountKey': key, 'x': 2697, 'z': 3207, 'level': 0, 'backpack': backpack,
        'worn': [[-1, 0]] * 19, 'identityAppearance': {'wear': wear, 'colours': [1, 2, 3, 4, 0, 0, 0, 0, 0, 0], 'textures': [0] * 10}}
path = os.path.join(sys.argv[1], 'accounts', hashlib.sha256(key.encode()).hexdigest() + '.json')
os.makedirs(os.path.dirname(path), exist_ok=True)
json.dump(save, open(path, 'w'))
PY
export CLIENT910_RECORD="$WORK/raw.rtr"
export CLIENT_ARGS=""
BACKPACK=96534535
EMOTES=38666248
PANIC=18
# Read the medium clue (slot 4) at 300, its Dig at 400; read the easy clue (slot 5) at 700, the Emotes tab at 750,
# panic at 800; open the medium casket (slot 4) at 1200.
export CLIENT910_UI_OPERATIONS="${OPERATIONS:-300,$BACKPACK,4,1;400,$BACKPACK,4,2;700,$BACKPACK,5,1;800,$EMOTES,$PANIC,1;1200,$BACKPACK,4,1}"
CYCLES=${CYCLES:-1400}
if [ -n "${SHOTS_DIR:-}" ]; then
  mkdir -p "$SHOTS_DIR"
  export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-330,520,730,860,1080,1260,1390}
  CLIENT_ARGS="--screenshot $SHOTS_DIR/trails.png"
fi
if [ -n "$SCOUT" ]; then
  export CLIENT910_UI_CLICKS="${CLICKS:-l,$EMOTES_TAB_CLICK,750}"
  flow_client trails notimeout "trail medium 2805 1" notimeout notimeout "warp 2676 3170" "trail easy 10224 1" notimeout notimeout \
    "talkto 5141" "dialogue continue" notimeout notimeout notimeout notimeout
else
  export CLIENT910_UI_CLICKS="${CLICKS:-l,$EMOTES_TAB_CLICK,750;l,$CONTINUE_CLICK,1000}"
  flow_client trails notimeout "trail medium 2805 1" notimeout notimeout "warp 2676 3170" "trail easy 10224 1" notimeout notimeout \
    "talkto 5141" notimeout notimeout notimeout notimeout notimeout
fi
flow_finish "$CYCLES" "$HERE"
