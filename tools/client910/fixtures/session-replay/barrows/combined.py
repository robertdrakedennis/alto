#!/usr/bin/env python3
"""Private final Barrows fight + Bob + installed-armour-stand source proposal."""
import argparse
import importlib.util
import json
from pathlib import Path
import sys


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"explicit module unavailable: {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    repair = load_module("private_barrows_repair", Path(__file__).with_name("repair_driver.py"))
    base = repair._base
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("control-module", "rules", "food-rules", "charge-rules", "plan", "symbols", "receipts", "journal", "result"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--socket", required=True)
    parser.add_argument("--diagnostics-root", type=Path, required=True)
    parser.add_argument("--deadline-seconds", type=float, default=base.DEFAULT_SESSION_SECONDS)
    parser.add_argument("--action-seconds", type=float, default=base.DEFAULT_ACTION_SECONDS)
    parser.add_argument("--fight-seconds", type=float, default=base.DEFAULT_FIGHT_SECONDS)
    parser.add_argument("--stall-seconds", type=float, default=base.DEFAULT_STALL_SECONDS)
    parser.add_argument("--socket-seconds", type=float, default=base.DEFAULT_SOCKET_SECONDS)
    args = parser.parse_args()
    if not 0 < args.deadline_seconds <= base.MAX_SESSION_SECONDS:
        parser.error("finite session policy exceeded")
    if not all(0 < value <= args.deadline_seconds for value in (
        args.action_seconds, args.fight_seconds, args.stall_seconds, args.socket_seconds)):
        parser.error("finite action/fight/stall/socket policy exceeded")
    if not args.diagnostics_root.is_absolute() or not args.diagnostics_root.is_dir():
        parser.error("diagnostics root must be an explicit existing absolute directory")
    private_root = args.diagnostics_root.resolve()
    for path in (args.journal, args.result):
        if not path.resolve().is_relative_to(private_root) or path.exists() or not path.parent.is_dir():
            parser.error("diagnostics require fresh files under private bosses root")
    if args.journal.resolve() == args.result.resolve():
        parser.error("journal/result must be distinct")
    control = driver = None
    result = {"status": "failed", "reason": "initialization incomplete"}
    try:
        module = load_module("ordinary_client_control", args.control_module)
        control = module.Control(str(args.socket), args.socket_seconds)
        driver = repair.RepairDriver(control, args)
        result = driver.run()
        return 0
    except (OSError, ValueError, KeyError, IndexError, TypeError, RuntimeError) as error:
        result = {"status": "failed", "reason": str(error),
            "stage": driver.stage if driver else "initialization",
            "last_receipt": driver.state if driver else None}
        print(json.dumps(result, sort_keys=True), file=sys.stderr)
        return 1
    finally:
        if control is not None:
            control.close()
        if driver is not None:
            driver.journal.close()
        with args.result.open("x", encoding="utf8") as output:
            json.dump(result, output, sort_keys=True, indent=2)


if __name__ == "__main__":
    raise SystemExit(main())
