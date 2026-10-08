#!/bin/sh
# Regenerate the world travel session-replay fixture (scenario_world_travel):
#   tools/client910/fixtures/session-replay/world-travel/record.sh
# Needs the generated world and skills tables (`npm --prefix server run data:generate -- world skills`).
# One real client, from a written save at the Lumbridge spawn (location dev_player_spawn, 3222 3222) with a genie's
# lamp, a vis wax and an amulet of glory (4) in the backpack and Agility at the Agility Pyramid's level:
# - the lamp's Rub (backpack slot 0, option 1): the skill choice window (1263); its Prayer button (1263:34, option 1,
#   the client's own choice) and Confirm (the pause button the window builds as Prayer's button, 1263:13 child 7);
# - the vis wax's Activate (slot 1, option 1) and the options box's first row (1188:8, a pause): ten Quick Teleport
#   charges;
# - Home Teleport (1465:18) and the network window's Burthorpe Quick Teleport (1092:13, option 2): the player is at
#   Burthorpe at once (a mid-session map transaction);
# - the glory's Rub (slot 2, option 4) and the options box's fourth row (1188:23, Al Kharid): a charge used, the
#   player in Al Kharid (a map transaction);
# - server commands from then on: `tele` beside Pollnivneach's smokey well (a map transaction), its Climb down (a
#   dungeon entrance resolved by the dated wiki's Smoke Dungeon page: the Smoke Dungeon, a map transaction) and the
#   dungeon rope's Climb-up back, then `tele` to the Agility Pyramid's start and its first obstacles (`oploc`, as the
#   client's menu sends them: the headless replay picks no scene locs).
# The UI operations come before the first map transaction or after it with slack (a transaction's length depends
# on the recording machine); server commands are held while a transaction runs and resume 100 cycles after its
# install. With SHOTS_DIR set it saves a screenshot at each SHOTS cycle and stops at the last (client-only).
# Env: LOBBY_PORT, WORLD_PORT (45770, 45771), CYCLES, the *_CYCLE values, SHOTS_DIR, SHOTS, KEEP_WORK,
# CARGO_TARGET_DIR, PREFERENCES.
LOBBY_PORT=${LOBBY_PORT:-45770}
WORLD_PORT=${WORLD_PORT:-45771}
export LOBBY_PORT WORLD_PORT
. "$(dirname "$0")/../flow-lib.sh"
export MAX_SECONDS=${MAX_SECONDS:-400}
CYCLES=${CYCLES:-4900}
RUB_CYCLE=${RUB_CYCLE:-300}
PRAYER_CYCLE=${PRAYER_CYCLE:-360}
CONFIRM_CYCLE=${CONFIRM_CYCLE:-420}
WAX_CYCLE=${WAX_CYCLE:-500}
CHARGE_CYCLE=${CHARGE_CYCLE:-560}
HOME_CYCLE=${HOME_CYCLE:-640}
QUICK_CYCLE=${QUICK_CYCLE:-700}
GLORY_CYCLE=${GLORY_CYCLE:-1100}
AL_KHARID_CYCLE=${AL_KHARID_CYCLE:-1160}
flow_start
python3 - "$WORK/players" <<'PY'
import hashlib, json, os, sys
key = 'traveller'
wear = [-1] * 19
for slot, kit in [(4, 19), (6, 27), (7, 37), (8, 1), (9, 34), (10, 43), (11, 11)]:
    wear[slot] = -2147483648 | kit
# A genie's lamp, a vis wax, an amulet of glory (4).
backpack = [[2528, 1], [32092, 1], [1712, 1]] + [[-1, 0]] * 25
save = {'version': 2, 'accountKey': key, 'x': 3222, 'z': 3222, 'level': 0, 'backpack': backpack,
        'worn': [[-1, 0]] * 19, 'identityAppearance': {'wear': wear, 'colours': [1, 2, 3, 4, 0, 0, 0, 0, 0, 0], 'textures': [0] * 10}}
path = os.path.join(sys.argv[1], 'accounts', hashlib.sha256(key.encode()).hexdigest() + '.json')
os.makedirs(os.path.dirname(path), exist_ok=True)
json.dump(save, open(path, 'w'))
PY
export CLIENT910_RECORD="$WORK/raw.rtr"
# Components as the client packs them (interface << 16 | component).
BACKPACK=96534535
PRAYER=82772002
CONFIRM=82771981
OPTION_1=77856776
OPTION_4=77856791
HOME_TELEPORT=96010258
BURTHORPE=71565325
export CLIENT910_UI_OPERATIONS="${OPERATIONS:-$RUB_CYCLE,$BACKPACK,0,1;$PRAYER_CYCLE,$PRAYER,-1,1;$CONFIRM_CYCLE,$CONFIRM,7,0;$WAX_CYCLE,$BACKPACK,1,1;$CHARGE_CYCLE,$OPTION_1,-1,0;$HOME_CYCLE,$HOME_TELEPORT,-1,1;$QUICK_CYCLE,$BURTHORPE,-1,2;$GLORY_CYCLE,$BACKPACK,2,4;$AL_KHARID_CYCLE,$OPTION_4,-1,0}"
export CLIENT_ARGS=""
if [ -n "${SHOTS_DIR:-}" ]; then
  mkdir -p "$SHOTS_DIR"
  export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-450,800,1300}
  CLIENT_ARGS="--screenshot $SHOTS_DIR/world-travel.png"
fi
# One command per 100 cycles once a map is installed (the first at cycle 101; a map transaction holds them until 100
# cycles after its install). Fourteen fillers let the UI operations (to the glory's teleport) finish first; then:
# Agility 75 (the pyramid's obstacles never fail from there, so the session is deterministic); beside Pollnivneach's
# smokey well (3311 2962); the well's Climb down (loc 36002 at 3310 2962) and the Smoke Dungeon rope's Climb-up (loc 6439
# at 3205 9379; two fillers while the climb plays);
# the pyramid's entrance (3354 2829); its stairs' Climb-up (10857 at 3354 2831), the low wall (10865 at 3354 2849),
# the ledge (10860 at 3364 2851) and the plank (10868 at 3375 2845), with fillers while the player walks to each.
flow_client traveller notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout \
  notimeout notimeout notimeout notimeout notimeout "setxp 16 1210421" "tele 3311 2962 0" "oploc 1 3310 2962 36002" \
  "oploc 1 3205 9379 6439" notimeout notimeout "tele 3354 2829 0" "oploc 1 3354 2831 10857" notimeout "oploc 1 3354 2849 10865" \
  notimeout notimeout notimeout notimeout notimeout "oploc 1 3364 2851 10860" notimeout notimeout notimeout notimeout \
  notimeout "oploc 1 3375 2845 10868" notimeout notimeout notimeout notimeout
flow_finish "$CYCLES" "$HERE"
