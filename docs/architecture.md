# Architecture

This public repository contains the Rust client and native tools. The cache
and a compatible online server are supplied separately. The client is a layered set of crates; the layering
is enforced by `tools/refactor/dag-check.py`.

## Processes and data

The client reads a packed cache through `--pack-root` and can render offline.
For online sessions it connects to a separately supplied JS5, lobby and world
peer. Protocol tables live in `rs910-protocol`, generated from the revision
registries. The default endpoints are localhost, lobby port 43594 and world
port 43594 plus the world id; CLI options select other endpoints.

The private development server, its content and its npm workspace are not
tracked in this repository. Recordings captured with that counterpart remain
client test fixtures and replay without its runtime.

## Client crates

The Rust workspace is `tools/Cargo.toml`. Crates live in
`tools/client910/crates/`, plus `tools/client910` (the shell binary and the
integration-level test suites) and `tools/native910` (the standalone CS2 and
cache library). Dependencies point downward only:

| Crate | Role | May depend on |
|---|---|---|
| `rs910-core` | std-only leaf utilities: byte readers, math, colour, text encoding, clock | nothing |
| `rs910-symbols` | generated content-symbol bindings: id newtypes and named constants (`sym gen`, docs/symbols.md); the engine stays data-driven, tests, replays and debug traces name what they use | nothing |
| `rs910-js5` | cache, disk store and the JS5 network client | core, native910 |
| `rs910-protocol` | wire protocol: opcode tables, framing, message parsers and builders, the login ciphers (ISAAC opcode masking, block cipher, RSA block) (no I/O) | core |
| `rs910-config` | typed decoders from cache bytes to config stores; ClientOptions | core, js5, native910 |
| `rs910-game` | game state: entities, packet appliers, variable domains, session state machine | core, protocol, config |
| `rs910-model` | CPU model layer: models, animation, particles, floors, font layout | core, config |
| `rs910-scene` | CPU scene layer: map loading, scene graph, occlusion, camera, minimap, `SceneSnapshot` | core, config, model, game |
| `rs910-audio` | the audio stack: decoder, voices, buses, mixer, output sink | core, config, js5 |
| `rs910-toolkit` | the 2D toolkit boundary: display lists, sprite and text batches, `Toolkit` trait, frame plan | core, model |
| `rs910-ui` | the retained UI: components, CS2 script host over native910's VM, menus, console, world map, social | core, protocol, config, game, model, scene, audio, toolkit, native910, symbols |
| `rs910-gpu-device` | the wgpu device layer | core, toolkit |
| `rs910-render-gpu` | the reference GPU toolkit over the device layer | core, model, scene, toolkit, gpu-device |
| `rs910-far-scene` | render-only far scene, CPU half (draw-distance rings, map squares) | core, js5, config, model, scene |
| `rs910-render-modern` | the default modern scene renderer ([`renderer/modern-renderer.md`](renderer/modern-renderer.md)) | core, model, scene, toolkit, gpu-device, far-scene |
| `rs910-client` | client core: the frame phases, session and login I/O, replay and recording | everything below the renderers |
| `client910` | the shell: window, input, backend choice, binaries | client, render-gpu, render-modern |

Fences (external crates allowed in one place only): `wgpu` in
`rs910-gpu-device`, `rs910-render-gpu` and `rs910-render-modern`;
`objc2-metal` (the Metal allocation total) in `rs910-gpu-device`; `winit` in
`client910`; `tokio` in `rs910-js5` and `rs910-client`; `cpal` in
`rs910-audio`. The exact tables are in `tools/refactor/layers.txt`.

## How a frame works

- **Logic** runs on the game's fixed 20 ms cycle from an injected clock, and
  owns all game state (entities, variables, scene, interface). **Rendering**
  reads a snapshot and does not advance state.
- `ClientCore` runs a fixed list of named phases per cycle over disjoint
  sub-states. Cross-owner requests go through a typed effect queue, not ad-hoc
  flags.
- The UI records its 2D calls once per frame into a `FramePlan` of display
  lists; a `Toolkit` backend (reference GPU, modern, or null) executes them. The
  scene reaches renderers as a renderer-neutral `SceneSnapshot`.
- All I/O goes through one `Io` boundary (clock, sockets, worker completions,
  platform). A live, a recording and a replay implementation exist, which is
  what makes recorded sessions replayable without a window or a server.
- CS2 scripts run on `native910`'s VM through a `Host` trait that `rs910-ui`
  implements.

## Revision seam

Revision-specific facts are data, not engine code: opcode numbers and packet
layouts, login blocks, config decoder opcode tables, CS2 command ids and arity,
cache archive layouts. They live in `rs910-protocol`, `rs910-config` and
`native910`'s tables. Game, scene, UI and renderer layers do not carry them as
literals. This is what allows other revisions to be added later.
