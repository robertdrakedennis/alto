# Quality gates

Every change must pass the gate before it merges. It checks that no test is
lost, no function body changed by a move, no new dependency cycle or layer or
fence violation appeared, no new trace of the original implementation, that
the replays and goldens still pass. The private server and its npm workspace
are excluded from the public gate. Rules for changes are in
[`docs/engineering.md`](../../docs/engineering.md).

## Gate

```sh
CARGO_TARGET_DIR=<lane target dir> tools/refactor/gate.sh --base main   # a lane, before asking to merge
tools/refactor/gate.sh --verify-record --base main                      # the lead, on the same tree
tools/refactor/gate.sh --full                                           # every step and crate
tools/refactor/gate.sh [--base REV] [--full] [--verify-record] [--fn-hash <before.tsv | git-rev>]
                       [--fn-hash-allow FILE] [--fn-hash-renames] [--fail-fast] [--only step,step]
```

| Case | What runs | Warm time (measured) |
|---|---|---|
| Identical tree (`--verify-record`) | nothing: a recorded pass with the same tree and options | 0.3 s |
| Typical Rust change (`--base`; one crate) | every Rust step; tests of the changed crates and their dependents | 193 s (rs910-ui) |
| No `--base` | every step and crate | about the `--full` time less its no-pack build |
| `--full` | every step and crate, and a real no-pack build that cross-checks the no-pack scan | 263 s |

