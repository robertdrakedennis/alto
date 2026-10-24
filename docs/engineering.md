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
- Interop must stay exact: packet bytes, login crypto, the saved client options, cache formats and CS2 script semantics. Everything else
  only has to look and play right. The original client was never
  deterministic, so do not chase pixel, draw-order or hook-order parity.
- Revision-specific facts (opcodes, packet layouts, config and CS2 opcodes,
  formats) live in the protocol, config and `native910` tables, never as
  literals in engine layers.
- Never add game data, decompiled source, jars or shader text to the repo. The
  cache is supplied by the user and stays gitignored (`docs/cache.md`).

## Invariants a change must keep

A refactor must leave these unchanged:

1. Outgoing packet bytes and their order.
2. The client options encoding and clamping.
3. Movement, variable and varbit state per tick.
4. Script (CS2) stack, branch and variable semantics.

## Sealed and unsealed subsystems

A subsystem is sealed when it is fully implemented and its observable output
is covered by tests or recorded fixtures. Sealed code may be
reshaped freely (function shapes, ownership, data layout, module and crate
boundaries, names). Unsealed code stays close to the behaviour it is still
being written to match. `#[allow(clippy::too_many_arguments)]` is a temporary
exception; replace it with typed parameters or owned state when reshaping.

## Testing

- Tests are not gates. Prove a feature once, end to end, on the real stack
  (the real client against the server, or the server's socket E2E for
  server-only behaviour) and show the observable result. Rerun a feature's
  proof when you change that feature.
- Keep a unit test only when it checks against an outside source of truth:
  packet bytes against the 910 client, the options codec, script semantics,
  cache decoding, a recorded capture. No tautologies (a test that only
  re-states the code), no mocks of our own code, no test per list item.
- Record observable behaviour once into a fixture beside the owning crate and
  compare against it (`tools/client910/fixtures/`, `tools/native910/tests/
  fixtures/`). Do not add reference harnesses to the repo. Tests need no
  other runtime and no original client.
- A control whose backend does not exist yet is `partial`, not failed.
- Do not commit one report or verification file per feature. The status doc is
  [`client-status.md`](client-status.md).

## Checks and commits

- Run `tools/check.sh` before committing Rust changes: generated protocol
  tables, `cargo fmt`, clippy with warnings denied (clippy is at zero) and the
  provenance count. CI runs the same script.
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
