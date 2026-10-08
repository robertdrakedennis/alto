#!/usr/bin/env python3
"""GWD composition of the shared ordinary observed-input driver.

No child process, protocol constructor, player-file write, RNG override or
gameplay endpoint. The finite recorder owns authentication/processes/budget.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import time
import saved_projection

GENERIC_PATH = Path(__file__).resolve().parent.parent / "barrows" / "driver.py"
GENERIC_SHA256 = "d5377a3458af568e527852b9b87d3c529c09a168428dcd363a36c19e85d38508"
if hashlib.sha256(GENERIC_PATH.read_bytes()).hexdigest() != GENERIC_SHA256:
    raise RuntimeError("Frozen ordinary input orchestration changed")
spec = importlib.util.spec_from_file_location("gwd_shared_observed_input", GENERIC_PATH)
shared = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shared)
Stop = shared.Stop
FIRST_ENTRY = 0
SINGLE_MATCH = 1
NO_COUNT = 0
WINDOW_SELECTION_BIAS = 1
SCANNER_PAGE_SIZE = 64
MAX_SCANNER_ROWS = 4096
ALTAR_APPROACH_TILES = 1
CAMP_APPROACH_TILES = 1
LOOT_RADIUS_TILES = 4
PRAYER_COMPONENT_GROUP = "prayer_book.prayer_buttons"
DEFAULT_DEADLINE_SECONDS = 1200
DEFAULT_FIGHT_SECONDS = 300
MAX_REJOIN_CAMP_KILLS = 40
ROPE_RESET_KILL_NAME = "exit-camp:1"
ROPE_BEFORE_SAVED_FILE = "rope-before-saved-state.json"
UNACCEPTED_NPC_REASON = "stale or unavailable NPC"
MAXIMUM_NPC_INTENT_REFRESHES = 1
RETALIATION_ON = 0
RETALIATION_OFF = 1
RETALIATION_COMPONENT = "legacy_combat.retaliation_toggle"
ESCAPE_AWT_CODE = 27
MODAL_CLOSE_CHILD = 1
PHYSICAL_INSTANCE_FIELD = 2
SPECIAL_COMPONENT = "legacy_combat.special_attack"
SPECIAL_INPUT = "combat/specials.json"
SPECIAL_UNARMED = 0
SPECIAL_ARMED = 1
NO_NATIVE_TARGET = -1
FIRST_OPERATION_EVENTS = 1 << shared.FIRST_OPERATION
KITE_WALL_MARGIN = 1
KITE_MINIMUM_FOOTPRINT_GAP = 2
KITE_SETTLED_SAMPLES = 2
KITE_RUN_BURST_DIVISOR = 2
KITE_PERCENT_MAXIMUM = 100
KITE_RUN_INPUT = "combat/run-energy.json"
KITE_RUN_BUTTON = "minimap.run_button"
KITE_RUN_TEXT = "minimap.run_energy_text"


class UnacceptedNpcIntent(Stop):
    def __init__(self, response):
        super().__init__("Unaccepted native NPC intent: " + json.dumps(response, sort_keys=True))
        self.response = response


class Driver(shared.Driver):
    def __init__(self, control, args):
        # Do not invoke the Barrows constructor/run, puzzle, Region or ledger methods.
        self.control, self.args = control, args
        self.rules, self.plan = shared.load_json(args.rules), shared.load_json(args.plan)
        self.food_rules = shared.load_json(args.food_rules)
        self.slayer_rules = shared.load_json(args.slayer_rules)
        self.symbols, self.receipts = shared.Symbols(args.symbols), shared.Receipts(args.receipts)
        self.started = time.monotonic()
        self.deadline = self.started + args.deadline_seconds
        self.requests = self.actions = self.walk_probes = NO_COUNT
        self.state, self.next_food_tick = None, None
        self.last_scans, self.full_health, self.completed = {}, {}, set()
        self.camp_attempts = self.rejoin_attempts = NO_COUNT
        self.food_receipts = []
        self.rejoin_kills = []
        self.rejoin = None
        self.stage = "ready"
        self.camp_selection_deadline = None
        self.special_preparation_deadline = None
        self.altar_approach_deadline = None
        self.special_preparation_context = None
        self.special_attempted = False
        self.kite_policy = self.plan.get("saraKite")
        self.kite_deadline, self.kite_context = None, None
        if self.kite_policy is not None and (self.plan["faction"] != "saradomin"
                or self.plan.get("saraSpecial") is not None):
            raise Stop("Local Sara movement and SGS policies must be exclusive")
        self.special_policy = self.plan.get("saraSpecial")
        special_inputs = {}
        if self.special_policy is not None:
            if self.plan["faction"] != "saradomin":
                raise Stop("The proposed special strategy belongs only to Sara")
            self.symbols.tables["varp"] = shared.symbol_rows(Path(args.symbols) / "varp.sym")
            special_inputs = {"specials": args.special_rules, "specialVarps": Path(args.symbols) / "varp.sym"}
            special_rules = shared.load_json(args.special_rules)["rules"]
            weapon = self.symbols.get("obj", "saradomin_godsword")
            matching = [row for row in special_rules["weapons"] if row["weapon"] == weapon]
            facts = [row for row in self.plan["equipmentFacts"] if row["item"] == weapon]
            if len(matching) != SINGLE_MATCH or len(facts) != SINGLE_MATCH:
                raise Stop("The declared special lacks the actual unique SGS rule/equipment")
            rule, equipment = matching[FIRST_ENTRY], facts[FIRST_ENTRY]["qualified"]
            expected = {"weapon": weapon, "mainHandSlot": equipment["slot"],
                "costFine": rule["costFine"], "maximumEnergyFine": special_rules["maximumEnergyFine"],
                "energyVarp": self.symbols.get("varp", "adrenaline_fine"),
                "armedVarp": self.symbols.get("varp", "special_attack_armed")}
            if any(self.special_policy.get(key) != value for key, value in expected.items()) \
                    or self.special_policy.get("source") != {"input": SPECIAL_INPUT,
                        "sha256": shared.digest(args.special_rules)} \
                    or rule["unavailable"] is not None or rule["activation"] != "armed" \
                    or rule["target"] != "primary" or rule["resource"] != "none" \
                    or rule["chargeFraction"] is not None or rule["cooldownTicks"] != NO_COUNT \
                    or rule["pairedWeapons"] or rule["quests"] \
                    or not any(effect["kind"] == "heal" for effect in rule["effects"]):
                raise Stop("Declared SGS policy/input differs from its ordinary qualified owner")
        elif self.plan["faction"] == "saradomin" and self.kite_policy is None:
            raise Stop("This proposed Sara driver requires the declared special strategy")
        self.journal = Path(args.journal).open("x", encoding="utf8")
        self.components = [self.symbols.component(name) for name in (
            "window_buttons.actions", "backpack.slots", PRAYER_COMPONENT_GROUP, RETALIATION_COMPONENT,
            "options_menu.settings_button", "gameplay_settings.categories",
            "gameplay_settings.options", "game_window.modal_close_button")]
        if self.special_policy is not None:
            self.components.append(self.symbols.component(SPECIAL_COMPONENT))
        if self.kite_policy is not None:
            self.symbols.tables["varp"] = shared.symbol_rows(Path(args.symbols) / "varp.sym")
            self.components += [self.symbols.component(name) for name in (KITE_RUN_BUTTON, KITE_RUN_TEXT)]
        self.retaliation_varp = self.plan.get("toolbar", {}).get("retaliationVarp")
        if isinstance(self.retaliation_varp, bool) or not isinstance(self.retaliation_varp, int) \
                or self.retaliation_varp < NO_COUNT:
            raise Stop("Declared plan lacks the generated named retaliation varp")
        self.query = {
            "varps": [self.retaliation_varp] + ([] if self.special_policy is None else
                [self.special_policy["energyVarp"], self.special_policy["armedVarp"]]),
            "varbits": sorted({row["varbit"] for row in self.rules["counts"]} |
                {row["activationBit"] for row in self.plan["prayers"]} |
                {self.symbols.get("varbit", name) for name in (
                    "current_life_points", "current_prayer_points", "legacy_combat_active",
                    "legacy_interface_mode", "slim_window_headers", "gameplay_settings_category")}),
            "inventories": [self.symbols.get("inv", name) for name in ("backpack", "worn_equipment")],
            "components": self.components,
            "client_varbits": [self.symbols.get("varbit", "legacy_selected_window")],
        }
        if self.kite_policy is not None:
            self.query["varps"].append(self.kite_policy["runModeVarp"])
        matches = [row for row in self.rules["arenas"] if row["id"] == self.plan["arenaId"]]
        if len(matches) != SINGLE_MATCH:
            raise Stop("Generated normal arena absent/ambiguous")
        self.arena = matches[FIRST_ENTRY]
        if self.kite_policy is not None:
            policy = self.kite_policy
            run = shared.load_json(args.run_rules)["rules"]
            weapon = self.plan.get("thrownFacts") or {}
            facts = [row for row in self.plan["equipmentFacts"] if row["item"] == policy["weapon"]]
            generals = [role for role in self.arena["roles"] if role["general"]]
            if len(generals) != SINGLE_MATCH or not generals[FIRST_ENTRY]["closePair"] \
                    or not all(attack["closeOnly"] for attack in generals[FIRST_ENTRY]["attacks"]) \
                    or policy["role"] != generals[FIRST_ENTRY]["title"] \
                    or len(facts) != SINGLE_MATCH or facts[FIRST_ENTRY]["qualified"]["kind"] != "ranged-thrown" \
                    or policy["weapon"] != weapon.get("item") \
                    or policy["mainHandSlot"] != facts[FIRST_ENTRY]["qualified"]["slot"] \
                    or policy["range"] != weapon.get("range", {}).get("range") \
                    or policy["speedTicks"] != weapon.get("speed") \
                    or policy["runModeVarp"] != self.symbols.get("varp", "run_mode") \
                    or policy["runDrainFine"] != run["baseDrainFine"] \
                    or policy["source"] != {"input": KITE_RUN_INPUT, "sha256": shared.digest(args.run_rules)} \
                    or any(type(policy[key]) is not int or policy[key] <= NO_COUNT
                        for key in ("range", "speedTicks", "runDrainFine", "runFinePerPercent", "runInitialFine")):
                raise Stop("Declared local ranged movement differs from native/generated owners")
        if self.special_policy is not None and [role["title"] for role in self.arena["roles"]
                if role["general"]] != [self.special_policy.get("role")]:
            raise Stop("Declared SGS intent is not the unique current Sara general")
        self.count_rule = next(row for row in self.rules["counts"]
            if row["faction"] == self.plan["faction"])
        fixture = shared.load_json(Path(args.plan).parent / "initial-fixture.json")
        if fixture["plan"] != self.plan:
            raise Stop("Declared account and actual plan differ")
        if self.kite_policy is not None and fixture["source"]["inputs"].get(KITE_RUN_INPUT) \
                != self.kite_policy["source"]["sha256"]:
            raise Stop("Actual initial fixture does not declare the finite run input epoch")
        if self.special_policy is not None and fixture["source"]["inputs"].get(SPECIAL_INPUT) \
                != self.special_policy["source"]["sha256"]:
            raise Stop("Actual initial fixture does not declare the SGS input epoch")
        self.account_path, self.account_key = saved_projection.declared_account_path(
            Path(args.plan).parent, fixture)
        self.log("start", {"inputs": {name: {"path": str(file), "sha256": shared.digest(file)}
            for name, file in {"rules": args.rules, "plan": args.plan, "food": args.food_rules,
                "slayer": args.slayer_rules,
                **({} if self.kite_policy is None else {"runRules": args.run_rules}),
                **special_inputs,
                "control": args.control_module, "driver": Path(__file__), "shared": GENERIC_PATH,
                "savedProjection": Path(saved_projection.__file__)}.items()},
            "proof": "ordinary core/input and normal RNG; no native visibility claim until recorder receipts",
            "budget": "GWD recorder owns zero-before-first/max-four; no foreign historical checks"})

    def request(self, command, deadline=None):
        until = self.deadline if deadline is None else min(self.deadline, deadline)
        if self.camp_selection_deadline is not None:
            until = min(until, self.camp_selection_deadline)
        if self.special_preparation_deadline is not None:
            until = min(until, self.special_preparation_deadline)
        if self.altar_approach_deadline is not None:
            until = min(until, self.altar_approach_deadline)
        if self.kite_deadline is not None:
            until = min(until, self.kite_deadline)
        remaining = until - time.monotonic()
        if remaining <= NO_COUNT or self.requests >= shared.MAX_REQUESTS:
            raise Stop("Finite ordinary GWD session/request bound exhausted")
        self.requests += SINGLE_MATCH
        response = self.control.request(command, timeout=min(self.args.socket_seconds, remaining))
        self.log("control", {"request": command, "response": response})
        if command.get("command", "").startswith("scan_"):
            self.last_scans[command["command"]] = {"request": command, "response": response}
        self.receipts.refresh()
        if command.get("command") == "action" and command.get("action", {}).get("kind") == "npc" \
                and response.get("status") == "refused" \
                and response.get("data", {}).get("reason") == UNACCEPTED_NPC_REASON:
            raise UnacceptedNpcIntent(response)
        if response.get("status") not in ("observed", "accepted"):
            raise Stop("Ordinary GWD controller refused: " + json.dumps(response, sort_keys=True))
        return response

    def act(self, action, state=None, guard=None):
        selected = state or self.observe()
        source_map = selected["map"]
        if not selected.get("ready") or not selected.get("focused"):
            self.wait("focused installed GWD before selected action", lambda current:
                current.get("focused"), selected["cycle"])
        # Only the admission cycle is refreshed. The selected target/serial/
        # label/item/stack facts remain unchanged for native resolution.
        fresh = self.observe()
        if fresh.get("map") != source_map or not fresh.get("ready") or not fresh.get("focused"):
            raise Stop("Selected GWD intent no longer has the same ready focused map")
        if self.special_preparation_context is not None and not self.special_guard(
                fresh, *self.special_preparation_context):
            raise Stop("Original SGS preparation changed before ordinary food/window input")
        if self.kite_context is not None:
            self.kite_guard(fresh)
        if guard is not None and not guard(fresh):
            raise Stop("The original SGS preparation admission changed before submission")
        self.actions += SINGLE_MATCH
        response = self.request({"command": "action", "map": source_map,
            "observed_cycle": fresh["cycle"], "action": action})
        if response.get("status") != "accepted":
            raise Stop("GWD intent did not receive real recorded acceptance")
        return response["cycle"]

    def act_scanned_npc(self, row, state, selected_map, guard=None):
        # Submit the exact fresh scanner observation without another snapshot
        # between its serial and the normal controller's live admission.
        scan = self.last_scans.get("scan_npcs", {}).get("response")
        if not isinstance(scan, dict) or scan.get("status") != "observed":
            raise Stop("The refreshed NPC lacks its actual scanner response")
        payload, cycle = scan.get("data", {}), scan.get("cycle")
        if isinstance(cycle, bool) or not isinstance(cycle, int) \
                or cycle < state["cycle"] or payload.get("map") != selected_map \
                or state.get("map") != selected_map or not state.get("ready") \
                or not state.get("focused"):
            raise Stop("The refreshed NPC scanner does not retain the original ready focused map")
        entries = payload.get("scan", {}).get("entries", [])
        exact = [entry for entry in entries if all(entry.get(key) == row.get(key)
            for key in ("index", "definition", "level", "update_serial"))]
        if len(exact) != SINGLE_MATCH or exact[FIRST_ENTRY] != row:
            raise Stop("The refreshed NPC is not the exact current scanner row")
        if guard is not None and not guard(state):
            raise Stop("Original SGS admission changed before refreshed NPC submission")
        _, operation = self.operation(row, ("Attack",))
        self.actions += SINGLE_MATCH
        response = self.request({"command": "action", "map": selected_map,
            "observed_cycle": cycle, "action": {"kind": "npc",
                **{key: row[key] for key in ("index", "definition", "update_serial")},
                "operation": operation}})
        if response.get("status") != "accepted":
            raise Stop("Refreshed NPC intent did not receive real recorded acceptance")
        return response["cycle"]

    def act_scanned_object(self, row, state, selected_map):
        scan = self.last_scans.get("scan_objects", {}).get("response")
        if not isinstance(scan, dict) or scan.get("status") != "observed":
            raise Stop("The selected ground stack lacks its actual scanner response")
        payload, cycle = scan.get("data", {}), scan.get("cycle")
        if isinstance(cycle, bool) or not isinstance(cycle, int) \
                or cycle < state["cycle"] or payload.get("map") != selected_map \
                or state.get("map") != selected_map or not state.get("ready") \
                or not state.get("focused") or payload.get("object_revision") != row["object_revision"]:
            raise Stop("The selected ground stack does not retain its ready focused map/revision")
        entries = payload.get("scan", {}).get("entries", [])
        if sum(entry == row for entry in entries) != SINGLE_MATCH:
            raise Stop("The selected ground stack is not the exact current scanner row")
        self.actions += SINGLE_MATCH
        response = self.request({"command": "action", "map": selected_map,
            "observed_cycle": cycle, "action": {"kind": "object",
                **{key: row[key] for key in ("definition", "x", "z", "level",
                    "stack_index", "count", "object_revision")}, "operation": "Take"}})
        if response.get("status") != "accepted":
            raise Stop("Selected ground intent did not receive real recorded acceptance")
        return response["cycle"]

    def scan(self, kind, query=None):
        rows, offset, stamp, revision = [], NO_COUNT, None, None
        while True:
            response = self.request({"command": kind, "query": {**(query or {}),
                "offset": offset, "limit": SCANNER_PAGE_SIZE}})
            payload = response["data"]
            if stamp is not None and payload["map"] != stamp:
                raise Stop("Map replaced during native scanner pagination")
            stamp = payload["map"]
            if kind == "scan_objects":
                current = payload["object_revision"]
                if revision is not None and current != revision:
                    raise Stop("Ground object mutation during scan; no mixed stack snapshot")
                revision = current
            page = payload["scan"]
            rows.extend(page["entries"])
            if len(rows) > MAX_SCANNER_ROWS:
                raise Stop("Native scan exceeds finite bound")
            following = page.get("next_offset")
            if following is None:
                return rows, stamp
            if following <= offset:
                raise Stop("Native scanner pagination did not advance")
            offset = following

    def open_window(self, destination, visible_component):
        if isinstance(destination, bool) or not isinstance(destination, int) or destination < NO_COUNT:
            raise Stop("Declared native window destination is absent or invalid")
        def opened(state):
            return self.client_bit(state, "legacy_selected_window") == destination + WINDOW_SELECTION_BIAS \
                and self.visible(state, visible_component)
        state = self.observe()
        if opened(state):
            return state
        parameter = self.symbols.get("param", "window_tab_destination")
        rows = self.children(self.symbols.component("window_buttons.actions"), parameters=(parameter,))
        matching = [row for row in rows if any(value.get("id") == parameter
            and value.get("present") is True and value.get("value", {}).get("kind") == "int"
            and value["value"]["value"] == destination
            for value in (row.get("value") or {}).get("parameters", []))]
        if len(matching) != SINGLE_MATCH:
            raise Stop("Loaded toolbar destination absent/ambiguous")
        operation, _ = self.operation(matching[FIRST_ENTRY], ("Open", "Close"))
        if operation != shared.FIRST_OPERATION:
            raise Stop("Native window selection has no unique first Open/Close operation")
        after = self.act(self.ui_action(matching[FIRST_ENTRY], ("Open", "Close")), state)
        return self.wait("native window selection and rooted content", opened, after)

    def open_backpack(self):
        return self.open_window(self.plan.get("toolbar", {}).get("backpackDestination"),
            "backpack.slots")

    def native_escape(self):
        state = self.observe()
        after = self.act({"kind": "key", "code": ESCAPE_AWT_CODE,
                          "pressed": True, "text": None}, state)
        self.act({"kind": "key", "code": ESCAPE_AWT_CODE,
                  "pressed": False, "text": None}, self.observe())
        return after

    def open_gameplay_settings(self):
        state = self.observe()
        if self.visible(state, "gameplay_settings.categories"):
            return state
        if not self.visible(state, "options_menu.settings_button"):
            after = self.native_escape()
            state = self.wait("ordinary Escape exposes native Settings button",
                lambda s: self.visible(s, "options_menu.settings_button"), after)
        row = self.component(state, "options_menu.settings_button")
        after = self.act(self.ui_action(row, ("Select",)), state)
        return self.wait("native Settings exposes gameplay categories",
            lambda s: self.visible(s, "gameplay_settings.categories"), after)

    def setting_row(self, root, index, choices, pending=False):
        if isinstance(index, bool) or not isinstance(index, int) or index < FIRST_ENTRY:
            raise Stop("cache-derived native setting child is invalid")
        parent = self.symbols.get("component", root)
        rows = self.children(self.symbols.component(root))
        matches = [row for row in rows if row["target"] == {"parent": parent, "child": index}]
        if len(matches) > SINGLE_MATCH:
            raise Stop(f"current native setting child is ambiguous: {root}:{index}")
        if not matches:
            if pending:
                return None
            raise Stop(f"current native setting child is absent: {root}:{index}")
        row = matches[FIRST_ENTRY]
        value = row.get("value") or {}
        ready = (value.get("rooted_visible") and value.get("onop")
                 and value.get("active_mask", FIRST_ENTRY) & (SINGLE_MATCH << shared.FIRST_OPERATION)
                 and any(label and shared.normal_text(label) in {shared.normal_text(choice) for choice in choices}
                         for label in value.get("ops") or []))
        if not ready:
            if pending:
                return None
            raise Stop("native Settings row lacks its current rooted server-operation grant")
        operation, _ = self.operation(row, choices)
        if operation != shared.FIRST_OPERATION:
            raise Stop("native Settings row lacks its cache-owned first operation")
        return row

    def setting_bit(self, route, name, expected, source_map):
        state = self.open_gameplay_settings()
        if state["map"] != source_map:
            raise Stop("source map changed while opening native Settings")
        if self.bit(state, name) == expected:
            return state
        category = route.get("category")
        if isinstance(category, bool) or not isinstance(category, int) or category < FIRST_ENTRY:
            raise Stop("cache-derived Settings category is invalid")
        path = route.get("selectionPath")
        if not isinstance(path, list) or not path or len(path) > shared.MAX_COMPONENT_DEPTH:
            raise Stop("cache-derived Settings selection path is absent or exceeds its finite bound")
        visited = set()
        for step in path:
            if not isinstance(step, dict) or any(isinstance(step.get(key), bool)
                    or not isinstance(step.get(key), int) or step[key] < FIRST_ENTRY
                    for key in ("index", "parent", "category")) or step["index"] in visited:
                raise Stop("cache-derived Settings selection path is invalid or cyclic")
            visited.add(step["index"])
        if path[-SINGLE_MATCH]["index"] != category or path[-SINGLE_MATCH]["category"] != category:
            raise Stop("cache-derived Settings path does not end at its setting category")
        parent = self.symbols.get("component", "gameplay_settings.categories")
        for position, step in enumerate(path):
            rows = self.children(self.symbols.component("gameplay_settings.categories"))
            state = self.observe()
            if state["map"] != source_map:
                raise Stop("source map changed while observing native Settings ancestry")
            def selectable(index):
                matches = [row for row in rows
                    if row["target"] == {"parent": parent, "child": index}]
                if len(matches) > SINGLE_MATCH:
                    raise Stop("current native Settings ancestry row is ambiguous")
                if not matches:
                    return None
                row = matches[FIRST_ENTRY]
                value = row.get("value") or {}
                if not value.get("rooted_visible") or not value.get("onop") \
                        or not value.get("active_mask", FIRST_ENTRY) & (SINGLE_MATCH << shared.FIRST_OPERATION):
                    return None
                labels = [label for label in value.get("ops") or []
                          if label and shared.normal_text(label) == shared.normal_text("Select")]
                if not labels:
                    return None
                operation, _ = self.operation(row, ("Select",))
                if operation != shared.FIRST_OPERATION:
                    raise Stop("native Settings ancestry lacks its cache-owned first operation")
                return row
            if position + SINGLE_MATCH < len(path):
                next_row = selectable(path[position + SINGLE_MATCH]["index"])
                if next_row is not None:
                    self.log("native_settings_ancestor_already_expanded", {"step": step,
                        "next": path[position + SINGLE_MATCH], "control": next_row,
                        "cycle": state["cycle"], "map": state["map"]})
                    continue
            row = selectable(step["index"])
            if row is None:
                raise Stop(f"cache-derived Settings ancestor is not currently Select-capable: {step}")
            after = self.act(self.ui_action(row, ("Select",)), state)
            def category_selected(observed):
                if observed["map"] != source_map:
                    raise Stop("source map changed while selecting native Settings category")
                return (self.bit(observed, "gameplay_settings_category") == step["category"]
                        and self.visible(observed, "gameplay_settings.options"))
            state = self.wait("native cache-derived Settings category published", category_selected, after)
            self.log("native_settings_category_result", {"step": step, "control": row,
                "cycle": state["cycle"], "map": state["map"],
                "category": self.bit(state, "gameplay_settings_category")})
        if self.bit(state, name) == expected:
            return state
        current = self.bit(state, name)
        if current not in (FIRST_ENTRY, SINGLE_MATCH):
            raise Stop("native Settings preference is absent or invalid")
        def toggle_ready(observed):
            if observed["map"] != source_map:
                raise Stop("source map changed while waiting for the native Settings operation grant")
            if self.bit(observed, "gameplay_settings_category") != category \
                    or not self.visible(observed, "gameplay_settings.options"):
                return False
            return self.setting_row("gameplay_settings.options", route.get("slot"),
                                    ("Toggle",), pending=True) is not None
        self.wait("native Settings server grants the current first Toggle", toggle_ready, state["cycle"])
        row = self.setting_row("gameplay_settings.options", route.get("slot"), ("Toggle",))
        state = self.observe()
        if state["map"] != source_map or self.bit(state, "gameplay_settings_category") != category \
                or not self.visible(state, "gameplay_settings.options"):
            raise Stop("native Settings category or source map changed before the single Toggle")
        current = self.bit(state, name)
        if current == expected:
            return state
        if current not in (FIRST_ENTRY, SINGLE_MATCH):
            raise Stop("native Settings preference is absent or invalid before the single Toggle")
        self.log("native_settings_toggle_ready", {"route": route, "control": row,
            "cycle": state["cycle"], "map": state["map"],
            "category": self.bit(state, "gameplay_settings_category")})
        after = self.act(self.ui_action(row, ("Toggle",)), state)
        def changed(observed):
            if observed["map"] != source_map:
                raise Stop("source map changed before native Settings preference publication")
            return self.bit(observed, name) == expected
        observed = self.wait(f"native Settings {name} publication", changed, after)
        self.log("native_settings_result", {"name": name, "before": current, "after": expected,
            "route": route, "control": row, "cycle": observed["cycle"], "map": observed["map"],
            "qualification": "cache-derived category/slot and current rooted native Toggle; no seeded preference"})
        return observed

    def slim_retaliation_pane(self):
        state = self.observe()
        source_map = state["map"]
        slim = self.bit(state, "slim_window_headers")
        fixed = self.bit(state, "legacy_interface_mode")
        if slim not in (FIRST_ENTRY, SINGLE_MATCH) or fixed not in (FIRST_ENTRY, SINGLE_MATCH):
            raise Stop("native Slim-header/layout preferences were not observed")
        if slim == SINGLE_MATCH:
            return state
        routes = self.plan.get("toolbar", {}).get("settings", {})
        for name in ("layout", "slim"):
            if not isinstance(routes.get(name), dict):
                raise Stop("initial plan lacks the named cache-derived Settings route")
        if fixed == SINGLE_MATCH:
            state = self.setting_bit(routes["layout"], "legacy_interface_mode", FIRST_ENTRY, source_map)
        state = self.setting_bit(routes["slim"], "slim_window_headers", SINGLE_MATCH, source_map)
        if fixed == SINGLE_MATCH:
            state = self.setting_bit(routes["layout"], "legacy_interface_mode", SINGLE_MATCH, source_map)
        if self.visible(state, "gameplay_settings.categories"):
            rows = self.children(self.symbols.component("game_window.modal_close_button"))
            parent = self.symbols.get("component", "game_window.modal_close_button")
            matches = [row for row in rows if row["target"] == {"parent": parent, "child": MODAL_CLOSE_CHILD}
                       and (row.get("value") or {}).get("rooted_visible")]
            if len(matches) != SINGLE_MATCH:
                raise Stop("native Settings modal Close child is absent/ambiguous")
            row = matches[FIRST_ENTRY]
            operation, _ = self.operation(row, ("Close",))
            if operation != shared.FIRST_OPERATION:
                raise Stop("native Settings Close lacks its ordinary first operation")
            after = self.act(self.ui_action(row, ("Close",)), state)
            state = self.wait("ordinary native Settings modal closes",
                lambda s: s["map"] == source_map and not self.visible(s, "gameplay_settings.categories"), after)
        if self.bit(state, "slim_window_headers") != SINGLE_MATCH or self.bit(state, "legacy_interface_mode") != fixed:
            raise Stop("native Slim/header route did not preserve the original layout")
        return state

    def retaliation_preference(self, state):
        rows = [row for row in state.get("varps", []) if row["id"] == self.retaliation_varp]
        if len(rows) != SINGLE_MATCH:
            return None
        return rows[FIRST_ENTRY].get("value")

    def transport_retaliation_off(self):
        """Only normal native input changes the preference; explicit Attack is unchanged."""
        state = self.observe()
        source_map = state["map"]
        before = self.retaliation_preference(state)

        def disabled(observed):
            if observed["map"] != source_map:
                raise Stop("Selected GWD map changed before retaliation OFF confirmation")
            return self.retaliation_preference(observed) == RETALIATION_OFF \
                and self.receipts.player().get("autoRetaliateDisabled") == RETALIATION_OFF

        def confirm(current, after, control=None):
            observed = current if disabled(current) else self.wait(
                "native and independent passive retaliation OFF", disabled, after)
            self.log("transport_retaliation_native_result", {"before": before,
                "after": self.retaliation_preference(observed), "control": control,
                "input_submitted": control is not None, "cycle": observed["cycle"],
                "map": observed["map"], "passive": self.receipts.player(),
                "qualification": "ordinary native UI preference; explicit Attack unchanged"})
            return observed

        if before == RETALIATION_OFF:
            return confirm(state, state["cycle"])
        if before != RETALIATION_ON:
            raise Stop("Actual native retaliation preference is absent or invalid")
        self.slim_retaliation_pane()
        state = self.open_window(self.plan["toolbar"].get("combatDestination"), RETALIATION_COMPONENT)
        if state["map"] != source_map:
            raise Stop("Selected GWD map changed while opening native Combat")
        current = self.retaliation_preference(state)
        if current == RETALIATION_OFF:
            return confirm(state, state["cycle"])
        if current != RETALIATION_ON:
            raise Stop("Actual native retaliation preference changed to an invalid value")
        row = self.component(state, RETALIATION_COMPONENT)
        value = (row or {}).get("value") or {}
        labels = value.get("ops") or []
        label = labels[FIRST_ENTRY] if labels else None
        if not value.get("rooted_visible") or not label \
                or not value.get("active_mask", NO_COUNT) & (SINGLE_MATCH << shared.FIRST_OPERATION):
            raise Stop("Loaded native retaliation toggle lacks rooted operation-one contract")
        action = {"kind": "ui", "target": row["target"], "serial": value["serial"],
            "operation": shared.FIRST_OPERATION, "expected_operation": label}
        after = self.act(action, state)
        observed = self.wait("normal native retaliation OFF before GWD transport", disabled, after)
        return confirm(observed, after, row)

    def prayers(self, enabled):
        self.open_window(self.plan["toolbar"]["prayerDestination"], PRAYER_COMPONENT_GROUP)
        for rule in self.plan["prayers"]:
            state = self.observe()
            expected = SINGLE_MATCH if enabled else NO_COUNT
            observed = self.bit(state, rule["activationBit"])
            if observed not in (NO_COUNT, SINGLE_MATCH):
                raise Stop("Requested Prayer PLAYER activation bit was not actually observed")
            if observed == expected:
                continue
            parent = self.symbols.get("component", PRAYER_COMPONENT_GROUP)
            rows = self.children(self.symbols.component(PRAYER_COMPONENT_GROUP))
            matching = [row for row in rows if row["target"] == {"parent": parent, "child": rule["button"]}
                and (row.get("value") or {}).get("rooted_visible")]
            if len(matching) != SINGLE_MATCH:
                raise Stop(f"Generated prayer button not actually loaded: {rule['name']}")
            row = matching[FIRST_ENTRY]
            verb = "Activate" if enabled else "Deactivate"
            requested = shared.normal_text(verb + " " + rule["name"])
            # Normalize colour markup only to identify the actual full native
            # label. Submit the untouched observed label/operation/serial.
            choices = tuple(label for label in row["value"].get("ops", []) if label
                and shared.normal_text(re.sub(r"</?col(?:=[^>]*)?>", "", label,
                    flags=re.IGNORECASE)) == requested)
            after = self.act(self.ui_action(row, choices), state)
            self.wait("native prayer activation-bit publication", lambda current:
                self.bit(current, rule["activationBit"]) == expected, after)

    def worn_count(self, state, item):
        families = [row for row in self.plan.get("physicalEquipment", []) if row["item"] == item]
        if len(families) > SINGLE_MATCH:
            raise Stop("Declared physical family is ambiguous")
        identities = {item}
        if families:
            identities.add(families[FIRST_ENTRY]["usedItem"])
        return sum(self.count(self.inventory(state, "worn_equipment"), value) for value in identities)

    def physical_wear_receipt(self, state):
        passive = self.receipts.player()
        saved = self.saved_state()
        evidence = []
        for family in self.plan.get("physicalEquipment", []):
            slot = family["slot"]
            actual = passive["worn"][slot]
            if actual[FIRST_ENTRY] != family["usedItem"] or actual[SINGLE_MATCH] != SINGLE_MATCH \
                    or len(actual) != PHYSICAL_INSTANCE_FIELD + SINGLE_MATCH:
                raise Stop("Ordinary physical armour did not reach its generated used identity")
            instance = actual[PHYSICAL_INSTANCE_FIELD]
            if not isinstance(instance, dict) or not isinstance(instance.get("key"), str) \
                    or type(instance.get("charges")) is not int \
                    or not NO_COUNT < instance["charges"] < family["freshCharges"]:
                raise Stop("Ordinary physical armour has no worn durable balance")
            balances = []
            for receipt in self.receipts.rows:
                if receipt.get("kind") != "state":
                    continue
                players = [player for player in receipt["players"] if player["pid"] == passive["pid"]]
                if len(players) != SINGLE_MATCH:
                    raise Stop("Physical wear observation lacks the exact player")
                worn = players[FIRST_ENTRY]["worn"][slot]
                if worn[FIRST_ENTRY] not in (family["item"], family["usedItem"]):
                    continue
                if len(worn) != PHYSICAL_INSTANCE_FIELD + SINGLE_MATCH \
                        or worn[PHYSICAL_INSTANCE_FIELD]["key"] != instance["key"]:
                    raise Stop("Ordinary physical armour identity changed or lost its durable metadata")
                balances.append(worn[PHYSICAL_INSTANCE_FIELD]["charges"])
            if not balances or not instance["charges"] < balances[FIRST_ENTRY] <= family["freshCharges"] \
                    or any(after > before for before, after in zip(balances, balances[SINGLE_MATCH:])) \
                    or saved["worn"][slot] != actual \
                    or self.count(self.inventory(state, "worn_equipment"), family["usedItem"]) != SINGLE_MATCH:
                raise Stop("Native used identity, observed monotone wear and saved physical balance differ")
            evidence.append({"family": family, "firstObservedCharges": balances[FIRST_ENTRY],
                "final": actual, "observations": len(balances), "saved": saved["worn"][slot]})
        return evidence

    def thrown_consumption_receipt(self, state):
        declared = self.plan.get("thrownAmmunition")
        if declared is None:
            return None
        observed = []
        for receipt in self.receipts.rows:
            if receipt.get("kind") != "state":
                continue
            passive = next(player for player in receipt["players"] if player["pid"] == self.receipts.player()["pid"])
            quantity = sum(slot[SINGLE_MATCH] for container in ("backpack", "worn")
                for slot in passive[container] if slot[FIRST_ENTRY] == declared["item"])
            if any(slot[FIRST_ENTRY] == declared["item"] for slot in passive["worn"]):
                observed.append(quantity)
        passive = self.receipts.player()
        saved = self.saved_state()
        final = sum(slot[SINGLE_MATCH] for container in ("backpack", "worn")
            for slot in passive[container] if slot[FIRST_ENTRY] == declared["item"])
        saved_final = sum(slot[SINGLE_MATCH] for container in ("backpack", "worn")
            for slot in saved[container] if slot[FIRST_ENTRY] == declared["item"])
        native_final = sum(self.count(self.inventory(state, container), declared["item"])
            for container in ("backpack", "worn_equipment"))
        if not observed or not any(NO_COUNT <= quantity < declared["count"] for quantity in observed) \
                or not NO_COUNT <= final < declared["count"] or saved_final != final or native_final != final:
            raise Stop("Ordinary thrown stack did not consume and persist through the native inventory owner")
        return {"declared": declared, "minimumObservedWornQuantity": min(observed),
            "final": final, "saved": saved_final, "native": native_final}

    def god_equipment_receipt(self, state):
        accessory = self.plan.get("godAccessory")
        if accessory is None:
            return None
        passive, saved = self.receipts.player(), self.saved_state()
        slot, item = accessory["slot"], accessory["item"]
        expected = [item, SINGLE_MATCH]
        if self.plan["faction"] not in accessory["factions"] \
                or passive["worn"][slot] != expected or saved["worn"][slot] != expected \
                or self.count(self.inventory(state, "worn_equipment"), item) != SINGLE_MATCH:
            raise Stop("Declared god accessory lacks actual native/passive/durable ordinary Wear")
        return {"declared": accessory, "passive": passive["worn"][slot],
            "saved": saved["worn"][slot], "nativeCount": SINGLE_MATCH}

    def item_control(self, state, root, item, choices):
        # Native inventory children already carry their own object and menu.
        # Use an admitted direct row before traversing unrelated leaf children.
        matching = []
        for row in self.children(self.symbols.component(root)):
            value = row.get("value") or {}
            if value.get("object") != item or not value.get("rooted_visible"):
                continue
            try:
                self.operation(row, choices)
            except Stop:
                continue
            matching.append(row)
        if matching:
            return sorted(matching, key=lambda row:
                (row["target"]["parent"], row["target"]["child"]))[FIRST_ENTRY]
        return super().item_control(state, root, item, choices)

    def equip(self):
        self.stage = "equip"
        items = [held["item"] if isinstance(held, dict) else held
            for held in self.plan.get("equipment", [])]
        self.equip_items(items)

    def equip_items(self, items):
        self.open_backpack()
        for item in items:
            # Ordinary login publication can select another native window after
            # the first opening. Resolve the current selection before each item.
            state = self.open_backpack()
            if self.worn_count(state, item):
                continue
            if not self.count(self.inventory(state, "backpack"), item):
                raise Stop("Declared ordinary equipment absent; no fixture insertion")
            row = self.item_control(state, "backpack.slots", item, ("Wear", "Wield"))
            after = self.act(self.ui_action(row, ("Wear", "Wield"), item), state)
            self.wait("actual captured native equipment transfer", lambda current, item=item:
                self.worn_count(current, item) > NO_COUNT, after)

    def route_rows(self, name):
        ids = self.plan["route"][name]
        by_id = {row["id"]: row for row in self.rules["crossings"]}
        if len(ids) != len(set(ids)) or any(key not in by_id for key in ids):
            raise Stop("Declared route does not map unique generated crossing identities")
        rows = [by_id[key] for key in ids]
        stage = "camp" if name == "camp" else "camp-return"
        if not rows or any(row.get("faction") != self.plan["faction"] or row["stage"] != stage for row in rows):
            raise Stop("Declared route crosses a different faction or phase")
        return rows

    def crossing(self, crossing, admitted=True):
        self.stage = "cross:" + crossing["id"]
        # The ordinary pathfinder chooses the approach. No actor relocation occurs here.
        side = crossing["from"]
        point = {"level": side["level"],
            "x": min(max(crossing["at"]["x"], side["x0"]), side["x1"]),
            "z": min(max(crossing["at"]["z"], side["z0"]), side["z1"])}
        self.walk(point, radius=ALTAR_APPROACH_TILES, allow_partial=True)
        state = self.observe()
        player = state["player"]
        if not (player["level"] == side["level"] and side["x0"] <= player["x"] <= side["x1"]
                and side["z0"] <= player["z"] <= side["z1"]):
            raise Stop("Pathfinder has not reached the generated source side")
        rows, stamp = self.scan("scan_locs", {"level": crossing["at"]["level"],
            "x": crossing["at"]["x"], "z": crossing["at"]["z"], "radius": NO_COUNT})
        matching = [row for row in rows if row["definition"] == crossing["loc"]
            or row["base_definition"] == crossing["loc"]]
        matching = [row for row in matching if shared.tile_key(row) == shared.tile_key(crossing["at"])]
        if len(matching) != SINGLE_MATCH or stamp != state["map"]:
            raise Stop("Actual generated crossing loc absent/ambiguous/stale")
        row = matching[FIRST_ENTRY]
        index = crossing["operation"] - SINGLE_MATCH
        labels = row.get("ops") or []
        if index < NO_COUNT or index >= len(labels) or not labels[index]:
            raise Stop("Generated operation has no actual native label")
        before = self.receipts.player()
        after = self.act(self.loc_action(row, labels[index]), state)
        expected = crossing["to"]["value"]
        if admitted:
            self.wait("generated native crossing landing", lambda current:
                shared.distance(current["player"], expected) == NO_COUNT, after, supervise_food=True)
        else:
            # Check the later server/native postcondition, not just retained-input acknowledgement.
            self.wait("low-count native chamber entry refused", lambda current:
                current["packets_applied"] > state["packets_applied"]
                and shared.distance(current["player"], expected) > NO_COUNT
                and self.receipts.player()["instance"] == before["instance"], after)
        self.log("crossing", {"id": crossing["id"], "admitted": admitted,
            "source": player, "expected": expected, "passive": self.receipts.player()})

    def faction_count(self, state):
        return self.bit(state, self.count_rule["varbit"])

    def saved_state(self):
        projection, _ = saved_projection.read_saved_projection(self.account_path, self.account_key)
        return projection

    def camp_eligible(self, definition, state):
        # This route chooses ordinary unrestricted camp creatures, rather than
        # creating a Slayer assignment, equipment unlock or skill fixture.
        rule = next((row for row in self.slayer_rules["monsters"]
            if row["npc"] == definition), None)
        if rule is None:
            return True
        stat = self.plan["campSelection"]["slayerStat"]
        levels = [row["value"]["base_level"] for row in state["stats"]
            if row["index"] == stat]
        if len(levels) != SINGLE_MATCH:
            raise Stop("Current native Slayer base-level observation absent/ambiguous")
        return levels[FIRST_ENTRY] >= rule["level"] and not any((
            rule.get("taskOnly"), rule.get("protection"), rule.get("gear"), rule.get("attacks")))

    def camp_approach_range(self, state):
        approach = self.plan["campApproach"]
        worn = self.inventory(state, "worn_equipment")
        slot = approach["mainHandSlot"]
        if isinstance(slot, bool) or not isinstance(slot, int) or not NO_COUNT <= slot < len(worn["items"]):
            raise Stop("Cache-derived main-hand observation is absent")
        item = worn["items"][slot]
        facts = [row for row in approach["weapons"] if row["item"] == item]
        if len(facts) != SINGLE_MATCH or not isinstance(facts[FIRST_ENTRY]["range"], int) \
                or facts[FIRST_ENTRY]["range"] < SINGLE_MATCH:
            raise Stop("Current native camp weapon lacks its declared ordinary range")
        return facts[FIRST_ENTRY]["range"]

    def camp_credit_possible(self, enemy, passive):
        # This is only a selection preference. Actual owner/count/body receipts
        # decide whether an ordinary contested camp death earned any credit.
        contributions = enemy.get("damageContributors")
        if not isinstance(contributions, list) or any(
                not isinstance(row, dict) or row.get("kind") not in ("player", "npc") or type(row.get("id")) is not int
                or type(row.get("total")) is not int or row["total"] < NO_COUNT
                for row in contributions):
            raise Stop("Current passive camp damage contributions are absent or invalid")
        own = sum(row["total"] for row in contributions
            if row["kind"] == "player" and row["id"] == passive["pid"])
        others = [row["total"] for row in contributions
            if row["kind"] != "player" or row["id"] != passive["pid"]]
        return own + enemy["hitpoints"] > max(others, default=NO_COUNT)

    def camp_actor(self):
        if self.camp_selection_deadline is not None:
            raise Stop("Nested camp selection cannot renew its original phase budget")
        self.camp_selection_deadline = min(self.deadline, time.monotonic() + self.args.action_seconds)
        try:
            return self.select_camp_actor()
        finally:
            self.camp_selection_deadline = None

    def select_camp_actor(self):
        # request() clamps every scan, Walk, food and supervised respawn wait to
        # this one original phase deadline, including inherited helper calls.
        camp_types = {row["npc"] for row in self.rules["factionNpcs"]
            if row["faction"] == self.plan["faction"]} - {
                role["type"] for arena in self.rules["arenas"] for role in arena["roles"]}
        rejected, explored, approached = set(), set(), set()
        phase_identity, phase_stamp = None, None

        def current_camp_context(current, stamp):
            nonlocal phase_identity, phase_stamp
            passive = self.receipts.player()
            identity = (passive["pid"], passive["generation"], passive["instance"])
            if not current.get("ready") or not current.get("terrain_present") \
                    or current.get("region") is not None or passive["life"] <= NO_COUNT \
                    or passive["instance"] is not None:
                raise Stop("Camp selection lost its positive ready normal-region player")
            if phase_identity is None:
                phase_identity, phase_stamp = identity, current["map"]
            context = ("session_instance", "connection_generation", "level", "width", "height")
            if identity != phase_identity or any(stamp[key] != phase_stamp[key] for key in context) \
                    or current["player"]["level"] != phase_stamp["level"] \
                    or stamp["terrain_generation"] < phase_stamp["terrain_generation"] \
                    or (stamp != phase_stamp and stamp["terrain_generation"] == phase_stamp["terrain_generation"]):
                raise Stop("Original camp player life/instance or session/connection/plane context changed")
            return passive

        while True:
            state = self.observe()
            passive = current_camp_context(state, state["map"])
            actors, stamp = self.scan("scan_npcs", {"level": state["player"]["level"]})
            current_camp_context(state, stamp)
            if stamp != state["map"]:
                self.log("camp_selection_discarded_rebuild", {"snapshot_map": state["map"],
                    "scan_map": stamp, "qualification": "Discard old selection; fresh observation under the same original phase deadline, no Attack"})
                continue
            if passive["instance"] is not None:
                raise Stop("Ordinary camp kill requires actual camp membership")
            enemies = (self.receipts.last_state or {}).get("enemies", [])
            native = actors
            actors = [actor for actor in native if actor["definition"] in camp_types
                and self.camp_eligible(actor["definition"], state)
                and str(actor["definition"]) in self.rules["campProfiles"]
                and any(enemy["id"] == actor["index"] and enemy["definition"] == actor["definition"]
                    and enemy["instance"] == passive["instance"] and enemy["visible"]
                    and NO_COUNT < enemy["hitpoints"] <= self.rules["campProfiles"][str(actor["definition"])]["hitpoints"]
                    and enemy["maximumLife"] == self.rules["campProfiles"][str(actor["definition"])]["hitpoints"]
                    and self.camp_credit_possible(enemy, passive)
                    and (enemy["id"], enemy["generation"]) not in rejected
                    and (enemy["id"], enemy["generation"], shared.tile_key(actor)) not in approached
                    for enemy in enemies)]
            if actors:
                actor = min(actors, key=lambda row: shared.distance(row, state["player"]))
                life = next(enemy for enemy in enemies if enemy["id"] == actor["index"])
                generation = life["generation"]
                player_identity = (passive["pid"], passive["generation"], passive["instance"])
                ordinary_range = self.camp_approach_range(state)
                # Approach through the ordinary retained Walk owner before the
                # first Attack. A failed approach never retries an accepted Attack.
                approached.add((actor["index"], generation, shared.tile_key(actor)))
                arrived = self.walk(actor, radius=CAMP_APPROACH_TILES, allow_partial=True)
                state = self.observe()
                current, current_map = self.scan("scan_npcs", {"definition": actor["definition"],
                    "level": actor["level"]})
                passive = current_camp_context(state, state["map"])
                current_camp_context(state, current_map)
                if (passive["pid"], passive["generation"], passive["instance"]) != player_identity:
                    raise Stop("Original camp player life/instance changed during Walk")
                if current_map != stamp or state["map"] != stamp:
                    self.log("camp_selection_discarded_rebuild", {"actor": actor,
                        "generation": generation, "original_map": stamp, "snapshot_map": state["map"],
                        "scan_map": current_map, "state": state,
                        "qualification": "Accepted ordinary Walk rebuilt its normal installed map; discard prior actor/stamp, reselect under the same original phase deadline, no Attack retry"})
                    continue
                fresh = [row for row in current if row["index"] == actor["index"]]
                lives = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
                    if enemy["id"] == actor["index"] and enemy["definition"] == actor["definition"]
                    and enemy["generation"] == generation and enemy["visible"]
                    and enemy["instance"] == passive["instance"]
                    and NO_COUNT < enemy["hitpoints"] <= self.rules["campProfiles"][str(actor["definition"])]["hitpoints"]
                    and enemy["maximumLife"] == self.rules["campProfiles"][str(actor["definition"])]["hitpoints"]]
                if not arrived or len(fresh) != SINGLE_MATCH or len(lives) != SINGLE_MATCH \
                        or passive["instance"] is not None or current_map != state["map"] \
                        or shared.distance(fresh[FIRST_ENTRY], state["player"]) > CAMP_APPROACH_TILES \
                        or state["player"]["route_length"] != NO_COUNT:
                    if arrived and len(fresh) == SINGLE_MATCH and len(lives) == SINGLE_MATCH \
                            and state["player"]["route_length"] == NO_COUNT \
                            and shared.tile_key(fresh[FIRST_ENTRY]) != shared.tile_key(actor) \
                            and (actor["index"], generation, shared.tile_key(fresh[FIRST_ENTRY])) not in approached:
                        self.log("camp_moved_life_replanned", {"actor": actor, "fresh_actor": fresh[FIRST_ENTRY],
                            "generation": generation, "state": state,
                            "qualification": "Successful ordinary Walk reached the prior destination; the same positive native life moved to a distinct observed tile, so reselect under the same phase deadline, no Attack"})
                        continue
                    rejected.add((actor["index"], generation))
                    self.log("camp_approach_rejected_before_attack", {"actor": actor,
                        "generation": generation, "ordinary_range": ordinary_range, "approach_tiles": CAMP_APPROACH_TILES, "state": state,
                        "qualification": "Normal physical approach did not establish the same positive visible life at ordinary physical adjacency; no NPC Attack was submitted"})
                    continue
                self.log("camp_approach_observed", {"actor": fresh[FIRST_ENTRY],
                    "generation": generation, "ordinary_range": ordinary_range, "approach_tiles": CAMP_APPROACH_TILES, "state": state,
                    "qualification": "Physical adjacency and fresh identity only; this does not establish LOS and actual subsequent combat must independently succeed"})
                return fresh[FIRST_ENTRY], self.rules["campProfiles"][str(actor["definition"])], state
            # A visible living camp actor is a navigation landmark only. The
            # next fresh roster must still establish bounded current health and physical
            # adjacency before select_camp_actor can return an Attack target.
            landmarks = [actor for actor in native
                if actor["definition"] in camp_types and self.camp_eligible(actor["definition"], state)
                and str(actor["definition"]) in self.rules["campProfiles"]
                and any(enemy["id"] == actor["index"] and enemy["definition"] == actor["definition"]
                    and enemy["instance"] == passive["instance"] and enemy["visible"]
                    and enemy["hitpoints"] > NO_COUNT
                    and (enemy["id"], enemy["generation"]) not in rejected | explored
                    and (enemy["id"], enemy["generation"], shared.tile_key(actor)) not in approached
                    for enemy in enemies)]
            if landmarks:
                landmark = min(landmarks, key=lambda row: shared.distance(row, state["player"]))
                life = next(enemy for enemy in enemies if enemy["id"] == landmark["index"])
                explored.add((landmark["index"], life["generation"]))
                approached.add((landmark["index"], life["generation"], shared.tile_key(landmark)))
                before = state
                arrived = self.walk(landmark, radius=CAMP_APPROACH_TILES, allow_partial=True)
                state = self.observe()
                current_camp_context(state, state["map"])
                self.log("camp_scout_observed", {"landmark": landmark, "generation": life["generation"],
                    "before": before, "state": state, "candidate_reached": arrived,
                    "actual_displaced": shared.tile_key(state["player"]) != shared.tile_key(before["player"]),
                    "qualification": "One ordinary Walk to a fresh native living camp landmark; navigation only, then discard prior roster and reselect under the same original phase deadline; no Attack"})
                continue
            def native_current_life(current):
                native, current_map = self.scan("scan_npcs", {"level": current["player"]["level"]})
                current_camp_context(current, current["map"])
                current_camp_context(current, current_map)
                if current_map != current["map"]:
                    self.log("camp_selection_discarded_rebuild", {"snapshot_map": current["map"],
                        "scan_map": current_map, "qualification": "Wait continues with fresh observation under the original phase deadline; no stale roster wake"})
                    return False
                return any(
                    enemy["definition"] in camp_types and enemy["visible"]
                    and self.camp_eligible(enemy["definition"], current)
                    and enemy["instance"] == self.receipts.player()["instance"]
                    and str(enemy["definition"]) in self.rules["campProfiles"]
                    and NO_COUNT < enemy["hitpoints"] <= self.rules["campProfiles"][str(enemy["definition"])]["hitpoints"]
                    and enemy["maximumLife"] == self.rules["campProfiles"][str(enemy["definition"])]["hitpoints"]
                    and self.camp_credit_possible(enemy, self.receipts.player())
                    and (enemy["id"], enemy["generation"]) not in rejected
                    and any(actor["index"] == enemy["id"] and actor["definition"] == enemy["definition"]
                        and actor["level"] == current["player"]["level"]
                        and (enemy["id"], enemy["generation"], shared.tile_key(actor)) not in approached
                        for actor in native)
                    for enemy in (self.receipts.last_state or {}).get("enemies", []))
            self.wait("ordinary positive current camp life", native_current_life, state["cycle"],
                supervise_food=True)

    def earn_rope_reset_witness(self):
        # The second real entry spent all forty. Earn one new count with the
        # same account and normal resources, then observe its ordinary save.
        state = self.observe()
        if self.faction_count(state) != NO_COUNT \
                or saved_projection.saved_count(self.saved_state(), self.count_rule) != NO_COUNT:
            raise Stop("Second entry did not leave the expected actual zero count")
        self.equip_items(self.plan.get("rejoinEquipment", self.plan["combatEquipment"]))
        while True:
            actor, profile, before_state = self.camp_actor()
            before = self.faction_count(before_state)
            if before != NO_COUNT:
                raise Stop("Unexpected camp count before the single rope witness kill")
            death = self.fight(actor, profile, ROPE_RESET_KILL_NAME, before_state["map"])
            if death is not None:
                break
        self.wait("ordinary camp kill publishes and saves the positive rope count", lambda current:
            self.faction_count(current) == SINGLE_MATCH
            and next(row["value"] for row in self.receipts.player()["counts"]
                if row["faction"] == self.plan["faction"]) == SINGLE_MATCH
            and saved_projection.saved_count(self.saved_state(), self.count_rule) == SINGLE_MATCH,
            self.observe()["cycle"], supervise_food=True)
        projection = self.saved_state()
        if saved_projection.saved_count(projection, self.count_rule) != SINGLE_MATCH:
            raise Stop("Positive count changed before its immutable save projection")
        saved_projection.write_new_projection(Path(self.args.journal).parent / ROPE_BEFORE_SAVED_FILE, projection)
        # Reuse the existing evidence schema; this is not part of the earlier
        # forty-kill replenishment or a synthetic counter event.
        self.log("rejoin_count_earned", {"name": ROPE_RESET_KILL_NAME, "before": before,
            "after": self.faction_count(self.state), "death": death, "state": self.state})

    def special_varp(self, state, field):
        identifier = self.special_policy[field]
        rows = [row for row in state.get("varps", []) if row["id"] == identifier]
        if len(rows) != SINGLE_MATCH or type(rows[FIRST_ENTRY].get("value")) is not int:
            raise Stop("Current native special varp is absent/ambiguous")
        return rows[FIRST_ENTRY]["value"]

    def special_guard(self, state, source_map, player_identity, armed):
        passive = self.receipts.player()
        slot = self.special_policy["mainHandSlot"]
        worn = self.inventory(state, "worn_equipment")
        energy = self.special_varp(state, "energyVarp")
        return state.get("ready") and state.get("focused") and state["map"] == source_map \
            and (passive["pid"], passive["generation"], passive["instance"]) == player_identity \
            and passive["life"] > NO_COUNT and self.bit(state, "current_life_points") > NO_COUNT \
            and self.bit(state, "legacy_combat_active") == SINGLE_MATCH \
            and state["player"]["target"] == NO_NATIVE_TARGET and passive.get("target") is None \
            and state["player"]["route_length"] == NO_COUNT \
            and NO_COUNT <= slot < len(worn["items"]) and slot < len(passive["worn"]) \
            and worn["items"][slot] == self.special_policy["weapon"] \
            and worn["counts"][slot] == SINGLE_MATCH \
            and passive["worn"][slot][:PHYSICAL_INSTANCE_FIELD] == [self.special_policy["weapon"], SINGLE_MATCH] \
            and self.special_policy["costFine"] <= energy <= self.special_policy["maximumEnergyFine"] \
            and self.special_varp(state, "armedVarp") == armed

    def special_row(self, state):
        target = self.symbols.component(SPECIAL_COMPONENT)
        rows = [self.component(state, SPECIAL_COMPONENT)] + self.children(target)
        matching = []
        for row in rows:
            value = (row or {}).get("value") or {}
            if not row or row["target"]["parent"] != target["parent"] \
                    or row["target"]["child"] not in (shared.STATIC_COMPONENT_CHILD, FIRST_ENTRY):
                continue
            ops = value.get("ops") or []
            if value.get("rooted_visible") and value.get("active_mask", NO_COUNT) & FIRST_OPERATION_EVENTS \
                    and ops and isinstance(ops[FIRST_ENTRY], str) and ops[FIRST_ENTRY].strip():
                matching.append((row, ops[FIRST_ENTRY]))
        if len(matching) > SINGLE_MATCH:
            raise Stop("Actual first-operation SGS component is ambiguous")
        return matching[FIRST_ENTRY] if matching else None

    def prepare_sara_special(self, row, state, passive, generation, fight_deadline):
        if self.special_attempted:
            raise Stop("The single declared Sara general special was already considered")
        self.special_attempted = True
        source_map = state["map"]
        player_identity = (passive["pid"], passive["generation"], passive["instance"])
        # Skipping an unavailable opportunity is not an energy refill or a
        # replacement Attack. An already armed unknown intent is refused.
        if self.special_varp(state, "armedVarp") != SPECIAL_UNARMED:
            raise Stop("SGS was already armed before the declared single intent")
        if not self.special_guard(state, source_map, player_identity, SPECIAL_UNARMED):
            self.log("sara_special_skipped", {"actor": row, "state": state,
                "reason": "Current idle/equipment/energy admission was not established; no special action"})
            return row, state, None
        self.special_preparation_deadline = min(fight_deadline,
            time.monotonic() + self.args.action_seconds)
        self.special_preparation_context = (source_map, player_identity, SPECIAL_UNARMED)
        try:
            # The existing native Eat owner may open Backpack; it does not
            # change the SGS weapon or implement any special healing itself.
            self.eat_if_needed(state)
            state = self.open_window(self.plan["toolbar"]["combatDestination"], SPECIAL_COMPONENT)
            observed = None
            def ready(current):
                nonlocal observed
                if not self.special_guard(current, source_map, player_identity, SPECIAL_UNARMED):
                    raise Stop("Original idle SGS admission changed during ordinary navigation")
                observed = self.special_row(current)
                return observed is not None
            state = self.wait("current rooted SGS first-operation server grant", ready, state["cycle"],
                deadline=self.special_preparation_deadline)
            selected, label = observed
            after = self.act(self.ui_action(selected, (label,)), state,
                guard=lambda fresh: self.special_guard(fresh, source_map, player_identity, SPECIAL_UNARMED))
            self.special_preparation_context = (source_map, player_identity, SPECIAL_ARMED)
            state = self.wait("single native SGS intent publishes armed state", lambda current:
                self.special_guard(current, source_map, player_identity, SPECIAL_ARMED), after,
                deadline=self.special_preparation_deadline)
            fresh, stamp = self.scan("scan_npcs", {"definition": row["definition"], "level": row["level"]})
            state = self.observe()
            matching = [actor for actor in fresh if actor["index"] == row["index"]
                and actor["definition"] == row["definition"] and actor["level"] == row["level"]]
            lives = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
                if enemy["id"] == row["index"] and enemy["definition"] == row["definition"]
                and enemy["generation"] == generation and enemy["instance"] == player_identity[PHYSICAL_INSTANCE_FIELD]
                and enemy.get("visible") and enemy["hitpoints"] > NO_COUNT]
            if stamp != source_map or len(matching) != SINGLE_MATCH or len(lives) != SINGLE_MATCH \
                    or not self.special_guard(state, source_map, player_identity, SPECIAL_ARMED):
                raise Stop("Original intended living general was not freshly observed after SGS UI")
            self.log("sara_special_armed", {"actor": matching[FIRST_ENTRY], "generation": generation,
                "selected_component": selected, "actual_label": label, "accepted_cycle": after,
                "state": state, "passive": self.receipts.player(),
                "proof": "Arming only; actual ordinary launch/energy/damage/healing remain in complete packets and passive receipts"})
            return matching[FIRST_ENTRY], state, lambda fresh: self.special_guard(
                fresh, source_map, player_identity, SPECIAL_ARMED)
        finally:
            self.special_preparation_deadline = None
            self.special_preparation_context = None

    def kite_guard(self, state):
        source_map, identity = self.kite_context
        passive = self.receipts.player()
        if not state.get("ready") or not state.get("focused") or state.get("map") != source_map \
                or (passive["pid"], passive["generation"], passive["instance"]) != identity \
                or passive["life"] <= NO_COUNT or not passive["membership"] \
                or passive["room"] != self.arena["roomId"] \
                or self.bit(state, "current_life_points") <= NO_COUNT:
            raise Stop("Original ranged movement map/player life/room admission changed")
        slot = self.kite_policy["mainHandSlot"]
        worn = self.inventory(state, "worn_equipment")
        if worn["items"][slot] != self.kite_policy["weapon"] or worn["counts"][slot] <= NO_COUNT \
                or passive["worn"][slot][FIRST_ENTRY] != self.kite_policy["weapon"]:
            raise Stop("Original ordinary ranged weapon/ammunition is no longer worn")
        return passive

    def kite_run_observation(self, state):
        passive = self.kite_guard(state)
        actual = passive.get("run")
        mode = [row.get("value") for row in state.get("varps", [])
            if row["id"] == self.kite_policy["runModeVarp"]]
        row = self.component(state, KITE_RUN_TEXT)
        value = (row or {}).get("value") or {}
        match = re.fullmatch(r"\s*(\d+)%\s*", value.get("text") or "")
        if not isinstance(actual, dict) or type(actual.get("enabled")) is not bool \
                or type(actual.get("energyFine")) is not int \
                or not NO_COUNT <= actual["energyFine"] <= self.kite_policy["runInitialFine"] \
                or len(mode) != SINGLE_MATCH or mode[FIRST_ENTRY] not in (NO_COUNT, SINGLE_MATCH) \
                or not value.get("rooted_visible") or match is None \
                or not NO_COUNT <= int(match.group(SINGLE_MATCH)) <= KITE_PERCENT_MAXIMUM:
            raise Stop("Current Run preference/text/balance is absent or invalid")
        if mode[FIRST_ENTRY] != int(actual["enabled"]) \
                or int(match.group(SINGLE_MATCH)) != actual["energyFine"] // self.kite_policy["runFinePerPercent"]:
            return None
        return actual

    def kite_run(self, state, enabled=None):
        actual = self.kite_run_observation(state)
        if actual is None:
            state = self.wait("current native/passive Run publication agrees", lambda current:
                self.kite_run_observation(current) is not None, state["cycle"], deadline=self.kite_deadline)
            actual = self.kite_run_observation(state)
        if enabled and actual["energyFine"] < self.kite_policy["runDrainFine"]:
            # Exhausted real energy uses the same ordinary Walk with Run OFF.
            enabled = False
            self.log("sara_kite_walking", {"run": actual,
                "meaning": "No recovery/refill assumption; ordinary one-step route fallback"})
        if enabled is None or actual["enabled"] == enabled:
            return state
        button = self.component(state, KITE_RUN_BUTTON)
        value = (button or {}).get("value") or {}
        labels = value.get("ops") or []
        label = labels[FIRST_ENTRY] if labels else None
        if not value.get("rooted_visible") or not value.get("onop") or not label \
                or not value.get("active_mask", NO_COUNT) & FIRST_OPERATION_EVENTS:
            raise Stop("Current Run button lacks its native first-operation grant")
        after = self.act(self.ui_action(button, (label,)), state)
        state = self.wait("one ordinary Run preference publication", lambda current:
            self.kite_run_observation(current) is not None
            and self.receipts.player()["run"]["enabled"] == enabled, after, deadline=self.kite_deadline)
        self.log("sara_kite_run", {"enabled": enabled, "actual_label": label,
            "accepted_cycle": after, "state": state, "run": self.receipts.player()["run"]})
        return state

    @staticmethod
    def kite_gap(player, actor):
        size = actor["size"]
        return max(max(actor["x"] - player["x"], NO_COUNT,
                player["x"] - (actor["x"] + size - SINGLE_MATCH)),
            max(actor["z"] - player["z"], NO_COUNT,
                player["z"] - (actor["z"] + size - SINGLE_MATCH)))

    def kite_walk(self, state, actor):
        fight_deadline = self.kite_deadline
        self.kite_deadline = min(fight_deadline, time.monotonic() + self.args.action_seconds)
        try:
            return self.kite_walk_in_action(state, actor)
        finally:
            self.kite_deadline = fight_deadline

    def kite_walk_in_action(self, state, actor):
        # Room-derived local waypoints are candidates, never a standability oracle.
        bounds = self.arena["bounds"]
        corners = [{"level": bounds["level"], "x": x, "z": z}
            for x in (bounds["x0"] + KITE_WALL_MARGIN, bounds["x1"] - KITE_WALL_MARGIN)
            for z in (bounds["z0"] + KITE_WALL_MARGIN, bounds["z1"] - KITE_WALL_MARGIN)]
        # Choose a distinct cadence-sized local segment, rather than a nearby
        # corner that the previous ordinary route may have stopped short of.
        candidates = []
        for corner in corners:
            span = shared.distance(state["player"], corner)
            if span < self.kite_policy["speedTicks"]:
                continue
            # Score the bounded hop itself, not the more distant corner.
            # The observed footprint can change while ordinary movement runs;
            # settlement and the next fresh native Attack still own admission.
            for hop in range(SINGLE_MATCH, min(self.kite_policy["speedTicks"], span) + SINGLE_MATCH):
                destination = {"level": corner["level"],
                    "x": state["player"]["x"] + round((corner["x"] - state["player"]["x"]) * hop / span),
                    "z": state["player"]["z"] + round((corner["z"] - state["player"]["z"]) * hop / span)}
                if KITE_MINIMUM_FOOTPRINT_GAP <= self.kite_gap(destination, actor) <= self.kite_policy["range"] \
                        and destination not in candidates:
                    candidates.append(destination)
        if not candidates:
            raise Stop("No distinct room-bound movement candidate in the observed weapon range")
        destination = max(candidates, key=lambda point:
            (self.kite_gap(point, actor), shared.distance(state["player"], point)))
        state = self.kite_run(state, True)
        before = (shared.tile_key(state["player"]), state["player"]["fine_x"], state["player"]["fine_z"])
        start_tick = self.receipts.last_state["tick"]
        burst_ticks = max(SINGLE_MATCH, self.kite_policy["speedTicks"] // KITE_RUN_BURST_DIVISOR)
        self.walk_probes += SINGLE_MATCH
        if self.walk_probes > shared.MAX_WALK_PROBES:
            raise Stop("Bounded ordinary route probes exhausted")
        after = self.act({"kind": "walk", "x": destination["x"], "z": destination["z"], "run": False}, state)
        previous, settled, changed = None, NO_COUNT, time.monotonic()
        disabled = False
        while time.monotonic() < self.kite_deadline:
            state = self.observe(deadline=self.kite_deadline)
            self.kite_guard(state)
            actual_run = self.receipts.player()["run"]
            if not disabled and (self.receipts.last_state["tick"] >= start_tick + burst_ticks
                    or actual_run["energyFine"] < self.kite_policy["runDrainFine"]):
                # A normal toggle changes this same queued route; no Walk retry.
                state = self.kite_run(state, False)
                disabled = True
            player = state["player"]
            identity = (shared.tile_key(player), player["fine_x"], player["fine_z"])
            if identity != previous:
                changed, settled = time.monotonic(), NO_COUNT
            elif state["cycle"] > after:
                settled += SINGLE_MATCH
            previous = identity
            passive = self.receipts.player()
            # The normal route may move near a candidate that is not standable.
            # Accept only real, independently settled displacement in this room;
            # never call a stopped-short endpoint the requested destination.
            if player["route_length"] == NO_COUNT \
                    and player["target"] == NO_NATIVE_TARGET and passive["target"] is None \
                    and shared.tile_key(player) == shared.tile_key(passive) \
                    and player["level"] == bounds["level"] \
                    and bounds["x0"] <= player["x"] <= bounds["x1"] \
                    and bounds["z0"] <= player["z"] <= bounds["z1"] \
                    and settled >= KITE_SETTLED_SAMPLES and identity != before \
                    and shared.tile_key(player) != before[FIRST_ENTRY]:
                state = self.kite_run(state, False)
                self.log("sara_kite_walk_settled", {"destination": destination,
                    "actual_endpoint": {key: player[key] for key in ("level", "x", "z")},
                    "candidate_reached": shared.distance(player, destination) == NO_COUNT,
                    "accepted_cycle": after, "before_physical": before, "state": state,
                    "run": self.receipts.player()["run"], "burst_ticks": burst_ticks})
                return state
            if time.monotonic() - changed > self.args.stall_seconds:
                raise Stop("Ordinary kite route stopped; no accepted Walk retry")
            self.eat_if_needed(state)
            time.sleep(shared.POLL_SECONDS)
        raise Stop("Original Sara fight deadline expired during normal movement")

    def fight_sara_kite(self, row, profile, name, selected_map):
        self.stage = "fight:" + name
        state = self.observe()
        passive = self.receipts.player()
        identity = (passive["pid"], passive["generation"], passive["instance"])
        self.kite_deadline = min(self.deadline, time.monotonic() + self.args.fight_seconds)
        self.kite_context = (selected_map, identity)
        try:
            self.kite_guard(state)
            matches = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
                if enemy["id"] == row["index"] and enemy["definition"] == row["definition"]
                and enemy["instance"] == identity[PHYSICAL_INSTANCE_FIELD] and enemy.get("visible")
                and enemy["hitpoints"] > NO_COUNT]
            if len(matches) != SINGLE_MATCH:
                raise Stop("Selected kite actor lacks its independent positive current life")
            generation = matches[FIRST_ENTRY]["generation"]
            full = [{**enemy, "tick": receipt["tick"]} for receipt in self.receipts.rows
                if receipt.get("kind") == "state" for enemy in receipt.get("enemies", [])
                if enemy["id"] == row["index"] and enemy["definition"] == row["definition"]
                and enemy["generation"] == generation and enemy["instance"] == identity[PHYSICAL_INSTANCE_FIELD]
                and enemy["hitpoints"] == profile["hitpoints"]]
            if not full:
                raise Stop("Actual full-health Sara publication absent")
            self.full_health[name] = full[FIRST_ENTRY]
            offset = len(self.receipts.rows)
            launched = NO_COUNT
            last_launch_tick, last_attack_cycle = None, state["cycle"]
            initial_gap = self.kite_gap(state["player"], {**matches[FIRST_ENTRY], "size": row["size"]})
            moved = KITE_MINIMUM_FOOTPRINT_GAP <= initial_gap <= self.kite_policy["range"]
            awaiting_launch = False
            refreshes = NO_COUNT
            prior_hp, changed = matches[FIRST_ENTRY]["hitpoints"], time.monotonic()
            while time.monotonic() < self.kite_deadline:
                state = self.observe(deadline=self.kite_deadline)
                self.kite_guard(state)
                deaths = [receipt for receipt in self.receipts.rows[offset:] if receipt.get("kind") == "death"
                    and (receipt.get("target") or {}).get("id") == row["index"]
                    and receipt["target"].get("definition") == row["definition"]
                    and receipt["target"].get("generation") == generation
                    and receipt["target"].get("instance") == identity[PHYSICAL_INSTANCE_FIELD]
                    and (receipt.get("source") or {}).get("kind") == "player"
                    and receipt["source"].get("id") == identity[FIRST_ENTRY]
                    and receipt["source"].get("generation") == identity[SINGLE_MATCH]]
                if state["cycle"] > last_attack_cycle and len(deaths) == SINGLE_MATCH:
                    self.kite_run(state, False)
                    self.completed.add(name)
                    self.log("credited_death", {"name": name, "full_health": full[FIRST_ENTRY],
                        "death": deaths[FIRST_ENTRY], "cycle": state["cycle"]})
                    return deaths[FIRST_ENTRY]
                lives = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
                    if enemy["id"] == row["index"] and enemy["definition"] == row["definition"]
                    and enemy["generation"] == generation and enemy["instance"] == identity[PHYSICAL_INSTANCE_FIELD]
                    and enemy.get("visible") and enemy["hitpoints"] > NO_COUNT]
                if len(lives) != SINGLE_MATCH:
                    raise Stop("Original kite NPC life disappeared without its credited death")
                actor = {**lives[FIRST_ENTRY], "size": row["size"]}
                if actor["hitpoints"] != prior_hp:
                    prior_hp, changed = actor["hitpoints"], time.monotonic()
                if time.monotonic() - changed > self.args.stall_seconds:
                    raise Stop("Ordinary kite fight stalled; no accepted Attack retry")
                flights = [receipt for receipt in self.receipts.rows[offset:] if receipt.get("kind") == "launch"
                    and receipt.get("style") == "ranged" and receipt.get("source", {}).get("kind") == "player"
                    and receipt["source"].get("id") == identity[FIRST_ENTRY]
                    and receipt["source"].get("generation") == identity[SINGLE_MATCH]
                    and receipt.get("target", {}).get("id") == row["index"]
                    and receipt["target"].get("definition") == row["definition"]
                    and receipt["target"].get("generation") == generation
                    and receipt["target"].get("instance") == identity[PHYSICAL_INSTANCE_FIELD]]
                if len(flights) > launched:
                    self.log("sara_kite_flight", {"actual_launches": flights[launched:],
                        "accepted_attack_cycle": last_attack_cycle})
                    launched, last_launch_tick = len(flights), flights[-SINGLE_MATCH]["tick"]
                    awaiting_launch, moved = False, False
                if not moved and not awaiting_launch:
                    state = self.kite_walk(state, actor)
                    moved = True
                    continue
                if awaiting_launch:
                    # A real accepted input without an observed shot is never reissued.
                    self.eat_if_needed(state)
                    time.sleep(shared.POLL_SECONDS)
                    continue
                if last_launch_tick is not None and self.receipts.last_state["tick"] \
                        < last_launch_tick + self.kite_policy["speedTicks"]:
                    self.eat_if_needed(state)
                    time.sleep(shared.POLL_SECONDS)
                    continue
                gap = self.kite_gap(state["player"], actor)
                if gap < KITE_MINIMUM_FOOTPRINT_GAP:
                    moved = False
                    continue
                if gap > self.kite_policy["range"]:
                    moved = False
                    continue
                self.eat_if_needed(state)
                state = self.observe(deadline=self.kite_deadline)
                self.kite_guard(state)
                rows, stamp = self.scan("scan_npcs", {"definition": row["definition"], "level": row["level"]})
                matching = [current for current in rows if current["index"] == row["index"]
                    and current["definition"] == row["definition"] and current["level"] == row["level"]]
                if stamp != selected_map or len(matching) != SINGLE_MATCH:
                    raise Stop("Exact kite actor is not in the original fresh scanner map")
                fresh = matching[FIRST_ENTRY]
                self.kite_guard(state)
                fresh_lives = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
                    if enemy["id"] == row["index"] and enemy["definition"] == row["definition"]
                    and enemy["generation"] == generation and enemy["instance"] == identity[PHYSICAL_INSTANCE_FIELD]
                    and enemy.get("visible") and enemy["hitpoints"] > NO_COUNT]
                if len(fresh_lives) != SINGLE_MATCH:
                    raise Stop("Original positive kite NPC life changed during the fresh scan")
                if fresh["size"] != row["size"]:
                    raise Stop("Original kite NPC footprint size changed during the fresh scan")
                fresh_gap = self.kite_gap(state["player"], fresh)
                if not KITE_MINIMUM_FOOTPRINT_GAP <= fresh_gap <= self.kite_policy["range"]:
                    self.log("sara_kite_gap_reselected", {"actor": fresh, "state": state,
                        "gap": fresh_gap, "scan_cycle": self.last_scans["scan_npcs"]["response"]["cycle"],
                        "meaning": "No Attack submitted; observe or make a distinct movement under the original fight deadline"})
                    moved = False
                    continue
                try:
                    last_attack_cycle = self.act_scanned_npc(fresh, state, selected_map,
                        guard=lambda current: bool(self.kite_guard(current)))
                except UnacceptedNpcIntent as refusal:
                    if refreshes >= MAXIMUM_NPC_INTENT_REFRESHES:
                        raise
                    refreshes += SINGLE_MATCH
                    self.log("unaccepted_attack_refreshed", {"name": name, "refusal": refusal.response,
                        "maximum_refreshes": MAXIMUM_NPC_INTENT_REFRESHES})
                    # Retry only this precise unaccepted admission; next loop rescans, not a new Walk.
                    continue
                # The bounded refresh belongs to this one unaccepted intent.
                # A later shot follows an actual launch and distinct movement;
                # its ordinary admission is a new intent, never an accepted retry.
                refreshes = NO_COUNT
                awaiting_launch = True
            raise Stop("Original Sara fight deadline exhausted; no resource or difficulty change")
        finally:
            self.kite_context, self.kite_deadline = None, None

    def fight(self, row, profile, name, selected_map):
        if self.kite_policy is not None and name == self.kite_policy["role"]:
            return self.fight_sara_kite(row, profile, name, selected_map)
        self.stage = "fight:" + name
        state = self.observe()
        if state["map"] != selected_map:
            raise Stop("Map changed after selecting the intended GWD fight actor")
        passive = self.receipts.player()
        matches = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
            if enemy["id"] == row["index"] and enemy["definition"] == row["definition"]
            and enemy["instance"] == passive["instance"] and enemy.get("visible")
            and enemy["hitpoints"] > NO_COUNT]
        if len(matches) != SINGLE_MATCH:
            raise Stop("Observed NPC lacks an independent current-life match")
        generation = matches[FIRST_ENTRY]["generation"]
        is_camp = name.startswith(("camp:", "rejoin-camp:", "exit-camp:"))
        initial_current_life = {**matches[FIRST_ENTRY], "tick": self.receipts.last_state["tick"]}
        if is_camp:
            self.camp_attempts += SINGLE_MATCH
            attempt_name = name + ":attempt:" + str(self.camp_attempts)
            before_count = self.faction_count(state)
            if not NO_COUNT < initial_current_life["hitpoints"] <= profile["hitpoints"] \
                    or initial_current_life["maximumLife"] != profile["hitpoints"] \
                    or initial_current_life.get("configuredRole") is not False \
                    or initial_current_life["instance"] is not None:
                raise Stop("Ordinary camp life is not positive and bounded by its configured maximum")
            initial_current_life["configuredMaximumLife"] = profile["hitpoints"]
        else:
            full = [{**enemy, "tick": receipt["tick"]} for receipt in self.receipts.rows
                if receipt.get("kind") == "state" for enemy in receipt.get("enemies", [])
                if enemy["id"] == row["index"] and enemy["definition"] == row["definition"]
                and enemy["generation"] == generation and enemy["instance"] == passive["instance"]
                and enemy["hitpoints"] == profile["hitpoints"]]
            if not full:
                raise Stop(f"Actual full-health {name} publication absent; no weakened fixture allowed")
            self.full_health[name] = full[FIRST_ENTRY]
        offset = len(self.receipts.rows)
        deadline, attack_guard = None, None
        if self.special_policy is not None and name == self.special_policy["role"]:
            deadline = min(self.deadline, time.monotonic() + self.args.fight_seconds)
            row, state, attack_guard = self.prepare_sara_special(row, state, passive, generation, deadline)
        # Lifecycle, full-health provenance and any equipped/armed special
        # guard are captured before this scan. Volatile NPC serials go directly
        # from the actual scan response into ordinary live native admission.
        if attack_guard is not None and not attack_guard(state):
            raise Stop("Original SGS admission changed before the intended NPC scan")
        fresh_rows, stamp = self.scan("scan_npcs", {"definition": row["definition"],
            "level": row["level"]})
        matching = [actor for actor in fresh_rows if actor["index"] == row["index"]
            and actor["definition"] == row["definition"] and actor["level"] == row["level"]]
        current_player = self.receipts.player()
        lives = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
            if enemy["id"] == row["index"] and enemy["definition"] == row["definition"]
            and enemy["generation"] == generation and enemy["instance"] == passive["instance"]
            and enemy.get("visible") and enemy["hitpoints"] > NO_COUNT]
        if stamp != selected_map or len(matching) != SINGLE_MATCH or len(lives) != SINGLE_MATCH \
                or (current_player["pid"], current_player["generation"], current_player["instance"]) \
                    != (passive["pid"], passive["generation"], passive["instance"]):
            raise Stop("Original intended NPC/player life or selected map changed before Attack")
        row = matching[FIRST_ENTRY]
        try:
            after = self.act_scanned_npc(row, state, selected_map, guard=attack_guard)
        except UnacceptedNpcIntent as refusal:
            # Refresh exactly once, only after this precise unaccepted native
            # intent. A received accepted Attack is never retried.
            state = self.observe()
            fresh_rows, stamp = self.scan("scan_npcs", {"definition": row["definition"],
                "level": state["player"]["level"]})
            if state["map"] != selected_map or stamp != selected_map:
                raise Stop("Selected GWD map changed after the refused NPC intent") from refusal
            matching = [actor for actor in fresh_rows if actor["index"] == row["index"]
                and actor["definition"] == row["definition"] and actor["level"] == row["level"]]
            current_player = self.receipts.player()
            lives = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
                if enemy["id"] == row["index"] and enemy["definition"] == row["definition"]
                and enemy["generation"] == generation and enemy["instance"] == passive["instance"]
                and enemy.get("visible") and enemy["hitpoints"] > NO_COUNT]
            if len(matching) != SINGLE_MATCH or len(lives) != SINGLE_MATCH \
                    or (current_player["pid"], current_player["generation"], current_player["instance"]) \
                        != (passive["pid"], passive["generation"], passive["instance"]):
                raise Stop("The same intended GWD positive visible life/instance was not re-observed") from refusal
            row = matching[FIRST_ENTRY]
            _, operation = self.operation(row, ("Attack",))
            self.log("unaccepted_attack_refreshed", {"name": name, "refusal": refusal.response,
                "actor": row, "map": stamp, "passive_generation": generation,
                "maximum_refreshes": MAXIMUM_NPC_INTENT_REFRESHES})
            after = self.act_scanned_npc(row, state, selected_map, guard=attack_guard)
        if deadline is None:
            deadline = min(self.deadline, time.monotonic() + self.args.fight_seconds)
        prior_hp, changed = matches[FIRST_ENTRY]["hitpoints"], time.monotonic()
        while time.monotonic() < deadline:
            state = self.observe()
            self.receipts.refresh()
            deaths = [receipt for receipt in self.receipts.rows[offset:] if receipt.get("kind") == "death"
                and (receipt.get("target") or {}).get("id") == row["index"]
                and receipt["target"].get("definition") == row["definition"]
                and receipt["target"].get("generation") == generation
                and (receipt.get("source") or {}).get("kind") == "player"
                and receipt["source"].get("id") == passive["pid"]]
            if state.get("ready") and state["cycle"] > after and len(deaths) == SINGLE_MATCH:
                if is_camp:
                    owners = [receipt for receipt in self.receipts.rows[offset:]
                        if receipt.get("kind") == "gwd_reward_owner"
                        and receipt.get("target", {}).get("id") == row["index"]
                        and receipt["target"].get("definition") == row["definition"]
                        and receipt["target"].get("generation") == generation
                        and receipt["target"].get("instance") == passive["instance"]
                        and receipt["tick"] == deaths[FIRST_ENTRY]["tick"]]
                    if len(owners) != SINGLE_MATCH:
                        raise Stop("Ordinary camp death lacks its exact actual reward-owner receipt")
                    owner = owners[FIRST_ENTRY].get("owner")
                    if owner is not None and (not isinstance(owner, dict) or owner.get("kind") != "player"
                            or type(owner.get("id")) is not int or type(owner.get("generation")) is not int
                            or owner.get("instance") != passive["instance"]):
                        raise Stop("Actual camp reward owner is not a valid same-instance player or null")
                    credited = owner is not None and owner.get("kind") == "player" \
                        and owner.get("id") == passive["pid"] and owner.get("generation") == passive["generation"]
                    expected_count = before_count + (SINGLE_MATCH if credited else NO_COUNT)
                    death = deaths[FIRST_ENTRY]
                    def body_and_count(current):
                        current_player = self.receipts.player()
                        if (current_player["pid"], current_player["generation"], current_player["instance"]) \
                                != (passive["pid"], passive["generation"], passive["instance"]) \
                                or current_player["life"] <= NO_COUNT:
                            raise Stop("Original camp player life or instance changed during body/count closure")
                        return current.get("ready") and self.receipts.last_state["tick"] >= death["hideTick"] \
                            and not any(enemy["id"] == row["index"] and enemy["generation"] == generation and enemy["visible"]
                                for enemy in self.receipts.last_state.get("enemies", [])) \
                            and self.faction_count(current) == expected_count \
                            and next(value["value"] for value in current_player["counts"]
                                if value["faction"] == self.plan["faction"]) == expected_count \
                            and saved_projection.saved_count(self.saved_state(), self.count_rule) == expected_count
                    self.wait("actual camp reward owner, normal body hide and native/saved count", body_and_count,
                        state["cycle"], supervise_food=True)
                    self.log("credited_death" if credited else "camp_uncredited_final_blow", {
                        "name": name, "attempt_name": attempt_name, "initial_current_life": initial_current_life,
                        "death": death, "reward_owner": owners[FIRST_ENTRY], "before_count": before_count,
                        "after_count": self.faction_count(self.state), "cycle": self.state["cycle"]})
                    if not credited:
                        return None
                    self.completed.add(name)
                    return death
                self.completed.add(name)
                self.log("credited_death", {"name": name, "full_health": full[FIRST_ENTRY],
                    "death": deaths[FIRST_ENTRY], "cycle": state["cycle"]})
                return deaths[FIRST_ENTRY]
            current = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
                if enemy["id"] == row["index"] and enemy["generation"] == generation]
            if current and current[FIRST_ENTRY]["hitpoints"] != prior_hp:
                prior_hp, changed = current[FIRST_ENTRY]["hitpoints"], time.monotonic()
            if time.monotonic() - changed > self.args.stall_seconds:
                raise Stop(f"Ordinary fight stalled: {name}; no blind attack spam or fixture rescue")
            if state.get("ready"):
                self.eat_if_needed(state)
            time.sleep(shared.POLL_SECONDS)
        raise Stop(f"Ordinary {name} fight deadline; no RNG/HP/gear mutation permitted")

    def take(self, death, prior_tokens):
        self.stage = "loot"
        tile = death["target"]
        self.walk(tile, radius=ALTAR_APPROACH_TILES, allow_partial=True)
        state = self.observe()
        rows, stamp = self.scan("scan_objects", {"x": tile["x"], "z": tile["z"],
            "level": tile["level"], "radius": LOOT_RADIUS_TILES})
        passive = self.receipts.player()
        choices = []
        for row in rows:
            same = [other for other in rows if shared.tile_key(other) == shared.tile_key(row)
                and other["definition"] == row["definition"]]
            server = [item for item in passive["ground"] if item["item"] == row["definition"]
                and item["count"] == row["count"] and shared.tile_key(item) == shared.tile_key(row)]
            if len(same) == len(server) == SINGLE_MATCH \
                    and server[FIRST_ENTRY]["token"] not in prior_tokens \
                    and shared.tile_key(row) == shared.tile_key(tile) and row["definition"] not in self.plan.get("recoveredAmmunition", []):
                choices.append((row, server[FIRST_ENTRY]))
        if not choices or stamp != state["map"]:
            raise Stop("No actual unambiguous private ground stack; no invented drop")
        row, receipt = min(choices, key=lambda choice: shared.distance(choice[FIRST_ENTRY], tile))
        before = self.count(self.inventory(state, "backpack"), row["definition"])
        # The full scan selected the unique newly published corpse reward.
        # Refresh only that exact item/tile after approach; recovered projectiles
        # can mutate the global object revision. No snapshot follows this scan.
        current, current_map = self.scan("scan_objects", {"definition": row["definition"],
            "x": row["x"], "z": row["z"], "level": row["level"], "radius": NO_COUNT})
        matching = [entry for entry in current if entry["definition"] == row["definition"]
            and entry["count"] == row["count"] and shared.tile_key(entry) == shared.tile_key(row)]
        current_player = self.receipts.player()
        tokens = [item for item in current_player["ground"] if item["token"] == receipt["token"]
            and item["item"] == row["definition"] and item["count"] == row["count"]
            and shared.tile_key(item) == shared.tile_key(row)]
        if current_map != stamp or len(matching) != SINGLE_MATCH or len(tokens) != SINGLE_MATCH \
                or (current_player["pid"], current_player["generation"], current_player["instance"]) \
                    != (passive["pid"], passive["generation"], passive["instance"]):
            raise Stop("Original corpse reward token or player life/map changed before native Take")
        row = matching[FIRST_ENTRY]
        after = self.act_scanned_object(row, state, stamp)
        self.wait("native Take inventory/ground receipt", lambda current:
            self.count(self.inventory(current, "backpack"), row["definition"]) == before + row["count"]
            and not any(item["token"] == receipt["token"] for item in self.receipts.player()["ground"]),
            after, supervise_food=True)
        remaining, _ = self.scan("scan_objects", {"definition": row["definition"],
            "x": row["x"], "z": row["z"], "level": row["level"], "radius": NO_COUNT})
        if remaining:
            raise Stop("Taken exact ground definition/tile still decoded")
        self.log("taken", {"observed": row, "independent": receipt, "before_count": before,
            "after_count": before + row["count"]})

    def eat_if_needed(self, state):
        policy = self.plan.get("roomFoodPolicy")
        passive = self.receipts.player()
        minimum = policy["minimumLife"] if policy and passive.get("room") == self.arena["roomId"] \
            and passive.get("membership") else None
        return super().eat_if_needed(state, minimum_life=minimum)

    @staticmethod
    def installed_tile(state, tile):
        area = state["map"]
        return tile["level"] == area["level"] \
            and area["base_x"] <= tile["x"] < area["base_x"] + area["width"] \
            and area["base_z"] <= tile["z"] < area["base_z"] + area["height"]

    def approach_altar(self, destination):
        state = self.observe()
        if self.installed_tile(state, destination):
            return self.walk(destination, radius=ALTAR_APPROACH_TILES, allow_partial=True)
        if self.altar_approach_deadline is not None:
            raise Stop("Nested altar frontier approach cannot renew its original deadline")
        original = self.receipts.player()
        identity = tuple(original.get(key) for key in ("pid", "generation", "instance"))
        area = state["map"]
        if destination["level"] != area["level"] or not state.get("ready") or not state.get("focused"):
            raise Stop("Altar frontier requires the original ready same-plane map")
        frontier = {"level": destination["level"],
            "x": min(max(destination["x"], area["base_x"]),
                area["base_x"] + area["width"] - SINGLE_MATCH),
            "z": min(max(destination["z"], area["base_z"]),
                area["base_z"] + area["height"] - SINGLE_MATCH)}
        self.altar_approach_deadline = min(self.deadline, time.monotonic() + self.args.action_seconds)
        try:
            self.walk(frontier, radius=ALTAR_APPROACH_TILES, allow_partial=True)
            current = self.observe()
            player = self.receipts.player()
            if tuple(player.get(key) for key in ("pid", "generation", "instance")) != identity \
                    or player["life"] <= NO_COUNT \
                    or any(current["map"].get(key) != area.get(key)
                        for key in ("session_instance", "connection_generation")) \
                    or not current.get("ready") or not current.get("focused") \
                    or not self.installed_tile(current, destination):
                raise Stop("Ordinary frontier Walk did not retain life/instance and install the altar")
            self.log("altar_frontier_walk", {"frontier": frontier, "destination": destination,
                "before_map": area, "state": current, "passive": player})
            return self.walk(destination, radius=ALTAR_APPROACH_TILES, allow_partial=True)
        finally:
            self.altar_approach_deadline = None

    def altar(self, leave=True):
        self.stage = "altar"
        self.prayers(False)
        altar = self.arena["altar"]
        self.approach_altar(altar["at"])
        row, _ = self.select_loc({altar["loc"]}, altar["at"], ("Pray-at",))
        # select_loc retains its observation map in self.state; act refreshes
        # only the admission cycle without substituting a new selection map.
        before = self.state
        after = self.act(self.loc_action(row, "Pray-at"), before)
        passive = self.receipts.player()
        matching = {row["item"] for row in self.rules["godItems"]
            if self.plan["faction"] in row["altarFactions"]}
        # Inspect current worn slots after real native Wield. No plan-only gear claim.
        item_count = sum(SINGLE_MATCH for slot in passive["worn"] if slot and slot[FIRST_ENTRY] in matching)
        bonus = item_count * altar["bonusFinePerItem"]["value"]
        expected = passive["maximumPrayerFine"] + bonus
        self.wait("native altar current Prayer and saved allowance", lambda current:
            self.bit(current, "current_prayer_points") == expected
            and self.receipts.player()["prayerFine"] == expected
            and self.receipts.player()["prayerAllowanceFine"] == bonus, after)
        self.log("altar_restored", {"expected": expected, "allowance": bonus,
            "matching_worn_count": item_count, "state": self.state,
            "passive": self.receipts.player()})
        if leave:
            self.altar_exit()

    def altar_exit(self):
        altar = self.arena["altar"]
        self.approach_altar(altar["at"])
        row, _ = self.select_loc({altar["loc"]}, altar["at"], ("Teleport",))
        before = self.state
        count = self.faction_count(before)
        after = self.act(self.loc_action(row, "Teleport"), before)
        self.wait("native altar exit membership/count", lambda current:
            shared.distance(current["player"], altar["exit"]["value"]) == NO_COUNT
            and self.receipts.player()["instance"] is None and self.faction_count(current) == count, after)
        self.log("altar_exit", {"count": count, "state": self.state,
            "passive": self.receipts.player()})

    def rejoin_camp_and_arena(self):
        # The same live account crosses back into camp; every missing count
        # comes from ordinary NPC deaths. No fresh seed or live counter write.
        # Sara's local itinerary earns normal dungeon faction credits on the
        # observed outer side, then makes the same real camp/arena re-entry.
        outer_credit = self.kite_policy is not None
        if not outer_credit:
            for crossing in self.route_rows("camp"):
                self.crossing(crossing)
            self.log("camp_rejoined", {"state": self.state, "passive": self.receipts.player()})
        self.equip_items(self.plan.get("rejoinEquipment", self.plan["combatEquipment"]))
        if self.bit(self.observe(), "current_prayer_points") > NO_COUNT:
            self.prayers(True)
        required = self.rules["entry"]["lobbyMinimum"]["value"]
        while self.faction_count(self.observe()) < required:
            if self.rejoin_attempts >= MAX_REJOIN_CAMP_KILLS:
                raise Stop("Bounded normal camp replenishment exhausted")
            actor, profile, state = self.camp_actor()
            before = self.faction_count(state)
            name = "rejoin-camp:" + str(self.camp_attempts + SINGLE_MATCH)
            self.rejoin_attempts += SINGLE_MATCH
            death = self.fight(actor, profile, name, state["map"])
            if death is None:
                continue
            self.wait("ordinary credited rejoin count increment", lambda current:
                self.faction_count(current) >= before + SINGLE_MATCH, self.observe()["cycle"])
            receipt = {"name": name, "before": before, "after": self.faction_count(self.state),
                "death": death, "state": self.state}
            self.rejoin_kills.append(receipt)
            self.log("rejoin_count_earned", receipt)
        if outer_credit:
            for crossing in self.route_rows("camp"):
                self.crossing(crossing)
            self.log("camp_rejoined", {"state": self.state, "passive": self.receipts.player()})
        for stage in ("lobby", "arena"):
            self.crossing(next(row for row in self.rules["crossings"]
                if row.get("faction") == self.plan["faction"] and row["stage"] == stage))
            if stage == "lobby":
                before_debit = self.faction_count(self.observe())
        debit = self.rules["entry"]["arenaDebit"]["value"]
        self.wait("actual rejoined room and one normal forty-count debit", lambda current:
            self.faction_count(current) == before_debit - debit
            and self.receipts.player()["room"] == self.arena["roomId"]
            and self.receipts.player()["membership"], self.observe()["cycle"], supervise_food=True)
        self.rejoin = {"before_debit": before_debit, "debit": debit,
            "after_debit": self.faction_count(self.state), "state": self.state,
            "passive": self.receipts.player(), "normal_credited_kills": len(self.rejoin_kills)}
        self.log("arena_rejoined", self.rejoin)
        # Only the sourced exit operation is repeated. No altar-cooldown bypass
        # and no requirement to kill already-completed arena lives a second time.
        self.altar_exit()
        # Reuse the same normal camp itinerary as the first altar exit before
        # selecting the positive-count witness from an actually observed camp.
        for crossing in self.route_rows("campReturn"):
            self.crossing(crossing)
        if outer_credit:
            self.earn_rope_reset_witness()
        for crossing in self.route_rows("camp"):
            self.crossing(crossing)
        if not outer_credit:
            self.earn_rope_reset_witness()
        for crossing in self.route_rows("campReturn"):
            self.crossing(crossing)

    def run(self):
        state = self.wait("authenticated normal GWD map/native counts", lambda current:
            current.get("focused") and self.bit(current, "legacy_combat_active") == SINGLE_MATCH
            and self.faction_count(current) == self.plan["initialCount"], NO_COUNT)
        if state.get("region") is not None:
            raise Stop("Normal GWD first fixture must use the public cache map, not a copied Region")
        self.equip()
        self.log("god_equipment_worn", {"receipt": self.god_equipment_receipt(self.observe())})
        self.prayers(True)
        self.transport_retaliation_off()
        faction = self.plan["faction"]
        for camp in self.route_rows("camp"):
            self.crossing(camp)
        if self.plan["prayerPolicy"]["campDrain"] == "ordinary-empty":
            self.wait("actual generated crossing empties Prayer and clears activation", lambda current:
                self.bit(current, "current_prayer_points") == NO_COUNT
                and all(self.bit(current, row["activationBit"]) == NO_COUNT for row in self.plan["prayers"]),
                self.observe()["cycle"])
            self.log("normal_prayer_empty", {"state": self.state,
                "meaning": "No fixture refill; all camp/arena fights use actual remaining resources"})
        self.equip_items(self.plan["combatEquipment"])
        while True:
            row, profile, state = self.camp_actor()
            stamp = state["map"]
            if self.fight(row, profile, "camp:" + profile["name"], stamp) is not None:
                break
        self.wait("one credited camp kill publishes fortieth count", lambda current:
            self.faction_count(current) == self.rules["entry"]["lobbyMinimum"]["value"], self.observe()["cycle"])
        for stage in ("lobby", "arena"):
            self.crossing(next(row for row in self.rules["crossings"] if row.get("faction") == faction and row["stage"] == stage))
        self.wait("one native count debit and public room membership", lambda current:
            self.faction_count(current) == NO_COUNT and self.receipts.player()["room"] == self.arena["roomId"]
            and self.receipts.player()["membership"], self.observe()["cycle"])
        if self.plan.get("restoreBeforeArena", False):
            self.equip_items(self.plan["altarEquipment"])
            self.altar(leave=False)
            self.prayers(True)
        self.equip_items(self.plan.get("arenaEquipment", self.plan["combatEquipment"]))
        # Reduce incoming pressure through genuine guard kills before the general.
        # No NPC target, health, RNG or membership state is changed by the driver.
        for role in sorted(self.arena["roles"], key=lambda row:
                not row["general"] if self.kite_policy is not None else row["general"]):
            state = self.observe()
            rows, stamp = self.scan("scan_npcs", {"definition": role["type"]})
            if len(rows) != SINGLE_MATCH or stamp != state["map"]:
                raise Stop("Normal room role absent/duplicated in decoded client state")
            prior_tokens = {item["token"] for item in self.receipts.player()["ground"]}
            death = self.fight(rows[FIRST_ENTRY], role["profile"], role["title"], stamp)
            if role["general"]:
                self.wait("ordinary body transition and visible loot", lambda current:
                    any(item["x"] == death["target"]["x"] and item["z"] == death["target"]["z"]
                        and item["item"] not in self.plan.get("recoveredAmmunition", [])
                        for item in self.receipts.player()["ground"]), self.observe()["cycle"], supervise_food=True)
                self.take(death, prior_tokens)
        self.transport_retaliation_off()
        self.equip_items(self.plan["altarEquipment"])
        if self.plan.get("restoreBeforeArena", False):
            # The same-life cooldown is respected: restoration happened once
            # before combat, so this is only the normal sourced exit operation.
            self.altar_exit()
        else:
            self.altar()
        # Altar teleport returns outside the first, forty-count lobby door. The
        # low-count refusal is proved there; do not pretend its locked lobby is reachable.
        entry = next(row for row in self.rules["crossings"] if row.get("faction") == faction and row["stage"] == "lobby")
        self.crossing(entry, admitted=False)
        for camp_return in self.route_rows("campReturn"):
            self.crossing(camp_return)
        self.rejoin_camp_and_arena()
        if self.faction_count(self.observe()) != SINGLE_MATCH \
                or saved_projection.saved_count(self.saved_state(), self.count_rule) != SINGLE_MATCH:
            raise Stop("Positive native/saved witness did not survive the ordinary camp return")
        self.crossing(next(row for row in self.rules["crossings"] if row["stage"] == "exit"))
        self.wait("actual rope publishes and saves all four zero counts", lambda current:
            all(self.bit(current, row["varbit"]) == NO_COUNT for row in self.rules["counts"])
            and all(saved_projection.saved_count(self.saved_state(), row) == NO_COUNT
                for row in self.rules["counts"]), self.observe()["cycle"])
        state = self.state
        self.stage = "complete"
        god_equipment = self.god_equipment_receipt(state)
        physical_wear = self.physical_wear_receipt(state)
        thrown_consumption = self.thrown_consumption_receipt(state)
        self.log("ordinary_thrown_consumption", {"receipt": thrown_consumption})
        self.log("physical_equipment_wear", {"items": physical_wear, "state": state, "passive": self.receipts.player()})
        self.log("complete", {"completed": sorted(self.completed), "full_health": self.full_health,
            "food": self.food_receipts, "state": state, "passive": self.receipts.player(), "rejoin": self.rejoin,
            "rejoin_kills": self.rejoin_kills, "god_equipment": god_equipment, "physical_wear": physical_wear, "thrown_consumption": thrown_consumption,
            "scope": "one declared faction's normal general and three guards; no rendered claim",
            "faction": faction, "ordinaryRandom": True})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("rules", "plan", "food-rules", "slayer-rules", "special-rules", "run-rules", "symbols", "receipts", "journal", "control-module", "control-sha", "socket"):
        parser.add_argument("--" + option, required=True)
    parser.add_argument("--deadline-seconds", type=float, default=DEFAULT_DEADLINE_SECONDS)
    parser.add_argument("--fight-seconds", type=float, default=DEFAULT_FIGHT_SECONDS)
    parser.add_argument("--action-seconds", type=float, default=shared.DEFAULT_ACTION_SECONDS)
    parser.add_argument("--stall-seconds", type=float, default=shared.DEFAULT_STALL_SECONDS)
    parser.add_argument("--socket-seconds", type=float, default=shared.DEFAULT_SOCKET_SECONDS)
    args = parser.parse_args()
    for value in (args.deadline_seconds, args.fight_seconds, args.action_seconds, args.stall_seconds, args.socket_seconds):
        if not NO_COUNT < value <= shared.MAX_SESSION_SECONDS:
            raise Stop("Finite positive driver deadlines required")
    if shared.digest(args.control_module) != args.control_sha:
        raise Stop("Hash-bound shared Control source changed")
    control_spec = importlib.util.spec_from_file_location("gwd_normal_control", args.control_module)
    control_module = importlib.util.module_from_spec(control_spec)
    control_spec.loader.exec_module(control_module)
    control = control_module.Control(args.socket, args.socket_seconds)
    driver = None
    try:
        driver = Driver(control, args)
        driver.run()
    except BaseException as caught:
        if driver is not None:
            driver.log("failure", {"reason": str(caught), "state": driver.state})
        raise
    finally:
        try:
            control.close()
        finally:
            if driver is not None:
                driver.journal.close()


if __name__ == "__main__":
    main()
