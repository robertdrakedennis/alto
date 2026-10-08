#!/usr/bin/env python3
"""Finite owner for the ordinary adaptive Barrows recording.

Source proposal only. Run only with Root's subsequently frozen, reviewed
manifest. This file does not import the drivers while validating that manifest.
It owns seed, lobby, world, client, driver, metadata and postprocessing directly;
it never launches the historical timed record.py or its outer wrapper.
"""
import argparse
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import time
import zlib


PRIVATE_ROOT = None
LAUNCH_LEDGER = None
EXPECTED_PRIOR_LAUNCHES = 3
MAXIMUM_CLIENT_LAUNCHES = 4
MANIFEST_FORMAT = 1
INDENT = 2
POLL_SECONDS = 0.2
IDENTITY_SAMPLE_SECONDS = 2
IDENTITY_QUERY_SECONDS = 10
METADATA_SECONDS = 30
TERM_SECONDS = 30
KILL_SECONDS = 10
DESCENDANT_SECONDS = 10
MAX_CAPTURE_SECONDS = 1800
MAX_SERVER_START_SECONDS = 90
MAX_TERMINAL_SECONDS = 120
MAX_LOG_BYTES = 64 * 1024 * 1024
MAX_JSON_BYTES = 32 * 1024 * 1024
MAX_PNG_BYTES = 128 * 1024 * 1024
MAX_PNG_PIXELS = 16 * 1024 * 1024
MAX_RECORD_BYTES = 64 * 1024 * 1024
MAX_RTR_BYTES_PER_POLL = 32 * 1024 * 1024
MAX_JOURNAL_LINE_BYTES = 1024 * 1024
SCREENSHOT_PERIOD_CYCLES = 250
SCREENSHOT_SENTINEL_CYCLE = 1_000_000
SCREENSHOT_DIGITS = 5
MAX_OBSERVATION_BRACKET_CYCLES = 100
WINDOW_WIDTH = 1024
WINDOW_HEIGHT = 768
MAX_CONTROL_SOCKET_PATH_BYTES = 104
FIRST_LANE_PORT = 48600
END_LANE_PORT = 48700
RTR_MAGIC = b"RTR1"
RTR_VERSION = 0x0001_0000
RTR_FILE_HEADER_BYTES = 8
RTR_RECORD_HEADER_BYTES = 12
NEXT_CYCLE = 1
NO_COMPLETE_CYCLE = -1
PNG_MAGIC = b"\x89PNG\r\n\x1a\n"
PNG_CHUNK_HEADER_BYTES = 8
PNG_CHUNK_CRC_BYTES = 4
PNG_HEADER_BYTES = 13
PNG_RGB_CHANNELS = 3
PNG_DEPTH = 8
PNG_RGB_COLOR = 2
PNG_FILTER_BYTE = 1
SEMANTIC_STATUS = "semantic_pass_requires_recorded_replay_and_readback_qualification"
OLD_INPUT_VARIABLES = (
    "CLIENT910_UI_OPERATIONS", "CLIENT910_UI_CLICKS", "CLIENT910_UI_HOVER",
    "CLIENT910_KEY_INPUT", "CLIENT910_SCREENSHOT_SERIES", "CLIENT910_RECORD",
    "CLIENT910_RECORD_MODE", "CLIENT910_CONTROL", "CLIENT910_SCREENSHOT_CYCLE",
    "CLIENT910_WINDOW_RESIZES", "CLIENT910_CONSOLE_COMMANDS",
    "CLIENT910_TYPE_INPUT", "CLIENT910_WHEEL_INPUT", "CLIENT910_TOOLKIT_INPUT",
    "CLIENT910_UI_INPUT", "CLIENT910_UI_CLICK", "CLIENT910_CUTSCENE",
    "CLIENT910_DROP_CONNECTION",
)


