#!/usr/bin/env python3
"""Record one hash-approved ordinary GWD adaptive headless route; no window.

--manifest <fresh faction freeze> --approval <independent Root receipt>
--out <fresh faction result root>. Run four separate invocations serially.
Only the unchanged qualified Owner starts/waits processes. It receives no launch
callback and its foreign native ledger is never accessed.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import signal
import shutil
import stat
import tempfile
import time

import common as contract

REQUIRED_FILES = {"launcher", "freezer", "common", "driver", "sharedDriver", "seed", "server",
    "foodObserver", "peerAdapter", "loginProvider", "design", "owner", "backendBinary", "control",
    "loginTestkit", "socketTestkit", "loginCrypto", "deviceEnvelope", "gwdRules", "foodRules",
    "combatRules", "slayerRules", "chargeRules", "specialRules", "runRules", "broker", "savedProjection", "nativePublications"}
BROKER_CLOSE_SECONDS = 50
STATE_KEYS = ("ready", "player", "terrain_present")


def wait(owner, pairs, deadline, predicate):
    sample_at = time.monotonic()
    while True:
        for name, (child, row) in pairs.items():
            if child.poll() is not None:
                row.update(exit=child.wait(timeout=contract.SOCKET_SECONDS), waited=True)
                owner.save()
                raise RuntimeError("Ordinary owned process ended before completion: " + name)
        if predicate():
            return
        now = time.monotonic()
        if now >= deadline:
            raise TimeoutError("Finite ordinary rehearsal deadline")
        if now >= sample_at:
            owner.sample()
            sample_at = now + contract.SAMPLE_SECONDS
        time.sleep(contract.POLL_SECONDS)


def finish(owner, module, manifest, paths, handlers, ledger_before):
    # Cleanup is independent of artifact/hash failures. No unowned/PID-pattern
    # signal and no source/data mutation occurs in finalization.
    for kind in handlers:
        try:
            signal.signal(kind, signal.SIG_IGN)
        except BaseException as error:
            owner.cleanup_error("Ignore second interruption", error)
    try:
        owner.cleanup()
    except BaseException as error:
        owner.cleanup_error("Qualified owner final cleanup", error)
        for child, row in reversed(owner.children):
            try:
                owner.stop(child, row)
            except BaseException as failure:
                owner.cleanup_error("Fallback exact direct final wait", failure)
    audits = {
        "source": lambda: contract.frozen_map(Path(manifest["runtime"]["root"]), manifest["runtime"]["sourceFiles"]),
        "generated": lambda: require_equal(module.generated(Path(manifest["runtime"]["root"])),
            manifest["runtime"]["generatedInputs"], "Generated epoch changed"),
        "pack": lambda: require_equal(contract.files_under(Path(manifest["runtime"]["root"]) / "server/data/pack"),
            manifest["runtime"]["packFiles"], "Pack epoch changed"),
        "symbols": lambda: require_equal(contract.files_under(Path(manifest["symbolsDirectory"]), ".sym"),
            manifest["symbolFiles"], "Symbol epoch changed"),
        "privateSources": lambda: [contract.checked(entry) for entry in manifest["files"].values()],
        "ledger": lambda: require_equal(Path(manifest["nativeLedger"]["path"]).read_bytes(),
            ledger_before, "GWD native ledger changed; no headless budget write allowed"),
        "executables": lambda: [contract.checked(entry) for entry in manifest["executables"].values()],
    }
    owner.result["postRunAudits"] = {}
    for name, audit in audits.items():
        try:
            audit()
            owner.result["postRunAudits"][name] = True
        except BaseException as error:
            owner.cleanup_error("Post-run " + name, error)
            owner.result["postRunAudits"][name] = False
    owner.result["allDirectChildrenWaited"] = all(row.get("waited") for row in owner.rows)
    if owner.result["allDirectChildrenWaited"] and owner.result.get("ownedSurvivors") == []:
        try:
            recorded = owner.output / "session.rtr"
            if not recorded.is_file() or recorded.stat().st_size <= contract.FIRST:
                raise RuntimeError("Actual observed-core RTR is absent or empty")
            owner.result["recordedSession"] = {**contract.binding(recorded),
                "bytes": recorded.stat().st_size, "boundAfterFinalWaits": True,
                "owningReplayPassed": False, "rendered": False}
        except BaseException as error:
            owner.cleanup_error("Actual recorded session evidence", error)
    owner.result["cleanupErrors"] = owner.cleanup_errors
    if owner.cleanup_errors or not owner.result["allDirectChildrenWaited"] \
            or owner.result.get("ownedSurvivors") != []:
        owner.result["passed"] = False
    for kind, handler in handlers.items():
        try:
            signal.signal(kind, handler)
        except BaseException as error:
            owner.cleanup_error("Signal handler restore", error)
    owner.best_effort_save("Final GWD result")


def require_equal(actual, expected, reason):
    if actual != expected:
        raise RuntimeError(reason)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("manifest", "approval", "out"):
        parser.add_argument("--" + flag, type=Path, required=True)
    args = parser.parse_args()
    manifest = contract.read_json(args.manifest)
    if manifest.get("format") != contract.FORMAT or manifest.get("faction") not in contract.FACTIONS \
            or manifest.get("account") != contract.ACCOUNT \
            or manifest.get("kind") != "ordinary-gwd-one-faction-headless-freeze":
        parser.error("Exact one-faction GWD manifest required")
    output = args.out.resolve()
    if not args.out.is_absolute() or output.exists() or output == contract.REPOSITORY_ROOT \
            or contract.REPOSITORY_ROOT in output.parents:
        parser.error("Fresh absolute per-arena result root outside the checkout required")
    output.mkdir(parents=True)
    module = contract.load_owner(manifest["files"]["owner"])
    root = Path(manifest["runtime"]["root"])
    owner = module.Owner(output, root)
    owner.result.update(actualClientLaunchesThisRun=contract.FIRST, nativeWindows=contract.FIRST,
        expectedPriorLaunches=contract.EXPECTED_NATIVE, maximumClientLaunches=contract.MAX_NATIVE,
        captureQualification="ordinary headless route only; native proof still pending", rendered=False,
        faction=manifest["faction"], manifest=contract.binding(args.manifest), approval=contract.binding(args.approval))
    handlers = {kind: signal.getsignal(kind) for kind in (signal.SIGTERM, signal.SIGINT)}
    for kind in handlers:
        signal.signal(kind, module.interrupt)
    paths, ledger_before, pairs = {}, None, {}
    try:
        if not root.is_absolute() or root.resolve() != contract.REPOSITORY_ROOT.resolve():
            raise RuntimeError("Manifest must bind this recorder checkout")
        if set(manifest["files"]) != REQUIRED_FILES:
            raise RuntimeError("Exact source dependency inventory differs")
        paths = {name: contract.checked(entry) for name, entry in manifest["files"].items()}
        if paths["launcher"] != Path(__file__).absolute() or paths["common"] != Path(contract.__file__).absolute() \
                or paths["owner"] != contract.OWNER_PATH:
            raise RuntimeError("Executing helper/common/qualified Owner differs from freeze")
        if paths["driver"].parent.parent / "barrows" / "driver.py" != paths["sharedDriver"] \
                or paths["server"].parent.parent / "barrows" / "food-receipts.mjs" != paths["foodObserver"] \
                or paths["driver"].with_name("saved_projection.py") != paths["savedProjection"]:
            raise RuntimeError("Frozen sibling import ownership differs")
        require_equal(manifest["limits"], {"driverSeconds": contract.DRIVER_SECONDS,
            "sessionSeconds": contract.SESSION_SECONDS, "startSeconds": contract.START_SECONDS},
            "Exact finite driver/session/start bounds required")
        approval = contract.read_json(args.approval)
        expected_approval = {"manifest": contract.binding(args.manifest), "head": manifest["runtime"]["head"],
            "tree": manifest["runtime"]["tree"], "base": manifest["runtime"]["base"],
            "backendSha256": manifest["files"]["backendBinary"]["sha256"],
            "faction": manifest["faction"], "ports": manifest["ports"], "mode": "ordinary-headless"}
        if approval.get("passed") is not True or any(approval.get(key) != value for key, value in expected_approval.items()):
            raise RuntimeError("Independent Root approval does not bind this exact source/faction/freeze")
        require_equal(manifest["backend"], {"entry": contract.ENTRY, "rendered": False,
            "executionMode": contract.HEADLESS_EXECUTION_MODE,
            "recordingBackend": contract.OBSERVED_BACKEND,
            "monotonicClock": contract.MONOTONIC_CLOCK_FORMAT},
            "Actual qualified recorded authenticated-headless backend is required")
        contract.prerequisite(manifest["prerequisite"], manifest["runtime"], manifest["files"]["backendBinary"])
        contract.checked(manifest["actualLandingReceipt"])
        contract.checked(manifest["sourceEpoch"])
        ledger_before = contract.ledger_bytes(manifest["nativeLedger"])
        contract.frozen_map(root, manifest["runtime"]["sourceFiles"])
        require_equal(module.generated(root), manifest["runtime"]["generatedInputs"], "Complete generated epoch differs")
        require_equal(contract.files_under(root / "server/data/pack"), manifest["runtime"]["packFiles"], "Pack epoch differs")
        require_equal(contract.files_under(Path(manifest["symbolsDirectory"]), ".sym"), manifest["symbolFiles"], "Symbols differ")
        executables = {name: str(contract.checked(entry)) for name, entry in manifest["executables"].items()}
        require_equal(set(executables), {"node", "python3"}, "Exact runtime executable inventory differs")
        ports = manifest["ports"]
        if len(set(ports.values())) != len(ports) or set(ports) != {"lobby", "world"} \
                or any(not isinstance(port, int) or not contract.FIRST_PORT <= port < contract.END_PORT for port in ports.values()):
            raise RuntimeError("Dedicated GWD port pair differs")
        environment = dict(os.environ)
        for name in list(environment):
            if name.startswith("CLIENT910_") or name in contract.FORBIDDEN_ENV:
                environment.pop(name, None)
        environment.update(ALTO_GWD_RUNTIME_ROOT=str(root), ALTO_GENERATED_DIR=str(root / "server/data/generated"),
            ALTO_DEV_NPCS="1", ALTO_LOGIN_CRYPTO="on", ALTO_TRACE_INFO="1", NODE_OPTIONS="")
        for name, argv, expected in (
            ("head", ["git", "rev-parse", "HEAD"], manifest["runtime"]["head"]),
            ("tree", ["git", "show", "--format=%T", "--no-patch", "HEAD"], manifest["runtime"]["tree"]),
            ("base", ["git", "merge-base", "HEAD", manifest["runtime"]["base"]], manifest["runtime"]["base"]),
            ("clean", ["git", "status", "--porcelain"], ""),
        ):
            require_equal(owner.run(name, argv, root, environment, contract.METADATA_SECONDS).strip(), expected,
                "Current source checkpoint differs: " + name)
        deadline = time.monotonic() + contract.SESSION_SECONDS
        scratch = Path(tempfile.mkdtemp(prefix="gw-")).resolve()
        work = scratch / "w"
        work.mkdir()
        transport, control_socket = scratch / "p.sock", scratch / "c.sock"
        if any(len(os.fsencode(path)) >= contract.MAX_SOCKET_BYTES for path in (transport, control_socket)):
            raise RuntimeError("Native Unix socket path bound exceeded")
        environment.update(ALTO_LOBBY_PORT=str(ports["lobby"]), ALTO_WORLD_PORT=str(ports["world"]),
            ALTO_PLAYER_DATA_DIR=str(work / "players"))
        owner.result.update(scratch=str(scratch), work=str(work), backendEntry=contract.ENTRY,
            head=manifest["runtime"]["head"], tree=manifest["runtime"]["tree"], base=manifest["runtime"]["base"],
            limitations=["No rendered/foreground/pixel/FPS proof", "Surface quest re-entry remains held",
                "Normal RNG/full HP; camp/arena rejoin earns all count through live NPC deaths",
                "No native budget spending or guard invocation"])
        owner.run("seed", [executables["node"], str(paths["seed"]), str(work), manifest["faction"]],
            root / "server", environment, contract.METADATA_SECONDS)
        plan = contract.read_json(work / "plan.json")
        fixture = contract.read_json(work / "initial-fixture.json")
        if plan.get("accountKey") != contract.ACCOUNT or plan.get("faction") != manifest["faction"] \
                or plan.get("arenaId") != manifest["expected"]["arenaId"] \
                or plan.get("initialCount") != manifest["expected"]["initialCount"] \
                or "resources" in fixture["account"] or "preflight" in plan \
                or fixture["source"]["seedSha256"] != manifest["files"]["seed"]["sha256"]:
            raise RuntimeError("Fresh normal pre-login account/plan/source differs")
        for relative, expected in fixture["source"]["inputs"].items():
            require_equal(contract.sha(root / "server/data/generated" / relative), expected,
                "Declared seed generated input differs")
        account_file = Path(fixture["accountPath"])
        if work not in account_file.parents or contract.sha(account_file) != fixture["accountSha256"]:
            raise RuntimeError("Fresh account path/hash is outside own scratch or differs")
        owner.result["initialFixture"] = {name: contract.binding(work / name)
            for name in ("plan.json", "initial-fixture.json")}
        owner.result["initialAccount"] = contract.binding(account_file)
        pairs["lobby"] = owner.start("lobby", [executables["node"], "src/lostcity/lobby.ts"], root / "server", environment, contract.SESSION_SECONDS)
        pairs["world"] = owner.start("world", [executables["node"], str(paths["server"]), str(work)], root / "server", environment, contract.SESSION_SECONDS)
        wait(owner, pairs, min(deadline, time.monotonic() + contract.START_SECONDS), lambda: all(
            f"listening on port {ports[name]}" in module.bounded_text(output / (name + ".log")).lower()
            for name in ("lobby", "world")))
        receipts = output / "backend-receipts.jsonl"
        peer = {"peerCount": contract.ONE, "headlessOnly": True, "broker": manifest["files"]["broker"],
            "loginTestkit": manifest["files"]["loginTestkit"], "deviceEnvelope": manifest["files"]["deviceEnvelope"],
            "receipts": str(receipts), "peer": {"name": contract.LOGIN_NAME, "port": ports["world"],
                "transport": str(transport), "control": str(control_socket), "deviceEnvelope": str(paths["deviceEnvelope"]),
                "modules": {name: manifest["files"][name] for name in ("socketTestkit", "loginProvider", "loginCrypto")}}}
        peer_file = output / "one-peer.json"
        contract.write_json(peer_file, peer)
        pairs["broker"] = owner.start("broker", [executables["node"], str(paths["peerAdapter"]), str(peer_file)], root / "server", environment, contract.SESSION_SECONDS)
        wait(owner, pairs, min(deadline, time.monotonic() + contract.START_SECONDS), lambda:
            transport.exists() and stat.S_ISSOCK(transport.stat().st_mode))
        core_env = dict(environment, CLIENT910_OBSERVED_TRANSPORT=str(transport), CLIENT910_CONTROL=str(control_socket),
            CLIENT910_OBSERVED_SESSION_SECONDS=str(contract.SESSION_SECONDS), CLIENT910_RECORD=str(output / "session.rtr"))
        pairs["core"] = owner.start("core", [str(paths["backendBinary"]), "--ignored", "--exact", contract.ENTRY,
            "--nocapture", "--test-threads=1"], root / "tools", core_env, contract.SESSION_SECONDS)
        backend = contract.BackendRows(receipts)
        def booted():
            backend.poll()
            return backend.boot is not None and control_socket.exists() and stat.S_ISSOCK(control_socket.stat().st_mode)
        wait(owner, pairs, min(deadline, time.monotonic() + contract.START_SECONDS), booted)
        observation = module.explicit_control(paths["control"], control_socket, contract.SOCKET_SECONDS)
        try:
            def ready():
                backend.poll()
                response = observation.request({"command": "snapshot"}, timeout=contract.SOCKET_SECONDS)
                if response.get("status") != "observed":
                    raise RuntimeError("Normal readiness observer refused")
                state = response.get("data", {})
                owner.result["startupObservation"] = state
                return all(state.get(key) for key in STATE_KEYS)
            wait(owner, pairs, min(deadline, time.monotonic() + contract.START_SECONDS), ready)
        finally:
            observation.close()
        if deadline - time.monotonic() < contract.DRIVER_SECONDS:
            raise TimeoutError("Startup consumed the fixed driver interval; no silent extension")
        journal = output / "driver-journal.jsonl"
        argv = [executables["python3"], str(paths["driver"]), "--rules", str(paths["gwdRules"]),
            "--plan", str(work / "plan.json"), "--food-rules", str(paths["foodRules"]),
            "--slayer-rules", str(paths["slayerRules"]),
            "--special-rules", str(paths["specialRules"]),
            "--run-rules", str(paths["runRules"]),
            "--symbols", manifest["symbolsDirectory"], "--receipts", str(work / "combat-receipts.jsonl"),
            "--journal", str(journal), "--control-module", str(paths["control"]),
            "--control-sha", manifest["files"]["control"]["sha256"], "--socket", str(control_socket),
            "--deadline-seconds", str(contract.DRIVER_SECONDS), "--fight-seconds", str(contract.FIGHT_SECONDS),
            "--action-seconds", str(contract.ACTION_SECONDS), "--socket-seconds", str(contract.SOCKET_SECONDS)]
        driver, driver_row = owner.start("driver", argv, root, environment, contract.DRIVER_SECONDS)
        driver_deadline = min(deadline, time.monotonic() + contract.DRIVER_SECONDS)
        def driver_finished():
            backend.poll()
            return driver.poll() is not None
        wait(owner, pairs, driver_deadline, driver_finished)
        driver_row.update(exit=driver.wait(timeout=contract.SOCKET_SECONDS), waited=True)
        if driver_row["exit"]:
            raise RuntimeError("Ordinary GWD driver failed; preserve exact journal/receipts")
        semantic = contract.semantic_journal(journal, contract.read_json(paths["gwdRules"]), plan)
        actions = [row["response"]["cycle"] for row in contract.journal_rows(journal)
            if row.get("kind") == "control" and row.get("request", {}).get("command") == "action"
            and row.get("response", {}).get("status") == "accepted"]
        def accepted_inputs_observed():
            backend.poll()
            return len(backend.input_cycles) >= len(actions)
        wait(owner, pairs, min(deadline, time.monotonic() + contract.SOCKET_SECONDS), accepted_inputs_observed)
        if backend.input_cycles != actions:
            raise RuntimeError("Controller accepted actions and actual applied-input cycle receipts differ")
        contract.write_json(output / "semantic-result.json", semantic)
        # Normal transport abort belongs to this owner after completed gameplay,
        # never to a second input/action producer. Each process is finally waited.
        owner.stop_direct(*pairs.pop("broker"), BROKER_CLOSE_SECONDS)
        owner.stop(*pairs.pop("core"))
        owner.stop(*pairs.pop("world"))
        owner.stop(*pairs.pop("lobby"))
        projection_spec = importlib.util.spec_from_file_location("gwd_closed_saved_projection", paths["savedProjection"])
        projection = importlib.util.module_from_spec(projection_spec)
        projection_spec.loader.exec_module(projection)
        processed = output / "processed"
        processed.mkdir()
        # Keep the complete actual RTR and every original publication. No
        # inferred applied-prefix filter or synthesized clock/input is used.
        recorded = output / "session.rtr"
        if not recorded.is_file() or recorded.stat().st_size <= contract.FIRST:
            raise RuntimeError("The actual ordinary recording is absent/empty")
        shutil.copyfile(recorded, processed / "session.rtr")
        require_equal(contract.sha(recorded), contract.sha(processed / "session.rtr"),
            "Whole original RTR copy changed")
        publication_spec = importlib.util.spec_from_file_location("gwd_ordered_native_publications", paths["nativePublications"])
        publication = importlib.util.module_from_spec(publication_spec)
        publication_spec.loader.exec_module(publication)
        _, pids, publication_lineage = publication.native_publications(output / "world.log", processed / "server-trace.jsonl")
        require_equal(pids, {semantic["final"]["passive"]["pid"]},
            "Original PLAYER/NPC publications do not belong to the actual player")
        for original, name in ((work / "plan.json", "plan.json"),
                (work / "combat-receipts.jsonl", "combat-receipts.jsonl"),
                (journal, "driver-journal.jsonl")):
            shutil.copyfile(original, processed / name)
        execution = {"status": semantic["status"], "executionMode": contract.HEADLESS_EXECUTION_MODE,
            "recordingBackend": contract.OBSERVED_BACKEND,
            "monotonicClock": contract.MONOTONIC_CLOCK_FORMAT, "rendered": False,
            "faction": semantic["faction"], "roleDefinitions": semantic["roleDefinitions"]}
        contract.write_json(processed / "execution.json", execution)
        owner.result.update(recordedFixture=str(processed), execution=execution,
            publicationLineage=publication_lineage, owningReplayPassed=False)
        owner.result["closedSavedState"] = projection.project_closed_recording(work, output, processed,
            owner.rows, contract.read_json(paths["gwdRules"]))
        backend.poll()
        if (work / "passive-observer-errors.json").exists():
            raise RuntimeError("Passive ordinary GWD observer failed")
        world_log = module.bounded_text(output / "world.log")
        if "[WORLD]: tick step" in world_log or "step failed" in world_log or "GWD_OBSERVER_ERROR" in world_log:
            raise RuntimeError("Actual World step/passive observer failure")
        passive_rows = contract.journal_rows(work / "combat-receipts.jsonl",
            maximum_bytes=contract.MAX_PASSIVE_RECEIPT_BYTES)
        if not any(row.get("kind") == "landed" for row in passive_rows):
            raise RuntimeError("Accepted ordinary landed-hit receipts absent")
        owner.result.update(passed=True, semantic=contract.binding(output / "semantic-result.json"),
            backend=backend.summary(), evidence={name: contract.binding(path) for name, path in {
                "backend": receipts, "journal": journal, "passive": work / "combat-receipts.jsonl",
                "plan": work / "plan.json", "fixture": work / "initial-fixture.json"}.items()})
    except BaseException as error:
        owner.result.update(passed=False, error=str(error))
    finally:
        finish(owner, module, manifest, paths, handlers, ledger_before)
        print(json.dumps({key: owner.result.get(key) for key in ("passed", "faction", "error",
            "allDirectChildrenWaited", "ownedSurvivors", "nativeWindows")}, sort_keys=True), flush=True)
    return contract.FIRST if owner.result["passed"] else contract.ONE


if __name__ == "__main__":
    raise SystemExit(main())
