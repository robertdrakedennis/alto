#!/usr/bin/env python3
"""Regenerate THIRD_PARTY_LICENSES.md: the licences of every third-party
dependency the repository ships or builds with.

    python3 tools/licenses.py            # write THIRD_PARTY_LICENSES.md
    python3 tools/licenses.py --check    # fail if the file is out of date
    python3 tools/licenses.py --out FILE # write somewhere else

Sources of truth
  * Rust: `cargo metadata --locked` for tools/Cargo.toml (the client, native
    tools and every rs910-* crate) and tools/refactor/rsscan/Cargo.toml (the
    gate helper). Every registry crate in the lockfiles is listed, for all
    target platforms. Licence texts are read from the crate sources in the
    cargo registry (cargo downloads them when metadata is requested).
Needs network access on a fresh checkout (crate sources). Python
3.8+ standard library only. The output is deterministic for a given pair of
lockfiles, so `--check` is stable.

The script flags, at the top of the output, any dependency whose licence
expression cannot be satisfied by a permissive licence (copyleft or unknown) and
any dependency for which no licence text ships with the package.
"""
import argparse
import collections
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "THIRD_PARTY_LICENSES.md"
RUST_MANIFESTS = [
    ("Rust workspace (tools/Cargo.lock)", ROOT / "tools" / "Cargo.toml"),
    ("Rust gate helper rsscan (tools/refactor/rsscan/Cargo.lock)",
     ROOT / "tools" / "refactor" / "rsscan" / "Cargo.toml"),
]

PERMISSIVE = {
    "MIT", "MIT-0", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib",
    "0BSD", "Unlicense", "CC0-1.0", "Unicode-3.0", "Unicode-DFS-2016", "BSL-1.0",
    "bzip2-1.0.6", "BlueOak-1.0.0", "Python-2.0", "WTFPL", "CC-BY-4.0",
    "LLVM-exception",
}
COPYLEFT_RE = re.compile(
    r"^(A?L?GPL|MPL|EPL|CDDL|EUPL|SSPL|OSL|CPL|CC-BY-(NC|SA)|Sleepycat|CECILL)", re.I)

# Packages whose metadata declares no licence although the shipped licence text
# identifies one (checked by hand against the text). Keyed by (ecosystem, name).
OVERRIDES = {}

MIT_TEMPLATE = """MIT License

Copyright (c) {holders}

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE."""

LICENSE_FILE_RE = re.compile(
    r"^(licen[cs]e|copying|unlicense|notice|copyright|patents)([-_.].*)?$", re.I)
LICENSE_DIRS = {"license", "licenses", "licence", "licences", "license-files"}


# ---------------------------------------------------------------- SPDX ----

def tokenize(expr):
    expr = expr.replace("/", " OR ")
    return re.findall(r"\(|\)|[^\s()]+", expr)


def parse(tokens):
    """or_expr := and_expr (OR and_expr)*; returns nested tuples."""
    pos = 0

    def peek():
        return tokens[pos] if pos < len(tokens) else None

    def take():
        nonlocal pos
        pos += 1
        return tokens[pos - 1]

    def atom():
        t = take()
        if t == "(":
            v = or_expr()
            if peek() == ")":
                take()
            return v
        if peek() and peek().upper() == "WITH":
            take()
            exc = take()
            return ("id", t, exc)
        return ("id", t, None)

    def and_expr():
        parts = [atom()]
        while peek() and peek().upper() == "AND":
            take()
            parts.append(atom())
        return parts[0] if len(parts) == 1 else ("and", parts)

    def or_expr():
        parts = [and_expr()]
        while peek() and peek().upper() == "OR":
            take()
            parts.append(and_expr())
        return parts[0] if len(parts) == 1 else ("or", parts)

    return or_expr()


def ids(node):
    if node[0] == "id":
        return [node[1]]
    return [i for p in node[1] for i in ids(p)]


def permissive_ok(node):
    """True when the expression can be satisfied by permissive licences only."""
    if node[0] == "id":
        return node[1] in PERMISSIVE
    if node[0] == "or":
        return any(permissive_ok(p) for p in node[1])
    return all(permissive_ok(p) for p in node[1])


def classify(expr):
    """Returns (status, note). status: ok | copyleft-alternative | flagged."""
    if not expr:
        return "flagged", "no licence field"
    try:
        node = parse(tokenize(expr))
    except (IndexError, ValueError):
        return "flagged", "unparseable licence expression"
    all_ids = set(ids(node))
    bad = sorted(i for i in all_ids if i not in PERMISSIVE)
    if permissive_ok(node):
        if bad:
            return "copyleft-alternative", (
                "offered under " + ", ".join(bad) + " as an alternative; used under the "
                "permissive alternative")
        return "ok", ""
    copyleft = [i for i in bad if COPYLEFT_RE.match(i)]
    if copyleft:
        return "flagged", "copyleft: " + ", ".join(copyleft)
    return "flagged", "unknown licence id: " + ", ".join(bad)


