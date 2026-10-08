"""Data-only qualification of an ordinary Corp capture; no game owner imports."""
ZERO = 0
ONE = 1
FULL_BEAST_LIFE = 100000
FULL_CORE_LIFE = 3500
GAME_TICK_MILLISECONDS = 600

def require(condition, reason):
    if not condition:
        raise ValueError(reason)

def same_actor(left, right):
    return all(left.get(key) == right.get(key) for key in ("kind", "id", "generation", "definition", "instance", "actorToken"))

def qualify(plan, initial, journal, combat):
    stages = {name: [row for row in journal if row["kind"] == name]
              for name in ("enter", "ordinary_melee_sample", "leave", "rejoin", "scope_narrowed", "complete")}
    for name in plan["recordingScope"]["required"] + ["complete"]:
        require(len(stages[name]) == ONE, "Missing/repeated required Corp stage: " + name)
    enter = stages["enter"][ZERO]
    require(enter["source"]["instance"] is None and enter["source"]["x"] < enter["loc"]["x"]
            and enter["passive"]["membership"] and enter["passive"]["room"] == plan["publicRoom"]
            and enter["state"]["player"]["x"] > enter["loc"]["x"], "Public same-square directional entry differs")
    sample = stages["ordinary_melee_sample"][ZERO]
    boss = sample["fullLife"]
    through_tick = sample["throughTick"]
    final_body = sample["finalBody"]
    require(same_actor(boss, final_body) and ZERO < final_body["hitpoints"] < FULL_BEAST_LIFE, "Sample original damaged living body differs")
    scoped = [row for row in combat if row["tick"] <= through_tick]
    require(boss["hitpoints"] == FULL_BEAST_LIFE == boss["maximumLife"]
            and boss["definition"] == plan["target"]["npc"]
            and boss["instance"] == enter["passive"]["instance"], "Shortened/foreign Corp life")
    require(not any(row["kind"] == "death" and same_actor(row["target"], boss) for row in scoped), "Full kill cannot be relabelled a sample")
    player = enter["passive"]
    hits = [row for row in scoped if row["kind"] == "landed" and same_actor(row["target"], boss)]
    require(hits and all(row["source"]["kind"] == "player" and row["source"]["id"] == player["pid"]
            and row["source"]["generation"] == player["generation"] for row in hits), "Unowned or empty boss damage")
    restriction = plan["restriction"]
    require(len({hit.get("weapon", {}).get("physicalKey") for hit in hits}) == ONE,
            "Ordinary physical weapon identity changed during the fight")
    for hit in hits:
        weapon = hit.get("weapon")
        forms = restriction["selectedWeapon"]["forms"]
        require(weapon and any(form["item"] == weapon["item"] and form["category"] == weapon["category"] for form in forms)
                and weapon.get("physicalKey") is not None and weapon["hand"] == "main",
                "Ordinary launch lacks expected frozen physical main-hand weapon")
        require(weapon["item"] not in restriction["fullDamageWeapons"]
                and weapon["category"] not in restriction["fullDamageClasses"], "Non-spear capture substituted full-damage weapon")
        expected = hit["launchedDamage"] * restriction["scale"]["numerator"] // restriction["scale"]["denominator"]
        require(hit["nativeDamage"] == expected and ZERO <= hit["actualDamage"] <= expected,
                "Actual native incoming damage differs from frozen half-damage restriction")
    healing = [row for row in scoped if row["kind"] == "npc_healing" and same_actor(row["target"], boss)]
    for row in healing:
        require(row["afterLife"] - row["beforeLife"] == row["acceptedHealing"]
                and ZERO <= row["acceptedHealing"] <= row["requested"]
                and row["afterLife"] <= FULL_BEAST_LIFE, "Observed ordinary NPC healing differs")
    require(sum(row["actualDamage"] for row in hits) == FULL_BEAST_LIFE - final_body["hitpoints"] + sum(row["acceptedHealing"] for row in healing),
            "Normal accepted damage does not account for actual sample HP loss plus healing")
    incoming = [row for row in scoped if row["kind"] == "landed" and same_actor(row["source"], boss)
                and row["target"]["id"] == player["pid"] and row["actualDamage"] > ZERO]
    foods = [row for row in scoped if row["kind"] == "food_commit"]
    require(incoming and foods, "Sample requires actual body incoming damage and ordinary food")
    require(sample["player"]["prayerFine"] < player["prayerFine"], "Ordinary selected prayer did not consume its actual initial resource")
    for food in (row for row in combat if row["kind"] == "food_commit"):
        require(food["healed"] == food["lifeAfter"] - food["lifeBefore"] and food["healed"] > ZERO
                and food["afterCount"] == ZERO and food["beforeCount"] == ONE, "Food was not an actual ordinary commit")
    observations = {"fullCore": [], "drainAndHealing": [], "positiveHitPause": [], "poisonSuppression": [], "bossDeathStop": []}
    snapshots = [row for row in combat if row["kind"] == "state"]
    core_rows = [enemy for row in snapshots for enemy in row["enemies"]
                 if enemy["definition"] == plan["core"]["npc"] and enemy["instance"] == boss["instance"]]
    lives = {(row["id"], row["generation"], row["actorToken"]) for row in core_rows}
    for index, generation, actor_token in lives:
        full = [row for row in core_rows if row["id"] == index and row["generation"] == generation and row["actorToken"] == actor_token
                and row["hitpoints"] == FULL_CORE_LIFE == row["maximumLife"]]
        require(full, "Naturally present core lacks ordinary full-health publication")
        observations["fullCore"].append(full[ZERO])
    core_hits = [row for row in combat if row["kind"] == "landed"
                 and row.get("source", {}).get("definition") == plan["core"]["npc"]
                 and row["source"]["instance"] == boss["instance"]]
    for hit in core_hits:
        require(hit["target"]["id"] == player["pid"] and hit["target"]["instance"] == boss["instance"]
                and hit["source"]["level"] == hit["target"]["level"]
                and max(abs(hit["source"]["x"] - hit["target"]["x"]), abs(hit["source"]["z"] - hit["target"]["z"])) <= plan["core"]["radiusTiles"]
                and ZERO <= hit["launchedDamage"] <= plan["core"]["maximumDamage"], "Natural core hit escaped native cohort/3x3/max contract")
        matches = [row for row in combat if row["kind"] == "npc_healing" and same_actor(row["target"], boss) and row["tick"] == hit["tick"] and row["requested"] == hit["actualDamage"]]
        require(len(matches) == ONE, "Natural core damage lacks actual ordinary boss-healing receipt")
        observations["drainAndHealing"].append({"hit": hit, "healing": matches[ZERO]})
    positive = [row for row in combat if row["kind"] == "landed"
                and row.get("target", {}).get("definition") == plan["core"]["npc"] and row["actualDamage"] > ZERO]
    for hit in positive:
        end = hit["tick"] + (plan["core"]["pauseMilliseconds"] + GAME_TICK_MILLISECONDS - ONE) // GAME_TICK_MILLISECONDS
        window = [row for row in snapshots if hit["tick"] < row["tick"] < end
                  and any(same_actor(enemy, hit["target"]) and enemy["currentLife"] > ZERO for enemy in row["enemies"])]
        if window and window[-ONE]["tick"] >= end - ONE:
            require(not any(same_actor(row["source"], hit["target"]) and hit["tick"] < row["tick"] < end for row in core_hits), "Natural positive-hit pause failed")
            observations["positiveHitPause"].append({"hit": hit, "throughTick": window[-ONE]["tick"]})
    poisoned = [(row["tick"], enemy) for row in snapshots for enemy in row["enemies"]
                if enemy["definition"] == plan["core"]["npc"] and enemy.get("poisoned") and enemy["currentLife"] > ZERO]
    for tick, core in poisoned:
        previous = any(prior_tick == tick - ONE and same_actor(prior_core, core)
                       for prior_tick, prior_core in poisoned)
        if previous:
            require(not any(hit["tick"] == tick and same_actor(hit["source"], core) for hit in core_hits), "Naturally continuously poisoned core still drained")
            observations["poisonSuppression"].append({"tick": tick, "actor": core,
                "meaning": "Two consecutive poisoned live phases; the first application tick is not misclassified"})
    full_route = len(stages["leave"]) == ONE and len(stages["rejoin"]) == ONE
    require(full_route != bool(stages["scope_narrowed"]), "Route omission was not explicitly qualified")
    if full_route:
        leave, rejoin = stages["leave"][ZERO], stages["rejoin"][ZERO]
        require(leave["source"]["membership"] and leave["source"]["x"] > leave["loc"]["x"]
                and leave["passive"]["instance"] is None and leave["state"]["player"]["x"] < leave["loc"]["x"]
                and rejoin["passive"]["instance"] == boss["instance"], "Ordinary directional exit/rejoin differs")
    return {"status": "ordinary_native_melee_sample_qualified", "scope": "bounded public native melee sample with exit/rejoin" if full_route else "bounded public native melee sample; exit/rejoin omitted",
        "initialHealth": FULL_BEAST_LIFE, "acceptedPlayerDamage": sum(row["actualDamage"] for row in hits),
        "acceptedBossHealing": sum(row["acceptedHealing"] for row in healing), "remainingBossHealth": final_body["hitpoints"], "sampleThroughTick": through_tick,
        "fullKillClaimed": False, "bossLootClaimed": False,
        "naturalCoreObservations": observations,
        "notObserved": [name for name, values in observations.items() if not values],
        "remainingSocketCoverage": plan["recordingScope"]["excluded"],
        "blockedMechanics": plan["core"]["blocked"], "unqualifiedMechanics": plan["core"].get("limitations", []), "owningReplayPassed": False, "rendered": False}
