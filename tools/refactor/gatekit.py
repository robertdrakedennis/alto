#!/usr/bin/env python3
"""Helpers of the merge gate (tools/refactor/gate.sh; README "Gate").

    gatekit.py plan [--base REV] [--full]
        Which parts of the gate a change needs, as shell assignments:
        GATE_RUST, GATE_SERVER (0/1), GATE_RUST_ALL (1 = every package),
        GATE_PACKAGES (the selected workspace packages), GATE_RSSCAN,
        GATE_RENDER_TIER (1 = run the render tier's tests), GATE_TEST_FILTER
        (the nextest filter that leaves them out, or empty) and GATE_PLAN (a
        one-line reason). Without --base, or with --full, everything is
        selected.
    gatekit.py key [OPTION...]
        The record key of the working tree (every tracked and untracked,
        not ignored file, exactly as on disk) plus the gate options and the
        local inputs the tree does not hold (toolchain, the pack).
    gatekit.py record write KEY RESULT.json | record find KEY
        Store or look up a gate record (records live in the git common dir,
        so every worktree of the repository shares them; GATE_RECORD_DIR
        overrides).
    gatekit.py lock -- COMMAND...
        Runs COMMAND while holding the machine-wide gate lock (one gate at a
        time; a second gate waits). The lock is an flock, so it is released
        when the holder exits, however it exits.
    gatekit.py replay-check [--without-render-tier] JUNIT.xml [JUNIT.xml...]
        Every test in replay-gate.txt must appear in the tests step's
        results for the client910 lib and have passed (the render tier's
        tests excepted with --without-render-tier, where the plan left them
        out; they must then not have run).
"""
import fcntl
import hashlib
import json
import os
import platform
import re
import shlex
import signal
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
import workspace  # noqa: E402

# A change to one of these runs every Rust package: they are the foundation
# every crate builds on, or the gate itself.
RUST_FOUNDATION_PACKAGES = {"rs910-core", "rs910-js5", "rs910-protocol"}
RUST_FOUNDATION_PATHS = (
    "tools/Cargo.toml", "tools/Cargo.lock", "tools/.config/", "rust-toolchain.toml",
    "tools/refactor/", "tools/revision/", "revisions/",
)
RUST_PATHS = ("tools/", "revisions/", "rust-toolchain.toml")
# The render tier: client910 replay tests that prove the renderer, the
# presentation cadence and the profiler are not observable (and that the
# injected clock makes a replay deterministic). A `--base` run includes them
# only when the change touches what they guard (RENDER_TIER_PACKAGES,
# RENDER_TIER_PATHS, or a file with profiler scopes); every run that tests
# every crate (`--full`, no `--base`, a foundation change) includes them.
RENDER_TIER_TESTS = (
    "app::session_replay::renderer_choice_is_observationally_inert",
    "app::session_replay::present_rate_is_observationally_inert",
    "app::session_replay::present_rate_is_observationally_inert_in_the_woodcutting_session",
    "app::session_replay::profiler_is_observationally_inert",
    "app::session_replay::profiler_is_observationally_inert_in_the_woodcutting_session",
    "app::session_replay::two_replays_on_the_injected_clock_have_identical_cycle_digests",
)
RENDER_TIER_PACKAGES = {
    "rs910-render-gpu", "rs910-render-modern", "rs910-gpu-device", "rs910-toolkit", "rs910-far-scene",
}
RENDER_TIER_PATHS = (
    # the app shell, the renderer choice and the replay harness itself
    "tools/client910/src/app/", "tools/client910/src/app.rs", "tools/client910/src/main.rs",
    "tools/client910/src/lib.rs", "tools/client910/src/active_toolkit.rs",
    "tools/client910/src/modern_display.rs", "tools/client910/src/debug_flags.rs",
    "tools/client910/src/session_replay.rs", "tools/client910/src/test_support.rs",
    # the client core's frame, phases and redraw (presentation), the
    # graphics runtime and the toolkit capabilities
    "tools/client910/crates/rs910-client/src/client_core.rs",
    "tools/client910/crates/rs910-client/src/client_core/",
    "tools/client910/crates/rs910-client/src/graphics_runtime.rs",
    "tools/client910/crates/rs910-client/src/toolkit_caps.rs",
    # the profiler and the clock
    "tools/client910/crates/rs910-core/src/profile.rs",
    "tools/client910/crates/rs910-core/src/logic_clock.rs",
)
# A changed Rust file that holds profiler scopes (before or after the
# change) is a profiler change.
PROFILER_MARK = "profile::"


def render_tier_filter():
    """The nextest filter expression that leaves the render tier out."""
    tests = " | ".join(f"test(={name})" for name in RENDER_TIER_TESTS)
    return f"not (package(client910) and ({tests}))"


