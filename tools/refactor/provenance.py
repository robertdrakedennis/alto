#!/usr/bin/env python3
"""Provenance ratchet for "our own client" (independence programme, 2026-09-29).

The client must read as our own engine, not as a translation of the
deobfuscated Java. This script counts, per crate and per file, the traces
that are left:

  obf_code     obfuscated identifiers in Rust code (strings included):
               method\\d{2,}, field\\d{2,}, class\\d{2,}, anInt\\d+, aClass\\d+,
               aBool\\d+, aString\\d+, aLong\\d+, aByte\\d+, aFloat\\d+,
               anIntArray\\d+, aDouble\\d+, anObject\\d+, and the parameter-style
               names argN / varN / localN. An identifier counts when it, or one
               of its `_`-separated parts, is one of these (`set_field1234`).
  obf_comment  the same identifiers in comments and doc comments
  cites        `.java` cites in code or comments: `Foo.java`, `Foo.java:123`,
               `Foo.java:12-40`
  ghidra       Ghidra `FUN_<hex>` references
  tma          `too_many_arguments` allows (`#[allow(clippy::too_many_arguments)]`)
  gen_lines    lines in files marked generated (a leading comment that starts
               with "Generated ...", `@generated` or `do not edit`). Registry
               output (a header naming `tools/revision/`) is our own table
               codegen, not a port, and is not counted.
  java_word    the bare words Java/java/JAVA as a word or as an identifier part
               (`java_canvas`, `JavaCamera`, `*_matches_java`, "Java int"),
               in code, strings and comments; `Foo.java` cites are not counted
               here (see cites) and `JavaScript` is a different word
  java_class_ref
               references to the original client's class names (the committed
               list java-class-names.txt): `Name.member`, `Name::member` and a
               bare multi-word `Name` in code, strings and comments. A name
               that our own Rust code defines as a type or module (`struct`,
               `enum`, `trait`, `type`, `union`, `mod`) is ours: only the Java
               style `Name.member` still counts for it. A single-word name
               counts only when qualified. `Name.java` is a cite.
  exempt       lines marked `// provenance: recorded-data (<reason>)` (recorded
               data that must keep its exact text); they are skipped by the
               other columns and counted here.
  java_exception
               Java exception/error names in code, strings and comments:
               `*Exception`, `NullPointer`, `...OutOfBounds`, `AIOOB`, `NPE`,
               `StackOverflow`, `OutOfMemory`, `Throwable`, `printStackTrace`,
               and snake_case forms such as `runtime_exception`
  lines        total lines (informational; not checked)

Crates: client910 (tools/client910 without crates/), native910, and every
tools/client910/crates/<crate> (rs910-*, including rs910-far-scene). All
`*.rs` files under a crate directory count, tests included.

Modes:
  report [--files] [--crate NAME] [--top N] [--by COLUMN]
                                              table per crate (and per file;
                                              --by ranks files by one column)
  check                                       fail when a crate's count rises
                                              above provenance-baseline.tsv
  check --update [--allow-increase]           rewrite the baseline; it may only
                                              go down unless --allow-increase
  --update [--allow-increase]                 same as `check --update`

A crate that is not in the baseline counts from zero, so a new crate starts
clean. A column that an older baseline does not have yet is initialised from
the current count by the next `--update` (it is not a regression). Run
`check --update` in the same change that lowers a count.
"""
import argparse
import re
import sys
from collections import OrderedDict
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
BASELINE = HERE / "provenance-baseline.tsv"

CHECKED = ["obf_code", "obf_comment", "cites", "ghidra", "tma", "gen_lines",
           "java_word", "java_class_ref", "java_exception", "exempt"]
COLUMNS = CHECKED + ["lines"]
CLASS_NAMES_FILE = HERE / "java-class-names.txt"

OBF_PART = re.compile(
    r"(?:method|field|class)\d{2,}"
    r"|(?:anInt|aClass|aBool|aString|aLong|aByte|aFloat|anIntArray|aDouble|anObject)\d+"
    r"|(?:arg|var|local)\d+")
OBF_PART_FULL = re.compile(OBF_PART.pattern + r"\Z")
IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
CITE = re.compile(r"(?<![A-Za-z0-9_])[A-Za-z_][A-Za-z0-9_]*\.java\b(?::\d+(?:-\d+)?)?")
GHIDRA = re.compile(r"(?<![A-Za-z0-9_])FUN_[0-9a-fA-F]+")
TMA = re.compile(r"too_many_arguments")
# A line holding recorded data that must keep its exact text (a fixture's input
# string) carries `// provenance: recorded-data (<reason>)`. Its traces are not
# counted; the `exempt` column counts such lines and may not rise either.
EXEMPT = re.compile(r"//\s*provenance:\s*recorded-data\s*\(\s*\S[^)]*\)")

