"""Exact source bindings and ordinary authenticated-headless GWD receipt audit."""
import hashlib
import importlib.util
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPOSITORY_ROOT = HERE.parents[4]
OWNER_PATH = HERE.parent / "barrows" / "record.py"
ENTRY = "app::session_replay::observed_session::authenticated_observed_session"
FACTIONS = ("bandos", "armadyl", "saradomin", "zamorak")
ACCOUNT = "alice"
LOGIN_NAME = "Alice"
MAX_NATIVE = 4
EXPECTED_NATIVE = 0
FORMAT = 1
DRIVER_SECONDS = 1680
SESSION_SECONDS = 1800
START_SECONDS = 90
METADATA_SECONDS = 30
ACTION_SECONDS = 40
FIGHT_SECONDS = 300
SOCKET_SECONDS = 10
POLL_SECONDS = 0.2
SAMPLE_SECONDS = 2
MAX_SOCKET_BYTES = 104
FIRST_PORT = 48800
END_PORT = 48900
MAX_JSON_BYTES = 32 * 1024 * 1024
MAX_RECEIPT_BYTES = 256 * 1024 * 1024
MAX_PASSIVE_RECEIPT_BYTES = 512 * 1024 * 1024
MAX_LINE_BYTES = 2 * 1024 * 1024
MAX_RECEIPT_ROWS = 200000
HASH_BLOCK_BYTES = 1024 * 1024
FIRST = 0
ONE = 1
NO_CYCLE = 0
MAX_WIRE_BYTE = 255
REQUIRED_ALTAR_EXITS = 2
HEADLESS_ROUTE_STATUS = "ordinary_headless_four_role_route_pass_requires_native_proof"
HEADLESS_EXECUTION_MODE = "headless_gwd_faction_route"
OBSERVED_BACKEND = "authenticated_headless"
MONOTONIC_CLOCK_FORMAT = "scoped-v1"
FEATURES = {"authenticated-observed-session", "ground-take", "client-domain-vars",
    "component-parameters", "walltime-request-id", "finite-session-budget",
    "authenticated-session-rtr", "scoped-monotonic-clock-replay"}
FORBIDDEN_ENV = {"ALTO_NPC_SPAWNS", "ALTO_THIEVING_ROLLS", "ALTO_PREFLIGHT_LOGIN_MODULE",
    "ALTO_BARROWS_RUNTIME_ROOT", "ALTO_PEST_RUNTIME_ROOT"}