def holds_profiler_scopes(base, path):
    now = ROOT / path
    if now.is_file() and PROFILER_MARK in now.read_text(errors="replace"):
        return True
    before = subprocess.run(["git", "show", f"{base}:{path}"], cwd=ROOT, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, text=True, errors="replace")
    return before.returncode == 0 and PROFILER_MARK in before.stdout


def render_tier_reason(base, rust_files, touched):
    """Why a `--base` change needs the render tier, or None."""
    hit = sorted(touched & RENDER_TIER_PACKAGES)
    if hit:
        return f"changed {', '.join(hit)}"
    for f in rust_files:
        if f.startswith(RENDER_TIER_PATHS):
            return f"changed {f}"
    for f in rust_files:
        if f.endswith(".rs") and holds_profiler_scopes(base, f):
            return f"{f} holds profiler scopes"
    return None


def git(*args, check=True, env=None):
    proc = subprocess.run(["git", *args], cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          text=True, env=env)
    if check and proc.returncode != 0:
        sys.exit(f"git {' '.join(args)} failed: {proc.stderr.strip()}")
    return proc.stdout


def changed_files(base):
    """Files that differ between `base` and the working tree as it is on disk
    (unstaged and untracked files included; a file identical to the base is
    not a change)."""
    if subprocess.run(["git", "rev-parse", "--verify", "--quiet", f"{base}^{{commit}}"], cwd=ROOT,
                      stdout=subprocess.DEVNULL).returncode != 0:
        sys.exit(f"gate: --base {base!r} is not a git revision")
    files = git("diff-tree", "-r", "--name-only", "--no-renames", base, tree_hash()).split("\n")
    return sorted(f for f in files if f)


def package_dirs():
    """[(relative dir, package)] of every workspace member, longest first."""
    out = []
    for name, manifest, _ in workspace.members():
        out.append((manifest.parent.relative_to(ROOT).as_posix() + "/", name))
    return sorted(out, key=lambda x: -len(x[0]))


def reverse_dependents():
    """{package: packages that depend on it, directly or not} in the workspace."""
    cmd = ["cargo", "metadata", "--format-version", "1", "--manifest-path", str(workspace.WORKSPACE)]
    if os.environ.get("CARGO_NET_OFFLINE"):
        cmd.append("--offline")
    meta = json.loads(subprocess.run(cmd, stdout=subprocess.PIPE, text=True, check=True).stdout)
    members = set(meta["workspace_members"])
    name_of = {p["id"]: p["name"] for p in meta["packages"]}
    users = {name_of[m]: set() for m in members}
    for node in meta["resolve"]["nodes"]:
        if node["id"] not in members:
            continue
        for dep in node["deps"]:
            if dep["pkg"] in members:
                users[name_of[dep["pkg"]]].add(name_of[node["id"]])
    closure = {}
    for pkg in users:
        seen, todo = set(), [pkg]
        while todo:
            for user in users[todo.pop()]:
                if user not in seen:
                    seen.add(user)
                    todo.append(user)
        closure[pkg] = seen
    return closure


def plan(base, full):
    """What a change needs: which step groups run and which crates' tests."""
    every = sorted(name for name, _, _ in workspace.members())
    everything = dict(rust=1, server=0, rust_all=1, packages=every, rsscan=1, render_tier=1)
    if full:
        return dict(everything, why="--full: every step and crate, real no-pack cross-check")
    if base is None:
        return dict(everything, why="no --base: every step and crate")
    files = changed_files(base)
    # No test or gate step reads Markdown (docs only).
    code = [f for f in files if not f.endswith(".md")]
    rust_files = [f for f in code if f.startswith(RUST_PATHS)]
    server = 0
    rsscan = int(any(f.startswith("tools/refactor/") for f in code))
    if not rust_files:
        return dict(rust=0, server=server, rust_all=0, packages=[], rsscan=0, render_tier=0,
                    why=f"{len(files)} changed files against {base}, none under tools/ or revisions/")
    dirs = package_dirs()
    touched, wide = set(), None
    for f in rust_files:
        owner = next((pkg for d, pkg in dirs if f.startswith(d)), None)
        if owner is not None:
            touched.add(owner)
        if wide is None and f.startswith(RUST_FOUNDATION_PATHS):
            wide = f"shared foundation changed ({f})"
        elif wide is None and (owner is None or not (f.endswith(".rs") or f.endswith("/Cargo.toml"))):
            # Outside every crate, or a non-Rust file inside one (fixtures and
            # recordings are read across crates): no safe narrowing.
            wide = f"{f} is not a crate's Rust source"
    users = reverse_dependents()
    selected = set(touched)
    for pkg in touched:
        selected |= users.get(pkg, set())
    if wide is None and selected & RUST_FOUNDATION_PACKAGES:
        hit = (touched & RUST_FOUNDATION_PACKAGES) or (selected & RUST_FOUNDATION_PACKAGES)
        wide = f"shared foundation changed ({', '.join(sorted(hit))})"
    if wide is not None:
        return dict(rust=1, server=server, rust_all=1, packages=every, rsscan=rsscan, render_tier=1,
                    why=wide)
    # The replay gate always runs. It lives in client910, which depends on
    # every client crate (so it is selected anyway); add it regardless.
    selected.add("client910")
    why = f"changed {', '.join(sorted(touched))} (+ dependents)"
    tier = render_tier_reason(base, rust_files, touched)
    why += f"; render tier: {tier}" if tier else "; render tier left out (no render, presentation, profiler or app-shell change)"
    return dict(rust=1, server=server, rust_all=int(set(every) <= selected), packages=sorted(selected),
                rsscan=0, render_tier=int(tier is not None), why=why)


