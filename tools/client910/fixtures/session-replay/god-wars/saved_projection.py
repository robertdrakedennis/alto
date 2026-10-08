"""Read-only projection of existing declared/saved GWD gameplay fields.

This never writes an account or gameplay resource. The recorder owns closure;
the driver uses the same projection to observe its positive pre-rope save.
"""
import hashlib
import json
import os
from pathlib import Path

MEBIBYTE_BYTES = 1024 * 1024
MAX_ACCOUNT_MEBIBYTES = 32
MAX_ACCOUNT_BYTES = MAX_ACCOUNT_MEBIBYTES * MEBIBYTE_BYTES
SHA256_HEX_BYTES = 64
FIRST_SLOT = 0
QUANTITY_SLOT = 1
MIN_SLOT_FIELDS = 2
MAX_SLOT_FIELDS = 3
NO_VALUE = 0
ONE_VALUE = 1
NATIVE_PRAYER_BITS = 14
SAVED_INTEGER_BITS = 32
LAST_INTEGER_BIT = SAVED_INTEGER_BITS - ONE_VALUE
JSON_INDENT = 2
MAX_SAVED_INTEGER = (ONE_VALUE << LAST_INTEGER_BIT) - ONE_VALUE
MIN_SAVED_INTEGER = -(ONE_VALUE << LAST_INTEGER_BIT)
MAX_NATIVE_PRAYER_FINE = (ONE_VALUE << NATIVE_PRAYER_BITS) - ONE_VALUE
SAVED_FIELDS = ("savedVarps", "backpack", "worn", "resources", "revision")
INITIAL_ACCOUNT_FIELDS = ("version", "members", "x", "z", "level", "skills", "backpack", "worn", "savedVarps")
INPUT_FIELDS = ("instances/gwd.json", "combat/rules.json", "food/foods.json", "slayer/rules.json", "combat/charges.json")
SARA_SPECIAL_INPUT = "combat/specials.json"
SARA_RUN_INPUT = "combat/run-energy.json"
RUN_MAXIMUM_FINE = 10000


def integer(value, label, minimum=NO_VALUE):
    if type(value) is not int or value < minimum:
        raise RuntimeError("Invalid existing saved field: " + label)
    return value


def sha256(value):
    if not isinstance(value, str) or len(value) != SHA256_HEX_BYTES \
            or any(character not in "0123456789abcdef" for character in value):
        raise RuntimeError("Invalid source SHA256")
    return value


def rows(value, label):
    if not isinstance(value, list):
        raise RuntimeError("Missing existing saved rows: " + label)
    for row in value:
        if not isinstance(row, list) or not MIN_SLOT_FIELDS <= len(row) <= MAX_SLOT_FIELDS:
            raise RuntimeError("Invalid existing saved row: " + label)
        integer(row[FIRST_SLOT], label + " identity", minimum=-ONE_VALUE)
        integer(row[QUANTITY_SLOT], label + " count")
    return value


def backing_rows(value):
    if not isinstance(value, list):
        raise RuntimeError("Missing savedVarps")
    seen = set()
    for row in value:
        if not isinstance(row, list) or len(row) != MIN_SLOT_FIELDS:
            raise RuntimeError("Invalid savedVarps row")
        identity = integer(row[FIRST_SLOT], "savedVarps identity")
        if identity in seen or type(row[QUANTITY_SLOT]) is not int \
                or not MIN_SAVED_INTEGER <= row[QUANTITY_SLOT] <= MAX_SAVED_INTEGER:
            raise RuntimeError("Duplicate or invalid savedVarps backing")
        seen.add(identity)
    return value


def declared_account_path(work, fixture):
    work = Path(work)
    if not work.is_absolute() or not work.is_dir() or work.resolve() != work:
        raise RuntimeError("Existing absolute owned scratch directory required")
    account_key = fixture["plan"]["accountKey"]
    if not isinstance(account_key, str) or not account_key \
            or fixture["account"].get("accountKey") != account_key:
        raise RuntimeError("Declared pre-login account identity differs")
    expected = work / "players" / "accounts" / (hashlib.sha256(account_key.encode("utf8")).hexdigest() + ".json")
    actual = Path(fixture["accountPath"])
    if actual != expected or actual.resolve() != expected or not actual.is_file():
        raise RuntimeError("Saved account is not the exact declared owned scratch file")
    return actual, account_key


