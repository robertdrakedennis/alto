#!/usr/bin/env python3
"""Finite ordinary-client recorder using the existing exact-process owner."""
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import sys
import time

SESSION_SECONDS = 600
START_SECONDS = 90
DRIVER_SECONDS = 400
CHECK_SECONDS = 45
BROKER_CLOSE_SECONDS = 50
POLL_SECONDS = 0.2
IDENTITY_SAMPLE_SECONDS = 2
MAX_LAUNCHES = 4
MAX_CONTROL_SOCKET_PATH_BYTES = 104
FORBIDDEN_ENV = frozenset((
    "ALTO_NPC_SPAWNS", "ALTO_DEV_NPCS", "ALTO_DEV_NPC_WANDER",
    "ALTO_THIEVING_ROLLS", "ALTO_WC_ALWAYS", "ALTO_SKILLS_ALWAYS",
    "ALTO_TRAIL_STEPS", "ALTO_GE_MARKET_MAKER", "ALTO_PREFLIGHT_LOGIN_MODULE",
))
FIRST = 0
ONE = 1
MIN_PORT = 48700
END_PORT = 48800
ENTRY = "app::session_replay::observed_session::authenticated_observed_session"


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def binding(path):
    path = Path(path).absolute()
    return {"path": str(path), "sha256": sha(path)}


def load(entry, name):
    if binding(entry["path"]) != entry:
        raise RuntimeError("Frozen helper changed: " + entry["path"])
    spec = importlib.util.spec_from_file_location(name, entry["path"])
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def epoch(root, extra):
    paths = []
    for relative in ("server/src", "server/data/generated", "packages/domain/src", "revisions/910/symbols"):
        paths += [path for path in (root / relative).rglob("*") if path.is_file()]
    for relative in extra:
        if Path(relative).is_absolute() or ".." in Path(relative).parts:
            raise RuntimeError("Frozen source path escapes own root")
        paths.append(root / relative)
    return {str(path.relative_to(root)): sha(path) for path in sorted(set(paths))}


def checked(entry):
    if binding(entry["path"]) != entry:
        raise RuntimeError("Frozen input changed: " + entry["path"])
    return entry["path"]


