#!/usr/bin/env python3
"""Small ordinary-client encounter route composed from the existing controller."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import time

HERE = Path(__file__).resolve().parent
SHARED = HERE.parent / "barrows" / "driver.py"
SHARED_SHA256 = "d5377a3458af568e527852b9b87d3c529c09a168428dcd363a36c19e85d38508"
ONE = 1
ZERO = 0
NEAR_LADDER_TILES = 1
LOOT_APPROACH_TILES = 1
LOOT_SCAN_RADIUS = 6
LOOT_PUBLICATION_SECONDS = 12
POLL_SECONDS = 0.2
MAX_CLIENT_SECONDS = 600
DEFAULT_ACTION_SECONDS = 40
DEFAULT_FIGHT_SECONDS = 240
DEFAULT_SOCKET_SECONDS = 10


def module(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


if hashlib.sha256(SHARED.read_bytes()).hexdigest() != SHARED_SHA256:
    raise RuntimeError("Shared ordinary input helper changed")
shared = module(SHARED, "boss_observed_helpers")


class Driver(shared.Driver):
    def __init__(self, control, args):
        self.control, self.args = control, args
        self.plan = shared.load_json(args.plan)
        self.food_rules = shared.load_json(args.food_rules)
        self.symbols = shared.Symbols(args.symbols)
        self.receipts = shared.Receipts(args.receipts)
        self.started = time.monotonic()
        self.deadline = self.started + args.deadline_seconds
        self.requests = self.actions = self.walk_probes = ZERO
        self.state = self.next_food_tick = None
        self.last_scans, self.food_receipts = {}, []
        self.stage = "ready"
        self.journal = Path(args.journal).open("x", encoding="utf8")
        self.query = {
            "varbits": [self.symbols.get("varbit", name) for name in
                ("current_life_points", "current_prayer_points", "legacy_combat_active")]
                + [row["activationBit"] for row in self.plan["prayers"]],
            "inventories": [self.symbols.get("inv", name) for name in
                ("backpack", "worn_equipment", "coin_wallet")],
            "components": [self.symbols.component(name) for name in
                ("window_buttons.actions", "backpack.slots", "prayer_book.prayer_buttons")],
            "client_varbits": [self.symbols.get("varbit", "legacy_selected_window")],
        }
        self.log("start", {"planSha256": shared.digest(args.plan),
            "sharedSha256": SHARED_SHA256, "policy": "normal input and passive observation only"})

    def prayers(self):
        self.open_window(self.plan["toolbar"]["prayerDestination"],
                         "prayer_book.prayer_buttons", "Prayer")
        for rule in self.plan["prayers"]:
            state = self.observe()
            if self.bit(state, rule["activationBit"]) == ONE:
                continue
            parent = self.symbols.get("component", "prayer_book.prayer_buttons")
            rows = self.children(self.symbols.component("prayer_book.prayer_buttons"))
            rows = [row for row in rows if row["target"] == {"parent": parent, "child": rule["button"]}
                    and (row.get("value") or {}).get("rooted_visible")]
            if len(rows) != ONE:
                raise shared.Stop("Native prayer control is not uniquely loaded")
            row = rows[ZERO]
            wanted = shared.normal_text("Activate " + rule["name"])
            choices = [text for text in row["value"].get("ops", []) if text
                and shared.normal_text(re.sub(r"</?col(?:=[^>]*)?>", "", text,
                    flags=re.IGNORECASE)) == wanted]
            after = self.act(self.ui_action(row, choices), state)
            self.wait("native prayer publication", lambda current:
                self.bit(current, rule["activationBit"]) == ONE, after)

    def entry(self, rejoin=False):
        self.stage = "rejoin" if rejoin else "enter"
        room = self.plan["room"]
        self.walk(room["exit"], radius=NEAR_LADDER_TILES)
        row, label = self.select_loc({room["entryLoc"]}, room["entryTile"], ("Climb-down",))
        before = self.observe()
        after = self.act(self.loc_action(row, label))
        self.wait("native public map and actual membership", lambda state:
            state.get("region") is None
            and state["map"]["terrain_generation"] != before["map"]["terrain_generation"]
            and shared.distance(state["player"], room["entrance"]) <= NEAR_LADDER_TILES
            and self.receipts.player().get("room") == room["id"] + "-public",
            after, supervise_food=True)
        self.log(self.stage, {"state": self.state, "passive": self.receipts.player()})

    def scanned_action(self, kind, row, operation):
        scan = self.last_scans["scan_" + ("npcs" if kind == "npc" else "objects")]["response"]
        keys = ("index", "definition", "update_serial") if kind == "npc" else (
            "definition", "x", "z", "level", "stack_index", "count", "object_revision")
        if row not in scan["data"]["scan"]["entries"]:
            raise shared.Stop("Selected scene entity is not the current scanner row")
        self.actions += ONE
        return self.request({"command": "action", "map": scan["data"]["map"],
            "observed_cycle": scan["cycle"], "action": {"kind": kind,
                **{key: row[key] for key in keys}, "operation": operation}})["cycle"]

    def fight(self):
        self.stage = "fight"
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
        def died(state):
            return any(receipt.get("kind") == "death" and receipt["target"]["id"] == row["index"]
                and receipt["target"]["generation"] == generation
                and receipt["source"]["id"] == passive["pid"]
                for receipt in self.receipts.rows[offset:])
        self.wait("ordinary boss death", died, after, supervise_food=True, deadline=deadline)
        landed = [receipt for receipt in self.receipts.rows[offset:] if receipt.get("kind") == "landed"
            and receipt["target"]["id"] == row["index"] and receipt["source"]["id"] == passive["pid"]]
        if not landed or not any(receipt["actualDamage"] > ZERO for receipt in landed):
            raise shared.Stop("Death has no ordinary player-hit receipts")
        self.log("boss_death", {"fullLife": full[ZERO], "landed": landed,
            "player": self.receipts.player(), "cycle": self.state["cycle"]})

    def loot(self):
        self.stage = "loot"
        item = self.plan["loot"]
        state = self.observe()
        before = self.count(self.inventory(state, "backpack"), item)
        deadline = min(self.deadline, time.monotonic() + LOOT_PUBLICATION_SECONDS)
        rows = []
        while time.monotonic() < deadline:
            state = self.observe()
            self.eat_if_needed(state)
            rows, _ = self.scan("scan_objects", {"definition": item,
                "x": state["player"]["x"], "z": state["player"]["z"],
                "level": state["player"]["level"], "radius": LOOT_SCAN_RADIUS})
            if rows:
                break
            time.sleep(POLL_SECONDS)
        if not rows:
            raise shared.Stop("Dated guaranteed boss loot was not published")
        row = min(rows, key=lambda actual: shared.distance(actual, state["player"]))
        self.walk(row, radius=LOOT_APPROACH_TILES)
        rows, _ = self.scan("scan_objects", {"definition": item, "x": row["x"], "z": row["z"], "radius": ZERO})
        if len(rows) != ONE:
            raise shared.Stop("Selected native loot stack changed")
        after = self.scanned_action("object", rows[ZERO], "Take")
        self.wait("normal owned loot transfer", lambda current:
            self.count(self.inventory(current, "backpack"), item) > before, after, supervise_food=True)
        self.log("loot", {"item": item, "state": self.state, "passive": self.receipts.player()})

    def leave(self):
        self.stage = "leave"
        room = self.plan["room"]
        state = self.observe()
        destination = (shared.Layout(state).resolve(room["exitTile"])
            if state.get("region") is not None else room["exitTile"])
        self.walk(destination, radius=NEAR_LADDER_TILES)
        row, label = self.select_loc({room["exitLoc"]}, destination, ("Climb-up",))
        after = self.act(self.loc_action(row, label))
        self.wait("native ladder exit and normal rebuild", lambda state:
            self.receipts.player().get("instance") is None
            and shared.distance(state["player"], room["exit"]) == ZERO, after, supervise_food=True)
        self.log("leave", {"state": self.state, "passive": self.receipts.player()})

    def run(self):
        self.observe()
        self.equip()
        self.prayers()
        self.entry()
        original = self.receipts.player()["instance"]
        self.fight()
        self.loot()
        self.leave()
        self.entry(rejoin=True)
        if self.receipts.player()["instance"] != original:
            raise shared.Stop("Rejoin replaced the original instance")
        self.log("complete", {"status": "ordinary_encounter_pass", "instance": original,
            "actions": self.actions, "foods": self.food_receipts, "final": self.state})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("plan", "food-rules", "symbols", "receipts", "journal", "control-module", "socket"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--deadline-seconds", type=int, default=MAX_CLIENT_SECONDS)
    parser.add_argument("--action-seconds", type=int, default=DEFAULT_ACTION_SECONDS)
    parser.add_argument("--fight-seconds", type=int, default=DEFAULT_FIGHT_SECONDS)
    parser.add_argument("--socket-seconds", type=int, default=DEFAULT_SOCKET_SECONDS)
    args = parser.parse_args()
    if not ZERO < args.deadline_seconds <= MAX_CLIENT_SECONDS:
        parser.error("Finite encounter budget required")
    control = module(Path(args.control_module), "boss_normal_control").Control(args.socket, args.socket_seconds)
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
