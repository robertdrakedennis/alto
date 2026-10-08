#!/usr/bin/env python3
"""fn-hash move check (code-quality programme, Phase 0 gate).

Hashes every item body (fn, impl header, struct, enum, const, static, type,
trait, macro_rules, item-level macro call) of client910 and native910 with a
real Rust parser (`syn`, in the helper crate `tools/refactor/rsscan`) and
compares two snapshots. A pure-move refactor (git mv, test extraction, crate
extraction with facades) must report zero code changes.

What the `code` hash ignores (see rsscan/src/main.rs):
  * whitespace, formatting, and all comments;
  * doc comments / `#[doc]` (hashed separately: `doc`);
  * visibility (`pub`, `pub(crate)`, `pub(super)`, `pub(in ..)`);
  * path prefixes `crate::`, `super::`, `self::`, `$crate::`, `client910::`,
    `rs910_*::` (so moving a module or crate does not change callers);
  * `use` differences: a single-name path is expanded through the enclosing
    module's (and fn body's) `use` declarations before hashing, and `use`
    statements inside fn bodies are dropped. `use super::*` globs are not
    resolved (they stay as written; moves keep them).

Non-doc comments (the `File.java:line` cites) are hashed separately
(`comments`, including the comment lines directly above an item). A comment
or doc change is reported as a warning; `--strict` makes it fail.

Identity is location-independent: `kind` + `key`, where key is the item name
qualified by its impl/trait context with paths reduced to the last segment
(`fn Engine::trap_context`, `fn Host for Engine::call`, `struct Pack`). The
same key in several modules (17 byte readers, `tests::roundtrip`, ...) is a
multiset: rows are paired by equal code hash first, and only the leftovers
are reported.

Usage:
  fn-hash.py snapshot OUT.tsv [--rev GIT_REV] [--root label=dir ...]
  fn-hash.py compare BEFORE.tsv AFTER.tsv [--strict] [--allow GLOB ...]
             [--kinds fn,struct,...] [--show-moves]
  fn-hash.py check GIT_REV [--strict] [--allow GLOB ...] [--renames]
      snapshot GIT_REV (via `git archive`) and the working tree, then compare.

`--renames` (compare and check) is the rename-aware mode for the "our own
client" rename lanes. Every item also has a `shape` hash: its `code` tokens with
each distinct identifier (locals, params, fields, called fn/method names, type
names) replaced by its first-occurrence index; keywords, primitive/prelude
names, macro names, attributes, literals and punctuation are kept exactly. An
item whose code hash changed but whose (kind, shape) matches an item that
disappeared is reported as `renamed old -> new` and does not fail. A rename is
refused (the item stays a failure) when
  * it reuses a name the item already had for something else (an operand or
    argument swap such as `self.a - self.b` -> `self.b - self.a`), or
  * a name is both the source and the target of renames in the run (a swap
    or cycle across items), or
  * a global name (field, method, type, fn) is renamed to different names in
    different items (only the plurality mapping is kept), or
  * a type-level name is renamed onto a type name that already existed
    (`impl A` -> `impl B` re-targets instead of renaming).
Any other change fails as before. The mapping of identifiers is printed
(`--show-renames` prints one line per item); ambiguous ones (one old name
mapped to several new names) are listed as warnings. `--rename-list FILE`
(`old new` per line) additionally requires every identifier rename to be listed.
Limit: a rename onto a different but equally shaped std name (`min` -> `max`)
is only visible in the mapping table, so use --rename-list when the lane knows
its renames.

Exit status of compare/check: 0 identical (warnings allowed), 1 code changes
(changed, disappeared, appeared, or re-keyed bodies), 2 usage/parse error.
"""
import argparse
import fnmatch
import os
import re
import subprocess
import sys
import tempfile
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
RSSCAN = HERE / "rsscan"

# label -> repo-relative directory. New crates under tools/client910/crates
# (Phase 2) and a tools/ workspace are picked up automatically.
DEFAULT_ROOTS = [
    ("client910", "tools/client910/src"),
    ("client910-tests", "tools/client910/tests"),
    ("client910-crates", "tools/client910/crates"),
    ("native910", "tools/native910/src"),
    ("native910-tests", "tools/native910/tests"),
    ("native910-examples", "tools/native910/examples"),
]


