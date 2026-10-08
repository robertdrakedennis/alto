# Engineering rules

Rules for changing the Rust client and tools. Behaviour is the contract;
structure is free. The goal is a real game client and engine that behaves like
the game's client, not a copy of anyone's class tree.

## Our own client

- The client is our own code. Use real names, typed state and engine-shaped
  modules. Do not copy or translate code, identifiers, shader text or data from
  the original game or from any reference implementation.
- No obfuscated-style identifiers (numbered `method`, `field`, `arg`, `var`,
  `anInt` names), no source-file cites into other projects, no transpiled or
  machine-generated ports. Provenance for a fact belongs in the pull request,
  not in the code.
- Behaviour must stay exact where it is observable: wire bytes, saved options,
  per-tick state, script semantics, hook order, pixels of the reference
  renderer and the order of the logic phases. These are what the gate checks.
- Revision-specific facts (opcodes, packet layouts, config and CS2 opcodes,
  formats) live in the protocol, config and `native910` tables, never as
  literals in engine layers.
- Never add game data, decompiled source, jars or shader text to the repo. The
  cache is supplied by the user and stays gitignored (`docs/cache.md`).

## Invariants a change must keep

A refactor must leave these unchanged (enforced by `tools/refactor/gate.sh`):

1. Outgoing packet bytes and their order.
2. The client options encoding and clamping.
3. Movement, variable and varbit state per tick.
4. Script (CS2) stack, branch and variable semantics, and interface hook order.
5. Rendered pixels of the reference renderer for a fixed scene on one machine.
6. The order of the logic phases within a cycle. Reordering a phase is
   behaviour work that needs evidence, not a refactor.

## Sealed and unsealed subsystems

A subsystem is sealed when it is fully implemented and its observable output
is covered by replays, goldens or recorded fixtures. Sealed code may be
reshaped freely (function shapes, ownership, data layout, module and crate
boundaries, names). Unsealed code stays close to the behaviour it is still
being written to match. `#[allow(clippy::too_many_arguments)]` is a temporary
exception; replace it with typed parameters or owned state when reshaping.

## Testing

- Keep four thin checks: packet byte equality, options codec, per-tick state,
  script semantics. Prefer a few end-to-end tests over many mirrored unit
  tests. No tautologies (a test that only re-states the code) and no test per
  list item.
- Record observable behaviour once into a fixture beside the owning crate and
  compare against it (`tools/client910/fixtures/`, `tools/native910/tests/
  fixtures/`). Do not add reference harnesses to the repo. The default suite needs no other runtime and no original client.
- Fast suite: `cargo test`. Pixel suites run
  before a release, not in the fast loop. A control whose backend does not
  exist yet is `partial`, not failed.
- Do not commit one report or verification file per feature. The status doc is
  [`client-status.md`](client-status.md).

## Gates and pull requests

- Run `tools/refactor/gate.sh` before merging any refactor. Clippy is at zero
  warnings and CI denies warnings. See `tools/refactor/README.md` for what each
  step checks.
- A move is two commits: the move itself, then the path fixes. Function bodies
  must be byte-identical after a move; `fn-hash` checks that.
- Performance changes carry a before and after number (`tools/perf/`).
- Remove dead code once it is proven unused; `#[allow(dead_code)]` is not a
  fix.

## Engine guidelines

- Fixed-step logic on the game's 20 ms cycle, variable-rate rendering. Anything
  time-based in a renderer takes an explicit interpolation input, not the wall
  clock.
- Own the state in one tree and borrow it per phase. Prefer typed queues at
  subsystem seams to shared mutable state.
- Hot paths use flat slot-indexed vectors, reused scratch buffers and keys
  parsed once, with no per-frame string formatting.
- GPU resources are owned by lifetime (toolkit, region, scene, frame), updated
  in place, with one pipeline cache keyed by variant.
- Decoders return errors for bad input and never panic on data from the
  server. Errors that repeat every frame are rate limited.
- Diagnostics sit behind flags and are applied at one documented point of the
  cycle, not woven through the loop body.
- New crates follow the conventions in [`tools/README.md`](../tools/README.md)
  and the layering in [`architecture.md`](architecture.md).
