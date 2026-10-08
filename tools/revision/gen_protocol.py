#!/usr/bin/env python3
"""Protocol registry generator (docs/symbols.md, "Protocol registry").

One definition per revision, `revisions/<rev>/protocol/{client,server,login}.tsv`
(name, opcode, size), generates every copy of the packet tables:

  client   tools/client910/crates/rs910-protocol/src/proto/tables.rs
  data     tools/protocol-reference/protocol-reference.json (tests read it)

Usage:
    python3 tools/revision/gen_protocol.py             write the outputs
    python3 tools/revision/gen_protocol.py --check     fail when an output differs
    python3 tools/revision/gen_protocol.py --rev 910   choose the revision

Independent anchor: each revision's tables must equal the recording of that
build's packet tables, `tools/client910/fixtures/recorded/protocol-<rev>/
protocol-tables.json`, which was captured once and is never generated. The
generator (write and `--check`) refuses tables that differ from it, so an
intended change to a revision's packets updates the recording in the same
change, and a new revision brings its own recording.

The outputs are the revision the code is built for (`CURRENT_REVISION`). The
generator is standard-library Python so the gate runs it with no install step,
and it is deterministic: the same tables always give the same bytes.

Table format: `#` comment lines, then a `name opcode size` header row, then one
row per packet sorted by opcode. Size is a byte count, `var-byte` or
`var-short`. Columns past the third (the packet layout, later) are ignored.
"""
import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CURRENT_REVISION = "910"

RUST_TABLES = ROOT / "tools/client910/crates/rs910-protocol/src/proto/tables.rs"
REFERENCE_JSON = ROOT / "tools/protocol-reference/protocol-reference.json"
RECORDED_DIR = ROOT / "tools/client910/fixtures/recorded"

NAME = re.compile(r"[A-Z][A-Z0-9_]*\Z")
VAR_BYTE = -1
VAR_SHORT = -2


class TableError(Exception):
    pass


def parse_size(text, where):
    if text == "var-byte":
        return VAR_BYTE
    if text == "var-short":
        return VAR_SHORT
    if re.fullmatch(r"(0|[1-9][0-9]*)", text):
        return int(text)
    raise TableError(f"{where}: size must be a count, var-byte or var-short, not {text!r}")


def read_table(path, dense):
    """Rows of one table as (name, opcode, size), validated."""
    rows = []
    header_seen = False
    for number, line in enumerate(path.read_text().splitlines(), 1):
        where = f"{path}:{number}"
        if not line.strip() or line.startswith("#"):
            continue
        fields = line.split("\t")
        if not header_seen:
            if fields[:3] != ["name", "opcode", "size"]:
                raise TableError(f"{where}: expected the header row 'name<TAB>opcode<TAB>size'")
            header_seen = True
            continue
        if len(fields) < 3:
            raise TableError(f"{where}: expected name, opcode and size")
        name, opcode_text, size_text = fields[:3]
        if not NAME.match(name):
            raise TableError(f"{where}: name {name!r} is not UPPER_SNAKE")
        if not re.fullmatch(r"(0|[1-9][0-9]*)", opcode_text) or int(opcode_text) > 255:
            raise TableError(f"{where}: opcode must be 0-255, not {opcode_text!r}")
        rows.append((name, int(opcode_text), parse_size(size_text, where)))
    if not header_seen:
        raise TableError(f"{path}: no header row")
    opcodes = [row[1] for row in rows]
    if opcodes != sorted(opcodes) or len(set(opcodes)) != len(opcodes):
        raise TableError(f"{path}: rows must be sorted by opcode with no repeats")
    names = [row[0] for row in rows]
    if len(set(names)) != len(names):
        dupes = sorted({n for n in names if names.count(n) > 1})
        raise TableError(f"{path}: repeated names {dupes}")
    if dense and opcodes != list(range(len(opcodes))):
        raise TableError(f"{path}: opcodes must be dense from 0 (the tables are indexed by opcode)")
    return rows


def read_revision(rev):
    folder = ROOT / "revisions" / rev / "protocol"
    return {
        "client": read_table(folder / "client.tsv", dense=True),
        "server": read_table(folder / "server.tsv", dense=True),
        "login": read_table(folder / "login.tsv", dense=False),
    }