def digest(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as source:
        for block in iter(lambda: source.read(MAX_RTR_BYTES_PER_POLL), b""):
            value.update(block)
    return value.hexdigest()


def read_json(path):
    path = Path(path)
    if path.stat().st_size > MAX_JSON_BYTES:
        raise RuntimeError(f"JSON exceeds finite bound: {path}")
    return json.loads(path.read_text())


def write_json(path, value):
    temporary = Path(str(path) + ".writing")
    with temporary.open("w", encoding="utf8") as output:
        json.dump(value, output, sort_keys=True, indent=INDENT)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(path)


def bounded_text(path):
    path = Path(path)
    if not path.exists():
        return ""
    if path.stat().st_size > MAX_LOG_BYTES:
        raise RuntimeError(f"Log exceeds finite evidence bound: {path}")
    return path.read_text(errors="replace")


def generated(root):
    directory = Path(root) / "server/data/generated"
    return {str(path.relative_to(root)): digest(path)
        for path in sorted(directory.rglob("*.json"))}


def check_file(entry):
    path = Path(entry["path"])
    if not path.is_absolute() or not path.is_file() or digest(path) != entry["sha256"]:
        raise RuntimeError(f"Frozen file differs or is unavailable: {path}")
    return path


PRIOR_NATIVE_RECEIPT_BYTES = 32 * 1024 * 1024


def prior_native_file(path):
    path = Path(path)
    if not path.is_absolute() or not path.is_file() or path.stat().st_size > PRIOR_NATIVE_RECEIPT_BYTES:
        raise RuntimeError(f"Original native receipt exceeds finite bound or is absent: {path}")
    return path


def prior_native_json(entry):
    # Reject the bound before either hash or JSON read, including manifests.
    prior_native_file(entry["path"])
    return read_json(check_file(entry))


def qualified_native_prior(manifest, prior):
    """Validate original closed receipts; historical missing fields stay absent.

    This is bounded saved-file validation only. No private module is imported,
    and none of these historical PIDs become this capture owner's live roots.
    """
    actual = manifest.get("actualNativePriorReceipts")
    if not isinstance(actual, dict) or actual.get("expectedPrior") != EXPECTED_PRIOR_LAUNCHES:
        raise RuntimeError("Frozen original native receipt qualification is absent")
    prior_native_file(LAUNCH_LEDGER)
    ledger_binding = {"path": str(LAUNCH_LEDGER), "sha256": digest(LAUNCH_LEDGER)}
    if actual.get("ledger") != ledger_binding or actual.get("ledgerMutated") is not False:
        raise RuntimeError("Original native receipt ledger binding differs")
    # Preserve the validator's source provenance without executing it.
    prior_native_file(actual["validator"]["path"])
    check_file(actual["validator"])
    original = prior_native_json(actual["manifest"])
    if original.get("format") != MANIFEST_FORMAT or original.get("expectedPrior") != EXPECTED_PRIOR_LAUNCHES or original.get("ledger") != ledger_binding:
        raise RuntimeError("Original native receipt manifest differs")
    entries = original.get("launches", [])
    ordinals = list(range(NEXT_CYCLE, MAXIMUM_CLIENT_LAUNCHES))
    if [row.get("launch") for row in prior] != ordinals or [row.get("launch") for row in entries] != ordinals:
        raise RuntimeError("Expected exactly the original three native launches")
    qualified = []
    for launch, entry in zip(prior, entries):
        if any(entry.get(name) != launch.get(name) for name in ("launch", "work", "pid")):
            raise RuntimeError("Original native ordinal/work/PID differs")
        if entry.get("startIdentity") != str(launch.get("start", "")).strip():
            raise RuntimeError("Original native start identity differs")
        inner = prior_native_json(entry["recordingReceipt"])
        result, children = inner["result"], inner.get("children", [])
        clients = [row for row in children if row.get("name") == "client"]
        if not children or not all(row.get("waited") is True and type(row.get("exit")) is int for row in children) or len(clients) != NEXT_CYCLE:
            raise RuntimeError("Original native direct children are not exactly finally waited")
        client = clients[0]
        if client.get("pid") != launch["pid"] or str(client.get("startIdentity", "")).strip() != entry["startIdentity"]:
            raise RuntimeError("Original native client PID/start differs")
        if result.get("work") != launch["work"] or result.get("provenance") != launch.get("sourceProof") or result.get("passed") is not False:
            raise RuntimeError("Original failed gameplay/work/source proof differs")
        if entry["launch"] == NEXT_CYCLE:
            if launch.get("waited") is not True or launch.get("exit") != client["exit"] or launch.get("receipt") != entry["recordingReceipt"]["path"] or launch.get("receiptSha256") != entry["recordingReceipt"]["sha256"]:
                raise RuntimeError("First imported native wait receipt differs")
            if entry.get("cleanupScope") != "direct-child-waits-only; historical descendants unavailable":
                raise RuntimeError("First native historical descendant limitation differs")
            outer_binding = None
        else:
            if result.get("allDirectChildrenWaited") is not True or any(result.get(name) != [] for name in ("cleanupErrors", "descendantSurvivors", "survivors")):
                raise RuntimeError("Original recording owner cleanup is incomplete")
            outer_binding = entry["outerReceipt"]
            outer = prior_native_json(outer_binding)
            if outer.get("passed") is not False or outer.get("work") != launch["work"] or outer.get("allDirectChildrenWaited") is not True:
                raise RuntimeError("Original outer failed capture/waits differ")
            if outer.get("recordingReceipt") != entry["recordingReceipt"]["path"] or outer.get("recordingReceiptSha256") != entry["recordingReceipt"]["sha256"]:
                raise RuntimeError("Original outer recording binding differs")
            if outer.get("recordingAllChildrenWaited") is not True or outer.get("recordingSurvivors") != [] or outer.get("windowBudgetAfter") != entry["launch"]:
                raise RuntimeError("Original outer native wait/budget differs")
            if entry.get("cleanupScope") != "recording-owner direct waits and observed-identity survivors empty":
                raise RuntimeError("Original recording cleanup qualification differs")
        qualified.append({"launch": entry["launch"], "work": entry["work"], "pid": client["pid"],
            "startIdentity": entry["startIdentity"], "nativeWaited": client["waited"], "nativeExit": client["exit"],
            "directChildren": len(children), "allDirectChildrenWaited": True,
            "recordingReceipt": entry["recordingReceipt"], "outerReceipt": outer_binding,
            "cleanupScope": entry["cleanupScope"], "gameplayAccepted": False})
    if actual.get("launches") != qualified:
        raise RuntimeError("Frozen native qualification does not match original closed receipts")
    return actual


class Owner:
    """Exact live Popen roots and descendants observed beneath those roots.

    Historical launch-ledger PIDs are evidence only; they are never roots.
    A reaped child's recycled PID cannot re-enter this owner's closure.
    """
    def __init__(self, output, root):
        self.output = output
        self.root = root
        self.children = []
        self.rows = []
        self.observed = {}
        self.logs = []
        self.cleanup_errors = []
        self.result = {"passed": False, "captureQualification": "pending",
            "actualClientLaunchesThisRun": 0, "expectedPriorLaunches": EXPECTED_PRIOR_LAUNCHES,
            "maximumClientLaunches": MAXIMUM_CLIENT_LAUNCHES}

    def save(self):
        write_json(self.output / "processes.json", self.rows)
        write_json(self.output / "observed-identities.json", list(self.observed.values()))
        write_json(self.output / "result.json", self.result)

    def cleanup_error(self, stage, error):
        self.cleanup_errors.append(f"{stage}: {error}")
        self.result["passed"] = False

    def best_effort_save(self, stage):
        # Persistence must never be a prerequisite for stopping/reaping an
        # already owned child or restoring the caller's signal handlers.
        try:
            self.save()
            return True
        except BaseException as error:
            self.cleanup_error(stage, error)
            self.result.setdefault("evidenceWriteErrors", []).append(str(error))
            return False

    def register(self, child, name, arguments, seconds, query=False):
        row = {"name": name, "pid": child.pid, "arguments": arguments,
            "deadlineSeconds": seconds, "startedMonotonic": time.monotonic(),
            "startedWallTime": time.time(), "startIdentity": None,
            "waited": False, "query": query}
        # This is the first operation after every successful Popen. No ps,
        # artifact write, identity lookup or assertion precedes registration.
        self.children.append((child, row))
        self.rows.append(row)
        return row

    def finish_query(self, child, row):
        self.stop_direct(child, row, IDENTITY_QUERY_SECONDS)

    def identities(self, persist=True):
        arguments = ["ps", "-axo", "pid=,ppid=,lstart="]
        child = subprocess.Popen(arguments, cwd=self.root, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        row = self.register(child, "process-identities", arguments, IDENTITY_QUERY_SECONDS, True)
        try:
            if persist:
                self.save()
            output, error = child.communicate(timeout=IDENTITY_QUERY_SECONDS)
            if child.returncode:
                raise RuntimeError(f"Owned process identity query failed: {error}")
            current = {}
            for line in output.splitlines():
                columns = line.split(maxsplit=2)
                if len(columns) == 3:
                    pid, parent, started = columns
                    current[int(pid)] = {"pid": int(pid), "parent": int(parent),
                        "start": started.strip()}
            row["startIdentity"] = current.get(child.pid, {}).get("start")
            return current
        finally:
            self.finish_query(child, row)

    def sample(self, persist=True):
        current = self.identities(persist=persist)
        owned = set()
        for child, row in self.children:
            if child.poll() is not None:
                continue
            identity = current.get(child.pid)
            if identity is None:
                continue
            expected = row.get("startIdentity")
            if expected is not None and identity["start"] != expected:
                continue
            if expected is None:
                row["startIdentity"] = identity["start"]
            owned.add(child.pid)
        owned.update(pid for pid, started in self.observed
            if current.get(pid, {}).get("start") == started)
        changed = True
        while changed:
            changed = False
            for pid, identity in current.items():
                if pid not in owned and identity["parent"] in owned:
                    owned.add(pid)
                    changed = True
        for pid in owned:
            identity = current[pid]
            key = pid, identity["start"]
            if key not in self.observed:
                self.observed[key] = {**identity, "firstObservedWallTime": time.time()}
            self.observed[key]["lastObservedWallTime"] = time.time()
        if persist:
            self.save()
        return current

    def exact_signal(self, child, row, action):
        if not any(owned is child and owned_row is row for owned, owned_row in self.children):
            raise RuntimeError("Refusing signal to an unowned direct Popen")
        if child.poll() is not None:
            return
        # A currently live, unreaped Popen cannot have had its PID recycled.
        # Popen.send_signal rechecks poll immediately before signaling. This
        # direct-child authority does not depend on ps or artifact availability;
        # descendant signals still require an observed matching PID/start.
        child.send_signal(action)
        row["termination"] = f"exact owned PID {action.name}"
        row["signalAuthority"] = "currently live unreaped owned Popen"
        if action == signal.SIGTERM:
            row["termSentMonotonic"] = time.monotonic()

    def stop_direct(self, child, row, grace_seconds):
        if row.get("waited") and child.poll() is not None:
            return
        try:
            if child.poll() is None:
                if row.get("termSentMonotonic") is None:
                    try:
                        self.exact_signal(child, row, signal.SIGTERM)
                    except BaseException as error:
                        self.cleanup_error(f"Direct TERM {row['name']}", error)
                # Grace starts even if TERM failed; KILL is still attempted
                # independently in finally. Neither grace nor wait invokes ps.
                started = row.get("termSentMonotonic", time.monotonic())
                row["termGraceSeconds"] = grace_seconds
                try:
                    child.wait(timeout=max(0, started + grace_seconds - time.monotonic()))
                except subprocess.TimeoutExpired:
                    pass
                except BaseException as error:
                    self.cleanup_error(f"Direct TERM wait {row['name']}", error)
        finally:
            try:
                if child.poll() is None:
                    try:
                        self.exact_signal(child, row, signal.SIGKILL)
                    except BaseException as error:
                        self.cleanup_error(f"Direct KILL {row['name']}", error)
                # Explicit bounded final wait also runs after query/artifact,
                # TERM, grace-wait or KILL failures. Only direct children wait.
                try:
                    row.update(exit=child.wait(timeout=KILL_SECONDS), waited=True)
                except BaseException as error:
                    row["cleanupError"] = str(error)
                    self.cleanup_error(f"Direct final wait {row['name']}", error)
            finally:
                self.best_effort_save(f"Direct cleanup receipt {row['name']}")

    def stop(self, child, row):
        self.stop_direct(child, row, IDENTITY_QUERY_SECONDS if row.get("query") else TERM_SECONDS)

    def start(self, name, arguments, cwd, environment, seconds, launch=None):
        log = (self.output / f"{name}.log").open("w")
        self.logs.append(log)
        launch_file = None
        try:
            launches = None
            if launch is not None:
                launch_file = LAUNCH_LEDGER.open("r+")
                fcntl.flock(launch_file, fcntl.LOCK_EX)
                launches = json.load(launch_file)
                if len(launches) != EXPECTED_PRIOR_LAUNCHES or len(launches) >= MAXIMUM_CLIENT_LAUNCHES:
                    raise RuntimeError("Actual cumulative Barrows launch budget changed")
                if digest(LAUNCH_LEDGER) != launch["ledgerSha256"]:
                    raise RuntimeError("Existing prior-launch evidence changed after review")
                # All args, cwd, env, output handles and budget checks precede
                # the wired guard. Nothing fallible is inserted between that
                # callback's successful return and actual client Popen.
                launch["guard"]()
            child = subprocess.Popen(arguments, cwd=cwd, env=environment,
                stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                start_new_session=True)
            row = self.register(child, name, arguments, seconds)
            if launch is not None:
                self.result["actualClientLaunchesThisRun"] += NEXT_CYCLE
                launches.append({"launch": len(launches) + NEXT_CYCLE,
                    "work": self.result["scratch"], "pid": child.pid, "start": None,
                    "startedWallTime": row["startedWallTime"],
                    "sourceProof": self.result["provenance"]})
                # Persist the actual fourth launch before ps or other artifacts.
                launch_file.seek(0)
                json.dump(launches, launch_file, indent=INDENT)
                launch_file.truncate()
                launch_file.flush()
                os.fsync(launch_file.fileno())
                fcntl.flock(launch_file, fcntl.LOCK_UN)
                launch_file.close()
                launch_file = None
            self.save()
            self.sample()
            if launch is not None:
                self.update_launch(child, row)
            return child, row
        finally:
            if launch_file is not None:
                fcntl.flock(launch_file, fcntl.LOCK_UN)
                launch_file.close()

    def update_launch(self, child, row):
        with LAUNCH_LEDGER.open("r+") as file:
            fcntl.flock(file, fcntl.LOCK_EX)
            launches = json.load(file)
            matching = [entry for entry in launches
                if entry["pid"] == child.pid and entry["work"] == self.result["scratch"]]
            if len(matching) != NEXT_CYCLE:
                raise RuntimeError("Actual launch identity absent/ambiguous in existing ledger")
            receipt = self.output / "processes.json"
            receipt_hash = None
            try:
                receipt_hash = digest(receipt)
            except BaseException as error:
                self.cleanup_error("Launch ledger receipt hash", error)
            matching[0].update(start=row.get("startIdentity"), waited=row.get("waited", False),
                exit=row.get("exit"), receipt=str(receipt), receiptSha256=receipt_hash,
                receiptComplete=receipt_hash is not None and not self.result.get("evidenceWriteErrors"))
            file.seek(0)
            json.dump(launches, file, indent=INDENT)
            file.truncate()
            file.flush()
            os.fsync(file.fileno())

    def run(self, name, arguments, cwd, environment, seconds):
        child, row = self.start(name, arguments, cwd, environment, seconds)
        deadline = time.monotonic() + seconds
        try:
            while child.poll() is None:
                if time.monotonic() >= deadline:
                    raise TimeoutError(f"Finite child deadline: {name}")
                self.sample()
                time.sleep(POLL_SECONDS)
            row.update(exit=child.wait(timeout=KILL_SECONDS), waited=True)
            self.save()
            if row["exit"]:
                raise RuntimeError(f"Owned child failed: {name}, exit={row['exit']}")
            return bounded_text(self.output / f"{name}.log")
        finally:
            self.stop(child, row)

    def finish_descendants(self):
        direct = {(child.pid, row.get("startIdentity")) for child, row in self.children}
        def remaining():
            current = self.sample(persist=False)
            return {key: row for key, row in self.observed.items()
                if key not in direct and current.get(key[0], {}).get("start") == key[1]}
        alive = remaining()
        for action in (signal.SIGTERM, signal.SIGKILL):
            for (pid, started), row in list(alive.items()):
                # No group signaling; recheck each established exact identity.
                if self.identities(persist=False).get(pid, {}).get("start") != started:
                    continue
                try:
                    os.kill(pid, action)
                    row["cleanupSignal"] = action.name
                except ProcessLookupError:
                    pass
            deadline = time.monotonic() + DESCENDANT_SECONDS
            while alive and time.monotonic() < deadline:
                time.sleep(POLL_SECONDS)
                alive = remaining()
            if not alive:
                break
        self.result["ownedSurvivors"] = list(alive.values())
        self.result["descendantQualification"] = "live-start observed identities; descendants are not direct waitable children"
        if alive:
            self.cleanup_errors.append("Exact previously observed descendants survived bounded cleanup")

    def cleanup(self):
        # Take the live closure before any direct root is stopped.
        try:
            self.sample(persist=False)
        except BaseException as error:
            self.cleanup_error("Pre-cleanup identity observation", error)
        for child, row in reversed(list(self.children)):
            try:
                self.stop(child, row)
            except BaseException as error:
                row["cleanupError"] = str(error)
                self.cleanup_error(f"Direct cleanup {row['name']}", error)
        try:
            self.finish_descendants()
        except BaseException as error:
            self.cleanup_error("Final descendant audit", error)
            self.result["ownedSurvivors"] = None
            self.result["descendantQualification"] = "audit unavailable; no zero-survivor claim"
        self.result["allDirectChildrenWaited"] = all(row.get("waited") for row in self.rows)
        self.result["cleanupErrors"] = self.cleanup_errors
        if self.cleanup_errors or not self.result["allDirectChildrenWaited"]:
            self.result["passed"] = False
        self.best_effort_save("Final cleanup receipt")
        for log in self.logs:
            try:
                log.close()
            except BaseException as error:
                self.cleanup_error("Owned log close", error)


class Recording:
    """Read only complete RTR1 records; NOWM begins, not completes, its cycle."""
    def __init__(self, path):
        self.path = path
        self.offset = RTR_FILE_HEADER_BYTES
        self.initialized = False
        self.latest_now = NO_COMPLETE_CYCLE
        self.focus = []
        self.counts = {}
        self.incomplete_tail_bytes = 0

    def poll(self):
        if not self.path.exists():
            return
        with self.path.open("rb") as source:
            if not self.initialized:
                header = source.read(RTR_FILE_HEADER_BYTES)
                if len(header) < RTR_FILE_HEADER_BYTES:
                    return
                if header[:len(RTR_MAGIC)] != RTR_MAGIC or struct.unpack("<I", header[len(RTR_MAGIC):])[0] != RTR_VERSION:
                    raise RuntimeError("Actual recording header/version differs from RTR1 owner")
                self.initialized = True
            source.seek(self.offset)
            used = 0
            while used < MAX_RTR_BYTES_PER_POLL:
                start = source.tell()
                header = source.read(RTR_RECORD_HEADER_BYTES)
                if not header:
                    break
                if len(header) != RTR_RECORD_HEADER_BYTES:
                    break
                cycle, tag, length = struct.unpack("<i4sI", header)
                if length > MAX_RECORD_BYTES:
                    raise RuntimeError("Actual RTR1 record exceeds finite byte bound")
                payload = source.read(length)
                if len(payload) != length:
                    break
                self.offset = source.tell()
                used += self.offset - start
                label = tag.decode("ascii", errors="replace")
                self.counts[label] = self.counts.get(label, 0) + NEXT_CYCLE
                if tag == b"NOWM":
                    if len(payload) != struct.calcsize("<q"):
                        raise RuntimeError("Malformed actual NOWM record")
                    if cycle < self.latest_now:
                        raise RuntimeError("RTR1 NOWM cycle ordering regressed")
                    self.latest_now = cycle
                elif tag == b"FOCS":
                    if len(payload) != NEXT_CYCLE or payload[0] not in (0, NEXT_CYCLE):
                        raise RuntimeError("Malformed actual FOCS record")
                    self.focus.append({"cycle": cycle, "focused": bool(payload[0])})
            self.incomplete_tail_bytes = self.path.stat().st_size - self.offset

    def complete_cycle(self):
        return self.latest_now - NEXT_CYCLE if self.latest_now >= 0 else NO_COMPLETE_CYCLE

    def focused_at(self, cycle):
        # ViewerApp starts focused; explicit FOCS records override that fact.
        focused = True
        for row in self.focus:
            if row["cycle"] > cycle:
                break
            focused = row["focused"]
        # A loss anywhere in this same logic cycle is conservative ambiguity.
        return focused and not any(not row["focused"] and row["cycle"] == cycle for row in self.focus)


def decode_png(path):
    data = path.read_bytes()
    if len(data) > MAX_PNG_BYTES or not data.startswith(PNG_MAGIC):
        raise RuntimeError("Actual readback is not a bounded PNG")
    offset = len(PNG_MAGIC)
    image = bytearray()
    dimensions = None
    ended = False
    while offset + PNG_CHUNK_HEADER_BYTES + PNG_CHUNK_CRC_BYTES <= len(data):
        length, kind = struct.unpack(">I4s", data[offset:offset + PNG_CHUNK_HEADER_BYTES])
        payload_start = offset + PNG_CHUNK_HEADER_BYTES
        payload_end = payload_start + length
        end = payload_end + PNG_CHUNK_CRC_BYTES
        if end > len(data):
            raise RuntimeError("Truncated actual PNG chunk")
        payload = data[payload_start:payload_end]
        crc = struct.unpack(">I", data[payload_end:end])[0]
        if zlib.crc32(kind + payload) & 0xFFFFFFFF != crc:
            raise RuntimeError("Actual PNG chunk CRC mismatch")
        if kind == b"IHDR":
            if dimensions is not None or len(payload) != PNG_HEADER_BYTES:
                raise RuntimeError("Actual PNG IHDR differs")
            width, height, depth, color, compression, filtering, interlace = struct.unpack(">IIBBBBB", payload)
            if not width or not height or width * height > MAX_PNG_PIXELS or (depth, color, compression, filtering, interlace) != (PNG_DEPTH, PNG_RGB_COLOR, 0, 0, 0):
                raise RuntimeError("Actual readback format is not the owning RGB8 PNG contract")
            dimensions = width, height
        elif kind == b"IDAT":
            image.extend(payload)
        elif kind == b"IEND":
            ended = True
            if end != len(data):
                raise RuntimeError("Actual PNG has bytes after IEND")
            break
        offset = end
    if not dimensions or not ended:
        raise RuntimeError("Actual PNG header/end unavailable")
    expected = (dimensions[0] * PNG_RGB_CHANNELS + PNG_FILTER_BYTE) * dimensions[1]
    inflater = zlib.decompressobj()
    decoded = inflater.decompress(image, expected + NEXT_CYCLE)
    if len(decoded) != expected or not inflater.eof or inflater.unused_data or inflater.unconsumed_tail:
        raise RuntimeError("Actual PNG decoded extent differs from its dimensions")
    return {"path": str(path), "sha256": digest(path), "width": dimensions[0],
        "height": dimensions[1], "bytes": len(data), "decodedBytes": len(decoded)}


def screenshots(work, recording):
    log = bounded_text(work / "client.log")
    request_pattern = re.compile(r"\[client910\] screenshot series (.+?) at logic cycle (-?\d+)([^\n]*)")
    written_pattern = re.compile(r"\[client910\] screenshot written: (.+?) \((\d+)x(\d+)\)")
    writes = {match.group(1): (int(match.group(2)), int(match.group(3)))
        for match in written_pattern.finditer(log)}
    result = []
    for request in request_pattern.finditer(log):
        path = Path(request.group(1))
        cycle = int(request.group(2))
        row = {"path": str(path), "requestCycle": cycle,
            "requestContext": request.group(3).strip(),
            "messageBoxRequest": "message box" in request.group(3),
            "completedCycle": recording.complete_cycle(),
            "focusedAtRequestCycle": recording.focused_at(cycle),
            "writeLogged": str(path) in writes, "decoded": None}
        if path.parent != work or not path.name.startswith("barrows_"):
            raise RuntimeError("Actual readback path is outside this fresh recording")
        if str(path) in writes and path.exists():
            row["decoded"] = decode_png(path)
            if (row["decoded"]["width"], row["decoded"]["height"]) != writes[str(path)]:
                raise RuntimeError("Actual readback extent differs from successful native write log")
        row["qualifiedWrittenCycle"] = bool(row["decoded"] and row["focusedAtRequestCycle"]
            and not row["messageBoxRequest"] and recording.complete_cycle() >= cycle)
        result.append(row)
    return result


def observations(journal):
    result = []
    if not journal.exists():
        return result
    with journal.open() as source:
        for line in source:
            if len(line) > MAX_JOURNAL_LINE_BYTES:
                raise RuntimeError("Driver journal line exceeds bound")
            if not line.strip():
                continue
            row = json.loads(line)
            request = row.get("request", {})
            response = row.get("response", {})
            state = response.get("data", {})
            if request.get("command") == "snapshot" and response.get("status") == "observed" and state.get("ready"):
                result.append({"stage": row["stage"], "cycle": state["cycle"],
                    "map": state.get("map"), "focused": state.get("focused"),
                    "player": state.get("player"), "region": state.get("region"),
                    "components": state.get("components")})
    return result


def bracket(shot, states):
    cycle = shot["requestCycle"]
    before = [state for state in states if state["cycle"] <= cycle]
    after = [state for state in states if state["cycle"] >= cycle]
    if not before or not after:
        return None
    lower, upper = before[-1], after[0]
    if not (lower["map"] == upper["map"] and lower["focused"] and upper["focused"]
        and lower["stage"] == upper["stage"] and upper["cycle"] - lower["cycle"] <= MAX_OBSERVATION_BRACKET_CYCLES):
        return None
    return {"before": lower, "after": upper,
        "qualification": "stable installed-state bracket around actual screenshot request; no exact submitted-frame/pixel parity claim"}


def explicit_control(module_path, socket_path, seconds):
    spec = importlib.util.spec_from_file_location("frozen_native_control", module_path)
    if spec is None or spec.loader is None:
        raise RuntimeError("Explicit frozen Control module unavailable")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.Control(str(socket_path), seconds)


def interrupt(_signum, _frame):
    raise KeyboardInterrupt("Owned adaptive recording interrupted")


def finalize_capture(owner, manifest, root, before, old_handlers):
    """Finish independent cleanup, audits and ledger updates despite IO failure."""
    try:
        # A second interruption must not abort the bounded owned finalization.
        for name in old_handlers:
            try:
                signal.signal(name, signal.SIG_IGN)
            except BaseException as error:
                owner.cleanup_error("Ignore interruption during cleanup", error)
        try:
            owner.cleanup()
        except BaseException as error:
            owner.cleanup_error("Unexpected cleanup failure", error)
            # Never let an unexpected outer cleanup failure skip still-owned
            # direct children. This path uses no process query or artifact IO.
            for child, row in reversed(list(owner.children)):
                try:
                    owner.stop(child, row)
                except BaseException as child_error:
                    owner.cleanup_error(f"Fallback direct cleanup {row['name']}", child_error)
            owner.result["allDirectChildrenWaited"] = all(row.get("waited") for row in owner.rows)
            owner.result["ownedSurvivors"] = None
            owner.result["descendantQualification"] = "audit unavailable; no zero-survivor claim"
    finally:
        try:
            if before is not None:
                try:
                    owner.result["generatedInputsAfter"] = generated(root)
                    owner.result["generatedInputsUnchanged"] = before == owner.result["generatedInputsAfter"]
                    if not owner.result["generatedInputsUnchanged"]:
                        owner.result["passed"] = False
                except BaseException as error:
                    owner.result["passed"] = False
                    owner.result["inputAuditError"] = str(error)
            try:
                final_source = {relative: digest(root / relative)
                    for relative in manifest["runtime"]["sourceFiles"]}
                owner.result["sourceFilesAfter"] = final_source
                owner.result["sourceFilesUnchanged"] = final_source == manifest["runtime"]["sourceFiles"]
                owner.result["binaryUnchanged"] = digest(Path(manifest["files"]["client"]["path"])) == manifest["files"]["client"]["sha256"]
                owner.result["frozenFilesAfter"] = {name: digest(Path(entry["path"])) for name, entry in manifest["files"].items()}
                owner.result["frozenFilesUnchanged"] = all(owner.result["frozenFilesAfter"][name] == entry["sha256"]
                    for name, entry in manifest["files"].items())
                if not owner.result["sourceFilesUnchanged"] or not owner.result["binaryUnchanged"] or not owner.result["frozenFilesUnchanged"]:
                    owner.result["passed"] = False
            except BaseException as error:
                owner.result["passed"] = False
                owner.result["sourceAuditError"] = str(error)
        finally:
            try:
                # A real Popen may have been registered before an artifact
                # failure prevented start() from returning its client pair.
                # Update its existing actual launch entry after all waits.
                for actual_client in [(child, row) for child, row in owner.children if row["name"] == "client"]:
                    try:
                        owner.update_launch(*actual_client)
                    except BaseException as error:
                        owner.result["passed"] = False
                        owner.result["launchLedgerFinalizationError"] = str(error)
                owner.result["cleanupErrors"] = owner.cleanup_errors
                owner.best_effort_save("Final capture receipt")
            finally:
                for name, handler in old_handlers.items():
                    try:
                        signal.signal(name, handler)
                    except BaseException as error:
                        owner.cleanup_error("Restore signal handler", error)


def main():
    global PRIVATE_ROOT, LAUNCH_LEDGER
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--diagnostics-root", type=Path, required=True)
    args = parser.parse_args()
    if not args.diagnostics_root.is_absolute() or not args.diagnostics_root.is_dir():
        parser.error("Explicit existing absolute diagnostics root required")
    PRIVATE_ROOT = args.diagnostics_root.resolve()
    LAUNCH_LEDGER = PRIVATE_ROOT / "client-launch-ledger.json"
    manifest = read_json(args.manifest)
    if manifest.get("diagnosticsRoot") != str(PRIVATE_ROOT):
        parser.error("Frozen manifest diagnostics root differs from the explicit owner")
    if manifest.get("format") != MANIFEST_FORMAT or manifest.get("readyForWindow") is not True:
        parser.error("Root's fully frozen current-tree manifest is required; the source template cannot launch")
    output = args.out.resolve()
    if not output.is_relative_to(PRIVATE_ROOT) or output.exists():
        parser.error("Fresh private bosses output is required")
    output.mkdir(parents=True)
    root = Path(manifest["runtime"]["root"])
    owner = Owner(output, root)
    client_pair = None
    before = None
    recording = None
    old_handlers = {name: signal.getsignal(name) for name in (signal.SIGTERM, signal.SIGINT)}
    for name in old_handlers:
        signal.signal(name, interrupt)
    try:
        if not root.is_absolute() or not root.is_dir():
            raise RuntimeError("Explicit runtime checkout unavailable")
        capture_seconds = manifest["seconds"]["capture"]
        driver_seconds = manifest["seconds"]["driver"]
        server_seconds = manifest["seconds"]["serverStart"]
        terminal_seconds = manifest["seconds"]["terminal"]
        if not (0 < driver_seconds < capture_seconds <= MAX_CAPTURE_SECONDS
            and 0 < server_seconds <= MAX_SERVER_START_SECONDS
            and 0 < terminal_seconds <= MAX_TERMINAL_SECONDS):
            raise RuntimeError("Reviewed finite capture budgets are outside policy")
        session_deadline = time.monotonic() + capture_seconds
        required_paths = {name: check_file(entry) for name, entry in manifest["files"].items()}
        if required_paths["launcher"] != Path(__file__).resolve():
            raise RuntimeError("Manifest is for a different launcher")
        approval_path = check_file(manifest["ownerApproval"])
        approval = read_json(approval_path)
        if approval.get("passed") is not True or approval.get("expectedPriorLaunches") != EXPECTED_PRIOR_LAUNCHES:
            raise RuntimeError("Root's exact fourth-launch readiness approval differs")
        approval_expected = {"head": manifest["runtime"]["head"], "tree": manifest["runtime"]["tree"],
            "binarySha256": manifest["files"]["client"]["sha256"],
            "launchLedgerSha256": manifest["ledger"]["sha256"]}
        if any(approval.get(key) != value for key, value in approval_expected.items()):
            raise RuntimeError("Root's readiness approval does not bind the current source/binary/prior ledger")
        if not manifest.get("preflightReceipts"):
            raise RuntimeError("Current-tree recipe/controller/source preflight receipts are required")
        for entry in manifest["preflightReceipts"]:
            check_file(entry)
        for executable in manifest["executables"].values():
            if not Path(executable).is_absolute() or not Path(executable).is_file():
                raise RuntimeError("Frozen executable paths must be absolute existing files")
        if str(LAUNCH_LEDGER) != manifest["ledger"]["path"] or manifest["ledger"]["expectedPrior"] != EXPECTED_PRIOR_LAUNCHES or manifest["ledger"]["maximum"] != MAXIMUM_CLIENT_LAUNCHES:
            raise RuntimeError("Manifest attempts a different cumulative launch ledger/budget")
        prior_native_file(LAUNCH_LEDGER)
        if digest(LAUNCH_LEDGER) != manifest["ledger"]["sha256"]:
            raise RuntimeError("Actual prior-three ledger differs from reviewed evidence")
        prior = read_json(LAUNCH_LEDGER)
        actual_native_prior = qualified_native_prior(manifest, prior)
        for path, expected in manifest["runtime"]["sourceFiles"].items():
            check_file({"path": str(root / path), "sha256": expected})
        before = generated(root)
        if before != manifest["runtime"]["generatedInputs"]:
            raise RuntimeError("Current complete generated input epoch differs")
        environment = dict(os.environ)
        for name in OLD_INPUT_VARIABLES:
            environment.pop(name, None)
        environment.pop("ALTO_NPC_SPAWNS", None)
        environment.update(ALTO_BARROWS_RUNTIME_ROOT=str(root), ALTO_TRACE_INFO="1", ALTO_DEV_NPCS="1")
        for label, arguments, expected in (
            ("git-head", ["git", "rev-parse", "HEAD"], manifest["runtime"]["head"]),
            ("git-tree", ["git", "show", "--format=%T", "--no-patch", "HEAD"], manifest["runtime"]["tree"]),
            ("git-status", ["git", "status", "--porcelain"], ""),
        ):
            if owner.run(label, arguments, root, environment, METADATA_SECONDS).strip() != expected:
                raise RuntimeError(f"Current source checkpoint differs: {label}")
        scratch_root = Path(manifest["scratchRoot"])
        if not scratch_root.is_absolute() or not scratch_root.is_dir():
            raise RuntimeError("Explicit existing short scratch root required")
        scratch = Path(tempfile.mkdtemp(prefix="alto-b4-", dir=scratch_root))
        owner.result.update(scratch=str(scratch), manifest=str(args.manifest.resolve()),
            manifestSha256=digest(args.manifest), preservedPriorLaunches=prior,
            actualNativePriorReceipts=actual_native_prior,
            provenance={"head": manifest["runtime"]["head"], "tree": manifest["runtime"]["tree"],
                "clientSha256": manifest["files"]["client"]["sha256"],
                "sourceSha256": manifest["runtime"]["sourceFiles"], "generatedInputs": before,
                "privateSources": manifest["files"], "ownerApproval": manifest["ownerApproval"]})
        owner.save()
        lobby_port, world_port = manifest["ports"]["lobby"], manifest["ports"]["world"]
        if not (isinstance(lobby_port, int) and isinstance(world_port, int)
            and FIRST_LANE_PORT <= lobby_port < END_LANE_PORT
            and FIRST_LANE_PORT <= world_port < END_LANE_PORT and lobby_port != world_port):
            raise RuntimeError("Reviewed ports are outside the dedicated bosses lane")
        environment.update(ALTO_LOBBY_PORT=str(lobby_port), ALTO_WORLD_PORT=str(world_port),
            ALTO_PLAYER_DATA_DIR=str(scratch / "players"))
        owner.run("seed", [manifest["executables"]["node"], str(required_paths["seed"]), str(scratch),
            str(required_paths["initialPlan"])], root / "server", environment, METADATA_SECONDS)
        plan = read_json(scratch / "plan.json")
        # A new initial account is declared here, before servers/login. No
        # later file write touches players, inventory, HP, coins or progress.
        owner.result["initialFixture"] = {name: {"path": str(scratch / name), "sha256": digest(scratch / name)}
            for name in ("plan.json", "initial-repair-fixture.json")}
        lobby = owner.start("lobby", [manifest["executables"]["node"], "src/lostcity/lobby.ts"],
            root / "server", environment, capture_seconds)
        world = owner.start("world", [manifest["executables"]["node"], str(required_paths["server"]), str(scratch)],
            root / "server", environment, capture_seconds)
        ready_deadline = min(session_deadline, time.monotonic() + server_seconds)
        while not all(f"listening on port {port}" in bounded_text(output / f"{name}.log").lower()
            for name, port in (("lobby", lobby_port), ("world", world_port))):
            if any(child.poll() is not None for child, _ in (lobby, world)) or time.monotonic() >= ready_deadline:
                raise RuntimeError("Ordinary lobby/world readiness failed")
            owner.sample()
            time.sleep(POLL_SECONDS)
        client_directory = scratch / "client"
        client_directory.mkdir()
        socket_path = scratch / "c.sock"
        if len(os.fsencode(socket_path)) >= MAX_CONTROL_SOCKET_PATH_BYTES:
            raise RuntimeError("Owned control socket exceeds the native Unix path bound")
        # Periodic diagnostic requests are independent of gameplay. The
        # unreachable final sentinel keeps --screenshot from auto-exiting.
        shots = list(range(SCREENSHOT_PERIOD_CYCLES, SCREENSHOT_SENTINEL_CYCLE, SCREENSHOT_PERIOD_CYCLES))
        shots.append(SCREENSHOT_SENTINEL_CYCLE)
        client_environment = dict(environment, CLIENT910_RECORD=str(scratch / "raw.rtr"),
            CLIENT910_RECORD_MODE="interactive", CLIENT910_CONTROL=str(socket_path),
            CLIENT910_LOG="info", CLIENT910_WINDOW_SIZE=f"{WINDOW_WIDTH},{WINDOW_HEIGHT}",
            CLIENT910_PREFERENCES_FILE=str(client_directory / "preferences.dat"),
            CLIENT910_VARC_FILE=str(client_directory / "client-vars.dat"),
            CLIENT910_UID192_FILE=str(client_directory / "random.dat"),
            CLIENT910_OUT_TRACE="1", CLIENT910_UI_TRACE_INPUT="1",
            CLIENT910_SCREENSHOT_SERIES=",".join(map(str, shots)))
        # Logs live under the stable private output; screenshots/RTR/cache use
        # the short fresh scratch so the Unix socket stays within OS bounds.
        arguments = [str(required_paths["client"]), "--renderer", "modern", "--direct-login",
            "--lobby-port", str(lobby_port), "--world-port", str(world_port),
            "--username", plan["accountKey"], "--password", "password",
            "--cache-dir", str(client_directory / "cache"), "--screenshot", str(scratch / "barrows.png")]
        def wired_guard():
            owner.run("wired-guard", ["bash", str(required_paths["wiredGuard"])], root, environment, METADATA_SECONDS)
            if time.monotonic() >= session_deadline:
                raise TimeoutError("Whole-capture deadline expired during final wired guard")
        # Actual client Popen happens only inside this guarded ledger lock.
        client_pair = owner.start("client", arguments, root / "tools/client910", client_environment,
            capture_seconds, {"ledgerSha256": manifest["ledger"]["sha256"], "guard": wired_guard})
        client, client_row = client_pair
        # Existing native writers use this log at private output; the scanner
        # below reads that exact file through a harmless scratch symlink.
        (scratch / "client.log").symlink_to(output / "client.log")
        recording = Recording(scratch / "raw.rtr")
        socket_deadline = min(session_deadline, time.monotonic() + server_seconds)
        while not socket_path.exists():
            if client.poll() is not None or time.monotonic() >= socket_deadline:
                raise RuntimeError("Ordinary client control socket did not start")
            recording.poll()
            owner.sample()
            time.sleep(POLL_SECONDS)
        if not stat.S_ISSOCK(socket_path.stat().st_mode):
            raise RuntimeError("Expected fresh control path is not a Unix socket")
        driver_result = output / "driver-result.json"
        journal = output / "driver-journal.jsonl"
        driver_arguments = [manifest["executables"]["python"], str(required_paths["driver"]),
            "--control-module", str(required_paths["control"]), "--socket", str(socket_path),
            "--rules", str(required_paths["barrowsRules"]), "--food-rules", str(required_paths["foodRules"]),
            "--charge-rules", str(required_paths["chargeRules"]), "--plan", str(scratch / "plan.json"),
            "--symbols", str(manifest["symbolsDirectory"]), "--receipts", str(scratch / "combat-receipts.jsonl"),
            "--journal", str(journal), "--result", str(driver_result),
            "--diagnostics-root", str(PRIVATE_ROOT), "--deadline-seconds", str(driver_seconds)]
        driver, driver_row = owner.start("driver", driver_arguments, root, environment, driver_seconds)
        driver_deadline = min(session_deadline, time.monotonic() + driver_seconds)
        next_sample = time.monotonic()
        while driver.poll() is None:
            recording.poll()
            if client.poll() is not None or any(child.poll() is not None for child, _ in (lobby, world)):
                raise RuntimeError("Ordinary client/server ended before adaptive driver completion")
            if time.monotonic() >= driver_deadline:
                raise TimeoutError("Adaptive driver exceeded its finite deadline")
            if time.monotonic() >= next_sample:
                owner.sample()
                next_sample = time.monotonic() + IDENTITY_SAMPLE_SECONDS
                owner.result["readbacksSoFar"] = screenshots(scratch, recording)
                owner.save()
            time.sleep(POLL_SECONDS)
        driver_row.update(exit=driver.wait(timeout=KILL_SECONDS), waited=True)
        owner.save()
        semantic = read_json(driver_result)
        owner.result["semanticResult"] = {"path": str(driver_result), "sha256": digest(driver_result), "value": semantic}
        if driver_row["exit"] or semantic.get("status") != SEMANTIC_STATUS or len(semantic.get("repairs", [])) != 2:
            raise RuntimeError("Full fight and both ordinary durable repairs did not complete")
        completion_cycle = semantic["final_repair_client"]["cycle"]
        owner.result["driverCompletionCycle"] = completion_cycle
        states = observations(journal)
        states.append({"stage": "terminal", **{key: semantic["final_repair_client"].get(key)
            for key in ("cycle", "map", "focused", "player", "region", "components")}})
        terminal_deadline = min(session_deadline, time.monotonic() + terminal_seconds)
        control = explicit_control(required_paths["control"], socket_path, min(terminal_seconds, METADATA_SECONDS))
        terminal = None
        try:
            # Driver has closed its one connection. The launcher now makes
            # observation-only requests; it injects no second action stream.
            while time.monotonic() < terminal_deadline:
                recording.poll()
                response = control.request({"command": "snapshot"}, timeout=min(METADATA_SECONDS, terminal_deadline - time.monotonic()))
                if response.get("status") != "observed":
                    raise RuntimeError("Terminal native observation refused")
                state = response["data"]
                if state.get("ready"):
                    states.append({"stage": "terminal", **{key: state.get(key)
                        for key in ("cycle", "map", "focused", "player", "region", "components")}})
                shots_now = screenshots(scratch, recording)
                for shot in shots_now:
                    if shot["qualifiedWrittenCycle"] and shot["requestCycle"] > completion_cycle:
                        shot["stateBracket"] = bracket(shot, states)
                        if shot["stateBracket"] is not None:
                            terminal = shot
                            break
                if terminal is not None:
                    break
                if client.poll() is not None:
                    raise RuntimeError("Client ended before an actual post-completion readback")
                owner.sample()
                time.sleep(POLL_SECONDS)
        finally:
            control.close()
        if terminal is None:
            raise RuntimeError("No successful focused native PNG after actual driver completion")
        owner.result["terminalScreenshot"] = terminal
        # Finish through the existing app SIGTERM shutdown owner. No new close
        # command, GPU switch, guessed key or window automation is introduced.
        owner.stop(client, client_row)
        if client_row.get("exit") != 0 or client_row.get("termination", "").endswith("SIGKILL"):
            raise RuntimeError("Ordinary client did not finish through graceful owned shutdown")
        recording.poll()
        if recording.incomplete_tail_bytes or recording.complete_cycle() < terminal["requestCycle"]:
            raise RuntimeError("Final RTR1 is truncated or lacks the terminal complete-cycle prefix")
        if not recording.counts.get("UIEV"):
            raise RuntimeError("Canonical accepted client input events are absent from the actual recording")
        owner.result["recording"] = {"path": str(recording.path), "sha256": digest(recording.path),
            "completeCycle": recording.complete_cycle(), "latestBegunCycle": recording.latest_now,
            "incompleteTailBytes": recording.incomplete_tail_bytes, "tagCounts": recording.counts,
            "focusEvents": recording.focus, "qualification": "complete NOWM prefix; owning native Replay still required"}
        qualified = []
        for shot in screenshots(scratch, recording):
            shot["stateBracket"] = bracket(shot, states)
            if shot["qualifiedWrittenCycle"] and shot["stateBracket"] is not None:
                qualified.append(shot)
        owner.result["qualifiedReadbacks"] = qualified
        owner.result["focusQualification"] = {"continuousFocused": not any(not row["focused"] for row in recording.focus),
            "scope": "actual FOCS plus stable installed-state brackets; PNG visual review remains required"}
        if (scratch / "passive-observer-errors.json").exists():
            raise RuntimeError("Passive observer errors are preserved; semantic proof cannot pass")
        world_log = bounded_text(output / "world.log")
        if "[WORLD]: tick step" in world_log or "step failed" in world_log:
            raise RuntimeError("Ordinary world step failed during recording")
        client_log = bounded_text(output / "client.log")
        if any(int(match.group(1)) for match in re.finditer(r"ui diagnostics: hooks \d+ failures (\d+)", client_log)):
            raise RuntimeError("Actual native UI hook failures occurred")
        # Freeze passive writers before extracting the finite recorded prefix.
        # World/lobby termination is ordinary shutdown, not a gameplay action.
        owner.stop(*world)
        owner.stop(*lobby)
        processed = scratch / "processed"
        processed.mkdir()
        owner.run("postprocess", [manifest["executables"]["python"], str(required_paths["postprocess"]),
            "--until", str(recording.complete_cycle() + NEXT_CYCLE), str(recording.path),
            str(output / "world.log"), str(processed)], root, environment, METADATA_SECONDS)
        for name in ("plan.json", "initial-repair-fixture.json", "combat-receipts.jsonl"):
            shutil.copyfile(scratch / name, processed / name)
        # Actual captured observations only; generated rule tables stay private.
        shutil.copyfile(driver_result, processed / "driver-result.json")
        shutil.copyfile(journal, processed / "driver-journal.jsonl")
        account_key = read_json(scratch / "plan.json")["accountKey"]
        account = scratch / "players/accounts" / (hashlib.sha256(account_key.encode("utf8")).hexdigest() + ".json")
        saved = read_json(account)
        # Record only the actual observed gameplay fields; raw player files,
        # credentials and unrelated saved account state remain private.
        write_json(processed / "final-saved-state.json", {key: saved[key] for key in
            ("backpack", "worn", "coins", "barrows", "house", "revision")})
        owner.result["closedSavedState"] = {"privateAccount": str(account),
            "privateAccountSha256": digest(account), "projection": str(processed / "final-saved-state.json"),
            "projectionSha256": digest(processed / "final-saved-state.json")}
        owner.result["evidenceFiles"] = {str(path): digest(path) for path in (
            scratch / "raw.rtr", scratch / "combat-receipts.jsonl", scratch / "plan.json",
            scratch / "initial-repair-fixture.json", output / "driver-result.json",
            output / "driver-journal.jsonl", output / "client.log", output / "world.log")}
        owner.result.update(passed=True, captureQualification="semantic/native recording captured; pending independent owning replay and visual review",
            processed=str(processed), limitations=["No pixel parity or FPS claim", "No screen body-pick claim from semantic menu operations",
                "Focus losses, if any, remain explicit", "Stable snapshot brackets do not identify an immutable GPU submitted frame",
                "Root must view crypt/Bob/POH/terminal PNGs and run the owning native Replay"])
    except BaseException as error:
        owner.result["error"] = str(error)
    finally:
        finalize_capture(owner, manifest, root, before, old_handlers)
        print(json.dumps({"passed": owner.result["passed"], "output": str(output),
            "scratch": owner.result.get("scratch"), "allDirectChildrenWaited": owner.result.get("allDirectChildrenWaited"),
            "ownedSurvivors": owner.result.get("ownedSurvivors"), "error": owner.result.get("error")}, sort_keys=True), flush=True)
    return 0 if owner.result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