def cmd_plan(args):
    base, full = None, False
    while args:
        a = args.pop(0)
        if a == "--base":
            base = args.pop(0) or None
        elif a == "--full":
            full = True
        else:
            sys.exit(f"plan: unknown argument {a}")
    p = plan(base, full)
    print(f"GATE_RUST={p['rust']}")
    print(f"GATE_SERVER={p['server']}")
    print(f"GATE_RUST_ALL={p['rust_all']}")
    print(f"GATE_RSSCAN={p['rsscan']}")
    print(f"GATE_RENDER_TIER={p['render_tier']}")
    print(f"GATE_TEST_FILTER={shlex.quote('' if p['render_tier'] else render_tier_filter())}")
    print(f"GATE_PACKAGES={shlex.quote(' '.join(p['packages']))}")
    print(f"GATE_PLAN={shlex.quote(p['why'])}")


# ---------------------------------------------------------------------------
# records
# ---------------------------------------------------------------------------

def tree_hash():
    """The git tree of the working tree as it is on disk: the index plus every
    unstaged change and untracked (not ignored) file, through a throwaway index."""
    with tempfile.TemporaryDirectory() as tmp:
        index = Path(tmp) / "index"
        real = Path(git("rev-parse", "--git-path", "index").strip())
        if not real.is_absolute():
            real = ROOT / real
        if real.exists():
            index.write_bytes(real.read_bytes())
        env = dict(os.environ, GIT_INDEX_FILE=str(index))
        git("add", "-A", ".", env=env)
        return git("write-tree", env=env).strip()


def pack_fingerprint():
    """Names, sizes and modification times of the packed cache (gitignored)."""
    pack = ROOT / "server/data/pack"
    if not pack.exists():
        return "absent"
    h = hashlib.sha256()
    for p in sorted(pack.rglob("*")):
        if p.is_file():
            st = p.stat()
            h.update(f"{p.relative_to(pack)}\t{st.st_size}\t{st.st_mtime_ns}\n".encode())
    return h.hexdigest()[:16]


def tool_version(*cmd):
    try:
        return subprocess.run(cmd, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                              text=True).stdout.strip()
    except OSError:
        return "absent"


def key_parts(options):
    base_revs = []
    opts = list(options)
    # A revision option names a commit by its hash, so a moved branch
    # (`--base main`) is not mistaken for the same gate run.
    for i, o in enumerate(opts[:-1]):
        if o in ("--base", "--fn-hash") and not Path(opts[i + 1]).is_file():
            sha = git("rev-parse", "--verify", "--quiet", opts[i + 1], check=False).strip()
            base_revs.append(f"{opts[i + 1]}={sha}")
    return {
        "tree": tree_hash(),
        "options": " ".join(opts),
        "revs": base_revs,
        "rustc": tool_version("rustc", "-V"),
        "pack": pack_fingerprint(),
    }


def key_of(parts):
    return hashlib.sha256(json.dumps(parts, sort_keys=True).encode()).hexdigest()[:32]


def record_dir():
    if os.environ.get("GATE_RECORD_DIR"):
        d = Path(os.environ["GATE_RECORD_DIR"])
    else:
        common = Path(git("rev-parse", "--git-common-dir").strip())
        d = (common if common.is_absolute() else ROOT / common) / "gate-records"
    d.mkdir(parents=True, exist_ok=True)
    return d


def cmd_key(args):
    parts = key_parts(args)
    print(json.dumps({"key": key_of(parts), **parts}))