def rsscan_bin():
    """Build (if needed) and return the rsscan helper binary."""
    cmd = ["cargo", "build", "--release", "--quiet", "--message-format=json",
           "--manifest-path", str(RSSCAN / "Cargo.toml")]
    if os.environ.get("CARGO_NET_OFFLINE") or os.environ.get("RSSCAN_OFFLINE"):
        cmd.append("--offline")
    proc = subprocess.run(cmd, cwd=ROOT, stdout=subprocess.PIPE, text=True)
    if proc.returncode != 0:
        sys.exit("building tools/refactor/rsscan failed")
    import json
    exe = None
    for line in proc.stdout.splitlines():
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("reason") == "compiler-artifact" and msg.get("executable"):
            exe = msg["executable"]
    if not exe:
        sys.exit("could not locate the rsscan binary")
    return exe


def run_items(roots, base):
    args = [f"{label}={base / rel}" for label, rel in roots if (base / rel).exists()]
    proc = subprocess.run([rsscan_bin(), "items", *args], stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True)
    sys.stderr.write(proc.stderr)
    if proc.returncode not in (0, 2):
        sys.exit("rsscan items failed")
    return proc.stdout


def snapshot(out, rev=None, roots=None):
    roots = roots or DEFAULT_ROOTS
    if rev:
        with tempfile.TemporaryDirectory(prefix="fnhash-") as tmp:
            paths = [rel for _, rel in roots
                     if subprocess.run(["git", "cat-file", "-e", f"{rev}:{rel}"], cwd=ROOT,
                                       stderr=subprocess.DEVNULL).returncode == 0]
            arch = subprocess.run(["git", "archive", "--format=tar", rev, *paths], cwd=ROOT,
                                  stdout=subprocess.PIPE, check=True).stdout
            subprocess.run(["tar", "-x", "-C", tmp], input=arch, check=True)
            text = run_items(roots, Path(tmp))
    else:
        text = run_items(roots, ROOT)
    Path(out).write_text(text)
    n = sum(1 for l in text.splitlines() if l and not l.startswith("#"))
    errs = [l for l in text.splitlines() if l.startswith("#parse-error")]
    print(f"fn-hash: wrote {out} ({n} items{', %d parse errors' % len(errs) if errs else ''})")
    return 2 if errs else 0


def load(path):
    rows = []
    for line in Path(path).read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        f = line.split("\t")
        kind, key, code, doc, com, ntok, loc, modpath = f[:8]
        # v1 snapshots (no shape column) still load; renames then never match.
        shape = f[8] if len(f) > 8 else "v1:" + code
        raw = [] if len(f) < 10 or f[9] == "-" else f[9].split(" ")
        idents = [x.rstrip("@") for x in raw]
        glob = {x[:-1] for x in raw if x.endswith("@")}
        rows.append(dict(kind=kind, key=key, code=code, doc=doc, com=com,
                         ntok=int(ntok), loc=loc, mod=modpath, shape=shape,
                         idents=idents, glob=glob))
    return rows


KEY_IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def key_idents(key):
    return KEY_IDENT.findall(key)


