#!/usr/bin/env python3
"""Private adaptive driver for the ordinary recorded Barrows client.

Source proposal only. This process has no server control endpoint, launches no
children and writes no player/game files. Every action uses the supplied normal
client Control API. Passive server receipts qualify results independently.
"""

import argparse
import hashlib
import importlib.util
import json
import re
from pathlib import Path
import sys
import time


COMPONENT_GROUP_SHIFT = 16
STATIC_COMPONENT_CHILD = -1
FIRST_OPERATION = 1
CONTINUE_OPERATION = 0
WINDOW_SELECTION_BIAS = 1
FIRST_SLOT = 0
NEXT_ENTRY = 1
CHUNK_TILES = 8
CHUNK_LAST_TILE = CHUNK_TILES - NEXT_ENTRY
LAND_PLANES = 4
SOURCE_X_SHIFT = 14
SOURCE_X_MASK = 0x3FF
SOURCE_Z_SHIFT = 3
SOURCE_Z_MASK = 0x7FF
SOURCE_LEVEL_SHIFT = 24
SOURCE_LEVEL_MASK = LAND_PLANES - NEXT_ENTRY
ROTATION_SHIFT = 1
ROTATION_MASK = LAND_PLANES - NEXT_ENTRY
DOOR_FACING = ((-1, 0), (0, 1), (1, 0), (0, -1))
CARDINAL_APPROACH = DOOR_FACING
SCAN_PAGE_SIZE = 64
MAX_SCAN_ENTRIES = 4096
MAX_COMPONENT_DEPTH = 4
MAX_RECEIPT_LINE_BYTES = 512 * 1024
MAX_RECEIPT_READ_BYTES = 1024 * 1024
MAX_SAVED_RECEIPTS = 20000
MAX_NAVIGATION_STEPS = 96
MAX_WALK_PROBES = 192
MAX_REQUESTS = 20000
MAX_SESSION_SECONDS = 1800
DEFAULT_SESSION_SECONDS = 1200
DEFAULT_ACTION_SECONDS = 40
DEFAULT_FIGHT_SECONDS = 180
DEFAULT_STALL_SECONDS = 35
DEFAULT_SOCKET_SECONDS = 10
POLL_SECONDS = 0.2
SETTLED_OBSERVATIONS = 3
WALK_STALL_SECONDS = 3
FOOD_SAFETY_NUMERATOR = 3
FOOD_SAFETY_DENOMINATOR = 5
DRIVER_FORMAT = 1
RETALIATION_ON = 0
RETALIATION_OFF = 1
ESCAPE_AWT_CODE = 27
MODAL_CLOSE_CHILD = 1

SEQUENCE_NAMES = ("sequence_first", "sequence_second", "sequence_third")
ANSWER_NAMES = ("answer_first", "answer_second", "answer_third")
BUTTON_NAMES = ANSWER_NAMES
TUNNEL_ENTER_TEXT = "Enter the tunnel"
TUNNEL_STAY_TEXT = "Stay here"


class Stop(RuntimeError):
    """A qualified refusal/stall, never an invitation to change game state."""


class UnacceptedNpcIntent(Stop):
    """The native client rejected this NPC intent before accepting input."""
    def __init__(self, response):
        super().__init__(f"native NPC intent was not accepted: {response}")
        self.response = response


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def load_json(path):
    return json.loads(Path(path).read_text())


def distance(left, right):
    if left["level"] != right["level"]:
        return float("inf")
    return max(abs(left["x"] - right["x"]), abs(left["z"] - right["z"]))


def tile_key(tile):
    return tile["level"], tile["x"], tile["z"]


def normal_text(text):
    return " ".join(str(text or "").split()).casefold()


def symbol_rows(path):
    result = {}
    for line in Path(path).read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) < 2:
            raise Stop(f"invalid symbol row in {path}")
        result[fields[FIRST_SLOT]] = fields[NEXT_ENTRY]
    return result


class Symbols:
    def __init__(self, directory):
        self.directory = Path(directory)
        self.tables = {kind: symbol_rows(self.directory / filename) for kind, filename in {
            "component": "component.sym", "inv": "inv.sym", "obj": "obj.sym",
            "varbit": "varbit.sym", "interface": "interface.sym", "param": "param.sym",
        }.items()}

    def get(self, kind, name):
        try:
            value = self.tables[kind][name]
        except KeyError as error:
            raise Stop(f"required named selector absent: {kind}.{name}") from error
        if kind == "component":
            group, child = (int(part) for part in value.split(":"))
            return (group << COMPONENT_GROUP_SHIFT) | child
        return int(value)

    def component(self, name):
        return {"parent": self.get("component", name), "child": STATIC_COMPONENT_CHILD}


class Layout:
    """The installed Region template decoder; no destination cache lookup."""
    def __init__(self, state):
        self.map = state["map"]
        self.region = state.get("region")

    @staticmethod
    def rotate(x, z, rotation):
        if rotation == 0:
            return x, z
        if rotation == 1:
            return z, CHUNK_LAST_TILE - x
        if rotation == 2:
            return CHUNK_LAST_TILE - x, CHUNK_LAST_TILE - z
        return CHUNK_LAST_TILE - z, x

    def resolve(self, source):
        if self.region is None:
            raise Stop("source-template tile requested in an ordinary map")
        chunks_x, chunks_z = self.region["chunks_x"], self.region["chunks_z"]
        templates = self.region["templates"]
        expected = LAND_PLANES * chunks_x * chunks_z
        if len(templates) != expected:
            raise Stop("installed Region template cardinality is not qualified")
        source_chunk_x = source["x"] // CHUNK_TILES
        source_chunk_z = source["z"] // CHUNK_TILES
        matches = []
        for index, template in enumerate(templates):
            if template < 0:
                continue
            if ((template >> SOURCE_X_SHIFT) & SOURCE_X_MASK) != source_chunk_x:
                continue
            if ((template >> SOURCE_Z_SHIFT) & SOURCE_Z_MASK) != source_chunk_z:
                continue
            if ((template >> SOURCE_LEVEL_SHIFT) & SOURCE_LEVEL_MASK) != source["level"]:
                continue
            plane, within = divmod(index, chunks_x * chunks_z)
            chunk_x, chunk_z = divmod(within, chunks_z)
            x, z = self.rotate(source["x"] % CHUNK_TILES, source["z"] % CHUNK_TILES,
                               (template >> ROTATION_SHIFT) & ROTATION_MASK)
            matches.append({"level": plane,
                            "x": self.map["base_x"] + chunk_x * CHUNK_TILES + x,
                            "z": self.map["base_z"] + chunk_z * CHUNK_TILES + z})
        if len(matches) != NEXT_ENTRY:
            raise Stop(f"source tile has {len(matches)} installed mappings: {source}")
        return matches[FIRST_SLOT]


class Receipts:
    """Append-only observation, never used to choose hidden tunnel/puzzle facts."""
    def __init__(self, path):
        self.path = Path(path)
        self.offset = 0
        self.identity = None
        self.buffer = b""
        self.rows = []
        self.last_state = None

    def refresh(self):
        if not self.path.exists():
            return
        metadata = self.path.stat()
        identity = metadata.st_dev, metadata.st_ino
        if self.identity is not None and identity != self.identity:
            raise Stop("passive receipt file identity changed")
        self.identity = identity
        if metadata.st_size < self.offset:
            raise Stop("passive receipt file was replaced/truncated")
        with self.path.open("rb") as source:
            source.seek(self.offset)
            chunk = source.read(MAX_RECEIPT_READ_BYTES)
        self.offset += len(chunk)
        self.buffer += chunk
        while b"\n" in self.buffer:
            line, self.buffer = self.buffer.split(b"\n", NEXT_ENTRY)
            if len(line) > MAX_RECEIPT_LINE_BYTES:
                raise Stop("passive receipt line exceeds finite bound")
            row = json.loads(line)
            self.rows.append(row)
            if len(self.rows) > MAX_SAVED_RECEIPTS:
                raise Stop("passive receipt count exceeds finite bound")
            if row.get("kind") == "state":
                self.last_state = row
        if len(self.buffer) > MAX_RECEIPT_LINE_BYTES:
            raise Stop("unterminated passive receipt exceeds finite bound")

    def player(self):
        self.refresh()
        players = (self.last_state or {}).get("players", [])
        if len(players) != NEXT_ENTRY:
            raise Stop("passive receipt must identify exactly one fixture account")
        return players[FIRST_SLOT]