# java_word: identifier parts (`JavaCamera`, `java_canvas`, `JAVA_GOLDEN`).
NAME_PARTS = re.compile(r"[A-Z]+(?![a-z])|[A-Z]?[a-z]+|[0-9]+")

# java_exception: identifier tokens naming a Java throwable.
EXC_TOKEN = re.compile(
    r"(?:[A-Z][A-Za-z0-9]*)?Exception"
    r"|(?:[A-Za-z]*OutOfBounds(?:Exception)?)"
    r"|NullPointer|AIOOBE?|NPE|StackOverflow(?:Error)?|OutOfMemory(?:Error)?"
    r"|NoSuchMethodError|NoSuchFieldError|NoClassDefFoundError|Throwable|printStackTrace")
EXC_SNAKE = re.compile(
    r"(?:^|_)(?:null_pointer|array_index_out_of_bounds|index_out_of_bounds|string_index_out_of_bounds"
    r"|class_cast|class_not_found|illegal_state|illegal_argument|number_format|runtime|arithmetic"
    r"|negative_array_size|io|security|unsupported_operation)_exception(?:_|$)")

# java_class_ref: a capitalised word that may be a class name.
CAP_WORD = re.compile(r"(?<![A-Za-z0-9_])[A-Z][A-Za-z0-9]*(?![A-Za-z0-9_])")
HUMPS = re.compile(r"[A-Z]+(?![a-z])|[A-Z][a-z0-9]*")
RUST_DEF = re.compile(r"\b(?:struct|enum|trait|type|union|mod)\s+([A-Z][A-Za-z0-9]*)")

# Leading-comment markers of generated files.
GEN_START = re.compile(r"^\s*(?://[/!]?|\*)\s*generated\b", re.I)
GEN_INLINE = re.compile(r"[.;]\s+Generated\b|;\s+generated by\b", re.I)
GEN_ANY = re.compile(r"@generated|do not edit by hand|DO NOT EDIT")
# Output of the revision registry generators (tools/revision/, and `sym gen`
# for the content-symbol bindings in rs910-symbols): tables generated from our
# own committed definitions, not a port of anyone's code.
GEN_REGISTRY = re.compile(r"tools/revision/|run sym -- gen")

# One lexer step: the next comment start, string, raw string or char literal.
LEX = re.compile(
    r"//[^\n]*"
    r"|/\*"
    r'|(?<![A-Za-z0-9_])b?r(#*)".*?"\1'
    r'|"(?:[^"\\]|\\.)*"'
    r"|'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^\\'\n])'",
    re.S)


def lex_pieces(src):
    """Yield (kind, text) for the whole source; kind is "code" (also char
    literals), "string" or "comment"."""
    pos, n = 0, len(src)
    while pos < n:
        m = LEX.search(src, pos)
        if not m:
            yield "code", src[pos:]
            break
        yield "code", src[pos:m.start()]
        tok = m.group(0)
        if tok.startswith("//"):
            yield "comment", tok
            pos = m.end()
        elif tok == "/*":
            depth, i = 1, m.end()
            while i < n and depth:
                a, b = src.find("/*", i), src.find("*/", i)
                if b < 0:
                    i = n
                    break
                if 0 <= a < b:
                    depth += 1
                    i = a + 2
                else:
                    depth -= 1
                    i = b + 2
            yield "comment", src[m.start():i]
            pos = i
        else:
            yield ("code" if tok.startswith("'") else "string"), tok
            pos = m.end()


def split_code_comments(src):
    """Return (code, comments): the source text outside comments (string and
    char literals stay in code) and the comment text."""
    code, com = [], []
    for kind, text in lex_pieces(src):
        (com if kind == "comment" else code).append(text)
    return "\n".join(code), "\n".join(com)


def split_regions(src):
    """Return (code, strings, comments) as newline-joined text; code has no
    string literals (char literals stay)."""
    parts = {"code": [], "string": [], "comment": []}
    for kind, text in lex_pieces(src):
        parts[kind].append(text)
    return ("\n".join(parts["code"]), "\n".join(parts["string"]), "\n".join(parts["comment"]))


def count_obf(text):
    n = 0
    for tok in IDENT.findall(text):
        if OBF_PART_FULL.match(tok) or any(OBF_PART_FULL.match(p) for p in tok.split("_") if p):
            n += 1
    return n


def count_java_word(text):
    """Java/java/JAVA as a word or an identifier part; `JavaScript` is not one."""
    n = 0
    for tok in IDENT.findall(CITE.sub(" ", text)):
        parts = NAME_PARTS.findall(tok)
        i = 0
        while i < len(parts):
            low = parts[i].lower()
            if low == "java":
                if i + 1 < len(parts) and parts[i + 1].lower() == "script":
                    i += 2
                    continue
                n += 1
            i += 1
    return n