def cmd_record(args):
    if args[:1] == ["write"] and len(args) == 3:
        key, result = args[1], Path(args[2])
        rec = json.loads(result.read_text())
        rec["key"] = key
        rec["host"] = platform.node()
        rec["worktree"] = str(ROOT)
        rec["written"] = time.strftime("%Y-%m-%dT%H:%M:%S%z")
        out = record_dir() / f"{key}.json"
        out.write_text(json.dumps(rec, indent=1) + "\n")
        print(f"gate record: {out}")
        return 0
    if args[:1] == ["find"] and len(args) == 2:
        p = record_dir() / f"{args[1]}.json"
        if not p.exists():
            return 1
        rec = json.loads(p.read_text())
        if rec.get("exit") != 0:
            return 1
        steps = ", ".join(f"{s['name']} {s['status']} {s['secs']}" for s in rec.get("steps", []))
        print(f"gate record {p.name}: exit 0 on tree {rec.get('tree')} ({rec.get('options') or 'no options'}), "
              f"{rec.get('worktree')} at {rec.get('written')}; {steps}")
        return 0
    sys.exit("usage: gatekit.py record write KEY RESULT.json | record find KEY")


# ---------------------------------------------------------------------------
# lock
# ---------------------------------------------------------------------------

def cmd_lock(args):
    if args[:1] == ["--"]:
        args = args[1:]
    if not args:
        sys.exit("usage: gatekit.py lock -- COMMAND...")
    # Gates never wait for each other. Each one registers itself, and its test
    # threads are the cores divided by the gates running when it starts. That
    # is not for CPU (the OS shares that) but for memory: a replay test holds
    # about 1.2 GB, so several gates each running a full set of replays at
    # once would swap. A lone gate uses the whole machine.
    reg = Path(os.environ.get("GATE_LOCK_FILE", Path.home() / ".cache/alto/gate.lock")).with_suffix(".d")
    reg.mkdir(parents=True, exist_ok=True)
    live = 0
    for entry in reg.iterdir():
        try:
            os.kill(int(entry.name), 0)
            live += 1
        except (ValueError, ProcessLookupError, PermissionError):
            entry.unlink(missing_ok=True)
    mine = reg / str(os.getpid())
    mine.write_text(f"{ROOT} since {time.strftime('%H:%M:%S')}\n")
    share = max(2, (os.cpu_count() or 8) // (live + 1))
    env = dict(os.environ)
    env.setdefault("NEXTEST_TEST_THREADS", str(share))
    print(f"gate: {live + 1} gate(s) running, {share} test threads", flush=True)
    child = subprocess.Popen(args, env=env)

    def forward(sig, _frame):
        child.send_signal(sig)

    for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, forward)
    try:
        rc = child.wait()
    finally:
        mine.unlink(missing_ok=True)
    return rc


# ---------------------------------------------------------------------------
# replay check
# ---------------------------------------------------------------------------

def replay_names():
    names = []
    for line in (HERE / "replay-gate.txt").read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            names.append(line)
    return names


def cmd_replay_check(args):
    without_tier = "--without-render-tier" in args
    args = [a for a in args if a != "--without-render-tier"]
    results = {}
    for path in args:
        if not Path(path).exists():
            print(f"replay gate: no test results at {path} (did the tests step run?)")
            return 1
        for case in ET.parse(path).getroot().iter("testcase"):
            # nextest: classname = binary id, name = the libtest name
            if case.get("classname") != "client910":
                continue
            failed = any(child.tag in ("failure", "error", "skipped", "flakyFailure", "rerunFailure")
                         for child in case)
            results[case.get("name")] = "failed" if failed else "passed"
    names = replay_names()
    unknown = [t for t in RENDER_TIER_TESTS if t not in names]
    if unknown:
        print(f"replay gate: render-tier tests missing from replay-gate.txt: {', '.join(unknown)}")
        return 1
    bad = []
    if without_tier:
        # The plan left the render tier out: its tests must not have run
        # (the filter is the reason they are absent), and every other entry
        # is checked as usual.
        ran = [t for t in RENDER_TIER_TESTS if t in results]
        if ran:
            bad += [f"{t}: ran although the plan left the render tier out" for t in ran]
        names = [n for n in names if n not in RENDER_TIER_TESTS]
        print(f"replay gate: render tier left out by the plan ({len(RENDER_TIER_TESTS)} tests; "
              "--full, runs without --base and render/presentation/profiler/app-shell changes run it)")
    for name in names:
        status = results.get(name)
        if status != "passed":
            bad.append(f"{name}: {status or 'did not run (missing, renamed, ignored or filtered out)'}")
    for b in bad:
        print(f"  FAIL {b}")
    if bad:
        print(f"replay gate: {len(names) - len(bad)} of {len(names)} replay/golden tests ran and passed")
        return 1
    print(f"replay gate: all {len(names)} replay/golden tests ran and passed (from the tests step)")
    return 0


def main(argv):
    if not argv:
        sys.exit(__doc__)
    cmd, args = argv[0], argv[1:]
    if cmd == "plan":
        return cmd_plan(args)
    if cmd == "key":
        return cmd_key(args)
    if cmd == "record":
        return cmd_record(args)
    if cmd == "lock":
        return cmd_lock(args)
    if cmd == "replay-check":
        return cmd_replay_check(args)
    sys.exit(__doc__)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]) or 0)