def read_saved_projection(account_path, account_key):
    path = Path(account_path)
    if not path.is_absolute() or not path.is_file() or path.resolve() != path \
            or path.stat().st_size > MAX_ACCOUNT_BYTES:
        raise RuntimeError("Finite existing owned account path required")
    raw = path.read_bytes()
    if len(raw) > MAX_ACCOUNT_BYTES:
        raise RuntimeError("Existing account grew beyond finite bound")
    account = json.loads(raw)
    if account.get("accountKey") != account_key:
        raise RuntimeError("Projection belongs to a different declared account")
    projection = {key: account[key] for key in SAVED_FIELDS}
    if "run" in account:
        projection["run"] = account["run"]
    validate_projection(projection)
    return projection, hashlib.sha256(raw).hexdigest()


def validate_projection(projection):
    if not isinstance(projection, dict) or set(projection) not in (set(SAVED_FIELDS), set(SAVED_FIELDS) | {"run"}):
        raise RuntimeError("Exact existing saved gameplay projection required")
    backing_rows(projection["savedVarps"])
    rows(projection["backpack"], "backpack")
    rows(projection["worn"], "worn")
    resources = projection["resources"]
    if not isinstance(resources, dict) or any(key not in ("life", "prayerFine", "prayerOverboostFine") for key in resources):
        raise RuntimeError("Existing resource projection has unexpected keys")
    integer(resources["life"], "life")
    prayer = integer(resources["prayerFine"], "prayerFine")
    allowance = integer(resources.get("prayerOverboostFine", NO_VALUE), "prayerOverboostFine")
    if prayer > MAX_NATIVE_PRAYER_FINE or allowance > MAX_NATIVE_PRAYER_FINE:
        raise RuntimeError("Saved prayer exceeds existing 14-bit publication")
    integer(projection["revision"], "revision")
    if "run" in projection:
        run = projection["run"]
        if not isinstance(run, dict) or set(run) != {"enabled", "energyFine"} \
                or type(run["enabled"]) is not bool \
                or integer(run["energyFine"], "run energy") > RUN_MAXIMUM_FINE \
                or (run["enabled"] and run["energyFine"] == NO_VALUE):
            raise RuntimeError("Existing saved Run balance is invalid")
    return projection


def project_initial_fixture(value):
    if not isinstance(value, dict) or not isinstance(value.get("account"), dict):
        raise RuntimeError("Existing declared initial fixture required")
    account = {key: value["account"][key] for key in INITIAL_ACCOUNT_FIELDS}
    rows(account["backpack"], "initial backpack")
    rows(account["worn"], "initial worn")
    backing_rows(account["savedVarps"])
    inputs = value["source"]["inputs"]
    special = value["plan"].get("saraSpecial")
    kite = value["plan"].get("saraKite")
    if special is not None and kite is not None:
        raise RuntimeError("Mutually exclusive declared Sara strategies required")
    expected_inputs = INPUT_FIELDS
    if special is not None:
        if value["plan"].get("faction") != "saradomin" or not isinstance(special, dict) \
                or special.get("source", {}).get("input") != SARA_SPECIAL_INPUT \
                or sha256(special["source"]["sha256"]) != inputs.get(SARA_SPECIAL_INPUT):
            raise RuntimeError("Declared Sara special input differs from its captured rule epoch")
        expected_inputs += (SARA_SPECIAL_INPUT,)
    elif kite is not None:
        if value["plan"].get("faction") != "saradomin" or not isinstance(kite, dict) \
                or kite.get("source", {}).get("input") != SARA_RUN_INPUT \
                or sha256(kite["source"]["sha256"]) != inputs.get(SARA_RUN_INPUT):
            raise RuntimeError("Declared Sara Run input differs from its captured rule epoch")
        expected_inputs += (SARA_RUN_INPUT,)
    elif value["plan"].get("faction") == "saradomin":
        raise RuntimeError("Sara requires its explicitly declared ordinary strategy input")
    if set(inputs) != set(expected_inputs):
        raise RuntimeError("Declared GWD initial input epoch differs")
    # No credentials, private path, account filename or unrelated account fields.
    return {"accountSha256": sha256(value["accountSha256"]), "account": account,
            "plan": value["plan"], "source": {"seedSha256": sha256(value["source"]["seedSha256"]),
            "inputs": {key: sha256(inputs[key]) for key in expected_inputs}}}


