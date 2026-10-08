#!/bin/sh
# Regenerate the divination, hunter and farming session-replay fixture (scenario_nature):
#   tools/client910/fixtures/session-replay/nature/record.sh
# One real client, standing on the path south of the Lumbridge tree patch
# (location lumbridge_tree_patch_path, 3229 3243), with a bird snare, a rake, a
# spade and an oak sapling in its backpack (a written save: a teleport would
# rebuild the map mid-session, which the replay does not install):
# - Hunter: lays the snare from the backpack (the backpack slot's Lay, a UI
#   operation) and steps west off it; a crimson swift added four tiles east
#   walks to the snare and is caught; the snare's Check gives the loot and XP.
# - Farming: rakes the patch clear, plants the sapling with the spade; later
#   `farmtick` runs one growth window (the oak's first form becomes its second),
#   then three more (grown: Check-health).
# - Divination: a pale wisp added east of the player is harvested (it opens into
#   its spring, NPC_INFO type change); an energy rift added west of the player
#   is configured (its Configure opens the conversion window, interface 131,
#   whose energy button sets the mode and closes it) and converts the memories
#   to energy.
# The dev server runs with the TEST-ONLY ALTO_SKILLS_ALWAYS=1 (every roll goes
# the player's way: the swift is caught, every harvest brings a memory, the
# spring lasts its longest, no crop sickens). Loc and NPC options are server
# commands (`oploc`, `opnpc`, `useloc`) as the client's menu would send them:
# the snare, the rift and the patch forms are animated locs the headless replay
# cannot pick, and the script cannot know the index of an added NPC.
# With SHOTS_DIR set it saves a screenshot at each SHOTS cycle and stops at the
# last (client-only; give CYCLES the last shot's cycle then).
# Env: LOBBY_PORT, WORLD_PORT (47420 and 47421), CYCLES (3100), SHOTS_DIR, SHOTS,
# KEEP_WORK, CARGO_TARGET_DIR.
# The ports are set before flow-lib.sh, which keeps any already set.
LOBBY_PORT=${LOBBY_PORT:-47420}
WORLD_PORT=${WORLD_PORT:-47421}
export LOBBY_PORT WORLD_PORT
. "$(dirname "$0")/../flow-lib.sh"
export ALTO_SKILLS_ALWAYS=1
export MAX_SECONDS=${MAX_SECONDS:-240}
CYCLES=${CYCLES:-3100}
flow_start
# The character's save (account key = the lower-cased login name).
python3 - "$WORK/players" <<'PY'
import hashlib, json, os, sys
key = 'nature'
wear = [-1] * 19
for slot, kit in [(4, 19), (6, 27), (7, 37), (8, 1), (9, 34), (10, 43), (11, 11)]:
    wear[slot] = -2147483648 | kit
# A bird snare, a rake, a spade and an oak sapling.
backpack = [[10006, 1], [5341, 1], [952, 1], [5370, 1]] + [[-1, 0]] * 24
save = {'version': 2, 'accountKey': key, 'x': 3229, 'z': 3243, 'level': 0, 'backpack': backpack,
        'worn': [[-1, 0]] * 19, 'identityAppearance': {'wear': wear, 'colours': [1, 2, 3, 4, 0, 0, 0, 0, 0, 0], 'textures': [0] * 10}}
path = os.path.join(sys.argv[1], 'accounts', hashlib.sha256(key.encode()).hexdigest() + '.json')
os.makedirs(os.path.dirname(path), exist_ok=True)
json.dump(save, open(path, 'w'))
PY
export CLIENT910_RECORD="$WORK/raw.rtr"
BACKPACK=96534535
ENERGY_BUTTON=8585226
# The bird snare's Lay (backpack slot 0, option 1) at cycle 350; the conversion window's energy button (131:10) at 2300.
export CLIENT910_UI_OPERATIONS="${OPERATIONS:-350,$BACKPACK,0,1;2300,$ENERGY_BUTTON,-1,1}"
export CLIENT_ARGS=""
if [ -n "${SHOTS_DIR:-}" ]; then
  mkdir -p "$SHOTS_DIR"
  export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-420,650,1450,2150,2260,2650,2800,3050}
  CLIENT_ARGS="--screenshot $SHOTS_DIR/nature.png"
fi
# One command per 100 cycles (command k runs at cycle 100k + 1):
#  1 notimeout; 2 Farming 15 (`setxp 19 2411`); 4 the crimson swift (NPC 5073) four tiles east;
#  7 Check the full snare (loc 19180 on the snare's tile); 8 Rake the patch (its parent loc 8391);
#  13 the sapling (slot 3) on the patch; 15 a pale wisp (NPC 18150) east; 16 Harvest it;
#  21 an energy rift (loc 87306, 2x2) west; 22 its Configure; 24 Convert memories; 27 one growth window; 29 three more.
flow_client nature notimeout "setxp 19 2411" notimeout "npcadd 5073 4 0" notimeout notimeout "oploc 1 3229 3243 19180" "oploc 1 3229 3244 8391" \
  notimeout notimeout notimeout notimeout "useloc 3 3229 3244 8391" notimeout "npcadd 18150 1 0" "opnpc 1 18150" notimeout notimeout notimeout notimeout \
  "locadd 87306 10 0 -2 -1" "oploc 2 3227 3242 87306" notimeout "oploc 1 3227 3242 87306" notimeout notimeout "farmtick" notimeout "farmtick 3" notimeout
flow_finish "$CYCLES" "$HERE"