def check_against_recording(rev, tables):
    """The tables must equal the revision's recorded packet tables."""
    path = RECORDED_DIR / f"protocol-{rev}" / "protocol-tables.json"
    if not path.exists():
        raise TableError(f"{path.relative_to(ROOT)}: revision {rev} has no recording of its packet tables")
    recorded = json.loads(path.read_text())
    problems = []
    for kind, key in (("client", "ClientProt"), ("server", "ServerProt"), ("login", "LoginProt")):
        want = {e["opcode"]: (e["name"], e["size"]) for e in recorded[key]}
        have = {o: (n, s) for n, o, s in tables[kind]}
        for opcode in sorted(set(want) | set(have)):
            if want.get(opcode) != have.get(opcode):
                problems.append(f"  {kind} opcode {opcode}: recording {want.get(opcode)}, table {have.get(opcode)}")
    if problems:
        raise TableError(
            f"revisions/{rev}/protocol differs from {path.relative_to(ROOT)} (name, size; -1 var-byte, -2 var-short):\n"
            + "\n".join(problems[:20])
            + "\nAn intended change to the packet tables updates the recording in the same change."
        )


def source_line(rev, kind):
    return f"revisions/{rev}/protocol/{kind}.tsv"


# -------------------------------------------------------------------- rust

RUSTFMT_WIDTH = 100 + 1  # a 100-column line plus its newline

RUST_HEADER = (
    "// Generated by tools/revision/gen_protocol.py from revisions/{rev}/protocol/*.tsv; do not edit.\n"
    "//\n"
    "// Opcode, framing size and name tables of the login, server and client packets.\n"
    "// The framing rule is in `super` (proto.rs).\n"
)


def rust_module(mod, doc, rows):
    out = [f"/// {doc}\n", f"pub mod {mod} {{\n"]
    out.append(f"    /// Number of opcodes in the table.\n    pub const COUNT: usize = {len(rows)};\n\n")
    for name, opcode, _ in rows:
        out.append(f"    pub const {name}: u8 = {opcode};\n")
    out.append(
        "\n    /// Framing size: `>= 0` fixed, `-1` a `u8` length prefix, `-2` a big-endian `u16`\n"
        "    /// length prefix. `None` for an opcode outside the table.\n"
        "    #[must_use]\n    pub const fn size(opcode: u8) -> Option<i32> {\n        match opcode {\n"
    )
    for name, _, size in rows:
        out.append(f"            {name} => Some({size}),\n")
    out.append(
        "            _ => None,\n        }\n    }\n\n"
        "    #[must_use]\n    pub const fn name(opcode: u8) -> &'static str {\n        match opcode {\n"
    )
    for name, _, _ in rows:
        arm = f'            {name} => "{name}",\n'
        if len(arm) > RUSTFMT_WIDTH:  # rustfmt moves a too-long arm body into a block
            arm = f'            {name} => {{\n                "{name}"\n            }}\n'
        out.append(arm)
    out.append('            _ => "UNKNOWN",\n        }\n    }\n}\n')
    return "".join(out)


def gen_rust(rev, tables):
    return (
        RUST_HEADER.format(rev=rev)
        + "\n"
        + rust_module("login", f"Login-stage opcodes ({len(tables['login'])} total).", tables["login"])
        + "\n"
        + rust_module("server", f"Server-to-client opcodes ({len(tables['server'])} total, dense from 0).", tables["server"])
        + "\n"
        + rust_module("client", f"Client-to-server opcodes ({len(tables['client'])} total, dense from 0).", tables["client"])
    )


# -------------------------------------------------------------------- json


def gen_json(rev, tables):
    def rows(kind):
        return [{"name": n, "opcode": o, "size": s} for n, o, s in tables[kind]]

    doc = {
        "source": f"revision {rev} protocol tables (name, opcode, size), generated from revisions/{rev}/protocol by tools/revision/gen_protocol.py",
        "ServerProt": rows("server"),
        "ClientProt": rows("client"),
        "LoginProt": rows("login"),
    }
    return json.dumps(doc, indent=1) + "\n"


# -------------------------------------------------------------------- main


def outputs(rev):
    tables = read_revision(rev)
    check_against_recording(rev, tables)
    return {
        RUST_TABLES: gen_rust(rev, tables),
        REFERENCE_JSON: gen_json(rev, tables),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--rev", default=CURRENT_REVISION)
    parser.add_argument("--check", action="store_true", help="fail when an output differs from the tables")
    args = parser.parse_args()
    try:
        files = outputs(args.rev)
    except (TableError, OSError) as error:
        print(f"gen_protocol: {error}", file=sys.stderr)
        return 1
    stale = [path for path, text in files.items() if not path.exists() or path.read_text() != text]
    if args.check:
        for path in stale:
            print(f"gen_protocol: {path.relative_to(ROOT)} is out of date; run python3 tools/revision/gen_protocol.py", file=sys.stderr)
        if not stale:
            print(f"gen_protocol: {len(files)} outputs match revisions/{args.rev}/protocol")
        return 1 if stale else 0
    for path in stale:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(files[path])
        print(f"wrote {path.relative_to(ROOT)}")
    if not stale:
        print("up to date")
    return 0


if __name__ == "__main__":
    sys.exit(main())
