#!/bin/sh
# Regenerate the bank session-replay fixture (scenario_inventory::bank_*):
#   tools/client910/fixtures/session-replay/bank/record.sh
# One real client against the dev server, no window input (the recording drops
# operating-system input). Server commands (one per 100 cycles): notimeout,
# invtest (a backpack of seven kinds of item), bankfill (seven bank rows),
# openbank (the window a banker opens). Scripted UI operations
# (CLIENT910_UI_OPERATIONS=cycle,component,child,option, IF_BUTTONn):
#   700 deposit one of backpack slot 2       740 deposit-all of slot 3
#   780 withdraw-5 of bank row 3             820 withdraw-X prompt on row 4,
#   850 typed "1000"                         900 note toggle (517:120)
#   940 withdraw-all of row 6 (as a note)    980 placeholder toggle (517:116)
#  1020 withdraw-all of row 5 (leaves a placeholder)
#  1100..1150 (CLIENT910_UI_INPUT): press bank row 1, drag it up onto the
#  "new tab" button and release: IF_BUTTOND, a tab is created.
# The trace is kept below cycle 1250.
. "$(dirname "$0")/../flow-lib.sh"
flow_start
BACKPACK=$((517 << 16 | 14)); ROWS=$((517 << 16 | 184)); NOTE=$((517 << 16 | 120)); PLACE=$((517 << 16 | 116))
GESTURE="1100,238,163,1,0"
for i in 1 2 3 4 5 6 7 8 9 10; do GESTURE="$GESTURE;$((1100 + i * 4)),238,$((163 - i * 4)),1,-1"; done
GESTURE="$GESTURE;1146,237,121,1,-1;1150,237,121,0,3"
export CLIENT910_RECORD="$WORK/raw.rtr"
export CLIENT910_UI_OPERATIONS="700,$BACKPACK,2,1;740,$BACKPACK,3,7;780,$ROWS,3,3;820,$ROWS,4,6;900,$NOTE,-1,1;940,$ROWS,6,7;980,$PLACE,-1,1;1020,$ROWS,5,7"
export CLIENT910_TYPE_INPUT='850:1000\n'
export CLIENT910_UI_INPUT="$GESTURE"
flow_client banker notimeout invtest bankfill openbank
flow_finish 1250 "$HERE"
