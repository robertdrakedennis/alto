#!/usr/bin/env python3
"""Generate a CS2 profile from registrations and reviewed decoder facts.

Facts supply build/client/cache identity and per-opcode special encodings or
recovered commands. Named Ghidra functions supply hints only. Local registration
exports and original binaries must never be committed.
"""
import argparse
import json
from pathlib import Path


def generate(registry, facts, functions):
    handler_field = str(facts["handler_offset"])
    width_field = str(facts["width_offset"])
    names = {int(row["address"], 0): row["name"] for row in functions["functions"]}
    encodings = facts["encodings"]
    commands = facts["commands"]
    analysis = facts.get("analysis", {})
    if registry["unknown"]:
        raise ValueError("registry contains unresolved writes")
    entries = registry["entries"]
    for opcode in set(encodings) | set(commands) | set(analysis):
        if opcode not in entries:
            raise ValueError(f"decoder facts reference absent opcode {opcode}")
    result = {key: facts[key] for key in ("build", "client_md5", "script_index_sha256", "script_archive")}
    result["string_encoding"] = facts.get("string_encoding")
    result["switch_lookup"] = facts.get("switch_lookup")
    result["analysis_locals"] = facts.get("analysis_locals")
    result["format"] = 1
    result["opcodes"] = []
    for opcode, entry in sorted(entries.items(), key=lambda pair: int(pair[0])):
        pointer = entry[handler_field]["value"]
        width = entry[width_field]["value"]
        if width not in (0, 1):
            raise ValueError(f"invalid width for opcode {opcode}")
        row = {
            "id": int(opcode), "encoding": encodings.get(opcode, "int" if width else "byte"),
            "command": commands.get(opcode), "symbol_hint": names.get(pointer),
            "handler_address": pointer,
        }
        if opcode in analysis:
            if opcode not in commands:
                raise ValueError(f"analysis contract needs a recovered command for opcode {opcode}")
            row["analysis"] = analysis[opcode]
        result["opcodes"].append(row)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("registry", "facts", "functions", "output"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--check", action="store_true", help="compare without writing")
    options = parser.parse_args()
    result = generate(json.loads(options.registry.read_text()), json.loads(options.facts.read_text()), json.loads(options.functions.read_text()))
    output = json.dumps(result, indent=2) + "\n"
    if options.check:
        if json.loads(options.output.read_text()) != result:
            raise SystemExit("CS2 profile differs from its registration and decoder facts")
    else:
        with options.output.open("x") as destination:
            destination.write(output)
    print(f"Profile contains {len(result['opcodes'])} registrations")


if __name__ == "__main__":
    main()
