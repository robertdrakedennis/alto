#!/bin/sh
# Regenerate the Make-X, fire, pouch and Abyss session-replay fixture (scenario_make_abyss):
#   tools/client910/fixtures/session-replay/make-and-abyss/record.sh
# One real client, standing east of the Lumbridge Swamp mine (location lumbridge_swamp_mine_east, 3229 3150) with a knife,
# a tinderbox, a small pouch, a bronze pickaxe, seven logs and five pure essence (a written save):
# - Firemaking: the first logs' Light (a UI operation) lights a fire; the player steps west. Normal logs burn for 120
#   seconds (200 ticks); then the fire is gone and a heap of ashes lies where it stood.
# - Make-X: the next logs' Craft opens Make-X (six can be made); the second product row's Select chooses the unstrung
#   shortbow, the quantity slider's Decrease button makes it five, and the Make button (a pause button) makes five.
# - Pouch: the small pouch's Fill, Empty and Fill.
# - Abyss (after the ashes): a Mage of Zamorak (the Wilderness parent NPC 2257) added two tiles north; Talk-to (the
#   miniquest is on hold: asking counts as finishing it, and his Teleport option appears), Teleport (layout 0: the
#   player stands before the rock of slot 1, loc 7143 at 3026 4813), then the rock's Mine.
# The dev server runs with the TEST-ONLY ALTO_SKILLS_ALWAYS=1 (every roll goes the player's way: the first layout,
# the rock passed at the first try). NPC and loc options are server commands (`opnpc`, `oploc`) as the client's menu
# would send them: the script cannot know an added NPC's index, and the headless replay cannot pick the Abyss's
# animated locs.
# Env: LOBBY_PORT, WORLD_PORT (47530 and 47531), CYCLES (8500), KEEP_WORK, CARGO_TARGET_DIR.
LOBBY_PORT=${LOBBY_PORT:-47530}
WORLD_PORT=${WORLD_PORT:-47531}
export LOBBY_PORT WORLD_PORT
. "$(dirname "$0")/../flow-lib.sh"
export ALTO_SKILLS_ALWAYS=1
export MAX_SECONDS=${MAX_SECONDS:-450}
CYCLES=${CYCLES:-8500}
flow_start
# The character's save (account key = the lower-cased login name).
python3 - "$WORK/players" <<'PYSAVE'
import hashlib, json, os, sys
key = 'makeabyss'
wear = [-1] * 19
for slot, kit in [(4, 19), (6, 27), (7, 37), (8, 1), (9, 34), (10, 43), (11, 11)]:
    wear[slot] = -2147483648 | kit
# A knife, a tinderbox, a small pouch, a bronze pickaxe, seven logs and five pure essence.
backpack = [[946, 1], [590, 1], [5509, 1], [1265, 1]] + [[1511, 1]] * 7 + [[7936, 1]] * 5 + [[-1, 0]] * 12
save = {'version': 2, 'accountKey': key, 'x': 3229, 'z': 3150, 'level': 0, 'backpack': backpack,
        'worn': [[-1, 0]] * 19, 'identityAppearance': {'wear': wear, 'colours': [1, 2, 3, 4, 0, 0, 0, 0, 0, 0], 'textures': [0] * 10}}
path = os.path.join(sys.argv[1], 'accounts', hashlib.sha256(key.encode()).hexdigest() + '.json')
os.makedirs(os.path.dirname(path), exist_ok=True)
json.dump(save, open(path, 'w'))
PYSAVE
export CLIENT910_RECORD="$WORK/raw.rtr"
export CLIENT_ARGS=""
BACKPACK=96534535
PRODUCT_LIST=89849878
SLIDER=89849875
MAKE_BUTTON=89784350
# UI operations (cycle,parent,child,op): Light of the logs in slot 4 (option 2); Craft of the logs in slot 5 (option 1);
# Select of the second product row (1371:22 child 5); the slider's Decrease (1371:19 child 0); Make (1370:30, its
# pause button: op 0); the pouch (slot 2): Fill, Empty, Fill.
export CLIENT910_UI_OPERATIONS="${OPERATIONS:-300,$BACKPACK,4,2;600,$BACKPACK,5,1;700,$PRODUCT_LIST,5,1;800,$SLIDER,0,1;900,$MAKE_BUTTON,-1,0;1600,$BACKPACK,2,1;1700,$BACKPACK,2,2;1800,$BACKPACK,2,1}"
# One command per 100 cycles (command k runs at cycle 100k + 1): 65 the mage two tiles north; 66 Talk-to; 67 Teleport;
# 68 Mine the rock of slot 1.
flow_client makeabyss notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout \
  notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout \
  notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout \
  notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout \
  notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout \
  notimeout notimeout notimeout notimeout "npcadd 2257 0 2" "opnpc 1 2257" "opnpc 4 2257" "oploc 1 3026 4813 7143" notimeout notimeout notimeout
flow_finish "$CYCLES" "$HERE"
