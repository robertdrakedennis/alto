#!/bin/sh
# Regenerate the Lumbridge General Store session-replay fixture (scenario_world::recorded_store_session_*):
#   tools/client910/fixtures/session-replay/world-store/record.sh
# One real client talks to the shopkeeper (a conversation with choices), buys from the shop it opens and teleports
# with the lodestone network. (The door click and the stairs click of this recording landed on the ground and walked
# the player: a scene click's pixel read off a settled camera did not hit the target the way it did in the scout run.
# Doors, ladders, stairs and banks are checked end to end by the server's WorldE2E test, not by this replay.)
# The character is placed by a written save (a teleport would rebuild the map mid-session, which the replay does not install):
# - the save puts the player at (3220, 3241), east of the store's door (loc 45476 at 3219,3241): a target more than
#   two tiles from the player is under a side panel or off the scene;
# - server commands (one per 100 cycles): notimeout, `invset 0 995 1000` (1000 coins), `npcadd 520 -2 0` (the
#   shopkeeper, NPC 520, just inside the door), notimeout, `talkto 520` (starts his conversation, as his Talk-to would:
#   the NPC's pixel cannot be read off a replay), eight notimeout, `warp 3215 3238 0` (the foot of the stairs, a
#   teleport inside the build area);
# - left clicks (canvas pixels, 1024x768): the door and, later, the stairs (both walked, see above), the client's own
#   clicks on the conversation (Continue, the first option, Continue) and on the shop (the first stock row, Buy, the
#   close button); then the client's own operations of the Home Teleport button (1465:18) and of the Lumbridge
#   button of the lodestone window (1092:18).
# The pixels of the door and of the stairs, of the Continue and option buttons and of the shop's rows and Buy button
# were read off scouting runs of the same world (SCOUT=1: stand points set by `warp`, the conversation answered by the
# `dialogue` console command) with `scenario_world::scout_pixels`, once the camera had settled on each stand.
# Env: SCOUT, SHOTS_DIR (screenshots at the SHOTS cycles, client-only), the *_CLICK pixels, KEEP_WORK, CARGO_TARGET_DIR.
. "$(dirname "$0")/../flow-lib.sh"
SCOUT=${SCOUT:-}
DOOR_CLICK=${DOOR_CLICK:-360,194}
STAIRS_CLICK=${STAIRS_CLICK:-550,150}
CLOSE_CLICK=${CLOSE_CLICK:-752,228}
CONTINUE_CLICK=${CONTINUE_CLICK:-261,750}
OPTION_CLICK=${OPTION_CLICK:-260,683}
ROW_CLICK=${ROW_CLICK:-291,298}
BUY_CLICK=${BUY_CLICK:-727,517}
flow_start
python3 - "$WORK/players" <<'PY'
import hashlib, json, os, sys
key = 'storekeeper'
wear = [-1] * 19
for slot, kit in [(4, 19), (6, 27), (7, 37), (8, 1), (9, 34), (10, 43), (11, 11)]:
    wear[slot] = -2147483648 | kit
save = {'version': 2, 'accountKey': key, 'x': 3220, 'z': 3241, 'level': 0, 'backpack': [[-1, 0]] * 28,
        'worn': [[-1, 0]] * 19, 'identityAppearance': {'wear': wear, 'colours': [1, 2, 3, 4, 0, 0, 0, 0, 0, 0], 'textures': [0] * 10}}
path = os.path.join(sys.argv[1], 'accounts', hashlib.sha256(key.encode()).hexdigest() + '.json')
os.makedirs(os.path.dirname(path), exist_ok=True)
json.dump(save, open(path, 'w'))
PY
export CLIENT910_RECORD="$WORK/raw.rtr"
export CLIENT_ARGS=""
if [ -n "$SCOUT" ]; then
  # One command per 100 cycles: P0 start, P1 at the door, P2 upstairs, P3 downstairs, then the talk and its screens.
  CYCLES=${CYCLES:-2100}
  if [ -n "${SHOTS_DIR:-}" ]; then
    mkdir -p "$SHOTS_DIR"
    export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-390,590,790,990,1190,1390,1590,1790}
    CLIENT_ARGS="--screenshot $SHOTS_DIR/world-store.png"
  fi
  flow_client storekeeper notimeout "invset 0 995 1000" "npcadd 520 -8 -1" notimeout "warp 3219 3241 0" notimeout "warp 3214 3239 1" notimeout "warp 3215 3238 0" notimeout "talkto 520" notimeout "dialogue continue" notimeout "dialogue pick 0" notimeout "dialogue continue" notimeout notimeout notimeout
else
  CYCLES=${CYCLES:-2350}
  export CLIENT910_UI_CLICKS="l,$DOOR_CLICK,350;l,$CONTINUE_CLICK,700;l,$OPTION_CLICK,800;l,$CONTINUE_CLICK,900;l,$ROW_CLICK,1100;l,$BUY_CLICK,1200;l,$CLOSE_CLICK,1300;l,$STAIRS_CLICK,1550"
  export CLIENT910_UI_OPERATIONS="1800,96010258,-1,1;1850,71565330,-1,1"
  if [ -n "${SHOTS_DIR:-}" ]; then
    mkdir -p "$SHOTS_DIR"
    export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-340,440,760,860,960,1160,1260,1360,1540,1660,1820,2000,2200,2400}
    CLIENT_ARGS="--screenshot $SHOTS_DIR/world-store.png"
  fi
  flow_client storekeeper notimeout "invset 0 995 1000" "npcadd 520 -2 0" notimeout "talkto 520" notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout "warp 3215 3238 0" notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout
fi
flow_finish "$CYCLES" "$HERE"
