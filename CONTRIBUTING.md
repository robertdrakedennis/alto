# Contributing

Alto is a game client engine that behaves exactly like the game it targets and
reads as our own code. The full rules are in [`AGENTS.md`](AGENTS.md) and
[`docs/engineering.md`](docs/engineering.md); this page is the short version.

## The rules for the client

1. **Interop is exact, the rest is "plays right".** Packet bytes, login crypto, the saved client options, cache formats and CS2 script semantics must
   match the 910 client. Rendering, timing and UI internals only need to
   look and play right; do not add pixel or hook-order goldens.
2. **Our own names and shape.** Real names, typed state, engine-shaped modules.
   No obfuscated-style identifiers (`method123`, `anInt45`, `arg0`).
3. **No Java cites, no translation.** Do not copy, translate or paraphrase
   decompiled or reference-client code, and do not add `File.java:12` cites or
   original class names to code or comments. Learn the behaviour, then write it
   in our own structure. Provenance for a fact goes in the pull request.
4. **Recorded fixtures.** Pin interop facts by recording them once and
   committing only the recording (`tools/client910/fixtures/recorded/<name>/`,
   `tools/native910/tests/fixtures/`). Use digest
   form for anything that holds pixels or cache content, and add a short row to
   the fixtures README. Never commit game data, jars, decompiled source, shader
   text, harnesses or cache files.
5. **Proofs over tests.** Prove a feature end to end on the real client and
   show the observable result. Keep a unit test only when it checks against
   an outside source of truth (the 910 client's bytes, the cache format, a
   recorded fixture). No tautologies and no test per list item.
6. **External counterparts.** Online sessions use a separately supplied
   compatible server. The private server and its npm workspace stay local.

## Run the checks

```bash
tools/check.sh   # fmt, clippy -D warnings (CI runs the same)
```

Run the tests of the crates you change with `cargo test`; tests that read the
cache are ignored with `--features no-pack`.

## Supply a cache

Alto ships no game data. Provide a revision 910 cache you have the right to use
and build the packed data from it: [`docs/setup.md`](docs/setup.md) and
[`docs/cache.md`](docs/cache.md).

## Commits and pull requests

- Small, focused commits with a message that says what changed and why.
- A move is two commits: the move, then the path fixes.
- Run the checks above before you push. Clippy is at zero warnings and
  formatting is enforced.
- Update the row in [`docs/client-status.md`](docs/client-status.md) when a
  feature moves from partial to working.
- Performance changes carry a before and after number.
- Third-party dependencies: after changing a lockfile, update
  `THIRD_PARTY_LICENSES.md`. New dependencies must be permissively licensed (MIT, Apache-2.0, BSD, ISC and
  similar).
- Do not commit secrets, `.env` files, player data or anything under the
  gitignored local directories.
