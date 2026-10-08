#!/usr/bin/env python3
"""Freeze a completed ordinary encounter; preserve its whole original recording."""
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
FULL_BOSS_LIFE = 17250
GEAR_TIER = 90
PROOF_SCOPE = "full-life-loot-retreat"
MAIN_HAND_SLOT = 3
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
    require(len(complete) == ONE and complete[ZERO]["status"] == "ordinary_chaos_public_full_life_loot_retreat_pass", "Encounter not completed")
    for stage in ("enter", "boss_death", "loot", "leave"):
        require(len([row for row in journal if row["kind"] == stage]) == ONE, "Missing or repeated route stage: " + stage)
    death = next(row for row in journal if row["kind"] == "boss_death")
    full = death["fullLife"]
    require(full["hitpoints"] == FULL_BOSS_LIFE and full["definition"] == plan["target"]["npc"]
            and full["instance"] is None, "Full public Chaos life was not admitted")
    require(sum(row["actualDamage"] for row in death["landed"]) == FULL_BOSS_LIFE
            and all(row["style"] == "melee" for row in death["landed"]), "Full native melee damage differs")
    require(death["reward"]["owner"]["id"] == next(row for row in journal if row["kind"] == "enter")["passive"]["pid"], "Real reward owner absent")
    selected = [row for row in combat if row["kind"] == "loot_table_roll"
                and row["tick"] == death["reward"]["tick"] and row["ownerId"] == death["reward"]["owner"]["id"]]
    require(len(selected) == ONE and type(selected[ZERO]["ownerMembers"]) is bool,
            "Exactly one actual membership-owned ordinary table roll is required")
    branch = "member" if selected[ZERO]["ownerMembers"] else "free"
    expected_table = f"wilderness:chaos-elemental:{plan['target']['npc']}:{branch}"
    require(selected[ZERO]["table"] == expected_table and selected[ZERO]["mainRolls"] == ONE,
            "Wrong source branch or unqualified main-roll multiplicity")
    require(selected[ZERO]["ownerGeneration"] == death["reward"]["owner"]["generation"]
            and selected[ZERO]["ownerMembers"] == death["reward"]["ownerMembers"],
            "Loot branch and ordinary reward owner are not the same observed actor")
    require(selected[ZERO]["rolled"] == death["reward"]["loot"],
            "Actual ordinary roll differs from the owner allocation; qualify any admission difference explicitly")
    specials = [row for row in journal if row["kind"] == "natural_special"]
    observed_effects = {row["effect"] for row in specials}
    require(observed_effects <= {"equipment-removal", "displacement"} and len(specials) == len(observed_effects), "Natural effects must be distinct qualified observations")
    require(complete[ZERO]["scope"] == PROOF_SCOPE and set(complete[ZERO]["naturalEffectsObserved"]) == observed_effects
            and set(complete[ZERO]["naturalEffectsUnobserved"]) == {"equipment-removal", "displacement"} - observed_effects, "Explicit full-life scope and unobserved natural branches required")
    for row in specials:
        delivery = row["delivery"]
        require(delivery in combat and delivery["source"]["id"] == full["id"]
                and delivery["source"]["generation"] == full["generation"]
                and delivery["launch"] in combat and delivery["tick"] >= delivery["launch"]["due"], "Unbound special delivery")
        require(row["passive"]["instance"] is None and row["state"].get("region") is None, "Public native special moved into an instance")
        before, after = delivery["before"], delivery["after"]
        require(before["generation"] == after["generation"] and before["instance"] is None and after["instance"] is None, "Special crossed player life or session")
        if row["effect"] == "displacement":
            require(after["relocation"] > before["relocation"] and (before["x"], before["z"]) != (after["x"], after["z"]) and before["level"] == after["level"], "No ordinary Confusion relocation")
        else:
            held = next(item for item in plan["kits"]["melee"]["equipment"] if item["slot"] == MAIN_HAND_SLOT)
            require(before["worn"][held["slot"]][ZERO] >= ZERO and after["worn"][held["slot"]][ZERO] < ZERO, "No ordinary Madness main-hand removal")
        require(any(control["kind"] == "control" and control["request"]["command"] == "snapshot" and control["response"]["cycle"] == row["state"]["cycle"] and all(control["response"]["data"][key] == row["state"][key] for key in ("player", "inventories", "varbits")) for control in journal), "Special lacks its actual native snapshot")
    require(all(next(row for row in journal if row["kind"] == stage)["passive"]["instance"] is None
                for stage in ("enter", "leave")), "Public walking was replaced by an instance")
    equipped = [row for row in journal if row["kind"] == "equipped_melee"]
    require(len(equipped) == ONE and not plan["loadout"]["blocked"]
            and plan["source"]["choice"] == {"style": "melee", "tier": GEAR_TIER, "armourTier": GEAR_TIER, "armour": "power", "hands": "dual-wield"}, "Unadmitted ordinary kit")
    require(any(slot[ZERO] < ZERO for slot in equipped[ZERO]["passive"]["backpack"]), "Wield did not create a native vacancy")
    loot = next(row for row in journal if row["kind"] == "loot")
    roll = death["reward"].get("loot")
    require(loot["reward"] == death["reward"], "Invented reward roll")
    if loot["outcome"] == "ordinary_empty_roll":
        require(roll is None, "Nonempty normal roll cannot qualify as empty")
    else:
        allocations = [roll, *roll.get("allocations", [])] if roll else []
        require(any(allocation["owner"] == death["reward"]["owner"]["id"] and any(drop["item"] == loot["item"] and drop["count"] == loot["ground"]["count"] for drop in allocation["drops"]) for allocation in allocations), "Take does not match real owned roll")
        require(loot["afterCount"] > loot["beforeCount"] and not any(item["token"] == loot["ground"]["token"] for item in loot["passive"]["ground"]), "Native credit or ground removal is absent")
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
    write("final-saved-state.json", {key: saved[key] for key in ("skills", "backpack", "worn", "savedVarps", "resources", "instance") if key in saved})
    write("backend-inputs.json", {"inputs": [{"cycle": cycle, "event": event} for cycle, event in inputs], "closure": closure[ZERO]})
    write("execution.json", {"status": "completed_ordinary_chaos_public_full_life_loot_retreat", "scope": PROOF_SCOPE, "naturalEffectsObserved": sorted(observed_effects), "naturalEffectsUnobserved": sorted({"equipment-removal", "displacement"} - observed_effects), "rendered": False, "captureOwnerPassed": True, "allOriginalChildrenWaited": True, "sourceEpochSha256": sha(raw / "source-input-epoch.json"), "launchesSpent": result["expectedPriorLaunches"] + ONE, "wholeBytes": len(recording), "wholeRecords": len(records), "wholeSha256": sha(raw / "session.rtr"), "sourceBindings": spec["files"], "publications": lineage, "owningReplayPassed": False})
    require(original_hashes == {str(path): sha(path) for path in originals}, "Original evidence changed")
    print(json.dumps({"passed": True, "owningReplayPassed": False, "fixture": str(out), "wholeSha256": sha(raw / "session.rtr"), "inputs": len(inputs)}))


if __name__ == "__main__":
    main()
