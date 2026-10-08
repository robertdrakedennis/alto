#!/usr/bin/env python3
"""rustfmt ratchet for tools/client910 and its rs910-* crates (quality item Q0.4).

About a third of client910's source files are not rustfmt-formatted, and
formatting them in one change would conflict with every open branch. Files
listed in `fmt-pending.txt` (next to this file) may stay unformatted; every
other file reachable from the crate must pass `cargo fmt --check`. Format a
pending file when its owner has a quiet window, then drop it from the list
(`--update` rewrites the list from the current state; it never adds a file
that is formatted today).

Since Phase 2.1 the check covers every client package of the tools/
workspace (client910 and `tools/client910/crates/rs910-*`, see
workspace.py); paths in fmt-pending.txt stay relative to tools/client910.

Usage: python3 tools/refactor/fmt-ratchet.py [--update]
"""
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
CRATE = ROOT / "tools/client910"
PENDING = HERE / "fmt-pending.txt"
sys.path.insert(0, str(HERE))
import workspace  # noqa: E402


def unformatted():
    proc = subprocess.run(
        ["cargo", "fmt", "--manifest-path", str(workspace.WORKSPACE), *workspace.pkg_args(),
         "--check"],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    if proc.returncode not in (0, 1) or (proc.returncode == 1 and "Diff in" not in proc.stdout):
        sys.exit(f"cargo fmt --check failed:\n{proc.stderr}")
    files = set()
    for m in re.finditer(r"^Diff in (.+?):\d+:$", proc.stdout, re.M):
        path = Path(m.group(1)).resolve()
        files.add(path.relative_to(CRATE.resolve()).as_posix())
    return files


def main(argv):
    now = unformatted()
    pending = set()
    if PENDING.exists():
        pending = {
            l.strip() for l in PENDING.read_text().splitlines()
            if l.strip() and not l.startswith("#")
        }
    new = sorted(now - pending)
    fixed = sorted(pending - now)
    print(f"client910 unformatted files: {len(now)} (pending list {len(pending)})")
    if "--update" in argv:
        keep = sorted(now & pending) if pending else sorted(now)
        PENDING.write_text(
            "# client910 files not yet rustfmt-formatted (see fmt-ratchet.py). Shrink only.\n"
            + "".join(f"{f}\n" for f in keep)
        )
        print(f"wrote {PENDING.relative_to(ROOT)} ({len(keep)} files)")
        if new and pending:
            print("not added (format these):", *new, sep="\n  ")
            return 1
        return 0
    for f in fixed:
        print(f"  now formatted, drop from fmt-pending.txt: {f}")
    for f in new:
        print(f"  UNFORMATTED (run rustfmt --edition 2021 on it): {f}")
    return 1 if new else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
