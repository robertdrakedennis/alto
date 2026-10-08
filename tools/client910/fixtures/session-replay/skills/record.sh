#!/bin/sh
# Regenerate the skills session-replay fixture (scenario_skills):
#   tools/client910/fixtures/session-replay/skills/record.sh
# One real client mines copper at the Lumbridge Swamp mine, then cooks shrimps
# through the Make-X interface (window 1007, product list 1371 in 1370:3).
# The dev server runs with the TEST-ONLY ALTO_SKILLS_ALWAYS=1 (every success
# roll hits, nothing burns: World.loadContent) and the character is placed by a
# written save (a teleport would rebuild the map mid-session, which the replay
# does not install):
# - the save puts the player at (3229, 3150) with four raw shrimps in four
#   backpack slots; the client runs the default (modern) renderer;
# - server commands (one per 100 cycles): notimeout, `locadd 2732 10 0 1 0`
#   (a fire east of the player), ten more notimeout to wait out the mining,
#   `makex 317` (stop and use the raw shrimps on the nearest fire: opens Make-X);
# - the mouse on the copper rock from cycle 400, a left click on it at cycle 450
#   (OPLOC1 "Mine"), and a left click on the Make-X "Cook" button at cycle 1450
#   (RESUME_PAUSEBUTTON).
# Screenshots (SHOTS, client-only) are for checking the points. The trace is
# kept below cycle 2150. Env: ROCK_CLICK (665,579), COOK_CLICK (622,524),
# KEEP_WORK, SHOTS_DIR (where screenshots go; none when unset), CARGO_TARGET_DIR.
. "$(dirname "$0")/../flow-lib.sh"
export ALTO_SKILLS_ALWAYS=1
ROCK_CLICK=${ROCK_CLICK:-665,579}
COOK_CLICK=${COOK_CLICK:-622,524}
flow_start
# The character's save (account key = the lower-cased login name).
python3 - "$WORK/players" <<'PY'
import hashlib, json, os, sys
key = 'mining'
wear = [-1] * 19
for slot, kit in [(4, 19), (6, 27), (7, 37), (8, 1), (9, 34), (10, 43), (11, 11)]:
    wear[slot] = -2147483648 | kit
backpack = [[317, 1]] * 4 + [[-1, 0]] * 24
save = {'version': 2, 'accountKey': key, 'x': 3229, 'z': 3150, 'level': 0, 'backpack': backpack,
        'worn': [[-1, 0]] * 19, 'identityAppearance': {'wear': wear, 'colours': [1, 2, 3, 4, 0, 0, 0, 0, 0, 0], 'textures': [0] * 10}}
path = os.path.join(sys.argv[1], 'accounts', hashlib.sha256(key.encode()).hexdigest() + '.json')
os.makedirs(os.path.dirname(path), exist_ok=True)
json.dump(save, open(path, 'w'))
PY
export CLIENT910_RECORD="$WORK/raw.rtr"
export CLIENT910_UI_HOVER="$ROCK_CLICK,400,449"
export CLIENT910_UI_CLICKS="l,$ROCK_CLICK,450;l,$COOK_CLICK,1450"
export CLIENT_ARGS=""
if [ -n "${SHOTS_DIR:-}" ]; then
  mkdir -p "$SHOTS_DIR"
  export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-470,900,1400,2000,2200}
  CLIENT_ARGS="--screenshot $SHOTS_DIR/skills.png"
fi
flow_client mining notimeout "locadd 2732 10 0 1 0" notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout "makex 317"
flow_finish 2150 "$HERE"
