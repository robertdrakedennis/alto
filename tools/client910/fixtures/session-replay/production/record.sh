#!/bin/sh
# Regenerate the production session-replay fixture (scenario_production):
#   tools/client910/fixtures/session-replay/production/record.sh
# One real client smelts two bronze bars at a furnace and smiths a helmet from
# them at an anvil (interface 37, the smelting and smithing window), then
# fletches a bow from logs through Make-X (window 1007), strings it, and mixes
# an attack potion from the Mix option of an unfinished one.
# Env: LOBBY_PORT, WORLD_PORT (47310 and 47311), KEEP_WORK, SHOTS_DIR (where
# screenshots go; none when unset), CARGO_TARGET_DIR.
LOBBY_PORT=${LOBBY_PORT:-47310}
WORLD_PORT=${WORLD_PORT:-47311}
export LOBBY_PORT WORLD_PORT
. "$(dirname "$0")/../flow-lib.sh"
export ALTO_SKILLS_ALWAYS=1
export MAX_SECONDS=${MAX_SECONDS:-400}
flow_start
# The character's save (account key = the lower-cased login name).
START_X=${START_X:-3228} START_Z=${START_Z:-3255} python3 - "$WORK/players" <<'PY'
import hashlib, json, os, sys
key = 'production'
wear = [-1] * 19
for slot, kit in [(4, 19), (6, 27), (7, 37), (8, 1), (9, 34), (10, 43), (11, 11)]:
    wear[slot] = -2147483648 | kit
# two copper and two tin ore, a hammer; logs, a knife and a bowstring; an unfinished potion and an eye of newt.
backpack = [[436, 1], [436, 1], [438, 1], [438, 1], [2347, 1], [1511, 1], [946, 1], [1777, 1], [91, 1], [221, 1]] + [[-1, 0]] * 18
save = {'version': 2, 'accountKey': key, 'x': int(os.environ.get('START_X', '3228')), 'z': int(os.environ.get('START_Z', '3255')), 'level': 0, 'backpack': backpack,
        'worn': [[-1, 0]] * 19, 'identityAppearance': {'wear': wear, 'colours': [1, 2, 3, 4, 0, 0, 0, 0, 0, 0], 'textures': [0] * 10}}
path = os.path.join(sys.argv[1], 'accounts', hashlib.sha256(key.encode()).hexdigest() + '.json')
os.makedirs(os.path.dirname(path), exist_ok=True)
json.dump(save, open(path, 'w'))
PY
# Pixel positions are for a 1500x950 window (the smelting window is wider than the 1024x768 layout can hold).
export CLIENT910_WINDOW_SIZE=${CLIENT910_WINDOW_SIZE:-1500,950}
BEGIN_CLICK=${BEGIN_CLICK:-952,531}
BOW_ROW=${BOW_ROW:-596,429}
FLETCH_CLICK=${FLETCH_CLICK:-859,615}
BACKPACK=96534535
export CLIENT910_RECORD="$WORK/raw.rtr"
export CLIENT_ARGS=""
if [ -n "${SHOTS_DIR:-}" ]; then
  mkdir -p "$SHOTS_DIR"
  export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-600,800,1050,1300,1700,1850,2000,2150,2400}
  CLIENT_ARGS="--screenshot $SHOTS_DIR/production.png"
fi
# Server commands, one per 100 cycles from cycle 101: `setxp 13 600` (Smithing 6, so bronze starts at full heat),
# `oploc` of the furnace's Smelt at cycle 501 and of the anvil's Smith at cycle 1001 (the headless replay cannot
# pick these animated locs from the scene to reproduce a click). Clicks: Begin Project of each window; the Craft
# option of the logs (backpack slot 5), the bow row and the Fletch button; the String option of the bow (slot 1),
# the Mix option of the unfinished potion (slot 8).
export CLIENT910_UI_HOVER="${HOVER:-$BOW_ROW,1850,1909;$FLETCH_CLICK,1912,1979}"
export CLIENT910_UI_CLICKS="${CLICKS:-l,$BEGIN_CLICK,660;l,$BEGIN_CLICK,1180;l,$BOW_ROW,1910;l,$FLETCH_CLICK,1980}"
export CLIENT910_UI_OPERATIONS="${OPERATIONS:-1850,$BACKPACK,5,1;2150,$BACKPACK,1,1;2300,$BACKPACK,8,1}"
flow_client production notimeout "setxp 13 600" notimeout notimeout "oploc 1 3226 3256 113261" notimeout notimeout notimeout notimeout "oploc 1 3229 3254 113258" notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout
flow_finish "${FINISH:-2380}" "$HERE"