def saved_count(projection, rule):
    values = dict(backing_rows(projection["savedVarps"]))
    backing = integer(rule["backing"], "generated count backing")
    start = integer(rule["start"], "generated count start")
    end = integer(rule["end"], "generated count end")
    if end < start or end > LAST_INTEGER_BIT or backing not in values:
        raise RuntimeError("Generated count does not identify a saved native backing")
    mask = (ONE_VALUE << (end - start + ONE_VALUE)) - ONE_VALUE
    return (values[backing] >> start) & mask


def write_new_projection(destination, projection):
    destination = Path(destination)
    if destination.exists():
        raise RuntimeError("Existing evidence projection is immutable")
    temporary = destination.with_name(destination.name + ".writing")
    with temporary.open("x", encoding="utf8") as output:
        json.dump(projection, output, indent=JSON_INDENT, sort_keys=True)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    # Atomic, exclusive publication: a concurrent evidence file is never
    # overwritten. A failed publication leaves its bounded partial evidence.
    os.link(temporary, destination)
    temporary.unlink()


def project_closed_recording(work, capture, processed, children, rules):
    """GWD-only postprocessing; call after ordinary World/lobby final waits.

    The unchanged process owner supplies its real child ledger. This function
    starts/signals no process and writes only fresh, public-safe evidence files.
    """
    for name in ("world", "lobby"):
        matching = [row for row in children if row.get("name") == name]
        if len(matching) != ONE_VALUE or matching[NO_VALUE].get("waited") is not True \
                or type(matching[NO_VALUE].get("exit")) is not int \
                or not isinstance(matching[NO_VALUE].get("startIdentity"), str) \
                or not matching[NO_VALUE]["startIdentity"]:
            raise RuntimeError("Exact owned server identity and final wait required: " + name)
    work, capture, processed = Path(work), Path(capture), Path(processed)
    if not processed.is_absolute() or not processed.is_dir() or processed.resolve() != processed:
        raise RuntimeError("Fresh owned processed evidence directory required")
    fixture = json.loads((work / "initial-fixture.json").read_bytes())
    account_path, account_key = declared_account_path(work, fixture)
    saved, raw_sha = read_saved_projection(account_path, account_key)
    before = validate_projection(json.loads((capture / "rope-before-saved-state.json").read_bytes()))
    count_rules = [row for row in rules["counts"] if row["faction"] == fixture["plan"]["faction"]]
    if len(count_rules) != ONE_VALUE or saved_count(before, count_rules[NO_VALUE]) != ONE_VALUE:
        raise RuntimeError("Actual positive pre-rope save is required")
    if any(saved_count(saved, row) != NO_VALUE for row in rules["counts"]):
        raise RuntimeError("Closed ordinary account has not saved every rope-reset zero")
    for name, value in (("initial-fixture.json", project_initial_fixture(fixture)),
            ("rope-before-saved-state.json", before), ("final-saved-state.json", saved)):
        write_new_projection(processed / name, value)
    # This receipt stays private; the raw account and unrelated fields are
    # never copied into the committed recording.
    return {"privateAccount": str(account_path), "privateAccountSha256": raw_sha,
        "projection": str(processed / "final-saved-state.json"),
        "projectionSha256": hashlib.sha256((processed / "final-saved-state.json").read_bytes()).hexdigest(),
        "positiveProjectionSha256": hashlib.sha256((processed / "rope-before-saved-state.json").read_bytes()).hexdigest()}