def match_renames(candidates, rename_list=None, before_globals=frozenset()):
    """Validate (before_row, after_row) pairs that have an equal shape.

    `before_globals` are the global names of the before snapshot.

    Returns (accepted, rejected, table): accepted pairs carry the identifier
    mapping; table is {old: {new: count}} of the accepted mappings.
    """
    info = []
    for rb, ra in candidates:
        old, new = rb["idents"], ra["idents"]
        mapping = {}
        ok = len(old) == len(new)
        if ok:
            # Key-level names (impl type, fn name) are part of the rename too.
            ko, kn = key_idents(rb["key"]), key_idents(ra["key"])
            pairs = [(o, n) for o, n in zip(old, new) if o != n]
            if len(ko) == len(kn):
                pairs += [(o, n) for o, n in zip(ko, kn) if o != n]
            for o, n in pairs:
                if mapping.setdefault(o, n) != n:
                    ok = False
            # A new name must not be one the item already used for something
            # else (operand / argument swaps normalise to the same shape).
            if ok and (set(mapping.values()) & set(old) or set(mapping) & set(new)):
                ok = False
        if ok:
            # Renaming a type/variant/const onto a type-level name that already
            # existed re-targets the item instead (`impl A` -> `impl B`), unless
            # the rename list names the pair.
            ok = all((o, n) in (rename_list or ())
                     or not (o in rb["glob"] and n[:1].isupper() and n in before_globals)
                     for o, n in mapping.items())
        if ok and rename_list is not None:
            ok = all((o, n) in rename_list for o, n in mapping.items())
        info.append((rb, ra, mapping, ok))
    # Two run-wide consistency rules; refusing an item can expose the next
    # violation, so repeat until stable.
    #  * a name that is both a rename source and a rename target means a swap
    #    or a cycle: refuse every rename that touches it;
    #  * a global name (field, method, type, fn: see rsscan `shape_of`) renamed
    #    to several new names: keep only the plurality mapping (all of them on
    #    a tie). This catches `self.a - self.b` -> `self.q - self.p` hiding
    #    behind a consistent a->p, b->q rename. Bare locals/params may differ
    #    per function and are only listed as warnings.
    while True:
        live = [(rb, ra, m) for rb, ra, m, ok in info if ok]
        srcs = {o for _, _, m in live for o in m}
        dsts = {n for _, _, m in live for n in m.values()}
        bad = srcs & dsts
        votes = defaultdict(lambda: defaultdict(int))
        for rb, _, m in live:
            for o, n in m.items():
                if o in rb["glob"]:
                    votes[o][n] += 1
        minority = set()  # (old, new) mappings that lost the vote
        for o, ns in votes.items():
            if len(ns) > 1:
                top = max(ns.values())
                winners = [n for n, c in ns.items() if c == top]
                keep = winners[0] if len(winners) == 1 else None
                minority |= {(o, n) for n in ns if n != keep}
        if not bad and not minority:
            break
        info = [(rb, ra, m, ok and not (set(m) | set(m.values())) & bad
                 and not any(pair in minority for pair in m.items()))
                for rb, ra, m, ok in info]
    accepted, rejected = [], []
    table = defaultdict(lambda: defaultdict(int))
    for rb, ra, m, ok in info:
        if ok:
            accepted.append((rb, ra, m))
            for o, n in m.items():
                table[o][n] += 1
        else:
            rejected.append((rb, ra))
    return accepted, rejected, table


def pair_by_shape(before_rows, after_rows):
    """Pair rows of equal (kind, shape); returns (pairs, left_before, left_after).
    Equal groups are zipped in (file, line) order."""
    def order(r):
        f, _, l = r["loc"].rpartition(":")
        return (f, int(l))
    gb, ga = defaultdict(list), defaultdict(list)
    for r in before_rows:
        gb[(r["kind"], r["shape"])].append(r)
    for r in after_rows:
        ga[(r["kind"], r["shape"])].append(r)
    pairs, lb, la = [], [], []
    for k in sorted(set(gb) | set(ga)):
        b, a = sorted(gb.get(k, []), key=order), sorted(ga.get(k, []), key=order)
        n = min(len(b), len(a))
        pairs += list(zip(b[:n], a[:n]))
        lb += b[n:]
        la += a[n:]
    return pairs, lb, la


def load_rename_list(path):
    out = set()
    for line in Path(path).read_text().splitlines():
        line = line.split("#", 1)[0].replace("->", " ").split()
        if len(line) == 2:
            out.add((line[0], line[1]))
    return out


