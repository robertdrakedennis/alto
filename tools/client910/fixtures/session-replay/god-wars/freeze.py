#!/usr/bin/env python3
"""Freeze a reviewed, clean GWD adaptive authenticated-headless epoch.

Run only under Root's metadata preparation lease, after combined controls land,
GWD rebases and a current-tree owning backend build and qualification exist.
Creates no account/server/core, builds nothing and does not issue approval.
"""
import argparse
import os
from pathlib import Path
import shutil
import signal

import common as contract

RUNTIME_RELATIVE = {
    "control": "tools/client910/scripts/control.py",
    "loginTestkit": "server/src/lostcity/network/SkillsE2E.testkit.ts",
    "socketTestkit": "server/src/lostcity/network/SocketE2E.testkit.ts",
    "loginCrypto": "server/src/lostcity/network/LoginCrypto.ts",
    "deviceEnvelope": "tools/client910/fixtures/session-replay/legacy-interface/session.rtr",
    "gwdRules": "server/data/generated/instances/gwd.json",
    "foodRules": "server/data/generated/food/foods.json",
    "slayerRules": "server/data/generated/slayer/rules.json",
    "combatRules": "server/data/generated/combat/rules.json",
    "chargeRules": "server/data/generated/combat/charges.json",
    "specialRules": "server/data/generated/combat/specials.json",
    "runRules": "server/data/generated/combat/run-energy.json",
}
HELPER_RELATIVE = {
    "launcher": "record.py", "freezer": "freeze.py",
    "common": "common.py", "driver": "driver.py",
    "savedProjection": "saved_projection.py", "seed": "seed.mjs",
    "server": "server.mjs", "peerAdapter": "peer_adapter.mjs",
    "loginProvider": "login_provider.mjs", "broker": "peer_broker.mjs",
    "nativePublications": "publications.py",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("root", "backend", "prerequisite", "landing-receipt", "native-ledger", "out"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--base", required=True, help="Exact actually landed Main SHA")
    parser.add_argument("--owner-sha256", required=True, help="Reviewed committed Barrows Owner source SHA256")
    parser.add_argument("--lobby-port", type=int, default=contract.FIRST_PORT)
    parser.add_argument("--world-port", type=int, default=contract.FIRST_PORT + contract.ONE)
    args = parser.parse_args()
    root, output = args.root.resolve(), args.out.resolve()
    if not args.out.is_absolute() or output.exists() or output == root or root in output.parents:
        parser.error("A fresh absolute GWD freeze root outside the checkout is required")
    if root != contract.REPOSITORY_ROOT.resolve():
        parser.error("This freezer must bind its own checkout")
    ports = {"lobby": args.lobby_port, "world": args.world_port}
    if len(set(ports.values())) != len(ports) or any(
            not contract.FIRST_PORT <= port < contract.END_PORT for port in ports.values()):
        parser.error("Two distinct GWD ports in the assigned range are required")
    output.mkdir(parents=True)
    owner_source = {"path": str(contract.OWNER_PATH), "sha256": args.owner_sha256}
    module = contract.load_owner(owner_source)
    owner = module.Owner(output, root)
    owner.result.update(nativeWindows=contract.FIRST, actualClientLaunchesThisRun=contract.FIRST,
        expectedPriorLaunches=contract.EXPECTED_NATIVE, maximumClientLaunches=contract.MAX_NATIVE,
        captureQualification="metadata freezer only; no headless or native dispatch")
    handlers = {kind: signal.getsignal(kind) for kind in (signal.SIGTERM, signal.SIGINT)}
    for kind in handlers:
        signal.signal(kind, module.interrupt)
    ledger = None
    try:
        environment = dict(os.environ)
        head = owner.run("head", ["git", "rev-parse", "HEAD"], root, environment, contract.METADATA_SECONDS).strip()
        tree = owner.run("tree", ["git", "show", "--format=%T", "--no-patch", "HEAD"], root, environment, contract.METADATA_SECONDS).strip()
        base = owner.run("base", ["git", "merge-base", "HEAD", args.base], root, environment, contract.METADATA_SECONDS).strip()
        if base != args.base or owner.run("clean", ["git", "status", "--porcelain"],
                root, environment, contract.METADATA_SECONDS).strip():
            raise RuntimeError("A clean GWD candidate directly after actual landed dependency is required")
        # This is an exact committed-tree inventory, not filename discovery.
        names = owner.run("tracked-tree", ["git", "ls-tree", "-r", "-z", "--name-only", "HEAD"],
            root, environment, contract.METADATA_SECONDS).split("\0")
        source_files = {name: contract.sha(root / name) for name in names if name}
        if not source_files:
            raise RuntimeError("Committed source inventory is empty")
        sources = {name: root / relative for name, relative in RUNTIME_RELATIVE.items()}
        sources.update({name: contract.HERE / relative for name, relative in HELPER_RELATIVE.items()})
        sources.update({
            "owner": contract.OWNER_PATH,
            "sharedDriver": contract.HERE.parent / "barrows" / "driver.py",
            "foodObserver": contract.HERE.parent / "barrows" / "food-receipts.mjs",
            "design": root / "docs/server-architecture.md",
            "backendBinary": args.backend.resolve(),
        })
        if contract.binding(sources["owner"]) != owner_source:
            raise RuntimeError("Reviewed committed Owner source changed")
        files = {name: contract.binding(path) for name, path in sources.items()}
        generated = module.generated(root)
        pack_root = root / "server/data/pack"
        symbols = root / "revisions/910/symbols"
        pack_files = contract.files_under(pack_root)
        symbol_files = contract.files_under(symbols, ".sym")
        if not generated or not pack_files or not symbol_files:
            raise RuntimeError("Actual generated/pack/symbol inventory is incomplete")
        runtime = {"root": str(root), "head": head, "tree": tree, "base": base,
            "sourceFiles": source_files, "generatedInputs": generated, "packFiles": pack_files}
        prereq = contract.binding(args.prerequisite)
        contract.prerequisite(prereq, runtime, files["backendBinary"])
        landing = contract.binding(args.landing_receipt)
        # Original landing receipt is hash-bound for independent Root review;
        # its format is not replaced by a fabricated lane-specific PASS schema.
        contract.read_json(args.landing_receipt)
        ledger_binding = contract.binding(args.native_ledger)
        ledger = contract.ledger_bytes(ledger_binding)
        source_epoch = {"runtime": runtime, "files": files, "symbols": symbol_files,
            "prerequisite": prereq, "actualLandingReceipt": landing,
            "nativeLedger": ledger_binding, "ports": ports}
        epoch = output / "source-epoch.json"
        contract.write_json(epoch, source_epoch)
        executable_paths = {name: shutil.which(name) for name in ("node", "python3")}
        if any(value is None for value in executable_paths.values()):
            raise RuntimeError("Actual absolute Node/Python runtime paths required")
        executables = {name: contract.binding(path) for name, path in executable_paths.items()}
        rules = contract.read_json(sources["gwdRules"])
        if {row["faction"] for row in rules["arenas"]} != set(contract.FACTIONS):
            raise RuntimeError("Four generated normal factions must be exact")
        manifests = []
        for faction in contract.FACTIONS:
            arena = next(row for row in rules["arenas"] if row["faction"] == faction)
            manifest = {
                "format": contract.FORMAT, "kind": "ordinary-gwd-one-faction-headless-freeze",
                "faction": faction, "account": contract.ACCOUNT,
                "currentProposalSource": {"head": head, "tree": tree, "base": base}, "runtime": runtime,
                "sourceEpoch": contract.binding(epoch), "files": files, "executables": executables,
                "symbolsDirectory": str(symbols), "symbolFiles": symbol_files,
                "prerequisite": prereq, "actualLandingReceipt": landing,
                "nativeLedger": ledger_binding, "ports": ports,
                "limits": {"driverSeconds": contract.DRIVER_SECONDS, "sessionSeconds": contract.SESSION_SECONDS,
                    "startSeconds": contract.START_SECONDS},
                "backend": {"entry": contract.ENTRY, "rendered": False,
                    "executionMode": contract.HEADLESS_EXECUTION_MODE,
                    "recordingBackend": contract.OBSERVED_BACKEND,
                    "monotonicClock": contract.MONOTONIC_CLOCK_FORMAT},
                "expected": {"arenaId": arena["id"], "roles": [
                    {"type": role["type"], "title": role["title"], "hitpoints": role["profile"]["hitpoints"]}
                    for role in arena["roles"]], "initialCount": rules["entry"]["lobbyMinimum"]["value"] - contract.ONE,
                    "arenaDebit": rules["entry"]["arenaDebit"]["value"]},
                "executionApproved": False,
                "qualification": "Frozen for independent review only; separate hash-bound Root approval and lease required",
            }
            destination = output / (faction + ".json")
            contract.write_json(destination, manifest)
            manifests.append(contract.binding(destination))
        contract.frozen_map(root, source_files)
        if module.generated(root) != generated or contract.files_under(pack_root) != pack_files \
                or contract.files_under(symbols, ".sym") != symbol_files \
                or args.native_ledger.read_bytes() != ledger:
            raise RuntimeError("Source/data/pack/symbol/native epoch changed during freezing")
        owner.result.update(passed=True, manifests=manifests, runtime=runtime,
            sourceEpoch=contract.binding(epoch), executionApproved=False)
    except BaseException as error:
        owner.result.update(passed=False, error=str(error))
    finally:
        for kind in handlers:
            signal.signal(kind, signal.SIG_IGN)
        owner.cleanup()
        for kind, handler in handlers.items():
            signal.signal(kind, handler)
        owner.best_effort_save("Final freezer result")
    return contract.FIRST if owner.result["passed"] else contract.ONE


if __name__ == "__main__":
    raise SystemExit(main())