def count_java_exception(text):
    n = 0
    for tok in IDENT.findall(text):
        if EXC_TOKEN.fullmatch(tok) or EXC_SNAKE.search(tok.lower()):
            n += 1
    return n


def load_class_names():
    """{name: multi_word} from the committed java-class-names.txt."""
    if not CLASS_NAMES_FILE.exists():
        sys.exit(f"provenance: {CLASS_NAMES_FILE.relative_to(ROOT)} is missing")
    names = {}
    for line in CLASS_NAMES_FILE.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            names[line] = len(HUMPS.findall(line)) >= 2
    return names


def count_class_refs(text, names, defined, is_code):
    """Class-name references in one region (see the module docstring)."""
    n = 0
    end = len(text)
    for m in CAP_WORD.finditer(text):
        name = m.group(0)
        multi = names.get(name)
        if multi is None:
            continue
        a, b = m.start(), m.end()
        if is_code and text[a - 2:a] == "::":
            continue
        if b + 1 < end and text[b] == "." and (text[b + 1].isalpha() or text[b + 1] == "_"):
            member = IDENT.match(text, b + 1).group(0)
            if member != "java":
                n += 1
            continue
        if name in defined:
            continue
        if b + 2 < end and text[b:b + 2] == "::" and (text[b + 2].isalpha() or text[b + 2] == "_"):
            n += 1
        elif multi:
            n += 1
    return n


def rust_defs(src):
    """Type and module names our own code defines (code region only)."""
    code = split_regions(src)[0]
    return set(RUST_DEF.findall(code))


def is_generated(src):
    """True when the file's leading comment block marks it generated."""
    marked = False
    for line in src.split("\n")[:40]:
        s = line.strip()
        if not s or s.startswith("#!["):
            continue
        if not s.startswith(("//", "/*", "*")):
            break
        if GEN_REGISTRY.search(line):
            return False
        if GEN_START.match(line) or GEN_INLINE.search(line) or GEN_ANY.search(line):
            marked = True
    return marked


def scan_file(src, names, defined):
    exempt = 0
    kept = []
    for line in src.split("\n"):
        if EXEMPT.search(line):
            exempt += 1
            kept.append("")
        else:
            kept.append(line)
    full = src
    src = "\n".join(kept)
    code, com = split_code_comments(src)
    ncode, strings, com2 = split_regions(src)
    lines = full.count("\n") + (0 if full.endswith("\n") or not full else 1)
    return OrderedDict(
        obf_code=count_obf(code),
        obf_comment=count_obf(com),
        cites=len(CITE.findall(src)),
        ghidra=len(GHIDRA.findall(src)),
        tma=len(TMA.findall(code)),
        gen_lines=lines if is_generated(src) else 0,
        java_word=count_java_word(code) + count_java_word(com),
        java_class_ref=(count_class_refs(ncode, names, defined, True)
                        + count_class_refs(strings, names, defined, False)
                        + count_class_refs(com2, names, defined, False)),
        java_exception=count_java_exception(code) + count_java_exception(com),
        exempt=exempt,
        lines=lines)


def crates():
    """[(name, dir, excluded subdirs)] in a stable order."""
    tools = ROOT / "tools"
    out = [("client910", tools / "client910", [tools / "client910" / "crates"]),
           ("native910", tools / "native910", [])]
    cdir = tools / "client910" / "crates"
    if cdir.is_dir():
        out += [(p.name, p, []) for p in sorted(cdir.iterdir()) if (p / "Cargo.toml").exists()]
    return out


def rs_files(base, excluded):
    for p in sorted(base.rglob("*.rs")):
        rel = p.relative_to(base).parts
        if "target" in rel or any(str(p).startswith(str(e) + "/") for e in excluded):
            continue
        yield p


def scan():
    """{crate: {file(rel to repo): counts}}"""
    names = load_class_names()
    sources = OrderedDict()
    defined = set()
    for name, base, excl in crates():
        files = OrderedDict()
        for p in rs_files(base, excl):
            src = p.read_text(errors="replace")
            files[str(p.relative_to(ROOT))] = src
            defined |= rust_defs(src)
        sources[name] = files
    return OrderedDict(
        (name, OrderedDict((f, scan_file(src, names, defined)) for f, src in files.items()))
        for name, files in sources.items())


def totals(files):
    t = OrderedDict((c, 0) for c in COLUMNS)
    for counts in files.values():
        for c in COLUMNS:
            t[c] += counts[c]
    return t


