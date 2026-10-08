#!/usr/bin/env python3
"""Magic-id ratchet for the client's Rust (docs/symbols.md, "Rules").

`rsscan magic-ids` (tools/refactor/rsscan/src/magic_ids.rs) finds integer
literals standing where a content id goes in client910 and every rs910-*
crate (engine and tests; the generated `rs910-symbols` crate and generated
files are skipped). This script compares the per-file counts with the
checked-in baseline (`magic-id-baseline.txt` next to this file):

* a count that rises, or any hit in a file not in the baseline (so every new
  file), fails: new and touched code names its ids;
* a count that falls also fails until the baseline is lowered with `--update`
  in the same change;
* `--update` rewrites the baseline and refuses to raise any count unless
  `--allow-increase` is given (only when the scan itself changes).

Usage:
    python3 tools/refactor/magic-id-ratchet.py            # check (the gate's magic-ids step)
    python3 tools/refactor/magic-id-ratchet.py --update   # lower the baseline
    python3 tools/refactor/magic-id-ratchet.py --list [path-substring]   # every hit
"""
import collections
import importlib.util
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
CLIENT = ROOT / "tools" / "client910"
BASELINE = HERE / "magic-id-baseline.txt"
GENERATED_CRATE = "rs910-symbols"
HELP = """
An unnamed content id in client Rust. Name it in the content-symbol registry (docs/symbols.md):
  1. npm --prefix server run sym -- find <id or display name> [--type obj|npc|loc|seq|spot|enum|param|struct|inv|varp|varbit|varc|interface|component|script|dbtable|dbrow]
     says what the id is in the cache and whether it already has a name;
  2. npm --prefix server run sym -- add <type> <our_name> <id>   (a component: <interface>.<role> <interface:component>)
     checks it, writes revisions/<rev>/symbols/<type>.sym and regenerates the bindings (TypeScript and rs910-symbols);
  3. use rs910_symbols::{obj, varbit, interface, component, ...} and write obj::OUR_NAME.id()
     (component::<interface>::ROLE.packed(), interface::NAME.id()) instead of the number.
A number the scan mistakes for an id (a transform slot named `component_id`) takes a
`// not a content id` comment on its line."""


def rsscan_bin():
    spec = importlib.util.spec_from_file_location("fnhash", HERE / "fn-hash.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.rsscan_bin()


def roots():
    out = [CLIENT / "src", CLIENT / "tests"]
    out += sorted(p / "src" for p in (CLIENT / "crates").iterdir() if (p / "src").is_dir() and p.name != GENERATED_CRATE)
    return [p for p in out if p.exists()]


def scan():
    proc = subprocess.run([rsscan_bin(), "magic-ids", *map(str, roots())], cwd=ROOT,
                          stdout=subprocess.PIPE, text=True)
    if proc.returncode != 0:
        sys.exit("rsscan magic-ids failed")
    hits = []
    for line in proc.stdout.splitlines():
        parts = line.split("\t")
        if parts[0] == "files":
            continue
        path, lineno, value, name = parts
        hits.append((str(Path(path).resolve().relative_to(ROOT)), int(lineno), value, name))
    return hits


def load_baseline():
    base = {}
    if BASELINE.exists():
        for line in BASELINE.read_text().splitlines():
            if line.strip() and not line.startswith("#"):
                path, n = line.split("\t")
                base[path] = int(n)
    return base


def write_baseline(counts):
    body = [
        "# Unnamed content ids per file in client Rust (see magic-id-ratchet.py). Lower only.",
        "# Columns: file, count (tab separated).",
        "# total %d" % sum(counts.values()),
    ]
    body += ["%s\t%d" % (path, n) for path, n in sorted(counts.items()) if n > 0]
    BASELINE.write_text("\n".join(body) + "\n")


def main(argv):
    hits = scan()
    if "--list" in argv:
        i = argv.index("--list")
        want = argv[i + 1] if i + 1 < len(argv) else ""
        for path, lineno, value, name in hits:
            if want in path:
                print("%s:%d\t%s%s" % (path, lineno, value, "\t(const %s)" % name if name else ""))
        return 0
    counts = collections.Counter(path for path, _, _, _ in hits)
    base = load_baseline()
    worse = sorted((p, base.get(p, 0), n) for p, n in counts.items() if n > base.get(p, 0))
    better = sorted((p, was, counts.get(p, 0)) for p, was in base.items() if counts.get(p, 0) < was)
    print("client Rust magic ids: %d (baseline %d)" % (sum(counts.values()), sum(base.values())))
    if "--update" in argv:
        if worse and "--allow-increase" not in argv:
            for p, was, now in worse:
                print("  would RAISE %s: %d -> %d" % (p, was, now))
            print("refusing to update: name these ids")
            return 1
        write_baseline(counts)
        print("wrote %s" % BASELINE.relative_to(ROOT))
        return 0
    for p, was, now in better:
        print("  improved  %s: %d -> %d" % (p, was, now))
    for p, was, now in worse:
        print("  WORSE     %s: %d -> %d" % (p, was, now))
        for path, lineno, value, name in hits:
            if path == p:
                print("            %s:%d  %s%s" % (path, lineno, value, "  (const %s)" % name if name else ""))
    if worse:
        print(HELP)
        return 1
    if better:
        print("the count fell: lower the baseline with `python3 tools/refactor/magic-id-ratchet.py --update`")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