def compare(before_path, after_path, strict=False, allow=(), kinds=None, show_moves=False,
            renames=False, show_renames=False, rename_list=None):
    before, after = load(before_path), load(after_path)
    if kinds:
        before = [r for r in before if r["kind"] in kinds]
        after = [r for r in after if r["kind"] in kinds]
    gb, ga = defaultdict(list), defaultdict(list)
    for r in before:
        gb[(r["kind"], r["key"])].append(r)
    for r in after:
        ga[(r["kind"], r["key"])].append(r)

    same, moved, doc_changed, com_changed, changed = [], [], [], [], []
    left_b, left_a = [], []
    for k in sorted(set(gb) | set(ga)):
        b, a = list(gb.get(k, [])), list(ga.get(k, []))
        # 1. identical code: same file first (so a same-named twin elsewhere
        #    is not mistaken for the moved copy), then anywhere; within each,
        #    prefer identical doc+comments
        def fileof(r):
            return r["loc"].rsplit(":", 1)[0]
        for same_file, exact in ((True, True), (True, False), (False, True), (False, False)):
            for rb in list(b):
                cands = [ra for ra in a if ra["code"] == rb["code"]
                         and (not same_file or fileof(ra) == fileof(rb))
                         and (not exact or (ra["doc"], ra["com"]) == (rb["doc"], rb["com"]))]
                if not cands:
                    continue
                cands.sort(key=lambda ra: (ra["mod"] != rb["mod"], ra["loc"]))
                ra = cands[0]
                b.remove(rb)
                a.remove(ra)
                same.append((rb, ra))
                if rb["doc"] != ra["doc"]:
                    doc_changed.append((rb, ra))
                if rb["com"] != ra["com"]:
                    com_changed.append((rb, ra))
                if fileof(rb) != fileof(ra):
                    moved.append((rb, ra))
        # 2. same key, different code: changed (pair by module path, then order)
        b.sort(key=lambda r: (r["mod"], r["loc"]))
        a.sort(key=lambda r: (r["mod"], r["loc"]))
        for rb in list(b):
            ra = (next((x for x in a if x["loc"].rsplit(":", 1)[0] == rb["loc"].rsplit(":", 1)[0]), None)
                  or next((x for x in a if x["mod"] == rb["mod"]), None) or (a[0] if a else None))
            if ra is None:
                break
            b.remove(rb)
            a.remove(ra)
            changed.append((rb, ra))
        left_b += b
        left_a += a

    # 3. identical body under a different key (rename / re-keyed)
    rekeyed = []
    by_code = defaultdict(list)
    for ra in left_a:
        by_code[(ra["kind"], ra["code"])].append(ra)
    gone = []
    for rb in left_b:
        c = by_code.get((rb["kind"], rb["code"]))
        if c and rb["ntok"] > 8:
            ra = c.pop(0)
            left_a.remove(ra)
            rekeyed.append((rb, ra))
        else:
            gone.append(rb)
    appeared = left_a

    renamed, rename_table, rename_rejected = [], {}, 0
    if renames:
        before_globals = set().union(*(r["glob"] for r in before)) if before else set()
        # 1. same key, body differs only by identifiers: renamed in place.
        same_shape = [(rb, ra) for rb, ra in changed if rb["shape"] == ra["shape"]]
        broken = [(rb, ra) for rb, ra in changed if rb["shape"] != ra["shape"]]
        # 2. rows that vanished or appeared (and the halves of broken changed
        #    pairs) pair by (kind, shape).
        pairs, _, _ = pair_by_shape(gone + [rb for rb, _ in broken],
                                    appeared + [ra for _, ra in broken])
        acc, rej, table = match_renames(same_shape + pairs, rename_list, before_globals)
        # 3. re-keyed items (identical body, new key) are renames only when
        #    every changed name in the key is a rename accepted above.
        rekeyed_bad = []
        for rb, ra in rekeyed:
            ko, kn = key_idents(rb["key"]), key_idents(ra["key"])
            if len(ko) == len(kn) and all(o == n or n in table.get(o, {}) for o, n in zip(ko, kn)):
                acc.append((rb, ra, {o: n for o, n in zip(ko, kn) if o != n}))
            else:
                rekeyed_bad.append((rb, ra))
        renamed, rename_table, rekeyed = acc, table, rekeyed_bad
        rename_rejected = len(rej)
        used_b = {id(rb) for rb, _, _ in acc}
        used_a = {id(ra) for _, ra, _ in acc}
        new_changed = [(rb, ra) for rb, ra in same_shape if id(rb) not in used_b]
        broken_b = {id(rb) for rb, _ in broken}
        broken_a = {id(ra) for _, ra in broken}
        for rb, ra in broken:
            if id(rb) not in used_b and id(ra) not in used_a:
                new_changed.append((rb, ra))
        gone = [r for r in gone + [rb for rb, _ in broken]
                if id(r) not in used_b and not (id(r) in broken_b and any(
                    x is r and id(y) not in used_a for x, y in broken))]
        appeared = [r for r in appeared + [ra for _, ra in broken]
                    if id(r) not in used_a and not (id(r) in broken_a and any(
                        y is r and id(x) not in used_b for x, y in broken))]
        changed = new_changed

    def allowed(r):
        return any(fnmatch.fnmatchcase(f"{r['kind']} {r['key']}", g) or fnmatch.fnmatchcase(r["key"], g)
                   for g in allow)

    def fmt(r):
        return f"{r['kind']} {r['key']}  [{r['loc']}]"

    fails = 0
    print(f"fn-hash compare: {len(before)} -> {len(after)} items; "
          f"{len(same)} identical ({len(moved)} moved file), {len(changed)} changed, "
          f"{len(rekeyed)} re-keyed, {len(gone)} disappeared, {len(appeared)} appeared; "
          f"{len(doc_changed)} doc-only, {len(com_changed)} comment-only changes")
    if renames:
        print(f"fn-hash renames: {len(renamed)} items renamed "
              f"({len(rename_table)} distinct old names renamed; "
              f"{rename_rejected} shape matches refused as swaps/cycles/unlisted)")
        if renamed:
            print("\nRENAMED (same shape, identifiers differ; no failure):")
            shown = renamed if show_renames else renamed[:10]
            for rb, ra, m in shown:
                detail = ", ".join(f"{o}->{n}" for o, n in m.items())
                print(f"  renamed {fmt(rb)}\n      -> {ra['kind']} {ra['key']}  [{ra['loc']}]"
                      f"{'  {' + detail + '}' if detail else ''}")
            if len(shown) < len(renamed):
                print(f"  ... {len(renamed) - len(shown)} more (--show-renames lists all)")
            print("\nrename table (old -> new x items):")
            amb = []
            for o in sorted(rename_table):
                news = rename_table[o]
                if len(news) > 1:
                    amb.append(o)
                print("  " + o + " -> " + ", ".join(f"{n} x{c}" for n, c in sorted(news.items())))
            if amb:
                print("  warning: one old name renamed to several new names: " + ", ".join(amb))
    sections = [
        ("CHANGED (same key, different body)", changed, True),
        ("RE-KEYED (identical body, different name/impl)", rekeyed, True),
    ]
    for title, pairs, is_fail in sections:
        if not pairs:
            continue
        print(f"\n{title}:")
        for rb, ra in pairs:
            tag = "allowed " if allowed(rb) or allowed(ra) else ""
            if is_fail and not tag:
                fails += 1
            print(f"  {tag}{fmt(rb)}\n      -> {ra['kind']} {ra['key']}  [{ra['loc']}]")
    for title, rows in (("DISAPPEARED", gone), ("APPEARED", appeared)):
        if not rows:
            continue
        print(f"\n{title}:")
        for r in rows:
            tag = "allowed " if allowed(r) else ""
            if not tag:
                fails += 1
            print(f"  {tag}{fmt(r)}")
    for title, pairs in (("doc changed (warning)", doc_changed), ("comments changed (warning)", com_changed)):
        if not pairs:
            continue
        print(f"\n{title}:")
        for rb, ra in pairs:
            print(f"  {fmt(rb)} -> [{ra['loc']}]")
            if strict and not (allowed(rb) or allowed(ra)):
                fails += 1
    if show_moves and moved:
        print("\nmoved (identical):")
        for rb, ra in moved:
            print(f"  {fmt(rb)} -> [{ra['loc']}]")
    if fails:
        print(f"\nfn-hash: {fails} differences. A pure move must show none; list intended "
              "ones with --allow 'fn key' (glob).")
        return 1
    print("fn-hash: no code differences.")
    return 0


