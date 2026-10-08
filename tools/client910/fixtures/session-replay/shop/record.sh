#!/bin/sh
# Regenerate the shop session-replay fixture (scenario_inventory::shop_*):
#   tools/client910/fixtures/session-replay/shop/record.sh
# One real client, server commands notimeout, invtest, openshop (the General
# Store's window). Scripted UI operations (cycle,component,child,option):
#   700 buy-5 of stock row 0                 760 the Sell tab (1265:32)
#   800 Info on backpack row 5 (selects it)  840 the +5 button (1265:79)
#   880 the Sell button (1265:144)           920 the Buy tab (1265:41)
#   960 buy-1 of stock row 1
# The trace is kept below cycle 1020.
. "$(dirname "$0")/../flow-lib.sh"
flow_start
ROWS=$((1265 << 16 | 20))
export CLIENT910_RECORD="$WORK/raw.rtr"
export CLIENT910_UI_OPERATIONS="700,$ROWS,0,3;760,$((1265 << 16 | 32)),-1,1;800,$ROWS,5,1;840,$((1265 << 16 | 79)),-1,1;880,$((1265 << 16 | 144)),-1,1;920,$((1265 << 16 | 41)),-1,1;960,$ROWS,1,2"
flow_client shopper notimeout invtest openshop
flow_finish 1020 "$HERE"
