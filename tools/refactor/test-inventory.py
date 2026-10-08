#!/usr/bin/env python3
"""Test-inventory lock (code-quality programme, Phase 0 gate).

Lists every test of client910 (all default test targets: since Phase 1.1 the
client910 lib; its bins have `test = false` and the lib `doctest = false`),
of the rs910-* crates split out of it (Phase 2; target `rs910-core/lib:...`)
and of native910 (lib, bin, tests/*), with its ignored status, plus
the server vitest tests (static scan, see below), and compares the list with
the committed baseline `test-inventory.txt`.

The check FAILS when
  * a baseline test disappears, or
  * a test runs in fewer configurations than before (run -> ignored,
    run -> ignored under no-pack, no-pack -> ignored),
unless the test is listed in `test-removals.txt` with a reason. New tests
and un-ignored tests only print a notice; run `--update` to record them.

Identity is location-independent, so refactors can move tests freely:
  1. tests are first paired by their full libtest path (in any target or
     crate of the same suite family);
  2. leftovers are paired by their final path segment (the fn name), so
     `app::tests::x` -> `app_tests::x`, or a move into another crate, is a
     move, not a disappearance;
  3. a collision (the same fn name leftover in several places, e.g. many
     `tests::roundtrip`) is a multiset: the check fails only if fewer
     copies remain than before, and it prints every candidate path on both
     sides so the reviewer can see which one went.
Doctests are keyed by their item path (file and line are dropped).

Status column:
  run      runs with default features and with --features no-pack
  nopack   runs by default, reported ignored under --features no-pack
           (needs server/data/pack; CI has no pack)
  ignored  #[ignore] in both configurations
The baseline is generated locally with both configurations (`--mode both`,
the default). CI has no pack and runs `--mode no-pack`: it cannot tell `run`
from `nopack` for tests it sees ignored, so there it checks names and that
no `run` test became ignored; the full status comparison happens in the
local gate (tools/refactor/gate.sh).

Reusing the gate's build (tools/refactor/gate.sh): `--nextest-list FILE`
reads the default-feature listing that the tests step wrote with
`cargo nextest list --message-format json`, so the inventory builds nothing.
`--nopack static` reads the no-pack status from the source instead of a
second (`--features no-pack`) build: the feature only switches
`#[cfg_attr(feature = "no-pack", ignore ...)]` on tests, and `rsscan tests`
lists every test fn with that attribute under its libtest path. The scan
refuses (and the inventory fails) when a crate uses the feature in any
other way, so it is exact; `--nopack check` runs both and fails on any
difference. `--packages a,b` limits the Rust suites (listing and baseline)
to those packages, for a gate that only tested the packages a change
affects.

Usage:
  test-inventory.py [--update] [--mode both|default|no-pack] [--no-server]
                    [--native-profile release|dev] [--dump FILE]
                    [--nextest-list FILE] [--nopack build|static|check]
                    [--packages a,b,...]
"""
import argparse
import fnmatch
import json
import os
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
BASELINE = HERE / "test-inventory.txt"
REMOVALS = HERE / "test-removals.txt"
PACKAGES = [
    # (suite, manifest, profile key). The client suite is client910 plus the
    # rs910-* crates of the tools/ workspace (Phase 2, see workspace.py);
    # `cargo metadata` lists every member, so list_package keeps only the
    # suite's own packages.
    ("client910", ROOT / "tools/client910/Cargo.toml", "client"),
    ("native910", ROOT / "tools/native910/Cargo.toml", "native"),
]
sys.path.insert(0, str(HERE))
import workspace  # noqa: E402


def suite_packages(key):
    """The package names a suite lists."""
    if key == "native":
        return {"native910"}
    return {name for name, _, _ in workspace.client_packages()}
RANK = {"run": 2, "nopack": 1, "ignored": 0}


VERBOSE = bool(os.environ.get("TEST_INVENTORY_VERBOSE"))


def log(msg):
    if VERBOSE:
        print(msg, file=sys.stderr, flush=True)


