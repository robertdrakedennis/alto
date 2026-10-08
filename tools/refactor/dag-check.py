#!/usr/bin/env python3
"""DAG / fence ratchet for client910 (code-quality programme, Phase 0 gate).

Builds the production module dependency graph of client910 with the `syn`
scanner in `tools/refactor/rsscan` (`rsscan deps`: rustc-style `mod` tree
walk from the crate root, every `crate::`/`super::`/`$crate::` reference,
`use` trees, paths inside macros; `#[cfg(test)]` modules and items are
excluded), attributes nested and `#[path]` modules to their top-level module,
and checks it against `layers.txt`:

* cycles: strongly connected components of the module graph;
* layer violations: a module edge whose target crate (per `[modules]`) is not
  the same crate or reachable from the source crate through `[crates]`
  (target-architecture.md §2.2) — i.e. it points up or sideways;
* fences: `wgpu`/`winit`/`tokio`/`cpal` named outside the crates allowed by
  `[fences]`;
* package level (`cargo metadata`): the same fence and crate-edge rules over
  the real Cargo packages, so the check keeps working once Phase 1-3 split
  client910 into `rs910-*` crates;
* real crates (Phase 2): every `rs910-*` member of the tools/ workspace is
  scanned from its own `lib.rs`; its top-level modules join the graph under
  their names (client910 reaches them through `rs910_core::m` paths or the
  `pub use` facades in its lib.rs), must be mapped to that crate in
  layers.txt, and the crate may only depend on the external crates its
  `[externals]` entry allows (`rs910-core:` = std only).

It is a ratchet (like clippy-ratchet.py): `dag-baseline.txt` records today's
SCCs and violations. The check fails when
  * a module that was not in a cycle joins one, or two baseline SCCs merge
    (every current SCC must be a subset of one baseline SCC);
  * a layer edge, fence use or package edge appears that is not in the
    baseline;
  * a production module is not classified in layers.txt.
Shrinking is reported; run with --update to lower the baseline.

Usage:
  dag-check.py [--update] [--report] [--json OUT]
"""
import argparse
import fnmatch
import json
import os
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
CLIENT = ROOT / "tools/client910"
sys.path.insert(0, str(HERE))
import workspace  # noqa: E402
LAYERS = HERE / "layers.txt"
BASELINE = HERE / "dag-baseline.txt"
MANIFESTS = [ROOT / "tools/Cargo.toml", CLIENT / "Cargo.toml", ROOT / "tools/native910/Cargo.toml"]


