#!/usr/bin/env python3
"""One CLI for an agent driving the client and the dev server together.

  harness.py up [--server] [--username U] [--password P] [--crypto-off] [--headless]
  harness.py down
  harness.py status
  harness.py client <json request>       raw request on the client control socket
  harness.py snapshot [--query JSON]
  harness.py chat [--limit N]
  harness.py cmd <dev command line>      sent by the client as CLIENT_CHEAT
  harness.py shot <png path>             capture the next presented frame
  harness.py act <action json>           walk/loc/npc/object/ui/pointer/key/wheel action
  harness.py server <route> [args]       state | players | player NAME | npcs [X Z [R]]
                                         | inspect PATH | command PLAYER LINE | eval CODE

`up` builds the dev client (`--features dev`), launches it logged in with its
control socket open and background input allowed, and with `--server` starts
the lobby and world from `--server-dir` with the world control endpoint on.
State lives in `--state-dir` so later calls find the running processes.
"""
import argparse
import json
import os
import signal
import subprocess
import sys
import time
import urllib.parse
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from control import Control  # noqa: E402

CLIENT_DIR = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
TOOLS_DIR = os.path.dirname(CLIENT_DIR)
DEFAULT_SERVER_DIR = os.path.join(os.path.dirname(TOOLS_DIR), "server")
DEFAULT_STATE_DIR = "/tmp/alto-harness"
DEFAULT_CONTROL_PORT = 43600
READY_TIMEOUT_SECONDS = 240
SERVER_TIMEOUT_SECONDS = 180
POLL_SECONDS = 0.5
REQUEST_TIMEOUT_SECONDS = 30
SHOT_TIMEOUT_SECONDS = 15
DEFAULT_NPC_RADIUS = 16


def state_path(args, name):
    return os.path.join(args.state_dir, name)


def read_state(args):
    try:
        with open(state_path(args, "state.json")) as handle:
            return json.load(handle)
    except FileNotFoundError:
        return {}


def write_state(args, state):
    with open(state_path(args, "state.json"), "w") as handle:
        json.dump(state, handle, indent=2)


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except (OSError, TypeError):
        return False


def spawn(command, cwd, log, env):
    with open(log, "ab") as out:
        return subprocess.Popen(command, cwd=cwd, stdout=out, stderr=subprocess.STDOUT, env=env, start_new_session=True).pid


def wait_for(predicate, seconds, what):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            if predicate():
                return
        except (OSError, ValueError, RuntimeError, KeyError):
            pass
        time.sleep(POLL_SECONDS)
    raise TimeoutError(f"{what} not ready after {seconds}s")


def client_request(args, request, timeout=REQUEST_TIMEOUT_SECONDS):
    control = Control(state_path(args, "client.sock"), timeout)
    try:
        return control.request(request)
    finally:
        control.close()


def client_action(args, action):
    control = Control(state_path(args, "client.sock"), REQUEST_TIMEOUT_SECONDS)
    try:
        return control.action(action)
    finally:
        control.close()


def server_call(args, route, query=None, body=None):
    port = read_state(args).get("control_port", args.control_port)
    url = f"http://127.0.0.1:{port}/{route}"
    if query:
        url += "?" + urllib.parse.urlencode(query)
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url, data=data, method="POST" if data is not None else "GET")
    try:
        with urllib.request.urlopen(request, timeout=REQUEST_TIMEOUT_SECONDS) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        return json.load(error)


def up(args):
    os.makedirs(args.state_dir, exist_ok=True)
    state = read_state(args)
    if args.server:
        env = dict(os.environ, ALTO_CONTROL_PORT=str(args.control_port))
        if args.crypto_off:
            env["ALTO_LOGIN_CRYPTO"] = "off"
        for name in ("lobby", "world"):
            if not alive(state.get(name)):
                state[name] = spawn(["npm", "run", name], args.server_dir, state_path(args, f"{name}.log"), env)
        state["control_port"] = args.control_port
        write_state(args, state)
        wait_for(lambda: server_call(args, "state")["ok"], SERVER_TIMEOUT_SECONDS, "world control endpoint")
    if alive(state.get("client")):
        print(json.dumps({"status": "already up", **state}))
        return 0
    subprocess.run(["cargo", "build", "--release", "-p", "client910", "--features", "dev"], cwd=TOOLS_DIR, check=True)
    socket_path = state_path(args, "client.sock")
    if os.path.exists(socket_path):
        os.unlink(socket_path)
    env = dict(os.environ, CLIENT910_CONTROL=socket_path, CLIENT910_CONTROL_UNFOCUSED="1")
    command = [os.path.join(TOOLS_DIR, "target", "release", "client910"), "--direct-login", "--username", args.username, "--password", args.password]
    if args.crypto_off:
        command += ["--login-crypto", "off"]
    if args.headless:
        command += ["--renderer", "null"]
    state["client"] = spawn(command, CLIENT_DIR, state_path(args, "client.log"), env)
    state["username"] = args.username
    write_state(args, state)
    wait_for(lambda: client_request(args, {"command": "snapshot"})["data"].get("ready"), READY_TIMEOUT_SECONDS, "client map")
    print(json.dumps({"status": "up", **state}))
    return 0


