//! The renderer-neutral scene snapshot (`docs/renderer/modern-renderer.md`):
//! what the 3D scene
//! contains in one frame, as CPU data borrowed from the scene, model and
//! game owners, for any backend to draw.
//!
//! **Build phase.** The shell builds one [`SceneSnapshot`] per redraw at
//! the toolkit handoff, after this frame's CPU passes: `LiveScene::update`
//! (the scene draw's roof, occlusion and draw-list pass), the dynamic loc refresh, the player/NPC model
//! refresh, the particle update and the skybox resolution. It borrows its
//! owners immutably, so no backend can change game or scene state while it
//! draws (the renderer-inertness invariant, plan §4(b) item 4).
//!
//! **Contents.** Camera and projection (`camera`; `SceneCamera::projection`
//! and `view_entries`), fog and environment (`env`: sun, fog planes, bloom,
//! levels, colour remapping), floors with their water and hard-shadow masks
//! per level (`floors`), static lights (`lights`) and the live model lights
//! (`live.model_lights`), static locs, walls, decors and dynamic locs (the
//! scene graph `scene` with `live.dynamic`), transient entities and NPC
//! bodies (`scene.temporary`), players with their animation frames, spot
//! shadows and hint arrows (`players`), particles (`particles`), the
//! underwater scene (`underwater`), the skybox (`sky`), and the frame's draw
//! lists (`live.draw.plan`: the opaque/transparent entity order after
//! occlusion and roofs).
//!
//! **Keys.** [`SceneSnapshot::entity_key`]: a backend caches its per-entity
//! resources under the entity's `live.entities` slot (stable for one
//! installed scene), its scene-graph source and its dynamic model revision.
//! The faithful GPU backend's cache (`rs910_render_gpu::scene_meshes`)
//! keeps the legacy upload points (scene install, loc changes, the dynamic
//! refresh) so its buffers are created in the legacy order; the modern
//! renderer reads the snapshot at draw time.
//!
//! The skybox joined it in lane Q-PREM1 (`sky`, [`crate::sky_frame::SkyFrame`],
//! resolved by the shell's `SkyCache`); the interface models are the UI's
//! `FramePlan` segments ([`crate::interface_model::Draw`]), below.
//!
//! # The backend contract
//!
//! What a 3D backend (the faithful GPU toolkit, the null backend,
//! `rs910-render-modern`) receives, in what order, what it must
//! answer and what it may keep. The shell (`client910::active_toolkit::
//! ActiveToolkit`) owns the backends and calls them; a new backend is a
//! `RendererKind` (`--renderer`, never the saved preferences) and a
//! `Backend` arm there.
//!
//! **Inputs.**
//!
//! 1. *The device* (programme Phase 4.1). The shell owns the one
//!    `rs910_gpu_device::gpu_device::Device` (instance, adapter, device,
//!    queue, the window surface and its configuration, screenshot
//!    readback) and lends it for each call: `&Device` to create or write
//!    resources, `&mut Device` to acquire, present or read back a frame. A
//!    backend never stores it.
//! 2. *The interface frame* (`ActiveToolkit::prepare_ui`): the retained
//!    UI's `rs910_toolkit::ui_output::Output<interface_model::Draw>`, i.e.
//!    a `rs910_toolkit::FramePlan`: the 2D calls in painter order
//!    (`ui_paint::Op`, see rs910-toolkit's extension points), the scene
//!    viewport segment and the interface-model segments. An interface
//!    model is renderer-neutral: the lit `GpuModel`, [`crate::interface_model::DrawSpace`]
//!    (model matrix, projection, the interface lighting) and its particles;
//!    each backend derives its own pipeline state from it (renderer plan
//!    A4; the faithful GPU toolkit's `ui_model_gpu::interface_uniforms`).
//! 3. *The scene frame* (`ActiveToolkit::frame_scene`): one
//!    [`SceneSnapshot`] per redraw, borrowed immutably for the call. It
//!    holds everything a backend needs to draw the same scene with its own
//!    lighting: camera and projection; environment (sun direction and
//!    colours, fog planes and range, bloom/levels/colour remapping);
//!    floors per level (heights, vertex colours, material batches, water
//!    fog, hard-shadow masks; normals from
//!    `floor::FloorGeometry::normal_grid`/`normal_at` whether or not the
//!    faithful build kept them); static lights per level and the live
//!    model lights (flicker intensities); static and dynamic locs with
//!    their transforms (the scene graph, `live.dynamic` with its model
//!    revisions; models are posed on the CPU, so the animation frame is in
//!    the model); NPC bodies, spot anims and projectiles
//!    (`scene.temporary`); players (bodies in `scene.temporary`, matrices,
//!    spot shadows and hint arrows through [`crate::player_draw::PlayerDraws`]);
//!    particles; the underwater scene; the skybox; the frame's draw lists
//!    (`live.draw.plan`: the opaque/transparent order after occlusion
//!    and roofs, the per-level floor tile selections); and the cache
//!    (`pack`, `materials`) for textures. Model colours are `GpuModel`
//!    streams: `colour_stream` bakes the model's ambient (the faithful
//!    path), `albedo_stream` does not.
//!
//! **Order of the calls in one redraw** (`ViewerApp::render_frame_inner`,
//! the order the client's redraw requires): `update_postprocess_environment`
//! (the frame's environment), `prepare_ui` (the interface frame; the
//! minimap base, when rebuilt, is drawn before it through
//! `render_minimap_base`), `prepare_console`, then the frame's CPU scene
//! passes, whose faithful GPU uploads run at the legacy upload points through
//! `ActiveToolkit::faithful` (the draw lists and floor tile selections, the
//! dynamic locs, the player bodies, the material walk, the particles, the
//! skybox), then `frame_scene` with the snapshot, which draws and presents.
//! Other frames: `frame_loading` (a loading screen without a scene) and
//! `frame_message_box` (a rebuild state's message box over the retained
//! last frame). Surface changes arrive as `resize`/`set_game_canvas`,
//! toolkit changes as `set_scene_effects`/`recreate_toolkit_targets`.
//!
//! **Answers.** Every backend answers the capability queries with the
//! faithful GPU toolkit's profile (`rs910_toolkit::capability::Profile`,
//! `RendererKind::capability_profile`), and the faithful toolkit keeps
//! running its uploads (mesh builds, the skybox's `createModel`) whatever
//! draws, so a failure the faithful toolkit would report (a sky model that
//! cannot be created) is the faithful one. A backend must be
//! observationally inert: packets, per-tick state, `ClientOptions` and CS2
//! hook order must not depend on it (the replay gate's
//! `renderer_choice_is_observationally_inert`). It draws nothing outside
//! the frame calls and never changes game or scene state.
//!
//! **Caching.** A backend may keep any resources of its own across frames:
//! per-entity resources under [`SceneSnapshot::entity_key`] (slot, source,
//! dynamic model revision; valid while one scene is installed), floors per
//! level until the scene is reinstalled, textures and materials by id, sky
//! models by `SkyboxKey`, interface models by their owner
//! (`interface_model::ModelOwner` identity) and 2D resources by `Rc`
//! identity. It must not keep borrows of the snapshot, the draws or the
//! device past the call.

