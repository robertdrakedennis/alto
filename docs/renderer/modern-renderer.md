# The modern renderer

`rs910-render-modern` is the client's default scene renderer (`--renderer
modern`; the reference GPU renderer stays selectable with `--renderer
faithful-gpu`, alias `classic`). It draws the renderer-neutral `SceneSnapshot` inside the
reference UI (`rs910_render_gpu::render::Renderer::frame_composite`). The shell
answers every capability query with the reference renderer's profile and keeps
the shared CPU scene preparation running, so the client behaves the same whichever
renderer draws: packets, game state and saved preferences do not change (the
replay gate's `renderer_choice_is_observationally_inert`). Its pixels are not
bound by the reference pixel checks; the reference GPU renderer stays the
correctness reference. It draws the hardware toolkit modes only: in toolkit mode
0 (`displayMode` 0, the loading screens) the reference GPU renderer draws the
whole frame, so choosing the software display mode in the settings still reaches
the reference renderer. Which renderer draws is a command-line choice, never a
saved preference or a packet.

A frame the surface cannot take (the window is occluded, or acquiring the
texture timed out) is skipped by both renderers (they compose through
`Renderer::begin_frame`) and logged: `[client910] frame skipped: the surface is
occluded (skipped N in a row)` on the first, every 300th after it, and `frames
resume after N skipped` when drawing returns.

It is one renderer with quality settings, not a set of per-feature switches.

## Current frame-rate baseline

On 4 October 2026, after the performance changes merged, the owner reported
"33 to ~120 fps". This is the current owner-measured baseline. Resolution,
render scale and in-game location were not specified. The result describes
that observed client session; the earlier controlled measurements below retain
their recorded settings and scope.

## Shared frame preparation

The shell prepares one renderer-neutral scene on the main thread. The selected
scene renderer and active toolkit determine whether that frame also needs the
faithful GPU scene resources: hardware toolkits drawing modern keep CPU posing,
particle simulation, sky resolution, picking, model lights and actor write-back,
while skipping the faithful scene meshes and their frame uploads. Toolkit 0 and
the faithful renderer retain the complete faithful path. Interface resources
continue through the shared toolkit in every mode.

Static loc replacements are installed when their retained request or model
changes, with a scene rebuild resetting the installation cache. Ground stack
selection is cached at the ground-object packet revision and map generation;
NPC draw definitions are cached at their resolved type, BAS, tint and preference
revision. Animation nodes and placement remain live inputs. A transient is
omitted only when both its planned tile columns and posed model bounds reject
the view; particle-bearing models and nearby owners are retained. The modern
off-screen caster pass excludes temporaries, so planner-rejected temporaries
cannot contribute a shadow. Actor posing, bindings and state writes remain on
the main thread.

Interface drawing indexes each component array's children by layer once per
walk, retaining array order within a layer, recursive subtree order, attached
interfaces and the deferred drag pass. The index lives only for that walk so
later draws see structural changes.

## Layout

Each subsystem module holds its CPU half and its WGSL snippets. The GPU halves
(the `impl ModernRenderer` blocks, pipelines and GPU state) live with the
renderer in `frame/gpu/`. No subsystem depends on the frame, so the module graph
stays acyclic (`tools/refactor/dag-check.py`).

| Module | Role |
|---|---|
| `frame` | `ModernRenderer`: `prepare_frame` (CPU resources), `record_frame` (ordered units and post), `draw` (the eager wrapper), targets. `passes` declares the frame graph; `units` groups its passes into encode units; `jobs` is the worker pool; `posing` poses the frame's models on it; `pipelines` is the pipeline cache; `resources` holds the loc mesh cache and the per-frame arena; `arenas` the loc meshes' shared pages; `submit` the draw packets, their sorted depth-only lists and the bind tracking; `startup` creates the renderer on a background thread; `prebuild` builds a scene's loc meshes and materials on the pool ahead of their draws; `underwater` draws the seabed and its locs (below). |
| `frame::gpu` | The GPU halves: `ambient`, `atmosphere`, `sky_layers`, `post`, `probes`, `point_lights`, `env_reflections`, `sun_shadows`, `point_shadows`, `interior`, `sprites`, `terrain`, `water`, `caustics`, `far`. |
| `settings` | `ModernSettings`, the quality settings (below). |
| `modern_debug_flags` | Diagnostic variables: checks and debug views. |
| `shaders` | Every WGSL module, composed once from ordered snippets (`Module`, `Library`). |
| `models` | The draw list and vertex streams, materials, forward shading constants, static and animated models. |
| `lighting` | Environment sun and ambient, the environment record (colour remap, angle fog), the look, point lights, the per-square ambient (`ambient`: block maths; `ambient_schedule`: capture schedule, cache, blend), light probes and image-based lighting, environment mapping. |
| `shadows` | Sun cascades, quality presets, off-screen and roof-hidden casters, point-light shadows (`point`: candidates, slots, levels, atlas layout; `presets`: the tables and the shader block). |
| `atmosphere` | Light scattering, distance fog, the sky (its shading law, the cube cross-fade), volumetrics, the classic skybox layers (the cubes' source and the decor sprites). |
| `post` | Ambient occlusion, eye adaptation, bloom, the tonemap and grading composite, depth of field, FXAA. |
| `water_body` | Water surfaces and reflection, body effects (shadow, foam), caustics. |
| `terrain` | The extended terrain meshes and the texture atlas. |
| `sprites` | Model billboards and particles. |
| `far` | The far scene's batching: geometry with its LOD lists, merged loc containers and their per-frame draw runs, the build jobs the far workers run. |

The far scene's renderer-neutral half is its own crate, `rs910-far-scene`:
the draw-distance levels, classes and model LOD rule, the ring
(the map squares around the camera focus), the square decode and its shared
cache, the private loc placement and containers, and the worker pool.

### The far scene (render distance)

`ModernSettings::far` (default level 2) draws a render distance around the
camera focus beyond the game's normal build area: the far terrain (every level),
the far locs merged per 16 x 16-tile container with distance classes, loc
categories and a model LOD rule, and the near extension (the window's static
content beyond the normal reach, its opaque locs drawn from merged containers).
Squares and containers are built on worker threads and uploaded within a
per-frame budget; the render thread builds nothing unless
`CLIENT910_MODERN_FAR_SYNC=1`. The merged loc containers live in loc pages of
the far scene's own `frame::arenas::LocArena` (16-bit chunks, so they outlive
scene installs, which reset the loc meshes' arena) and are ordinary draw packets
(`Geometry::Far`, `frame::submit`): one per batch and LOD run, joined to the
frame's draw list where the near extension's per-loc draws go, so they reach the
depth pre-pass, the geometry pass, the water reflection and the forward pass like
any loc (never the shadow passes).

## Frame

`frame::passes::FRAME` declares every pass in frame order. Each declaration
gives the pass's stage, its resolution class (full, scaled or fixed), its sample
count (the forward target's or one), what it reads and writes, and where a
multisampled target resolves. Every pass begins through
`ModernRenderer::begin_pass`, which names it from the table. In debug builds
(tests included) it also asserts that the frame keeps the declared stage order.

| Stage | Passes |
|---|---|
| Probes | probe filter, sky and capture, projection; environment capture (only when the scene or the settled environment changes); the per-square ambient capture (one face a frame while squares are pending) |
| Sky (the frame's background, before the shadows) | the decor sprites, when the sky has any, into the forward target (MSAA, resolved to the sky source); then the sky shading (which clears the forward target first): the environment's cube fogged by elevation |
| Shadows | sun shadows: the static casters of stale cascades into the static atlas, then the cascade atlas's stale tiles (kept, restored, redrawn, dynamic casters on top); point shadows (one pass over the atlas: changed faces only) |
| Caustics | caustic rays, caustics resolve (compute) |
| Depth | depth pre-pass |
| Ambient occlusion | geometry, AO (SSAO or HBAO), blur x, blur y |
| Forward | forward lighting (MSAA, resolved at its end); with water, the reflection (culled to its frustum), then group 0 (resolved into the scene copy), the water surface (no resolve) and group 2 (resolved into the frame) |
| Atmosphere | volumetrics at half size (depth, march, bilateral apply), depth of field (focus, spread, blur, composite) over the resolved frame |
| Post | luminance, adaptation, bright pass, bloom (Kawase), composite, FXAA, and at a render scale other than 100% the upscale into the frame |

Resolution classes: *full* is the scene's size (the frame's target, or the
scaled scene viewport at a render scale, below), *scaled* a divisor of it
(the AO quality, bloom, the volumetrics' half size, a half-size reflection), *output*
the shell's drawable (the upscale), *fixed* a fixed size.

Resolves and transient targets. Each multisampled target resolves once, at the
boundary whose pass reads it (the sky into the sky source; forward group 0 into
the water's scene copy, or a copy of the scissor at one sample; group 2 or the
plain forward pass into the frame). The frame has two HDR resolves: an
atmosphere pass reads one and writes the other and the post chain reads the last
written, so nothing is copied back (except the volumetrics' result under the
depth of field, whose taps reach past the scissor). The sky source, the water's
scene copy and that twin are one texture (`passes::ALIASES`: their lifetimes do
not overlap, `aliased_resources_are_never_live_together`); the decor sprites draw
into the forward target, which the sky shading then clears; the occlusion before
its blur shares the occlusion map's texture; the depth of field's six targets
exist only while it is on.

Water reflection. The pass draws, in the frame's order, only the draws whose
bounding box (`models::bounds`, camera-local, per entity) meets the reflected
clip volume, whose near plane is the water plane (`frame::gpu::water_reflection`;
draws without a box, such as floors, are always drawn; far containers carry
their selected-model run bounds). A box outside the volume writes no texel, so the reflection is the one of
drawing every draw. Without a reflection plane the pass does not run.

Volumetrics run a half-size path: the farthest depth of each 2x2, the march over
it into a half-size scattering target, then a bilateral upsample apply at full
size. The AO chain defaults to full size, with a half-size quality choice.

The depth pre-pass and the AO geometry pass are not merged: their outputs
differ (the pre-pass writes the forward depth at the forward sample count for
the opaque entities; the geometry pass writes normals and positions at one
sample, with its own `Depth32Float`, for the terrain, the floors and the opaque
entities), and filling the forward depth with the floors changes what their
batch blending draws over. Merging them into one normal pass that also serves as
the depth pre-pass would change the AO and forward frames.

Bind groups follow update frequency: group 0 is the frame block and the AO map
(written once per frame); group 1 the material's own textures (what a draw
changes only for the materials outside the arrays, below); groups 2 and 3 the
pass inputs (the shadow atlas and its receive block, point lights, probes);
group 4 the material arrays (set once per pass). Per-draw data rides the
instance stream.

The declarations are where performance work starts: per-pass encoders (the
encode units, "Threading") and per-pass timestamps (the bench,
[`tools/perf/README.md`](../../tools/perf/README.md)) both read them.

### The sky

The sky is a cube map sampled by view direction (`atmosphere::sky`, `frame::gpu::sky_cube`),
cross-faded with the previous cube when the environment's cube changes, fogged toward the fog
colour by elevation, with a glow lobe around the sun and a constant exposure offset. It has no
gradient, sun disc or cloud layer. A frame without a cube shows the flat fog colour plus the
exposure. It is what both looks use (the level the cube's display-referred colour enters
the composite at is `Look::sky_exposure`, as it was for the classic sky layers).

- **The environment's cube** is the snapshot's sky target (`SkyFrame::target`: the environment's
  box, or the box it fades to). `atmosphere::sky_fade` holds the timeline: 5000 ms between cubes
  (an override's explicit duration replaces it), none for the first environment and for one
  installed without a transition (a teleport), going back to the previous cube swaps the
  roles and back-dates the clock so the picture does not jump, and a change to a cube that is
  not ready waits. The exposure added last is 0 in every frame; the ambient capture draws
  the same sky into each capture face (`ModernRenderer::prepare_sky_face`, the frame's cubes,
  blend, fog, glow and level) at its capture exposure (`lighting::ambient::CAPTURE_EXPOSURE`).
  `ModernRenderer::set_sky_exposure_offset` sets the exposure of the frame's own sky.
- **The cube's texels are a stand-in**: the 910 cache holds no cube-map sky, so each cube is
  the environment's existing sky box (its dome model, or its tiled material) drawn once from
  the origin into the six faces of an `Rgba16Float` cube of 512 texels a face
  (`frame::gpu::sky_cube::CUBE_RES`; 12.6 MB a cube, up to four kept. The sky is smooth, and
  the frames at 1024, 512 and 256 differ by at most one 8-bit step at 512 and three at 256), in
  the classic sky's display-referred colour over the fog colour of that moment, the dome
  model unfaded. A box is baked when first seen and again when what it is made of changes. The
  bake submits its own command buffer after the frame's uploads; it is not a pass of the
  frame. There is no distance scattering on the sky.
- **Drawn** by the sky shading pass (group 3: the previous and current cube) over the scene
  viewport before the shadow passes and the geometry, which covers it by depth.
  The frame itself records only the decor sprites (`prepare_sky`); the box's fills, material
  and dome model are drawn by the bake and by the probe captures (`capture_sky_models`), which
  record the dome only when a capture runs.
- **Decor sprites** (`SkyLayer::Decor`) stay screen-space layers over the cube: they draw into
  the sky source (transparent, premultiplied coverage) and the shading blends them under the
  fog. They fade with the cube cross-fade (`Blend::weight`).

### The underwater scene and sky decor

With water detail high the snapshot carries the underwater scene (`SceneSnapshot::underwater`:
the seabed's floor and the locs standing on it). `frame::underwater` draws it as
ordinary geometry below the water: the seabed is one more floor (slot
`BED_LEVEL` of the floor cache, every tile selected, never a shadow caster) drawn
after the level floors, and each loc is cached in a loc page like the scene's
static locs and drawn in the opaque or transparent entity phase. The water's
refraction and depth read that geometry, so the shallows show the bed. Without
the underwater scene (the default water detail) the frame is unchanged.

Sky decorations (`SkyLayer::Decor`) are baked sprites (`rs910_scene::sky_decor`)
the sky pass draws as squares over the sky, at the direction's
projection through the sky's rotation-only view (mode 3 of the sky layer
shader). No 910 content has a decor.

## Submission

How the model draws (locs, NPCs and players, floors, sky models) reach a pass
(`frame::submit`, `frame::arenas`):

- **Draw packets.** A `Draw` is self-contained: its geometry is an offset in
  a shared buffer set, its material an id (a layer of the material arrays,
  or the bind group of group 1), its per-draw data (model matrix, material
  parameters, point lights, the material's layer) an `Instance` record in the
  frame's instance buffer. The instance stream (vertex slot 2, instance
  step) is bound once per pass and each draw addresses its record by its
  first instance, so no draw changes a bind group or a uniform for its own
  data.
- **Material arrays.** Changing a bind group per draw was about 60% of the
  CPU time of a frame's draws (wgpu validates and tracks it, Metal re-emits
  the texture state; Lumbridge set 5,200 of them a frame). The RT5 diffuse
  and aux maps are layers of two 2D arrays of 128 texel layers
  (`models::material_arrays`): the 128 texel maps as they are, the 64 texel
  ones tiled twice over each way (a 64 texel map at half its coordinates:
  the shader halves `uv`, takes the 64 texel size and levels, and each mip
  level of the layer is that level of the map tiled). A material's layer and
  its tiling ride the high bits of the draw's flags in its instance record.
  The arrays and their repeat-both sampler are group 4 of the model
  pipelines, set once per pass, so a draw of an array material binds nothing
  of its material (`frame::submit`), and the shader samples the layer with
  the same explicit mip levels and addressing as the material's own texture.
  The aux map is only sampled for materials that have one, and the second
  mip level only with a fraction of the blend. The material's own texture
  and group 1 stay for what the arrays do not hold: materials that clamp or
  repeat on one axis only, 256 texel maps, RT7 atlases (their meta block must
  not be read by an array draw: `Bound` binds a neutral group 1 after one),
  materials past the 511 layers, and the billboards and particles, whose
  vertices carry no layer (Lumbridge still sets 750 groups a frame for them).
  There is one array pair and one sampler because a shader that picks a
  texture or a sampler by a per-draw value costs the GPU more (about 0.4 ms
  of Lumbridge's 8) than the bind group changes it saves the CPU.
  `rs910_gpu_device::renderer_limits` asks for the 512 array layers and the
  five bind groups; with fewer layers fewer materials are arrayed.
- **Shared geometry.** Every loc mesh lives in a few large loc pages (one
  vertex, colour and index buffer each, 512 Ki vertices and 2 Mi indices;
  a bigger mesh gets its own page): a mesh is a base vertex and a first
  index. 16-bit indices per mesh stay. A dynamic loc's next pose is written
  into its ranges while it fits, else it moves and its old ranges are freed
  at the next frame. The cache policy (one mesh per scene slot, the scene
  token, `EntityKey::changes`) is `frame::resources`'; a scene change frees
  every range and keeps the pages. Posed models and sky models share the
  per-frame arena; floors keep their per-level buffers.
- **Bind tracking.** `Bound` holds what a pass has bound: a draw sets its
  page (vertex slots 0 and 1, the index buffer) and its own-texture material
  only when they differ from the draw before, and the arrays once. Anything
  else drawn in between (terrain, far scene, sprites, water, a capture's
  floors) resets it.
- **Sorted depth-only lists.** The sun shadow cascades and the depth
  pre-pass write depth only, and the depth test keeps the nearest depth
  whatever order the draws come in, so their lists (the pre-pass's
  `FramePackets`, the shadow cache's per-cascade packets) are sorted by
  material, buffer set and first index, with the draw's index as the tie. Each
  page's draws then bind once per list (and each own-texture material's too). The forward, geometry (it
  writes normal and position, where equal depths keep the last draw), water
  reflection, point-shadow and capture passes keep the draw order.
- **Frame packets.** `FramePackets` (the pre-pass) and the shadow cache's
  per-cascade packets (`frame::gpu::shadow_cache`: each cascade's dynamic
  casters, and its static ones only in a frame that redraws them) are built
  once at the end of `draw` (after the cascade masks) and only read by
  `encode`, like the draw list itself, so a parallel encode can take the
  lists as they are.

Repeated models are not instanced: locs rarely share a model (the headless
bench's Lumbridge view draws 2,005 locs with 1,929 distinct models, Draynor
1,030 with 874), so instancing by model would merge at most 4-15% of the loc
draws.

## Ambient

The lit surfaces take `ambient colour * SH(normal) * SSAO`. With the verified
look (`LookMode::captured_ambient`) `SH` is one second-order irradiance block
per map square (64 x 64 tiles), seven `vec4`s holding the cosine convolution
over pi, so a white surround shows 1.0 for every normal
(`lighting::ambient`). The block is the projection of a capture of the world
(`frame::gpu::ambient`, schedule `lighting::ambient_schedule`):

- **Capture.** Every square the scene window touches is seen from above its
  loaded bounds (centre, `EYE_CLEARANCE` above the top of the terrain of every
  level and the locs on it) in the six axis directions, 90 degrees, near 512,
  far 65536, with the flat ambient, no sun shadows and no SSAO, the sky layers
  and models and the frame's fog. At most one face a frame, squares in load
  order: `encode_ambient` renders the face in the probes unit (it reuses the
  probe captures' candidates, culling and draws, `frame::gpu::probes`) and
  copies it to a readback buffer; the buffer is mapped after the frame's
  submission and read a frame or more later, so nothing waits for the GPU.
- **Projection.** The sixth face of a square goes to a worker thread: the
  texels become the tone-mapped display byte over 256, are weighted by their
  solid angle `4 / (1 + x^2 + y^2)^1.5`, projected onto the second-order real
  basis, scaled by `4 pi / sum(weights)` and packed.
- **Cache and blend.** A block is cached under the environment's key (colours,
  fog, sky layers, and how many sky textures are ready: a sky that arrives
  asks for every square again) and the square. The key must hold two frames
  before squares are requested (a colour transition changes it every frame); a
  new key drops the pending work. A square shows its current blend: the block
  shown when a result arrives mixed into the new one, all 28 floats, over
  750 ms; the first result blends from the default block (up sky blue, down
  dark).
- **Shading.** The shader reads a table of 16 x 16 squares around the camera
  (`lighting/ambient.wgsl`) and takes the cell under each fragment; squares
  that are not loaded (the far scene's) take the nearest loaded square's block.

Stand-ins, each a named constant: the face size (`FACE_RES`, 128), the capture
target's tone mapping (the renderer's own), the environment record's capture
offset and exposure (`CAPTURE_OFFSET`, `CAPTURE_EXPOSURE`; not decoded, zero),
the capture order, that the capture draws the near scene only (no far scene, no
water), keeps the near scene's model detail and the frame's point lights, one
block per square for all levels, and a block per fragment where the reference
binds it per node.

The classic-calibrated look keeps the zone probes of `lighting::probes`
(9 x 9 per window, captured in one go), which its values were calibrated with.
Under the verified look (the default) only the global probe and the environment
cube are captured (the water's sky, reflections and env masks read them), not
the zone grid.

## The look

The default look (`LookMode::Verified`, `lighting::look::Look::verified`) takes
what the evidence gives: the sun and ambient colours from the environment record
with no multiplier (the sun gain below aside), the record's tone map and filmic
operator, its colour remap at its data weights, and the ambient captured per
square. Three values the evidence does not give are labelled stand-ins, each tuned against
statistics of the official look reference shots (kept outside the repository)
and nothing else. The shots show newer models and textures, so only lighting,
shadows, haze, sky and colour balance are compared: the luminance percentiles,
the first and last of them over each other (the shadow contrast), the colour
balance, and region means.

| Stand-in | Earlier look | Verified look | What it targets |
|---|---|---|---|
| sun gain (`Look::sun`) | 2 (with light unit 3) | 2.8 | The 910 data's albedos (tile colours, textures) are darker than the modern assets', and the modern client has no multiplier; the ambient, which the shadows show, stays at its proven level (1; the earlier look's was 3). The roof view's luminance percentiles land on the reference's: median 0.324 against 0.324, darkest 5% 0.115 against 0.109, brightest 5% 0.625 against 0.630, mean 0.335 against 0.339. The castle view's shadow contrast (5th over 95th percentile) is 0.168 against 0.146 |
| in-scattering scale (`Look::scatter_inscatter`) | 1.5 | 0.65 | The aerial view's haze: mean luminance 0.474 against 0.433, blue over green 0.78 against 0.81. At the proven light level 1.5 gives 0.39 and 0.98, a lilac fog over the near ground (saturation 0.15 against 0.42). The aerial view's darkest 5% stay lifted (0.28 against 0.15) |
| volumetric sky share (`Look::volumetric_sky_share`) | 0.5 | 0.1 | The sky's colour (the volumetric scattering also lights the sky): reference `(0.83, 0.86, 0.98)`, saturation 0.15, blue over red 1.18. With the gain, 0.5 whitens it to grey `(0.97, 0.97, 0.96)`; 0.1 gives `(0.81, 0.87, 0.94)`, saturation 0.14, blue over red 1.16 |

The sky's own level (`Look::sky_exposure`) and the haze density
(`DENSITY_SCALE`) stay: with the values above the sky's mean colour is within
0.03 of the reference, and sweeping the density (0.1 to 0.25) moved the aerial
view's saturation but not its luminance statistics (the reference's 0.42
saturation is largely the newer textures'). The SSAO values, the volumetric
scattering and extinction, the specular constants and the ambient capture's
offset and exposure have no statistic here that separates them and are
unchanged. The constants of the earlier look keep their values:
`classic-calibrated` draws the frames it did.

Reproduce with the offline views of `modern_bench.sh`: the four reference
framings are the scenes `ref-aerial`, `ref-castle`, `ref-roof` and
`ref-street` of `tools/perf/modern_perf_bench.rs`, with the statistics in
`tools/perf/look_stats.py`.

## Shadow caches

The shadow pass costs CPU, not GPU, so the shadow maps are kept while what
they show is unchanged (`shadows::cache`, GPU half `frame::gpu::shadow_cache`):

- Each caster draw is static or dynamic. Dynamic: posed this frame (the
  arena: NPCs, players, projectiles, spot anims), a dynamic loc whose model
  changed in the last 30 frames, a scrolling material, or a loc draw no
  visible entity owns. Static casters are summarised per cascade by an
  order-independent signature (the owning entity's key, the loc page
  offset, material, index range, instance parameters, scene-local matrix),
  with the terrain's and roof-hidden tiles' selections. (A loc mesh is
  rewritten in place on a new model, so the key, not the page offset,
  identifies its content.)
- A cascade whose fit and static signature are unchanged keeps its map.
  With dynamic casters, the static ones live in a second atlas; the tile is
  restored from it and the dynamic casters drawn on top (a depth map keeps
  the nearest depth, so this is the map of drawing all of them, bit for bit).
  Only stale tiles are cleared. The off-screen static casters are not even
  prepared in a frame that keeps every map.
- A moving camera keeps cascade *k*'s fit for up to 1, 1, 2, 3 frames: the
  far cascades are redrawn every second or third frame. The map is read
  through the fit it was drawn with, so the shadows do not move; only the far
  cascades' coverage edge lags, by at most 1 (cascade 2) or 2 (cascade 3)
  frames of camera motion. A jump of 1/16 of the cascade's radius, a zoom or a
  turned sun refits at once.
- Point-light shadow faces are redrawn only when their light changed, their
  static casters changed or dynamic casters meet them (now or at the last
  draw); see "Point-light shadows" below.

A still scene with a still camera draws no caster after the warm-up and its
frame is the frame of drawing every map. Memory: the static atlas is the cascade
atlas's size (16 MB at LOW-HIGH, 64 MB at ULTRA, 256 MB at ULTRA+), created when
a cascade first has dynamic casters.

## Point-light shadows

Point shadows exist exactly when the sun shadows do, at the sun shadows'
quality (`ModernSettings::shadows`; there is no separate setting). The rules
are `shadows::point` (with their grades: proven, or a labelled stand-in) and
the tables `shadows::presets`; the GPU half is `frame::gpu::point_shadows`.

- **Technique.** Each shadowed light owns six 90 degree cube faces in one shared
  2-D depth atlas, sampled with hardware depth compare. The stored depth is the
  squared distance to the light over the squared radius (linear, written
  explicitly), so the lookup compares `d² / r²` (plus the level's bias, after
  moving the receiver along its normal by `50 * (2 - attenuation)`). A slot owns
  a block of the atlas holding its faces at every resolution level (level `k`
  is `face >> k`); the block in the shader names the current level's six face
  rectangles. The quality picks the filter (2x2 box, 12-tap disk, 4x4
  approximation, 4x4 box), the fade by view distance, the slot count (2, 2, 3, 4,
  4), the levels (2, 3, 3, 4, 4), the level-0 face size (256, 256, 512, 512,
  1024) and the maximum distance (10000, 12500, 16667, 25000, 25000).
- **Selection** (`frame::gpu::point_shadows::select_point_shadows`, early in the
  frame). Candidates are the lights that cast shadows, are on, reach 350 units,
  have their 0.8-radius sphere in the camera's frustum and are nearer than the
  maximum distance. They are scored (distance to the sphere's near side, radius
  and intensity, `f32` arithmetic) and sorted; a light takes a free slot in that
  order and keeps it (a better newcomer never evicts), so at most 4 lights ever
  cast. The level follows the light's projected size (big: level 0; tiny: the
  lowest); a level change redraws that light's faces.
- **Faces.** The face camera sits at the light (near 0.25, far the radius). Casters
  are the frame's shadow-casting entity draws (visible and off-screen) within the
  radius plus a margin, culled per face; floors and the terrain do not cast (they
  are the receivers under a lamp). A face is drawn only when its frustum meets the
  camera's.
- **Reuse.** The reference client draws every visible face every frame; this
  renderer draws a face of a slot's level only when its light changed, its static
  casters' signature changed or dynamic casters meet it (now or when it was last
  drawn), so what a face holds is what drawing it gives
  (`frame::tests::point_shadows`: the kept faces equal a redraw). Off-screen
  static casters are prepared only when they lie within a shadowed light's reach.
- **Stand-ins** (not provable): the view-axis sign of the priority score, the
  order of equal scores, the slot release (8 frames out of the candidate set),
  the atlas packing, the per-light cast flag (the map data has none: every light
  has it), and the reuse above. The half-resolution copy of the atlas for the
  volumetric march is not made: this renderer's volumetrics do not march point
  lights. Casters come from the frame's draws, not the whole scene graph, and
  the small-object cull has no data source.
- **Captures.** The per-square ambient capture draws without point shadows (its
  light bind group carries a block of none); the frame and the probe captures use
  them.
- **Memory.** The atlas exists once a light is slotted: 1024 x 1024 (LOW,
  MEDIUM), 4096 x 2048 (HIGH, ULTRA), 8192 x 4096 (ULTRA+, 128 MB).

## Threading

The frame's CPU work runs on a small pool (`frame::jobs`), a fork and a join
inside `draw`: `Jobs::map` runs job `i` for every `i` on the render thread and
the workers, results in job order. It stands in for the engine's shared job pool
(`rs910_core::jobs`), whose fixed pool and named join will replace it.

Thread budget. Beside it run the far scene's streaming pools
(`rs910_far_scene::far_jobs`): up to three terrain workers and two loc workers,
busy while the ring streams in and idle once it is built. The frame's pool takes
the cores those five leave, at least two and at most four threads, the render
thread included (`frame::jobs::default_threads`): on a 10-core machine the
render thread and three workers. `CLIENT910_MODERN_THREADS=N` sets the frame's
count; `1` runs every job on the render thread, in order (the synchronous mode
for tests and determinism debugging).

- **Encode units** (`frame::units`). The passes of `frame::passes::FRAME`
  are grouped into units: probes, sky, sun shadows (static tiles and casters),
  point shadows, caustics, depth pre-pass, ambient occlusion, water
  reflection, forward (with water: its group 0), the water surfaces and
  forward group 2, atmosphere and post (the small passes share a unit: a
  command encoder costs wgpu about 170 allocations). Each unit
  records into its own command encoder on whichever thread takes it (the
  longest first, `CLAIM_ORDER`); `encode` submits the command buffers in
  the declared order (`UNITS`) with one `queue.submit`, so the GPU runs the
  passes in frame order whatever order the threads finished in. The post
  chain writes the frame's own target, so it records into the frame's
  encoder (the shell's, submitted after `draw`). A unit only reads what
  `draw` prepared (`&ModernRenderer`); the few prepare-only values that are
  not `Sync` live in `exclusive::Exclusive`. Debug builds check that each
  pass begins in its own unit and in stage order on its thread, and that
  the units in submission order keep the frame's order.
- **Posed models** (`frame::posing`). The models posed every frame (the
  draw list's entities without a cache key) find their animation maps on the
  render thread in the draw order, are posed on the pool from their model and
  map alone, and are taken by their draws in the draw order, so the arena,
  the instances and the counters are what posing each at its draw gives.
- **Determinism.** The submission order is fixed, results come back in job
  order, and nothing the frame shows depends on which thread ran a job or
  when; the synchronous mode records the same command buffers on one
  thread (`threaded_and_synchronous_encodes_draw_the_identical_frame`).
- **Plugging in.** A subsystem's passes are a unit: a row in `UNITS` (and
  `CLAIM_ORDER`) and an arm in `ModernRenderer::encode_unit`.
- **Where it stops scaling.** wgpu validates and tracks every pass under
  state the threads share (registry lookups per state change, reference
  counts of the shared bind groups and buffers, the textures' init-tracker
  locks), and the Metal driver encodes each pass on its thread: a unit
  records 1.3-1.9x slower with three others running than alone, so three or
  four threads is the useful count (the default caps it at four; eight
  threads do no better). wgpu 30 (which replaced 22 for its Metal compile
  path, see "Start-up") records a pass cheaply and validates and encodes it
  when the command encoder finishes, and its per-bind-group costs (the
  validation and texture tracking of every set, Metal's state update) made the
  Lumbridge frame's draw 8.5 / 7.5 / 6.6 / 6.4 ms on 1 / 2 / 3 / 4 threads
  against 7.3 / 5.9 / 5.4 / 5.0 on wgpu 22. Nearly all of that was the
  5,200 material bind group changes: with the material arrays ("Submission")
  the draw takes 4.4 / 3.6 / 3.3 ms on 1 / 2 / 4 threads (Draynor 2.6 / 2.2 /
  2.1, river 3.2 / 2.6 / 2.4, castle 1.8 / 1.7 / 1.5), the encode is 0.65 ms of it on four
  threads, and the CPU work of the frame (cycles of all threads) grows 25%
  from one thread to four where it grew 70%. What is left is the
  serial prepare (off-screen casters, far scene, entities, uploads). Render
  bundles did not help on wgpu 22: executing a bundle in the pass cost about
  75% of recording its draws directly.

## Render scale

`ModernSettings::render_scale` (`frame::scale`; 50-200%) renders the scene at
that fraction of the scene viewport's pixels per axis. At 100% nothing changes.
Otherwise every scene target is the scaled viewport's size with
the viewport at its origin; the post chain runs at that size; the composite (and
FXAA) write the display frame into a scaled LDR target, and the upscale pass
(`post/upscale.wgsl`) resamples it into the shell's viewport and scissor,
bilinear. The reference UI draws over it at the drawable's resolution, so it
stays crisp.

The camera keeps the viewport's own size (the projection is in viewport pixels),
so the scaled image is the same view and a world point lands on the pixel the
full-resolution frame, and the client's picking (which works in viewport
coordinates, unaware of the scale), puts it (`frame::tests::scale`). Everything
measured in pixels (the SSAO and HBAO radii, the bloom and volumetrics sizes,
the water's reflection) follows the scaled viewport; the sky's decor sprites keep
their viewport-pixel placement. The shell's physical and logical sizes are unchanged:
the renderer gets the drawable's size and viewport as before and scales inside.

The default is automatic (`RenderScale::auto`): 100% on a low-DPI display (window
scale factor below 1.25), and on a high-DPI one the share of the scene viewport
that renders about 1600 x 1000 pixels (`HIGH_DPI_BUDGET`), per axis in steps of 5
within 50-100%: 50% for a full-size window at scale factor 2, 100% for a window of
up to 1600 x 1000 physical pixels. The shell tells the renderer the window's scale
factor (`ModernRenderer::set_display`, at the window's creation and when it
changes). Precedence: `CLIENT910_MODERN_RENDER_SCALE`, then the saved choice, then
automatic. The saved choice is one line in `players/modern-renderer.conf` beside the
preferences (`render_scale=auto|50..200`), written by the developer console command
`renderscale auto|50..200` (`renderscale` alone reports it); it is the client's own
file and changes no option or packet. The effective scale is logged when it changes
(`[modern] render scale 50% (automatic: scale factor 2.00, scene viewport 6400000
pixels)`).

Measured online on an M1 Max (Retina, 1600 x 1000 logical, 3200 x 2000 physical,
1x MSAA, the Draynor teleport scene, two runs each): at 100% the redraw takes
40.9 ms (24 fps) with a GPU span of 26.4 ms; automatic (50%, 1600 x 1000 scene
pixels) takes 24.5 to 25.3 ms (39.5 to 41 fps) with a GPU span of 9.8 to 10.7 ms.
What remains of the wall time is the interface and composition at the full
drawable and the logic.

## Shaders and pipelines

Each shader module is a fixed, ordered list of `.wgsl` snippets. The snippets
live with their subsystem, plus a few constants generated from Rust
(`models::shading::constants_wgsl`). `shaders::Module` names each module and
its variant:

- `Forward`: the lit models and floors, depth pre-pass, casters, sprites, probe
  capture and water reflection.
- `AoGeometry`.
- `Terrain`.
- `Water { multisampled }` and `Atmosphere { multisampled }`: the scene depth's
  binding type follows the sample count.
- `SkyLayers`.
- `Post { look }`: the composite's tone-map operator follows the look.
- `ProbeFilters`, `ProbeProjection`, `CausticsResolve`, `ShadowFill` (the
  shadow maps' tile clears and restores).

`shaders::Library` composes and compiles each module once. Nothing edits shader
text at run time. `shaders::tests::every_module_composes_to_valid_wgsl` parses
and validates every module and variant with naga.

`frame::pipelines::Variants` is the one pipeline cache. Every set is keyed by
its module variant and the forward target's sample count. It covers the
forward-target set (forward, no-depth-write, depth pre-pass, sky layers,
sprites), the terrain's lit pass, the water set (by sample count and
debug view) and the atmosphere set. `ModernRenderer::new` builds every set the
frame draws with: the forward-target set, the probe capture, terrain, water,
atmosphere and the post chain. No pipeline is built lazily in the first frame
(the sky's capture-face pipeline, which only the verified look's ambient capture
uses, is built with its first face).
An anti-aliasing change (`set_samples`) builds only the sets that depend on
the count, once per count, so switching back compiles nothing;
`prepare_sample_counts` builds a list of counts' sets ahead.

### Start-up and a scene's first frame

- **Creation off the render thread** (`frame::startup`). The shell starts a
  `Startup` when the session's first anti-aliasing level arrives (the login
  screens) and takes the renderer at its first use; the thread runs
  `ModernRenderer::new` and then, with a warm shader cache (creation under
  `startup::WARM_MS`), the pipeline sets of the other counts the client can
  switch to (1, 2 and 4 samples where the HDR target supports them), so
  neither the first scene frame nor a later anti-aliasing change compiles.
  `ModernRenderer::new` creates its independent pipelines at once
  (`frame::compile`: scoped threads, results in argument order, one thread in
  the synchronous mode): the forward, post and shadow sets together, then the
  probe capture, caustic, terrain, water and atmosphere sets together
  (`select_count_sets`). wgpu's Metal backend (wgpu 30; wgpu 22 compiled under
  the device's lock) compiles on the threads that ask, so a cold shader cache
  (first run after a shader or driver change) takes 2.0 s instead of 6.9 s and
  a warm one 40 ms instead of 75. A cold compile on the startup thread still
  takes the cores the login screens use. wgpu's `PipelineCache` is still a
  no-op on Metal (Vulkan only), and the OS's shader cache is what makes the
  warm case fast.
- **Scene builds ahead of their draws** (`frame::prebuild`). Before the
  entity loops (visible entities, off-screen casters, the probe capture's
  locs) the loc meshes not cached yet are built on the pool: their shape
  models decoded there (`models::rt7::decode_models`), the model streams built
  (`Rt7Plan::build`, split from `Rt7Cache::streams` so it touches no cache)
  and their new materials' maps decoded (`Textures::prefetch`, a floor's
  batches likewise); `prepare_entity` and `Textures::ensure` take them at
  the draws and count and upload them there, so the frame is the one of
  building everything at its draw
  (`frame::tests::lifecycle::builds_ahead_of_the_draws_leave_the_frame_unchanged`).
  The terrain's layer texels are made on the pool too
  (`TerrainScene::build_with`).
- **Staged buffer writes** (`rs910_gpu_device::uploads`). Modern meshes,
  instances and uniforms use the device's persistent staging belt, as the
  reference renderer does. Loc-page writes still coalesce adjacent ranges;
  each run copies into a reused belt chunk. Every submission, including a sky
  or probe bake, submits the ordered pending copies first. A prepared frame
  whose drawable cannot be acquired rolls back unsent producer progress and
  submits only its pending uploads. Cache entries keep their installed bytes,
  and repeated occlusion does not accumulate an unsubmitted copy encoder.
- **Animated loc streams** (`frame::posing`). Stale dynamic locs join the
  frame's existing pose jobs. The streams are retained by model address for
  the frame and reused by its draws, captures and shadow passes; the loc mesh
  cache keeps them installed across frames whose pose revision is unchanged.
  Classic CPU poses still supply bounds, picking and overlay heights. Modern
  scenes omit the reference floor hard-shadow raster and cast their GPU shadows.

## Presentation and pacing

The shell keeps the completed game canvas separately from the surface drawable.
A present between logic cycles copies that retained canvas and performs the same
canvas scale-up and screenshot step; it does not prepare or draw a new scene.
Full redraws still own UI painting, camera write-back and the native redraw
counters. A resized canvas or recreated device receives a full redraw before its
new dimensions become a retained presentation source. A direct switch between
modern and toolkit 0 invalidates the retained canvas too: the next full frame
must come from the newly selected scene backend, even when the device survives.
Faithful and null toolkit-answer changes keep their existing presentation path.

The modern frame has three phases: prepare its scene resources without holding a
drawable, acquire the surface, then record the ordered units and canvas post/UI
commands. Those command buffers go through one device submission, followed by
ambient readback scheduling and presentation. The scene data, pass order and
faithful composition remain unchanged. Surface acquisition failure skips scene
recording. The prepared token restores unsent probe progress, exposure history
and shadow-cache metadata; its ambient face returns to the same capture round.
Successfully submitted sky bakes and queued uploads remain installed. Explicit
staging encoders retain ordered copies until an actual device submission; a
skipped frame cannot discard writes while their cache pages are marked ready.
No game state leaves the logic thread.

The event loop uses the existing logic timer's pending wake deadline with
`WaitUntil`; it does not resample or advance that timer to choose a wake. Native
events can wake it early, while fixed-clock diagnostic sessions keep polling and
run one recorded cycle per callback. The CPU-usage option retains its existing
frame-tail sleep and packet-visible redraw counters stay on full redraws.

## GPU budget settings and redraw

Ambient occlusion has a resolution quality choice, `AoResolution::Full` or
`Half`, independent of its estimator (`Off`, SSAO, HBAO or HBAO Ultra). Full
remains the default. Half renders normal, position, depth, occlusion and blur
at half the scene's width and height, rounding target extents up. Its viewport
and scissor use the same divisor; its sampling radii use that viewport, and
its blur measures tap reach in scene pixels (seven Full taps over ±3 pixels,
three Half taps over ±2 pixels). Forward model, terrain, probe and
caustic shading map their scene pixels to the occlusion target with the same
divisor. This also applies when the scene already uses a render scale.
Half deliberately trades thin-feature occlusion and edge detail for less GPU
work. It is a modern quality choice and never changes the faithful renderer,
the options codec, simulation state or draw order.

Merged far loc containers do not enter the occlusion geometry pass. The
near scene's entities and ground retain their occlusion; distant containers
therefore lose their screen-space contribution to ambient darkening. This is
an intentional modern quality difference. Far terrain still supplies ground
depth. The forward and depth passes retain the containers. Their compatible
consecutive packets share an indexed indirect argument stream; supported
devices can issue a run with multi-draw, while the fallback preserves each
packet's original order. The reflection list also tests the camera-local union of the models actually
submitted by each compatible run against the reflected clip volume. A rejected
container cannot write a reflected texel.

Point-shadow face invalidation uses each caster's posed camera-local bounds,
its actual light radius and face clip volume. The previous fixed three-tile
margin is only a conservative preparation hint for deferred off-screen
models; it must not make an unrelated moving model redraw a cached face.
Unknown bounds remain conservative. A changed static signature, light or
scene still invalidates the cache, a dynamic caster overlapping a face still
redraws it, and a caster leaving a face clears the old depth once. A skipped
submission restores the cache's previous progress, as for the other frame
producers.

## Settings

`settings::ModernSettings` is passed to `ModernRenderer::new`. The shell fills
it with `ModernSettings::from_env()`: the defaults plus the dev overrides. It
is never written to the client options or the preferences.

Where the client has an option, the option drives the renderer:

- the shadow options (scenery shadows, shadow quality, character shadows),
  through `set_shadow_settings`;
- the anti-aliasing level, as the forward sample count (`set_samples`) and
  FXAA at one sample;
- the reference toolkit's bloom state (`set_faithful_bloom`).

| Setting | Default | Dev override | Notes |
|---|---|---|---|
| `shadows` | the options | `CLIENT910_MODERN_SHADOWS=off\|low\|medium\|high\|ultra\|ultraplus` | client options 0-4 map to the presets |
| `ao` | HBAO | `CLIENT910_MODERN_AO=off\|ssao\|hbao\|hbao-ultra` | |
| `ao_resolution` | full | `CLIENT910_MODERN_AO_RESOLUTION=full\|half` | half-size AO geometry and map; intentional modern quality difference |
| `env_reflections` | proven | `CLIENT910_MODERN_ENV_REFLECTIONS=all` | `all` adds materials whose reflection use is unproven |
| `far` | level 2 | `CLIENT910_MODERN_FAR=off\|0..4` | render distance levels |
| `volumetrics` | on | `CLIENT910_MODERN_VOLUMETRICS=off` | |
| `bloom` | the reference toolkit's bloom state | `CLIENT910_MODERN_BLOOM=on\|off` | client option |
| `fxaa` | on at one sample | `CLIENT910_MODERN_FXAA=on\|off` | client anti-aliasing level |
| `dof` | off | `CLIENT910_MODERN_DOF=on` | |
| `look` | verified | `CLIENT910_MODERN_LOOK=verified\|classic-calibrated` | `verified` uses the proven values, the per-square captured ambient and the stand-ins of "The look"; `classic-calibrated` is the earlier look (zone probes, its own light multipliers and tone map), kept for comparison |
| `reflections` | full | `CLIENT910_MODERN_REFLECTIONS=off\|half\|full` | half-size or full-size water reflection |
| `render_scale` | auto | `CLIENT910_MODERN_RENDER_SCALE=auto\|0.5..2` (or `50..200`) | auto is 100% on a low-DPI display and about 1600 x 1000 pixels of scene on a high-DPI one ("Render scale"); the console command `renderscale auto\|50..200` sets and saves a choice |

Diagnostics (`modern_debug_flags`, and `rs910_far_scene::far_debug_flags`):

- `CLIENT910_MODERN_CHECK`: compares the draw list, terrain, models, billboards,
  particles and far scene with the reference lists, and logs mismatches.
- `_TEXTURES=bc|etc|png`: the model texture source.
- `_GRADING=off`: the ungraded frame.
- `_PROBES=sh|ibl|metal`: probe debug output.
- `_WATER=env|debugN`: water debug output.
- `_TERRAIN=no-spec|levelN`: terrain debug output.
- `_FAR_SYNC=1`: build the whole far ring before each frame.
- `_THREADS=N`: the frame's threads, the render thread included (`1`:
  synchronous; see "Threading").

## Provenance policy

- The reference renderer and the game's data define the shared scene: draw
  order, floors, lights, billboards and particles.
- The modern renderer's frame structure, shaders and effects are our own
  design and our own WGSL and Rust. No shader source text, binary or asset of
  any game client is copied into this repository (see `NOTICE`).
- The look's stand-ins are tuned against real screenshots kept outside the
  repository, never against frames of the reference renderer.
- Revision numbers and facts about other clients appear nowhere in module,
  type, function, test, setting or file names.
- Only values that can be established with certainty from data or measurement
  drive behaviour; anything else is an explicit, documented default.

### Cache data the renderer reads

The 910 cache carries extra data used only by the modern renderer, decoded by
side tables in `rs910-config` (`nxt` module) that leave the reference decoders
unchanged:

| Data | Meaning |
|---|---|
| Material extras (materials archive, version 1 records) | Texture references (diffuse, normal, compound maps) and material scalars. |
| Terrain map file (map group file 5) | Terrain for the modern renderer: heights, water fields and tile colours (equal to the reference build's). |
| Map files 6, 7 and 8 | Per-square environment trailer, lights, water flow and water type. |
| Water type configs (config group 76) | Normal-map, diffuse, foam and mask materials plus scalars. |
| Texture archives (4) | Compressed texture copies per id with dimensions, format, faces and mips. |
| Model archive (47) | The same models as the reference archive, in a newer layout. |

## Tests

`frame::tests` holds the end-to-end suite by subsystem. The GPU tests are
ignored by default; run them with
`cargo test -p rs910-render-modern -- --include-ignored --test-threads=2`.

- `scene`: the Lumbridge frame is finite, covered and repeatable; every setting
  reaches a finite repeatable frame; animated locs keep the loc cache bounded;
  the sorted depth passes on shared loc pages leave the frame unchanged; a
  threaded and a synchronous encode draw the identical frame.
- `lifecycle`: meshes and materials built ahead on the pool give the frame of
  building them at their draws; a renderer made on its startup thread draws the
  frames of one made in place.
- `ambient`: under the verified look every square of the window gets a captured
  block of its own, six faces at one a frame, that differs from the default and
  between squares; the shader shades each fragment with its square's block; two
  runs capture the same blocks and draw the same settled frame.
- `shadows`: penumbra widens with the filter and the umbra stays binary; hidden
  roofs shade interior floors; cascade culling leaves the frame unchanged; still
  scenes keep their maps; moving casters and a turning sun move their shadows.
- `point_shadows`: a synthetic night scene of several lights over occluders: the
  best lights per quality cast and the rest do not, slotted lights are not evicted
  and free their slot after leaving, faces land in their atlas rectangles at their
  level, frames repeat, the shadow fades by the maximum distance; Lumbridge at
  night keeps its faces and they equal a redraw.
- `post`: AO modes darken a crease, not open ground.
- `water`: the reflection is the mirror image; culling leaves the frame
  unchanged; the body fades to its sunlit opaque colour.
- `atmosphere`: haze grows with distance; volumetrics gather the unshadowed sun
  repeatably.
- `scale`: a 50% scale gives a finite quarter-size frame, nothing written
  outside the scissor, and a marker within a pixel of where 100% puts it.
  (`settings::tests` holds the automatic scale's rule.)
- `underwater`: with water detail high the seabed and its locs are drawn (their
  meshes cached, the frame differs from the one without them), and repeat frames
  build nothing.
- `sky`: the cube is sampled by view direction; no sky is the flat fog colour and a sky is
  fogged by elevation (the angle fog on sample directions), with the sun's glow lobe and the
  exposure offset; a settled cross-fade mixes cubes linearly over the default time or an
  override's (going back does not jump); frames repeat; Lumbridge's box bakes once into a cube
  with clouds; an attached decor is drawn where its direction projects, and only there.
  `atmosphere::sky_fade` tests the timeline on a fixed clock (default, override, zero
  duration, going back, waiting for a cube).
- `far`: far squares meet the near terrain at equal heights; streamed far scene
  converges to the synchronous frame; the batched near extension equals the
  per-loc frame with less than half its draws.
- `passes::tests`: aliased resources are never live together (CPU).
- CPU tests (need the cache): the draw list covers the reference lists, the
  modern models equal the reference models, the terrain covers the reference
  floor at the same heights.

The subsystem GPU suite bounds repeated-capture differences at one channel
value in 2,000 (`frame::tests::Noise`). Strict benchmark comparisons have a
separate byte-equality contract: unchanged-baseline repeats have also differed
in fixed-clock captures, so warming alone does not establish exactness. The
source of those differences remains under investigation.

On 4 October 2026 the owner deferred this investigation to prioritize Legacy
content. The preserved faithful-renderer control repeats the same scene,
camera and clock with equal submitted-input fingerprints: the earlier build
differs by up to three pixels and one channel step across sixteen captures;
the performance build differs by up to eight pixels and three channel steps.
CPU-written input fingerprints do not establish equality of GPU-produced
textures or backend execution. These comparisons remain failures of exact
byte equality; no golden, tolerance or renderer baseline was changed. Ordinary
gate passes do not cover the ignored full-pixel cases.

Timing and views are tools, not tests (`tools/perf/`, see its README):

- `modern_bench.sh build|run|check`: the headless matrix bench with the
  regression guard. Run it on every renderer change.
- `modern_bench.sh views` renders the offline views as raw RGBA for strict
  byte comparisons between two builds. Each scene is constructed after resetting
  the fixed clock; fresh renderers and sample changes capture at fixed frame IDs,
  after asserting that ambient captures, projection and fades have settled. A
  manifest records each frame's dimensions, renderer frame ID and simulation clock.
  `modern_views_diff.py` rejects missing, extra, malformed or unequal frames and
  differing capture metadata. An optional repeated baseline is checked with the
  same strict rule; it never supplies a tolerance. This capture setup repair does
  not change production rendering or resolve the preserved repeat differences.
  The strict comparator requires a completed canonical manifest on both sides;
  legacy raw captures remain diagnostic artifacts. `modern_bench.sh settings-views`
  renders raw frames per settings combination separately and does not produce the
  canonical manifest this comparator requires.
- `modern_bench.sh client` and `modern_online.sh` run the online session;
  `modern_hitch_sum.py` reports its hitches.
- `modern_bench.sh hitches` times the start, region changes and settings
  changes of one renderer (worst frames, pipelines compiled per frame;
  `MODERN_BENCH_COLD=1`: a cold Metal shader cache).

## Adding a feature

1. Put the CPU half (the data, the rules, their unit tests where the maths is
   the feature's own) in its subsystem module.
2. Put the WGSL in a snippet beside it. Add the snippet to the modules that need
   it in `shaders::Module::parts`, in order. Never patch text at run time. A
   genuine shader variant is a new `Module` field, part of the cache key.
3. Put the GPU half in `frame/gpu/<subsystem>.rs`:
   - Build its pipelines in `ModernRenderer::new`, through
     `frame::pipelines::Variants` when they depend on the sample count.
   - Declare each new pass in `frame::passes::FRAME` (stage, resolution,
     samples, reads, writes, resolve) and begin it with `begin_pass`.
4. If a player would choose it, make it a `ModernSettings` field. Default it to
   today's frame unless the change is proven, drive it from a client option
   where the option's meaning is established, and give it a `CLIENT910_MODERN_*`
   dev override in `ModernSettings::from_vars`. Don't add an `=off` switch for a
   layer.
5. Add or extend one end-to-end test in `frame::tests` that checks what the
   frame must show. Don't mirror the shader on the CPU or pin pixel values.
6. Verify:
   - frames before and after with `modern_bench.sh views` (the default frame
     must not change unless the feature is meant to change it);
   - `modern_bench.sh check` against the base;
   - `CLIENT910_MODERN_CHECK` 0 mismatches online;
   - `tools/refactor/gate.sh`.


## Owned frames and render-thread handoff

The main thread finishes the existing logic and redraw preparation phases in
order. It owns input, CS2, UI planning, roof and visibility state, classic CPU
posing, particle updates, picking, overlay heights and draw-cycle writes. It then
copies the read-only render inputs into one of two reusable owned frame slots.
Static models and immutable floor construction data are shared by stable resource
identity; changed models, actor matrices, draw selections, light fades, sky layers
and particle draws belong to that frame. A captured logic-clock sample travels
with the frame, so rendering an older frame cannot read a newer logic cycle.

A persistent render thread owns the modern renderer. It prepares the scene,
acquires the drawable after preparation, records the scene and post chain, submits
and presents. Main-thread UI preparation retains its CPU sprite/font owner caches;
its owned GPU drawing packets and buffers accompany the scene packet. The render
thread encodes the UI and console composition as well as the scene. Mutable UI buffers and framebuffer textures alternate with the two frame slots;
console plans keep immutable buffers until a changed plan creates new ones. Submission consumes the
packets in frame order; the main thread can prepare the following frame while the
previous frame renders. There is one pending render and one writable slot, with
backpressure instead of a growing queue or dropped frames.

Persistent minimap views belong to the renderer's registry. A new UI slot seeds
its external bindings from that registry once; base creation and release update
both existing slots. Frozen drawing packets retain their captured bind groups and
views, so releasing or replacing a base cannot alter an older packet's pixels.
Minimap operations wait for pending rendering before changing the registry.

Each installed model-light table owns an identity token retained by its snapshots
and installed GPU grid. Captures copy light colours, flicker, intensities and fade
state while keeping that installation identity; a new table gets a new identity.
Static light grids and point-shadow slots therefore survive alternating owned
frames without retaining stale mutable light values. Graphics preference sync
checks the GPU resource sample/bloom state before waiting for a render. Equal
settings still validate capabilities and initialize the modern forward sample
choice; actual target changes retain the ordered barrier. Window preference sync
uses the same authoritative canvas predicate as resource installation: logical
canvas, physical size at the current UI scale, and offscreen target presence for
the current surface and sample count. Equal canvas calls keep the pending frame;
physical or lifecycle changes wait before rebuilding depth and canvas resources.

The render thread serializes every modern resource write, prepare, encode and
submission. GPU commands retain the resources they name until execution completes;
a reused frame arena is written only by a later ordered submission. The shared
device uploader preserves ordered cache writes, including internal sky/probe
submissions. An acquire failure restores unsent producer state and drains only
uploads, as in the synchronous path. It publishes no completed canvas or screenshot.
A completed canvas retains the exact copy used by ordinary presents.

Loading and message boxes, resize and canvas changes, backend/toolkit transitions,
anti-aliasing changes, metrics, device recovery and shutdown are ordered barriers.
They wait for the pending frame and consume its completion before changing GPU
state or returning to the synchronous faithful path. Screenshot requests attach to
the intended owned frame; completion and failure reach the main thread in order.
No renderer thread mutates game state, and the faithful renderer keeps its current
uploads, pass order and pixel contract.

Caster eligibility and cascade masks use ordered chunks on the existing renderer
job pool. Mutable entity cache, texture and arena installation stays ordered on
the render thread. Independent instance records and transformed bounds are then
prepared on the pool and appended in input order, including their instance slots. Floating-point reductions and
transparent ordering retain their serial order. Synchronous worker mode uses the
same result assembly and is the determinism control.

Validation covers owned-frame isolation while logic changes the source, ordered
backpressure and shutdown, resize/backend barriers, skipped acquires with changing
inputs, screenshot attribution, staged uploads and nonblack exact pixels. Recorded
session replays cover renderer, present and profiler inertness. Quiet warm online
measurements compare the reviewed parent with this change, and a real-clock run
checks pacing without advancing the fixed clock. Default modern captures compare byte-for-byte with their controlled baseline.