def down(args):
    state = read_state(args)
    for name in ("client", "world", "lobby"):
        pid = state.pop(name, None)
        if alive(pid):
            os.killpg(pid, signal.SIGTERM)
    write_state(args, state)
    print(json.dumps({"status": "down"}))
    return 0


def shot(args):
    path = os.path.abspath(args.path)
    if os.path.exists(path):
        os.unlink(path)
    response = client_request(args, {"command": "screenshot", "path": path})
    if response.get("status") != "accepted":
        print(json.dumps(response))
        return 1
    wait_for(lambda: os.path.getsize(path) > 0, SHOT_TIMEOUT_SECONDS, "screenshot")
    print(json.dumps({"status": "written", "path": path}))
    return 0


def server(args):
    route, rest = args.route, args.rest
    if route == "player":
        response = server_call(args, "player", {"name": rest[0], "depth": rest[1] if len(rest) > 1 else 3})
    elif route == "npcs":
        query = {"x": rest[0], "z": rest[1], "radius": rest[2] if len(rest) > 2 else DEFAULT_NPC_RADIUS} if len(rest) >= 2 else None
        response = server_call(args, "npcs", query)
    elif route == "inspect":
        response = server_call(args, "inspect", {"path": rest[0], "depth": rest[1] if len(rest) > 1 else 3})
    elif route == "command":
        response = server_call(args, "command", body={"player": rest[0], "command": " ".join(rest[1:])})
    elif route == "eval":
        response = server_call(args, "eval", body={"code": " ".join(rest)})
    else:
        response = server_call(args, route)
    print(json.dumps(response, indent=1))
    return 0 if response.get("ok") else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--state-dir", default=os.environ.get("ALTO_HARNESS_DIR", DEFAULT_STATE_DIR))
    parser.add_argument("--control-port", type=int, default=DEFAULT_CONTROL_PORT)
    commands = parser.add_subparsers(dest="command", required=True)
    start = commands.add_parser("up")
    start.add_argument("--server", action="store_true", help="also start the lobby and world")
    start.add_argument("--server-dir", default=DEFAULT_SERVER_DIR)
    start.add_argument("--username", default="tester")
    start.add_argument("--password", default="password")
    start.add_argument("--crypto-off", action="store_true")
    start.add_argument("--headless", action="store_true", help="null renderer: no pixels, no screenshots")
    commands.add_parser("down")
    commands.add_parser("status")
    raw = commands.add_parser("client")
    raw.add_argument("json")
    snapshot = commands.add_parser("snapshot")
    snapshot.add_argument("--query", default="{}")
    chat = commands.add_parser("chat")
    chat.add_argument("--limit", type=int, default=32)
    cmd = commands.add_parser("cmd")
    cmd.add_argument("line", nargs="+")
    capture = commands.add_parser("shot")
    capture.add_argument("path")
    act = commands.add_parser("act")
    act.add_argument("json")
    remote = commands.add_parser("server")
    remote.add_argument("route")
    remote.add_argument("rest", nargs="*")
    args = parser.parse_args()
    os.makedirs(args.state_dir, exist_ok=True)
    try:
        if args.command == "up":
            return up(args)
        if args.command == "down":
            return down(args)
        if args.command == "status":
            state = read_state(args)
            print(json.dumps({key: (value, alive(value)) if key in ("client", "lobby", "world") else value for key, value in state.items()}))
            return 0
        if args.command == "shot":
            return shot(args)
        if args.command == "server":
            return server(args)
        if args.command == "act":
            response = client_action(args, json.loads(args.json))
        elif args.command == "client":
            response = client_request(args, json.loads(args.json))
        elif args.command == "snapshot":
            response = client_request(args, {"command": "snapshot", "query": json.loads(args.query)})
        elif args.command == "chat":
            response = client_request(args, {"command": "chat", "limit": args.limit})
        else:
            response = client_request(args, {"command": "cheat", "line": " ".join(args.line)})
        print(json.dumps(response, indent=1))
        return 0 if response.get("status") in ("observed", "accepted") else 1
    except (OSError, ValueError, RuntimeError, TimeoutError, subprocess.CalledProcessError) as error:
        print(json.dumps({"status": "error", "reason": str(error)}), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