def table(rows, first="crate"):
    head = [first] + COLUMNS
    body = [[name] + [str(c[k]) for k in COLUMNS] for name, c in rows]
    w = [max(len(r[i]) for r in [head] + body) for i in range(len(head))]
    fmt = lambda r: "  ".join(v.ljust(w[0]) if i == 0 else v.rjust(w[i]) for i, v in enumerate(r))
    return "\n".join([fmt(head), fmt(["-" * x for x in w])] + [fmt(r) for r in body])


def load_baseline():
    base = {}
    if BASELINE.exists():
        for line in BASELINE.read_text().splitlines():
            if not line.strip() or line.startswith("#"):
                continue
            f = line.split("\t")
            if f[0] == "crate":
                cols = f[1:]
                continue
            base[f[0]] = dict(zip(cols, map(int, f[1:])))
    return base


def write_baseline(per_crate):
    out = ["# Provenance counts per crate (tools/refactor/provenance.py). Lower only; `lines` is informational.",
           "\t".join(["crate"] + COLUMNS)]
    for name, t in per_crate.items():
        out.append("\t".join([name] + [str(t[c]) for c in COLUMNS]))
    BASELINE.write_text("\n".join(out) + "\n")


def cmd_report(a):
    res = scan()
    per_crate = OrderedDict((n, totals(f)) for n, f in res.items())
    rows = list(per_crate.items())
    grand = OrderedDict((c, sum(t[c] for t in per_crate.values())) for c in COLUMNS)
    print(table(rows + [("TOTAL", grand)]))
    if a.files:
        for name, files in res.items():
            if a.crate and name != a.crate:
                continue
            cols = [a.by] if a.by else [c for c in CHECKED if c != "gen_lines"]
            ranked = sorted(files.items(), key=lambda kv: (-sum(kv[1][c] for c in cols), kv[0]))
            ranked = [kv for kv in ranked if sum(kv[1][c] for c in (cols if a.by else CHECKED)) > 0]
            if a.top:
                ranked = ranked[:a.top]
            if ranked:
                print(f"\n{name}: files with provenance traces")
                print(table(ranked, "file"))
    return 0


def cmd_check(a):
    res = scan()
    now = OrderedDict((n, totals(f)) for n, f in res.items())
    base = load_baseline()
    zero = {c: 0 for c in COLUMNS}
    worse, better, fresh = [], [], []
    for name, t in now.items():
        b = base.get(name, zero)
        for c in CHECKED:
            if c not in b:  # column added after the baseline was written: initialised, not compared
                fresh.append(c)
            elif t[c] > b[c]:
                worse.append((name, c, b[c], t[c]))
            elif t[c] < b[c]:
                better.append((name, c, b[c], t[c]))
    for name in base:
        if name not in now:
            better.append((name, "(crate removed)", 0, 0))
    total = sum(t[c] for t in now.values() for c in CHECKED)
    btotal = sum(b.get(c, 0) for b in base.values() for c in CHECKED)
    print(f"provenance: {total} traces (baseline {btotal}) in {len(now)} crates")
    if fresh:
        print(f"  new columns not in the baseline yet: {', '.join(sorted(set(fresh)))} (run --update to initialise)")
    for name, c, was, cur in better:
        print(f"  improved  {name}.{c}: {was} -> {cur}")
    for name, c, was, cur in worse:
        print(f"  REGRESSED {name}.{c}: {was} -> {cur}")
    if a.update:
        if worse and not a.allow_increase and BASELINE.exists():
            print("refusing to raise the baseline; fix the regressions or pass --allow-increase")
            return 1
        write_baseline(now)
        print(f"wrote {BASELINE.relative_to(ROOT)}")
        return 0
    if worse:
        print("New provenance traces: use real names, no `.java` cites, no generated or "
              "`too_many_arguments` code. `report --files --crate NAME` lists where.")
        return 1
    if better:
        print("Counts fell: run `provenance.py check --update` and commit the lower baseline.")
    return 0


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("report")
    r.add_argument("--files", action="store_true", help="also list files with traces")
    r.add_argument("--crate", help="limit --files to one crate")
    r.add_argument("--top", type=int, default=0, help="limit --files rows per crate")
    r.add_argument("--by", choices=CHECKED, help="rank and filter --files by one column")
    c = sub.add_parser("check")
    c.add_argument("--update", action="store_true", help="rewrite the baseline")
    c.add_argument("--allow-increase", action="store_true", help="with --update: allow counts to rise")
    if argv[:1] in (["--update"], ["--allow-increase"]):
        argv = ["check", *argv]  # `provenance.py --update` is `check --update`
    a = ap.parse_args(argv)
    return cmd_report(a) if a.cmd == "report" else cmd_check(a)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
