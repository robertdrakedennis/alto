#!/bin/sh
# Regenerate the trade session-replay fixture (scenario_inventory::trade_*):
#   tools/client910/fixtures/session-replay/trade/record.sh
# Two real clients against one dev server; the recorded one is "alice", "bob"
# is a second client with its own scripted inputs (not recorded). Both run
# notimeout, invtest and ask the other to trade twice ("Trade with": the
# second request finds the first and opens the window). Operations
# (cycle,component,child,option): both offer from the trade side panel (336:0),
#   alice 800 (slot 2, Offer) and 840 (slot 3, Offer-5); bob 820 (slot 4, Offer-All);
#   accept (335:48): alice 1100, bob 1140; confirm accept (334:44): alice 1400, bob 1440.
# The two clients' logic clocks drift by a few tens of cycles at most, so every
# stage is spaced by more than 200 cycles. The trace is kept below cycle 1550.
. "$(dirname "$0")/../flow-lib.sh"
flow_start
SIDE=$((336 << 16)); ACCEPT=$((335 << 16 | 48)); CONFIRM=$((334 << 16 | 44))
export CLIENT910_RECORD="$WORK/raw.rtr"
export CLIENT910_UI_OPERATIONS="800,$SIDE,2,1;840,$SIDE,3,2;1100,$ACCEPT,-1,1;1400,$CONFIRM,-1,1"
flow_client alice notimeout invtest "trade bob" "trade bob"
unset CLIENT910_RECORD
export CLIENT910_UI_OPERATIONS="820,$SIDE,4,4;1140,$ACCEPT,-1,1;1440,$CONFIRM,-1,1"
flow_client bob notimeout invtest "trade alice" "trade alice"
flow_finish 1550 "$HERE"
