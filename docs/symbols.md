# Symbols and revision tables

The client names content through revision registries. Never guess an id or
write an unnamed id into engine code, tests or scenarios.

## Registries

| Registry | Location | Client consumer |
|---|---|---|
| Content symbols | `revisions/<rev>/symbols/<type>.sym` | `rs910-symbols` generated Rust bindings |
| Protocol | `revisions/<rev>/protocol/{client,server,login}.tsv` | `rs910-protocol` and recorded table checks |
| CS2 instruction set | `tools/native910/data/opcodes-<rev>.txt` | native910's opcode book |

## Content symbols

One file per content type contains sorted, tab-separated `name`, `id` and
`fingerprint` fields. Names are our own lower_snake_case identifiers, unique
within a type. Components use `interface.component`; locations name a
`level:x:z` tile. Fingerprints identify what the revision's cache must say for
an id to mean what the symbol claims.

The committed Rust bindings live in
`tools/client910/crates/rs910-symbols/src/`. They provide typed ids and named
constants: `rs910_symbols::{obj, npc, varbit, component, location, ...}`.
Generated bindings make builds independent of the private server and its npm
workspace. Do not edit generated bindings by hand.

Content-binding regeneration currently uses maintainer-local tooling, which
is not shipped in this repository. Resolve a new symbol against the cache,
record its fingerprint in the registry and have the bindings regenerated
together. Existing names and generated bindings are sufficient to build and
test the client. New and touched Rust code uses symbols, not raw ids. Places
belong in the registry too.

## Protocol registry

Each revision has tab-separated `name`, `opcode` and `size` rows, sorted by
opcode. Sizes are fixed byte counts, `var-byte` or `var-short`. Opcode numbers
belong in these tables, never in engine layers.

`tools/client910/crates/rs910-protocol/src/proto/tables.rs` holds the client's
packet names, opcodes and framing sizes; keep it equal to the tables.

The independent anchor is
`tools/client910/fixtures/recorded/protocol-<rev>/protocol-tables.json`.
It is a committed recording; `rs910-protocol`'s `proto::tests` fails if the
client's tables disagree with it. An intended protocol change updates its
recording in the same change. Server packet names describe the incoming wire
contract.
