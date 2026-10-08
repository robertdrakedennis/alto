"""Private combined continuation: ordinary travel, Bob and installed POH repair.

Unrun source proposal. This extends the preserved fight driver; no account or
game-state writer exists here. Initial declarations are seed-combined.mjs only.
"""
import importlib.util
import time
from pathlib import Path

_spec = importlib.util.spec_from_file_location("preserved_barrows_fight", Path(__file__).with_name("driver.py"))
_base = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_base)
Stop = _base.Stop
ONE = _base.NEXT_ENTRY
ZERO = _base.FIRST_SLOT
INSTANCE_INDEX = 2
STANDARD_TELEPORT_DEFAULT = 0
NEAREST_HALF_DENOMINATOR = 2
MAX_EXTERIOR_ROUTE_SEGMENTS = 64
MAX_WEAR_TARGET_CHOICES = 8
ROUTE_EDGE_MARGIN = _base.CHUNK_TILES
REPAIR_TEXT = "Repair"
REPAIR_QUESTION = "Repair this equipment for {price} coins?"


def same_physical(rows, key):
    result = [slot for slot in rows if len(slot) > INSTANCE_INDEX
              and slot[INSTANCE_INDEX].get("key") == key]
    if len(result) != ONE:
        raise Stop(f"expected exactly one physical instance {key}, observed {result}")
    return result[ZERO]


