#!/bin/sh
# Regenerate the Grand Exchange session-replay fixture (scenario_exchange):
#   tools/client910/fixtures/session-replay/exchange/record.sh
# Needs the generated exchange table (`npm --prefix server run data:generate -- exchange`).
# One real client, standing at the development spawn (3222, 3222) with ten logs
# in its backpack (a written save), opens the Grand Exchange from a clerk added
# beside it (the clerk's Exchange option: the hero window's Grand Exchange tab,
# interfaces 105, 107, 651 and 1666), and sells the logs through the real
# window, as UI operations (CLIENT910_UI_OPERATIONS=cycle,component,child,option):
# - SELL_CYCLE: slot 0's Sell button (105:37);
# - OFFER_CYCLE: the logs' Offer in the backpack beside the window (107:7, child 0):
#   the server sets the offer up (item, quantity 10, the guide price);
# - CONFIRM_CYCLE: Confirm Offer (105:302): the logs leave the backpack and the
#   offer row arrives (UPDATE_STOCKMARKET_SLOT);
# - VIEW_CYCLE: slot 0's box, option 1: the offer's progress screen;
# - COLLECT_CYCLE: the coins' Collect (105:284), once the offer has completed.
# The dev server runs with the DEV-ONLY ALTO_GE_MARKET_MAKER=3: a market maker
# buys three logs a sweep (every five ticks) at the offer's price, so the row
# goes 3, 6, 9 then 10 of 10 while the progress screen is open. Server commands,
# one per 100 cycles from cycle 101: notimeout, `npcadd <clerk> 1 0`, notimeout,
# `opnpc 1 <clerk>` (the clerk's Exchange), then notimeout.
# With SHOTS_DIR set it saves a screenshot at each SHOTS cycle and stops at the
# last (client-only; give CYCLES the last shot's cycle then).
# Env: the *_CYCLE values, CYCLES (2000), SHOTS_DIR, SHOTS, KEEP_WORK,
# LOBBY_PORT (47440), WORLD_PORT (47441), CARGO_TARGET_DIR.
LOBBY_PORT=${LOBBY_PORT:-47440}
WORLD_PORT=${WORLD_PORT:-47441}
export LOBBY_PORT WORLD_PORT
. "$(dirname "$0")/../flow-lib.sh"
export ALTO_GE_MARKET_MAKER=3
export MAX_SECONDS=${MAX_SECONDS:-180}
CYCLES=${CYCLES:-2000}
SELL_CYCLE=${SELL_CYCLE:-650}
OFFER_CYCLE=${OFFER_CYCLE:-720}
CONFIRM_CYCLE=${CONFIRM_CYCLE:-800}
VIEW_CYCLE=${VIEW_CYCLE:-880}
COLLECT_CYCLE=${COLLECT_CYCLE:-1700}
# The Grand Exchange clerk (NPC 2241, "Grand Exchange clerk").
CLERK=2241
flow_start
# The character's save (account key = the lower-cased login name): ten logs (obj 1511).
python3 - "$WORK/players" <<'PY'
import hashlib, json, os, sys
key = 'exchange'
wear = [-1] * 19
for slot, kit in [(4, 19), (6, 27), (7, 37), (8, 1), (9, 34), (10, 43), (11, 11)]:
    wear[slot] = -2147483648 | kit
backpack = [[1511, 1]] * 10 + [[-1, 0]] * 18
save = {'version': 2, 'accountKey': key, 'x': 3222, 'z': 3222, 'level': 0, 'backpack': backpack,
        'worn': [[-1, 0]] * 19, 'identityAppearance': {'wear': wear, 'colours': [1, 2, 3, 4, 0, 0, 0, 0, 0, 0], 'textures': [0] * 10}}
path = os.path.join(sys.argv[1], 'accounts', hashlib.sha256(key.encode()).hexdigest() + '.json')
os.makedirs(os.path.dirname(path), exist_ok=True)
json.dump(save, open(path, 'w'))
PY
export CLIENT910_RECORD="$WORK/raw.rtr"
SELL=$((105 << 16 | 37))
SIDE=$((107 << 16 | 7))
CONFIRM=$((105 << 16 | 302))
BOX=$((105 << 16 | 24))
COLLECT_COINS=$((105 << 16 | 284))
export CLIENT910_UI_OPERATIONS="${OPERATIONS:-$SELL_CYCLE,$SELL,-1,1;$OFFER_CYCLE,$SIDE,0,1;$CONFIRM_CYCLE,$CONFIRM,-1,1;$VIEW_CYCLE,$BOX,-1,1;$COLLECT_CYCLE,$COLLECT_COINS,-1,1}"
export CLIENT_ARGS=""
if [ -n "${SHOTS_DIR:-}" ]; then
  mkdir -p "$SHOTS_DIR"
  export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-600,760,840,1000,1300,1600,1800}
  CLIENT_ARGS="--screenshot $SHOTS_DIR/exchange.png"
fi
flow_client exchange notimeout "npcadd $CLERK 1 0" notimeout "opnpc 1 $CLERK" notimeout notimeout notimeout notimeout notimeout notimeout \
  notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout notimeout
flow_finish "$CYCLES" "$HERE"
