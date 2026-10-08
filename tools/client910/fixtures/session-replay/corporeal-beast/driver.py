#!/usr/bin/env python3
"""Finite Corp route through normal native controls; never assists live state."""
import argparse
import hashlib
import importlib.util
from pathlib import Path
import time

HERE = Path(__file__).resolve().parent
BASE = HERE.parent / "boss-encounters" / "driver.py"
BASE_SHA256 = "ddcf83c88bc4b1c758c9a67dd06d2959afc6ef9e4bd5647e4e38c8b1098cc0ab"
ZERO = 0
ONE = 1
NEAR_PASSAGE_TILES = 1
LOOT_SCAN_RADIUS = 6
LOOT_PUBLICATION_SECONDS = 30
CORE_STOP_OBSERVATION_SECONDS = 5
POLL_SECONDS = 0.2
FIGHT_SECONDS = 120
CLIENT_SECONDS = 360
ACTION_SECONDS = 40
SOCKET_SECONDS = 10
EXIT_REJOIN_RESERVE_SECONDS = 140

def module(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value

if hashlib.sha256(BASE.read_bytes()).hexdigest() != BASE_SHA256:
    raise RuntimeError("Hash-bound ordinary encounter helper changed; rebind after adaptation")
base = module(BASE, "corp_ordinary_encounter_helpers")
shared = base.shared
NEXT_ENTRY = shared.NEXT_ENTRY
FIRST_SLOT = shared.FIRST_SLOT
FOOD_SAFETY_NUMERATOR = shared.FOOD_SAFETY_NUMERATOR
FOOD_SAFETY_DENOMINATOR = shared.FOOD_SAFETY_DENOMINATOR

class Driver(base.Driver):
    def entry(self, rejoin=False):
        self.stage = "rejoin" if rejoin else "enter"
        room = self.plan["room"]
        self.walk(room["exit"], radius=NEAR_PASSAGE_TILES)
        row, label = self.select_loc({room["entryLoc"]}, room["entryTile"], ("Go-through",))
        before = self.observe()
        passive_before = dict(self.receipts.player())
        if passive_before["instance"] is not None or before["player"]["x"] >= row["x"]:
            raise shared.Stop("Entry requires observed nonmember on west side of actual native passage")
        after = self.act(self.loc_action(row, label), before)
        self.wait("actual public membership and native east arrival", lambda state:
            state.get("region") is None
            and self.receipts.player().get("room") == self.plan["publicRoom"]
            and self.receipts.player().get("membership") is True
            and shared.distance(state["player"], room["entrance"]) <= NEAR_PASSAGE_TILES
            and state["player"]["x"] > row["x"], after, supervise_food=True)
        self.log(self.stage, {"state": self.state, "passive": self.receipts.player(),
            "source": passive_before, "loc": row, "beforeMap": before["map"],
            "mapRequirement": "actual native arrival and membership; same-square terrain generation need not change"})

    def sample(self):
        self.stage = "ordinary_melee_sample"
        target = self.plan["target"]
        deadline = min(self.deadline, time.monotonic() + self.args.fight_seconds)
        row = None
        while time.monotonic() < deadline:
            state = self.observe()
            rows, _ = self.scan("scan_npcs", {"definition": target["npc"], "level": state["player"]["level"]})
            if len(rows) == ONE:
                row = rows[ZERO]
                break
            time.sleep(POLL_SECONDS)
        if row is None:
            raise shared.Stop("Timed native inhabitant did not appear")
        self.receipts.refresh()
        passive = self.receipts.player()
        full = [enemy for receipt in self.receipts.rows if receipt.get("kind") == "state"
            for enemy in receipt.get("enemies", []) if enemy["id"] == row["index"]
            and enemy["definition"] == target["npc"] and enemy["instance"] == passive["instance"]
            and enemy["hitpoints"] == target["profile"]["hitpoints"]]
        if not full:
            raise shared.Stop("Independent full-health boss publication is absent")
        generation = full[ZERO]["generation"]
        offset = len(self.receipts.rows)
        rows, fight_map = self.scan("scan_npcs", {"definition": target["npc"], "level": row["level"]})
        rows = [current for current in rows if current["index"] == row["index"]]
        if len(rows) != ONE:
            raise shared.Stop("Intended boss left the installed scanner before Attack")
        row = rows[ZERO]
        _, operation = self.operation(row, ("Attack",))
        try:
            after = self.scanned_action("npc", row, operation)
        except shared.UnacceptedNpcIntent as refusal:
            # Native admission refused this intent before recording an input.
            # Refresh the same live actor once; accepted attacks never repeat.
            self.actions -= ONE
            state = self.observe()
            rows, stamp = self.scan("scan_npcs", {"definition": target["npc"], "level": row["level"]})
            matching = [current for current in rows if current["index"] == row["index"]
                and current["definition"] == row["definition"]]
            current_player = self.receipts.player()
            lives = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
                if enemy["definition"] == row["definition"] and enemy["id"] == row["index"]
                and enemy["generation"] == generation and enemy["instance"] == passive["instance"]
                and enemy.get("visible") and enemy["hitpoints"] > ZERO]
            if (stamp != fight_map or state["map"] != fight_map or len(matching) != ONE
                    or len(lives) != ONE or current_player["instance"] != passive["instance"]):
                raise shared.Stop("Same live boss was not re-observed after refused intent") from refusal
            row = matching[ZERO]
            _, operation = self.operation(row, ("Attack",))
            self.log("unaccepted_attack_refreshed", {"refusal": refusal.response,
                "actor": row, "map": stamp, "passive_generation": generation,
                "maximum_refreshes": ONE})
            after = self.scanned_action("npc", row, operation)
        while time.monotonic() < deadline:
            state = self.observe()
            self.eat_if_needed(state)
            current = self.receipts.player()
            if current["life"] <= ZERO or current["instance"] != passive["instance"]:
                raise shared.Stop("Ordinary sample died or left the admitted life; no assistance")
            time.sleep(POLL_SECONDS)
        self.observe()
        self.receipts.refresh()
        publications = [receipt for receipt in self.receipts.rows
            if receipt.get("kind") == "native_publication"
            and any(enemy["id"] == row["index"] and enemy["generation"] == generation
                and enemy["actorToken"] == full[ZERO]["actorToken"] for enemy in receipt["enemies"])]
        if not publications:
            raise shared.Stop("No actual before-flush native sample publication")
        final = publications[-ONE]
        body = [enemy for enemy in final["enemies"] if enemy["id"] == row["index"]
            and enemy["generation"] == generation and enemy["actorToken"] == full[ZERO]["actorToken"]]
        if len(body) != ONE or not ZERO < body[ZERO]["hitpoints"] < full[ZERO]["maximumLife"]:
            raise shared.Stop("Sample requires the original ordinarily damaged live full-health body")
        self.log("ordinary_melee_sample", {"fullLife": full[ZERO], "finalBody": body[ZERO],
            "throughTick": final["tick"], "player": self.receipts.player(),
            "cycle": self.state["cycle"], "scope": "Bounded ordinary sample; full kill and boss loot unclaimed"})

    def eat_if_needed(self, state, minimum_life=None):
        life = self.bit(state, "current_life_points")
        # Capacity is observed from the independent account receipt, not assigned.
        passive = self.receipts.player()
        maximum = passive["maximumLife"]
        if life is None:
            return False
        if minimum_life is not None:
            if isinstance(minimum_life, bool) or not isinstance(minimum_life, int) or not 0 < minimum_life < maximum:
                raise shared.Stop("Declared ordinary food threshold is outside observed life capacity")
            needs_food = life < minimum_life
        else:
            needs_food = life * FOOD_SAFETY_DENOMINATOR < maximum * FOOD_SAFETY_NUMERATOR
        if not needs_food:
            return False
        observed_tick = self.receipts.last_state["tick"]
        if self.next_food_tick is not None and observed_tick < self.next_food_tick:
            return False
        item = self.plan["food"]["item"]
        facts = [food for food in self.food_rules["foods"] if food["item"] == item]
        if len(facts) != NEXT_ENTRY:
            raise shared.Stop("ordinary selected food row is not uniquely generated")
        state = self.open_backpack()
        before = self.inventory(state, "backpack")
        total = self.count(before, item)
        if total <= 0:
            raise shared.Stop("food exhausted while injured; no fixture heal permitted")
        row = self.item_control(state, "backpack.slots", item, ("Eat",))
        passive_before = dict(passive)
        tick_before = observed_tick
        offset = len(self.receipts.rows)
        after = self.act(self.ui_action(row, ("Eat",), item), state)
        result = self.wait("selected native food consumption/LP publication", lambda s:
            self.count(self.inventory(s, "backpack"), item) == total - NEXT_ENTRY
            and s["packets_applied"] > state["packets_applied"]
            and any(receipt.get("kind") == "food_commit" and receipt.get("item") == item
                    and receipt.get("pid") == passive_before["pid"] and receipt.get("healed", 0) > 0
                    for receipt in self.receipts.rows[offset:]), after)
        self.receipts.refresh()
        rows = [receipt for receipt in self.receipts.rows[offset:] if receipt.get("kind") == "state"]
        evidence = []
        for receipt in rows:
            for player in receipt["players"]:
                count = sum(slot[NEXT_ENTRY] for slot in player["backpack"] if slot and slot[FIRST_SLOT] == item)
                if count == total - NEXT_ENTRY:
                    evidence.append({"tick": receipt["tick"], "life": player["life"],
                                     "food_count": count, "regeneration": player["regeneration"]})
        commits = [receipt for receipt in self.receipts.rows[offset:]
                   if receipt.get("kind") == "food_commit" and receipt.get("item") == item
                   and receipt.get("pid") == passive_before["pid"]]
        if not evidence or len(commits) != NEXT_ENTRY or commits[FIRST_SLOT].get("healed", 0) <= 0:
            raise shared.Stop("native food publication lacks passive saved-account consumption receipt")
        commit = commits[FIRST_SLOT]
        if commit.get("beforeItem") != item or commit.get("beforeCount") != NEXT_ENTRY or commit.get("afterCount") != 0:
            raise shared.Stop("ordinary food commit does not consume exactly one observed singleton")
        if commit["healed"] != commit["lifeAfter"] - commit["lifeBefore"] or commit["healed"] > facts[FIRST_SLOT]["maximumHealing"]:
            raise shared.Stop("ordinary food commit healing is inconsistent with qualified food cap")
        accepted_tick = commits[FIRST_SLOT]["tick"]
        self.next_food_tick = accepted_tick + self.food_rules["rules"]["consumptionTicks"]
        record = {"target": row["target"], "item": item, "before_life": life,
            "after_life": self.bit(result, "current_life_points"),
            "server_before": {"life": passive_before["life"], "tick": tick_before},
            "server_rows": evidence, "food_commit": commits[FIRST_SLOT],
            "meaning": "Synchronous ordinary food commit supplies healing before concurrent damage; net client LP need not rise"}
        self.food_receipts.append(record)
        self.log("food", record)
        return True


    def leave(self):
        self.stage = "leave"
        room = self.plan["room"]
        # East approach is named native data. Walking to the west blocked leaf
        # would destroy the directional evidence before the operation arrives.
        self.walk(room["entrance"], radius=NEAR_PASSAGE_TILES)
        row, label = self.select_loc({room["exitLoc"]}, room["exitTile"], ("Go-through",))
        before = self.observe()
        passive_before = dict(self.receipts.player())
        if not passive_before.get("membership") or before["player"]["x"] <= row["x"]:
            raise shared.Stop("Exit requires observed member on east side of actual native passage")
        after = self.act(self.loc_action(row, label), before)
        self.wait("ordinary passage exit", lambda state:
            self.receipts.player().get("instance") is None
            and not self.receipts.player().get("membership")
            and shared.distance(state["player"], room["exit"]) == ZERO
            and state["player"]["x"] < row["x"], after, supervise_food=True)
        self.log("leave", {"state": self.state, "passive": self.receipts.player(),
            "source": passive_before, "loc": row, "beforeMap": before["map"]})

    def run(self):
        self.observe()
        self.equip()
        self.prayers()
        self.entry()
        original = self.receipts.player()["instance"]
        self.sample()
        desired = []
        if self.deadline - time.monotonic() >= EXIT_REJOIN_RESERVE_SECONDS:
            self.leave()
            self.entry(rejoin=True)
            if self.receipts.player()["instance"] != original:
                raise shared.Stop("Ordinary public rejoin replaced the original instance")
            desired = ["leave", "rejoin"]
        else:
            self.log("scope_narrowed", {"omitted": ["leave", "rejoin"],
                "reason": "Fixed remaining native client budget below exit/rejoin reserve",
                "remainingSeconds": self.deadline - time.monotonic(),
                "socketLifecycleRequired": True})
        self.log("complete", {"status": "ordinary_melee_sample_pass", "instance": original,
            "scope": {"required": self.plan["recordingScope"]["required"], "completedDesired": desired,
                "core": "Natural observations only; hard core matrix remains socket work"},
            "actions": self.actions, "foods": self.food_receipts, "final": self.state})

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("plan", "food-rules", "symbols", "receipts", "journal", "control-module", "socket"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--deadline-seconds", type=int, default=CLIENT_SECONDS)
    parser.add_argument("--action-seconds", type=int, default=ACTION_SECONDS)
    parser.add_argument("--fight-seconds", type=int, default=FIGHT_SECONDS)
    parser.add_argument("--socket-seconds", type=int, default=SOCKET_SECONDS)
    args = parser.parse_args()
    if not ZERO < args.fight_seconds <= FIGHT_SECONDS or not args.fight_seconds < args.deadline_seconds <= CLIENT_SECONDS:
        parser.error("Finite ordinary full-health initial Corp sample budget required")
    control = module(Path(args.control_module), "corp_normal_control").Control(args.socket, args.socket_seconds)
    driver = None
    try:
        driver = Driver(control, args)
        driver.run()
    finally:
        if driver is not None:
            driver.journal.close()
        control.close()

if __name__ == "__main__":
    main()
