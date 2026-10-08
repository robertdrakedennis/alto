#!/bin/sh
# Regenerate the Cook's Assistant session-replay fixture (scenario_quest::recorded_cooks_assistant_*):
#   tools/client910/fixtures/session-replay/quest/record.sh
# One real client does Cook's Assistant from the offer to the scroll: the cook's conversation, the quest noticeboard
# (the journal window's overview) and its Accept Quest button, the quest list's row turning from not started to
# started to complete, the hand-in of the three ingredients, the complete scroll with the quest point, and the
# journal opened from the quest list. Gathering the ingredients is covered by the server's QuestsE2E test; here the
# `invset` command puts them in the backpack.
# The character is placed by a written save beside the cook's tile in Lumbridge Castle's kitchen (3208, 3214).
# Server commands (one per 100 cycles from cycle 101): notimeout, `npcadd 278 1 1` (the cook, NPC 278, one tile
# north-east), notimeout, `talkto 278` (his Talk-to: the replay cannot pick an NPC from the scene), then notimeout
# while the client answers, three `invset` (top-quality milk 15413, extra fine flour 15414, super large egg 15412),
# `talkto 278` again, notimeout to the end.
# The client's own input: left clicks on the chatbox's Continue and on the first option (canvas pixels of the
# 1024x768 layout, read off a scout run with `scenario_world::scout_pixels`), and its own operations of Accept Quest
# (1500:404), the complete scroll's Continue (1244:21) and the quest list's row of Cook's Assistant (1783:7, child 1,
# its first option: the journal once started).
# SCOUT=1 runs the same session with the `dialogue continue` / `dialogue pick 0` commands answering for the client.
# Env: SCOUT, SHOTS_DIR (screenshots at the SHOTS cycles), KEEP_WORK, CARGO_TARGET_DIR, LOBBY_PORT, WORLD_PORT.
# The ports are set before flow-lib.sh, whose defaults would otherwise win.
LOBBY_PORT=${LOBBY_PORT:-47520}
WORLD_PORT=${WORLD_PORT:-47521}
export LOBBY_PORT WORLD_PORT
. "$(dirname "$0")/../flow-lib.sh"
export MAX_SECONDS=${MAX_SECONDS:-500}
SCOUT=${SCOUT:-}
CONTINUE_CLICK=${CONTINUE_CLICK:-261,750}
PLAIN_CLICK=${PLAIN_CLICK:-261,750}
OPTION_CLICK=${OPTION_CLICK:-260,673}
flow_start
python3 - "$WORK/players" <<'PY'
import hashlib, json, os, sys
key = 'questcook'
wear = [-1] * 19
for slot, kit in [(4, 19), (6, 27), (7, 37), (8, 1), (9, 34), (10, 43), (11, 11)]:
    wear[slot] = -2147483648 | kit
save = {'version': 2, 'accountKey': key, 'x': 3208, 'z': 3214, 'level': 0, 'backpack': [[-1, 0]] * 28,
        'worn': [[-1, 0]] * 19, 'identityAppearance': {'wear': wear, 'colours': [1, 2, 3, 4, 0, 0, 0, 0, 0, 0], 'textures': [0] * 10}}
path = os.path.join(sys.argv[1], 'accounts', hashlib.sha256(key.encode()).hexdigest() + '.json')
os.makedirs(os.path.dirname(path), exist_ok=True)
json.dump(save, open(path, 'w'))
PY
export CLIENT910_RECORD="$WORK/raw.rtr"
export CLIENT_ARGS=""
# The client's operations: Accept Quest, the scroll's Continue, the journal from the quest list.
ACCEPT=98304404
SCROLL_CONTINUE=81526805
LIST_ROWS=116850695
export CLIENT910_UI_OPERATIONS="${OPERATIONS:-1250,$ACCEPT,-1,1;3650,$SCROLL_CONTINUE,-1,1;3750,$LIST_ROWS,1,1}"
CYCLES=${CYCLES:-3900}
if [ -n "${SHOTS_DIR:-}" ]; then
  mkdir -p "$SHOTS_DIR"
  export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-350,560,1060,1200,1300,1800,2250,2400,3000,3450,3700,3850,3990}
  CLIENT_ARGS="--screenshot $SHOTS_DIR/quest.png"
fi
if [ -n "$SCOUT" ]; then
  # Commands 4..: the answers of the first conversation (continue, pick 0, continue x5 to the noticeboard, Accept by the
  # operation at 1150, continue x5), the ingredients, the second conversation (continue x12), notimeout.
  flow_client questcook notimeout "npcadd 278 1 1" notimeout "talkto 278" \
    "dialogue continue" "dialogue pick 0" "dialogue continue" "dialogue continue" "dialogue continue" "dialogue continue" "dialogue continue" \
    notimeout "dialogue continue" "dialogue continue" "dialogue continue" "dialogue continue" "dialogue continue" \
    "invset 0 15413 1" "invset 1 15414 1" "invset 2 15412 1" "talkto 278" \
    "dialogue continue" "dialogue continue" "dialogue continue" "dialogue continue" "dialogue continue" "dialogue continue" \
    "dialogue continue" "dialogue continue" "dialogue continue" "dialogue continue" "dialogue continue" "dialogue continue" \
    notimeout notimeout notimeout notimeout notimeout
else
  # The same answers as the client's own clicks, 50 cycles after each command slot.
  C=$CONTINUE_CLICK; P=$PLAIN_CLICK; O=$OPTION_CLICK
  export CLIENT910_UI_CLICKS="${CLICKS:-l,$C,550;l,$O,650;l,$C,750;l,$C,850;l,$C,950;l,$C,1050;l,$C,1150;l,$C,1350;l,$C,1450;l,$C,1550;l,$C,1650;l,$C,1750;l,$C,2250;l,$P,2350;l,$C,2450;l,$P,2550;l,$C,2650;l,$P,2750;l,$C,2850;l,$C,2950;l,$C,3050;l,$C,3150;l,$C,3250;l,$C,3350}"
  flow_client questcook notimeout "npcadd 278 1 1" notimeout "talkto 278" \
    notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout \
    "invset 0 15413 1" "invset 1 15414 1" "invset 2 15412 1" "talkto 278" \
    notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout \
    notimeout notimeout notimeout notimeout notimeout
fi
flow_finish "$CYCLES" "$HERE"