def parse_roots(items):
    if not items:
        return None
    return [tuple(x.split("=", 1)) for x in items]


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("snapshot")
    s.add_argument("out")
    s.add_argument("--rev")
    s.add_argument("--root", action="append", help="label=repo-relative dir (repeatable)")
    for name in ("compare", "check"):
        c = sub.add_parser(name)
        if name == "compare":
            c.add_argument("before")
            c.add_argument("after")
        else:
            c.add_argument("rev")
            c.add_argument("--root", action="append")
        c.add_argument("--strict", action="store_true", help="doc/comment changes also fail")
        c.add_argument("--allow", action="append", default=[], help="glob over 'kind key' or key")
        c.add_argument("--kinds", help="comma list, e.g. fn")
        c.add_argument("--show-moves", action="store_true")
        c.add_argument("--renames", action="store_true",
                       help="rename-aware: items differing only by identifiers do not fail")
        c.add_argument("--show-renames", action="store_true", help="list every renamed item")
        c.add_argument("--rename-list", help="file of `old new` pairs; other identifier renames fail")
    a = ap.parse_args(argv)
    if a.cmd == "snapshot":
        return snapshot(a.out, a.rev, parse_roots(a.root))
    kinds = set(a.kinds.split(",")) if a.kinds else None
    rl = load_rename_list(a.rename_list) if a.rename_list else None
    if a.cmd == "compare":
        return compare(a.before, a.after, a.strict, a.allow, kinds, a.show_moves,
                       a.renames, a.show_renames, rl)
    with tempfile.TemporaryDirectory(prefix="fnhash-") as tmp:
        b, w = Path(tmp) / "before.tsv", Path(tmp) / "after.tsv"
        roots = parse_roots(a.root)
        if snapshot(b, a.rev, roots) or snapshot(w, None, roots):
            return 2
        return compare(b, w, a.strict, a.allow, kinds, a.show_moves,
                       a.renames, a.show_renames, rl)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
