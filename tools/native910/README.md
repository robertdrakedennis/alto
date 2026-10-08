# native910 CS2 workflow

Run commands from the Alto repository root. `--pack-root` defaults to
`server/data/pack`; pass it explicitly when working with a different corpus.

## Inspect and edit

```sh
cargo run --manifest-path tools/native910/Cargo.toml -- registry
cargo run --manifest-path tools/native910/Cargo.toml -- dump --out-dir /tmp/910-source
cargo run --manifest-path tools/native910/Cargo.toml -- dump --script 5093 --out-dir /tmp/910-timer
cargo run --manifest-path tools/native910/Cargo.toml -- dump-interfaces --interface bank --inames /path/to/inames.txt --out-dir /tmp/910-bank
cargo run --manifest-path tools/native910/Cargo.toml -- inspect --out-dir /tmp/910-evidence
cargo run --manifest-path tools/native910/Cargo.toml -- analyze --script 1258 --arg i:10858 --arg i:1
cargo run --manifest-path tools/native910/Cargo.toml -- assemble \
  --input /tmp/910-source/5093_0.rs2 --output /tmp/5093.bin \
  --symbols /tmp/910-source/symbols.txt
```

`registry` distinguishes numeric dispatch entries of the shipped command set from unverified synthetic
entries and reports the established stack/state contracts. Unknown does not mean
zero arguments or a no-op. The Rust `semantics` API additionally exposes dynamic
type dependencies and execution ownership. These are separate from differential
verification: an implemented operation has not necessarily been tested over its
entire input domain.

`analyze` reports the universal signature separately from the supplied entry context.
Typed arguments use `i:<int>`, `l:<long>`, `s:<text>` or `n:` for a null object;
`i:?`, `l:?` and `s:?` leave a position unknown. Omitted arguments are unknown.
Proven constants select exact boolean, signed comparison and switch edges and
specialize unresolved callees. Those proofs never overwrite a generic helper
signature. Recursion and exhausted analysis budgets remain unresolved. Host
results stay opaque, and a stack proof does not certify successful host execution.

Binary local counts include argument slots. Source declarations list only the
additional locals; the compiler calculates the total. Source round trips preserve
the original bytes while argument names remain available for authoring.

Selected dumps retain all corpus names and argument signatures, infer return
contracts only for their call dependencies, and verify original bytes before
writing. Other return contracts in those registries stay unknown.
`inspect --out-dir` exports calls, direct variable accesses,
component hierarchy, cached and script-installed hooks, dynamic children with
creator PCs, unresolved sites, clears versus missing script IDs, return-signature
status and conservative transitive resource footprints. `inputs.txt` pins the
packs and analysis code. Retain the conditions in `entry-contexts.tsv`: cached
hook argument refinement describes that entry, and other runtime entries can
supply different arguments. A missing edge never proves missing behavior.

## Build a script project

Create a manifest beside the source files:

```text
base-sha256 <SHA-256 of client.scripts.js5>
script 200000 main main.rs2
script 200001 compute compute.rs2
```

The IDs above are examples. Check the target corpus before allocating new IDs.
Manifest entries with existing IDs replace those scripts in the generated build.
Unlisted scripts retain their original containers. Symbol names must be unique.

`main.rs2`:

```text
[clientscript,main]()
int $result;
$result = ~compute(20 + 21);
push_int_local($result);
return(0);
```

`compute.rs2`:

```text
[proc,compute](int $x)
push_int_local($x);
push_constant_int(1);
add(0);
return(0);
```

Build into a new directory:

```sh
cargo run --manifest-path tools/native910/Cargo.toml -- build \
  --manifest /path/to/project.txt --output /path/to/build-1
```