# ---------------------------------------------------------- text files ----

def norm_text(raw):
    text = raw.decode("utf-8", errors="replace").replace("\r\n", "\n").replace("\r", "\n")
    lines = [l.rstrip() for l in text.split("\n")]
    return "\n".join(lines).strip("\n")


def license_files_in_dir(path):
    """(relative name, normalised text) for licence-like files of a directory."""
    found = []
    if not path.is_dir():
        return found
    for entry in sorted(path.iterdir(), key=lambda p: p.name.lower()):
        if entry.is_file() and LICENSE_FILE_RE.match(entry.name):
            found.append((entry.name, norm_text(entry.read_bytes())))
        elif entry.is_dir() and entry.name.lower() in LICENSE_DIRS:
            for sub in sorted(entry.iterdir(), key=lambda p: p.name.lower()):
                if sub.is_file():
                    found.append((entry.name + "/" + sub.name, norm_text(sub.read_bytes())))
    return found


# ---------------------------------------------------------------- Rust ----

def cargo_metadata(manifest):
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked",
           "--manifest-path", str(manifest)]
    out = subprocess.run(cmd, stdout=subprocess.PIPE, stdin=subprocess.DEVNULL,
                         check=True).stdout
    return json.loads(out)


def rust_packages(manifest):
    meta = cargo_metadata(manifest)
    by_id = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    # Normal + build edges only: what a released build compiles in or runs.
    normal = set()
    stack = list(meta["workspace_members"])
    while stack:
        cur = stack.pop()
        if cur in normal:
            continue
        normal.add(cur)
        for dep in nodes[cur]["deps"]:
            if any(k.get("kind") in (None, "build") for k in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    members = set(meta["workspace_members"])
    out = []
    for pid, pkg in by_id.items():
        if pid in members or not pkg.get("source"):
            continue
        files = license_files_in_dir(Path(pkg["manifest_path"]).parent)
        lf = pkg.get("license_file")
        if lf:
            p = Path(pkg["manifest_path"]).parent / lf
            if p.is_file() and not any(n == p.name for n, _ in files):
                files.append((p.name, norm_text(p.read_bytes())))
        out.append({
            "name": pkg["name"], "version": pkg["version"],
            "license": pkg.get("license") or "",
            "repository": pkg.get("repository") or pkg.get("homepage") or "",
            "authors": [re.sub(r"\s*<[^>]*>", "", a).strip() for a in pkg.get("authors") or []],
            "scope": "normal" if pid in normal else "dev-only",
            "files": files,
        })
    return out


# ------------------------------------------------- fill-in and output ----

def apply_overrides(eco_key, pkgs):
    for p in pkgs:
        ov = OVERRIDES.get((eco_key, p["name"]))
        if ov and not p["license"]:
            p["license"], p["note"] = ov


def synthesize_missing(all_pkgs):
    """Packages that publish no licence file get the standard text of the
    permissive licence they declare, naming the holders from their manifest.
    Apache-2.0 uses the text the other packages ship (it has no per-holder
    part)."""
    apache = None
    for p in all_pkgs:
        for _, t in p["files"]:
            if t.startswith("Apache License") and "Version 2.0, January 2004" in t:
                apache = t
                break
        if apache:
            break
    for p in all_pkgs:
        if p["files"]:
            continue
        try:
            node = parse(tokenize(p["license"])) if p["license"] else None
        except (IndexError, ValueError):
            node = None
        have = set(ids(node)) if node else set()
        holders = ", ".join(p.get("authors") or []) or f"the {p['name']} authors"
        if "MIT" in have:
            p["files"] = [("(synthesised)", MIT_TEMPLATE.format(holders=holders))]
            p["synthesised"] = "MIT"
        elif "Apache-2.0" in have and apache:
            p["files"] = [("(synthesised)", apache)]
            p["synthesised"] = "Apache-2.0"
        elif "CC0-1.0" in have:
            p["files"] = [("(synthesised)", "CC0 1.0 Universal: the authors dedicated this work to the public domain.\n"
                           "See https://creativecommons.org/publicdomain/zero/1.0/legalcode")]
            p["synthesised"] = "CC0-1.0"


# -------------------------------------------------------------- output ----

def build(rust_sets):
    lines = []
    w = lines.append
    every = [(eco, p) for eco, pk in rust_sets for p in pk]

    w("# Third-party licences")
    w("")
    w("Generated by `python3 tools/licenses.py`; do not edit by hand. Lists every")
    w("third-party dependency in the Rust lockfiles (`tools/Cargo.lock`,")
    w("`tools/refactor/rsscan/Cargo.lock`), with the licence text that ships in each package.")
    w("Alto's own licence is in `LICENSE`; component origins are in `NOTICE`.")
    w("")

    # summary
    w("## Summary")
    w("")
    eco_rows = collections.OrderedDict()
    for eco, pk in rust_sets:
        eco_rows[eco] = pk
    for eco, pk in eco_rows.items():
        normal = sum(1 for p in pk if p["scope"] != "dev-only")
        w(f"- {eco}: {len(pk)} packages ({normal} compiled in or run, "
          f"{len(pk) - normal} dev-only)")
    w("")
    w("Packages by licence expression (as declared by each package):")
    w("")
    for label, sel in (("Rust", [p for _, p in every]),):
        c = collections.Counter(p["license"] or "(none)" for p in sel)
        w(f"**{label}** ({len(sel)})")
        w("")
        w("| Licence expression | Packages |")
        w("|---|---:|")
        for k, v in sorted(c.items(), key=lambda kv: (-kv[1], kv[0])):
            w(f"| {k} | {v} |")
        w("")

    # flags
    flagged, alt, synth = [], [], []
    for eco, p in every:
        status, note = classify(p["license"])
        if status == "flagged":
            flagged.append((eco, p, note))
        elif status == "copyleft-alternative":
            alt.append((eco, p, note))
        if p.get("synthesised"):
            synth.append((eco, p))
    w("## Flags")
    w("")
    if flagged:
        w("Copyleft or unknown licences (review before release):")
        w("")
        for eco, p, note in flagged:
            w(f"- {p['name']} {p['version']} ({eco}): `{p['license'] or 'none'}`, {note}")
    else:
        w("No dependency is copyleft-only or has an unknown licence: every package can be")
        w("used under a permissive licence (MIT, Apache-2.0, BSD, ISC, Zlib, Unlicense,")
        w("CC0, Unicode and similar).")
    w("")
    if alt:
        w("Packages that also offer a copyleft alternative (we use the permissive one):")
        w("")
        for eco, p, note in alt:
            w(f"- {p['name']} {p['version']} ({eco}): `{p['license']}`")
        w("")
    noted = [(eco, p) for eco, p in every if p.get("note")]
    if noted:
        w("Licence taken from the shipped text because the package declares none:")
        w("")
        for eco, p in noted:
            w(f"- {p['name']} {p['version']} ({eco}): {p['license']}; {p['note']}")
        w("")
    if synth:
        w(f"{len(synth)} packages publish no licence file in their archive. For these the")
        w("standard text of their declared licence is reproduced below, naming the copyright")
        w("holders from the package manifest; the upstream repository holds the original")
        w("notice.")
        w("")
        w(", ".join(f"{p['name']} {p['version']}" for eco, p in synth))
        w("")

    # package tables
    def table(title, pk):
        w(f"## {title}")
        w("")
        w("| Package | Version | Licence | Scope |")
        w("|---|---|---|---|")
        for p in sorted(pk, key=lambda p: (p["name"].lower(), p["version"])):
            w(f"| {p['name']} | {p['version']} | {p['license'] or '(none)'} | {p['scope']} |")
        w("")

    for eco, pk in rust_sets:
        table(eco, pk)

    # texts, deduplicated by content
    texts = collections.OrderedDict()
    for eco, p in every:
        for fname, text in p["files"]:
            if not text:
                continue
            key = hashlib.sha256(text.encode()).hexdigest()
            entry = texts.setdefault(key, {"text": text, "users": set()})
            entry["users"].add(f"{p['name']} {p['version']}")
    w("## Licence texts")
    w("")
    w("Each distinct licence or notice text appears once, followed by the packages that")
    w("ship it.")
    w("")

    def first_line(t):
        for l in t.split("\n"):
            if l.strip():
                return l.strip()[:80]
        return ""

    def heading(t):
        head = first_line(t)
        for l in t.split("\n"):
            if re.match(r"^\W*copyright\b", l.strip(), re.I) and l.strip()[:80] != head:
                return f"{head} / {l.strip()[:90]}"
        return head

    ordered = sorted(texts.values(), key=lambda e: (heading(e["text"]).lower(),
                                                    sorted(e["users"])[0].lower()))
    for i, e in enumerate(ordered, 1):
        users = sorted(e["users"], key=str.lower)
        w(f"### {i}. {heading(e['text'])}")
        w("")
        w("Used by: " + ", ".join(users))
        w("")
        w("```text")
        w(e["text"])
        w("```")
        w("")
    return "\n".join(lines).rstrip("\n") + "\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--check", action="store_true", help="fail if the output file is stale")
    ap.add_argument("--out", type=Path, default=OUT)
    args = ap.parse_args()

    rust_sets = [(label, rust_packages(m)) for label, m in RUST_MANIFESTS]
    for _, pk in rust_sets:
        apply_overrides("rust", pk)
    synthesize_missing([p for _, pk in rust_sets for p in pk])
    text = build(rust_sets)
    if args.check:
        current = args.out.read_text() if args.out.exists() else ""
        if current != text:
            print(f"{args.out.name} is out of date: run python3 tools/licenses.py", file=sys.stderr)
            return 1
        print(f"{args.out.name} is up to date")
        return 0
    args.out.write_text(text)
    total = sum(len(p) for _, p in rust_sets)
    print(f"wrote {args.out} ({total} packages)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
