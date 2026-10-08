#!/usr/bin/env python3
"""Freeze a completed ordinary special/repair route; preserve its whole original recording."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import struct
from collections import Counter

SPECIAL_ONE = 1
SPECIAL_ZERO = 0
SPECIAL_INITIAL_LEVEL = 99
SPECIAL_BYTE_RECORD_LIMIT = 256 * 1024 * 1024
SPECIAL_RECORD_HEADER_BYTES = 12
SPECIAL_FILE_HEADER_BYTES = 8
SPECIAL_RECORD_VERSION = 0x10000
SPECIAL_FIRST_RECORD_CYCLE = -1
SPECIAL_CLOCK_SAMPLE_BYTES = 8
SPECIAL_MAX_CLOCK_SAMPLES = 16384


def special_sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def special_require(condition, reason):
    if not condition:
        raise ValueError(reason)


def special_load(path):
    return json.loads(Path(path).read_text())


def special_rows(path):
    raw = Path(path).read_bytes()
    special_require(raw.endswith(b"\n"), "Partial evidence row")
    return [json.loads(line) for line in raw.splitlines()]


def special_main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("spec")
    parser.add_argument("destination")
    args = parser.parse_args()
    spec = special_load(args.spec)
    raw, root, out = Path(spec["out"]), Path(spec["root"]), Path(args.destination)
    special_require(not out.exists(), "Fresh fixture destination required")
    result = special_load(raw / "result.json")
    special_require(result["passed"] and result["allDirectChildrenWaited"] and result["sourceInputsUnchanged"]
            and not result["ownedSurvivors"] and not result["cleanupErrors"], "Capture owner did not qualify")
    processes = special_rows(raw / "processes.jsonl") if (raw / "processes.jsonl").exists() else special_load(raw / "processes.json")
    special_require(all(row.get("waited") and isinstance(row.get("exit"), int) for row in processes), "Unwaited child")
    for name in ("core", "driver", "broker", "wired-guard"):
        found = [row for row in processes if row["name"] == name and not row.get("query")]
        special_require(len(found) == SPECIAL_ONE and found[SPECIAL_ZERO]["exit"] == SPECIAL_ZERO, "Nonzero ordinary process: " + name)
    for helper in spec["files"].values():
        special_require(special_sha(helper["path"]) == helper["sha256"], "Changed frozen helper")
    for relative, expected in special_load(raw / "source-input-epoch.json").items():
        special_require(special_sha(root / relative) == expected, "Changed captured source or generated input: " + relative)
    work = Path(result["work"])
    plan, initial = special_load(work / "plan.json"), special_load(work / "initial-fixture.json")
    saved = special_load(initial["accountPath"])
    special_require(all(skill["level"] == SPECIAL_INITIAL_LEVEL for skill in initial["account"]["skills"]), "Initial native levels differ")
    special_require(not plan["loadout"]["blocked"] and "resources" not in initial["account"], "Assisted or blocked initial fixture")
    journal, combat, backend = special_rows(raw / "driver-journal.jsonl"), special_rows(work / "combat-receipts.jsonl"), special_rows(raw / "backend-receipts.jsonl")
    complete = [row for row in journal if row["kind"] == "complete"]
    special_require(len(complete) == SPECIAL_ONE and complete[SPECIAL_ZERO]["status"] == "ordinary_special_repair_pass", "Special/repair route not completed")
    for stage in ("outside_counts", "durable_repair", "equipped", "special_activated", "ordinary_sample", "field_retired"):
        special_require(len([row for row in journal if row["kind"] == stage]) == SPECIAL_ONE, "Missing or repeated route stage: " + stage)
    repairs = [row for row in combat if row.get("kind") == "repair_commit"]
    special_require(len(repairs) == SPECIAL_ONE, "One actual guarded repair commit required")
    repair = repairs[SPECIAL_ZERO]
    special_require(repair["before"]["coins"] - repair["after"]["coins"] == repair["price"]
        and repair["saved"]["coins"] == repair["after"]["coins"], "Actual durable repair payment differs")
    launches = [row for row in combat if row.get("kind") == "special_launch"]
    installs = [row for row in combat if row.get("kind") == "field_install" and row["accepted"]]
    special_require(len(launches) == SPECIAL_ONE and len(installs) == SPECIAL_ONE, "One special transaction and installation required")
    launch, install = launches[SPECIAL_ZERO], installs[SPECIAL_ZERO]
    special_require(launch["outcome"] == "launched" and launch["before"]["fine"] - launch["after"]["fine"] == plan["special"]["rule"]["costFine"], "Actual ordinary special cost differs")
    special_require(install["primary"] is None and not install["outcomes"], "Short sample cannot claim periodic target damage")
    retired = [row for row in combat if row.get("kind") == "field_retire" and row["key"] == install["key"]]
    special_require(len(retired) == SPECIAL_ONE and not retired[SPECIAL_ZERO]["cancelled"]
        and retired[SPECIAL_ZERO]["tick"] == install["tick"] + install["durationTicks"], "Natural field endpoint differs")
    special_require(not (work / "passive-observer-errors.json").exists(), "Passive observer failed")
    closure = [row for row in backend if row["kind"] == "peer-finally-closed"]
    special_require(len(closure) == SPECIAL_ONE and closure[SPECIAL_ZERO]["closed"] and closure[SPECIAL_ZERO]["coreClosed"]
            and not closure[SPECIAL_ZERO]["errors"] and not closure[SPECIAL_ZERO]["socketPromises"]["pending"], "Graceful close was not acknowledged")
    special_require(backend[-SPECIAL_ONE] == closure[SPECIAL_ZERO], "Backend has evidence after terminal close")
    recording = (raw / "session.rtr").read_bytes()
    special_require(len(recording) <= SPECIAL_BYTE_RECORD_LIMIT and recording[:4] == b"RTR1"
            and struct.unpack_from("<I", recording, 4)[SPECIAL_ZERO] == SPECIAL_RECORD_VERSION, "Recording framing")
    cursor, records = SPECIAL_FILE_HEADER_BYTES, []
    while cursor < len(recording):
        special_require(cursor + SPECIAL_RECORD_HEADER_BYTES <= len(recording), "Partial record header")
        cycle, tag, size = struct.unpack_from("<i4sI", recording, cursor)
        cursor += SPECIAL_RECORD_HEADER_BYTES
        special_require(cursor + size <= len(recording), "Partial record body")
        records.append((cycle, tag, recording[cursor:cursor + size]))
        cursor += size
    inputs = [(cycle, list(data)) for cycle, tag, data in records if tag == b"UIEV"]
    applied = [(row["cycle"], row["event"]) for row in backend if row["kind"] == "input" and row["outcome"]["status"] == "applied"]
    accepted = [row for row in journal if row["kind"] == "control" and row["request"]["command"] == "action" and row["response"]["status"] == "accepted"]
    special_require(inputs == applied and [cycle for cycle, _ in inputs] == [row["response"]["cycle"] for row in accepted], "Actual native inputs differ")
    special_require(len(inputs) == complete[SPECIAL_ZERO]["actions"], "Native action denominator differs")
    clocks = Counter((cycle, tag) for cycle, tag, _ in records if tag in (b"CLKS", b"CLKF", b"CLKD", b"CLKI"))
    special_require(clocks[SPECIAL_FIRST_RECORD_CYCLE, b"CLKS"] == SPECIAL_ONE, "Initial monotonic clock missing")
    special_require({cycle for cycle, _ in inputs} == {cycle for cycle, tag in clocks if tag == b"CLKI"}, "Input clock denominator differs")
    for cycle, tag, data in records:
        if tag == b"NOWM" and cycle > SPECIAL_ZERO:
            special_require(clocks[cycle, b"CLKF"] == SPECIAL_ONE and clocks[cycle, b"CLKD"] == SPECIAL_ONE, "Logic phase clock differs")
        if tag in (b"CLKS", b"CLKF", b"CLKD", b"CLKI"):
            special_require(len(data) % SPECIAL_CLOCK_SAMPLE_BYTES == SPECIAL_ZERO and len(data) // SPECIAL_CLOCK_SAMPLE_BYTES <= SPECIAL_MAX_CLOCK_SAMPLES, "Clock scope framing differs")
    originals = [raw / "session.rtr", raw / "driver-journal.jsonl", work / "combat-receipts.jsonl", Path(initial["accountPath"])]
    original_hashes = {str(path): special_sha(path) for path in originals}
    out.mkdir(parents=True)
    publications = Path(spec["files"]["publications"]["path"])
    module_spec = importlib.util.spec_from_file_location("ordinary_boss_publications", publications)
    module = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(module)
    _, pids, lineage = module.native_publications(raw / "world.log", out / "server-trace.jsonl")
    special_require(len(pids) == SPECIAL_ONE, "Foreign live account in publications")
    for name, source in (("session.rtr", raw / "session.rtr"), ("driver-journal.jsonl", raw / "driver-journal.jsonl"), ("combat-receipts.jsonl", work / "combat-receipts.jsonl")):
        shutil.copyfile(source, out / name)
        special_require(special_sha(source) == special_sha(out / name), "Whole original fixture copy differs")
    def write(name, value):
        (out / name).write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")
    write("plan.json", {key: value for key, value in plan.items() if key != "accountKey"})
    write("initial-fixture.json", {"fixtureOnly": True, "account": initial["account"], "initialAccountSha256": initial["accountSha256"], "seedSourceSha256": spec["files"]["seed"]["sha256"]})
    write("final-saved-state.json", {key: saved[key] for key in ("skills", "backpack", "worn", "coins", "savedVarps", "resources", "instance") if key in saved})
    write("original-failures.json", {"laneLaunches": [json.loads(line) for line in Path(spec["launchLedger"]).read_text().splitlines() if line.strip()], "priorCaptureDirectories": spec.get("priorCaptureDirectories", [])})
    write("backend-inputs.json", {"inputs": [{"cycle": cycle, "event": event} for cycle, event in inputs], "closure": closure[SPECIAL_ZERO]})
    write("execution.json", {"periodicDamageExercised": False, "presentationQualified": False, "status": "completed_ordinary_special_repair", "rendered": False, "captureOwnerPassed": True, "allOriginalChildrenWaited": True, "sourceEpochSha256": special_sha(raw / "source-input-epoch.json"), "launchesSpent": result["expectedPriorLaunches"] + SPECIAL_ONE, "wholeBytes": len(recording), "wholeRecords": len(records), "wholeSha256": special_sha(raw / "session.rtr"), "sourceBindings": spec["files"], "publications": lineage, "owningReplayPassed": False})
    special_require(original_hashes == {str(path): special_sha(path) for path in originals}, "Original evidence changed")
    print(json.dumps({"passed": True, "owningReplayPassed": False, "fixture": str(out), "wholeSha256": special_sha(raw / "session.rtr"), "inputs": len(inputs)}))


if __name__ == "__main__":
    special_main()