The build links declared argument signatures before compiling callers. It resolves
value calls as callee return summaries become available, including named cached
helpers. Literal call arguments can establish a context-specific result type
when the generic helper has multiple return shapes. The result must still be
exactly one value; other shapes fail with script-specific errors. Context proofs
remain separate from generated universal signatures. Unresolved dependencies
fail with script-specific errors. It validates encoded output and numeric callee
references, then publishes the build directory. Existing output directories are
rejected. The original pack is never replaced.
Proven stack underflow, invalid operands/control flow and missing callees reject
the build. `stack-analysis.tsv` records complete or context-required analysis for
each edited script; runtime-dependent shapes stay explicit.

Outputs include `client.scripts.js5`, individual `.bin` and `.rs2` files,
`symbols.txt`, `<id>.map.tsv` (instruction index → one-based source line), the
project manifest, and a build report with SHA-256 identities for the base, output,
sources, registry and config inputs.

The writer currently supports the provisioned single-file script-group layout
and indexes with name, length and checksum metadata. Changed containers are
uncompressed; unchanged containers remain byte-identical. A no-change rebuild
returns the original complete pack bytes. Digest-bearing or unknown index flags
are rejected when changes are requested.

For already assembled numeric `.bin` files, `repack --input-dir DIR --output FILE`
creates a new script pack. It does not overwrite an existing output file.

## Build an interface project

The same manifest can include components, with the interface archive pinned:

```text
base-sha256 <SHA-256 of client.scripts.js5>
interfaces-sha256 <SHA-256 of client.interfaces.js5>
names names.txt
inames inames.txt
script <script-id> initialize initialize.rs2
component <interface-id> <file-id> frame.ifc
component <interface-id> <other-file-id> label.ifc
```

`names` and `inames` are optional curated registries. New component identities
are registered before scripts are assembled, so authored scripts can reference
their symbolic names. Components can reuse the lossless `.ifc` source of an
existing frame. Keep its identity when editing that frame; allocating a new
interface also requires updating scripts whose explicit references still point
to the old interface. Original local names and structured source are not present
in the cache; the exact low-level source remains the authoring fallback.

Hooks link to both base and authored scripts. Their heads must resolve, and their
typed tails must fit the callee's local banks. Short tails are valid: hook entry
initializes fresh locals, including unused long slots. Edited groups are checked
for absent parents and layer cycles. Output includes both packs, component source
and binary files, symbol inputs and their hashes. Unedited sibling files and
untouched group containers are preserved; new sparse component groups are
supported. Changed groups use one stripe with signed file-length deltas.
Replacing a script also revalidates unedited cached hooks when the interface
archive is present; that input's hash accompanies the build. An explicit
interface hash is enforced for script-only projects too, which retain the pinned
interface image in their output.

`ui_goldens::ui_authoring_replay` builds a frame and scripts from local templates,
loads the generated packs through the real client reader, decodes `IF_OPENTOP`,
executes lifecycle hooks, checks runtime creator identity, paints text and drives
the pointer hook through ordinary UI input. Its generated packs remain local.

## Execute and debug

```sh
cargo run --manifest-path tools/native910/Cargo.toml -- \
  --pack-root /path/to/build-1 trace --input /path/to/build-1/200000.bin
```

The trace command uses a deterministic fixture host and empty root arguments.
Calls resolve from the selected script pack. Unsupported host behavior fails with
script/PC context; it is not filled in with placeholder results. `--break-at PC`
stops before a root instruction. This CLI is an inspection entry point, not an
interactive debugger frontend.

The Rust APIs provide persistent debugging control:

- `Session::for_script` supplies cache ID, arguments and optional event context.
- `Vm::step` executes one instruction; failed sessions cannot resume partially
  applied operations.
- `Debugger` supports instruction/source stepping, breakpoints, continuation,
  and variable/component write watchpoints.
- Snapshots expose typed stacks, locals, frames, current script ID, last executed
  instruction and reported host writes. Source maps connect these to authored lines.
- `returns::analyze` reports the first obstructing instruction and reason, such
  as underflow, incompatible merge, missing callee or unknown command.

`RuntimeHost` is a controlled fixture host with explicit variable definitions,
arrays, component lookup and component text operations. Its component store does
not implement the game's rendering, redraw or delayed-event lifecycle. The legacy
Rust login approximation lives in `preview`; it is not a semantic oracle.

