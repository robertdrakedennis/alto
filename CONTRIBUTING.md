# Contributing

Alto is a game client engine that behaves exactly like the game it targets and
reads as our own code. The full rules are in [`AGENTS.md`](AGENTS.md) and
[`docs/engineering.md`](docs/engineering.md); this page is the short version.

## The rules for the client

1. **Behaviour is exact.** Packet bytes, saved options, per-tick state, script
   semantics, hook order and reference-renderer pixels do not change by
   accident. The gate checks them.
2. **Our own names and shape.** Real names, typed state, engine-shaped modules.
   No obfuscated-style identifiers (`method123`, `anInt45`, `arg0`).
3. **No Java cites, no translation.** Do not copy, translate or paraphrase
   decompiled or reference-client code, and do not add `File.java:12` cites or
   original class names to code or comments. Learn the behaviour, then write it
   in our own structure. Provenance for a fact goes in the pull request.
4. **Provenance gate.** `python3 tools/refactor/provenance.py check` counts the
   remaining traces per crate and fails if any count rises. Never dodge it with
   escapes or string tricks.
5. **Recorded fixtures.** Pin observable behaviour by recording it once and
   committing only the recording (`tools/client910/fixtures/recorded/<name>/`,
   `tools/native910/tests/fixtures/`). Use digest
   form for anything that holds pixels or cache content, and add a short row to
   the fixtures README. Never commit game data, jars, decompiled source, shader
   text, harnesses or cache files.
6. **Meaningful tests.** A test must fail on a real regression: a session
   replay, a recorded fixture, a golden or an end-to-end run through the real
   host. No tautologies and no test per list item.
7. **External counterparts.** Online sessions use a separately supplied
   compatible server. The private server and its npm workspace stay local.

## Run the checks

Without a cache (this is what CI runs):

```bash
cargo fmt --manifest-path tools/native910/Cargo.toml --check
python3 tools/refactor/fmt-ratchet.py
python3 tools/refactor/dag-check.py
python3 tools/refactor/provenance.py check
python3 tools/refactor/clippy-ratchet.py -- --features no-pack
cargo clippy --manifest-path tools/native910/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path tools/native910/Cargo.toml --features no-pack
cargo test --manifest-path tools/Cargo.toml $(python3 tools/refactor/workspace.py --pkg-args) --lib --features no-pack
python3 tools/refactor/test-inventory.py --mode no-pack --no-server --native-profile dev --quiet

```

With a cache, run the whole gate before you open a pull request:

```bash
tools/refactor/gate.sh --base main     # Rust gates for what the change touches (tools/refactor/README.md)
```

Add `--fn-hash <git-rev>` when the change should be a pure refactor (see
[`tools/refactor/README.md`](tools/refactor/README.md)); list intended
behaviour differences in an allow file with a reason each. Cache-dependent
client tests are ignored with `--features no-pack`.

## Supply a cache

Alto ships no game data. Provide a revision 910 cache you have the right to use
and build the packed data from it: [`docs/setup.md`](docs/setup.md) and
[`docs/cache.md`](docs/cache.md).

## Commits and pull requests

- Small, focused commits with a message that says what changed and why.
- A move is two commits: the move, then the path fixes.
- Run the checks above before you push. Clippy is at zero warnings, formatting
  is enforced, and no test may disappear (`tools/refactor/test-removals.txt`
  lists intended removals with a reason).
- Update the row in [`docs/client-status.md`](docs/client-status.md) when a
  feature moves from partial to working.
- Performance changes carry a before and after number (`tools/perf/`).
- Third-party dependencies: after changing a lockfile, run
  `python3 tools/licenses.py` and commit `THIRD_PARTY_LICENSES.md`. New
  dependencies must be permissively licensed (MIT, Apache-2.0, BSD, ISC and
  similar).
- Do not commit secrets, `.env` files, player data or anything under the
  gitignored local directories.
