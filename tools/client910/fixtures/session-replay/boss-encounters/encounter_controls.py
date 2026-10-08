"""Source-only common helpers for the existing ordinary encounter Driver.

Adopt by composing this mixin before boss.Driver. Supply the already retained
Stop/UnacceptedNpcIntent types and a symbol-resolved plan; create no transport,
processes or server control endpoint here. Boss policies supply completion
predicates, not timing, RNG, hit amounts or live state mutations.
"""

import re

ABSENT_ITEM = -1
ZERO = 0
ONE = 1
ITEM_FIELD = 0
PHYSICAL_FIELD = 2


class EncounterControlsMixin:
    def encounter_failure(self, message):
        raise self.encounter_stop_type(message)

    def passive_player(self):
        self.receipts.refresh()
        return self.receipts.player()

    def eat_if_needed(self, state, minimum_life=None):
        # Initial stock is a finite resource. Empty stock is an observation;
        # death/full-life/outcome assertions remain with observe and the policy.
        passive = self.passive_player()
        permitted = self.encounter_can_consume(state, passive)
        if permitted is not True:
            return False
        food = self.encounter_recipe["foodItem"]
        if self.count(self.inventory(state, "backpack"), food) == ZERO:
            if not getattr(self, "empty_food_reported", False):
                self.log("initial_food_exhausted", {"state": state, "passive": passive})
                self.empty_food_reported = True
            return False
        # Retain the current successful food transaction/cooldown/receipt owner.
        # Its supported food must match this resolved recipe before adoption.
        return super().eat_if_needed(state, minimum_life)

    def encounter_step(self, name, intent, completed, seconds=None):
        state = self.observe()
        if completed(state):
            return state
        self.eat_if_needed(state)
        state = self.observe()
        if completed(state):
            return state
        after = intent(state)
        return self.wait(name, completed, after, seconds=seconds, supervise_food=True)

    def scan_near(self, kind, radius, definition=None):
        state = self.observe()
        player = state["player"]
        rows, stamp = self.scan(kind, {"definition": definition, "x": player["x"],
            "z": player["z"], "level": player["level"], "radius": radius})
        if stamp != state["map"]:
            self.encounter_failure("Nearby scan crossed its observed installed map epoch")
        return rows, stamp

    def observed_life(self, definition, full_life=None, prior=None):
        # Native filtering addresses resolved visible IDs, not passive/raw IDs.
        native, stamp = self.scan_near("scan_npcs", self.encounter_recipe["scanRadius"])
        state = self.observe()
        if state["map"] != stamp:
            self.encounter_failure("Native actor scan crossed its installed map epoch")
        player = self.passive_player()
        native = [row for row in native if row["base_definition"] == definition
            and row["level"] == state["player"]["level"]
            and (prior is None or row["index"] == prior["actor"])]
        lives = [enemy for enemy in (self.receipts.last_state or {}).get("enemies", [])
            if enemy["definition"] == definition and enemy["instance"] == player["instance"]
            and enemy.get("visible") and enemy["hitpoints"] > ZERO
            and any(row["index"] == enemy["id"] for row in native)]
        if len(native) != ONE or len(lives) != ONE:
            self.encounter_failure("One installed actor and independently observed live owner are required")
        row, life = native[ZERO], lives[ZERO]
        bound = {"actor": row["index"], "definition": definition, "generation": life["generation"],
            "instance": player["instance"], "pid": player["pid"], "playerGeneration": player["generation"],
            "actorToken": life.get("actorToken"), "level": row["level"], "map": stamp, "fullLife": full_life,
            "observedDefinitions": [definition], "visibleDefinition": row["definition"],
            "updateSerial": row["update_serial"]}
        # Visible definition and update serial qualify this scan/input only;
        # later publications may change them without replacing the raw actor.
        if full_life is not None and life["hitpoints"] != full_life:
            self.encounter_failure("Full native life is absent; do not reset or shorten it")
        if prior is not None and any(bound[key] != prior[key] for key in
                ("actor", "definition", "generation", "instance", "pid", "playerGeneration", "actorToken", "level")):
            self.encounter_failure("Observed actor/life/member changed")
        return row, bound

    def attack_observed(self, binding, completed):
        # Observe completion before demanding a still-positive target life.
        state = self.observe()
        if completed(state):
            return state
        # Accepted input is never repeated merely because damage has not arrived.
        row, current = self.observed_life(binding["definition"], prior=binding)
        source_map = current["map"]
        _, operation = self.operation(row, ("Attack",))
        offset = len(self.receipts.rows)
        try:
            after = self.scanned_action("npc", row, operation)
        except self.encounter_unaccepted_type as refusal:
            self.actions -= ONE
            state = self.observe()
            if completed(state):
                return state
            row, current = self.observed_life(binding["definition"], prior=binding)
            if current["map"] != source_map:
                self.encounter_failure("Refused action refresh crossed an installed map epoch")
            self.log("unaccepted_attack_refreshed", {"refusal": refusal.response,
                "binding": current, "maximumRefreshes": ONE})
            _, operation = self.operation(row, ("Attack",))
            after = self.scanned_action("npc", row, operation)
        def admitted(state):
            if completed(state):
                return True
            player = self.passive_player()
            if any(player[key] != binding[bound_key] for key, bound_key in
                    (("pid", "pid"), ("generation", "playerGeneration"), ("instance", "instance"))):
                self.encounter_failure("Attack admission lost its player life/member")
            target = player.get("target") or {}
            if target.get("id") == binding["actor"] and target.get("generation") == binding["generation"]:
                return True
            return any(receipt.get("kind") == "launch"
                and (receipt.get("source") or {}).get("id") == binding["pid"]
                and (receipt.get("source") or {}).get("generation") == binding["playerGeneration"]
                and (receipt.get("target") or {}).get("id") == binding["actor"]
                and (receipt.get("target") or {}).get("generation") == binding["generation"]
                for receipt in self.receipts.rows[offset:])
        return self.wait("ordinary admitted pursuit or observed completion", admitted, after, supervise_food=True)

    def observed_transition(self, prior, definition, generation_delta=ZERO):
        # Only the small encounter policy calls this after its native phase/form
        # publication predicate. All other targeting keeps the exact definition.
        row, current = self.observed_life(definition)
        keys = ("actor", "actorToken", "instance", "pid", "playerGeneration", "level", "map")
        if any(current[key] != prior[key] for key in keys) or current["generation"] != prior["generation"] + generation_delta:
            self.encounter_failure("Published form transition replaced the actor/life/member/map")
        current["fullLife"] = prior["fullLife"]
        # Admit only definitions actually scanned on this same phase/life owner.
        current["observedDefinitions"] = list(dict.fromkeys([*prior["observedDefinitions"], definition]))
        self.log("native_form_observed", {"before": prior, "after": current})
        return row, current

    def physical_item(self, held, row, allow_fresh=False):
        if held["capacity"] is None:
            return
        actual = row[PHYSICAL_FIELD] if len(row) > PHYSICAL_FIELD else None
        previous = self.physical_owners.get(held["item"])
        if actual is None and allow_fresh and previous is None:
            return  # Native Wield may create this prelogin item's first instance.
        if (not actual or not actual.get("key") or not ZERO < actual["charges"] <= held["capacity"]
                or (previous and (actual["key"] != previous["key"] or actual["charges"] > previous["charges"]))):
            self.encounter_failure("Equipment switched identity, refilled, or lost positive native balance")
        self.physical_owners[held["item"]] = {"key": actual["key"], "charges": actual["charges"]}

    def equipped_observed(self, state, held):
        native = self.inventory(state, "worn_equipment")["items"]
        passive = self.passive_player()["worn"]
        slot = held["slot"]
        if slot >= len(native) or slot >= len(passive):
            self.encounter_failure("Native equipment slot is absent")
        if native[slot] not in held["identities"] or passive[slot][ITEM_FIELD] != native[slot]:
            return False
        self.physical_item(held, passive[slot])
        return True

    def equip_observed(self, kit):
        self.physical_owners = getattr(self, "physical_owners", {})
        self.open_backpack()
        for held in kit["equipment"]:
            state = self.observe()
            self.eat_if_needed(state)
            state = self.observe()
            if self.equipped_observed(state, held):
                continue
            identities = [item for item in held["identities"]
                if self.count(self.inventory(state, "backpack"), item) > ZERO]
            if len(identities) != ONE:
                self.encounter_failure("Initial/displaced native kit item is absent or ambiguous")
            item = identities[ZERO]
            sources = [row for row in self.passive_player()["backpack"] if row[ITEM_FIELD] == item]
            if len(sources) != ONE:
                self.encounter_failure("Physical source item is absent or ambiguous")
            self.physical_item(held, sources[ZERO], allow_fresh=True)
            row = self.item_control(state, "backpack.slots", item, ("Wear", "Wield"))
            after = self.act(self.ui_action(row, ("Wear", "Wield"), item), state)
            self.wait("native equipment and physical owner publication", lambda current, held=held:
                self.equipped_observed(current, held), after, supervise_food=True)

    def prayer_observed(self, rule, enabled):
        self.open_window(self.plan["toolbar"]["prayerDestination"], "prayer_book.prayer_buttons", "Prayer")
        state = self.observe()
        wanted = ONE if enabled else ZERO
        active = self.bit(state, rule["activationBit"])
        remaining = self.bit(state, "current_prayer_points")
        if active not in (ZERO, ONE) or remaining is None:
            self.encounter_failure("Actual native prayer activation/resource publication is absent")
        if active == wanted:
            return state
        if enabled and remaining <= ZERO:
            self.log("initial_prayer_exhausted", {"rule": rule, "state": state})
            return state
        parent = self.symbols.get("component", "prayer_book.prayer_buttons")
        rows = [row for row in self.children(self.symbols.component("prayer_book.prayer_buttons"))
            if row["target"] == {"parent": parent, "child": rule["button"]}
            and (row.get("value") or {}).get("rooted_visible")]
        if len(rows) != ONE:
            self.encounter_failure("Current native prayer control is absent or ambiguous")
        label = ("Activate " if enabled else "Deactivate ") + rule["name"]
        normalize = self.encounter_normal_text
        choices = tuple(actual for actual in rows[ZERO]["value"].get("ops", []) if actual
            and normalize(re.sub(r"</?col(?:=[^>]*)?>", "", actual)) == normalize(label))
        after = self.act(self.ui_action(rows[ZERO], choices), state)
        return self.wait("native prayer activation publication", lambda current:
            self.bit(current, rule["activationBit"]) == wanted
            or (enabled and self.bit(current, "current_prayer_points") == ZERO), after, supervise_food=True)

    def exact_loc_observed(self, pose, operation):
        # pose is resolved from the observed instance template or public map;
        # baseDefinition may be a dynamic replacement's actual new base.
        rows, stamp = self.scan("scan_locs", {"x": pose["x"], "z": pose["z"],
            "level": pose["level"], "radius": ZERO})
        matches = [row for row in rows if row["definition"] == pose["definition"]
            and row["base_definition"] == pose["baseDefinition"]
            and row["shape"] == pose["shape"] and row["angle"] == pose["angle"]]
        if len(matches) != ONE:
            self.encounter_failure("Exact placed/native loc identity or orientation is absent")
        _, label = self.operation(matches[ZERO], (operation,))
        state = self.observe()
        if state["map"] != stamp or state["player"]["level"] != pose["level"]:
            self.encounter_failure("Loc selection crossed an installed map/plane epoch")
        return self.loc_action(matches[ZERO], label), state

    def require_completion_budget(self, binding, offset):
        impacts = [row for row in self.receipts.rows[offset:] if row.get("kind") == "landed"
            and (row.get("source") or {}).get("id") == binding["pid"]
            and (row.get("source") or {}).get("generation") == binding["playerGeneration"]
            and (row.get("target") or {}).get("id") == binding["actor"]
            and (row.get("target") or {}).get("generation") == binding["generation"]
            and (row.get("target") or {}).get("definition") in binding["observedDefinitions"]
            and (row.get("target") or {}).get("instance") == binding["instance"]
            and (binding["actorToken"] is None
                or (row.get("target") or {}).get("actorToken") == binding["actorToken"])]
        if binding["fullLife"] is None or sum(row["actualDamage"] for row in impacts) != binding["fullLife"]:
            self.encounter_failure("Observed completion lacks exact ordinary full-life accounting")
        return impacts