Null references remain distinct from empty strings through the object stack,
locals, calls and component hook arguments. Concatenation renders them as
`"null"`, `string_length` returns zero, and dereferencing string operations fail.
String variable reads retain their separate `"null"` substitution. UTF-16
operations preserve lone surrogate units. String-only host handlers receive the
non-null stack suffix; nullable consumers can use `Host::trap_objects_context`.

## Required verification

```sh
sh tools/native910/verify.sh
# Optimized large-corpus checks:
sh tools/native910/verify.sh --release
```

This profile requires all local corpus packs used by the tests, records evidence
hashes, runs Clippy and the corpus suite (which includes the recorded-trace,
timer and database fixtures), and runs the explicit project-build and
script/interface-repack tests. The server implementation independently checks
rebuilt archives, including index re-encoding, group checksums, lengths and every
file split. Missing
required inputs fail.

The frozen traces in `tests/fixtures/recorded/` and
`tests/fixtures/cs2-traces/` were recorded once from the original client's
script interpreter with controlled setup. Traces of real cache scripts are
committed as one FNV-1a 64 digest per instruction (`*.dig`) and the scripts
themselves are read from your local pack at test time, so no cache content is
stored in the repository; traces of our own synthetic scripts stay verbatim.
They are compared with the Rust VM with no other runtime: instruction positions, typed stacks, locals and call depth, with
strings encoded as UTF-16 units to avoid ambiguous text formatting. The timer
fixture additionally compares the variable value and component text at every
instruction. This is VM evidence, not a full client boot.

The real-script acceptance case uses scripts **5093 and 5094**. Starting client
variable 995 at 400 and providing a dynamic component produces `0:00:07`.
Editing the decrement from 1 to 51 produces `0:00:06`. The test dumps source,
proves the baseline round trip, assembles the edit, rebuilds the pack, reloads the
script and compares the native execution with the recorded traces. The rebuilt
pack is written to `target/timer-acceptance/` for the server-side check.

Neither case claims a live JS5 network session, full event-loop parity, or
complete command coverage.


### Semantic flow diagnostics

`cargo run --offline --manifest-path tools/native910/Cargo.toml --example semantic_coverage`
loads the required cache packs and reports known return signatures and ranked
first blockers. `dataflow::analyze` exposes per-PC typed stacks, local values and
reaching producer PCs. Constants survive agreeing CFG merges; conflicting loop
values widen to unknown. Config-backed variables/params, enum output types and
verified hook descriptors resolve dynamic stack traffic. Unknown calls and host
operations remain explicit analysis failures. This analyzes normal completion
shapes; it does not prove absence of runtime exceptions or implement host behavior.


### Dependency-ranked closure

Pass an output directory to the coverage example to retain the root ranking,
every unresolved script's blocker chain, the known-ID set and transitive resource
footprints:

```sh
cargo run --offline --manifest-path tools/native910/Cargo.toml --example semantic_coverage -- tools/native910/target/closure-current
```

`resources.tsv` includes opaque host commands, missing callees and invalid-flow
flags. A footprint is a conservative set of possible accesses, never a purity
certificate. Return summaries propagate constants and argument identities;
recursive signatures require base-return evidence and validation of every path.
Project builds limit inference to the edited scripts' dependency closure.
The required profile includes the per-ID coverage gate and the recorded database
tuple/default fixture.


The current registry classifies all 1,432 opcodes of the 910 command set, including runtime
conditions and traps. The committed stack-contract table
(`src/cs2_stack_contracts.rs`) holds 1,268 fixed stack shapes, checked against the
registry by the tests.
The corpus has 14,267 fixed signatures, one non-returning script and 45 scripts
with explicit divergent or runtime-dependent roots (selector-dependent return
shapes in the graphics-setting scripts and state-dependent world-list
operations); contextual analysis for those is the remaining work. Use
`--release` for the full coverage example.
