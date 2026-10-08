#!/usr/bin/env python3
"""One native Bob repair, outside-GWD readback and nearby Legacy melee sample."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import time

SPECIAL_HERE = Path(__file__).resolve().parent
SPECIAL_SHARED = SPECIAL_HERE.parent / "barrows" / "driver.py"
SPECIAL_SHARED_SHA256 = "d5377a3458af568e527852b9b87d3c529c09a168428dcd363a36c19e85d38508"
SPECIAL_ZERO = 0
SPECIAL_ONE = 1
SPECIAL_INSTANCE_INDEX = 2
SPECIAL_INITIAL_LEVEL = 99
SPECIAL_SESSION_SECONDS = 360
SPECIAL_ACTION_SECONDS = 45
SPECIAL_FIGHT_SECONDS = 120
SPECIAL_SOCKET_SECONDS = 10
SPECIAL_SCAN_RADIUS = 8
SPECIAL_SERVICE_RADIUS = 1
SPECIAL_FIRST_OPERATION_EVENTS = 2
SPECIAL_REPAIR_TEXT = "Repair"
SPECIAL_REPAIR_QUESTION = "Repair this equipment for {price} coins?"


def special_module(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


if hashlib.sha256(SPECIAL_SHARED.read_bytes()).hexdigest() != SPECIAL_SHARED_SHA256:
    raise RuntimeError("Shared ordinary input helper changed")
special_shared = special_module(SPECIAL_SHARED, "special_ordinary_helpers")


def special_physical(slots, key):
    matches = [slot for slot in slots if len(slot) > SPECIAL_INSTANCE_INDEX
        and slot[SPECIAL_INSTANCE_INDEX].get("key") == key]
    if len(matches) != SPECIAL_ONE:
        raise special_shared.Stop("Physical repair identity is missing or repeated")
    return matches[SPECIAL_ZERO]


class SpecialDriver(special_shared.Driver):
    def __init__(self, control, args):
        self.control, self.args = control, args
        self.plan = special_shared.load_json(args.plan)
        self.food_rules = special_shared.load_json(args.food_rules)
        self.symbols = special_shared.Symbols(args.symbols)
        self.receipts = special_shared.Receipts(args.receipts)
        self.started = time.monotonic()
        self.deadline = self.started + args.deadline_seconds
        self.requests = self.actions = self.walk_probes = SPECIAL_ZERO
        self.state = self.next_food_tick = None
        self.last_scans, self.food_receipts = {}, []
        self.stage = "ready"
        self.journal = Path(args.journal).open("x", encoding="utf8")
        self.special_varps = special_shared.symbol_rows(Path(args.symbols) / "varp.sym")
        names = ("window_buttons.actions", "backpack.slots", "prayer_book.prayer_buttons",
                 "legacy_combat.special_attack", "dialogue_options.first_option", "game_window.dialogue_slot")
        self.query = {"varbits": [self.symbols.get("varbit", name) for name in
            ("current_life_points", "current_prayer_points", "legacy_combat_active")]
            + [row["varbit"] for row in self.plan["gwd"]["counts"]]
            + [row["activationBit"] for row in self.plan["prayers"]],
            "varps": [int(self.special_varps[name]) for name in ("adrenaline_fine", "special_attack_armed")],
            "inventories": [self.symbols.get("inv", name) for name in ("backpack", "worn_equipment", "coin_wallet")],
            "components": [self.symbols.component(name) for name in names],
            "client_varbits": [self.symbols.get("varbit", "legacy_selected_window")]}
        self.log("start", {"planSha256": special_shared.digest(args.plan), "sharedSha256": SPECIAL_SHARED_SHA256,
            "policy": "Normal native input, pathfinder and passive receipts; no live state edits"})

    def act(self, action, state=None):
        state = state or self.observe()
        fresh = self.observe()
        if not fresh.get("ready") or not fresh.get("focused") or fresh["map"] != state["map"]:
            raise special_shared.Stop("Fresh native admission is not ready in the same map")
        response = self.request({"command": "action", "map": fresh["map"],
            "observed_cycle": fresh["cycle"], "action": action})
        if response["status"] != "accepted":
            raise special_shared.Stop("Native action was not accepted")
        self.actions += SPECIAL_ONE
        return response["cycle"]

    def special_life(self, row):
        self.receipts.refresh()
        matching = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
            if enemy["id"] == row["index"] and enemy["definition"] == row["definition"]
            and enemy.get("visible") and enemy.get("alive") is True
            and enemy["instance"] == self.receipts.player()["instance"]]
        if len(matching) != SPECIAL_ONE:
            raise special_shared.Stop("Selected native NPC has no current passive life")
        return matching[SPECIAL_ZERO]

    def special_npc_action(self, row, operation):
        original_map = self.last_scans["scan_npcs"]["response"]["data"]["map"]
        original_life = self.special_life(row)
        action = {"kind": "npc", "index": row["index"], "definition": row["definition"],
            "update_serial": row["update_serial"], "operation": operation}
        try:
            return self.act(action)
        except special_shared.UnacceptedNpcIntent as refusal:
            state = self.observe()
            rows, stamp = self.scan("scan_npcs", {"definition": row["definition"], "level": row["level"]})
            matching = [current for current in rows if current["index"] == row["index"]
                and current["definition"] == row["definition"]]
            if stamp != original_map or state["map"] != original_map or len(matching) != SPECIAL_ONE:
                raise special_shared.Stop("Refused NPC intent lost its same installed map/identity") from refusal
            refreshed = matching[SPECIAL_ZERO]
            life = self.special_life(refreshed)
            if life["generation"] != original_life["generation"] or life["instance"] != original_life["instance"]:
                raise special_shared.Stop("Refused NPC intent changed passive life/session") from refusal
            self.log("unaccepted_npc_refreshed", {"refusal": refusal.response, "actor": refreshed,
                "map": stamp, "passive_generation": life["generation"], "maximum_refreshes": SPECIAL_ONE})
            return self.act({**action, "update_serial": refreshed["update_serial"]}, state)

    def special_outside_counts(self):
        self.stage = "outside_counts"
        state = self.observe()
        if self.bit(state, "legacy_combat_active") != SPECIAL_ONE:
            raise special_shared.Stop("Ordinary Legacy login publication is missing")
        if not all(self.bit(state, row["varbit"]) == SPECIAL_ZERO for row in self.plan["gwd"]["counts"]):
            state = self.wait("first ordinary outside count reset publication before movement",
                lambda current: all(self.bit(current, row["varbit"]) == SPECIAL_ZERO for row in self.plan["gwd"]["counts"]), state["cycle"])
        passive = self.receipts.player()
        if any(row["value"] != SPECIAL_ZERO for row in passive["counts"]):
            raise special_shared.Stop("Passive outside count owner differs from native readback")
        self.log("outside_counts", {"native": state, "passive": passive,
            "initialSavedVarps": self.plan["gwd"]["initialSavedVarps"], "liveCountWrites": False})

    def special_repair(self):
        self.stage = "bob_repair"
        bob = self.plan["repair"]["bob"]
        self.walk(bob["spawn"], radius=SPECIAL_SERVICE_RADIUS)
        state = self.observe()
        rows, stamp = self.scan("scan_npcs", {"definition": bob["npc"], "level": state["player"]["level"]})
        if len(rows) != SPECIAL_ONE or stamp != state["map"]:
            raise special_shared.Stop("Ordinary Bob is absent/ambiguous")
        before = self.receipts.player()
        repair = self.plan["repair"]
        original = special_physical(before["backpack"] + before["worn"], repair["key"])
        family = repair["family"]
        price = (family["coinRepair"]["fullCost"] * (family["capacity"] - original[SPECIAL_INSTANCE_INDEX]["charges"]) + family["capacity"] - SPECIAL_ONE) // family["capacity"]
        offset = len(self.receipts.rows)
        _, operation = self.operation(rows[SPECIAL_ZERO], (bob["operation"],))
        after = self.special_npc_action(rows[SPECIAL_ZERO], operation)
        state = self.wait("live native Bob repair quote", lambda value: self.visible(value, "dialogue_options.first_option"), after)
        first = self.component(state, "dialogue_options.first_option")
        option_group = first["target"]["parent"] >> special_shared.COMPONENT_GROUP_SHIFT
        roots = [row for row in self.children(self.symbols.component("game_window.dialogue_slot"))
            if row.get("relation") == "mounted" and row["target"]["child"] == special_shared.STATIC_COMPONENT_CHILD
            and row["target"]["parent"] >> special_shared.COMPONENT_GROUP_SHIFT == option_group
            and (row.get("value") or {}).get("rooted_visible")]
        if len(roots) != SPECIAL_ONE:
            raise special_shared.Stop("Native quote mount is absent/ambiguous")
        direct = self.children(roots[SPECIAL_ZERO]["target"])
        texts = [roots[SPECIAL_ZERO]] + direct
        for row in direct:
            if (row.get("value") or {}).get("rooted_visible"):
                texts += self.children(row["target"])
        quote_texts = [str((row.get("value") or {}).get("text") or "") for row in texts
            if (row.get("value") or {}).get("rooted_visible")]
        if not any(special_shared.normal_text(text) == special_shared.normal_text(SPECIAL_REPAIR_QUESTION.format(price=price)) for text in quote_texts):
            raise special_shared.Stop("Native quote does not equal the actual ordinary repair price")
        descendants = [first] + self.children(first["target"], recursive=True)
        if not any(special_shared.normal_text((row.get("value") or {}).get("text")) == special_shared.normal_text(SPECIAL_REPAIR_TEXT) for row in descendants):
            raise special_shared.Stop("Native pause option does not carry Repair text")
        after = self.act(self.ui_action(first, continuation=True), state)
        self.wait("ordinary guarded durable repair", lambda value: any(row.get("kind") == "repair_commit"
            and row["pid"] == before["pid"] and row["price"] == price for row in self.receipts.rows[offset:]), after)
        commits = [row for row in self.receipts.rows[offset:] if row.get("kind") == "repair_commit" and row["pid"] == before["pid"]]
        if len(commits) != SPECIAL_ONE:
            raise special_shared.Stop("Native confirmation produced repeated repair commits")
        commit = commits[SPECIAL_ZERO]
        repaired = special_physical(commit["after"]["backpack"] + commit["after"]["worn"], repair["key"])
        saved = special_physical(commit["saved"]["backpack"] + commit["saved"]["worn"], repair["key"])
        if repaired != saved or repaired[SPECIAL_ZERO] != family["coinRepair"]["output"] or repaired[SPECIAL_INSTANCE_INDEX]["charges"] != family["capacity"]:
            raise special_shared.Stop("Same repaired UUID/native identity/full balance was not saved")
        if commit["before"]["coins"] - commit["after"]["coins"] != price or commit["saved"]["coins"] != commit["after"]["coins"]:
            raise special_shared.Stop("Repair payment differs from the native quote")
        self.wait("repaired native inventory publication", lambda value: self.count(self.inventory(value, "backpack"), repaired[SPECIAL_ZERO]) == SPECIAL_ONE, after)
        self.log("durable_repair", {"price": price, "original": original, "repaired": repaired,
            "commit": commit, "quote_texts": quote_texts, "native": self.state})

    def special_prayers(self):
        self.stage = "prayers"
        self.open_window(self.plan["toolbar"]["prayerDestination"], "prayer_book.prayer_buttons", "Prayer")
        for rule in self.plan["prayers"]:
            state = self.observe()
            if self.bit(state, rule["activationBit"]) == SPECIAL_ONE:
                continue
            rows = [row for row in self.children(self.symbols.component("prayer_book.prayer_buttons"))
                if row["target"] == {"parent": self.symbols.get("component", "prayer_book.prayer_buttons"), "child": rule["button"]}
                and (row.get("value") or {}).get("rooted_visible")]
            if len(rows) != SPECIAL_ONE:
                raise special_shared.Stop("Native Prayer control is absent/ambiguous")
            row = rows[SPECIAL_ZERO]
            expected = special_shared.normal_text("Activate " + rule["name"])
            choices = [text for text in row["value"].get("ops", []) if text and special_shared.normal_text(re.sub(r"</?col(?:=[^>]*)?>", "", text, flags=re.IGNORECASE)) == expected]
            after = self.act(self.ui_action(row, choices), state)
            self.wait("native Prayer activation", lambda value: self.bit(value, rule["activationBit"]) == SPECIAL_ONE, after)

    def special_button(self, state):
        target = self.symbols.component("legacy_combat.special_attack")
        candidates = [self.component(state, "legacy_combat.special_attack")] + self.children(target)
        matching = []
        for row in candidates:
            value = (row or {}).get("value") or {}
            if not row or row["target"]["parent"] != target["parent"] or row["target"]["child"] not in (special_shared.STATIC_COMPONENT_CHILD, SPECIAL_ZERO):
                continue
            operations = value.get("ops") or []
            if value.get("rooted_visible") and value.get("active_mask", SPECIAL_ZERO) & SPECIAL_FIRST_OPERATION_EVENTS and operations and isinstance(operations[SPECIAL_ZERO], str) and operations[SPECIAL_ZERO].strip():
                matching.append((row, operations[SPECIAL_ZERO]))
        if len(matching) > SPECIAL_ONE:
            raise special_shared.Stop("Native special first-operation grant is ambiguous")
        return matching[SPECIAL_ZERO] if matching else None

    def special_sample(self):
        self.stage = "special_sample"
        target = self.plan["target"]
        self.walk(target["spawn"], radius=SPECIAL_SERVICE_RADIUS)
        state = self.observe()
        rows, stamp = self.scan("scan_npcs", {"definition": target["npc"], "level": state["player"]["level"],
            "x": target["spawn"]["x"], "z": target["spawn"]["z"], "radius": SPECIAL_SCAN_RADIUS})
        candidates = [(row, self.special_life(row)) for row in rows]
        candidates = [(row, life) for row, life in candidates if life["hitpoints"] == target["profile"]["hitpoints"]
            and special_shared.distance(row, state["player"]) <= self.plan["special"]["field"]["radiusTiles"]]
        if not candidates or stamp != state["map"]:
            raise special_shared.Stop("No genuine nearby full-health native life is installed")
        row, full_life = min(candidates, key=lambda pair: special_shared.distance(pair[SPECIAL_ZERO], state["player"]))
        self.open_window(self.plan["toolbar"]["combatDestination"], "legacy_combat.special_attack", "Combat")
        state = self.observe()
        if self.special_button(state) is None:
            state = self.wait("current native special first-operation server grant",
                lambda current: self.special_button(current) is not None, state["cycle"])
        button, label = self.special_button(state)
        offset = len(self.receipts.rows)
        after = self.act(self.ui_action(button, (label,)), state)
        energy_id = int(self.special_varps["adrenaline_fine"])
        def cost_published(value):
            launches = [receipt for receipt in self.receipts.rows[offset:]
                if receipt.get("kind") == "special_launch" and receipt["outcome"] == "launched"]
            return len(launches) == SPECIAL_ONE and [row["value"] for row in value["varps"]
                if row["id"] == energy_id] == [launches[SPECIAL_ZERO]["after"]["fine"]]
        self.wait("native special cost and fixed field installation", cost_published, after)
        launches = [receipt for receipt in self.receipts.rows[offset:] if receipt.get("kind") == "special_launch"]
        installs = [receipt for receipt in self.receipts.rows[offset:] if receipt.get("kind") == "field_install" and receipt["accepted"]]
        if len(launches) != SPECIAL_ONE or len(installs) != SPECIAL_ONE:
            raise special_shared.Stop("Special installation was absent or repeated")
        launch, install = launches[SPECIAL_ZERO], installs[SPECIAL_ZERO]
        if launch["outcome"] != "launched" or launch["before"]["fine"] - launch["after"]["fine"] != self.plan["special"]["rule"]["costFine"]:
            raise special_shared.Stop("Ordinary special energy transaction differs from the generated cost")
        if install["primary"] is not None or install["outcomes"]:
            raise special_shared.Stop("Short sample unexpectedly captured a periodic primary")
        energy = [value["value"] for value in self.state["varps"] if value["id"] == energy_id]
        if energy != [launch["after"]["fine"]]:
            raise special_shared.Stop("Actual native energy publication differs from the cost transaction")
        self.log("special_activated", {"launch": launch, "install": install, "native": self.state,
            "nativeEnergyId": energy_id, "nativeEnergy": energy[SPECIAL_ZERO], "selected": button, "actualLabel": label})
        rows, current_map = self.scan("scan_npcs", {"definition": row["definition"], "level": row["level"]})
        matching = [current for current in rows if current["index"] == row["index"]]
        if len(matching) != SPECIAL_ONE or current_map != stamp:
            raise special_shared.Stop("Full-health target changed before native Attack")
        row = matching[SPECIAL_ZERO]
        live = self.special_life(row)
        if (live["generation"] != full_life["generation"] or live["hitpoints"] != full_life["hitpoints"]
                or special_shared.distance(row, install["area"]["center"]) > install["area"]["radiusTiles"]):
            raise special_shared.Stop("Selected life changed before native Attack")
        _, operation = self.operation(row, ("Attack",))
        after = self.special_npc_action(row, operation)
        self.wait("genuine nearby full-life ordinary melee death", lambda value: any(receipt.get("kind") == "death"
            and receipt["target"]["id"] == row["index"] and receipt["target"]["generation"] == full_life["generation"]
            and receipt["source"]["id"] == launch["pid"] for receipt in self.receipts.rows[offset:]), after,
            seconds=self.args.fight_seconds, supervise_food=True)
        hits = [receipt for receipt in self.receipts.rows[offset:] if receipt.get("kind") == "landed"
            and receipt["target"]["id"] == row["index"] and receipt["target"]["generation"] == full_life["generation"]
            and receipt["source"]["id"] == launch["pid"]]
        if not hits or sum(receipt["actualDamage"] for receipt in hits) != full_life["hitpoints"]:
            raise special_shared.Stop("Full-life death has no matching ordinary credited damage")
        self.log("ordinary_sample", {"fullLife": full_life, "landed": hits, "native": self.state})
        self.stage = "field_lifetime"
        self.wait("ordinary field natural retirement", lambda value: any(receipt.get("kind") == "field_retire"
            and receipt["key"] == install["key"] and not receipt["cancelled"]
            and receipt["tick"] == install["tick"] + install["durationTicks"] for receipt in self.receipts.rows[offset:]),
            self.state["cycle"], seconds=self.args.action_seconds, supervise_food=True)
        retired = [receipt for receipt in self.receipts.rows[offset:] if receipt.get("kind") == "field_retire" and receipt["key"] == install["key"]]
        self.log("field_retired", {"receipt": retired[SPECIAL_ZERO], "native": self.state,
            "periodicDamageExercised": False, "presentationQualified": False})

    def run(self):
        self.special_outside_counts()
        self.special_repair()
        self.equip()
        self.log("equipped", {"native": self.state, "passive": self.receipts.player()})
        self.special_prayers()
        self.special_sample()
        self.log("complete", {"status": "ordinary_special_repair_pass", "actions": self.actions,
            "foods": self.food_receipts, "native": self.state, "passive": self.receipts.player()})


def special_main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("plan", "food-rules", "symbols", "receipts", "journal", "control-module", "socket"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--deadline-seconds", type=int, default=SPECIAL_SESSION_SECONDS)
    parser.add_argument("--action-seconds", type=int, default=SPECIAL_ACTION_SECONDS)
    parser.add_argument("--fight-seconds", type=int, default=SPECIAL_FIGHT_SECONDS)
    parser.add_argument("--socket-seconds", type=int, default=SPECIAL_SOCKET_SECONDS)
    args = parser.parse_args()
    if not SPECIAL_ZERO < args.deadline_seconds <= SPECIAL_SESSION_SECONDS:
        parser.error("Finite special/repair budget required")
    control = special_module(Path(args.control_module), "special_native_control").Control(args.socket, args.socket_seconds)
    driver = None
    try:
        driver = SpecialDriver(control, args)
        driver.run()
    finally:
        if driver is not None:
            driver.journal.close()
        control.close()


if __name__ == "__main__":
    special_main()
