#!/usr/bin/env python3
"""Future leased capture binding only; current source draft is never executed."""
import argparse
import hashlib
import json
import os
from pathlib import Path

ONE = 1
ZERO = 0
MAXIMUM_CLIENT_LAUNCHES = 4
MAXIMUM_SOCKET_BYTES = 104
MINIMUM_PORT = 48700
END_PORT = 48800
NODE_DIRECTORY = "/Users/robert/Library/Application Support/Herd/config/nvm/versions/node/v24.18.0/bin"
PREFIX = "tools/client910/fixtures/session-replay/"
CORP_PREFIX = PREFIX + "corporeal-beast/"
HELPERS = {
    "owner": PREFIX + "barrows/record.py", "record": CORP_PREFIX + "record.py",
    "freeze": CORP_PREFIX + "freeze.py", "contract": CORP_PREFIX + "contract.py",
    "seed": CORP_PREFIX + "seed.mjs", "server": CORP_PREFIX + "server.mjs",
    "driver": CORP_PREFIX + "driver.py", "control": "tools/client910/scripts/control.py",
    "sharedDriver": PREFIX + "barrows/driver.py", "baseDriver": PREFIX + "boss-encounters/driver.py",
    "foodObserver": PREFIX + "barrows/food-receipts.mjs", "publications": PREFIX + "god-wars/publications.py",
    "peerAdapter": PREFIX + "god-wars/peer_adapter.mjs", "broker": PREFIX + "god-wars/peer_broker.mjs",
    "loginProvider": PREFIX + "god-wars/login_provider.mjs", "deviceEnvelope": PREFIX + "legacy-interface/session.rtr",
    "loginTestkit": "server/src/lostcity/network/SkillsE2E.testkit.ts",
    "socketTestkit": "server/src/lostcity/network/SocketE2E.testkit.ts",
    "loginCrypto": "server/src/lostcity/network/LoginCrypto.ts", "wiredGuard": "ref/independence/wired-guard.sh",
}

def bind(path):
    path = Path(path).absolute()
    return {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("root", "binary-manifest", "adoption-manifest", "out", "spec", "launch-ledger"):
        parser.add_argument("--" + name, required=True)
    for name in ("lobby-port", "world-port"):
        parser.add_argument("--" + name, required=True, type=int)
    args = parser.parse_args()
    root, out, spec = Path(args.root).absolute(), Path(args.out).absolute(), Path(args.spec).absolute()
    if root == Path("/Users/robert/projects/alto") or out.exists() or spec.exists():
        raise ValueError("Own worktree, fresh evidence and specification required")
    ports = {"lobby": args.lobby_port, "world": args.world_port}
    if len(set(ports.values())) != len(ports) or any(not MINIMUM_PORT <= value < END_PORT for value in ports.values()):
        raise ValueError("Explicit leased Corp ports required; no automatically guessed live ports")
    if any(len(str(out / "runtime" / name).encode()) >= MAXIMUM_SOCKET_BYTES for name in ("p.sock", "c.sock")):
        raise ValueError("Choose a fresh short durable evidence directory")
    manifest = json.loads(Path(args.binary_manifest).read_text())
    if manifest["sourceRoot"] != str(root) or not manifest["sources"]:
        raise ValueError("Own current compiled-source manifest required; source-only preparation cannot supply it")
    adoption = json.loads(Path(args.adoption_manifest).read_text())
    if adoption.get("status") != "adopted_on_actual_main_after_dk_kq" or adoption.get("root") != str(root):
        raise ValueError("Real predecessor adoption receipt required; provisional source checkpoint is insufficient")
    adopted = adoption["sources"]
    if not adopted or any(bind(root / relative)["sha256"] != expected for relative, expected in adopted.items()):
        raise ValueError("Adopted exact source hashes differ")
    files = {name: bind(root / relative) for name, relative in HELPERS.items()}
    files["binaryManifest"] = bind(args.binary_manifest)
    files["binary"] = bind(manifest["binary"]["path"])
    files["adoptionManifest"] = bind(args.adoption_manifest)
    if files["binary"] != manifest["binary"]:
        raise ValueError("Actual compiled binary differs from its manifest")
    food = root / "server/data/generated/food/foods.json"
    files["foodRules"] = bind(food)
    value = {"lane": "corporeal-beast", "root": str(root), "out": str(out), "ports": ports,
        "files": files, "adoptedSources": adopted, "launchLedger": str(Path(args.launch_ledger).absolute()),
        "seedArgs": ["corporeal-beast"], "maximumActualClientLaunches": MAXIMUM_CLIENT_LAUNCHES,
        "environment": {"PATH": NODE_DIRECTORY + os.pathsep + os.environ["PATH"],
            "CARGO_BUILD_JOBS": "2", "NEXTEST_TEST_THREADS": "4", "RUST_TEST_THREADS": "1"},
        "driverArgs": ["--socket", "{control}", "--control-module", files["control"]["path"],
            "--plan", "{work}/plan.json", "--food-rules", str(food), "--symbols", str(root / "revisions/910/symbols"),
            "--receipts", "{work}/combat-receipts.jsonl", "--journal", "{out}/driver-journal.jsonl"]}
    spec.parent.mkdir(parents=True, exist_ok=True)
    spec.write_text(json.dumps(value, indent=2) + "\n")
    print(spec)

if __name__ == "__main__":
    main()
