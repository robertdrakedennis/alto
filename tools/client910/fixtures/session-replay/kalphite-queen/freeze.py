#!/usr/bin/env python3
"""Freeze a qualified whole ordinary two-form Queen recording."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import struct
from collections import Counter

ONE = 1
ZERO = 0
INITIAL_LEVEL = 99
MELEE_WEAPON_TIER = 92
RANGED_WEAPON_TIER = 90
ARMOUR_TIER = 90
PHYSICAL_INSTANCE_FIELD = 2
BYTE_RECORD_LIMIT = 256 * 1024 * 1024
RECORD_HEADER_BYTES = 12
FILE_HEADER_BYTES = 8
RECORD_VERSION = 0x10000
FIRST_RECORD_CYCLE = -1
CLOCK_SAMPLE_BYTES = 8
MAX_CLOCK_SAMPLES = 16384


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def load(path):
    return json.loads(Path(path).read_text())


def rows(path):
    raw = Path(path).read_bytes()
    require(raw.endswith(b"\n"), "Partial evidence row")
    return [json.loads(line) for line in raw.splitlines()]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("spec")
    parser.add_argument("destination")
    args = parser.parse_args()
    spec = load(args.spec)
    raw, root, out = Path(spec["out"]), Path(spec["root"]), Path(args.destination)
    require(not out.exists(), "Fresh fixture destination required")
    result = load(raw / "result.json")
    require(result["passed"] and result["allDirectChildrenWaited"] and result["sourceInputsUnchanged"]
            and not result["ownedSurvivors"] and not result["cleanupErrors"], "Capture owner did not qualify")
    processes = rows(raw / "processes.jsonl") if (raw / "processes.jsonl").exists() else load(raw / "processes.json")
    require(all(row.get("waited") and isinstance(row.get("exit"), int) for row in processes), "Unwaited child")
    for name in ("core", "driver", "broker", "wired-guard"):
        found = [row for row in processes if row["name"] == name and not row.get("query")]
        require(len(found) == ONE and found[ZERO]["exit"] == ZERO, "Nonzero ordinary process: " + name)
    for helper in spec["files"].values():
        require(sha(helper["path"]) == helper["sha256"], "Changed frozen helper")
    for relative, expected in load(raw / "source-input-epoch.json").items():
        require(sha(root / relative) == expected, "Changed captured source or generated input: " + relative)
    work = Path(result["work"])
    plan, initial = load(work / "plan.json"), load(work / "initial-fixture.json")
    saved = load(initial["accountPath"])
    require(all(skill["level"] == INITIAL_LEVEL for skill in initial["account"]["skills"]), "Initial native levels differ")
    require(not plan["loadout"]["blocked"] and "resources" not in initial["account"], "Assisted or blocked initial fixture")
    journal, combat, backend = rows(raw / "driver-journal.jsonl"), rows(work / "combat-receipts.jsonl"), rows(raw / "backend-receipts.jsonl")
    complete = [row for row in journal if row["kind"] == "complete"]
    require(len(complete) == ONE and complete[ZERO]["status"] == "ordinary_queen_full_pass", "Encounter not completed")
    for stage in ("equipped_ground", "enter", "full_ground_to_flying", "equipped_flying", "boss_death", "full_flying_death", "loot", "leave", "rejoin"):
        require(len([row for row in journal if row["kind"] == stage]) == ONE, "Missing or repeated route stage: " + stage)
    phase = next(row for row in journal if row["kind"] == "full_ground_to_flying")
    entered = next(row for row in journal if row["kind"] == "enter")
    actor, generation = phase["fullLife"]["id"], phase["oldGeneration"]
    require(phase["newGeneration"] == generation + ONE, "Transformation must open exactly one new life")
    require(phase["fullLife"]["definition"] == plan["ground"]["npc"]
            and phase["fullLife"]["hitpoints"] == plan["ground"]["maximumLife"], "Unqualified first-life budget")
    impacts = [row for row in combat if row.get("kind") == "landed"
        and row["source"]["id"] == entered["passive"]["pid"]
        and row["target"]["id"] == actor and row["target"]["generation"] == generation
        and row["target"]["definition"] == plan["ground"]["npc"]]
    require(sum(row["actualDamage"] for row in impacts) == plan["ground"]["maximumLife"], "First-life ordinary damage differs")
    require(any(row["target"]["currentLife"] == ZERO for row in impacts), "Fatal first-life impact is absent")
    require(not any(row.get("kind") in ("death", "reward_owner")
        and row.get("target", {}).get("id") == actor
        and row["target"]["generation"] == generation for row in combat), "Interim rewarding death is forbidden")
    require(entered["passive"]["xp"] == phase["passive"]["xp"], "First life incorrectly awarded experience")
    require(any(row.get("kind") == "state" and any(enemy["id"] == actor
        and enemy["definition"] == plan["flying"]["npc"] and enemy["generation"] == generation + ONE
        and enemy["hitpoints"] == plan["flying"]["maximumLife"]
        and enemy["instance"] == entered["passive"]["instance"] for enemy in row.get("enemies", []))
        for row in combat), "Full same-session second life was not observed")
    ordered = ["equipped_ground", "enter", "full_ground_to_flying", "equipped_flying", "boss_death", "full_flying_death", "loot", "leave", "rejoin", "complete"]
    positions = [next(index for index, row in enumerate(journal) if row["kind"] == stage) for stage in ordered]
    require(positions == sorted(positions), "Ordinary Queen stages were reordered")
    flying = next(row for row in journal if row["kind"] == "full_flying_death")
    second = flying["fullLife"]
    require(second["id"] == actor and second["generation"] == generation + ONE
            and second["definition"] == plan["flying"]["npc"]
            and second["hitpoints"] == plan["flying"]["maximumLife"]
            and second["instance"] == entered["passive"]["instance"], "Flying life replaced actor/session or shortened full health")
    second_impacts = [row for row in combat if row.get("kind") == "landed"
        and (row.get("source") or {}).get("id") == entered["passive"]["pid"]
        and row["target"]["id"] == actor and row["target"]["generation"] == generation + ONE
        and row["target"]["definition"] == plan["flying"]["npc"]]
    require(sum(row["actualDamage"] for row in second_impacts) == plan["flying"]["maximumLife"]
            and all(row["style"] == "ranged" for row in second_impacts)
            and any(row["target"]["currentLife"] == ZERO for row in second_impacts), "Full ordinary ranged flying-life damage differs")
    deaths = [row for row in combat if row.get("kind") == "death" and row.get("target", {}).get("id") == actor]
    rewards = [row for row in combat if row.get("kind") == "reward_owner" and row.get("target", {}).get("id") == actor]
    require(len(deaths) == ONE and len(rewards) == ONE
            and deaths[ZERO]["target"]["generation"] == generation + ONE
            and rewards[ZERO]["target"]["generation"] == generation + ONE
            and rewards[ZERO]["target"]["definition"] == plan["flying"]["npc"]
            and (rewards[ZERO].get("owner") or {}).get("id") == entered["passive"]["pid"], "Final ordinary death/reward owner is not unique")
    require(flying["reward"] == rewards[ZERO], "Journal final roll differs from passive ordinary reward")
    require(any(after > before for before, after in zip(phase["passive"]["xp"], next(row for row in journal if row["kind"] == "loot")["passive"]["xp"])), "Final corpse did not grant ordinary combat experience")
    physical_owners = {}
    for name, style, weapon, hands in (("ground", "melee", MELEE_WEAPON_TIER, "two-handed"), ("flying", "ranged", RANGED_WEAPON_TIER, "dual-wield")):
        kit = plan["kits"][name]
        require(not kit["loadout"]["blocked"] and kit["choice"] == {
            "style": style, "tier": weapon, "armourTier": ARMOUR_TIER, "armour": "power", "hands": hands}, "Unadmitted native gear choice")
        equipped = next(row for row in journal if row["kind"] == "equipped_" + name)
        require(equipped["equipment"] == kit["equipment"], "Phase equipment selectors differ")
        for held in kit["equipment"]:
            worn = equipped["passive"]["worn"][held["slot"]]
            require(worn[ZERO] in held["identities"], "Native phase worn identity differs")
            if held["capacity"] is not None:
                physical = worn[PHYSICAL_INSTANCE_FIELD] if len(worn) > PHYSICAL_INSTANCE_FIELD else None
                require(physical and physical.get("key") and ZERO < physical["charges"] <= held["capacity"], "Phase worn physical owner lacks a positive native balance")
                prior = physical_owners.get(held["item"])
                require(prior is None or (physical["key"] == prior["key"] and physical["charges"] <= prior["charges"]), "Switch replaced or refilled an observed physical item")
                physical_owners[held["item"]] = {**physical, "identities": held["identities"]}
    loot = next(row for row in journal if row["kind"] == "loot")
    final_items = loot["passive"]["worn"] + loot["passive"]["backpack"]
    for prior in physical_owners.values():
        owned = [row for row in final_items if row[ZERO] in prior["identities"]]
        require(len(owned) == ONE, "Observed physical equipment was lost or duplicated")
        physical = owned[ZERO][PHYSICAL_INSTANCE_FIELD] if len(owned[ZERO]) > PHYSICAL_INSTANCE_FIELD else None
        require(physical and physical.get("key") == prior["key"] and ZERO < physical["charges"] <= prior["charges"], "Final ordinary gear replaced or refilled its physical owner")
    reward = rewards[ZERO]
    roll = reward.get("loot")
    allocations = [] if roll is None else [roll, *roll.get("allocations", [])]
    eligible = [drop for allocation in allocations if allocation["owner"] == entered["passive"]["pid"] for drop in allocation["drops"]]
    require(loot["reward"] == reward, "Final loot observation differs from ordinary roll")
    if loot["outcome"] == "ordinary_empty_roll":
        require(roll is None and not eligible, "Empty outcome was manufactured from an unresolved foreign-owned roll")
        # Ordinary recovered ammunition is retained in the passive ground rows; a null final roll awards none.
    elif loot["outcome"] == "ordinary_taken_roll":
        stack = loot["ground"]
        require(stack["owner"] == entered["passive"]["pid"]
                and {"item": stack["item"], "count": stack["count"]} in eligible
                and loot["afterCount"] > loot["beforeCount"]
                and not any(row["token"] == stack["token"] for row in loot["passive"]["ground"]), "Final real owner-eligible stack was not transferred")
        require(loot["destination"] == ("coin_wallet" if stack["item"] == plan["currencyItem"] else "backpack"), "Final native destination differs from currency/item owner")
        takes = [row for row in journal if row["kind"] == "control" and row["request"].get("command") == "action"
            and row["response"]["status"] == "accepted" and row["request"]["action"].get("kind") == "object"
            and row["request"]["action"].get("operation") == "Take"
            and all(row["request"]["action"].get(key) == stack[source] for key, source in (("definition", "item"), ("x", "x"), ("z", "z"), ("level", "level"), ("count", "count")))]
        require(len(takes) == ONE, "Final loot lacks one exact accepted native Take")
    else:
        raise ValueError("Unqualified final loot outcome")
    left = next(row for row in journal if row["kind"] == "leave")
    rejoined = next(row for row in journal if row["kind"] == "rejoin")
    require(left["passive"]["instance"] is None
            and all(left["passive"][axis] == plan["room"]["exit"][axis] for axis in ("level", "x", "z")), "Ordinary native exit differs")
    require(rejoined["passive"]["instance"] == entered["passive"]["instance"] == complete[ZERO]["instance"]
            and rejoined["passive"]["room"] == plan["room"]["id"] + "-public", "Native rejoin replaced canonical public membership")
    require(not (work / "passive-observer-errors.json").exists(), "Passive observer failed")
    closure = [row for row in backend if row["kind"] == "peer-finally-closed"]
    require(len(closure) == ONE and closure[ZERO]["closed"] and closure[ZERO]["coreClosed"]
            and not closure[ZERO]["errors"] and not closure[ZERO]["socketPromises"]["pending"], "Graceful close was not acknowledged")
    require(backend[-ONE] == closure[ZERO], "Backend has evidence after terminal close")
    recording = (raw / "session.rtr").read_bytes()
    require(len(recording) <= BYTE_RECORD_LIMIT and recording[:4] == b"RTR1"
            and struct.unpack_from("<I", recording, 4)[ZERO] == RECORD_VERSION, "Recording framing")
    cursor, records = FILE_HEADER_BYTES, []
    while cursor < len(recording):
        require(cursor + RECORD_HEADER_BYTES <= len(recording), "Partial record header")
        cycle, tag, size = struct.unpack_from("<i4sI", recording, cursor)
        cursor += RECORD_HEADER_BYTES
        require(cursor + size <= len(recording), "Partial record body")
        records.append((cycle, tag, recording[cursor:cursor + size]))
        cursor += size
    inputs = [(cycle, list(data)) for cycle, tag, data in records if tag == b"UIEV"]
    applied = [(row["cycle"], row["event"]) for row in backend if row["kind"] == "input" and row["outcome"]["status"] == "applied"]
    accepted = [row for row in journal if row["kind"] == "control" and row["request"]["command"] == "action" and row["response"]["status"] == "accepted"]
    require(inputs == applied and [cycle for cycle, _ in inputs] == [row["response"]["cycle"] for row in accepted], "Actual native inputs differ")
    require(len(inputs) == complete[ZERO]["actions"], "Native action denominator differs")
    clocks = Counter((cycle, tag) for cycle, tag, _ in records if tag in (b"CLKS", b"CLKF", b"CLKD", b"CLKI"))
    require(clocks[FIRST_RECORD_CYCLE, b"CLKS"] == ONE, "Initial monotonic clock missing")
    require({cycle for cycle, _ in inputs} == {cycle for cycle, tag in clocks if tag == b"CLKI"}, "Input clock denominator differs")
    for cycle, tag, data in records:
        if tag == b"NOWM" and cycle > ZERO:
            require(clocks[cycle, b"CLKF"] == ONE and clocks[cycle, b"CLKD"] == ONE, "Logic phase clock differs")
        if tag in (b"CLKS", b"CLKF", b"CLKD", b"CLKI"):
            require(len(data) % CLOCK_SAMPLE_BYTES == ZERO and len(data) // CLOCK_SAMPLE_BYTES <= MAX_CLOCK_SAMPLES, "Clock scope framing differs")
    originals = [raw / "session.rtr", raw / "driver-journal.jsonl", work / "combat-receipts.jsonl", Path(initial["accountPath"])]
    original_hashes = {str(path): sha(path) for path in originals}
    out.mkdir(parents=True)
    publications = root / "tools/client910/fixtures/session-replay/god-wars/publications.py"
    module_spec = importlib.util.spec_from_file_location("ordinary_boss_publications", publications)
    module = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(module)
    _, pids, lineage = module.native_publications(raw / "world.log", out / "server-trace.jsonl")
    require(len(pids) == ONE, "Foreign live account in publications")
    for name, source in (("session.rtr", raw / "session.rtr"), ("driver-journal.jsonl", raw / "driver-journal.jsonl"), ("combat-receipts.jsonl", work / "combat-receipts.jsonl")):
        shutil.copyfile(source, out / name)
        require(sha(source) == sha(out / name), "Whole original fixture copy differs")
    def write(name, value):
        (out / name).write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")
    write("plan.json", {key: value for key, value in plan.items() if key != "accountKey"})
    write("initial-fixture.json", {"fixtureOnly": True, "account": initial["account"], "initialAccountSha256": initial["accountSha256"], "seedSourceSha256": spec["files"]["seed"]["sha256"]})
    write("final-saved-state.json", {key: saved[key] for key in ("skills", "backpack", "worn", "savedVarps", "resources", "instance", "coins") if key in saved})
    write("backend-inputs.json", {"inputs": [{"cycle": cycle, "event": event} for cycle, event in inputs], "closure": closure[ZERO]})
    write("execution.json", {"status": "completed_ordinary_queen_full_route", "rendered": False, "captureOwnerPassed": True, "allOriginalChildrenWaited": True, "sourceEpochSha256": sha(raw / "source-input-epoch.json"), "launchesSpent": result["expectedPriorLaunches"] + ONE, "wholeBytes": len(recording), "wholeRecords": len(records), "wholeSha256": sha(raw / "session.rtr"), "sourceBindings": spec["files"], "publications": lineage, "owningReplayPassed": False})
    require(original_hashes == {str(path): sha(path) for path in originals}, "Original evidence changed")
    print(json.dumps({"passed": True, "owningReplayPassed": False, "fixture": str(out), "wholeSha256": sha(raw / "session.rtr"), "inputs": len(inputs)}))


if __name__ == "__main__":
    main()