def sha(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as source:
        for block in iter(lambda: source.read(HASH_BLOCK_BYTES), b""):
            result.update(block)
    return result.hexdigest()


def binding(path):
    path = Path(path).absolute()
    if not path.is_file():
        raise RuntimeError("Required source/evidence file unavailable: " + str(path))
    return {"path": str(path), "sha256": sha(path)}


def checked(entry):
    path = Path(entry["path"])
    if not path.is_absolute() or binding(path) != entry:
        raise RuntimeError("Frozen binding changed: " + str(path))
    return path


def read_json(path):
    path = Path(path)
    if path.stat().st_size > MAX_JSON_BYTES:
        raise RuntimeError("Finite JSON bound exceeded: " + str(path))
    return json.loads(path.read_text())


def write_json(path, value):
    # Imported only by the future leased freezer/runner. Original failed roots
    # are never reused, overwritten or deleted.
    import os
    path = Path(path)
    temporary = path.with_name(path.name + ".writing")
    with temporary.open("x", encoding="utf8") as stream:
        json.dump(value, stream, sort_keys=True, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)


def load_owner(entry):
    owner_path = checked(entry)
    if owner_path.resolve() != OWNER_PATH.resolve():
        raise RuntimeError("Manifest must bind the committed Barrows process owner")
    spec = importlib.util.spec_from_file_location("gwd_qualified_process_owner", owner_path)
    if spec is None or spec.loader is None:
        raise RuntimeError("Qualified Owner import unavailable")
    owner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(owner)
    return owner


def files_under(root, suffix=None):
    root = Path(root)
    return {str(path.relative_to(root)): sha(path) for path in sorted(root.rglob("*"))
        if path.is_file() and (suffix is None or path.suffix == suffix)}


def frozen_map(root, values):
    root = Path(root)
    for relative, expected in values.items():
        path = Path(relative)
        if path.is_absolute() or ".." in path.parts or sha(root / path) != expected:
            raise RuntimeError("Frozen source/input changed: " + str(relative))


def ledger_bytes(entry):
    path = checked(entry)
    raw = path.read_bytes()
    if json.loads(raw) != []:
        raise RuntimeError("GWD native ledger must still be actual zero; no foreign ledger admitted")
    return raw


def prerequisite(entry, runtime, binary):
    receipt = read_json(checked(entry))
    if receipt.get("passed") is not True or receipt.get("head") != runtime["head"] \
            or receipt.get("tree") != runtime["tree"] or receipt.get("binary") != binary \
            or receipt.get("allDirectChildrenWaited") is not True or receipt.get("ownedSurvivors") != [] \
            or not FEATURES.issubset(set(receipt.get("features", []))):
        raise RuntimeError("Actual current combined-control qualification is incomplete")
    evidence = receipt.get("evidence", [])
    if not evidence:
        raise RuntimeError("Prerequisite summary must bind original build/owning-test/landing receipts")
    for actual in evidence:
        checked(actual)
    return receipt


class BackendRows:
    """Ordering in the real broker stream; transport batches are never clocks."""
    def __init__(self, path):
        self.path = Path(path)
        self.identity = None
        self.offset = FIRST
        self.incomplete = FIRST
        self.rows = FIRST
        self.source = self.transport = None
        self.login = self.boot = self.closed = None
        self.received = self.inputs = self.written = self.cycles = FIRST
        self.next_sequence = FIRST
        self.cycle = None
        self.pending_input_cycle = None
        self.current_input = False
        self.current_written = False
        self.packet_names = set()
        self.input_cycles = []
        self.written_cycles = []

    @staticmethod
    def integer(value):
        return isinstance(value, int) and not isinstance(value, bool)

    def accept(self, row):
        if not isinstance(row, dict) or row.get("account") not in (None, ACCOUNT):
            raise RuntimeError("Malformed/foreign backend row")
        kind = row.get("kind")
        if self.closed is not None:
            raise RuntimeError("Backend receipt after terminal closed row")
        if kind == "headless-source-manifest":
            if self.rows != ONE:
                raise RuntimeError("Source receipt is not first")
            self.source = row
        elif kind == "peer-transport-listening":
            peer = (self.source or {}).get("spec", {}).get("peer", {})
            if self.source is None or self.transport is not None or self.login is not None \
                    or row.get("account") != ACCOUNT or row.get("rendered") is not False \
                    or row.get("transport") != peer.get("transport") \
                    or row.get("control") != peer.get("control") \
                    or not all(isinstance(peer.get(key), str) and Path(peer[key]).is_absolute()
                        for key in ("transport", "control")):
                raise RuntimeError("Exact one owned transport must listen before actual login")
            self.transport = row
        elif kind == "actual-peer-login":
            if self.transport is None or self.login is not None or self.boot is not None or row.get("account") != ACCOUNT \
                    or (row.get("profile") or {}).get("username") != LOGIN_NAME:
                raise RuntimeError("One fresh actual Alice login must precede boot")
            self.login = row
        elif kind == "booted":
            if self.login is None or self.boot is not None or row.get("account") != ACCOUNT \
                    or row.get("rendered") is not False or row.get("virtual_headless") is not True \
                    or row.get("session_seconds") != SESSION_SECONDS:
                raise RuntimeError("Current finite ordinary core boot identity differs")
            self.boot = row
        elif kind == "actual-peer-received":
            if self.boot is None or row.get("account") != ACCOUNT \
                    or row.get("sequence") != self.next_sequence or not isinstance(row.get("frames"), list):
                raise RuntimeError("Fresh server batch sequence differs")
            self.next_sequence += ONE
            self.received += ONE
        elif kind == "input":
            cycle = row.get("cycle")
            expected = self.cycle + ONE if self.cycle is not None else cycle
            if self.boot is None or row.get("account") != ACCOUNT or not self.integer(cycle) \
                    or cycle != expected or self.pending_input_cycle is not None \
                    or (row.get("outcome") or {}).get("status") != "applied" \
                    or not isinstance(row.get("event"), list) or not row["event"]:
                raise RuntimeError("Applied retained input must precede its cycle-tail receipt")
            self.pending_input_cycle = cycle
            self.input_cycles.append(cycle)
            self.inputs += ONE
        elif kind == "logic_cycle":
            cycle = row.get("cycle")
            if self.boot is None or row.get("account") != ACCOUNT or not self.integer(cycle) \
                    or (self.cycle is not None and cycle != self.cycle + ONE) \
                    or row.get("rendered") is not False or not isinstance(row.get("read_wire"), list) \
                    or not all(self.integer(value) and FIRST <= value <= MAX_WIRE_BYTE for value in row["read_wire"]) \
                    or self.pending_input_cycle not in (None, cycle):
                raise RuntimeError("Actual normal logic-cycle receipt order differs")
            self.current_input = self.pending_input_cycle == cycle
            self.pending_input_cycle = None
            self.current_written = False
            self.cycle = cycle
            self.cycles += ONE
        elif kind == "written":
            cycle = row.get("cycle")
            if self.login is None or not isinstance(row.get("wire"), list) \
                    or not all(self.integer(value) and FIRST <= value <= MAX_WIRE_BYTE for value in row["wire"]):
                raise RuntimeError("Original ordinary output wire is malformed")
            if self.boot is None:
                if cycle != NO_CYCLE:
                    raise RuntimeError("Startup output must precede boot with actual no-cycle mark")
            elif cycle != self.cycle or self.current_written or self.pending_input_cycle is not None:
                raise RuntimeError("Written output must follow its exact logic cycle once")
            else:
                self.current_written = True
                frames = row.get("frames")
                if not isinstance(frames, list) or not all(isinstance(name, str) for name in frames):
                    raise RuntimeError("Decoded owning client-frame names absent")
                if "CLIENT_CHEAT" in frames:
                    raise RuntimeError("No console/cheat action is admitted")
                self.packet_names.update(frames)
                self.written_cycles.append(cycle)
                self.written += ONE
        elif kind == "peer-finally-closed":
            promises = row.get("socketPromises") or {}
            if self.boot is None or row.get("account") != ACCOUNT or row.get("rendered") is not False \
                    or row.get("closed") is not True or row.get("errors") != [] \
                    or promises.get("pending") != [] or self.pending_input_cycle is not None:
                raise RuntimeError("Normal peer finally close/settled promises are unqualified")
            self.closed = row
        else:
            raise RuntimeError("Unrecognized backend receipt: " + str(kind))

    def poll(self):
        if not self.path.exists():
            return
        info = self.path.stat()
        identity = info.st_dev, info.st_ino
        if (self.identity is not None and identity != self.identity) or info.st_size < self.offset \
                or info.st_size > MAX_RECEIPT_BYTES:
            raise RuntimeError("Backend file replaced/truncated/over bound")
        self.identity = identity
        with self.path.open("rb") as source:
            source.seek(self.offset)
            while True:
                start = source.tell()
                line = source.readline(MAX_LINE_BYTES + ONE)
                if not line:
                    self.incomplete = FIRST
                    break
                if len(line) > MAX_LINE_BYTES:
                    raise RuntimeError("Backend row too large")
                if not line.endswith(b"\n"):
                    self.incomplete = len(line)
                    break
                self.rows += ONE
                if self.rows > MAX_RECEIPT_ROWS:
                    raise RuntimeError("Backend row count exceeds finite bound")
                self.accept(json.loads(line))
                self.offset = source.tell()

    def summary(self):
        if self.closed is None or self.incomplete or not all((self.received, self.inputs, self.written, self.cycles)):
            raise RuntimeError("Incomplete ordinary backend proof")
        if not any(cycle > min(self.input_cycles) for cycle in self.written_cycles):
            raise RuntimeError("No ordinary written output after retained input")
        return {"login": self.login, "boot": self.boot, "closed": self.closed,
            "received": self.received, "inputs": self.inputs, "written": self.written,
            "logicCycles": self.cycles, "lastLogicCycle": self.cycle,
            "packetNames": sorted(self.packet_names), "ordering": "actual broker/core rows; no server-batch logic clock"}


def journal_rows(path, maximum_bytes=MAX_RECEIPT_BYTES):
    path = Path(path)
    if path.stat().st_size > maximum_bytes:
        raise RuntimeError("Gameplay journal exceeds finite bound")
    rows = []
    with path.open("rb") as source:
        for line in source:
            if len(line) > MAX_LINE_BYTES or not line.endswith(b"\n") or len(rows) >= MAX_RECEIPT_ROWS:
                raise RuntimeError("Gameplay journal malformed/truncated/over bound")
            rows.append(json.loads(line))
    return rows


def semantic_journal(path, rules, plan):
    rows = journal_rows(path)
    completed = [row for row in rows if row.get("kind") == "complete"]
    if len(completed) != ONE or rows[-ONE] is not completed[FIRST] \
            or any(row.get("kind") == "failure" for row in rows):
        raise RuntimeError("One untruncated normal gameplay completion is required")
    final = completed[FIRST]
    arena = next(row for row in rules["arenas"] if row["id"] == plan["arenaId"])
    deaths = [row for row in rows if row.get("kind") == "credited_death"]
    by_name = {row["name"]: row for row in deaths}
    if len(by_name) != len(deaths):
        raise RuntimeError("Duplicate claimed credited life")
    for role in arena["roles"]:
        row = by_name.get(role["title"])
        target = (row or {}).get("death", {}).get("target", {})
        source = (row or {}).get("death", {}).get("source", {})
        full = (row or {}).get("full_health", {})
        if target.get("definition") != role["type"] or source.get("kind") != "player" \
                or full.get("hitpoints") != role["profile"]["hitpoints"] \
                or full.get("id") != target.get("id") or full.get("generation") != target.get("generation"):
            raise RuntimeError("Actual full-health role/credited generation proof absent: " + role["title"])
    if final.get("ordinaryRandom") is not True or final.get("faction") != plan["faction"]:
        raise RuntimeError("Faction/normal-RNG declaration differs")
    kinds = [row.get("kind") for row in rows]
    if kinds.count("taken") != ONE or kinds.count("altar_restored") != ONE \
            or kinds.count("altar_exit") != REQUIRED_ALTAR_EXITS or kinds.count("camp_rejoined") != ONE \
            or kinds.count("arena_rejoined") != ONE:
        raise RuntimeError("Real Take/altar/exit/rejoin receipt set incomplete")
    rejoin = final.get("rejoin") or {}
    earned = final.get("rejoin_kills") or []
    if not earned or len(earned) != rejoin.get("normal_credited_kills") \
            or rejoin.get("debit") != rules["entry"]["arenaDebit"]["value"] \
            or rejoin.get("before_debit", FIRST) < rules["entry"]["lobbyMinimum"]["value"] \
            or rejoin.get("after_debit") != rejoin["before_debit"] - rejoin["debit"] \
            or (rejoin.get("passive") or {}).get("membership") is not True:
        raise RuntimeError("Normal earned-count second arena admission is incomplete")
    for kill in earned:
        if kill.get("after", FIRST) < kill.get("before", FIRST) + ONE \
                or (kill.get("death") or {}).get("source", {}).get("kind") != "player":
            raise RuntimeError("Rejoin count was not earned by an ordinary credited death")
    rope_earned = [row for row in rows if row.get("kind") == "rejoin_count_earned" and row.get("name") == "exit-camp:1"]
    if len(rope_earned) != ONE or rope_earned[FIRST].get("before") != FIRST \
            or rope_earned[FIRST].get("after") != ONE \
            or rope_earned[FIRST].get("death") != by_name.get("exit-camp:1", {}).get("death"):
        raise RuntimeError("One real additional camp death must earn the positive rope witness")
    exits = [row for row in rows if row.get("kind") == "altar_exit"]
    if rope_earned[FIRST]["state"]["cycle"] <= exits[-ONE]["state"]["cycle"]:
        raise RuntimeError("Positive rope witness must follow the second actual altar exit")
    crossings = [row for row in rows if row.get("kind") == "crossing"]
    sequence = plan["route"]["camp"] + [next(row["id"] for row in rules["crossings"]
        if row.get("faction") == plan["faction"] and row["stage"] == stage) for stage in ("lobby", "arena")]
    lobby_id = next(row["id"] for row in rules["crossings"]
        if row.get("faction") == plan["faction"] and row["stage"] == "lobby")
    expected = sequence + [lobby_id] + plan["route"]["campReturn"] + sequence \
        + plan["route"]["campReturn"] + plan["route"]["camp"] \
        + plan["route"]["campReturn"] + [next(row["id"] for row in rules["crossings"] if row["stage"] == "exit")]
    if [row["id"] for row in crossings] != expected \
            or [row["admitted"] for row in crossings].count(False) != ONE \
            or crossings[len(sequence)].get("admitted") is not False:
        raise RuntimeError("Actual crossing/low-count-refusal/rejoin/rope route differs")
    final_bits = {row["id"]: row.get("value") for row in final["state"].get("varbits", [])}
    if any(final_bits.get(row["varbit"]) != FIRST for row in rules["counts"]):
        raise RuntimeError("Final native rope exit did not reset every faction count")
    restore = next(row for row in rows if row.get("kind") == "altar_restored")
    passive = restore.get("passive") or {}
    if passive.get("prayerFine") != restore.get("expected") \
            or passive.get("prayerAllowanceFine") != restore.get("allowance") \
            or restore.get("expected") != passive.get("maximumPrayerFine", FIRST) + restore.get("allowance", FIRST):
        raise RuntimeError("Ordinary altar Prayer/allowance receipt differs")
    take = next(row for row in rows if row.get("kind") == "taken")
    if take.get("after_count") != take.get("before_count", FIRST) + take.get("observed", {}).get("count", FIRST):
        raise RuntimeError("Actual Take backpack delta differs")
    ammo = plan.get("ammunition")
    if ammo is not None:
        inventories = [row.get("value") for row in final["state"].get("inventories", [])]
        if not inventories or any(not row or row.get("truncated")
                or len(row["items"]) != len(row["counts"]) for row in inventories):
            raise RuntimeError("Actual final ranged inventory publication incomplete")
        native_count = sum(count for row in inventories for item, count in zip(row["items"], row["counts"])
            if item == ammo["item"])
        passive_count = sum(slot[ONE] for container in ("backpack", "worn")
            for slot in final["passive"][container] if slot and slot[FIRST] == ammo["item"])
        if not FIRST <= native_count < ammo["count"] or native_count != passive_count:
            raise RuntimeError("Actual ranged shots did not consume/publish declared compatible ammunition")
    return {"status": "ordinary_headless_four_role_route_pass_requires_native_proof",
        "faction": plan["faction"], "roleDefinitions": [row["type"] for row in arena["roles"]],
        "rejoinKills": len(earned), "rejoin": rejoin, "final": final,
        "journal": binding(path), "qualification": "no renderer/foreground/pixel/FPS/surface-quest claim"}