| Step | What it runs | Fails when |
|---|---|---|
| `generated` | `tools/revision/gen_protocol.py --check`: regenerates the protocol tables from `revisions/<rev>/protocol/*.tsv` in memory, after checking the tables against the recording in `fixtures/recorded/protocol-<rev>/` (always runs) | the tables differ from that recording, or a committed generated file (`rs910-protocol` tables, `protocol-reference.json`) differs from its tables |
| `fmt` | `cargo fmt --check` for native910 and `rsscan`; `fmt-ratchet.py` (client910 and the rs910-* crates) | any file is unformatted (`fmt-pending.txt` is empty) |
| `clippy` | `clippy-ratchet.py` (client910 and rs910-*, all targets, per-lint counts); native910 `clippy -D warnings` | a lint count rises above `clippy-baseline.txt`; any native910 warning |
| `dag` | `dag-check.py` | a new cycle, layer edge, fence use or unclassified module |
| `provenance` | `provenance.py check` | any crate's count of traces of the original implementation rises above `provenance-baseline.tsv` |
| `magic-ids` | `magic-id-ratchet.py` (`rsscan magic-ids` over client910 and every rs910-* crate but `rs910-symbols`) | a file's count of unnamed content ids rises above `magic-id-baseline.txt`, a new file has any, or a count falls without `--update` |
| `tests` | one `cargo nextest run --profile gate` over the selected crates (default features, the `test` profile; `rsscan`'s tests when `tools/refactor/` changed) | any test fails |
| `replay` | reads the tests step's results (`tools/target/nextest/<profile>/junit.xml`) | any test in `replay-gate.txt` is missing from them, did not pass, or was ignored |
| `inventory` | `test-inventory.py` on the tests step's `cargo nextest list` (no build), no-pack status read from the source | a test disappears, or runs in fewer configurations |
| `fn-hash` | `fn-hash.py` against a snapshot or git revision (optional; `--fn-hash-renames` makes it rename-aware) | a function or item body changed, appeared or disappeared |

The script runs every step and prints a summary with the time of each. Use
`--fail-fast` to stop at the first failure and `--only dag,inventory` to run
selected steps (an `--only` run is never recorded). It runs cargo offline unless
`GATE_ONLINE=1`. `cargo-nextest` must be installed (`cargo install --locked
cargo-nextest`).

### What a change runs (`--base`)

`--base REV` (or the `--fn-hash` revision) compares the working tree,
untracked files included, with `REV` (`gatekit.py plan`):

- **Rust steps** run when anything under `tools/` or `revisions/` (or
  `rust-toolchain.toml`) changed, Markdown aside, except the content-symbol
  registry (`revisions/<rev>/symbols/`) itself: its Rust bindings are the
  generated sources of `rs910-symbols`, which select that crate and its
  dependents like any crate change. Otherwise they are skipped.
- **Tests:** the crates whose Rust sources (`*.rs`, `Cargo.toml`) changed,
  plus every workspace crate that depends on them (`cargo metadata`). Every
  crate's tests run when a shared foundation
  changed: `rs910-core`, `rs910-js5` or `rs910-protocol` (directly or as a
  dependency of a changed crate), `tools/Cargo.toml`, `tools/Cargo.lock`,
  `tools/.config/`, `rust-toolchain.toml`, `revisions/`, `tools/revision/` or
  `tools/refactor/`; and when a changed file under `tools/` is not a crate's
  Rust source (fixtures and recordings are read across crates).
- **The replay gate always runs** with the Rust steps: client910 depends on
  every client crate, so it is always selected.
- **The render tier** (`gatekit.py` `RENDER_TIER_TESTS`: renderer choice,
  present rate and profiler inertness, and the injected-clock determinism
  test) proves that rendering, presentation and the profiler change nothing
  observable. A `--base` run includes it only when the change touches what it
  guards: a render crate (`rs910-render-gpu`, `rs910-render-modern`,
  `rs910-gpu-device`, `rs910-toolkit`, `rs910-far-scene`), the app shell and
  the replay harness (client910 `src/app/`, `app.rs`, `main.rs`, `lib.rs`,
  `active_toolkit.rs`, `modern_display.rs`, `debug_flags.rs`,
  `session_replay.rs`, `test_support.rs`), the client core's frame, phases
  and redraw (rs910-client `client_core*`, `graphics_runtime.rs`,
  `toolkit_caps.rs`), `rs910-core`'s `profile.rs` or `logic_clock.rs`, or any
  Rust file that holds profiler scopes (`profile::`) before or after the
  change. Otherwise the tests step filters it out (`-E`; the listing stays
  whole) and the replay step requires that it did not run and checks every
  other entry. Every run that tests every crate (`--full`, no `--base`, a
  foundation change) runs it.
- Without `--base` every step and every crate runs. `--full` also builds
  no-pack for real and cross-checks the no-pack scan.

### Gate records and `--verify-record`

A passing run (not `--only`, and only when the tree did not change while it
ran) writes a record keyed by the working tree's git tree (the index plus
every unstaged change and untracked file, through a throwaway index: `git
add -A; git write-tree`), the options (`--full`, `--base`/`--fn-hash` resolved
to commits, the `--fn-hash-allow` file's hash, `--only`), `rustc -V`
and a fingerprint of `server/data/pack` (names, sizes, mtimes). The
record (`<key>.json`: exit code, plan, steps and their times) lives in the git
common directory (`.git/gate-records/`, shared by every worktree, never
committed; `GATE_RECORD_DIR` overrides). `--verify-record` passes at once when
a record with the same key exists; any difference (one byte of the tree, an
option, the toolchain, the pack) runs the gate. Gitignored inputs other than
the pack (`server/data/generated`) are not in the key.

### Concurrent gates

Gates never wait for each other. `gate.sh` re-runs itself under `gatekit.py lock`, which only registers the run in `~/.cache/alto/gate.lock.d/<pid>`, cleaning up entries for processes that have died.

The run's `NEXTEST_TEST_THREADS` is the machine's cores divided by the number of gates running, so a lone gate uses the whole machine. The divide is for memory, not CPU: a replay test holds about 1.2 GB, so several gates each running a full set of replays at once would swap.

`--verify-record` runs before registering.

### Builds: target dirs and incremental compilation

- Keep one `CARGO_TARGET_DIR` per lane for the lane's life: the gate is fast
  because its build is warm.
- Leave incremental compilation on (do not set `CARGO_INCREMENTAL=0`): after a
  one-function change in rs910-scene, rebuilding every test binary took
  about 6 s with it and about 77 s (51 s at opt-level 1) without.
- The `test` profile builds everything at opt-level 2 with line tables only
  (`tools/Cargo.toml`), so native910's tests no longer need `--release`.
- sccache is opt-in: `RUSTC_WRAPPER=sccache tools/refactor/gate.sh ...` (or
  any cargo command; `brew install sccache`). It caches the third-party
  crates across target dirs, so a new lane's first build is shorter. It
  cannot cache the workspace crates, which compile incrementally, so it does
  not speed up a warm gate.

### Intentional reductions and their safety nets

| Reduction | Safety net |
|---|---|
| `--base` runs only the changed crates and their dependents | the replay gate always runs; foundation changes, non-Rust files in `tools/` and runs without `--base` test every crate; `--full` before a release |
| `--base` leaves the render tier out unless the change touches render, presentation, profiler or app-shell code | the trigger list above (crates, paths, and any file with profiler scopes before or after the change); the replay step fails if a tier test ran while left out or any other replay entry is missing; `--full`, runs without `--base` and foundation changes run it. CI has no pack, so it never runs these tests: `--full` before a release is their net |
| Replay and settings-world test state is not torn down under nextest (`test_support::ReclaimedAtExit`) | only the drop is skipped, after the test's last assertion, in a process that exits right after the one test; `cargo test` (shared process) drops as before |
| No `--features no-pack` build or run locally | the no-pack status is read from the source exactly (the inventory fails on any non-canonical use of the feature); `--full` builds no-pack and fails if it disagrees with the scan; CI builds and runs the real no-pack suite and inventory on every push |
| `--verify-record` does not re-run | the record key covers the whole tree, the options, the toolchains and the pack |

### Measurements

Measured 2026-10-02 on a 10-core machine shared with other lanes (load 15 to
50, so absolute times are about twice an idle machine's). Warm: the lane's
target dir already built. Before: the gate as it was (no `--base`; every
crate twice, native910 `--release`, the replay tests run twice).

| Case | Before | After |
|---|---|---|
| Warm, no change, every crate | 620 s (tests 465, replay 132, inventory 10) | `--full`: 263 s in the historical combined checkout (tests 196, inventory 19.5) |
| Typical Rust change (one function in rs910-ui) | 564 s (tests 331, replay 122, inventory 81) | 193 s (tests 171, inventory 0.9) |
| Identical tree | 620 s | 0.3 s |
| Cold, fresh target dir | 2511 s (inventory 1248, tests 680, replay 397, clippy 156) | `--full` 584 s (tests 346, inventory 135, clippy 49) |
| Target dir after one cold run | about 22 GB (debug) plus release | 6.8 GB |

Lane G-FAST3 (2026-10-02, a one-function rs910-ui change: client910,
rs910-client and rs910-ui, the tests step alone, warm, one run each on a
machine shared with other lanes): before, 587 tests in 401.5 s wall, 3170
test-seconds, 926 CPU-seconds (load 55 to 75); after, 581 tests (the render
tier left out) in 220.5 s wall, 2005 test-seconds, 729 CPU-seconds (load 24
to 43). A started replay costs about 2.5 CPU-seconds and its teardown 0.7 to
1.2 s of wall time (freeing about 1.2 GB value by value under memory
pressure); nextest runs each test in its own process, so the teardown is
skipped there (`ReclaimedAtExit`). Sharing a started replay across tests
cannot help under one process per test, and inside one test only the render
tier starts the same session twice.

The tests step has no long floor: no test takes much over 30 s under that
load. The replay tests run one recorded session each and run their replays on
parallel threads (client910 `session_replay::ReplayPlan`; nextest reserves a
slot per thread, `threads-required` in `tools/.config/nextest.toml`), and the
player model and pose oracles buffer their output (they took 50 to 130 s in
unbuffered writes). The step is bound by the total test time instead: about
1400 test-seconds over ten slots at load 23 for a one-function rs910-ui
change (tests 192 s, gate 219 s, 973 CPU-seconds against 1122 before).

| Test (warm; before: the six side by side at load 3 to 7; after: through nextest at load 15 to 33) | Before | After |
|---|---|---|
| `renderer_choice_is_observationally_inert` | 72.8 s | 20.7 s |
| `client_core_replays_match_the_frozen_loopback_harness` (5 sessions; now one test each) | 66.8 s | 21.8 to 27.9 s |
| `present_rate_is_observationally_inert` (2 sessions; now one test each) | 58.3 s | 19.4 and 31.1 s |
| `profiler_is_observationally_inert` (2 sessions; now one test each) | 29.7 s | 29.3 and 34.9 s |
| `player_pose_oracle::composed_player_poses` | 62.6 s | 3.9 s |
| `player_model_oracle::real_cache_player_bodies` | 46.0 s | 2.9 s |

CI (`.github/workflows/client.yml`, job `rust`) runs on a fresh
clone with no cache, no JDK and no `ref/`: fmt, the clippy ratchet, native910
clippy, the DAG ratchet, the provenance check, the real no-pack tests of
native910, client910, every rs910-* crate and `rsscan`, and the inventory in
no-pack mode. The replay
gate, the full inventory status check and fn-hash need the packed cache
(`server/data/pack`) or a before-snapshot, so they run locally.

## Typical refactor pull request

```sh
python3 tools/refactor/fn-hash.py snapshot /tmp/before.tsv   # on the base commit
# ... git mv / codemod ...
tools/refactor/gate.sh --fn-hash /tmp/before.tsv --base main
# or, after committing: tools/refactor/gate.sh --fn-hash origin/main
```

A pure move shows `fn-hash: no code differences`. A change that alters bodies on
purpose lists the expected keys with `--allow 'fn Engine::*'` and the reviewer
reads the CHANGED list. The client packages come from `workspace.py`, so a new
crate is covered by every gate once it is a workspace member.

## `test-inventory.py`: test-inventory lock

- Lists every test of client910, native910 and the rs910-* crates with its
  ignored status. In the gate it reads the tests step's `cargo nextest list
  --message-format json` (`--nextest-list`), so it builds nothing; alone it
  builds with `cargo test --no-run` and lists each binary (`--list`, `--list
  --ignored`).
- Baseline: `test-inventory.txt` (`target  test  status`). The status is `run`
  (runs in both configurations), `nopack` (runs by default, ignored under
  `--features no-pack`) or `ignored`. Regenerate it with `--update` on a machine
  with the pack; `--update` refuses to run while tests are missing unless they
  are listed in `test-removals.txt`.
- The no-pack status without a no-pack build (`--nopack static`, the gate's
  default): the `no-pack` feature does nothing but switch
  `#[cfg_attr(feature = "no-pack", ignore ...)]` on tests, so `rsscan tests`
  walks each test target's module tree like rustc and lists every test fn with
  that attribute under its libtest path. It also counts every `feature =
  "no-pack"` token sequence; when a crate uses the feature any other way
  (`#[cfg(feature = "no-pack")]`, an attribute inside a macro), the counts
  differ and the inventory fails rather than guess. `--nopack build` (the
  default outside the gate) builds no-pack and lists it; `--nopack check`
  (`gate.sh --full`) does both and fails on any difference.
- `--packages a,b` checks only those crates' part of the baseline (the gate's
  selection; unselected crates did not change). The legacy `--no-server` flag
  is accepted; this public inventory always collects only Rust tests.
- Moves are allowed. Tests are paired by their full libtest path, then by their
  final path segment. A name left over in several places is treated as a
  multiset, and the check prints the candidate paths on both sides.
- A disappeared test, or one that now runs in fewer configurations, fails,
  unless it is listed in `test-removals.txt` as `<glob over path> <reason>` (the
  reason is mandatory). New and un-ignored tests only print a notice.
- CI has no pack, so it runs `--mode no-pack --native-profile dev`: every
  `#[cfg_attr(feature = "no-pack", ignore)]` test still compiles and is listed.
  It fails only when a test the baseline marks `run` is ignored under no-pack.

## `fn-hash.py`: moved bodies stay byte-identical

`rsscan items` parses every file with `syn`. It lives in `rsscan/`, its own
workspace with its own lockfile. For each item it records a location-free key
and hashes (`code`, `doc`, `comments` and the rename-aware `shape`).

- **Items covered:** fn (free, impl, trait default), impl header, struct, enum,
  union, const, static, type, trait, mod (its attributes), `macro_rules!` and
  item-level macro calls.
- **Key:** kind plus name, qualified by the impl or trait context with paths cut
  to their last segment. The same key in several modules is a multiset. Rows
  pair by equal code hash first; only leftovers are reported.
- **`code` hash:** the tokens after removing whitespace, comments, doc
  attributes, visibility and `#[path]`, stripping crate path prefixes, and
  expanding single-name paths through the module's `use` declarations.
- **`doc` and `comments` hashes:** doc attributes and the non-doc comments inside
  the item. A change here is a warning; `--strict` makes it fail.

`compare` reports CHANGED, RE-KEYED (identical body, new name), DISAPPEARED and
APPEARED. `check <rev>` snapshots a git revision (via `git archive`) and the
working tree. The scanned roots are client910 `src` and `tests`,
`tools/client910/crates` and native910 `src`, `tests` and `examples`; add more
with `--root label=dir`.

### Rename-aware mode (`--renames`, gate flag `--fn-hash-renames`)

For rename lanes, where every code hash changes. `rsscan items` also emits, per
item, a `shape` hash (the `code` tokens with each distinct identifier replaced
by its first-occurrence index; keywords, primitives, macro names, literals and
operators are kept exactly) and the distinct identifiers in first-occurrence
order. `fn-hash.py check REV --renames` pairs items whose code hash changed by
`(kind, shape)` and reports each pair as `renamed old -> new`. Any other change
fails as before.

A shape match is refused, and the item fails, when it could hide a real change:
the new name is one the item already used for something else (swapped operands
or fields); a name is both the source and a target of renames in one run; a
global name is renamed to different names in different items; or a type-level
name is renamed onto a type name that already existed. A rename onto a
different but equally shaped name that is consistent everywhere cannot be told
from a rename by shape alone; it shows in the rename table. When the lane knows
its renames, pass `--rename-list FILE` (`old new` per line): every identifier
rename outside the list fails.

## `provenance.py`: traces of the original implementation (ratchet)

The client must read as our own engine. `provenance.py` counts, per crate and
per file, over all `*.rs` files (tests included): obfuscated-style identifiers
in code and in comments, source-file cites, decompiler function addresses,
`too_many_arguments` allows, generated-code lines, references to the original
implementation's language, class names and exception names, and lines marked as
recorded data.

A line marked `// provenance: recorded-data (<reason>)` holds recorded data
whose exact text a fixture depends on. Its traces are not counted in the other
columns; the marker needs a reason, and the exemption count is checked like the
others, so every new exemption shows up in review.

```sh
python3 tools/refactor/provenance.py report [--files [--crate NAME] [--top N] [--by COLUMN]]
python3 tools/refactor/provenance.py check            # gate step `provenance`
python3 tools/refactor/provenance.py check --update   # lower the baseline
```

`check` fails when any checked column of a crate rises above
`provenance-baseline.tsv`. `--update` rewrites the baseline and refuses to raise
any count unless `--allow-increase` is given. `java-class-names.txt` is the
committed name list behind the class-name column, so the check runs without any
local reference material.

## `dag-check.py`: module DAG and fences (ratchet)

The graph spans real crates: every `rs910-*` member is scanned from its own
`lib.rs`, and client910 is scanned with the crate aliases so paths through
crate boundaries resolve. A module that lives in a crate must be mapped to that
crate in `layers.txt`, and a crate with an `[externals]` row may only depend on
the external crates it lists.

- `rsscan deps` walks the module tree the way rustc does: it follows `mod x;`,
  `#[path]` and `mod.rs`, records every reference from one top-level module to
  another (`crate::`, `super::`, `$crate::`, `use` trees, paths inside macros)
  and every `wgpu`, `winit`, `tokio` or `cpal` path. Anything under
  `#[cfg(test)]` is ignored.
- `layers.txt` holds three tables: `[crates]` (allowed crate dependencies),
  `[fences]` (external-crate fences) and `[modules]` (which crate each module
  belongs to).
- A module edge is a **layer violation** when its target crate is neither the
  source's own crate nor reachable from it (an edge that points up or sideways).
- `dag-baseline.txt` is the ratchet. The check fails on a module joining a
  cycle, a new layer edge or fence use, a package-level fence or edge violation
  (checked with `cargo metadata`), or a production module with no entry in
  `layers.txt`. When the graph improves, run `--update` to commit the smaller
  baseline. `--report` lists every cycle, violation and first site; `--json`
  writes the full analysis.

## Replay gate (`replay-gate.txt`)

The runtime replays and golden traces every change must keep green. They assert
their results. They run in the gate's tests step (client910 is selected for
every Rust change), and the replay step requires every listed test to appear in
that run's results and to have passed; a missing, renamed, ignored or failing
entry fails the gate (`gatekit.py replay-check`).
`--only replay` runs just these tests first:

- **Session and scenario replays.** Recorded server frames go through the
  production owners. The tests assert outbound packets, var state, the minimap
  queue and menus.
- **Retained-UI replays.** Settings, equipment, consumables, run toggle, player
  picking to follow, observer and animation replays.
- **Draw traces and goldens.** Scene draw decisions, occlusion fixtures, minimap,
  particles and the scene build against recorded goldens.

Some tests with the same names are `#[ignore]`d GPU comparisons and diagnostics;
the gate does not run those. Recordings live in `tools/client910/fixtures/`.

## Files

| File | Purpose |
|---|---|
| `gate.sh` | the single gate command (both languages) |
| `gatekit.py` | the gate's plan (diff selection), records, lock and replay check |
| `../.config/nextest.toml` | the nextest profiles (`gate`, `full`) and the slots the multi-threaded replay tests reserve |
| `test-inventory.py`, `test-inventory.txt`, `test-removals.txt` | test-inventory lock |
| `fn-hash.py`, `rsscan/` | item hashes and move check (`rsscan` also serves the DAG scan) |
| `dag-check.py`, `layers.txt`, `dag-baseline.txt` | module DAG, layers, fences, crate externals |
| `provenance.py`, `provenance-baseline.tsv`, `java-class-names.txt` | ratchet for traces of the original implementation |
| `magic-id-ratchet.py`, `magic-id-baseline.txt` | ratchet for unnamed content ids in client Rust (docs/symbols.md) |
| `workspace.py` | the client packages of the workspace, for every gate |
| `replay-gate.txt` | replay and golden tests the gate runs |
| `clippy-ratchet.py`, `clippy-baseline.txt` | clippy ratchet |
| `fmt-ratchet.py`, `fmt-pending.txt` | rustfmt ratchet |
| `replay-overlay.sh` | export, pack and run the recorded-session cache overlay |

## Golden sessions (local only)

`replay-overlay.sh export|pack|run` builds and replays the cache overlay (about
13.5 MB of cache excerpts) that the recorded sessions read. It is game data and
is never committed: it lives under `tools/target`. CI runs it only when the
overlay secret is configured; the replay gate in `gate.sh` covers the same
sessions on a machine with the packed cache.
