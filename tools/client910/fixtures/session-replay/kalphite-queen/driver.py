#!/usr/bin/env python3
"""Full two-form Queen route through ordinary native input."""
import argparse
import hashlib
from pathlib import Path
import time

HERE = Path(__file__).resolve().parent
BOSS = HERE.parent / "boss-encounters" / "driver.py"
BOSS_SHA256 = "ddcf83c88bc4b1c758c9a67dd06d2959afc6ef9e4bd5647e4e38c8b1098cc0ab"
ZERO = 0
ONE = 1
PHYSICAL_INSTANCE_FIELD = 2
ENTRY_DISTANCE_TILES = 1
NPC_SCAN_APPROACH_TILES = 8
POLL_SECONDS = 0.2
MAX_CLIENT_SECONDS = 600


def load_boss():
    import importlib.util
    if hashlib.sha256(BOSS.read_bytes()).hexdigest() != BOSS_SHA256:
        raise RuntimeError("Inherited ordinary encounter driver changed; rebind source first")
    spec = importlib.util.spec_from_file_location("queen_ordinary_boss_driver", BOSS)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


boss = load_boss()
shared = boss.shared


class Driver(boss.Driver):
    def entry(self, rejoin=False):
        self.stage = "rejoin" if rejoin else "enter"
        room = self.plan["room"]
        self.walk(room["exit"], radius=ENTRY_DISTANCE_TILES)
        row, operation = self.select_loc({room["entryLoc"], room["entryParent"]},
            room["entryTile"], ("Quick-climb",))
        if row["definition"] != room["entryLoc"] or row["base_definition"] != room["entryParent"]:
            raise shared.Stop("Native attached upper rope variant is not installed")
        before = self.observe()
        after = self.act(self.loc_action(row, operation))
        self.wait("ordinary public Queen membership and map", lambda state:
            state.get("region") is None
            and state["map"]["terrain_generation"] != before["map"]["terrain_generation"]
            and shared.distance(state["player"], room["entrance"]) <= ENTRY_DISTANCE_TILES
            and self.receipts.player().get("room") == room["id"] + "-public",
            after, supervise_food=True)
        self.log(self.stage, {"state": self.state, "passive": self.receipts.player()})

    def fight_ground(self):
        self.stage = "full_ground_to_flying"
        ground, flying = self.plan["ground"], self.plan["flying"]
        self.walk(self.plan["room"]["spawn"], radius=NPC_SCAN_APPROACH_TILES)
        deadline = min(self.deadline, time.monotonic() + self.args.fight_seconds)
        row = None
        while time.monotonic() < deadline:
            state = self.observe()
            rows, fight_map = self.scan("scan_npcs", {"definition": ground["npc"],
                "level": state["player"]["level"]})
            if len(rows) == ONE:
                row = rows[ZERO]
                break
            time.sleep(POLL_SECONDS)
        if row is None:
            raise shared.Stop("Canonical full first form is absent; do not reset its life")
        player = self.receipts.player()
        full = [enemy for receipt in self.receipts.rows if receipt.get("kind") == "state"
            for enemy in receipt.get("enemies", []) if enemy["id"] == row["index"]
            and enemy["definition"] == ground["npc"] and enemy["instance"] == player["instance"]
            and enemy["hitpoints"] == ground["maximumLife"]]
        if not full:
            raise shared.Stop("Independent full-health first life is absent")
        generation = full[ZERO]["generation"]
        offset = len(self.receipts.rows)
        rows, fight_map = self.scan("scan_npcs", {"definition": ground["npc"], "level": row["level"]})
        matches = [current for current in rows if current["index"] == row["index"]]
        if len(matches) != ONE:
            raise shared.Stop("Intended first-life actor left the installed scene")
        row = matches[ZERO]
        _, operation = self.operation(row, ("Attack",))
        try:
            after = self.scanned_action("npc", row, operation)
        except shared.UnacceptedNpcIntent as refusal:
            self.actions -= ONE
            state = self.observe()
            rows, stamp = self.scan("scan_npcs", {"definition": ground["npc"], "level": row["level"]})
            matches = [current for current in rows if current["index"] == row["index"]
                and current["definition"] == row["definition"]]
            lives = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
                if enemy["id"] == row["index"] and enemy["definition"] == ground["npc"]
                and enemy["generation"] == generation and enemy["instance"] == player["instance"]
                and enemy.get("visible") and enemy["hitpoints"] > ZERO]
            if (stamp != fight_map or state["map"] != fight_map or len(matches) != ONE
                    or len(lives) != ONE or self.receipts.player()["instance"] != player["instance"]):
                raise shared.Stop("Same ground life was not observed after refused Attack") from refusal
            row = matches[ZERO]
            _, operation = self.operation(row, ("Attack",))
            self.log("unaccepted_attack_refreshed", {"refusal": refusal.response,
                "actor": row, "maximum_refreshes": ONE, "generation": generation})
            after = self.scanned_action("npc", row, operation)
        def transformed(state):
            same = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
                if enemy["id"] == row["index"] and enemy["definition"] == flying["npc"]
                and enemy["instance"] == player["instance"] and enemy.get("visible")
                and enemy["generation"] == generation + ONE
                and enemy["hitpoints"] == flying["maximumLife"]]
            native, stamp = self.scan("scan_npcs", {"definition": flying["npc"], "level": row["level"]})
            return len(same) == ONE and stamp == fight_map and any(n["index"] == row["index"] for n in native)
        self.wait("one native full-life transformation", transformed, after,
            supervise_food=True, deadline=deadline)
        landed = [receipt for receipt in self.receipts.rows[offset:] if receipt.get("kind") == "landed"
            and receipt["target"]["id"] == row["index"]
            and receipt["target"]["definition"] == ground["npc"]
            and receipt["target"]["generation"] == generation
            and receipt["source"]["id"] == player["pid"]]
        if sum(receipt["actualDamage"] for receipt in landed) != ground["maximumLife"]:
            raise shared.Stop("First life has no exact ordinary full-life damage accounting")
        if any(receipt.get("kind") in ("death", "reward_owner")
                and receipt.get("target", {}).get("id") == row["index"]
                for receipt in self.receipts.rows[offset:]):
            raise shared.Stop("First-form depletion incorrectly emitted death or loot ownership")
        if self.receipts.player()["xp"] != player["xp"]:
            raise shared.Stop("First-form depletion incorrectly awarded combat experience")
        self.queen_life = {"actor": row["index"], "generation": generation + ONE,
            "instance": player["instance"], "pid": player["pid"], "groundGeneration": generation}
        self.log("full_ground_to_flying", {"fullLife": full[ZERO], "landed": landed,
            "oldGeneration": generation, "newGeneration": generation + ONE,
            "state": self.state, "passive": self.receipts.player()})

    def worn(self, state, held):
        slot = held["slot"]
        native = self.inventory(state, "worn_equipment")
        passive = self.receipts.player()["worn"]
        if slot >= len(native["items"]) or slot >= len(passive):
            raise shared.Stop("Native phase equipment slot is absent")
        actual = native["items"][slot]
        if actual not in held["identities"] or passive[slot][ZERO] != actual:
            return False
        if held["capacity"] is not None:
            physical = passive[slot][PHYSICAL_INSTANCE_FIELD] if len(passive[slot]) > PHYSICAL_INSTANCE_FIELD else None
            previous = getattr(self, "physical_owners", {}).get(held["item"])
            return bool(physical and physical.get("key") and ZERO < physical["charges"] <= held["capacity"]
                and (previous is None or (physical["key"] == previous["key"] and physical["charges"] <= previous["charges"])))
        return True

    def remember_physical(self, held, row):
        if held["capacity"] is None:
            return
        physical = row[PHYSICAL_INSTANCE_FIELD] if len(row) > PHYSICAL_INSTANCE_FIELD else None
        if physical is None:
            return  # A fresh prelogin item may receive its first native instance during Wield.
        prior = self.physical_owners.get(held["item"])
        if (not physical.get("key") or not ZERO < physical["charges"] <= held["capacity"]
            or (prior is not None and (prior["key"] != physical["key"] or physical["charges"] > prior["charges"]))):
            raise shared.Stop("Native phase switch replaced or refilled its physical item")
        self.physical_owners[held["item"]] = {"key": physical["key"], "charges": physical["charges"]}

    def equip_phase(self, phase):
        self.stage = "equip_" + phase
        selected = self.plan["kits"][phase]
        self.physical_owners = getattr(self, "physical_owners", {})
        self.open_backpack()
        for held in selected["equipment"]:
            state = self.observe()
            self.eat_if_needed(state)
            state = self.observe()
            if self.worn(state, held):
                self.remember_physical(held, self.receipts.player()["worn"][held["slot"]])
                continue
            candidates = [identity for identity in held["identities"]
                if self.count(self.inventory(state, "backpack"), identity) > ZERO]
            if len(candidates) != ONE:
                raise shared.Stop("Native initial/displaced phase item is absent or ambiguous")
            item = candidates[ZERO]
            passive_sources = [row for row in self.receipts.player()["backpack"] if row[ZERO] == item]
            if len(passive_sources) != ONE:
                raise shared.Stop("Native phase source item ownership is absent or ambiguous")
            self.remember_physical(held, passive_sources[ZERO])
            row = self.item_control(state, "backpack.slots", item, ("Wear", "Wield"))
            after = self.act(self.ui_action(row, ("Wear", "Wield"), item), state)
            self.wait("native phase equipment/physical publication", lambda current, held=held:
                self.worn(current, held), after, supervise_food=True)
            self.remember_physical(held, self.receipts.player()["worn"][held["slot"]])
        self.plan["prayers"] = selected["prayers"]
        self.prayers()
        self.log("equipped_" + phase, {"equipment": selected["equipment"],
            "choice": selected["choice"], "state": self.state, "passive": self.receipts.player()})

    def fight_flying(self):
        self.walk(self.plan["room"]["entrance"], radius=ENTRY_DISTANCE_TILES)
        self.walk(self.plan["room"]["spawn"], radius=NPC_SCAN_APPROACH_TILES)
        self.log("flying_range", {"state": self.state, "passive": self.receipts.player()})
        life = self.queen_life
        passive = self.receipts.player()
        full = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
            if enemy["id"] == life["actor"] and enemy["generation"] == life["generation"]
            and enemy["definition"] == self.plan["flying"]["npc"]
            and enemy["instance"] == life["instance"] and enemy["hitpoints"] == self.plan["flying"]["maximumLife"]]
        if len(full) != ONE or passive["instance"] != life["instance"]:
            raise shared.Stop("Same full flying life is absent after ordinary gear switch")
        self.plan["target"] = {"npc": self.plan["flying"]["npc"],
            "profile": {"hitpoints": self.plan["flying"]["maximumLife"]}}
        offset = len(self.receipts.rows)
        boss.Driver.fight(self)
        self.receipts.refresh()
        impacts = [row for row in self.receipts.rows[offset:] if row.get("kind") == "landed"
            and row["target"]["id"] == life["actor"] and row["target"]["generation"] == life["generation"]
            and row["target"]["definition"] == self.plan["flying"]["npc"]
            and row["source"]["id"] == life["pid"]]
        if sum(row["actualDamage"] for row in impacts) != self.plan["flying"]["maximumLife"]:
            raise shared.Stop("Flying life lacks exact full native ordinary damage accounting")
        rewards = [row for row in self.receipts.rows[offset:] if row.get("kind") == "reward_owner"
            and row["target"]["id"] == life["actor"] and row["target"]["generation"] == life["generation"]]
        if len(rewards) != ONE or (rewards[ZERO].get("owner") or {}).get("id") != life["pid"]:
            raise shared.Stop("Final Queen reward must belong to the real fixture player")
        self.final_reward = rewards[ZERO]
        self.log("full_flying_death", {"fullLife": full[ZERO], "landed": impacts,
            "reward": self.final_reward, "state": self.state, "passive": self.receipts.player()})

    def loot(self):
        self.stage = "loot"
        life, reward = self.queen_life, self.final_reward
        deadline = min(self.deadline, time.monotonic() + boss.LOOT_PUBLICATION_SECONDS)
        def corpse_gone(state):
            return any(enemy["id"] == life["actor"] and enemy["generation"] == life["generation"]
                and enemy["definition"] == self.plan["flying"]["npc"] and not enemy["visible"]
                for enemy in (self.receipts.last_state or {}).get("enemies", []))
        self.wait("ordinary final corpse release", corpse_gone, self.state["cycle"],
            supervise_food=True, deadline=deadline)
        roll = reward.get("loot")
        allocations = [] if roll is None else [roll, *roll.get("allocations", [])]
        eligible = [drop for allocation in allocations if allocation["owner"] == life["pid"]
            for drop in allocation["drops"]]
        at = reward["target"]
        observed = [row for row in self.receipts.player()["ground"]
            if row["owner"] == life["pid"] and shared.distance(row, at) == ZERO]
        if not eligible:
            if roll is not None:
                raise shared.Stop("Missing eligible loot is not a qualified empty final roll")
            self.log("loot", {"outcome": "ordinary_empty_roll", "reward": reward,
                "state": self.state, "passive": self.receipts.player()})
            return
        allowed = {(drop["item"], drop["count"]) for drop in eligible}
        matches = [row for row in observed if (row["item"], row["count"]) in allowed]
        if not matches:
            raise shared.Stop("Actual final rolled loot lacks an owner-eligible ground publication")
        selected = matches[ZERO]
        self.walk(selected, radius=boss.LOOT_APPROACH_TILES)
        state = self.observe()
        plane = state["player"]["level"]
        if selected["level"] != plane:
            raise shared.Stop("Selected owned stack left the observed player plane")
        rows, _ = self.scan("scan_objects", {"definition": selected["item"],
            "level": plane, "x": selected["x"], "z": selected["z"], "radius": ZERO})
        rows = [row for row in rows if row["count"] == selected["count"]
            and row["level"] == selected["level"]]
        if len(rows) != ONE:
            raise shared.Stop("Final owner-eligible native stack is absent or ambiguous")
        destination = "coin_wallet" if selected["item"] == self.plan["currencyItem"] else "backpack"
        before = self.count(self.inventory(self.observe(), destination), selected["item"])
        after = self.scanned_action("object", rows[ZERO], "Take")
        self.wait("ordinary owner-eligible final loot transfer", lambda current:
            self.count(self.inventory(current, destination), selected["item"]) > before,
            after, supervise_food=True)
        if any(row["token"] == selected["token"] for row in self.receipts.player()["ground"]):
            raise shared.Stop("Taken final ground stack remains published")
        self.log("loot", {"outcome": "ordinary_taken_roll", "item": selected["item"],
            "ground": selected, "destination": destination, "beforeCount": before,
            "afterCount": self.count(self.inventory(self.state, destination), selected["item"]),
            "reward": reward, "state": self.state, "passive": self.receipts.player()})

    def run(self):
        self.observe()
        self.equip_phase("ground")
        self.entry()
        original = self.receipts.player()["instance"]
        self.fight_ground()
        self.equip_phase("flying")
        self.fight_flying()
        self.loot()
        self.leave()
        self.entry(rejoin=True)
        if self.receipts.player()["instance"] != original:
            raise shared.Stop("Native rejoin replaced the original canonical public instance")
        self.log("complete", {"status": "ordinary_queen_full_pass", "instance": original,
            "actions": self.actions, "foods": self.food_receipts, "final": self.state,
            "scope": "Two full ordinary Queen lives, natural gear switch, final actual loot or empty, native exit/rejoin"})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("plan", "food-rules", "symbols", "receipts", "journal", "control-module", "socket"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--deadline-seconds", type=int, default=MAX_CLIENT_SECONDS)
    parser.add_argument("--action-seconds", type=int, default=boss.DEFAULT_ACTION_SECONDS)
    parser.add_argument("--fight-seconds", type=int, default=boss.DEFAULT_FIGHT_SECONDS)
    parser.add_argument("--socket-seconds", type=int, default=boss.DEFAULT_SOCKET_SECONDS)
    args = parser.parse_args()
    if not ZERO < args.deadline_seconds <= MAX_CLIENT_SECONDS:
        parser.error("Finite ordinary session budget required")
    control = boss.module(Path(args.control_module), "queen_normal_control").Control(args.socket, args.socket_seconds)
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