def cargo(args, manifest, capture=True):
    args = list(args)
    tail = []
    if "--" in args:
        i = args.index("--")
        args, tail = args[:i], args[i:]
    cmd = ["cargo", *args, "--manifest-path", str(manifest)]
    if not VERBOSE and args and args[0] == "test":
        cmd.append("--quiet")
    if os.environ.get("CARGO_NET_OFFLINE"):
        cmd.append("--offline")
    cmd += tail
    log("+ " + " ".join(cmd))
    return subprocess.run(cmd, cwd=ROOT, stdout=subprocess.PIPE if capture else None, text=True)


def features_for(manifest):
    proc = cargo(["metadata", "--no-deps", "--format-version", "1"], manifest)
    if proc.returncode != 0:
        sys.exit("cargo metadata failed")
    meta = json.loads(proc.stdout)
    out = []
    for p in meta["packages"]:
        out.append((p["name"], set(p["features"]), p["targets"], p["manifest_path"]))
    return out


def parse_list(text):
    tests = []
    for line in text.splitlines():
        m = re.match(r"^(.*): (test|benchmark)$", line)
        if m:
            tests.append(m.group(1))
    return tests


def nextest_listing(path, wanted):
    """{(target, name): ignored} from `cargo nextest list --message-format json`."""
    data = json.loads(Path(path).read_text())
    out = {}
    for suite in data["rust-suites"].values():
        pkg = workspace.package_name(suite["package-id"])
        if pkg not in wanted:
            continue
        target = f"{pkg}/{suite['kind']}:{suite['binary-name']}"
        for name, case in suite["testcases"].items():
            out[(target, name)] = bool(case["ignored"])
    return out


