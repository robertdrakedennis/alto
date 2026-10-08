#!/bin/sh
# Regenerate the agility and thieving session-replay fixture (scenario_support):
#   tools/client910/fixtures/session-replay/support/record.sh
# Needs the generated tables (`npm --prefix server run data:generate -- skills`).
# One real client runs the last four obstacles of the Gnome Stronghold course
# (the balancing rope, the branch down, the net and the pipe), then picks a
# Menaphite marketeer's pocket twice and steals from a Baker's stall. The
# character starts at the rope's west end on the upper platform, (2477, 3420,
# 2), by a written save (a teleport would rebuild the map mid-session, which
# the replay does not install). The dev server runs with the TEST-ONLY
# ALTO_THIEVING_ROLLS, so the first pocket is picked (30 coins), the second
# catches the thief (the stun) and the stall gives a cake.
# - server commands, one per 100 cycles from cycle 101: notimeout, `setxp 17
#   67983` (Thieving 46, the marketeer's level); `warp` (no new map) to the
#   start of the next obstacle before each click after the first, where the
#   camera shows it ahead of the player: (2486, 3420, 2) before the branch,
#   (2486, 3425, 0) before the net, (2487, 3430, 0) before the pipe and again
#   after it; then `npcadd 24476 1 0` (a marketeer east of the player, standing
#   still; its left click is Pickpocket) and `locadd 34384 10 0 -3 0` (a Baker's
#   stall west of the player);
# - left clicks with the mouse resting on the target from 10 cycles before:
#   ROPE_CYCLE, BRANCH_CYCLE, NET_CYCLE, PIPE_CYCLE (OPLOC1: each crossing is a
#   forced movement), the marketeer at MARKETEER_CYCLE and CAUGHT_CYCLE
#   (OPNPC3) and the stall at STALL_CYCLE (OPLOC2 "Steal-from").
# The pixels were found with CLIENT910_SCREENSHOT_SERIES in a 1024x768 window
# (screenshots are 2048x1536, twice the click coordinates) by a run with no
# clicks (the warps put the player where each click is made). The last
# recording used ROPE_CLICK=600,415 BRANCH_CLICK=612,445 NET_CLICK=470,280
# PIPE_CLICK=515,262 MARKETEER_CLICK=600,365 STALL_CLICK=300,395.
# The client is stopped after CYCLES logic cycles. With SHOTS_DIR set it saves a
# screenshot at each SHOTS cycle and stops at the last (client-only).
# Env: ROPE_CLICK, BRANCH_CLICK, NET_CLICK, PIPE_CLICK, MARKETEER_CLICK,
# STALL_CLICK (x,y; an empty one records without that click), the *_CYCLE
# values, CYCLES (2800), SHOTS_DIR, SHOTS, KEEP_WORK, LOBBY_PORT (45690),
# WORLD_PORT (45691), CARGO_TARGET_DIR.
LOBBY_PORT=${LOBBY_PORT:-45690}
WORLD_PORT=${WORLD_PORT:-45691}
. "$(dirname "$0")/../flow-lib.sh"
export ALTO_THIEVING_ROLLS=0,0,0,0,0,0.999,0,0
ROPE_CLICK=${ROPE_CLICK-}
BRANCH_CLICK=${BRANCH_CLICK-}
NET_CLICK=${NET_CLICK-}
PIPE_CLICK=${PIPE_CLICK-}
MARKETEER_CLICK=${MARKETEER_CLICK-}
STALL_CLICK=${STALL_CLICK-}
ROPE_CYCLE=${ROPE_CYCLE:-500}
BRANCH_CYCLE=${BRANCH_CYCLE:-900}
NET_CYCLE=${NET_CYCLE:-1200}
PIPE_CYCLE=${PIPE_CYCLE:-1500}
MARKETEER_CYCLE=${MARKETEER_CYCLE:-2100}
CAUGHT_CYCLE=${CAUGHT_CYCLE:-2300}
STALL_CYCLE=${STALL_CYCLE:-2650}
CYCLES=${CYCLES:-2800}
flow_start
# The character's save (account key = the lower-cased login name).
python3 - "$WORK/players" <<'PY'
import hashlib, json, os, sys
key = 'acrobat'
wear = [-1] * 19
for slot, kit in [(4, 19), (6, 27), (7, 37), (8, 1), (9, 34), (10, 43), (11, 11)]:
    wear[slot] = -2147483648 | kit
save = {'version': 2, 'accountKey': key, 'x': 2477, 'z': 3420, 'level': 2, 'backpack': [[-1, 0]] * 28,
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
add_click "$ROPE_CLICK" "$ROPE_CYCLE"
add_click "$BRANCH_CLICK" "$BRANCH_CYCLE"
add_click "$NET_CLICK" "$NET_CYCLE"
add_click "$PIPE_CLICK" "$PIPE_CYCLE"
add_click "$MARKETEER_CLICK" "$MARKETEER_CYCLE"
add_click "$MARKETEER_CLICK" "$CAUGHT_CYCLE"
add_click "$STALL_CLICK" "$STALL_CYCLE"
[ -z "$CLICKS" ] || export CLIENT910_UI_CLICKS="$CLICKS" CLIENT910_UI_HOVER="$HOVER"
export CLIENT_ARGS=""
if [ -n "${SHOTS_DIR:-}" ]; then
  mkdir -p "$SHOTS_DIR"
  export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-490,890,1190,1490,2090}
  CLIENT_ARGS="--screenshot $SHOTS_DIR/support.png"
fi
# One command per 100 cycles (command k runs at cycle 100k + 1): the warps at 801, 1101, 1401 and 1801 (back in front of
# the pipe, where the thieving is done), the marketeer at 1901 and the stall at 2001; fillers between.
flow_client acrobat notimeout "setxp 17 67983" notimeout notimeout notimeout notimeout notimeout "warp 2486 3420 2" \
  notimeout notimeout "warp 2486 3425 0" notimeout notimeout "warp 2487 3430 0" notimeout notimeout notimeout "warp 2487 3430 0" \
  "npcadd 24476 1 0" "locadd 34384 10 0 -3 0"
flow_finish "$CYCLES" "$HERE"