class Driver:
    def __init__(self, control, args):
        self.control = control
        self.args = args
        self.rules = load_json(args.rules)
        self.plan = load_json(args.plan)
        self.food_rules = load_json(args.food_rules)
        self.symbols = Symbols(args.symbols)
        self.receipts = Receipts(args.receipts)
        self.started = time.monotonic()
        self.deadline = self.started + args.deadline_seconds
        self.requests = 0
        self.state = None
        self.last_scans = {}
        self.stage = "ready"
        self.actions = 0
        self.completed = set()
        self.full_health = {}
        self.food_receipts = []
        self.withdrawals = []
        self.completed_generation = None
        self.puzzle_receipts = []
        self.walk_probes = 0
        self.tunnel = None
        self.tunnel_crossings = []
        self.next_food_tick = None
        self.journal = Path(args.journal).open("x", encoding="utf8")
        self.components = self.static_components()
        self.retaliation_varp = self.plan.get("toolbar", {}).get("retaliationVarp")
        if isinstance(self.retaliation_varp, bool) or not isinstance(self.retaliation_varp, int) or self.retaliation_varp < FIRST_SLOT:
            raise Stop("initial plan lacks the named cache-bound retaliation varp")
        self.query = {
            "varps": [self.retaliation_varp],
            "varbits": sorted(set([crypt["slainBit"] for crypt in self.rules["crypts"]] +
                [door["bit"] for door in self.rules["doors"]] +
                [rope["bit"] for rope in self.rules["ropes"]] +
                [self.symbols.get("varbit", name) for name in (
                    "current_life_points", "current_prayer_points", "legacy_combat_active",
                    "legacy_interface_mode", "slim_window_headers", "gameplay_settings_category",
                    "barrows_reward_opened", "barrows_total_kills") ])),
            "inventories": [self.symbols.get("inv", name) for name in (
                "backpack", "worn_equipment", "barrows_pending_rewards", "coin_wallet")],
            "components": self.components,
            "client_varbits": [self.symbols.get("varbit", "legacy_selected_window")],
        }
        self.query["varbits"] = sorted(set(self.query["varbits"]) |
            {rule["activationBit"] for rule in self.plan["prayers"]})
        self.log("start", {"format": DRIVER_FORMAT, "inputs": {
            name: {"path": str(path), "sha256": digest(path)} for name, path in {
                "rules": args.rules, "plan": args.plan, "control": args.control_module,
                "food_rules": args.food_rules, "driver": Path(__file__),
            }.items()}, "budget": "launches remain owned by recorder; expected prior3/max4",
            "driver_policy": {"food_threshold": [FOOD_SAFETY_NUMERATOR, FOOD_SAFETY_DENOMINATOR],
                "action_seconds": args.action_seconds, "stall_seconds": args.stall_seconds,
                "deadline_seconds": args.deadline_seconds},
            "proof_boundary": "semantic menu/native component recording; not screen picking, rendered parity or FPS"})

    def static_components(self):
        names = ["window_buttons.actions", "backpack.slots", "loot_claim_window.item_area", "dialogue_options.first_option",
                 "game_window.dialogue_slot", "message_box.continue_button",
                 "npc_chat.continue_button", "player_chat.continue_button"]
        names += [f"barrows_pattern_choices.{name}" for name in SEQUENCE_NAMES + ANSWER_NAMES + BUTTON_NAMES]
        names += ["prayer_book.prayer_buttons", "legacy_combat.retaliation_toggle"]
        names += ["options_menu.settings_button", "gameplay_settings.categories",
                  "gameplay_settings.options", "game_window.modal_close_button"]
        return [self.symbols.component(name) for name in dict.fromkeys(names)]

    def log(self, kind, value):
        self.journal.write(json.dumps({"kind": kind, "stage": self.stage,
            "elapsed_seconds": time.monotonic() - self.started, **value}, sort_keys=True) + "\n")
        self.journal.flush()

    def request(self, command, deadline=None):
        until = self.deadline if deadline is None else min(self.deadline, deadline)
        remaining = until - time.monotonic()
        if remaining <= 0 or self.requests >= MAX_REQUESTS:
            raise Stop("finite driver session/request deadline exhausted")
        self.requests += NEXT_ENTRY
        response = self.control.request(command, timeout=min(self.args.socket_seconds, remaining))
        self.log("control", {"request": command, "response": response})
        if command.get("command", "").startswith("scan_"):
            self.last_scans[command["command"]] = {"request": command, "response": response}
        self.receipts.refresh()
        if (command.get("command") == "action"
                and command.get("action", {}).get("kind") == "npc"
                and response.get("status") == "refused"
                and response.get("data", {}).get("reason") == "stale or unavailable NPC"):
            raise UnacceptedNpcIntent(response)
        if response.get("status") not in ("observed", "accepted"):
            raise Stop(f"ordinary controller refused: {response}")
        return response

    def observe(self, deadline=None):
        response = self.request({"command": "snapshot", "query": self.query}, deadline=deadline)
        self.state = response["data"]
        if self.state.get("ready"):
            life = self.bit(self.state, "current_life_points")
            if self.stage != "ready" and life is not None and life <= 0:
                raise Stop("player death observed; no fixture restoration permitted")
        return self.state

    def wait(self, name, predicate, after, seconds=None, supervise_food=False, deadline=None):
        deadline = min(self.deadline, deadline if deadline is not None
            else time.monotonic() + (seconds or self.args.action_seconds))
        last = None
        while time.monotonic() < deadline:
            state = self.observe(deadline=deadline)
            last = state
            if state.get("ready") and state["cycle"] > after and predicate(state):
                self.log("postcondition", {"name": name, "cycle": state["cycle"],
                    "map": state["map"], "player": state.get("player"),
                    "packets_applied": state.get("packets_applied")})
                return state
            if supervise_food and state.get("ready") and state.get("focused"):
                self.eat_if_needed(state)
            time.sleep(min(POLL_SECONDS, max(0, deadline - time.monotonic())))
        raise Stop(f"no progress: {name}; {self.diagnostic(last)}")

    def diagnostic(self, state=None):
        state = state or self.state or {}
        return json.dumps({"cycle": state.get("cycle"), "packets_applied": state.get("packets_applied"),
            "focused": state.get("focused"), "ready": state.get("ready"), "map": state.get("map"),
            "player": state.get("player"), "varbits": state.get("varbits"),
            "inventories": state.get("inventories"), "stage": self.stage,
            "last_scans": self.last_scans}, sort_keys=True)

    def bit(self, state, name_or_id):
        identifier = self.symbols.get("varbit", name_or_id) if isinstance(name_or_id, str) else name_or_id
        for row in state.get("varbits", []):
            if row["id"] == identifier:
                return row.get("value")
        return None

    def client_bit(self, state, name):
        identifier = self.symbols.get("varbit", name)
        for row in state.get("client_varbits", []):
            if row["id"] == identifier:
                return row.get("value")
        return None

    def inventory(self, state, name):
        identifier = self.symbols.get("inv", name)
        for row in state.get("inventories", []):
            if row["id"] == identifier and row.get("value") is not None:
                value = row["value"]
                if value.get("truncated") or len(value["items"]) != len(value["counts"]):
                    raise Stop(f"inventory {name} is incomplete")
                return value
        raise Stop(f"native inventory {name} not received")

    @staticmethod
    def count(inventory, item):
        return sum(count for actual, count in zip(inventory["items"], inventory["counts"]) if actual == item)

    def component(self, state, name):
        target = self.symbols.component(name)
        for row in state.get("components", []):
            if row["target"] == target:
                return row if row.get("value") is not None else None
        return None

    def visible(self, state, name):
        row = self.component(state, name)
        return bool(row and row["value"]["rooted_visible"])

    def act(self, action, state=None):
        state = state or self.observe()
        source_map = state["map"]
        if not state.get("ready") or not state.get("focused"):
            state = self.wait("focused installed client before action",
                lambda s: s.get("focused"), state["cycle"])
        # Keep the original intent and target facts. A fresh read-only snapshot
        # supplies only the admission cycle; native resolution still checks the
        # retained actor, component, object and menu identities when applying it.
        fresh = self.observe()
        if fresh.get("map") != source_map:
            raise Stop("installed map changed before submitting selected action")
        if not fresh.get("ready") or not fresh.get("focused"):
            raise Stop("fresh installed client is not ready and focused before action")
        self.actions += NEXT_ENTRY
        response = self.request({"command": "action", "map": fresh["map"],
            "observed_cycle": fresh["cycle"], "action": action})
        if response.get("status") != "accepted":
            raise Stop(f"action did not receive recorded acceptance: {response}")
        return response["cycle"]

    def scan(self, kind, query=None):
        query = dict(query or {})
        rows, offset, stamp = [], 0, None
        while True:
            response = self.request({"command": kind, "query": {
                "definition": None, "name": None, "level": None, "x": None, "z": None,
                "radius": None, **query, "offset": offset, "limit": SCAN_PAGE_SIZE}})
            payload = response["data"]
            if stamp is not None and payload["map"] != stamp:
                raise Stop("map replaced during scanner pagination")
            stamp = payload["map"]
            page = payload["scan"]
            rows.extend(page["entries"])
            if len(rows) > MAX_SCAN_ENTRIES:
                raise Stop("installed scanner exceeds finite bound")
            next_offset = page.get("next_offset")
            if next_offset is None:
                return rows, stamp
            if next_offset <= offset:
                raise Stop("scanner pagination did not advance")
            offset = next_offset

    def children(self, parent, recursive=False, depth=0, seen=None, parameters=()):
        if depth > MAX_COMPONENT_DEPTH:
            raise Stop("loaded component tree exceeds driver depth")
        seen = set() if seen is None else seen
        identity = parent["parent"], parent["child"]
        if identity in seen:
            raise Stop("loaded component traversal repeated an identity")
        seen.add(identity)
        rows, offset = [], 0
        while True:
            response = self.request({"command": "scan_components", "parent": parent,
                "offset": offset, "limit": SCAN_PAGE_SIZE, "component_parameters": list(parameters)})
            page = response["data"]["scan"]
            rows.extend(page["entries"])
            if len(rows) > MAX_SCAN_ENTRIES:
                raise Stop("loaded component tree exceeds driver bound")
            next_offset = page.get("next_offset")
            if next_offset is None:
                break
            if next_offset <= offset:
                raise Stop("component pagination did not advance")
            offset = next_offset
        if recursive:
            descendants = []
            for row in rows:
                value = row.get("value") or {}
                if value and value.get("has_children", True):
                    # A missing optional has_children is not a reason to guess
                    # flattened slot IDs. An actual leaf scan returns no rows.
                    descendants.extend(self.children(row["target"], True, depth + NEXT_ENTRY, seen, parameters))
            rows.extend(descendants)
        return rows

    @staticmethod
    def operation(row, choices):
        ops = row.get("ops")
        if ops is None:
            ops = row.get("value", {}).get("ops")
        found = [(index + FIRST_OPERATION, label) for index, label in enumerate(ops or [])
                 if label and normal_text(label) in {normal_text(choice) for choice in choices}]
        if len(found) != NEXT_ENTRY:
            raise Stop(f"native operation unavailable/ambiguous: {choices}; {row}")
        return found[FIRST_SLOT]

    def ui_action(self, row, choices=None, item=None, continuation=False):
        value = row["value"]
        if not value.get("rooted_visible"):
            raise Stop("native UI target is not visibly rooted")
        operation, label = (CONTINUE_OPERATION, None) if continuation else self.operation(row, choices)
        action = {"kind": "ui", "target": row["target"], "serial": value["serial"],
                  "operation": operation}
        if item is not None:
            action["expected_object"] = item
        if label is not None:
            action["expected_operation"] = label
        return action

    def item_control(self, state, root, item, choices):
        matches = []
        for row in self.children(self.symbols.component(root), recursive=True):
            value = row.get("value") or {}
            if value.get("object") != item or not value.get("rooted_visible"):
                continue
            try:
                self.operation(row, choices)
            except Stop:
                continue
            matches.append(row)
        if not matches:
            raise Stop(f"no current visible native {choices} for object {item} under {root}")
        # The driver chooses one actual loaded child, not a guessed slot encoding.
        return sorted(matches, key=lambda row: (row["target"]["parent"], row["target"]["child"]))[FIRST_SLOT]

    def open_window(self, destination, visible_component, name):
        if isinstance(destination, bool) or not isinstance(destination, int) or destination < 0:
            raise Stop(f"initial plan lacks the actual cache-derived {name} destination")
        def opened(state):
            return self.client_bit(state, "legacy_selected_window") == destination + WINDOW_SELECTION_BIAS \
                and self.visible(state, visible_component)
        state = self.observe()
        if opened(state):
            return state
        parameter_id = self.symbols.get("param", "window_tab_destination")
        rows = self.children(self.symbols.component("window_buttons.actions"), parameters=(parameter_id,))
        matches = [row for row in rows if any(parameter.get("id") == parameter_id
            and parameter.get("present") is True and parameter.get("value", {}).get("kind") == "int"
            and parameter["value"]["value"] == destination
            for parameter in (row.get("value") or {}).get("parameters", []))]
        if len(matches) != NEXT_ENTRY:
            raise Stop(f"actual loaded native {name} destination is absent/ambiguous: {matches}")
        state = self.observe()
        operation, _ = self.operation(matches[FIRST_SLOT], ("Open", "Close"))
        if operation != FIRST_OPERATION:
            raise Stop(f"actual native {name} button does not expose a unique first Open/Close operation")
        after = self.act(self.ui_action(matches[FIRST_SLOT], ("Open", "Close")), state)
        return self.wait(f"native {name} selection and visibly rooted pane", opened, after)

    def retaliation_preference(self, state):
        identifier = self.retaliation_varp
        for row in state.get("varps", []):
            if row["id"] == identifier:
                return row.get("value")
        return None

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
        if isinstance(index, bool) or not isinstance(index, int) or index < FIRST_SLOT:
            raise Stop("cache-derived native setting child is invalid")
        parent = self.symbols.get("component", root)
        rows = self.children(self.symbols.component(root))
        matches = [row for row in rows if row["target"] == {"parent": parent, "child": index}]
        if len(matches) > NEXT_ENTRY:
            raise Stop(f"current native setting child is ambiguous: {root}:{index}")
        if not matches:
            if pending:
                return None
            raise Stop(f"current native setting child is absent: {root}:{index}")
        row = matches[FIRST_SLOT]
        value = row.get("value") or {}
        ready = (value.get("rooted_visible") and value.get("onop")
                 and value.get("active_mask", FIRST_SLOT) & (NEXT_ENTRY << FIRST_OPERATION)
                 and any(label and normal_text(label) in {normal_text(choice) for choice in choices}
                         for label in value.get("ops") or []))
        if not ready:
            if pending:
                return None
            raise Stop("native Settings row lacks its current rooted server-operation grant")
        operation, _ = self.operation(row, choices)
        if operation != FIRST_OPERATION:
            raise Stop("native Settings row lacks its cache-owned first operation")
        return row

    def setting_bit(self, route, name, expected, source_map):
        state = self.open_gameplay_settings()
        if state["map"] != source_map:
            raise Stop("source map changed while opening native Settings")
        if self.bit(state, name) == expected:
            return state
        category = route.get("category")
        path = route.get("selectionPath")
        if not isinstance(path, list) or not path or len(path) > MAX_COMPONENT_DEPTH:
            raise Stop("cache-derived Settings selection path is absent or exceeds its finite bound")
        visited = set()
        for step in path:
            if not isinstance(step, dict) or any(isinstance(step.get(key), bool)
                    or not isinstance(step.get(key), int) or step[key] < FIRST_SLOT
                    for key in ("index", "parent", "category")) or step["index"] in visited:
                raise Stop("cache-derived Settings selection path is invalid or cyclic")
            visited.add(step["index"])
        if path[-NEXT_ENTRY]["index"] != category or path[-NEXT_ENTRY]["category"] != category:
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
                if len(matches) > NEXT_ENTRY:
                    raise Stop("current native Settings ancestry row is ambiguous")
                if not matches:
                    return None
                row = matches[FIRST_SLOT]
                value = row.get("value") or {}
                if not value.get("rooted_visible") or not value.get("onop") \
                        or not value.get("active_mask", FIRST_SLOT) & (NEXT_ENTRY << FIRST_OPERATION):
                    return None
                labels = [label for label in value.get("ops") or []
                          if label and normal_text(label) == normal_text("Select")]
                if not labels:
                    return None
                operation, _ = self.operation(row, ("Select",))
                if operation != FIRST_OPERATION:
                    raise Stop("native Settings ancestry lacks its cache-owned first operation")
                return row
            if position + NEXT_ENTRY < len(path):
                next_row = selectable(path[position + NEXT_ENTRY]["index"])
                if next_row is not None:
                    self.log("native_settings_ancestor_already_expanded", {"step": step,
                        "next": path[position + NEXT_ENTRY], "control": next_row,
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
        if current not in (FIRST_SLOT, NEXT_ENTRY):
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
        if current not in (FIRST_SLOT, NEXT_ENTRY):
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
        if slim not in (FIRST_SLOT, NEXT_ENTRY) or fixed not in (FIRST_SLOT, NEXT_ENTRY):
            raise Stop("native Slim-header/layout preferences were not observed")
        if slim == NEXT_ENTRY:
            return state
        routes = self.plan.get("toolbar", {}).get("settings", {})
        for name in ("layout", "slim"):
            if not isinstance(routes.get(name), dict):
                raise Stop("initial plan lacks the named cache-derived Settings route")
        if fixed == NEXT_ENTRY:
            state = self.setting_bit(routes["layout"], "legacy_interface_mode", FIRST_SLOT, source_map)
        state = self.setting_bit(routes["slim"], "slim_window_headers", NEXT_ENTRY, source_map)
        if fixed == NEXT_ENTRY:
            state = self.setting_bit(routes["layout"], "legacy_interface_mode", NEXT_ENTRY, source_map)
        if self.visible(state, "gameplay_settings.categories"):
            rows = self.children(self.symbols.component("game_window.modal_close_button"))
            parent = self.symbols.get("component", "game_window.modal_close_button")
            matches = [row for row in rows if row["target"] == {"parent": parent, "child": MODAL_CLOSE_CHILD}
                       and (row.get("value") or {}).get("rooted_visible")]
            if len(matches) != NEXT_ENTRY:
                raise Stop("native Settings modal Close child is absent/ambiguous")
            row = matches[FIRST_SLOT]
            operation, _ = self.operation(row, ("Close",))
            if operation != FIRST_OPERATION:
                raise Stop("native Settings Close lacks its ordinary first operation")
            after = self.act(self.ui_action(row, ("Close",)), state)
            state = self.wait("ordinary native Settings modal closes",
                lambda s: s["map"] == source_map and not self.visible(s, "gameplay_settings.categories"), after)
        if self.bit(state, "slim_window_headers") != NEXT_ENTRY or self.bit(state, "legacy_interface_mode") != fixed:
            raise Stop("native Slim/header route did not preserve the original layout")
        return state


    def tunnel_retaliation_off(self):
        """Use the native preference so incoming creatures do not replace a walk."""
        state = self.observe()
        source_map = state["map"]
        current = self.retaliation_preference(state)
        if current == RETALIATION_OFF:
            return state
        if current != RETALIATION_ON:
            raise Stop("native auto-retaliation preference is absent or invalid")
        self.slim_retaliation_pane()
        state = self.open_window(self.plan.get("toolbar", {}).get("combatDestination"),
            "legacy_combat.retaliation_toggle", "Legacy combat")
        if state["map"] != source_map:
            raise Stop("source map changed while opening native Legacy combat")
        current = self.retaliation_preference(state)
        if current == RETALIATION_OFF:
            return state
        if current != RETALIATION_ON:
            raise Stop("native auto-retaliation preference changed to an invalid value")
        row = self.component(state, "legacy_combat.retaliation_toggle")
        value = (row or {}).get("value") or {}
        labels = value.get("ops") or []
        label = labels[FIRST_SLOT] if labels else None
        if not value.get("rooted_visible") or not label or not value.get("active_mask", FIRST_SLOT) & (NEXT_ENTRY << FIRST_OPERATION):
            raise Stop("native retaliation toggle lacks its visible first-operation contract")
        # The ordinary server owner qualifies operation one for this named
        # control; the exact current native label still qualifies admission.
        action = {"kind": "ui", "target": row["target"], "serial": value["serial"],
            "operation": FIRST_OPERATION, "expected_operation": label}
        after = self.act(action, state)
        def disabled(observed):
            if observed["map"] != source_map:
                raise Stop("source map changed before native retaliation OFF confirmation")
            return (self.retaliation_preference(observed) == RETALIATION_OFF
                    and self.receipts.player().get("autoRetaliateDisabled") == RETALIATION_OFF)
        observed = self.wait("native auto-retaliation OFF before tunnel navigation", disabled, after)
        self.log("tunnel_retaliation_native_result", {"before": current,
            "after": self.retaliation_preference(observed), "control": row,
            "cycle": observed["cycle"], "map": observed["map"],
            "qualification": "ordinary native UI preference; explicit Attack remains ordinary"})
        return observed

    def open_backpack(self):
        return self.open_window(self.plan.get("toolbar", {}).get("backpackDestination"),
            "backpack.slots", "Backpack")

    def protection_rule(self, crypt):
        styles = []
        if crypt["attacks"].get("magic") is not None:
            styles.append("magic")
        if crypt["attacks"].get("ranged") is not None:
            styles.append("ranged")
        if crypt["profile"]["maxHit"] > 0:
            styles.append("melee")
        if len(styles) != NEXT_ENTRY:
            raise Stop("generated brother has no unique ordinary incoming style for protection")
        rules = [rule for rule in self.plan["prayers"] if rule["group"] == f"protect-{styles[FIRST_SLOT]}"]
        if len(rules) != NEXT_ENTRY:
            raise Stop("generated matching protection Prayer is absent/ambiguous")
        return rules[FIRST_SLOT]

    def set_protection(self, crypt, enabled):
        rule = self.protection_rule(crypt)
        expected = NEXT_ENTRY if enabled else FIRST_SLOT
        state = self.observe()
        current = self.bit(state, rule["activationBit"])
        if current not in (FIRST_SLOT, NEXT_ENTRY):
            raise Stop("matching Prayer PLAYER activation bit was not observed")
        if current == expected:
            self.log("protection_already_observed", {"brother": crypt["brother"],
                "prayer": rule["name"], "enabled": enabled, "cycle": state["cycle"],
                "map": state["map"], "activation_bit": rule["activationBit"], "value": current})
            return enabled
        remaining = self.bit(state, "current_prayer_points")
        if remaining is None:
            raise Stop("current Prayer resource was not actually observed")
        if enabled and remaining <= FIRST_SLOT:
            self.log("protection_unavailable", {"brother": crypt["brother"], "prayer": rule["name"],
                "cycle": state["cycle"], "remaining_prayer": self.bit(state, "current_prayer_points"),
                "reason": "observed real resource depletion; no refill or action retry"})
            return False
        state = self.open_window(self.plan.get("toolbar", {}).get("prayerDestination"),
            "prayer_book.prayer_buttons", "Prayer")
        if self.bit(state, rule["activationBit"]) == expected:
            return enabled
        remaining = self.bit(state, "current_prayer_points")
        if remaining is None:
            raise Stop("current Prayer resource was not actually observed")
        if enabled and remaining <= FIRST_SLOT:
            self.log("protection_unavailable", {"brother": crypt["brother"], "prayer": rule["name"],
                "cycle": state["cycle"], "remaining_prayer": self.bit(state, "current_prayer_points"),
                "reason": "resource depleted while opening the native pane; no action retry"})
            return False
        parent = self.symbols.get("component", "prayer_book.prayer_buttons")
        rows = self.children(self.symbols.component("prayer_book.prayer_buttons"))
        matches = [row for row in rows if row["target"] == {"parent": parent, "child": rule["button"]}
            and (row.get("value") or {}).get("rooted_visible")]
        if len(matches) != NEXT_ENTRY:
            raise Stop(f"actual loaded native matching Prayer button is absent/ambiguous: {rule['name']}")
        row = matches[FIRST_SLOT]
        verb = "Activate" if enabled else "Deactivate"
        expected_label = normal_text(f"{verb} {rule['name']}")
        choices = tuple(label for label in row["value"].get("ops", []) if label and
            normal_text(re.sub(r"</?col(?:=[^>]*)?>", "", label)) == expected_label)
        state = self.observe()
        after = self.act(self.ui_action(row, choices), state)
        result = self.wait("native matching Prayer PLAYER activation publication", lambda observed:
            self.bit(observed, rule["activationBit"]) == expected or
            (enabled and self.bit(observed, "current_prayer_points") == FIRST_SLOT), after)
        active = self.bit(result, rule["activationBit"]) == NEXT_ENTRY
        self.log("protection_native_result", {"brother": crypt["brother"], "prayer": rule["name"],
            "requested_enabled": enabled, "observed_enabled": active, "target": row["target"],
            "serial": row["value"]["serial"], "observed_operations": row["value"].get("ops"),
            "activation_bit": rule["activationBit"], "value": self.bit(result, rule["activationBit"]),
            "cycle": result["cycle"], "map": result["map"],
            "remaining_prayer": self.bit(result, "current_prayer_points"),
            "meaning": "actual loaded native operation and PLAYER publication; not borrowed GWD visibility proof"})
        if not enabled and active:
            raise Stop("normal native Prayer deactivation was not published")
        return active

    def equip(self):
        self.stage = "equip"
        self.open_backpack()
        for held in self.plan.get("equipment", []):
            item = held["item"] if isinstance(held, dict) else held
            state = self.observe()
            if self.count(self.inventory(state, "worn_equipment"), item):
                continue
            if not self.count(self.inventory(state, "backpack"), item):
                raise Stop(f"initial fixture equipment {item} absent; no insertion permitted")
            row = self.item_control(state, "backpack.slots", item, ("Wear", "Wield"))
            after = self.act(self.ui_action(row, ("Wear", "Wield"), item), state)
            self.wait("native equipment transfer", lambda s, item=item:
                self.count(self.inventory(s, "worn_equipment"), item) > 0, after)

    def eat_if_needed(self, state, minimum_life=None):
        life = self.bit(state, "current_life_points")
        # Capacity is observed from the independent account receipt, not assigned.
        passive = self.receipts.player()
        maximum = passive["maximumLife"]
        if life is None:
            return False
        if minimum_life is not None:
            if isinstance(minimum_life, bool) or not isinstance(minimum_life, int) or not 0 < minimum_life < maximum:
                raise Stop("Declared ordinary food threshold is outside observed life capacity")
            needs_food = life < minimum_life
        else:
            needs_food = life * FOOD_SAFETY_DENOMINATOR < maximum * FOOD_SAFETY_NUMERATOR
        if not needs_food:
            return False
        observed_tick = self.receipts.last_state["tick"]
        if self.next_food_tick is not None and observed_tick < self.next_food_tick:
            return False
        item = self.symbols.get("obj", "shark")
        facts = [food for food in self.food_rules["foods"] if food["item"] == item]
        if len(facts) != NEXT_ENTRY:
            raise Stop("ordinary Shark row is not uniquely generated")
        state = self.open_backpack()
        before = self.inventory(state, "backpack")
        total = self.count(before, item)
        if total <= 0:
            raise Stop("food exhausted while injured; no fixture heal permitted")
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
            raise Stop("native food publication lacks passive saved-account consumption receipt")
        commit = commits[FIRST_SLOT]
        if commit.get("beforeItem") != item or commit.get("beforeCount") != NEXT_ENTRY or commit.get("afterCount") != 0:
            raise Stop("ordinary food commit does not consume exactly one observed singleton")
        if commit["healed"] != commit["lifeAfter"] - commit["lifeBefore"] or commit["healed"] > facts[FIRST_SLOT]["maximumHealing"]:
            raise Stop("ordinary food commit healing is inconsistent with qualified food cap")
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

    def walk(self, destination, radius=0, allow_partial=False):
        state = self.observe()
        if distance(state["player"], destination) <= radius and state["player"]["route_length"] == 0:
            return True
        if state["player"]["level"] != destination["level"]:
            raise Stop("normal Walk cannot change plane")
        self.walk_probes += NEXT_ENTRY
        if self.walk_probes > MAX_WALK_PROBES:
            raise Stop("bounded ordinary route probes exhausted")
        after = self.act({"kind": "walk", "x": destination["x"], "z": destination["z"], "run": False}, state)
        deadline = min(self.deadline, time.monotonic() + self.args.action_seconds)
        prior, settled = None, 0
        last_change = time.monotonic()
        last_cycle = after
        while time.monotonic() < deadline:
            state = self.observe()
            if not state.get("ready") or state["cycle"] <= after:
                time.sleep(POLL_SECONDS)
                continue
            player = state["player"]
            if distance(player, destination) <= radius and player["route_length"] == 0:
                self.log("walk_arrived", {"destination": destination, "player": player,
                    "cycle": state["cycle"], "map": state["map"]})
                return True
            # A retained route is an intention, not physical progress. The
            # native actor can stop with queued waypoints still present.
            identity = (tile_key(player), player["fine_x"], player["fine_z"])
            if identity != prior:
                last_change = time.monotonic()
                settled = 0
            elif state["cycle"] > last_cycle:
                settled += NEXT_ENTRY
            last_cycle = state["cycle"]
            prior = identity
            self.eat_if_needed(state)
            if settled >= SETTLED_OBSERVATIONS and time.monotonic() - last_change >= WALK_STALL_SECONDS:
                if allow_partial:
                    self.log("walk_stopped_short", {"destination": destination, "state": state})
                    return False
                raise Stop(f"normal route stopped short of {destination}; {self.diagnostic(state)}")
            time.sleep(POLL_SECONDS)
        raise Stop(f"unresolved normal route to {destination}; {self.diagnostic(state)}")

    def select_loc(self, definitions, near, operations, exact_tile=None):
        state = self.observe()
        rows, stamp = self.scan("scan_locs", {"level": near["level"],
            "x": near["x"], "z": near["z"], "radius": NEXT_ENTRY})
        if state["map"] != stamp:
            raise Stop("map changed while choosing installed loc")
        candidates = [row for row in rows if row["definition"] in definitions
            or row["base_definition"] in definitions]
        candidates = [row for row in candidates if distance(row, near) <= NEXT_ENTRY]
        if exact_tile is not None:
            candidates = [row for row in candidates if tile_key(row) == tile_key(exact_tile)]
            if len(candidates) != NEXT_ENTRY:
                raise Stop(f"generated crossing leaf is not uniquely installed: {exact_tile}; scan={candidates}")
            leaf_pose = candidates[FIRST_SLOT]
            if leaf_pose["angle"] != exact_tile["angle"] or leaf_pose["shape"] != exact_tile["form"]:
                raise Stop(f"native crossing leaf pose differs from generated geometry: {exact_tile}; leaf={leaf_pose}")
        candidates.sort(key=lambda row: (distance(row, near), row["x"], row["z"]))
        for row in candidates:
            try:
                _, label = self.operation(row, operations)
                return row, label
            except Stop:
                continue
        raise Stop(f"installed loc/cache operation not found: ids={sorted(definitions)}, near={near}, ops={operations}; scan={candidates}")

    def loc_action(self, row, label):
        return {"kind": "loc", **{key: row[key] for key in (
            "definition", "x", "z", "level", "shape", "angle")}, "operation": label}

    def dig(self, crypt):
        self.stage = f"dig:{crypt['brother']}"
        self.walk(crypt["mound"], crypt["digRadius"])
        state = self.observe()
        before = state["map"]
        if crypt["digLocations"]:
            near = crypt["mound"]
            rows, stamp = self.scan("scan_locs", {"level": near["level"],
                "x": near["x"], "z": near["z"], "radius": crypt["digRadius"]})
            choices = [row for row in rows if any(row["definition"] == source["loc"]
                and tile_key(row) == tile_key(source) for source in crypt["digLocations"])]
            if not choices:
                raise Stop("generated Dig-with loc not installed; no synthetic fallback")
            row = min(choices, key=lambda value: distance(value, state["player"]))
            _, label = self.operation(row, ("Dig-with",))
            after = self.act(self.loc_action(row, label), state)
        else:
            state = self.open_backpack()
            item = self.symbols.get("obj", "spade")
            row = self.item_control(state, "backpack.slots", item, ("Dig",))
            after = self.act(self.ui_action(row, ("Dig",), item), state)
        state = self.wait("installed copied crypt after native Dig", lambda s:
            s["map"]["terrain_generation"] != before["terrain_generation"]
            and s.get("region") is not None, after)
        expected = Layout(state).resolve(crypt["arrival"])
        if distance(state["player"], expected) > NEXT_ENTRY:
            self.wait("mapped crypt arrival", lambda s: distance(s["player"], expected) <= NEXT_ENTRY,
                state["cycle"])
        self.log("dig_installed", {"brother": crypt["brother"], "expected_arrival": expected,
            "meaning": "decoded map install only; recorder must provide actual post-Dig PNG/readback"})

    def live_brothers(self, state=None):
        state = state or self.observe()
        definitions = {crypt["profile"]["npc"]: crypt for crypt in self.rules["crypts"]}
        rows, stamp = self.scan("scan_npcs", {"level": state["player"]["level"]})
        if state["map"] != stamp:
            raise Stop("map changed during NPC selection")
        return [(row, definitions[row["definition"]]) for row in rows
                if row["definition"] in definitions and self.bit(state, definitions[row["definition"]]["slainBit"]) == 0]

    def fight(self, row, crypt):
        self.stage = f"fight:{crypt['brother']}"
        state = self.observe()
        fight_map = state["map"]
        passive = self.receipts.player()
        candidates = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
                      if enemy["definition"] == crypt["profile"]["npc"]
                      and enemy["id"] == row["index"] and enemy["instance"] == passive["instance"]]
        if len(candidates) != NEXT_ENTRY:
            raise Stop("client NPC index lacks a matching independent current-life receipt")
        current_life = candidates[FIRST_SLOT]["generation"]
        # Find the independently observed initial full-health publication, not a weakened actor.
        full = [{**enemy, "tick": receipt["tick"]} for receipt in self.receipts.rows if receipt.get("kind") == "state"
                for enemy in receipt.get("enemies", [])
                if enemy["definition"] == crypt["profile"]["npc"]
                and enemy["id"] == row["index"] and enemy["generation"] == current_life
                and enemy["hitpoints"] == crypt["profile"]["hitpoints"]
                and enemy["instance"] == passive["instance"]]
        if not full:
            raise Stop(f"brother {crypt['brother']} lacks passive full-HP spawn evidence")
        self.full_health[crypt["brother"]] = full[FIRST_SLOT]
        self.eat_if_needed(state)
        protection_active = self.set_protection(crypt, True)
        self.open_backpack()
        # UI input can span actual NPC updates. Select this same intended actor
        # again, retaining the source map and independently observed life.
        state = self.observe()
        rows, stamp = self.scan("scan_npcs", {"definition": row["definition"],
            "level": state["player"]["level"]})
        if stamp != fight_map or state["map"] != fight_map:
            raise Stop("source map changed during pre-Attack Prayer input")
        matching = [actor for actor in rows if actor["index"] == row["index"]
            and actor["definition"] == row["definition"]]
        if len(matching) != NEXT_ENTRY:
            raise Stop("same intended brother was not actually re-observed after Prayer input")
        current_player = self.receipts.player()
        lives = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
            if enemy["definition"] == row["definition"] and enemy["id"] == row["index"]
            and enemy["generation"] == current_life and enemy["instance"] == passive["instance"]
            and enemy.get("visible") and enemy["hitpoints"] > FIRST_SLOT]
        if current_player["instance"] != passive["instance"] or len(lives) != NEXT_ENTRY:
            raise Stop("passive intended brother life/instance changed during Prayer input")
        row = matching[FIRST_SLOT]
        _, label = self.operation(row, ("Attack",))
        state = self.observe()
        if state["map"] != fight_map:
            raise Stop("source map changed after the actual post-Prayer NPC rescan")
        rule = self.protection_rule(crypt)
        protection_value = self.bit(state, rule["activationBit"])
        if protection_value not in (FIRST_SLOT, NEXT_ENTRY):
            raise Stop("post-UI Prayer PLAYER activation bit was not actually observed")
        protection_active = protection_value == NEXT_ENTRY
        self.log("fight_target_rescanned", {"brother": crypt["brother"], "cycle": state["cycle"],
            "map": state["map"], "actor": row, "passive_generation": current_life,
            "prayer": rule, "protection_observed": protection_active,
            "remaining_prayer": self.bit(state, "current_prayer_points")})
        try:
            after = self.act({"kind": "npc", **{key: row[key] for key in (
                "index", "definition", "update_serial")}, "operation": label}, state)
        except UnacceptedNpcIntent as refusal:
            # Only the refused intent can be refreshed, once. Native admission
            # still checks the new serial; an accepted Attack is never repeated.
            state = self.observe()
            rows, stamp = self.scan("scan_npcs", {"definition": row["definition"],
                "level": state["player"]["level"]})
            if stamp != fight_map or state["map"] != fight_map:
                raise Stop("source map changed after unaccepted NPC intent") from refusal
            matching = [actor for actor in rows if actor["index"] == row["index"]
                and actor["definition"] == row["definition"]]
            current_player = self.receipts.player()
            lives = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
                if enemy["definition"] == row["definition"] and enemy["id"] == row["index"]
                and enemy["generation"] == current_life and enemy["instance"] == passive["instance"]
                and enemy.get("visible") and enemy["hitpoints"] > FIRST_SLOT]
            if (len(matching) != NEXT_ENTRY or len(lives) != NEXT_ENTRY
                    or current_player["instance"] != passive["instance"]):
                raise Stop("same intended NPC life was not re-observed after refusal") from refusal
            row = matching[FIRST_SLOT]
            _, label = self.operation(row, ("Attack",))
            self.log("unaccepted_attack_refreshed", {"brother": crypt["brother"],
                "refusal": refusal.response, "actor": row, "map": stamp,
                "passive_generation": current_life, "maximum_refreshes": NEXT_ENTRY})
            after = self.act({"kind": "npc", **{key: row[key] for key in (
                "index", "definition", "update_serial")}, "operation": label}, state)
        deadline = min(self.deadline, time.monotonic() + self.args.fight_seconds)
        last_progress = time.monotonic()
        prior_signature = None
        while time.monotonic() < deadline:
            state = self.observe()
            if state.get("ready") and state["cycle"] > after and self.bit(state, crypt["slainBit"]) == NEXT_ENTRY:
                self.completed.add(crypt["brother"])
                self.log("brother_slain", {"brother": crypt["brother"], "cycle": state["cycle"]})
                self.set_protection(crypt, False)
                self.open_backpack()
                return
            if state.get("ready"):
                rule = self.protection_rule(crypt)
                protection_value = self.bit(state, rule["activationBit"])
                if protection_value not in (FIRST_SLOT, NEXT_ENTRY):
                    raise Stop("in-fight Prayer PLAYER activation bit was not actually observed")
                if protection_active and protection_value != NEXT_ENTRY:
                    self.log("protection_ended", {"brother": crypt["brother"], "prayer": rule["name"],
                        "cycle": state["cycle"], "remaining_prayer": self.bit(state, "current_prayer_points"),
                        "value": self.bit(state, rule["activationBit"]), "policy": "no automatic action retry or refill"})
                    protection_active = False
                self.eat_if_needed(state)
                rows, _ = self.scan("scan_npcs", {"definition": row["definition"]})
                signature = json.dumps([(actor["index"], actor.get("hits"), actor.get("stats"))
                                       for actor in rows], sort_keys=True)
                if signature != prior_signature:
                    last_progress, prior_signature = time.monotonic(), signature
            if time.monotonic() - last_progress > self.args.stall_seconds:
                raise Stop(f"fight stalled for {crypt['brother']}; raw target is not reinterpreted or re-Attacked blindly; {self.diagnostic(state)}")
            time.sleep(POLL_SECONDS)
        raise Stop(f"brother fight deadline: {crypt['brother']}; {self.diagnostic(state)}")

    def tunnel_option(self, text):
        # Select an actually loaded native pause row by its own text subtree,
        # not an inferred component stride or a numerical dialogue answer.
        state = self.observe()
        first = self.component(state, "dialogue_options.first_option")
        if first is None or not first["value"].get("rooted_visible"):
            raise Stop("actual native option container is not visibly rooted")
        layer = first["value"].get("layer")
        if isinstance(layer, bool) or not isinstance(layer, int) or layer < FIRST_SLOT:
            raise Stop("actual native option row has no installed parent layer")
        parent = {"parent": layer, "child": STATIC_COMPONENT_CHILD}
        matches = []
        # Observe direct sibling pause rows under the actual native option
        # container. Only each row's text subtree is traversed recursively.
        for row in self.children(parent):
            value = row.get("value") or {}
            if not value.get("rooted_visible") or not (value.get("active_mask", FIRST_SLOT) & (NEXT_ENTRY << CONTINUE_OPERATION)):
                continue
            texts = [row] + self.children(row["target"], recursive=True)
            if any(normal_text((child.get("value") or {}).get("text")) == normal_text(text)
                   for child in texts):
                matches.append(row)
        if len(matches) != NEXT_ENTRY:
            raise Stop(f"actual native pause row is absent/ambiguous for {text}: {matches}")
        self.log("tunnel_option_observed", {"text": text, "row": matches[FIRST_SLOT]})
        return matches[FIRST_SLOT]

    def tunnel_dialogue(self, enter):
        state = self.observe()
        if not self.visible(state, "dialogue_options.first_option"):
            raise Stop("actual tunnel choice interface is absent")
        source_map = state["map"]
        text = TUNNEL_ENTER_TEXT if enter else TUNNEL_STAY_TEXT
        row = self.tunnel_option(text)
        state = self.observe()
        if state["map"] != source_map:
            raise Stop("source map changed during tunnel option observation")
        after = self.act(self.ui_action(row, continuation=True), state)
        if not enter:
            state = self.wait("native Stay here closes tunnel dialogue in the same crypt", lambda s:
                s["map"] == source_map and not self.visible(s, "dialogue_options.first_option"), after)
            self.log("tunnel_choice_deferred", {"brother": self.tunnel["brother"],
                "observed_option": row, "cycle": state["cycle"], "map": state["map"],
                "next": "ordinary stairs after actual dialogue dismissal"})
            return
        self.wait("tunnel dialogue closes and current player reaches generated tunnel arrival", lambda s:
            not self.visible(s, "dialogue_options.first_option") and any(
                distance(s["player"], Layout(s).resolve(tile)) <= NEXT_ENTRY
                for tile in self.rules["tunnelArrival"]), after)

    def search(self, crypt, enter=False):
        self.stage = f"search:{crypt['brother']}"
        state = self.observe()
        if self.visible(state, "dialogue_options.first_option"):
            raise Stop("Search requires the previous dialogue actually closed before submission")
        source_map = state["map"]
        near = Layout(state).resolve(crypt["sarcophagusTile"])
        row, label = self.select_loc({crypt["sarcophagus"]}, near, ("Search",))
        after = self.act(self.loc_action(row, label), state)

        def intended_living_brother(state):
            choices = [(actor, facts) for actor, facts in self.live_brothers(state)
                       if facts["brother"] == crypt["brother"]]
            player = self.receipts.player()
            enemies = (self.receipts.last_state or {}).get("enemies", [])
            return [(actor, facts) for actor, facts in choices if any(
                enemy["id"] == actor["index"] and enemy["definition"] == actor["definition"]
                and enemy["instance"] == player["instance"] and enemy.get("visible")
                and enemy["hitpoints"] > FIRST_SLOT for enemy in enemies)]

        def searched(state):
            if state["map"] != source_map:
                raise Stop("installed map changed before the intended Search outcome")
            exact = intended_living_brother(state)
            dialogue = self.visible(state, "dialogue_options.first_option")
            if len(exact) > NEXT_ENTRY or (exact and dialogue):
                raise Stop("Search outcome is ambiguous between intended living brother and dialogue")
            return dialogue or len(exact) == NEXT_ENTRY

        state = self.wait("fresh tunnel choice or intended living brother after Search", searched, after)
        if self.visible(state, "dialogue_options.first_option"):
            if self.tunnel is not None and self.tunnel["brother"] != crypt["brother"]:
                raise Stop("a second different crypt produced a fresh tunnel choice")
            self.tunnel = crypt
            self.tunnel_dialogue(enter)
            return
        exact = intended_living_brother(state)
        if len(exact) != NEXT_ENTRY:
            raise Stop(f"Search lacks one intended current living brother: {exact}")
        self.fight(*exact[FIRST_SLOT])

    def stairs(self, crypt):
        self.stage = f"stairs:{crypt['brother']}"
        state = self.observe()
        near = Layout(state).resolve(crypt["staircaseTile"])
        row, label = self.select_loc({crypt["staircase"]}, near, ("Climb-up", "Climb up"))
        after = self.act(self.loc_action(row, label), state)
        self.wait("ordinary staircase exterior", lambda s: s.get("region") is None
            and distance(s["player"], crypt["surface"]) <= NEXT_ENTRY, after)

    def puzzle(self, door, after):
        state = self.wait("actual native puzzle models", lambda s:
            self.visible(s, f"barrows_pattern_choices.{BUTTON_NAMES[FIRST_SLOT]}"), after)
        sequence = [self.component(state, f"barrows_pattern_choices.{name}")["value"]["model"]
                    for name in SEQUENCE_NAMES]
        answers = []
        for name in ANSWER_NAMES:
            row = self.component(state, f"barrows_pattern_choices.{name}")
            if row and row["value"]["rooted_visible"]:
                answers.append(row["value"]["model"])
        matches = [puzzle for puzzle in self.rules["puzzles"]
                   if puzzle["sequence"] == sequence and puzzle["answers"] == answers]
        if len(matches) != NEXT_ENTRY:
            raise Stop(f"actual puzzle models ambiguous/unmatched: sequence={sequence},answers={answers}")
        correct = matches[FIRST_SLOT]["correct"]
        row = self.component(state, f"barrows_pattern_choices.{BUTTON_NAMES[correct]}")
        # This owning native control's operation1 is generated by BarrowsWindows.answer.
        # Prefer the actual label; refuse if absent rather than assume an arbitrary button.
        labels = row["value"].get("ops") or []
        label = labels[FIRST_SLOT] if labels else None
        if not label:
            raise Stop("owning Barrows answer operation1 has no current native menu label")
        after = self.act(self.ui_action(row, (label,)), state)
        result = self.wait("correct native puzzle opens its door", lambda s:
            self.bit(s, door["bit"]) == NEXT_ENTRY
            and not self.visible(s, f"barrows_pattern_choices.{BUTTON_NAMES[FIRST_SLOT]}"), after)
        receipt = {"sequence": sequence, "answers": answers, "chosen": correct,
                   "door_bit": door["bit"], "cycle": result["cycle"]}
        self.puzzle_receipts.append(receipt)
        self.log("puzzle", receipt)

    def approach_targets(self, target, width=1, length=1):
        # Candidate adjacent tiles are only route requests. Terrain flags are not
        # interpreted as a standability oracle; ordinary server routing decides.
        candidates = []
        for x in range(target["x"], target["x"] + width):
            candidates.extend([{"level": target["level"], "x": x, "z": target["z"] - NEXT_ENTRY},
                               {"level": target["level"], "x": x, "z": target["z"] + length}])
        for z in range(target["z"], target["z"] + length):
            candidates.extend([{"level": target["level"], "x": target["x"] - NEXT_ENTRY, "z": z},
                               {"level": target["level"], "x": target["x"] + width, "z": z}])
        return candidates

    def remember_crossing(self, door, near, far, state):
        run = self.receipts.player().get("run") or {}
        generation = run.get("generation")
        if generation is None or door["bit"] not in run.get("openedDoors", []):
            raise Stop("confirmed native crossing lacks the same saved open-door run")
        if distance(state["player"], far) != FIRST_SLOT or state["player"]["route_length"] != FIRST_SLOT:
            raise Stop("door breadcrumb requires actual settled far-side arrival")
        receipt = {"door_bit": door["bit"], "near": dict(near), "far": dict(far),
            "map": dict(state["map"]), "generation": generation, "cycle": state["cycle"]}
        if self.tunnel_crossings:
            previous = self.tunnel_crossings[-NEXT_ENTRY]
            if (previous["door_bit"] == receipt["door_bit"]
                    and previous["map"] == receipt["map"]
                    and previous["generation"] == generation
                    and previous["near"] == receipt["far"]
                    and previous["far"] == receipt["near"]):
                self.tunnel_crossings.pop()
                self.log("door_crossed", {**receipt, "path_effect": "reverse_last_crossing"})
                return
        if len(self.tunnel_crossings) >= MAX_NAVIGATION_STEPS:
            raise Stop("observed door path exceeds finite navigation bound")
        self.tunnel_crossings.append(receipt)
        self.log("door_crossed", {**receipt, "path_effect": "remember_confirmed_crossing"})

    def reverse_crossings(self):
        """Walk back across physically open boundaries using observed endpoints."""
        while self.tunnel_crossings:
            prior = self.tunnel_crossings[-NEXT_ENTRY]
            state = self.observe()
            run = self.receipts.player().get("run") or {}
            if state["map"] != prior["map"] or run.get("generation") != prior["generation"]:
                raise Stop("map or saved run changed before observed return crossing")
            doors = [door for door in self.rules["doors"] if door["bit"] == prior["door_bit"]]
            if (len(doors) != NEXT_ENTRY or self.bit(state, prior["door_bit"]) != NEXT_ENTRY
                    or prior["door_bit"] not in run.get("openedDoors", [])):
                raise Stop("observed return path lacks its unique currently open native door")
            # Swung native leaves do not expose Open. Both steps remain normal
            # Walk requests through the actual server collision/pathfinder.
            self.walk(prior["far"])
            state = self.observe()
            if state["map"] != prior["map"]:
                raise Stop("map changed while approaching the recorded far-side boundary")
            self.walk(prior["near"])
            result = self.observe()
            run = self.receipts.player().get("run") or {}
            if result["map"] != prior["map"] or run.get("generation") != prior["generation"]:
                raise Stop("map or saved run changed after native return Walk")
            if (self.bit(result, prior["door_bit"]) != NEXT_ENTRY
                    or prior["door_bit"] not in run.get("openedDoors", [])
                    or distance(result["player"], prior["near"]) != FIRST_SLOT
                    or result["player"]["route_length"] != FIRST_SLOT):
                raise Stop("normal return Walk lacks settled original near-side arrival")
            self.tunnel_crossings.pop()
            self.log("door_returned", {"door_bit": prior["door_bit"],
                "near": prior["far"], "far": prior["near"], "map": result["map"],
                "generation": prior["generation"], "cycle": result["cycle"],
                "input_provenance": "ordinary native Walk through physically open boundary",
                "original_crossing_cycle": prior["cycle"]})

    def route(self, source, definitions, operations, return_path=False):
        """Online door exploration, with every crossing confirmed in client state."""
        self.stage = "tunnel_route"
        self.tunnel_retaliation_off()
        if return_path:
            self.reverse_crossings()
        attempted = set()
        for _ in range(MAX_NAVIGATION_STEPS):
            state = self.observe()
            for row, crypt in self.live_brothers(state):
                self.fight(row, crypt)
                state = self.observe()
            goal = Layout(state).resolve(source)
            row, label = self.select_loc(definitions, goal, operations)
            width, length = row["width"], row["length"]
            if row["angle"] % 2:
                width, length = length, width
            edges = self.approach_targets(row, width, length)
            edges.sort(key=lambda edge: distance(edge, state["player"]))
            if state["player"]["route_length"] == 0 and any(distance(state["player"], edge) == 0 for edge in edges):
                return row, label
            # A direct ordinary route may already pass physically open doors.
            for edge in edges:
                key = ("goal", tile_key(state["player"]), tile_key(edge))
                if key in attempted:
                    continue
                attempted.add(key)
                if self.walk(edge, allow_partial=True):
                    return self.select_loc(definitions, Layout(self.observe()).resolve(source), operations)
            state = self.observe()
            layout = Layout(state)
            choices = []
            for door in self.rules["doors"]:
                for placement in door["placements"]:
                    dx, dz = DOOR_FACING[placement["angle"] & ROTATION_MASK]
                    source_sides = [placement, {"level": placement["level"],
                        "x": placement["x"] + dx, "z": placement["z"] + dz}]
                    sides = [layout.resolve(tile) for tile in source_sides]
                    for near, far in (sides, list(reversed(sides))):
                        key = ("door", door["bit"], tile_key(near), tile_key(far))
                        if key in attempted:
                            continue
                        score = distance(state["player"], near) + distance(far, goal)
                        leaf = {**layout.resolve(placement), "angle": placement["angle"], "form": placement["form"]}
                        choices.append((score, key, door, near, far, leaf))
            choices.sort(key=lambda value: (value[FIRST_SLOT], str(value[NEXT_ENTRY])))
            crossed = False
            for _, key, door, near, far, leaf_tile in choices:
                attempted.add(key)
                if not self.walk(near, allow_partial=True):
                    continue
                state = self.observe()
                was_open = self.bit(state, door["bit"]) == NEXT_ENTRY
                source_map = state["map"]
                if was_open:
                    if not self.walk(far, allow_partial=True):
                        continue
                    result = self.observe()
                    if result["map"] != source_map:
                        raise Stop("map changed after normal open-boundary Walk")
                else:
                    leaf, op = self.select_loc(set(door["locs"]), near, ("Open",), exact_tile=leaf_tile)
                    state = self.observe()
                    source_map = state["map"]
                    self.log("door_crossing_intent", {"door_bit": door["bit"],
                        "placement": leaf_tile, "near": near, "far": far, "leaf": leaf,
                        "cycle": state["cycle"], "map": source_map})
                    after = self.act(self.loc_action(leaf, op), state)
                    if door["puzzle"]:
                        self.puzzle(door, after)
                    def arrived(observed):
                        if observed["map"] != source_map:
                            raise Stop("map changed after accepted native door operation")
                        return (self.bit(observed, door["bit"]) == NEXT_ENTRY
                            and distance(observed["player"], far) == FIRST_SLOT
                            and observed["player"]["route_length"] == FIRST_SLOT)
                    result = self.wait("native door bit and settled far-side boundary crossing", arrived, after)
                self.remember_crossing(door, near, far, result)
                crossed = True
                break
            if not crossed:
                raise Stop(f"no reachable untried generated door or destination; {self.diagnostic()}")
        raise Stop(f"bounded tunnel navigation exhausted; {self.diagnostic()}")

    def chest(self):
        self.stage = "chest"
        chest = self.rules["chest"]
        ids = {chest["baseLoc"], chest["closedLoc"], chest["openLoc"]}
        row, label = self.route(chest["tile"], ids, ("Open", "Search", "Quick-loot", "Quickloot"))
        state = self.observe()
        if any(self.bit(state, crypt["slainBit"]) == 0 for crypt in self.rules["crypts"]):
            after = self.act(self.loc_action(row, label), state)
            state = self.wait("hidden brother summons on ordinary chest operation", lambda s:
                bool(self.live_brothers(s)), after)
            choices = self.live_brothers(state)
            if len(choices) != NEXT_ENTRY:
                raise Stop("hidden brother selection is not unique")
            self.fight(*choices[FIRST_SLOT])
            row, label = self.route(chest["tile"], ids, ("Open", "Search", "Quick-loot", "Quickloot"))
        state = self.observe()
        if not all(self.bit(state, crypt["slainBit"]) == NEXT_ENTRY for crypt in self.rules["crypts"]):
            raise Stop("chest refuses driver completion with fewer than six native slain bits")
        after = self.act(self.loc_action(row, label), state)
        self.wait("ordinary chest pending inventory and native UI", lambda s:
            self.bit(s, "barrows_reward_opened") == NEXT_ENTRY
            and self.visible(s, "loot_claim_window.item_area"), after)

    def take_rewards(self):
        self.stage = "take_rewards"
        for _ in range(MAX_SCAN_ENTRIES):
            state = self.observe()
            pending = self.inventory(state, "barrows_pending_rewards")
            entries = [(item, count) for item, count in zip(pending["items"], pending["counts"])
                       if item >= 0 and count > 0]
            if not entries:
                self.wait("all pending rewards durably removed", lambda s:
                    not any(count > 0 for count in self.inventory(s, "barrows_pending_rewards")["counts"]),
                    state["cycle"])
                passive = self.receipts.player()
                run = passive.get("run") or {}
                if not run.get("claimed") or run.get("rewards"):
                    raise Stop("empty client pending inventory lacks claimed/empty durable run receipt")
                self.completed_generation = run
                self.log("claimed_run", {"run": run, "coins": passive["coins"],
                    "backpack": passive["backpack"]})
                return
            item, _ = entries[FIRST_SLOT]
            amount_before = self.count(pending, item)
            backpack_before = self.count(self.inventory(state, "backpack"), item)
            wallet_before = sum(self.inventory(state, "coin_wallet")["counts"])
            row = self.item_control(state, "loot_claim_window.item_area", item, ("Take",))
            after = self.act(self.ui_action(row, ("Take",), item), state)
            def transferred(state):
                pending_after = self.count(self.inventory(state, "barrows_pending_rewards"), item)
                amount = amount_before - pending_after
                if amount <= 0:
                    return False
                if item == self.symbols.get("obj", "coins"):
                    return sum(self.inventory(state, "coin_wallet")["counts"]) - wallet_before == amount
                return self.count(self.inventory(state, "backpack"), item) - backpack_before == amount

            result = self.wait("single native Take pending/destination transaction", transferred, after)
            passive = self.receipts.player()
            remaining = sum(reward["count"] for reward in (passive.get("run") or {}).get("rewards", [])
                            if reward["item"] == item)
            if remaining != self.count(self.inventory(result, "barrows_pending_rewards"), item):
                raise Stop("client Take publication lacks passive durably changed pending run")
            receipt = {"target": row["target"], "item": item, "before": amount_before,
                "after": self.count(self.inventory(result, "barrows_pending_rewards"), item),
                "server_pending": remaining, "cycle": result["cycle"]}
            self.withdrawals.append(receipt)
            self.log("take", receipt)
        raise Stop("reward withdrawal exceeds finite bound")

    def rope(self):
        self.stage = "rope_exit"
        state = self.observe()
        ropes = [rope for rope in self.rules["ropes"] if self.bit(state, rope["bit"]) == NEXT_ENTRY]
        if len(ropes) != NEXT_ENTRY:
            raise Stop("actual installed active rope bit is not unique")
        rope = ropes[FIRST_SLOT]
        row, label = self.route(rope["tile"], {rope["baseLoc"], rope["loc"]}, ("Climb-up", "Climb up"), return_path=True)
        after = self.act(self.loc_action(row, label), self.observe())
        self.wait("native rope leaves copied encounter", lambda s: s.get("region") is None
            and distance(s["player"], self.tunnel["surface"]) <= NEXT_ENTRY, after)

    def acceptance(self, old_generation):
        self.receipts.refresh()
        final = self.receipts.player()
        deaths = [row for row in self.receipts.rows if row.get("kind") == "death"
                  and row.get("target") == "npc"]
        for crypt in self.rules["crypts"]:
            spawn = self.full_health.get(crypt["brother"])
            if spawn is None or not any(row["targetId"] == spawn["id"]
                and row["targetDefinition"] == crypt["profile"]["npc"]
                and row["tick"] >= spawn["tick"]
                and row.get("sourceId") == final["pid"] for row in deaths):
                raise Stop(f"missing independent full-HP/credited death pair: {crypt['brother']}")
        if not self.food_receipts or not self.puzzle_receipts or not self.withdrawals:
            raise Stop("native food, model puzzle and durable Take all remain required")
        if self.completed_generation is None or len(self.completed_generation.get("killed", [])) != len(self.rules["crypts"]):
            raise Stop("six-brother claimed durable run receipt missing before reset")
        run = final.get("run") or {}
        if run.get("generation") == old_generation or run.get("killed") or run.get("claimed"):
            raise Stop("passive new-run reset not independently confirmed")
        return {"status": "semantic_pass_requires_recorded_replay_and_readback_qualification",
            "six_full_health": self.full_health, "credited_deaths": deaths,
            "food": self.food_receipts, "puzzles": self.puzzle_receipts,
            "withdrawals": self.withdrawals, "old_generation": old_generation,
            "claimed_run": self.completed_generation,
            "new_generation": run["generation"], "final_client": self.state,
            "final_server": final, "requests": self.requests, "actions": self.actions,
            "limitations": ["No pixel parity/FPS claim", "Menu command is not screen body-picking",
                "Native recorder separately owns window ledger/focus/readback/cleanup",
                "Bob/POH repair is not implemented by this driver and must be qualified before final launch"]}

    def run(self):
        initial = self.observe()
        initial = self.wait("ready native Legacy game", lambda s:
            self.bit(s, "legacy_combat_active") == NEXT_ENTRY
            and (self.bit(s, "current_life_points") or 0) > 0 and s.get("player") is not None,
            initial["cycle"])
        if initial.get("region") is not None:
            raise Stop("driver requires declared initial exterior fixture, not arbitrary live resume")
        self.equip()
        self.stage = "navigation_preferences"
        self.tunnel_retaliation_off()
        for crypt in self.rules["crypts"]:
            self.dig(crypt)
            self.search(crypt)
            self.stairs(crypt)
        if self.tunnel is None or len(self.completed) != len(self.rules["crypts"]) - NEXT_ENTRY:
            raise Stop("five killed ordinary brothers plus one observed tunnel choice required")
        self.dig(self.tunnel)
        self.search(self.tunnel, enter=True)
        old_generation = self.receipts.player()["run"]["generation"]
        self.chest()
        self.take_rewards()
        self.rope()
        self.stage = "new_run_reset"
        self.dig(self.rules["crypts"][FIRST_SLOT])
        state = self.observe()
        self.wait("native slain/reward bits reset on actual re-entry", lambda s:
            all(self.bit(s, crypt["slainBit"]) == 0 for crypt in self.rules["crypts"])
            and self.bit(s, "barrows_reward_opened") == 0, state["cycle"])
        self.stage = "semantic_acceptance"
        return self.acceptance(old_generation)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--control-module", type=Path, required=True)
    parser.add_argument("--socket", required=True)
    parser.add_argument("--rules", type=Path, required=True)
    parser.add_argument("--food-rules", type=Path, required=True)
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--symbols", type=Path, required=True)
    parser.add_argument("--receipts", type=Path, required=True)
    parser.add_argument("--journal", type=Path, required=True)
    parser.add_argument("--result", type=Path, required=True)
    parser.add_argument("--diagnostics-root", type=Path, required=True)
    parser.add_argument("--deadline-seconds", type=float, default=DEFAULT_SESSION_SECONDS)
    parser.add_argument("--action-seconds", type=float, default=DEFAULT_ACTION_SECONDS)
    parser.add_argument("--fight-seconds", type=float, default=DEFAULT_FIGHT_SECONDS)
    parser.add_argument("--stall-seconds", type=float, default=DEFAULT_STALL_SECONDS)
    parser.add_argument("--socket-seconds", type=float, default=DEFAULT_SOCKET_SECONDS)
    args = parser.parse_args()
    if not 0 < args.deadline_seconds <= MAX_SESSION_SECONDS:
        parser.error("session deadline exceeds finite driver policy")
    if not all(0 < value <= args.deadline_seconds for value in (
        args.action_seconds, args.fight_seconds, args.stall_seconds, args.socket_seconds)):
        parser.error("action/fight/stall/socket deadlines must be bounded by session")
    if not args.diagnostics_root.is_absolute() or not args.diagnostics_root.is_dir():
        parser.error("diagnostics root must be an explicit existing absolute directory")
    private_root = args.diagnostics_root.resolve()
    for path in (args.journal, args.result):
        if not path.resolve().is_relative_to(private_root):
            parser.error("driver diagnostic writes must stay within the private bosses root")
        if path.exists() or not path.parent.is_dir():
            parser.error("diagnostic paths must be fresh files in an existing private folder")
    if args.journal.resolve() == args.result.resolve():
        parser.error("journal and result must be distinct fresh files")
    control, driver = None, None
    result = {"status": "failed", "reason": "driver initialization did not complete"}
    try:
        spec = importlib.util.spec_from_file_location("ordinary_client_control", args.control_module)
        if spec is None or spec.loader is None:
            raise Stop("explicit normal control module is unavailable")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        control = module.Control(args.socket, args.socket_seconds)
        driver = Driver(control, args)
        result = driver.run()
        return 0
    except (Stop, OSError, ValueError, KeyError, IndexError, TypeError, RuntimeError) as error:
        result = {"status": "failed", "reason": str(error),
                  "last_receipt": driver.state if driver else None,
                  "stage": driver.stage if driver else "initialization"}
        print(json.dumps(result, sort_keys=True), file=sys.stderr)
        return 1
    finally:
        if control is not None:
            control.close()
        if driver is not None:
            driver.journal.close()
        # Private diagnostic output only. Never writes a cache/account/scene file.
        with args.result.open("x", encoding="utf8") as output:
            json.dump(result, output, sort_keys=True, indent=2)
            output.write("\n")


if __name__ == "__main__":
    raise SystemExit(main())