class RepairDriver(_base.Driver):
    def __init__(self, control, args):
        super().__init__(control, args)
        self.charge_rows = _base.load_json(args.charge_rules)["families"]
        self.charge_by_item = {row["item"]: row for row in self.charge_rows}
        self.repair = self.plan.get("repair")
        if not self.repair or not self.repair.get("sourceOnlyInitialFixture"):
            raise Stop("combined run requires the explicitly declared initial repair fixture")
        self.hood_key = self.repair["hood"]["key"]
        self.repair_proofs = []
        self.travel_proofs = []
        self.query["components"] += [self.symbols.component(name) for name in (
            "minimap.home_teleport_button", "lodestone_network.lumbridge_button",
            "lodestone_network.burthorpe_button")]
        self.query["varbits"] += [self.symbols.get("varbit", "quick_teleport_by_default")]
        self.log("repair_inputs", {"charges": {"path": str(args.charge_rules),
            "sha256": _base.digest(args.charge_rules)}, "initial_fixture": self.repair,
            "meaning": "Declared before login; no live inventory/skill/house mutations"})

    def physical(self, player=None):
        player = player or self.receipts.player()
        return same_physical(player["backpack"] + player["worn"], self.hood_key)

    def batch_price(self, player, fraction=None):
        fraction = fraction or (ONE, ONE)
        numerator, denominator = fraction
        total, damaged = ZERO, ZERO
        for slot in player["backpack"] + player["worn"]:
            family = self.charge_by_item.get(slot[ZERO])
            if not family or not family.get("coinRepair") or slot[ONE] != ONE:
                continue
            if len(slot) > INSTANCE_INDEX:
                charges = slot[INSTANCE_INDEX]["charges"]
            elif family["depleted"]["kind"] == "identity" and family["depleted"]["item"] == family["item"]:
                charges = ZERO
            else:
                charges = family["freshCharges"]
            if charges is None or charges >= family["capacity"]:
                continue
            if not isinstance(charges, int) or charges < ZERO:
                raise Stop("ordinary repair cohort has an unresolved charge balance")
            top = family["coinRepair"]["fullCost"] * (family["capacity"] - charges) * numerator
            bottom = family["capacity"] * denominator
            if fraction == (ONE, ONE):
                price = (top + bottom - ONE) // bottom
            else:
                # Nonnegative integer equivalent of the existing Math.round.
                price = (top * NEAREST_HALF_DENOMINATOR + bottom) // (bottom * NEAREST_HALF_DENOMINATOR)
            total += price
            damaged += ONE
        if damaged == ZERO:
            raise Stop("native repair must have an actual damaged physical cohort")
        return total

    def wait_lodestone_main_complete(self, state, place, deadline):
        # Installed destination is not animation completion. Keep the original
        # Teleport deadline; do not submit Walk or infer an animation duration.
        stamp = state["map"]
        main = state.get("player", {}).get("animation", {}).get("main")
        if not isinstance(main, dict) or not isinstance(main.get("sequence"), int):
            raise Stop("native arrival main observation is unavailable")
        selected = main["sequence"]
        last_cycle = state["cycle"] - ONE
        last_node = None
        while time.monotonic() < deadline:
            if state.get("ready") and state["cycle"] > last_cycle:
                if state["map"] != stamp or state.get("region") is not None:
                    raise Stop("installed lodestone map changed before main completion")
                player = state["player"]
                if _base.distance(player, place) > ONE or player["route_length"] != ZERO:
                    raise Stop("lodestone actor moved or acquired a route before completion")
                main = player.get("animation", {}).get("main")
                if not isinstance(main, dict) or not isinstance(main.get("sequence"), int):
                    raise Stop("native arrival main observation is unavailable")
                node = {key: main.get(key) for key in (
                    "sequence", "skeletal", "frame", "time", "delay", "finished",
                    "moving_priority", "stationary_priority")}
                if node != last_node:
                    self.log("lodestone_main_observation", {"cycle": state["cycle"],
                        "map": state["map"], "main": node})
                    last_node = node
                # The ordinary movement gate tests sequence presence, not
                # finished. A still-installed finished node must also clear.
                if main["sequence"] < ZERO:
                    self.log("postcondition", {"name": "ordinary lodestone main cleared",
                        "cycle": state["cycle"], "map": state["map"], "player": player,
                        "packets_applied": state.get("packets_applied")})
                    return state
                if main["sequence"] != selected:
                    raise Stop("arrival main was replaced before observed completion")
                last_cycle = state["cycle"]
            remaining = deadline - time.monotonic()
            if remaining <= ZERO:
                break
            time.sleep(min(_base.POLL_SECONDS, remaining))
            state = self.observe(deadline=deadline)
        raise Stop(f"lodestone main did not clear within original action deadline; {self.diagnostic(state)}")

    def teleport(self, destination):
        self.stage = f"native_lodestone:{destination}"
        state = self.observe()
        if self.bit(state, "quick_teleport_by_default") != STANDARD_TELEPORT_DEFAULT:
            raise Stop("ordinary Teleport option requires observed standard default; no charge/variable override")
        home = self.component(state, "minimap.home_teleport_button")
        if home is None:
            raise Stop("native Home Teleport control is not currently loaded")
        labels = home["value"].get("ops") or []
        label = labels[ZERO] if labels else None
        if not label or _base.normal_text(label) != "lodestone network":
            raise Stop("actual Home first operation does not open the native Lodestone network")
        # Ordinary Home operation1 opens the network; operation2 uses the prior
        # destination even when its current native display label is identical.
        action = self.ui_action(home, continuation=True)
        action["operation"] = _base.FIRST_OPERATION
        action["expected_operation"] = label
        after = self.act(action, state)
        button_name = f"lodestone_network.{destination}_button"
        state = self.wait("native lodestone network mounted", lambda s: self.visible(s, button_name), after)
        button = self.component(state, button_name)
        before = state["map"]
        after = self.act(self.ui_action(button, ("Teleport",)), state)
        deadline = min(self.deadline, time.monotonic() + self.args.action_seconds)
        place = self.repair["travel"][destination]
        remaining = deadline - time.monotonic()
        if remaining <= ZERO:
            raise Stop("original lodestone action deadline exhausted before arrival")
        state = self.wait("ordinary start-unlocked lodestone arrival", lambda s:
            s.get("region") is None and s["map"] != before
            and _base.distance(s["player"], place) <= ONE
            and s["player"]["route_length"] == ZERO, after, deadline=deadline)
        state = self.wait_lodestone_main_complete(state, place, deadline)
        self.travel_proofs.append({"destination": destination, "client": state,
            "server": self.receipts.player(), "policy": "normal start-unlocked, no Quick Teleport or seeded flags"})

    def travel_walk(self, destination, radius=ZERO):
        for _ in range(MAX_EXTERIOR_ROUTE_SEGMENTS):
            state = self.observe()
            if state.get("region") is not None:
                raise Stop("exterior route cannot manufacture a copied-map exit")
            if _base.distance(state["player"], destination) <= radius and state["player"]["route_length"] == ZERO:
                return
            stamp = state["map"]
            # Finite current-scene route probes; collision/pathfinder is the ordinary server's.
            lower_x, upper_x = stamp["base_x"] + ROUTE_EDGE_MARGIN, stamp["base_x"] + stamp["width"] - ROUTE_EDGE_MARGIN
            lower_z, upper_z = stamp["base_z"] + ROUTE_EDGE_MARGIN, stamp["base_z"] + stamp["height"] - ROUTE_EDGE_MARGIN
            probe = {"level": destination["level"],
                "x": min(upper_x, max(lower_x, destination["x"])),
                "z": min(upper_z, max(lower_z, destination["z"]))}
            before = _base.tile_key(state["player"])
            if not self.walk(probe, radius if probe == destination else ZERO, allow_partial=True):
                if _base.tile_key(self.state["player"]) == before:
                    raise Stop(f"no ordinary exterior route progress; {self.diagnostic()}")
        raise Stop("bounded exterior route exhausted; no teleport or guessed doorway fallback")

    def native_repair(self, action, kind, fraction=None):
        state = self.observe()
        before = self.receipts.player()
        original = self.physical(before)
        price = self.batch_price(before, fraction)
        offset = len(self.receipts.rows)
        after = self.act(action, state)
        state = self.wait("native Repair confirmation mounted", lambda s:
            self.visible(s, "dialogue_options.first_option"), after)
        first = self.component(state, "dialogue_options.first_option")
        if first is None or not first["value"].get("rooted_visible"):
            raise Stop("actual native repair option is not visibly rooted")
        option_group = first["target"]["parent"] >> _base.COMPONENT_GROUP_SHIFT
        roots = [row for row in self.children(self.symbols.component("game_window.dialogue_slot"))
                 if row.get("relation") == "mounted"
                 and row["target"]["child"] == _base.STATIC_COMPONENT_CHILD
                 and row["target"]["parent"] >> _base.COMPONENT_GROUP_SHIFT == option_group
                 and (row.get("value") or {}).get("rooted_visible")]
        if len(roots) != ONE:
            raise Stop("native repair quote lacks exactly one rooted option-group mount")
        # Native quote text belongs to the header beside the option container.
        # Observe only the mounted root, its direct children and their direct
        # children; do not recursively traverse the whole dialogue namespace.
        direct = self.children(roots[ZERO]["target"])
        rows = roots + direct
        for row in direct:
            if (row.get("value") or {}).get("rooted_visible"):
                rows.extend(self.children(row["target"]))
        texts = [str((row.get("value") or {}).get("text") or "") for row in rows
                 if (row.get("value") or {}).get("rooted_visible")]
        question = REPAIR_QUESTION.format(price=price)
        if not any(_base.normal_text(text) == _base.normal_text(question) for text in texts):
            raise Stop(f"native quote does not equal captured ordinary price {price}; texts={texts}")
        first = self.component(state, "dialogue_options.first_option")
        descendants = self.children(first["target"], recursive=True)
        if not any(_base.normal_text((row.get("value") or {}).get("text")) == _base.normal_text(REPAIR_TEXT)
                   for row in [first] + descendants):
            raise Stop("native first pause option does not carry Repair text")
        after = self.act(self.ui_action(first, continuation=True), state)
        self.wait("ordinary durable Repair commit", lambda s: any(row.get("kind") == "repair_commit"
            and row.get("method") == "prepareCoinRepairBatch" and row.get("price") == price
            and row.get("pid") == before["pid"] for row in self.receipts.rows[offset:]), after)
        matches = [row for row in self.receipts.rows[offset:] if row.get("kind") == "repair_commit"
            and row.get("pid") == before["pid"] and row.get("price") == price]
        if len(matches) != ONE:
            raise Stop("repair confirmation lacks exactly one accepted original commit")
        commit = matches[ZERO]
        repaired = same_physical(commit["after"]["backpack"] + commit["after"]["worn"], self.hood_key)
        saved = same_physical(commit["saved"]["backpack"] + commit["saved"]["worn"], self.hood_key)
        family = self.repair["hood"]["family"]
        if repaired != saved or repaired[ZERO] != family["coinRepair"]["output"] or repaired[INSTANCE_INDEX]["charges"] != family["capacity"]:
            raise Stop("repair did not persist same physical UUID/full balance/native output")
        if commit["before"]["coins"] - commit["after"]["coins"] != price or commit["saved"]["coins"] != commit["after"]["coins"]:
            raise Stop("repair wallet debit is not the exact saved quote")
        self.wait("native repaired inventory publication", lambda s:
            self.count(self.inventory(s, "backpack"), repaired[ZERO]) +
            self.count(self.inventory(s, "worn_equipment"), repaired[ZERO]) >= ONE, after)
        proof = {"kind": kind, "price": price, "fraction": fraction or (ONE, ONE),
            "before_physical": original, "commit": commit, "quote_texts": texts, "client": self.state}
        self.repair_proofs.append(proof)
        self.log("durable_repair", proof)

    def bob_repair(self):
        self.stage = "bob_repair"
        bob = self.repair["bob"]
        self.travel_walk(bob["spawn"], radius=ONE)
        rows, stamp = self.scan("scan_npcs", {"definition": bob["npc"], "level": bob["spawn"]["level"]})
        if len(rows) != ONE or stamp != self.state["map"]:
            raise Stop("normal generated Bob is absent/ambiguous in installed map")
        row = rows[ZERO]
        _, label = self.operation(row, (bob["operation"],))
        self.native_repair({"kind": "npc", "index": row["index"], "definition": row["definition"],
            "update_serial": row["update_serial"], "operation": label}, "Bob native Repair-all")

    def ordinary_wear(self):
        self.stage = "ordinary_outgoing_wear"
        state = self.open_backpack()
        fresh = self.repair["hood"]["family"]["item"]
        row = self.item_control(state, "backpack.slots", fresh, ("Wear", "Wield"))
        after = self.act(self.ui_action(row, ("Wear", "Wield"), fresh), state)
        self.wait("same repaired physical hood worn", lambda s:
            self.count(self.inventory(s, "worn_equipment"), fresh) == ONE, after)
        wear = self.repair["wear"]
        self.travel_walk(wear["spawn"], radius=ONE)
        rows, stamp = self.scan("scan_npcs", {"definition": wear["spawn"]["npc"], "level": wear["spawn"]["level"]})
        rows = sorted(rows, key=lambda row: _base.distance(row, self.state["player"]))[:MAX_WEAR_TARGET_CHOICES]
        if not rows or stamp != self.state["map"]:
            raise Stop("generated wear target absent from installed normal map")
        # One actual native target; no fake damage, spawned replacement or charge assignment.
        row = rows[ZERO]
        _, label = self.operation(row, (wear["operation"],))
        before = self.physical()
        offset = len(self.receipts.rows)
        after = self.act({"kind": "npc", "index": row["index"], "definition": row["definition"],
            "update_serial": row["update_serial"], "operation": label}, self.observe())
        self.wait("real outgoing launch plus physical charge decrement", lambda s:
            self.physical()[INSTANCE_INDEX]["charges"] < before[INSTANCE_INDEX]["charges"]
            and any(receipt.get("kind") == "launch" and receipt.get("source") == "player"
                and receipt.get("sourceId") == self.receipts.player()["pid"]
                and receipt.get("targetDefinition") == row["definition"] for receipt in self.receipts.rows[offset:]),
            after, supervise_food=True)
        self.wait("normal target death before travel", lambda s: any(receipt.get("kind") == "death"
            and receipt.get("targetDefinition") == row["definition"]
            and receipt.get("sourceId") == self.receipts.player()["pid"] for receipt in self.receipts.rows[offset:]),
            self.state["cycle"], supervise_food=True)
        self.log("ordinary_wear", {"before": before, "after": self.physical(),
            "target": row, "receipts": self.receipts.rows[offset:]})

    def house_entry(self):
        self.stage = "ordinary_house_entry"
        portal = self.repair["house"]["portal"]
        self.travel_walk(portal["exit"], radius=ONE)
        rows, stamp = self.scan("scan_locs", {"definition": portal["loc"], "level": portal["exit"]["level"]})
        if len(rows) != ONE or stamp != self.state["map"]:
            raise Stop("actual named house portal absent/ambiguous; no invented placement offset")
        row = rows[ZERO]
        _, label = self.operation(row, ("Enter house",))
        state = self.observe()
        old = state["map"]
        after = self.act(self.loc_action(row, label), state)
        self.wait("actual ordinary owned-house instance", lambda s:
            s.get("region") is not None and s["map"] != old and
            self.receipts.player().get("instance") is not None and
            self.receipts.player().get("house") == self.repair["house"]["saved"], after)

    def stand_repair(self):
        self.stage = "installed_armour_stand_repair"
        stand = self.repair["house"]["stand"]
        rows, stamp = self.scan("scan_locs", {"definition": stand["loc"], "level": self.state["player"]["level"]})
        if len(rows) != ONE or stamp != self.state["map"]:
            raise Stop("actual installed armour stand is absent/ambiguous")
        row = rows[ZERO]
        ops = row.get("ops") or []
        label = ops[stand["allOperation"] - ONE] if len(ops) >= stand["allOperation"] else None
        if not label or _base.normal_text(label) not in {"repair", "repair-all"}:
            raise Stop("actual stand operation is not the generated native repair option")
        level = self.receipts.player().get("smithing")
        if not isinstance(level, int) or level < ZERO:
            raise Stop("actual captured Smithing level receipt is missing")
        fraction = (max(ZERO, stand["smithingDenominator"] - level), stand["smithingDenominator"])
        self.native_repair(self.loc_action(row, label), "Installed armour stand native Repair", fraction)

    def house_exit(self):
        self.stage = "ordinary_house_exit"
        # Content identity is named; actual copied destination is selected from installed locs.
        definitions = _base.symbol_rows(self.symbols.directory / "loc.sym")
        exit_id = int(definitions["house_exit_portal"])
        rows, stamp = self.scan("scan_locs", {"definition": exit_id, "level": self.state["player"]["level"]})
        if len(rows) != ONE or stamp != self.state["map"]:
            raise Stop("installed garden Exit portal missing/ambiguous")
        row = rows[ZERO]
        _, label = self.operation(row, ("Enter", "Exit", "Leave", "Leave house"))
        after = self.act(self.loc_action(row, label), self.observe())
        self.wait("ordinary house exit at named exterior portal", lambda s:
            s.get("region") is None and self.receipts.player().get("instance") is None
            and _base.distance(s["player"], self.repair["house"]["portal"]["exit"]) <= ONE, after)

    def run(self):
        fight = super().run()
        # Fight acceptance/reset is retained before leaving the fresh empty crypt.
        self.stairs(self.rules["crypts"][ZERO])
        self.teleport("lumbridge")
        self.bob_repair()
        self.ordinary_wear()
        self.teleport("burthorpe")
        self.house_entry()
        self.stand_repair()
        self.house_exit()
        paid = self.receipts.player()["coins"]
        final_physical = self.physical()
        self.house_entry()
        if self.receipts.player()["coins"] != paid or self.physical() != final_physical:
            raise Stop("ordinary house re-entry altered repaired balance/wallet")
        self.house_exit()
        if len(self.repair_proofs) != INSTANCE_INDEX:
            raise Stop("both Bob and POH durable/native repairs remain required")
        run = self.receipts.player().get("run") or {}
        if run.get("generation") != fight["new_generation"] or run.get("killed") or run.get("claimed"):
            raise Stop("post-repair saved run no longer matches the observed empty reset run")
        fight["limitations"] = [item for item in fight["limitations"] if "Bob/POH" not in item]
        fight["repairs"] = self.repair_proofs
        fight["travel"] = self.travel_proofs
        fight["final_repair_player"] = self.receipts.player()
        fight["final_repair_client"] = self.observe()
        fight["limitations"] += ["Repair travel/control source is unqualified until owning tests and this actual recording",
            "Root separately owns input/controller/native replay/readback/cleanup acceptance"]
        return fight
