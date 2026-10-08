#!/usr/bin/env python3
"""The Cargo workspace at tools/Cargo.toml, as the refactor gates see it.

Phase 2 splits client910 into `rs910-*` crates under
`tools/client910/crates/`. The gates treat "the client" as client910 plus
every `rs910-*` member (clippy ratchet, fmt ratchet, test inventory, DAG
scan); native910 keeps its own strict gates. This module is the one place
that knows how to list them.

    python3 tools/refactor/workspace.py            # name<TAB>manifest per client package
    python3 tools/refactor/workspace.py --pkg-args # "-p client910 -p rs910-core ..."
"""
import json
import os
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
WORKSPACE = ROOT / "tools/Cargo.toml"


def members():
    """[(name, manifest_path, features)] of every workspace member."""
    cmd = ["cargo", "metadata", "--no-deps", "--format-version", "1",
           "--manifest-path", str(WORKSPACE)]
    if os.environ.get("CARGO_NET_OFFLINE"):
        cmd.append("--offline")
    proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if proc.returncode != 0:
        sys.exit(f"cargo metadata failed for {WORKSPACE}:\n{proc.stderr}")
    meta = json.loads(proc.stdout)
    ids = set(meta["workspace_members"])
    return [(p["name"], Path(p["manifest_path"]), set(p["features"]))
            for p in meta["packages"] if p["id"] in ids]


def package_name(package_id):
    """A package name from a cargo package id (`path+file:///.../rs910-core#0.1.0`
    or `...#name@0.1.0`, `registry+...#name@1.0`)."""
    tail = package_id.rsplit("/", 1)[-1]
    name, _, rest = tail.partition("#")
    if "@" in rest:
        return rest.split("@", 1)[0]
    return name


def is_client(name):
    return name == "client910" or name.startswith("rs910-")


def client_packages():
    """[(name, manifest_path, features)] of client910 and the rs910-* crates."""
    return sorted((m for m in members() if is_client(m[0])), key=lambda m: m[0])


def pkg_args():
    return [a for name, _, _ in client_packages() for a in ("-p", name)]


if __name__ == "__main__":
    if "--pkg-args" in sys.argv:
        print(" ".join(pkg_args()))
    else:
        for name, manifest, _ in client_packages():
            print(f"{name}\t{manifest}")
