# Engine and studio

Where Alto is going. Client completion comes first: the studio seams below are
built inside that work so nothing has to be redone, and the content pipeline
and reflection come before any editor.

## Mission

Alto becomes an open-source RuneScape **engine and studio**, not only a client.

- **Engine.** It runs the revision 910 client with behaviour checked against
  recordings. It is built so later revisions load through a revision seam (rules
  and tables as data), and it draws with a modern renderer by default (`--renderer modern`,
  [`renderer/modern-renderer.md`](renderer/modern-renderer.md)).
- **Studio.** Content can be made with it: an interface editor, a map editor, a
  CS2 IDE and debugger, model, animation, particle and material editors,
  config and variable editors, dialogue and quest graphs, packet and variable
  inspectors. Play-in-editor runs the real client core against the dev server.
- **Everything is a node.** One reflection and schema system covers a scene
  tree (world, region, tile, loc, NPC, light, water, emitter; interface,
  component, child), graphs (render graph, material and particle graphs,
  dialogue and quest, CS2 flow) and an evidence graph that answers questions
  such as "what reads this varbit?" and "what makes this button visible?".

What keeps it maintainable for years:

1. The gates are the spec: replays, goldens, function-body hashes and the
   layering check (`tools/refactor/README.md`).
2. Revisions are data, not engine code.
3. A few stable, versioned seams: `Io`, `SceneSnapshot`, the revision seam, a
   `Reflect` and `NodeId` scheme, and an extension API.
4. It stays legally clean: no game code, jars, shaders or cache data in the
   repository.
5. wgpu for portability.
6. Headless and deterministic operation.
7. Small crates, fast tests, and CI that needs no private data.

## Engine systems and their state

| System | State | Direction |
|---|---|---|
| Frame timing | Logic runs on the fixed 20 ms cycle; the render rate is separate. Render-side work that still writes game state is being split from presentation. | Interpolated presentation, decoupled from logic. |
| Threading | Named single-purpose threads plus a small frame job pool in the modern renderer. | One fixed job pool with named joins, used by loading, scene build and rendering. |
| Asset streaming | Cache groups, maps, models and textures load on workers; region changes rebuild the scene. | Per-group invalidation, incremental per-square scene build, streaming budgets. |
| Render graph | The modern renderer declares its passes as data; the 2D UI is a recorded frame plan executed by a toolkit backend. | Render into an editor-owned texture, not only the window surface. |
| World and entities | Entity state in `rs910-game`, scene in `rs910-scene`, handed to renderers as `SceneSnapshot`. | Stable entity and node ids across the game and the editor. |
| Revision layer | Opcode, packet, config and CS2 tables live in protocol, config and `native910` data. | Generated protocol and CS2 tables from one registry; a second revision as the proof. |
| Audio | Native stack with a `cpal` sink. | Complete the remaining stream types. |
| Tooling and debug | Session record and replay, fixed clock, profiler, CS2 debugger API. | An inert observer API for inspectors. |
| Input | Native input enters the retained keyboard and pointer owners at event time. | Record accepted events with their payload and clock, then replay them at the same event boundary. |
| Extension API | Internal seams exist (`Shell`, `Io`, toolkit, `SceneSnapshot`, the CS2 host trait). | A public, inert observer and extension API. |

## Studio readiness

### Content pipeline

- `native910` decodes and re-encodes CS2 scripts, interface components (with an
  editable text form), several config kinds, database tables and sprites. It
  builds linked script/interface projects into new immutable packs, preserving
  untouched groups byte for byte and unedited component siblings. Source hashes,
  symbolic names, instruction maps and stack-analysis diagnostics accompany the
  build. Sparse multi-file interface groups retain signed stripe deltas.
- Repack works for scripts and interfaces. Maps, models, textures,
  animations and most configs still
  need encoders and repackers, and map groups need key handling.
- The dev server watches its pack directory and serves a rebuilt pack; the
  client picks it up through a full reload and relog. There is no per-group
  invalidation yet.
- The edit loop: dump selected scripts and interface frames, author their
  sources, build the pinned project, publish the generated archives locally,
  reload and relog. A client replay proves an authored frame through packet
  decoding, lifecycle and pointer hooks, runtime child creation and painting.
- `inspect --out-dir` exports a pinned static evidence graph and unresolved-site
  ledger. Cached hook-entry assumptions remain explicit; resource footprints
  retain opaque host effects. It is not a complete runtime causality graph.

### Reflection and schema

Runtime types are plain structs and each content type has two models: the
lossless authoring model in `native910` and the runtime decode projection in
the client. A derive-based `Reflect` trait (field list, type ids, get and set by
path, visit) fits as additive implementations. Undo, redo, diffing and
serialisation belong on the authoring model; runtime reflection is read only
(inspectors, the evidence graph). A node id is a small tagged value (`domain`,
`id`, `sub`) whose domain is one of script, component, config, var, asset,
loc, tile, entity or message.

### Editor host

Play-in-editor is feasible: the client core already runs without a window
behind a shell trait, and `Io` already has live, recording and replay
implementations, which is the basis of a scripted-server sandbox. Missing:

- an embeddable shell (the scene, entity and camera owners are in the shell
  binary today; they need to live in a library);
- rendering into an editor-owned texture;
- stable node ids from picking, and picking that resolves to a source
  definition (config id, interface source span, CS2 creator);