/// The scene state the client hands the toolkit this frame (see the
/// module docs; `sw_toolkit::model`'s scene input before renderer plan A1).
pub struct SceneSnapshot<'a> {
    /// Owned read-only projection used by asynchronous scene frames.
    pub owned: Option<&'a owned::RenderData>,
    /// The clock sample captured at the main-thread handoff.
    pub time_ms: Option<i64>,
    pub camera: crate::camera::SceneCamera,
    pub env: &'a crate::env::EnvFrame,
    pub live: Option<&'a crate::live_scene::LiveScene>,
    pub scene: Option<&'a crate::scene::Scene>,
    pub floors: &'a [Option<crate::floor::FloorGeometry>],
    /// The static lights per level (baked from `StaticLight`s; the
    /// faithful backends bake them into floor passes at scene install).
    pub lights: &'a [Vec<crate::floorlight::BakedLight>],
    pub players: Option<&'a dyn crate::player_draw::PlayerDraws>,
    pub floor_base: [i32; 2],
    pub materials: Option<&'a crate::texture::MaterialStore>,
    /// The client's shared cache reader (`None`: no pack-backed scene).
    pub pack: Option<&'a crate::cache::Pack>,
    pub blackout: bool,
    /// The local player's index.
    pub local_player: Option<usize>,
    /// The scene particle systems' draw lists.
    pub particles: Option<&'a crate::particle::Runtime>,
    /// The underwater tiles and level-0 underwater height map (water detail 2
    /// with an underwater map square).
    pub underwater: Option<Underwater<'a>>,
    /// This frame's skybox (`None`: the scene clears to the fog colour).
    pub sky: Option<crate::sky_frame::SkyFrame<'a>>,
}

/// The underwater scene the scene draw walks first.
pub struct Underwater<'a> {
    pub floor: &'a crate::floor::FloorGeometry,
    pub models: &'a [crate::rebuild::UnderwaterModel],
}

/// The key a backend caches one scene entity's resources under: its
/// `live.entities` slot (the draw lists' id, stable while one scene is
/// installed), its scene-graph source, the loc changes the slot has seen
/// (`LiveScene::slot_changes`: a loc change replaces the slot's model in
/// place) and, for a dynamic loc, its model revision
/// (`DynamicScene::revision`, bumped when the model changes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntityKey {
    pub id: usize,
    pub source: crate::scene::EntityRef,
    pub changes: u32,
    pub revision: Option<u64>,
}

pub mod owned;

impl<'a> SceneSnapshot<'a> {
    pub fn live_frame(&self) -> Option<owned::LiveFrame<'a>> {
        self.owned
            .and_then(|data| data.live.as_ref())
            .map(owned::LiveFrame::owned)
            .or_else(|| self.live.map(owned::LiveFrame::borrowed))
    }
    pub fn model(&self, source: crate::scene::EntityRef) -> Option<&'a crate::gpumodel::GpuModel> {
        if let Some(owned) = self.owned {
            owned.models.get(&source).map(std::sync::Arc::as_ref)
        } else {
            crate::dynamic_scene::model(self.scene?, source)
        }
    }

    /// [`EntityKey`] of draw-list entity `id` (`None` without a live scene or
    /// for an id outside it).
    pub fn entity_key(&self, id: usize) -> Option<EntityKey> {
        self.live_frame()?.entity_key(id)
    }
}
