#!/usr/bin/env python3
"""Finite NDJSON CLI for the ordinary client's local live controller."""
import argparse
import json
import os
import socket
import sys
import time

MAX_RESPONSE_BYTES = 512 * 1024
MAX_REQUEST_BYTES = 16 * 1024
READ_CHUNK_BYTES = 4096
DEFAULT_SCAN_LIMIT = 64
DEFAULT_TIMEOUT_SECONDS = 10
FIRST_REQUEST_OFFSET = 1
MAX_WAIT_SECONDS = 300
OBSERVATION_POLL_SECONDS = 0.2
DEADLINE_EXPIRED_SECONDS = 0


class Control:
    def __init__(self, path, timeout):
        self.timeout = timeout
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            self.socket.settimeout(timeout)
            self.socket.connect(path)
        except BaseException:
            self.socket.close()
            raise
        # Wall time shares an origin across controller processes, including
        # system Python versions whose monotonic clock starts per process.
        self.next_id = time.time_ns()
        self.buffer = bytearray()

    def close(self):
        self.socket.close()

    def limit_timeout(self, deadline):
        remaining = deadline - time.monotonic()
        if remaining <= DEADLINE_EXPIRED_SECONDS:
            raise TimeoutError("controller request exceeded its deadline")
        self.socket.settimeout(remaining)

    def request(self, command, timeout=None):
        budget = self.timeout if timeout is None else min(self.timeout, timeout)
        deadline = time.monotonic() + budget
        self.next_id += FIRST_REQUEST_OFFSET
        identifier = self.next_id
        message = {"id": identifier, "request": command}
        encoded = json.dumps(message, separators=(",", ":")).encode() + b"\n"
        if len(encoded) > MAX_REQUEST_BYTES:
            raise ValueError("controller request exceeds bound")
        self.limit_timeout(deadline)
        self.socket.sendall(encoded)
        while b"\n" not in self.buffer:
            self.limit_timeout(deadline)
            chunk = self.socket.recv(READ_CHUNK_BYTES)
            if not chunk:
                raise RuntimeError("controller closed before acknowledging request")
            self.buffer.extend(chunk)
            if len(self.buffer) > MAX_RESPONSE_BYTES:
                raise RuntimeError("controller response exceeds bound")
        line, _, tail = self.buffer.partition(b"\n")
        self.buffer = bytearray(tail)
        response = json.loads(line)
        if response.get("id") != identifier:
            raise RuntimeError("response request identity mismatch")
        return response

    def action(self, action, expected=None):
        observed = self.request({"command": "snapshot"})
        if observed.get("status") != "observed":
            return observed
        state = observed["data"]
        if not state.get("ready") or state.get("map") is None:
            return {"status": "refused", "cycle": state["cycle"], "data": {"reason": "installed game map is not ready"}}
        return self.request({"command": "action", "map": state["map"] if expected is None else expected, "observed_cycle": state["cycle"], "action": action})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", default=os.environ.get("CLIENT910_CONTROL"))
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT_SECONDS)
    commands = parser.add_subparsers(dest="command", required=True)
    snapshot = commands.add_parser("snapshot")
    snapshot.add_argument("--query", default="{}", help="bounded varps/varbits/inventories/components/tiles JSON")
    for name in ("scan-locs", "scan-npcs", "scan-objects"):
        scan = commands.add_parser(name)
        scan.add_argument("--definition", type=int)
        scan.add_argument("--name")
        scan.add_argument("--level", type=int)
        scan.add_argument("--x", type=int)
        scan.add_argument("--z", type=int)
        scan.add_argument("--radius", type=int)
        scan.add_argument("--offset", type=int, default=0)
        scan.add_argument("--limit", type=int, default=DEFAULT_SCAN_LIMIT)
    walk = commands.add_parser("walk")
    walk.add_argument("x", type=int)
    walk.add_argument("z", type=int)
    walk.add_argument("--run", action="store_true", help="explicit run is refused; send ordinary retained modifier input instead")
    action = commands.add_parser("action")
    action.add_argument("json", help="typed loc/npc/object/ui/pointer/key/wheel action JSON; key codes use the native AWT mapping")
    action.add_argument("--expected-map", help="optional captured map JSON for explicit stale checks")
    wait = commands.add_parser("wait")
    wait.add_argument("--query", default="{}")
    wait.add_argument("--path", required=True, help="dot path in snapshot data, using array indices")
    wait.add_argument("--equals", required=True, help="expected JSON scalar/value")
    wait.add_argument("--seconds", type=float, default=DEFAULT_TIMEOUT_SECONDS)
    args = parser.parse_args()
    if not args.socket or not 0 < args.timeout <= MAX_WAIT_SECONDS:
        parser.error("a socket path and bounded positive timeout are required")
    control = None
    try:
        control = Control(args.socket, args.timeout)
        if args.command == "snapshot":
            response = control.request({"command": "snapshot", "query": json.loads(args.query)})
        elif args.command.startswith("scan-"):
            keys = ("definition", "name", "level", "x", "z", "radius", "offset", "limit")
            response = control.request({"command": args.command.replace("-", "_"), "query": {key: getattr(args, key) for key in keys}})
        elif args.command == "walk":
            response = control.action({"kind": "walk", "x": args.x, "z": args.z, "run": args.run})
        elif args.command == "action":
            response = control.action(json.loads(args.json), json.loads(args.expected_map) if args.expected_map else None)
        else:
            if not 0 < args.seconds <= MAX_WAIT_SECONDS:
                parser.error("wait deadline is outside the finite bound")
            expected = json.loads(args.equals)
            deadline = time.monotonic() + args.seconds
            prior_cycle = None
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("actual observed state did not satisfy wait condition")
                response = control.request({"command": "snapshot", "query": json.loads(args.query)}, timeout=remaining)
                if response.get("status") != "observed":
                    break
                value = response["data"]
                for part in args.path.split("."):
                    value = value[int(part)] if isinstance(value, list) else value[part]
                cycle = response["data"]["cycle"]
                if value == expected and (prior_cycle is None or cycle > prior_cycle):
                    break
                prior_cycle = cycle
                if time.monotonic() >= deadline:
                    raise TimeoutError("actual observed state did not satisfy wait condition")
                time.sleep(min(OBSERVATION_POLL_SECONDS, max(0, deadline - time.monotonic())))
        print(json.dumps(response, sort_keys=True))
        return 0 if response.get("status") in ("observed", "accepted") else 1
    except (OSError, ValueError, KeyError, IndexError, RuntimeError, TimeoutError) as error:
        print(json.dumps({"status": "error", "reason": str(error)}), file=sys.stderr)
        return 1
    finally:
        if control is not None:
            control.close()


if __name__ == "__main__":
    raise SystemExit(main())
