#!/bin/sh
# Regenerate the player-owned house session-replay fixture (scenario_house):
#   tools/client910/fixtures/session-replay/house/record.sh
# One real client, standing at the Taverley house portal (location
# taverley_house_portal_exit, 2883 3453), with coins, two planks' worth of a
# chair, steel nails, a hammer and a saw, and a written save that already owns
# a house in Taverley in building mode: a garden (its exit portal built) in the
# middle of the grid and a parlour north of it (Construction's PLACEHOLDER
# starter layout; the estate agent's sale is the server E2E's).
# - The portal's "Enter building mode" (op 3): the house is laid as an
#   instanced region (REBUILD_REGION), the client fetches the template map
#   squares and installs it (a mid-session map transaction).
# - The garden's south doorway's Build (op 5): the room menu (interface 402);
#   its Parlour row's Build (402:72, a UI operation): a parlour south of the
#   garden, turned a quarter so its west doorway faces the garden (a second
#   REBUILD_REGION).
# - The new parlour's first chair space's Build: the furniture menu (1306 over
#   inventory 398); its first option's Build (the pause button the menu's
#   script makes as child 4 of 1306:6, a UI operation with op 0) builds a crude
#   wooden chair.
# Loc options are server commands (`oploc`, as the client's menu sends them):
# the headless replay picks no scene locs. Commands wait for each map
# transaction (they resume 100 cycles after it is installed), so the room
# menu's and the furniture menu's UI operations are timed from the recorded
# installs (MAPI):
# set ROOM_CYCLE and CHAIR_CYCLE from a first run's raw.rtr when they drift.
# With SHOTS_DIR set it saves a screenshot at each SHOTS cycle and stops at the
# last (client-only; give CYCLES the last shot's cycle then).
# Env: LOBBY_PORT, WORLD_PORT (47440 and 47441), CYCLES (1050), ROOM_CYCLE
# (420), CHAIR_CYCLE (760), SHOTS_DIR, SHOTS, KEEP_WORK, CARGO_TARGET_DIR.
# The ports are set before flow-lib.sh, which keeps any already set.
LOBBY_PORT=${LOBBY_PORT:-47440}
WORLD_PORT=${WORLD_PORT:-47441}
export LOBBY_PORT WORLD_PORT
. "$(dirname "$0")/../flow-lib.sh"
export MAX_SECONDS=${MAX_SECONDS:-240}
CYCLES=${CYCLES:-1050}
ROOM_CYCLE=${ROOM_CYCLE:-420}
CHAIR_CYCLE=${CHAIR_CYCLE:-760}
flow_start
# The character's save (account key = the lower-cased login name), with the
# house rooms read from the generated Construction table.
python3 - "$WORK/players" "$ROOT/server/data/generated/skills/construction.json" <<'PY'
import hashlib, json, os, sys
key = 'builder'
table = json.load(open(sys.argv[2]))
room = {r['name']: r for r in table['rooms']}
portal = next(f['obj'] for f in table['furniture'] if f['name'] == 'Exit portal')
centre = next(i for i, h in enumerate(room['Garden']['hotspots']) if any(portal in k['furniture'] for k in table['hotspots'] if h['loc'] in k['locs']))
wear = [-1] * 19
for slot, kit in [(4, 19), (6, 27), (7, 37), (8, 1), (9, 34), (10, 43), (11, 11)]:
    wear[slot] = -2147483648 | kit
# Coins, planks, steel nails, a hammer and a saw.
backpack = [[995, 5000], [960, 2], [1539, 2], [2347, 1], [8794, 1]] + [[-1, 0]] * 23
house = {'location': 1, 'buildMode': True, 'rooms': [
    {'room': room['Garden']['obj'], 'x': 3, 'z': 3, 'floor': 1, 'rotation': 0, 'furniture': [{'hotspot': centre, 'obj': portal}]},
    {'room': room['Parlour']['obj'], 'x': 3, 'z': 4, 'floor': 1, 'rotation': 0, 'furniture': []}]}
save = {'version': 2, 'accountKey': key, 'x': 2883, 'z': 3453, 'level': 0, 'backpack': backpack, 'house': house,
        'worn': [[-1, 0]] * 19, 'identityAppearance': {'wear': wear, 'colours': [1, 2, 3, 4, 0, 0, 0, 0, 0, 0], 'textures': [0] * 10}}
path = os.path.join(sys.argv[1], 'accounts', hashlib.sha256(key.encode()).hexdigest() + '.json')
os.makedirs(os.path.dirname(path), exist_ok=True)
json.dump(save, open(path, 'w'))
PY
export CLIENT910_RECORD="$WORK/raw.rtr"
# The room menu's Parlour Build (402:72, op 1) and the furniture menu's first
# option's Build (1306:6 child 4, its pause button).
PARLOUR_BUILD=26345544
FIRST_OPTION=85590022
export CLIENT910_UI_OPERATIONS="${OPERATIONS:-$ROOM_CYCLE,$PARLOUR_BUILD,-1,1;$CHAIR_CYCLE,$FIRST_OPTION,4,0}"
export CLIENT_ARGS=""
if [ -n "${SHOTS_DIR:-}" ]; then
  mkdir -p "$SHOTS_DIR"
  export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-300,440,700,1000}
  CLIENT_ARGS="--screenshot $SHOTS_DIR/house.png"
fi
# One command per 100 cycles once the map is installed (the first at cycle 101;
# a map transaction holds them until 100 cycles after its install):
#  1 notimeout; 2 the Taverley portal's Enter building mode (loc 15477 at 2880 3451);
#  -- the house is installed --
#  3 the garden's south doorway's Build (Door hotspot 15313 at 6467 64; the house's
#    instance is the first block of the free map, its garden chunk at 6464 64); 4 notimeout;
#  -- the room menu's Build at ROOM_CYCLE; the second region is installed --
#  5 the new parlour's chair space's Build (Chair space 15410 at 6468 61); 6-9 notimeout.
flow_client builder notimeout "oploc 3 2880 3451 15477" "oploc 5 6467 64 15313" notimeout \
  "oploc 1 6468 61 15410" notimeout notimeout notimeout notimeout
flow_finish "$CYCLES" "$HERE"
