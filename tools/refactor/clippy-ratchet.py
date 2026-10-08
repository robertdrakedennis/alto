#!/usr/bin/env python3
"""Clippy/rustc warning ratchet for tools/client910 and its rs910-* crates
(quality item Q0.4).

client910 still carries a backlog of clippy warnings, so CI cannot run
`cargo clippy -- -D warnings` on it yet. Instead this script counts the
deduplicated warnings per lint over `cargo clippy --all-targets` and compares
them with the checked-in baseline (`clippy-baseline.txt` next to this file):

* a lint whose count rises, or a lint that is not in the baseline, fails;
* a lint whose count falls is reported, and the baseline should be lowered
  in the same change (`--update`), so the ratchet only ever tightens.

rustc lints (`dead_code`, `unused_*`, ...) are counted the same way and have
a baseline of zero, so any new compiler warning fails.

Usage:
    python3 tools/refactor/clippy-ratchet.py [--update] [-- extra cargo args]
    python3 tools/refactor/clippy-ratchet.py --json clippy.json [--update]

Since Phase 2.1 it lints every client package of the tools/ workspace
(client910 and `tools/client910/crates/rs910-*`, see workspace.py), all
targets of each, and counts their warnings together; native910 has its own
`clippy -D warnings` gate.

Environment: CARGO_TARGET_DIR is honoured by cargo as usual.
"""
import collections
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
BASELINE = HERE / "clippy-baseline.txt"
sys.path.insert(0, str(HERE))
import workspace  # noqa: E402
MANIFEST = workspace.WORKSPACE


def run_clippy(extra):
    cmd = [
        "cargo", "clippy", "--manifest-path", str(MANIFEST), *workspace.pkg_args(),
        "--all-targets", "--message-format=json", *extra,
    ]
    print("+", " ".join(cmd), file=sys.stderr, flush=True)
    proc = subprocess.run(cmd, stdout=subprocess.PIPE, text=True)
    if proc.returncode != 0:
        sys.exit(f"cargo clippy failed with exit code {proc.returncode}")
    return proc.stdout.splitlines()


def count(lines):
    seen = set()
    for line in lines:
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("reason") != "compiler-message":
            continue
        if msg.get("target", {}).get("name") is None or not workspace.is_client(
                workspace.package_name(msg.get("package_id", ""))):
            continue
        diag = msg["message"]
        if diag["level"] == "error":
            sys.exit("compile error:\n" + diag.get("rendered", ""))
        if diag["level"] != "warning" or not diag.get("code"):
            continue
        spans = [s for s in diag["spans"] if s["is_primary"]] or diag["spans"]
        if not spans:
            continue
        s = spans[0]
        seen.add((diag["code"]["code"], s["file_name"], s["line_start"],
                  s["column_start"], diag["message"]))
    return collections.Counter(k[0] for k in seen), seen


def load_baseline():
    base = {}
    if BASELINE.exists():
        for line in BASELINE.read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            lint, n = line.split()
            base[lint] = int(n)
    return base


def write_baseline(counts):
    body = [
        "# Deduplicated client910 warning counts per lint over",
        "# `cargo clippy --all-targets` (see clippy-ratchet.py). Lower only.",
        f"# total {sum(counts.values())}",
    ]
    body += [f"{lint} {n}" for lint, n in sorted(counts.items())]
    BASELINE.write_text("\n".join(body) + "\n")


def main(argv):
    update = "--update" in argv
    argv = [a for a in argv if a != "--update"]
    extra = []
    if "--" in argv:
        i = argv.index("--")
        argv, extra = argv[:i], argv[i + 1:]
    if argv[:1] == ["--json"]:
        lines = Path(argv[1]).read_text().splitlines()
    else:
        lines = run_clippy(extra)
    counts, items = count(lines)
    base = load_baseline()
    worse, better = [], []
    for lint in sorted(set(counts) | set(base)):
        now, was = counts.get(lint, 0), base.get(lint, 0)
        if now > was:
            worse.append((lint, was, now))
        elif now < was:
            better.append((lint, was, now))
    print(f"client910 warnings: {sum(counts.values())} (baseline {sum(base.values())})")
    for lint, was, now in better:
        print(f"  improved  {lint}: {was} -> {now}")
    for lint, was, now in worse:
        print(f"  REGRESSED {lint}: {was} -> {now}")
        for k in sorted(items):
            if k[0] == lint:
                print(f"      {k[1]}:{k[2]}:{k[3]} {k[4]}")
    if update:
        write_baseline(counts)
        print(f"wrote {BASELINE.relative_to(ROOT)}")
        return 0
    if worse:
        print("New warnings: fix them (preferred) or justify an #[allow] with a reason.")
        return 1
    if better:
        print("Counts fell: run with --update and commit the lower baseline.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
