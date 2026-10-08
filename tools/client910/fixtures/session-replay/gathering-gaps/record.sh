#!/bin/sh
# Regenerate the gathering-gaps session-replay fixture (scenario_gathering_gaps):
#   tools/client910/fixtures/session-replay/gathering-gaps/record.sh
# One real client, standing west of the Catherby fruit tree patch (location
# catherby_fruit_tree_patch_west, 2859 3433), with a bronze ore box, two copper
# ores, an impling jar, a rake, a spade and an apple sapling in its backpack (a
# written save: a teleport would rebuild the map mid-session):
# - Mining: two copper rocks (2x2) are added on the open ground south-west of the
#   patch; mining the first lights the second as a rockertunity (the client's
#   large rockertunity spot on its tile, for this player alone); mining the lit
#   one puts it out.
# - Hunter: a baby impling added beside the player is caught into the jar.
# - Mining: the ore box's Fill (backpack slot 0, option 1, a UI operation) takes
#   the two copper ores into the box's copper varbit.
# - Farming: the patch is raked, the apple sapling planted with the spade, six
#   growth windows run (`farmtick 6`: grown, Check-health), and its health checked.
# The dev server runs with the TEST-ONLY ALTO_SKILLS_ALWAYS=1 (every roll goes
# the player's way: the rockertunity lights, the impling is caught, no crop
# sickens). Loc and NPC options are server commands (`oploc`, `opnpc`, `useloc`)
# as the client's menu would send them: the added rocks and NPC are not the
# script's to pick, and the patch forms are animated locs.
# With SHOTS_DIR set it saves a screenshot at each SHOTS cycle and stops at the
# last (client-only; give CYCLES the last shot's cycle then).
# Env: LOBBY_PORT, WORLD_PORT (47440 and 47441), CYCLES (2900), SHOTS_DIR, SHOTS,
# KEEP_WORK, CARGO_TARGET_DIR.
# The ports are set before flow-lib.sh, which keeps any already set.
LOBBY_PORT=${LOBBY_PORT:-47440}
WORLD_PORT=${WORLD_PORT:-47441}
export LOBBY_PORT WORLD_PORT
. "$(dirname "$0")/../flow-lib.sh"
export ALTO_SKILLS_ALWAYS=1
export MAX_SECONDS=${MAX_SECONDS:-240}
CYCLES=${CYCLES:-2900}
flow_start
# The character's save (account key = the lower-cased login name).
python3 - "$WORK/players" <<'PY'
import hashlib, json, os, sys
key = 'gaps'
wear = [-1] * 19
for slot, kit in [(4, 19), (6, 27), (7, 37), (8, 1), (9, 34), (10, 43), (11, 11)]:
    wear[slot] = -2147483648 | kit
# A bronze ore box, two copper ores, an impling jar, a rake, a spade and an apple sapling.
backpack = [[44779, 1], [436, 1], [436, 1], [11260, 1], [5341, 1], [952, 1], [5496, 1]] + [[-1, 0]] * 21
save = {'version': 2, 'accountKey': key, 'x': 2859, 'z': 3433, 'level': 0, 'backpack': backpack,
        'worn': [[-1, 0]] * 19, 'identityAppearance': {'wear': wear, 'colours': [1, 2, 3, 4, 0, 0, 0, 0, 0, 0], 'textures': [0] * 10}}
path = os.path.join(sys.argv[1], 'accounts', hashlib.sha256(key.encode()).hexdigest() + '.json')
os.makedirs(os.path.dirname(path), exist_ok=True)
json.dump(save, open(path, 'w'))
PY
export CLIENT910_RECORD="$WORK/raw.rtr"
BACKPACK=96534535
# The ore box's Fill (backpack slot 0, option 1) at cycle 1650.
export CLIENT910_UI_OPERATIONS="${OPERATIONS:-1650,$BACKPACK,0,1}"
export CLIENT_ARGS=""
if [ -n "${SHOTS_DIR:-}" ]; then
  mkdir -p "$SHOTS_DIR"
  export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-700,1000,1450,1750,2500,2850}
  CLIENT_ARGS="--screenshot $SHOTS_DIR/gaps.png"
fi
# One command per 100 cycles (command k runs at cycle 100k + 1):
#  1 notimeout; 2 Farming 27 (`setxp 19 9730`); 3 Hunter 40 (`setxp 21 37224`);
#  4, 5 two copper rocks (loc 113148, 2x2) at 2855 3428 and 2855 3431; 6 Mine the first;
#  9 Mine the second (the lit one); 12 a baby impling (NPC 1028) east; 13 Catch it;
#  17 Rake the patch (its parent loc 7965); 22 the sapling (slot 6) on the patch;
#  24 six growth windows; 26 Check-health.
flow_client gaps notimeout "setxp 19 9730" "setxp 21 37224" "locadd 113148 10 0 -4 -5" "locadd 113148 10 0 -4 -2" "oploc 1 2855 3428 113148" \
  notimeout notimeout "oploc 1 2855 3431 113148" notimeout notimeout "npcadd 1028 1 0" "opnpc 1 1028" notimeout notimeout notimeout \
  "oploc 1 2860 3433 7965" notimeout notimeout notimeout notimeout "useloc 6 2860 3433 7965" notimeout "farmtick 6" notimeout \
  "oploc 1 2860 3433 7965" notimeout notimeout
flow_finish "$CYCLES" "$HERE"