def rsscan_bin():
    import importlib.util
    spec = importlib.util.spec_from_file_location("fnhash", HERE / "fn-hash.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.rsscan_bin()


def static_nopack(manifest, wanted):
    """{(target, name)} of the tests a `--features no-pack` build reports
    ignored, read from the source (see the module docstring)."""
    rsscan = rsscan_bin()
    out = set()
    for pkg, features, targets, _ in features_for(manifest):
        if pkg not in wanted:
            continue
        for t in targets:
            kind = t["kind"][0]
            if not t.get("test") or kind not in ("lib", "bin", "test"):
                continue
            proc = subprocess.run([rsscan, "tests", t["src_path"]], stdout=subprocess.PIPE, text=True)
            if proc.returncode != 0:
                sys.exit(f"rsscan tests failed for {pkg} {kind}:{t['name']}")
            counts, flagged = {}, []
            for line in proc.stdout.splitlines():
                cols = line.split("\t")
                if cols[0] == "test" and cols[3] == "1":
                    flagged.append(cols[1])
                elif cols[0] in ("nopack-attrs", "nopack-tokens"):
                    counts[cols[0]] = int(cols[1])
            if counts.get("nopack-attrs") != counts.get("nopack-tokens"):
                sys.exit(f"{pkg} {kind}:{t['name']}: `feature = \"no-pack\"` appears {counts.get('nopack-tokens')} times "
                         f"but only {counts.get('nopack-attrs')} are the canonical test attribute "
                         f"`#[cfg_attr(feature = \"no-pack\", ignore ...)]`; the no-pack status cannot be read "
                         f"from the source. Use the canonical attribute, or run with --nopack build.")
            if "no-pack" not in features:
                continue  # the attribute never switches on in this package
            for name in flagged:
                out.add((f"{pkg}/{kind}:{t['name']}", name))
    return out


def list_package(manifest, mode, profile, key, wanted=None):
    """{(target, name): ignored} for one suite's packages in one feature mode.
    The packages build in one cargo invocation per feature set (so cargo
    builds them in parallel), then every test binary lists its tests."""
    feats = ["--features", "no-pack"] if mode == "no-pack" else []
    prof = ["--release"] if profile == "release" else []
    out = {}
    wanted = suite_packages(key) if wanted is None else wanted
    pkgs = [p for p in features_for(manifest) if p[0] in wanted]
    groups = defaultdict(list)
    for pkg in pkgs:
        groups[tuple(feats) if (mode != "no-pack" or "no-pack" in pkg[1]) else ()].append(pkg)
    for feats_p, members in groups.items():
        names = {m[0] for m in members}
        sel = [a for m in members for a in ("-p", m[0])]
        proc = cargo(["test", "--no-run", "--message-format=json", *sel, *prof, *feats_p], manifest)
        if proc.returncode != 0:
            sys.exit(f"cargo test --no-run failed for {', '.join(sorted(names))} ({mode})")
        exes = []
        for line in proc.stdout.splitlines():
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            pkg = workspace.package_name(msg.get("package_id", ""))
            if (msg.get("reason") == "compiler-artifact" and msg.get("executable")
                    and msg.get("profile", {}).get("test") and pkg in names):
                t = msg["target"]
                exes.append((pkg, f"{t['kind'][0]}:{t['name']}", msg["executable"]))
        dirs = {m[0]: Path(m[3]).parent for m in members}
        for pkg, target, exe in sorted(set(exes)):
            full = subprocess.run([exe, "--list"], stdout=subprocess.PIPE, text=True, cwd=dirs[pkg])
            ign = subprocess.run([exe, "--list", "--ignored"], stdout=subprocess.PIPE, text=True, cwd=dirs[pkg])
            if full.returncode != 0 or ign.returncode != 0:
                sys.exit(f"{exe} --list failed")
            ignored = set(parse_list(ign.stdout))
            for name in parse_list(full.stdout):
                out[(f"{pkg}/{target}", name)] = name in ignored
        # doctests of library targets
        for pkg, _, targets, _ in members:
            if any("lib" in t["kind"] and t.get("doctest") for t in targets):
                full = cargo(["test", "--doc", "-p", pkg, *prof, *feats_p, "--", "--list"], manifest)
                ign = cargo(["test", "--doc", "-p", pkg, *prof, *feats_p, "--", "--list", "--ignored"], manifest)
                ignored = {doc_name(n) for n in parse_list(ign.stdout)}
                for n in parse_list(full.stdout):
                    out[(f"{pkg}/doc", doc_name(n))] = doc_name(n) in ignored
    return out


def doc_name(n):
    # "src/vm.rs - vm::Host::call (line 12)" -> "doc::vm::Host::call"
    n = re.sub(r"\s*\(line \d+\)(\s*-\s*compile( fail)?)?$", "", n)
    item = n.split(" - ", 1)[-1]
    return "doc::" + item


# ---------------------------------------------------------------------------
# inventory
# ---------------------------------------------------------------------------

def collect(mode, native_profile, server, nextest_list=None, nopack="build", packages=None):
    """Returns {(suite_target, name): status}."""
    inv = {}
    for suite, manifest, key in PACKAGES:
        wanted = suite_packages(key) if packages is None else suite_packages(key) & packages
        if not wanted:
            continue
        profile = native_profile if key == "native" else "test"
        if mode in ("both", "default"):
            if nextest_list:
                d = nextest_listing(nextest_list, wanted)
            else:
                d = list_package(manifest, "default", profile, key, wanted)
        if mode in ("both", "no-pack"):
            if nopack in ("static", "check") and mode == "both":
                flagged = static_nopack(manifest, wanted)
                missing = sorted(k for k in flagged if k not in d)
                if missing:
                    sys.exit("no-pack scan: tests with the no-pack attribute that the build does not list "
                             f"(path resolution differs from rustc?): {missing[:5]}")
                p = {k: ig or k in flagged for k, ig in d.items()}
            if nopack in ("build", "check") or mode != "both":
                built = list_package(manifest, "no-pack", profile, key, wanted)
                if nopack == "check":
                    diff = sorted(k for k in set(p) | set(built) if p.get(k) != built.get(k))
                    if diff:
                        sys.exit(f"no-pack scan and no-pack build disagree on {len(diff)} tests: {diff[:5]}")
                p = built
        if mode == "both":
            for k in set(d) | set(p):
                # a test compiled in only one configuration counts as
                # ignored in the other
                di, pi = d.get(k, True), p.get(k, True)
                inv[k] = "ignored" if di else ("nopack" if pi else "run")
        elif mode == "default":
            for k, ig in d.items():
                inv[k] = "ignored" if ig else "run"
        else:
            for k, ig in p.items():
                inv[k] = "ignored?" if ig else "run"
    return inv


def leaf(name):
    if name.startswith("doc::"):
        return name
    if " > " in name or "::" not in name:
        return re.sub(r" #\d+$", "", name)
    return name.rsplit("::", 1)[-1]


def family(target):
    # server tests never pair with Rust tests, and vice versa
    return "server" if target.startswith("server/") else "rust"


def load_baseline():
    inv = {}
    if not BASELINE.exists():
        return None
    for line in BASELINE.read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        target, name, status = line.split("\t")
        inv[(target, name)] = status
    return inv


def load_removals():
    rules = []
    if not REMOVALS.exists():
        return rules
    for n, line in enumerate(REMOVALS.read_text().splitlines(), 1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        parts = line.split(None, 1)
        if len(parts) < 2 or not parts[1].strip():
            sys.exit(f"test-removals.txt:{n}: every entry needs a reason: {line!r}")
        rules.append((parts[0], parts[1].strip()))
    return rules


def removal_reason(rules, target, name):
    for pat, reason in rules:
        if (fnmatch.fnmatchcase(name, pat) or fnmatch.fnmatchcase(f"{target}::{name}", pat)
                or fnmatch.fnmatchcase(f"{target.split('/')[0]}::{name}", pat)):
            return reason
    return None


def write_baseline(inv):
    counts = defaultdict(lambda: defaultdict(int))
    for (t, _), s in inv.items():
        counts[t][s] += 1
    head = [
        "# Test-inventory lock (tools/refactor/test-inventory.py). Regenerate with --update",
        "# after adding tests; removing or ignoring one needs an entry in test-removals.txt.",
        "# target\ttest\tstatus (run | nopack = ignored without server/data/pack | ignored)",
    ]
    for t in sorted(counts):
        c = counts[t]
        head.append(f"# {t}: {sum(c.values())} tests ({', '.join(f'{k} {v}' for k, v in sorted(c.items()))})")
    body = [f"{t}\t{n}\t{s}" for (t, n), s in sorted(inv.items())]
    BASELINE.write_text("\n".join(head + body) + "\n")


def compare(base, cur, mode):
    rules = load_removals()
    fails, notes = [], []
    moved = 0
    pairs = []
    # 1. exact full name (any target in the same family)
    bleft = defaultdict(list)
    for (t, n), s in base.items():
        bleft[(family(t), n)].append((t, s))
    cleft = defaultdict(list)
    for (t, n), s in cur.items():
        cleft[(family(t), n)].append((t, s))
    bl, cl = [], []
    for k in set(bleft) | set(cleft):
        b, c = sorted(bleft.get(k, [])), sorted(cleft.get(k, []))
        # same target first
        for tb, sb in list(b):
            m = next((x for x in c if x[0] == tb), None)
            if m:
                b.remove((tb, sb))
                c.remove(m)
                pairs.append(((tb, k[1], sb), (m[0], k[1], m[1])))
        for (tb, sb), (tc, sc) in zip(list(b), list(c)):
            pairs.append(((tb, k[1], sb), (tc, k[1], sc)))
            moved += 1
        n = min(len(b), len(c))
        bl += [(tb, k[1], sb) for tb, sb in b[n:]]
        cl += [(tc, k[1], sc) for tc, sc in c[n:]]
    # 2. leftovers by leaf name (moves across modules/crates)
    gb, gc = defaultdict(list), defaultdict(list)
    for x in bl:
        gb[(family(x[0]), leaf(x[1]))].append(x)
    for x in cl:
        gc[(family(x[0]), leaf(x[1]))].append(x)
    gone, added = [], []
    for k in sorted(set(gb) | set(gc)):
        b, c = sorted(gb.get(k, [])), sorted(gc.get(k, []))
        if (len(b) > 1 or len(c) > 1) and len(b) != len(c):
            notes.append(f"collision on {k[1]!r}: before {[f'{x[0]}::{x[1]}' for x in b]}, "
                         f"after {[f'{y[0]}::{y[1]}' for y in c]}")
        # pair equal status first, so an ambiguous group reports the
        # disappearance rather than a spurious status change
        for same in (True, False):
            for x in list(b):
                y = next((y for y in c if not same or y[2] == x[2]), None)
                if y is None:
                    continue
                b.remove(x)
                c.remove(y)
                pairs.append((x, y))
                moved += 1
        gone += b
        added += c
    for t, n, s in gone:
        r = removal_reason(rules, t, n)
        if r:
            notes.append(f"removed (listed: {r}): {t}::{n}")
        else:
            fails.append(f"DISAPPEARED: {t}::{n} [{s}]")
    for t, n, s in added:
        notes.append(f"new test: {t}::{n} [{s}]")
    # 3. status regressions
    for (tb, nb, sb), (tc, nc, sc) in pairs:
        if mode == "no-pack":
            # CI: `ignored?` = ignored under no-pack (baseline nopack/ignored)
            if sc == "ignored?" and sb == "run":
                r = removal_reason(rules, tc, nc)
                (notes if r else fails).append(
                    f"{'ignored (listed: ' + r + ')' if r else 'NEWLY IGNORED under no-pack'}: {tc}::{nc}")
            elif sc == "run" and sb != "run":
                notes.append(f"now runs without the pack (was {sb}): {tc}::{nc}")
            continue
        if mode == "default":
            sc_rank = RANK["run"] if sc == "run" else RANK["ignored"]
            sb_rank = RANK["run"] if sb in ("run", "nopack") else RANK["ignored"]
        else:
            sc_rank, sb_rank = RANK[sc], RANK[sb]
        if sc_rank < sb_rank:
            r = removal_reason(rules, tc, nc)
            (notes if r else fails).append(
                f"{'status ' + sb + ' -> ' + sc + ' (listed: ' + r + ')' if r else 'RUNS LESS: ' + sb + ' -> ' + sc}: {tc}::{nc}")
        elif sc_rank > sb_rank:
            notes.append(f"runs more ({sb} -> {sc}): {tc}::{nc}")
    return fails, notes, moved, len(added)


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--update", action="store_true")
    ap.add_argument("--mode", choices=["both", "default", "no-pack"], default="both")
    ap.add_argument("--no-server", action="store_true")
    ap.add_argument("--native-profile", choices=["release", "dev"], default="release")
    ap.add_argument("--dump", help="write the current inventory here (baseline format)")
    ap.add_argument("--quiet", action="store_true", help="print only failures and the summary")
    ap.add_argument("--nextest-list", help="reuse this `cargo nextest list --message-format json` listing")
    ap.add_argument("--nopack", choices=["build", "static", "check"], default="build")
    ap.add_argument("--packages", help="comma-separated workspace packages to check (default: all)")
    a = ap.parse_args(argv)
    packages = None
    if a.packages is not None:
        packages = {p for p in a.packages.split(",") if p}
    if a.update and packages is not None:
        sys.exit("--update needs every Rust suite (no --packages)")
    cur = collect(a.mode, a.native_profile, not a.no_server, a.nextest_list, a.nopack, packages)
    counts = defaultdict(int)
    for s in cur.values():
        counts[s] += 1
    print(f"test inventory ({a.mode}): {len(cur)} tests "
          f"({', '.join(f'{k} {v}' for k, v in sorted(counts.items()))})")
    if a.dump:
        Path(a.dump).write_text("".join(f"{t}\t{n}\t{s}\n" for (t, n), s in sorted(cur.items())))
    if a.update:
        if a.mode != "both":
            sys.exit("--update needs --mode both (a machine with server/data/pack)")
        base = load_baseline()
        if base is not None:
            fails, _, _, _ = compare(base, cur, a.mode)
            if fails:
                print("refusing to update: fix these or list them in test-removals.txt first:")
                for f in fails:
                    print(f"  {f}")
                return 1
        write_baseline(cur)
        print(f"wrote {BASELINE.relative_to(ROOT)}")
        return 0
    base = load_baseline()
    if base is None:
        print("no baseline: run with --update")
        return 1
    if a.no_server:
        base = {k: v for k, v in base.items() if family(k[0]) != "server"}
    if packages is not None:
        # Only the selected packages were listed: compare their part of the
        # baseline (the others' sources did not change).
        base = {k: v for k, v in base.items() if family(k[0]) == "server" or k[0].split("/")[0] in packages}
    fails, notes, moved, added = compare(base, cur, a.mode)
    if not a.quiet:
        for n in notes:
            print(f"  {n}")
    for f in fails:
        print(f"  FAIL {f}")
    print(f"baseline {len(base)} -> now {len(cur)}; {moved} moved, {added} new, {len(fails)} failures")
    if fails:
        print("test inventory: failed. Restore the test, or list it in tools/refactor/test-removals.txt "
              "with a reason (the removal must be intended and reviewed).")
        return 1
    if added:
        print("test inventory: ok; new tests present, run with --update to lock them.")
    else:
        print("test inventory: ok.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