- an authoring command log with apply and revert, and a build, publish and
  reload transaction;
- group-level hot reload.

### First editors

- **Interface editor.** The text codec, CS2 source and bytecode round trip,
  cross-reference index and the retained runtime exist. Missing: interface
  automatic symbol-backed id allocation for new interfaces, reload of one
  interface group, a component view over one reflected model, a viewport,
  a property editor and an undo log. Runtime children now retain their creator
  script, instruction and event context; a live inspector still needs to expose
  that identity and the provenance of later property writes.
- **Map editor.** Decoders for map squares, tile flags, lights and the extra
  terrain files exist, as do two renderers, picking and the camera. Missing:
  encoders for terrain and locs with map-group repack, incremental per-square
  rebuild and partial GPU re-upload, a scene mutation API, gizmos and overlays,
  server collision regeneration and an undo log.

### Seams built inside completion work

1. Invalidate asset stores per (archive, group, generation), not per session.
2. Entity, loc-instance and tile ids, with picks that return them.
3. Expose the recorded runtime component creator identity to picking and the
   inspector, then retain the provenance of property mutations.
4. A scripted-server `Io` implementation on top of replay.
5. A render-to-external-target path in the frame graph.
6. Generate the protocol and CS2 tables from one registry shared by `native910`,
   `rs910-protocol` and the server tables.
7. Split the scene build into pure per-square steps.

## Studio layers

The studio is a development environment: select something in the running
game, navigate to its definition and behaviour, edit it, build it, run it and
inspect the whole chain of effects, then keep a reproducible test.

1. **Reproducible evidence.** Each experiment records the cache identity, patches,
   launch configuration, inputs and tool revision, so a scenario can be replayed and
   compared at the same semantic checkpoints.
2. **Lossless content workspace.** Imported cache bytes are a versioned base,
   authored sources are tracked inputs, generated packs are outputs. Untouched
   groups are preserved exactly; new scripts and interfaces get new ids;
   builds are atomic and can be rolled back.
3. **Executable semantic registry.** One structured registry generates
   documentation, compiler checks, interpreter dispatch metadata and coverage
   reports, with separate namespaces for CS2 instructions, client and server
   packets, login messages, zone messages, config tags and server script
   commands. Unknown effects stay explicit.
4. **CS2 tools and debugger.** Typed control-flow and dataflow analysis above
   the exact low-level form, source spans and maps, rename and find references,
   stepping, breakpoints and watchpoints.
5. **Protocol and causality.** Message schemas recovered from real read and
   write paths, a decoded packet inspector and replay harness, and traces that
   connect an action to its request, the server handler, the mutation and the
   client effect.
6. **Variables and content semantics.** A catalogue keyed by domain and id with
   storage, readers, writers and consumers, with observed correlation kept
   separate from established meaning.
7. **Interface runtime and studio.** Hierarchy explorer, canvas, property
   editor, symbolic hooks and a live inspector that distinguishes authored,
   computed, server-set and script-written values.
8. **Authoritative game runtime and RuneScript.** A versioned server language
   and runtime, developed in vertical feature slices; reconstructed rules are
   labelled as such, separately from recovered facts.
9. **The wider studio.** Terrain, locs, collision, models, materials,
   animation, particles, lighting, audio, cutscenes and world map as authoring
   domains sharing ids, builds, evidence and scenario tooling.

Underneath sits a typed evidence graph: nodes are scripts, components, vars,
configs, assets, messages, handlers and systems; edges are calls, reads,
writes, creates, installs-hook, sends, renders and depends-on. Every edge
records its source build, evidence and confidence (directly recovered,
statically proven, observed, inferred or unknown), and unresolved sites are
stored with their reasons.

Coverage is reported in separate measures (bytes decoded, bytes re-encoded,
commands specified, commands executed, references resolved, messages
understood, features editable). "Decoded" is never shown as "understood".

## Live session controls

The ordinary window remains the input owner. Scripted fixture recordings keep
their input isolation; `CLIENT910_RECORD_MODE=interactive` accepts native input
and records the canonical retained event, its timestamp and delivery order.
Replays apply those events between the same logic cycles. Loading and developer
console input remain with their existing owners.

An opt-in local `CLIENT910_CONTROL` socket observes the installed client session
and returns its cycle, map identity, local player, loaded entities, cache
operations and native interfaces. It accepts bounded requests, resolves targets
against that observed state and passes accepted actions to the ordinary input,
menu and route owners. It cannot write protocol bytes or change game state
directly. Replies distinguish admission from a later observed game result.

Content drivers choose one action from fresh state and wait for its actual
postcondition before proceeding. Walking uses the ordinary route pathfinder;
location and combat operations use the installed instances and cache options.
Stale identities, unavailable operations and blocked or timed-out progress
produce explicit failures. Recordings preserve the action sequence that actually
happened, so replay checks remain deterministic without imposing a timed recipe
on the live run.

Fixture-only headless observed sessions reuse the same startup and core owners.
An authenticated broker supplies that account's profile, server variables and
ordered world bytes. Startup prepares and acknowledges the real local pack map;
subsequent logic uses the wall clock. Controller admission occurs after the
logic frame, as in the windowed app, and its packet output flushes on the next
ordinary cycle. Authentication replacement drops buffered input/output and
requires a new owner. This backend does not render or establish a content
completion proof; each recorder still owns transport deadlines and final waits.
