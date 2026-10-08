#!/usr/bin/env python3
"""One full-life ordinary public encounter; natural special coverage is conditional."""
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
POLL_SECONDS = 0.2
MAX_CLIENT_SECONDS = 600
MAX_NATURAL_ROUTE_RECOVERIES = 2


def load_boss():
    import importlib.util
    if hashlib.sha256(BOSS.read_bytes()).hexdigest() != BOSS_SHA256:
        raise RuntimeError("Inherited ordinary encounter driver changed; rebind source first")
    spec = importlib.util.spec_from_file_location("chaos_ordinary_boss_driver", BOSS)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


boss = load_boss()
shared = boss.shared


FULL_BOSS_LIFE = 17250
PROOF_SCOPE = "full-life-loot-retreat"

controls = boss.module(HERE.parent / "boss-encounters" / "encounter_controls.py", "ordinary_encounter_controls")

class Driver(controls.EncounterControlsMixin, boss.Driver):
    encounter_stop_type = shared.Stop
    encounter_unaccepted_type = shared.UnacceptedNpcIntent
    encounter_normal_text = staticmethod(shared.normal_text)

    def __init__(self, control, args):
        super().__init__(control, args)
        self.encounter_recipe = {"foodItem": self.plan["food"]["item"], "scanRadius": self.plan["scanRadius"]}
        self.encounter_can_consume = lambda state, player: player["instance"] is None and player["life"] > ZERO
        self.natural_effects = set()
        self.effect_offset = ZERO
        self.interrupted_admission_effects = set()

    def public(self):
        state = self.observe()
        passive = self.receipts.player()
        if passive["instance"] is not None or state.get("region") is not None:
            raise shared.Stop("Public Chaos route entered an instance")
        return state, passive

    def walk_public(self, destination):
        for recovery in range(MAX_NATURAL_ROUTE_RECOVERIES + ONE):
            original_player = self.passive_player()
            original_identity = (original_player["pid"], original_player["generation"], original_player["instance"])
            offset = len(self.receipts.rows)
            try:
                return self.walk(destination, radius=ZERO)
            except shared.Stop as failure:
                if not str(failure).startswith("normal route stopped short") or recovery == MAX_NATURAL_ROUTE_RECOVERIES:
                    raise
                state, player = self.public()
                native = state["player"]
                worn = self.inventory(state, "worn_equipment")["items"]
                effects = [row for row in self.receipts.rows[offset:]
                    if row.get("kind") == "special_delivery"
                    and row["source"]["definition"] == self.plan["target"]["npc"]
                    and row["after"]["pid"] == player["pid"]
                    and row["before"]["generation"] == row["after"]["generation"] == player["generation"]
                    and row["before"]["instance"] is None and row["after"]["instance"] is None
                    and shared.distance(native, row["after"]) == shared.distance(player, row["after"]) == ZERO
                    and native["level"] == row["after"]["level"]
                    and ((row["effect"] == "displacement" and row["after"]["relocation"] > row["before"]["relocation"])
                         or (row["effect"] == "equipment-removal" and self.worn_after_delivery(state, player, row["after"])))]
                if (native["route_length"] != ZERO or not effects
                        or (player["pid"], player["generation"], player["instance"]) != original_identity):
                    raise
                self.log("walk_interrupted_by_observed_natural_effect", {"destination": destination,
                    "delivery": effects[-ONE], "state": state, "passive": player,
                    "recovery": recovery + ONE, "maximumRecoveries": MAX_NATURAL_ROUTE_RECOVERIES,
                    "priorWalkStoppedShort": True, "observedRouteLength": native["route_length"],
                    "acceptedPendingActionRepeated": False})

    def entry(self):
        self.stage = "enter"
        state, passive = self.public()
        original_identity = (passive["pid"], passive["generation"], passive["instance"])
        after = self.act({"kind": "walk", "x": self.plan["arrival"]["x"],
            "z": self.plan["arrival"]["z"], "run": False}, state)
        deadline = min(self.deadline, time.monotonic() + self.args.action_seconds)
        while time.monotonic() < deadline:
            state, passive = self.public()
            if (passive["pid"], passive["generation"], passive["instance"]) != original_identity:
                raise shared.Stop("Initial visibility walk lost its player life/public owner")
            self.eat_if_needed(state)
            try:
                row, binding = self.observed_life(self.plan["target"]["npc"], full_life=FULL_BOSS_LIFE)
            except shared.Stop as refusal:
                if str(refusal) != "One installed actor and independently observed live owner are required":
                    raise
                time.sleep(POLL_SECONDS)
                continue
            self.log(self.stage, {"state": self.state, "passive": self.passive_player(), "nativeActor": row,
                "binding": binding, "initialWalkAcceptedCycle": after,
                "approachOwner": "ordinary native Attack pathfinder", "arrivalSquareClaim": False,
                "initialWalkCompletionClaim": False, "initialWalkRepeated": False})
            return
        raise shared.Stop("Initial native visibility did not admit the full public boss life")

    def worn_after_delivery(self, state, player, delivered):
        native = self.inventory(state, "worn_equipment")["items"]
        current, prior = player["worn"], delivered["worn"]
        if native != [slot[ZERO] for slot in current] or len(current) != len(prior):
            return False
        held_slots = {held["slot"]: held for held in self.plan["kits"]["melee"]["equipment"]}
        for slot, (actual, earlier) in enumerate(zip(current, prior)):
            held = held_slots.get(slot)
            if held is None or held["capacity"] is None or actual[ZERO] < ZERO or earlier[ZERO] < ZERO:
                if actual != earlier:
                    return False
                continue
            physical, previous = actual[PHYSICAL_INSTANCE_FIELD], earlier[PHYSICAL_INSTANCE_FIELD]
            if (actual[ZERO] not in held["identities"] or earlier[ZERO] not in held["identities"]
                    or actual[ONE] != earlier[ONE] or not physical or not previous
                    or physical["key"] != previous["key"] or physical["key"] != self.physical_keys[slot]
                    or not ZERO < physical["charges"] <= previous["charges"] <= held["capacity"]):
                return False
        return True

    def completed_life(self, state):
        self.receipts.refresh()
        return any(row.get("kind") == "death" and row["target"]["id"] == self.life["actor"]
            and row["target"]["generation"] == self.life["generation"] for row in self.receipts.rows[self.effect_offset:])

    def attack_admission_finished(self, state):
        if self.completed_life(state):
            return True
        player = self.passive_player()
        native = self.inventory(state, "worn_equipment")["items"]
        if (player["pid"] != self.life["pid"] or player["generation"] != self.life["playerGeneration"]
                or player["instance"] != self.life["instance"] or player.get("target") is not None
                or native != [slot[ZERO] for slot in player["worn"]]):
            return False
        effects = [row for row in self.receipts.rows[self.effect_offset:]
            if row.get("kind") == "special_delivery"
            and row["source"]["id"] == self.life["actor"]
            and row["source"]["generation"] == self.life["generation"]
            and row["after"]["pid"] == self.life["pid"]
            and row["before"]["generation"] == row["after"]["generation"] == self.life["playerGeneration"]
            and row["before"]["instance"] == row["after"]["instance"] == self.life["instance"]
            and shared.distance(state["player"], row["after"]) == shared.distance(player, row["after"]) == ZERO
            and state["player"]["level"] == row["after"]["level"]
            and (row["source"]["id"], row["source"]["generation"], row["tick"], row["effect"]) not in self.interrupted_admission_effects]
        missing = any(native[held["slot"]] not in held["identities"] for held in self.plan["kits"]["melee"]["equipment"])
        effects = [row for row in effects if
            (row["effect"] == "equipment-removal" and missing and self.worn_after_delivery(state, player, row["after"]))
            or (row["effect"] == "displacement" and row["after"]["relocation"] > row["before"]["relocation"])]
        if state["player"]["route_length"] != ZERO or not effects:
            return False
        event = effects[-ONE]
        self.interrupted_admission_effects.add((event["source"]["id"], event["source"]["generation"], event["tick"], event["effect"]))
        self.observe_natural(state, player)
        self.log("attack_interrupted_by_observed_natural_effect", {"delivery": event,
            "state": state, "passive": player, "observedRouteLength": ZERO,
            "acceptedPendingActionRepeated": False})
        return True

    def attack(self):
        return self.attack_observed(self.life, self.attack_admission_finished)

    def observe_natural(self, state, player):
        self.receipts.refresh()
        for event in self.receipts.rows[self.effect_offset:]:
            if (event.get("kind") != "special_delivery" or event["effect"] in self.natural_effects
                    or event["source"]["id"] != self.life["actor"]
                    or event["source"]["generation"] != self.life["generation"]
                    or event["after"]["pid"] != self.life["pid"]):
                continue
            native = self.inventory(state, "worn_equipment")["items"]
            if not self.worn_after_delivery(state, player, event["after"]):
                continue
            if shared.distance(state["player"], player) != ZERO:
                continue
            if event["effect"] == "displacement" and shared.distance(state["player"], event["after"]) != ZERO:
                continue
            self.natural_effects.add(event["effect"])
            self.log("natural_special", {"effect": event["effect"], "delivery": event, "state": state, "passive": player})

    def gear_keys(self):
        return {held["slot"]: self.receipts.player()["worn"][held["slot"]][PHYSICAL_INSTANCE_FIELD]["key"]
            for held in self.plan["kits"]["melee"]["equipment"] if held["capacity"] is not None}

    def recover_gear(self):
        self.equip_phase("melee", recover=True)
        if any(self.physical_owners[held["item"]]["key"] != self.physical_keys[held["slot"]]
                for held in self.plan["kits"]["melee"]["equipment"] if held["capacity"] is not None):
            raise shared.Stop("Natural re-equip replaced a captured physical item")

    def fight(self):
        self.stage = "fight"
        state, passive = self.public()
        _, self.life = self.observed_life(self.plan["target"]["npc"], full_life=FULL_BOSS_LIFE)
        full = next(enemy for enemy in self.receipts.last_state["enemies"]
            if enemy["id"] == self.life["actor"] and enemy["generation"] == self.life["generation"])
        if not any(slot[ZERO] < ZERO for slot in passive["backpack"]):
            raise shared.Stop("Ordinary Wield did not create a genuine backpack vacancy")
        offset = len(self.receipts.rows)
        self.effect_offset = ZERO
        self.attack()
        deadline = min(self.deadline, time.monotonic() + self.args.fight_seconds)
        death = None
        while time.monotonic() < deadline:
            state, current = self.public()
            self.eat_if_needed(state)
            self.observe_natural(state, current)
            deaths = [row for row in self.receipts.rows[offset:] if row.get("kind") == "death"
                and row["target"]["id"] == self.life["actor"]
                and row["target"]["generation"] == self.life["generation"]]
            if deaths:
                if len(deaths) != ONE:raise shared.Stop("Public actor died more than once")
                death = deaths[ZERO];break
            if any(not self.equipped_observed(state, held) for held in self.plan["kits"]["melee"]["equipment"]):
                self.recover_gear()
            if (self.receipts.player().get("target") or {}).get("id") != self.life["actor"]:
                self.attack()
            time.sleep(POLL_SECONDS)
        if death is None:raise shared.Stop("Full ordinary Chaos life did not finish within budget")
        impacts = self.require_completion_budget(self.life, offset)
        rewards = [row for row in self.receipts.rows[offset:] if row.get("kind") == "reward_owner"
            and row["target"]["id"] == self.life["actor"] and row["target"]["generation"] == self.life["generation"]]
        discord = [row for row in self.receipts.rows[offset:] if row.get("kind") == "landed"
            and (row.get("source") or {}).get("id") == self.life["actor"] and row["target"]["id"] == self.life["pid"]]
        if (sum(row["actualDamage"] for row in impacts) != FULL_BOSS_LIFE or not discord
                or len(rewards) != ONE or (rewards[ZERO].get("owner") or {}).get("id") != self.life["pid"]):
            raise shared.Stop("Full ordinary life, natural Discord or real reward ownership is absent")
        self.final_reward = rewards[ZERO]
        self.log("boss_death", {"fullLife": full, "landed": impacts, "death": death,
            "reward": self.final_reward, "state": self.state, "passive": self.receipts.player()})

    def leave(self):
        self.stage = "leave"
        self.walk_public(self.plan["retreat"])
        state, passive = self.public()
        if shared.distance(state["player"], self.plan["retreat"]) != ZERO or passive.get("target") is not None:
            raise shared.Stop("Ordinary public walk-away has not cleared our pursuit")
        self.log("leave", {"state": state, "passive": passive, "safeAreaClaim": False})

    def run(self):
        self.public()
        self.equip_phase("melee")
        self.physical_keys = self.gear_keys()
        self.entry()
        self.fight()
        self.loot()
        self.leave()
        # Native return/rejoin remains unproved here; owning socket routes cover it.
        self.log("complete", {"status": "ordinary_chaos_public_full_life_loot_retreat_pass", "instance": None,
            "actions": self.actions, "foods": self.food_receipts, "final": self.state,
            "scope": PROOF_SCOPE, "naturalEffectsObserved": sorted(self.natural_effects),
            "naturalEffectsUnobserved": sorted({"equipment-removal", "displacement"} - self.natural_effects)})

    def equip_phase(self, phase, recover=False):
        self.stage = "equip_" + phase
        selected = self.plan["kits"][phase]
        self.equip_observed(selected)
        for prayer in selected["prayers"]:
            self.prayer_observed(prayer, True)
        self.log("recovered_equipment" if recover else "equipped_" + phase, {"equipment": selected["equipment"],
            "choice": selected["choice"], "state": self.state, "passive": self.receipts.player()})

    def loot(self):
        self.stage = "loot"
        life, reward = self.life, self.final_reward
        deadline = min(self.deadline, time.monotonic() + boss.LOOT_PUBLICATION_SECONDS)
        def corpse_gone(state):
            return any(enemy["id"] == life["actor"] and enemy["generation"] == life["generation"]
                and enemy["definition"] == self.plan["target"]["npc"] and not enemy["visible"]
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
            if roll is not None or observed:
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
    control = boss.module(Path(args.control_module), "chaos_normal_control").Control(args.socket, args.socket_seconds)
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
