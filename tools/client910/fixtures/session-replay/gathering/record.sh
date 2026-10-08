#!/bin/sh
# Regenerate the gathering session-replay fixture (scenario_gathering):
#   tools/client910/fixtures/session-replay/gathering/record.sh
# One real client, standing at Lumbridge (3234, 3232) beside the willow of the
# map (loc 38616 at (3235, 3232)), chops the willow until it falls and grows
# back, twice. The script also sets up an iron rock and a lure spot and clicks
# them between the two chops, in the hope of recording mining with the stamina
# bar and a second fishing method. WHAT THE COMMITTED RECORDING CONTAINS: only
# the two willow chops proved out. The two other clicks were recorded as walk
# clicks (the client did not pick the rock added with `locadd`, and the spot had
# already moved along the bank before its click), so scenario_gathering replays
# the willow and treats those presses as walks; the iron rock with the stamina
# bar and the lure spot are covered by GatheringE2E.integration.test.ts. The
# dev server runs with the TEST-ONLY ALTO_SKILLS_ALWAYS=1 (every roll goes the
# player's way: the first roll of each kind hits, the willow falls after its
# first log and drops a nest, ores come in pairs, the lure spot catches every
# attempt and moves after its shortest wait) and the character is placed by a
# written save (a teleport would rebuild the map mid-session, which the replay
# does not install):
# - the save puts the player at (3234, 3232); server commands (one per 100
#   cycles from cycle 101) give the levels (`setxp 8 13363` = Woodcutting 30,
#   `setxp 14 4470` = Mining 20, `setxp 10 4470` = Fishing 20), the tools
#   (`invset`: bronze hatchet, fly fishing rod, 50 feathers), put the iron rock
#   west of the player (`locadd`) and, just before its click, a lure spot in the
#   water beyond the willow (`npcadd 329 2 1`);
# - left clicks with the mouse resting on the target from 10 cycles before: the
#   willow at cycle 900 and again at 2000 (OPLOC1 "Chop down"), the rock at 1200
#   and the spot at 2300 (both recorded as walks). The willow's hanging branches
#   cover the rock and the spot, so they are clicked while the willow is down.
#   The player stands next to all three, so the camera never moves; the pixels
#   were found with CLIENT910_SCREENSHOT_SERIES in a 1024x768 window (screenshots
#   are 2048x1536, twice the click coordinates). The last recording used
#   WILLOW_CLICK=547,146 IRON_CLICK=420,400 SPOT_CLICK=692,350.
# The client is stopped after CYCLES logic cycles. With SHOTS_DIR set it saves a
# screenshot at each SHOTS cycle and stops at the last (client-only; the
# recording is unchanged), so give CYCLES the last shot's cycle then. An empty
# click variable records without that click.
# Env: WILLOW_CLICK, IRON_CLICK, SPOT_CLICK (x,y), WILLOW_CYCLE (900), IRON_CYCLE
# (1200), WILLOW_AGAIN_CYCLE (2000), SPOT_CYCLE (2300), CYCLES (3000), SHOTS_DIR,
# SHOTS, KEEP_WORK, LOBBY_PORT, WORLD_PORT, CARGO_TARGET_DIR.
. "$(dirname "$0")/../flow-lib.sh"
export ALTO_SKILLS_ALWAYS=1
WILLOW_CLICK=${WILLOW_CLICK-}
IRON_CLICK=${IRON_CLICK-}
SPOT_CLICK=${SPOT_CLICK-}
WILLOW_CYCLE=${WILLOW_CYCLE:-900}
IRON_CYCLE=${IRON_CYCLE:-1200}
WILLOW_AGAIN_CYCLE=${WILLOW_AGAIN_CYCLE:-2000}
SPOT_CYCLE=${SPOT_CYCLE:-2300}
CYCLES=${CYCLES:-3000}
flow_start
# The character's save (account key = the lower-cased login name).
python3 - "$WORK/players" <<'PY'
import hashlib, json, os, sys
key = 'gatherer'
wear = [-1] * 19
for slot, kit in [(4, 19), (6, 27), (7, 37), (8, 1), (9, 34), (10, 43), (11, 11)]:
    wear[slot] = -2147483648 | kit
save = {'version': 2, 'accountKey': key, 'x': 3234, 'z': 3232, 'level': 0, 'backpack': [[-1, 0]] * 28,
        'worn': [[-1, 0]] * 19, 'identityAppearance': {'wear': wear, 'colours': [1, 2, 3, 4, 0, 0, 0, 0, 0, 0], 'textures': [0] * 10}}
path = os.path.join(sys.argv[1], 'accounts', hashlib.sha256(key.encode()).hexdigest() + '.json')
os.makedirs(os.path.dirname(path), exist_ok=True)
json.dump(save, open(path, 'w'))
PY
export CLIENT910_RECORD="$WORK/raw.rtr"
CLICKS=
HOVER=
add_click() { # click point, cycle
  [ -n "$1" ] || return 0
  CLICKS="${CLICKS:+$CLICKS;}l,$1,$2"
  HOVER="${HOVER:+$HOVER;}$1,$(($2 - 10)),$(($2 + 1))"
}
add_click "$WILLOW_CLICK" "$WILLOW_CYCLE"
add_click "$IRON_CLICK" "$IRON_CYCLE"
add_click "$WILLOW_CLICK" "$WILLOW_AGAIN_CYCLE"
add_click "$SPOT_CLICK" "$SPOT_CYCLE"
[ -z "$CLICKS" ] || export CLIENT910_UI_CLICKS="$CLICKS" CLIENT910_UI_HOVER="$HOVER"
export CLIENT_ARGS=""
if [ -n "${SHOTS_DIR:-}" ]; then
  mkdir -p "$SHOTS_DIR"
  export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-850,1900,3100}
  CLIENT_ARGS="--screenshot $SHOTS_DIR/gathering.png"
fi
# One command per 100 cycles (command k runs at cycle 100k + 1): notimeout, the levels, the tools, the iron rock; fillers; the lure spot (command 22, cycle 2201) just before its click.
flow_client gatherer notimeout "setxp 8 13363" "setxp 14 4470" "setxp 10 4470" "invset 0 1351 1" "invset 1 309 1" "invset 2 314 50" "locadd 113158 10 0 -1 0" \
  notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout "npcadd 329 2 1"
flow_finish "$CYCLES" "$HERE"