def main():
    spec = json.loads(Path(sys.argv[ONE]).read_text())
    if spec.get("lane") != "corporeal-beast" or spec.get("maximumActualClientLaunches") != MAX_LAUNCHES:
        raise RuntimeError("Bounded Corp specification required")
    root, out = Path(spec["root"]), Path(spec["out"])
    if not root.is_absolute() or root == Path('/Users/robert/projects/alto') or out.exists():
        raise RuntimeError("Own worktree and fresh result directory required")
    out.mkdir(parents=True)
    helpers = spec["files"]
    paths = {name: checked(value) for name, value in helpers.items()}
    extra_sources = {*json.loads(Path(paths["binaryManifest"]).read_text())["sources"], *spec["adoptedSources"]}
    owner_module = load(helpers["owner"], "ordinary_existing_process_owner")
    owner = owner_module.Owner(out, root)
    owner.result.update(expectedPriorLaunches=FIRST, maximumClientLaunches=MAX_LAUNCHES,
        actualClientLaunchesThisRun=FIRST, rendered=False, captureQualification="bounded ordinary native melee sample; full kill/loot not claimed; pixels unqualified")
    handlers = {kind: signal.getsignal(kind) for kind in (signal.SIGTERM, signal.SIGINT)}
    for kind in handlers:
        signal.signal(kind, owner_module.interrupt)
    frozen = None
    try:
        ports = spec["ports"]
        if set(ports) != {"lobby", "world"} or len(set(ports.values())) != len(ports):
            raise RuntimeError("Distinct assigned ports required")
        for port in ports.values():
            if not MIN_PORT <= port < END_PORT:
                raise RuntimeError("Port outside owned lane range")
            with socket.socket() as probe:
                probe.bind(("127.0.0.1", port))
        frozen = epoch(root, extra_sources)
        (out / "source-input-epoch.json").write_text(json.dumps(frozen, sort_keys=True, indent=2) + "\n")
        build = json.loads(Path(paths["binaryManifest"]).read_text())
        if build["sourceRoot"] != str(root) or not build["sources"]:
            raise RuntimeError("Ordinary core was not built from this worktree")
        if {key: build["binary"][key] for key in ("path", "sha256")} != helpers["binary"]:
            raise RuntimeError("Compiled core does not match its source manifest")
        for relative, expected in build["sources"].items():
            path = Path(relative)
            if path.is_absolute() or ".." in path.parts or sha(root / path) != expected:
                raise RuntimeError("Compiled source differs: " + relative)
        owner.result["compiledSourceManifest"] = helpers["binaryManifest"]
        for relative, expected in spec["adoptedSources"].items():
            if sha(root / relative) != expected:
                raise RuntimeError("Corp adoption tree changed: " + relative)
        env = dict(os.environ)
        for key in list(env):
            if key.startswith("CLIENT910_") or key in FORBIDDEN_ENV:
                env.pop(key)
        requested_env = spec.get("environment", {})
        if any(key.startswith("CLIENT910_") or key in FORBIDDEN_ENV for key in requested_env):
            raise RuntimeError("Capture environment attempts to restore gameplay assistance")
        env.update(requested_env)
        # Account, plan and passive receipts survive a reboot beside the capture.
        scratch = out / "runtime"
        scratch.mkdir()
        work = scratch / "w"
        work.mkdir()
        transport, control = scratch / "p.sock", scratch / "c.sock"
        if any(len(str(path).encode()) >= MAX_CONTROL_SOCKET_PATH_BYTES for path in (transport, control)):
            raise RuntimeError("Durable runtime path exceeds the native socket pathname bound")
        env.update(ALTO_BOSS_RUNTIME_ROOT=str(root), ALTO_GENERATED_DIR=str(root / "server/data/generated"),
            ALTO_PLAYER_DATA_DIR=str(work / "players"), ALTO_LOBBY_PORT=str(ports["lobby"]),
            ALTO_WORLD_PORT=str(ports["world"]), ALTO_LOGIN_CRYPTO="on", ALTO_TRACE_INFO="1", NODE_OPTIONS="")
        executables = {name: shutil.which(name, path=env.get("PATH")) for name in ("node", "python3")}
        if any(value is None for value in executables.values()):
            raise RuntimeError("Node and Python runtimes are required")
        owner.result.update(scratch=str(scratch), work=str(work), files=helpers, ports=ports,
            limitations=["No pixels or FPS proof", "Initial fixture only; no live state assistance"])
        owner.run("seed", [executables["node"], paths["seed"], str(work), *spec.get("seedArgs", [])],
                  root / "server", env, CHECK_SECONDS)
        pairs = {}
        pairs["lobby"] = owner.start("lobby", [executables["node"], "src/lostcity/lobby.ts"], root / "server", env, SESSION_SECONDS)
        pairs["world"] = owner.start("world", [executables["node"], paths["server"], str(work)], root / "server", env, SESSION_SECONDS)
        def wait(predicate, seconds):
            deadline, sample_at = time.monotonic() + seconds, time.monotonic()
            while True:
                if predicate():
                    return
                for name, (child, row) in pairs.items():
                    if child.poll() is not None:
                        row.update(exit=child.wait(), waited=True)
                        raise RuntimeError("Owned ordinary process ended early: " + name)
                now = time.monotonic()
                if now >= deadline:
                    raise TimeoutError("Bounded ordinary recorder stage expired")
                if now >= sample_at:
                    owner.sample()
                    sample_at = now + IDENTITY_SAMPLE_SECONDS
                time.sleep(POLL_SECONDS)
        wait(lambda: all((out / (name + ".log")).exists()
            and f"listening on port {ports[name]}" in (out / (name + ".log")).read_text().lower()
            for name in ("lobby", "world")), START_SECONDS)
        peer = {"peerCount": ONE, "headlessOnly": True, "broker": helpers["broker"],
            "loginTestkit": helpers["loginTestkit"], "deviceEnvelope": helpers["deviceEnvelope"],
            "receipts": str(out / "backend-receipts.jsonl"), "peer": {
                "name": "Alice", "port": ports["world"], "transport": str(transport),
                "control": str(control), "deviceEnvelope": paths["deviceEnvelope"],
                "modules": {name: helpers[name] for name in ("socketTestkit", "loginProvider", "loginCrypto")}}}
        peer_path = out / "one-peer.json"
        peer_path.write_text(json.dumps(peer, indent=2) + "\n")
        pairs["broker"] = owner.start("broker", [executables["node"], paths["peerAdapter"], str(peer_path)], root / "server", env, SESSION_SECONDS)
        wait(transport.exists, START_SECONDS)
        owner.run("wired-guard", ["bash", "ref/independence/wired-guard.sh"], root, env, CHECK_SECONDS)
        ledger = Path(spec["launchLedger"])
        ledger.parent.mkdir(parents=True, exist_ok=True)
        with ledger.open("a+", encoding="utf8") as stream:
            fcntl.flock(stream, fcntl.LOCK_EX)
            stream.seek(FIRST)
            prior = [json.loads(line) for line in stream if line.strip()]
            if len(prior) >= MAX_LAUNCHES:
                raise RuntimeError("This lane exhausted its four actual client launches")
            stream.write(json.dumps({"ordinal": len(prior) + ONE, "time": time.time(),
                "root": str(root), "out": str(out), "binary": helpers["binary"]}) + "\n")
            stream.flush()
            os.fsync(stream.fileno())
            owner.result["expectedPriorLaunches"] = len(prior)
        core_env = dict(env, CLIENT910_OBSERVED_TRANSPORT=str(transport), CLIENT910_CONTROL=str(control),
            CLIENT910_OBSERVED_SESSION_SECONDS=str(SESSION_SECONDS), CLIENT910_RECORD=str(out / "session.rtr"))
        pairs["core"] = owner.start("core", [paths["binary"], "--ignored", "--exact", ENTRY,
            "--nocapture", "--test-threads=1"], root / "tools", core_env, SESSION_SECONDS)
        owner.result["actualClientLaunchesThisRun"] = ONE
        wait(control.exists, START_SECONDS)
        observer = load(helpers["control"], "ordinary_recording_control").Control(str(control), CHECK_SECONDS)
        try:
            def ready():
                response = observer.request({"command": "snapshot"})
                return response.get("status") == "observed" and all(response.get("data", {}).get(key)
                    for key in ("ready", "player", "terrain_present"))
            wait(ready, START_SECONDS)
        finally:
            observer.close()
        argv = [executables["python3"], paths["driver"], *[value.replace("{work}", str(work))
            .replace("{out}", str(out)).replace("{control}", str(control)) for value in spec["driverArgs"]]]
        driver, row = owner.start("driver", argv, root, env, DRIVER_SECONDS)
        wait(lambda: driver.poll() is not None, DRIVER_SECONDS)
        row.update(exit=driver.wait(), waited=True)
        if row["exit"]:
            raise RuntimeError("Ordinary encounter driver failed; preserve exact evidence")
        broker, broker_row = pairs.pop("broker")
        owner.stop_direct(broker, broker_row, BROKER_CLOSE_SECONDS)
        if broker_row["exit"]:
            raise RuntimeError("Ordinary broker did not close cleanly")
        wait(lambda: pairs["core"][FIRST].poll() is not None, CHECK_SECONDS)
        core, row = pairs.pop("core")
        row.update(exit=core.wait(), waited=True)
        if row["exit"]:
            raise RuntimeError("Ordinary core did not close cleanly")
        owner.result.update(passed=True, owningReplayPassed=False)
    except BaseException as error:
        owner.result.update(passed=False, error=repr(error))
    finally:
        try:
            owner.cleanup()
        finally:
            try:
                owner.result["sourceInputsUnchanged"] = frozen == epoch(root, extra_sources)
                if not owner.result["sourceInputsUnchanged"]:
                    owner.result["passed"] = False
                for name, value in helpers.items():
                    checked(value)
                recorded = out / "session.rtr"
                if recorded.exists():
                    owner.result["actualRecording"] = {**binding(recorded), "bytes": recorded.stat().st_size}
            except BaseException as error:
                owner.cleanup_error("Closed evidence binding", error)
            finally:
                for kind, handler in handlers.items():
                    signal.signal(kind, handler)
                owner.best_effort_save("Final ordinary recorder")
    print(json.dumps({key: owner.result.get(key) for key in
        ("passed", "error", "allDirectChildrenWaited", "ownedSurvivors", "actualClientLaunchesThisRun")}), flush=True)
    return FIRST if owner.result["passed"] else ONE


if __name__ == "__main__":
    raise SystemExit(main())