def rsscan_bin():
    sys.path.insert(0, str(HERE))
    import importlib.util
    spec = importlib.util.spec_from_file_location("fnhash", HERE / "fn-hash.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.rsscan_bin()


def load_layers():
    crates, fences, modules, externals = {}, {}, [], {}
    section = None
    for raw in LAYERS.read_text().splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if line.startswith("["):
            section = line.strip("[]")
            continue
        if section == "crates":
            name, deps = line.split(":", 1)
            crates[name.strip()] = deps.split()
        elif section == "fences":
            name, allowed = line.split(":", 1)
            fences[name.strip()] = set(allowed.split())
        elif section == "externals":
            name, allowed = line.split(":", 1)
            externals[name.strip()] = set(allowed.split())
        elif section == "modules":
            pat, crate = line.split()
            if crate not in crates:
                sys.exit(f"layers.txt: module {pat} maps to unknown crate {crate}")
            modules.append((pat, crate))
    closure = {}
    for c in crates:
        seen, stack = set(), [c]
        while stack:
            for d in crates.get(stack.pop(), []):
                if d not in seen:
                    seen.add(d)
                    stack.append(d)
        closure[c] = seen
        if c in seen:
            sys.exit(f"layers.txt: crate table is cyclic at {c}")
    return crates, closure, fences, modules, externals


def crate_of(module, modules):
    for pat, crate in modules:
        if fnmatch.fnmatchcase(module, pat):
            return crate
    return None


def split_crates():
    """[(crate name, lib.rs)] of the rs910-* crates of the tools/ workspace."""
    out = []
    for name, manifest, _ in workspace.client_packages():
        if name != "client910":
            out.append((name, manifest.parent / "src/lib.rs"))
    return out


def scan():
    """Run rsscan deps over the crate root(s); returns (rows, {module: crate}).

    The second value maps the top-level modules of the split rs910-* crates
    to the crate they physically live in. Rows of a split crate's own root
    are renamed from `<root>` to `<crate>`."""
    exe = rsscan_bin()
    src = CLIENT / "src"
    crates = split_crates()
    aliases = []
    for name, lib in crates:
        aliases += ["--crate-alias", f"{name.replace('-', '_')}={lib}"]
    runs = []
    if (src / "lib.rs").exists():
        runs.append((None, [str(src / "lib.rs"), *aliases]))
        runs.append((None, [str(src / "main.rs"), "--alias", "client910", "--names-from",
                            str(src / "lib.rs"), "--prefix", "<root>", *aliases]))
    else:
        runs.append((None, [str(src / "main.rs")]))
    for name, lib in crates:
        others = []
        for other, olib in crates:
            if other != name:
                others += ["--crate-alias", f"{other.replace('-', '_')}={olib}"]
        runs.append((name, [str(lib), *others]))
    rows, home = [], {}
    for crate, args in runs:
        proc = subprocess.run([exe, "deps", *args], stdout=subprocess.PIPE, text=True)
        if proc.returncode != 0:
            sys.exit(f"rsscan deps failed: {' '.join(args)}")
        for l in proc.stdout.splitlines():
            if not l or l.startswith("#"):
                continue
            r = l.split("\t")
            if crate is not None:
                r = [f"<{crate}>" if (i in (1, 2) and v == "<root>") else v
                     for i, v in enumerate(r)]
                if r[0] == "module" and "::" not in r[1]:
                    home[r[1]] = crate
            rows.append(r)
    return rows, home


def tarjan(nodes, adj):
    index, low, on, stack, out = {}, {}, set(), [], []
    counter = [0]
    sys.setrecursionlimit(10000)

    def strong(v):
        index[v] = low[v] = counter[0]
        counter[0] += 1
        stack.append(v)
        on.add(v)
        for w in sorted(adj.get(v, ())):
            if w not in index:
                strong(w)
                low[v] = min(low[v], low[w])
            elif w in on:
                low[v] = min(low[v], index[w])
        if low[v] == index[v]:
            comp = []
            while True:
                w = stack.pop()
                on.discard(w)
                comp.append(w)
                if w == v:
                    break
            out.append(sorted(comp))

    for v in sorted(nodes):
        if v not in index:
            strong(v)
    return out


def cargo_packages():
    pkgs = {}
    for m in MANIFESTS:
        if not m.exists():
            continue
        proc = subprocess.run(["cargo", "metadata", "--no-deps", "--format-version", "1",
                               "--offline", "--manifest-path", str(m)],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if proc.returncode != 0:
            sys.exit(f"cargo metadata failed for {m}:\n{proc.stderr}")
        for p in json.loads(proc.stdout)["packages"]:
            deps = {d["rename"] or d["name"] for d in p["dependencies"] if d["kind"] in (None, "build")}
            pkgs[p["name"]] = deps
    return pkgs


def analyse():
    crates, closure, fences, modules, externals = load_layers()
    rows, home = scan()
    test_mod = {}
    for r in rows:
        # a top-level module is test-only when its declaration is #[cfg(test)]
        if r[0] == "module" and "::" not in r[1]:
            test_mod.setdefault(r[1], r[3] == "1")
    prod = {m for m, t in test_mod.items() if not t}
    errors = [r for r in rows if r[0] == "error"]
    if errors:
        sys.exit("rsscan errors:\n" + "\n".join("\t".join(e) for e in errors))

    adj = defaultdict(set)
    edge_sites = defaultdict(list)
    fence_sites = defaultdict(list)
    for r in rows:
        if r[0] == "edge" and r[4] == "0" and r[1] in prod and r[2] in prod:
            adj[r[1]].add(r[2])
            edge_sites[(r[1], r[2])].append(r[3])
        elif r[0] == "ext" and r[4] == "0" and r[1] in prod:
            fence_sites[(r[2], r[1])].append(r[3])

    def mapped(m):
        # a split crate's root is its own crate
        if m.startswith("<rs910-") and m.endswith(">"):
            return m[1:-1]
        return crate_of(m, modules)

    unmapped = sorted(m for m in prod if mapped(m) is None)
    mc = {m: mapped(m) for m in prod}
    # a module that lives in a real crate must be classified as that crate
    misplaced = sorted((m, c, mc.get(m)) for m, c in home.items()
                       if m in prod and mc.get(m) != c)

    sccs = [c for c in tarjan(prod, adj) if len(c) > 1]
    sccs.sort(key=lambda c: (-len(c), c))

    layer = {}
    for (a, b), sites in edge_sites.items():
        ca, cb = mc.get(a), mc.get(b)
        if ca and cb and ca != cb and cb not in closure[ca]:
            layer[(a, b)] = (ca, cb, sites)

    fence = {}
    for (ext, m), sites in fence_sites.items():
        c = mc.get(m)
        if c and ext in fences and c not in fences[ext]:
            fence[(ext, m)] = (c, sites)

    pkg_fence, pkg_edge, pkg_extern = {}, {}, {}
    ours = set(crates)
    for pkg, deps in cargo_packages().items():
        if pkg not in crates:
            continue
        for d in sorted(deps):
            if d in fences and pkg not in fences[d]:
                pkg_fence[(pkg, d)] = True
            elif d in ours and d not in closure[pkg]:
                pkg_edge[(pkg, d)] = True
            if pkg in externals and d not in ours and d not in externals[pkg]:
                pkg_extern[(pkg, d)] = True

    crate_adj = defaultdict(set)
    for (a, b) in edge_sites:
        if mc.get(a) and mc.get(b) and mc[a] != mc[b]:
            crate_adj[mc[a]].add(mc[b])
    crate_sccs = [c for c in tarjan({c for c in mc.values() if c}, crate_adj) if len(c) > 1]

    return dict(prod=prod, adj=adj, sccs=sccs, layer=layer, fence=fence, pkg_fence=pkg_fence,
                pkg_edge=pkg_edge, pkg_extern=pkg_extern, unmapped=unmapped, misplaced=misplaced,
                home=home, mc=mc, crate_sccs=crate_sccs, nedges=sum(len(v) for v in adj.values()))


def baseline_lines(a):
    out = [
        "# dag-check.py ratchet baseline (lower only; regenerate with --update).",
        f"# modules {len(a['prod'])}, module edges {a['nedges']}, non-trivial SCCs {len(a['sccs'])} "
        f"(sizes {' '.join(str(len(c)) for c in a['sccs'])}), layer violations {len(a['layer'])}, "
        f"fence violations {len(a['fence'])}, package fence {len(a['pkg_fence'])}, package edges {len(a['pkg_edge'])}",
    ]
    out += [f"scc {len(c)} {' '.join(c)}" for c in a["sccs"]]
    out += [f"layer {x} {y}  # {a['layer'][(x, y)][0]} -> {a['layer'][(x, y)][1]}" for x, y in sorted(a["layer"])]
    out += [f"fence {e} {m}  # {a['fence'][(e, m)][0]}" for e, m in sorted(a["fence"])]
    out += [f"pkg-fence {p} {d}" for p, d in sorted(a["pkg_fence"])]
    out += [f"pkg-edge {p} {d}" for p, d in sorted(a["pkg_edge"])]
    return out


def load_baseline():
    base = dict(sccs=[], layer=set(), fence=set(), pkg_fence=set(), pkg_edge=set())
    if not BASELINE.exists():
        return None
    for raw in BASELINE.read_text().splitlines():
        line = raw.split("#", 1)[0].split()
        if not line:
            continue
        kind = line[0]
        if kind == "scc":
            base["sccs"].append(set(line[2:]))
        elif kind == "layer":
            base["layer"].add((line[1], line[2]))
        elif kind == "fence":
            base["fence"].add((line[1], line[2]))
        elif kind == "pkg-fence":
            base["pkg_fence"].add((line[1], line[2]))
        elif kind == "pkg-edge":
            base["pkg_edge"].add((line[1], line[2]))
    return base


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--update", action="store_true")
    ap.add_argument("--report", action="store_true", help="print every SCC, violation and site")
    ap.add_argument("--json", help="write the analysis as JSON")
    args = ap.parse_args(argv)
    a = analyse()

    sizes = [len(c) for c in a["sccs"]]
    print(f"client910 module graph: {len(a['prod'])} modules, {a['nedges']} edges; "
          f"{len(sizes)} non-trivial SCCs (sizes {sizes}); "
          f"{len(a['layer'])} layer violations; {len(a['fence'])} fence violations "
          f"({', '.join(f'{e} {n}' for e, n in sorted(count_by(a['fence'], 0).items()))}); "
          f"packages: {len(a['pkg_fence'])} fence, {len(a['pkg_edge'])} edge, "
          f"{len(a['pkg_extern'])} external-dependency violations; "
          f"{len(a['home'])} modules in split crates; "
          f"crate-level SCCs {[len(c) for c in a['crate_sccs']]}")
    if args.json:
        Path(args.json).write_text(json.dumps({
            "sccs": a["sccs"],
            "layer": [dict(src=x, dst=y, src_crate=v[0], dst_crate=v[1], sites=v[2]) for (x, y), v in sorted(a["layer"].items())],
            "fence": [dict(crate=e, module=m, layer=v[0], sites=v[1]) for (e, m), v in sorted(a["fence"].items())],
            "pkg_fence": sorted(a["pkg_fence"]), "pkg_edge": sorted(a["pkg_edge"]),
            "pkg_extern": sorted(a["pkg_extern"]), "crate_home": a["home"],
            "module_crate": a["mc"], "crate_sccs": a["crate_sccs"],
        }, indent=1))
    if args.report:
        for c in a["sccs"]:
            print(f"  scc[{len(c)}]: {' '.join(c)}")
        for (x, y), (cx, cy, sites) in sorted(a["layer"].items()):
            print(f"  layer {x} -> {y} ({cx} -> {cy}) at {sites[0]}{' +%d' % (len(sites) - 1) if len(sites) > 1 else ''}")
        for (e, m), (c, sites) in sorted(a["fence"].items()):
            print(f"  fence {e} in {m} ({c}) at {sites[0]} +{len(sites) - 1}")

    if a["unmapped"]:
        print("UNMAPPED production modules (add them to tools/refactor/layers.txt [modules]):")
        for m in a["unmapped"]:
            print(f"  {m}")
    for m, c, got in a["misplaced"]:
        print(f"MISPLACED module {m}: it lives in crate {c}, layers.txt maps it to {got}")
    for (pkg, d) in sorted(a["pkg_extern"]):
        print(f"EXTERNAL dependency {pkg} -> {d}: not allowed by layers.txt [externals]")

    if args.update:
        BASELINE.write_text("\n".join(baseline_lines(a)) + "\n")
        print(f"wrote {BASELINE.relative_to(ROOT)}")
        return 1 if a["unmapped"] or a["misplaced"] or a["pkg_extern"] else 0

    base = load_baseline()
    if base is None:
        print("no dag-baseline.txt: run with --update")
        return 1
    fails = []
    for c in a["sccs"]:
        if not any(set(c) <= b for b in base["sccs"]):
            owners = [b for b in base["sccs"] if b & set(c)]
            new = sorted(set(c) - set().union(*owners)) if owners else c
            fails.append(f"new/merged cycle ({len(c)} modules): newly cyclic {new}; "
                         f"merges {len(owners)} baseline SCC(s)")
            for m in new[:10]:
                outs = sorted(x for x in a["adj"][m] if x in c)
                fails.append(f"    {m} -> {outs[:8]}")
    for key, label in (("layer", "layer violation"), ("fence", "fence violation"),
                       ("pkg_fence", "package fence violation"), ("pkg_edge", "package edge violation")):
        for k in sorted(set(a[key]) - base[key]):
            detail = a[key][k]
            where = ""
            if key == "layer":
                where = f" ({detail[0]} -> {detail[1]}) at {', '.join(detail[2][:3])}"
            elif key == "fence":
                where = f" ({detail[0]}) at {', '.join(detail[1][:3])}"
            fails.append(f"new {label}: {k[0]} -> {k[1]}{where}")
    if a["unmapped"]:
        fails.append(f"{len(a['unmapped'])} unmapped modules")
    if a["misplaced"]:
        fails.append(f"{len(a['misplaced'])} modules mapped to another crate than their own")
    if a["pkg_extern"]:
        fails.append(f"{len(a['pkg_extern'])} external dependencies outside [externals]")

    improved = []
    for b in base["sccs"]:
        if any(set(c) & b and not set(c) <= b for c in a["sccs"]):
            continue  # merged into a bigger cycle: reported as a failure above
        cur = [c for c in a["sccs"] if set(c) <= b]
        if sum(len(c) for c in cur) < len(b) or len(cur) != 1:
            improved.append(f"SCC of {len(b)} now {[len(c) for c in cur]}")
    for key in ("layer", "fence", "pkg_fence", "pkg_edge"):
        for k in sorted(base[key] - set(a[key])):
            improved.append(f"{key} {k[0]} -> {k[1]} gone")
    for s in improved:
        print(f"  improved: {s}")
    for f in fails:
        print(f"  FAIL {f}")
    if fails:
        print("dag-check: failed. Break the new edge, or (with review) classify the module in layers.txt.")
        return 1
    if improved:
        print("dag-check: ok; the graph improved, run with --update and commit the lower baseline.")
    else:
        print("dag-check: ok (no new cycles or violations).")
    return 0


def count_by(d, idx):
    c = defaultdict(int)
    for k in d:
        c[k[idx]] += 1
    return c


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
