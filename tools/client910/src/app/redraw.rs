//! The redraw as named steps (code-quality programme Phase 4.4, Phase 5;
//! lane E-A1): `ViewerApp::render_frame` is the redraw, with the game and
//! scene draws and the interface pass, run on winit's `RedrawRequested` after the logic cycles
//! of `about_to_wait`. The step list, its order and its cadence are the
//! core's (`ClientCore::redraw`, rs910-client `client_core/redraw.rs`); this
//! module is the windowed client's [`RedrawShell`].
//!
//! Cadence: the first redraw after a frame with logic cycles is a **full
//! redraw** (every state-writing step, `ClientCore::full_redraw_state`, then
//! the frame is drawn and kept); every other redraw (more `about_to_wait`
//! turns than cycles under `ControlFlow::Poll`, resize/expose redraws) is a
//! **present**: R0-R2 as always, then the kept frame is drawn again
//! ([`RedrawFrame::retained`]; the scene's temporary entities stay installed
//! until the next logic cycle, [`ViewerApp::end_presentation`]).
//!
//! | step | shell method | client behaviour | writes |
//! |---|---|---|---|
//! | R0 loading | `render_loading_frame` | the loading screen draw | the loading screen (every redraw) |
//! | R1 keep-alive | [`ViewerApp::redraw_keepalive`] | (the first frame's shader/model preparation blocks the event loop) | io |
//! | R2 gate | `message_box` over `ClientCore::redraw_gate` | the message box in the rebuild states | draw |
//! | R3 environment | `ClientCore::environment_frame`, then `environment` (`build_env_frame`) | the partial environment update in the scene draw; the core's CPU half also runs at P9 | core environment; the skybox owner's fades |
//! | R5 2D scene overlays | `scene_overlays` | the 2D entity elements, cover markers, scene text (redraw); the minimap reads `ClientCore::minimap` (its flag is set at P9) | paint inputs; reads the previous full redraw's `min_y`/`draw_cycle` (UI paints before this frame's entities, §2 #3) |
//! | R6 particle tick | `tick_particles` | the particle tick in the game and title/lobby draws | `ParticleHost` |
//! | R7 interfaces | `paint_interfaces` | the interface pass, the message box in states 14/19, component particles | UI (stops the frame on a UI error) |
//! | R8 positioned sound | `ClientCore::positioned_sound_frame` | the positioned sound update in the game draw; the hooks it derives from retained state are packet-time logic (programme §8) | audio |
//! | R9 view | `update_view` | follow camera update on the redraw's elapsed time for camera states 1/2/4; cam2 (state 3) runs in the UI tick | `ViewCamera` |
//! | scene draw | `ClientCore::begin_draw_scene` | the scene cycle increment, the minimap flag's arrival | core |
//! | R10 actor placement | `place_actors` | entity placement from the scene draw, NPC draw priority and deferred scene adds | scene graph, NPC scene flags |
//! | R11 scene entities | `build_scene_entities` | entity models built as the scene draw draws them; the NPC draw writes `minY`/`height` and `drawCycle` | GPU caches; the animation/`min_y`/`draw_cycle` write-back |
//! | R12 player bodies | `refresh_player_bodies` | the picking of the drawn scene | renderer; the pick frame the next UI tick reads |
//! | R13 frame resources | `prepare_frame_resources` | material animation, the particle update, the skybox update | renderer, particles, skybox |
//! | R14 scene handoff | `present` | the scene draw's toolkit handoff (renderer plan A1), screenshots | draw (every redraw; a present draws the kept frame) |
//!
//! The zone loc changes apply at
//! P9 (`Game::apply_location_snapshots` in `update_session_logic`), and
//! the toolkit clock runs at P5 (`ViewerApp::session_cycle_phase`).
use super::*;

/// R5's 2D overlays and minimap frames, handed to the interface pass.
pub(super) struct SceneOverlays {
    entity_elements: crate::entity_elements::Elements,
    tile_hint_arrows: Vec<crate::entity_elements::Draw>,
    scene_icons: Vec<crate::ui_backend::SceneIcon>,
    cover_markers: Vec<crate::ui_runtime::CoverMarker>,
    scene_text: Vec<crate::ui_backend::SceneText>,
    minimap: Option<crate::minimap::Frame>,
    compass: Option<crate::minimap::CompassFrame>,
}

/// The redraw's hand-offs between its steps and the frame presents draw
/// again.
#[derive(Default)]
pub(super) struct RedrawFrame {
    /// R1's keep-alive, until the redraw ends.
    keepalive: Option<crate::loading_connection::LoadingConnection>,
    /// R3's frame environment, for the full redraw's later steps.
    env: Option<crate::env::EnvFrame>,
    /// R5's overlays for R7.
    overlays: Option<SceneOverlays>,
    /// The last full redraw's drawn frame, drawn again by presents until
    /// the next logic cycle ([`ViewerApp::end_presentation`]).
    retained: Option<Retained>,
}

/// What R14 drew a full redraw's frame with (besides the scene, meshes,
/// bodies and particles it left installed).
struct Retained {
    env: crate::env::EnvFrame,
    camera: OrbitCamera,
    blackout: bool,
    local: Option<usize>,
}

impl ViewerApp {
    /// CPU scene state is shared; only the selected scene renderer needs its meshes.
    pub(super) fn faithful_scene_required(&self) -> bool {
        self.renderer.as_ref().is_none_or(|renderer| {
            renderer.kind() != crate::active_toolkit::RendererKind::Modern
                || renderer.toolkit0()
                || rs910_render_modern::modern_debug_flags::flags().check
        })
    }

    /// The redraw (`ClientCore::redraw` over this shell); true
    /// for a full redraw, false for a present.
    pub(super) fn render_frame(&mut self) -> bool {
        let full = ClientCore::redraw(self) == RedrawKind::Full;
        if full {
            self.clean_caches();
        }
        full
    }

    /// Before a frame's logic cycles (`about_to_wait`): the last full
    /// redraw's temporary scene entities leave the scene, so the logic sees
    /// the scene without them as before, and presents stop until the next
    /// full redraw.
    pub(super) fn end_presentation(&mut self) {
        if let Some(scene) = self.scene.graph.as_mut() {
            scene.clear_temporary();
        }
        self.frame.retained = None;
    }

    /// R1: initial shader/model preparation can block the native event
    /// loop. As during a map build, no other writer runs until the returned
    /// keep-alive drops at the end of the redraw.
    fn redraw_keepalive(&self) -> Option<crate::loading_connection::LoadingConnection> {
        if self.core.scene_cycle == 0 || self.entities.players.is_none() {
            self.core
                .session
                .as_ref()
                .filter(|s| !s.polling_dead && s.io.world.pending_writes.is_empty())
                .and_then(|s| s.io.world.stream.as_ref())
                .and_then(|stream| crate::loading_connection::LoadingConnection::start(stream).ok())
        } else {
            None
        }
    }

    /// R5: the 2D scene elements (draw2DEntityElements), cover markers and
    /// scene text, the cross sprites, then the minimap frames.
    fn collect_scene_overlays(&mut self, env_frame: &crate::env::EnvFrame) -> SceneOverlays {
        let (entity_elements, tile_hint_arrows) = self.scene_entity_elements();
        let (scene_icons, cover_markers) = self.scene_cover_marker_overlays();
        let scene_text = self.scene_text_overlays();
        // The walk click's minimap flag and cross state are the core's
        // (`ClientCore::take_minimap_flag` at P9, `ClientCore::cross_state`).
        if let Some(ui) = self.core.session.ui_mut() {
            if ui.target.cross_sprites.is_empty() {
                if let (Some(pack), Some(d)) =
                    (self.minimap.pack.as_ref(), self.minimap.defaults.as_ref())
                {
                    // The default cross sprites.
                    ui.target.cross_sprites = crate::minimap::sprite_frames(pack, d.cross)
                        .unwrap_or_else(|error| {
                            crate::logging::warn_repeated!(
                                "[client910] cross sprites unavailable: {error:#}"
                            );
                            Vec::new()
                        });
                }
            }
        }
        let (minimap, compass) = self.minimap_frames(env_frame);
        SceneOverlays {
            entity_elements,
            tile_hint_arrows,
            scene_icons,
            cover_markers,
            scene_text,
            minimap,
            compass,
        }
    }

    /// R7: the interface pass over this frame's overlays; false when a UI
    /// error stops the frame.
    fn paint_interface_pass(
        &mut self,
        env_frame: &crate::env::EnvFrame,
        overlays: SceneOverlays,
    ) -> bool {
        // The scene's black fill (-16777216) replaces the
        // fog-colour clear while the scene is blacked out.
        let clear = if self.drawscene_black() {
            [0.0; 3]
        } else {
            env_frame.clear
        };
        let (Some(session), Some(renderer)) = (&mut self.core.session, &mut self.renderer) else {
            return true;
        };
        let particles = self.particles.runtime.as_mut();
        let ui = &mut session.ui;
        ui.engine.scene.cover_markers = overlays.cover_markers;
        ui.target.scene_icons = overlays.scene_icons;
        ui.target.entity_elements = overlays.entity_elements;
        ui.target.tile_hint_arrows = overlays.tile_hint_arrows;
        ui.target.scene_text = overlays.scene_text;
        ui.target.minimap = overlays.minimap;
        ui.target.compass = overlays.compass;
        ui.target.world_map = Some(ui.engine.world_map.clone());
        // The full environment update feeds the effect state, then the
        // toolkit decides whether the layer is captured. CLIENT910_POSTFX_OFF forces the capture off (A/B
        // diagnostics only).
        renderer.update_postprocess_environment(env_frame, &mut self.environment.colour_remappers);
        ui.target.postprocess_enabled =
            renderer.postprocess_capture() && !crate::debug_flags::flags().postfx_off;
        let ready = session
            .game
            .as_ref()
            .is_some_and(|g| g.runtime.feed.state.initialized);
        match ui.paint(self.core.cycle, ready, clear) {
            Ok(mut output) => {
                // States 14/19 draw the message box instead of the game; the
                // retained frame stands in for the undrawn canvas.
                if let Some(text) = loading_host::message_box_text(session.machine.state) {
                    let mut painter = crate::ui_paint::Painter::new(output.paint.size);
                    let (w, h) = renderer.canvas_size();
                    if let Some(fonts) = ui.state.fonts.as_ref() {
                        if let Err(error) = crate::message_box::draw(
                            &mut painter,
                            fonts,
                            &ui.engine.builtins.message_box,
                            &text,
                            [w as i32, h as i32],
                            self.lifecycle.client_frame,
                        ) {
                            crate::logging::warn_repeated!("[client910] message box: {error:#}");
                        }
                    }
                    // The op recording (the null backend's digest) carries
                    // the same calls.
                    output.recording.ops.append(&mut painter.recording.ops);
                    output.paint.quads.extend(painter.finish().quads);
                }
                // Component particles bind after the interface pass.
                if let Some(runtime) = particles {
                    crate::ui_models::bind_particles(
                        &mut output.models,
                        runtime,
                        i64::from(self.core.cycle),
                    );
                }
                if let Err(error) = renderer.prepare_ui(output) {
                    crate::logging::warn_repeated!("[client910] UI upload: {error:#}");
                    return false;
                }
                true
            }
            Err(error) => {
                crate::logging::warn_repeated!("[client910] UI frame stopped: {error:#}");
                false
            }
        }
    }

    /// R9: the lens profile, scene viewport and console layer, then the
    /// follow camera on the elapsed logic-clock time since the last full
    /// redraw.
    fn update_view_state(&mut self) {
        // The viewport_set* scripts update the client's lens profile during the
        // UI tick. Install it on the live camera before scene projection,
        // picking, and environment fog consume the next frame.
        if let Some(ui) = self.core.session.ui() {
            self.view.camera.viewport_profile = ui.state.viewport_profile;
            self.view.camera.scene_viewport = ui.state.viewport.map(|(rect, _)| rect);
        }
        if let Some(renderer) = self.renderer.as_ref() {
            self.view.camera.viewport = renderer.scene_size();
        }
        if let Some(renderer) = &mut self.renderer {
            if let Err(error) = renderer.prepare_console(
                &self.console.developer,
                self.core.cycle,
                self.input.focused,
            ) {
                crate::logging::warn_repeated!("[client910] console draw: {error:#}");
            }
        }
        let camera_now = self.clock.logic.sample() / 1_000_000;
        let camera_elapsed = self
            .view
            .last_redraw
            .map_or(0, |last| camera_now.wrapping_sub(last));
        self.view.last_redraw = Some(camera_now);
        if let Err(error) = self.update_follow_camera(camera_elapsed) {
            crate::logging::warn_repeated!("[client910] follow camera: {error:#}");
        }
    }

    /// R10: entity placement for players, the NPC scene flags the 2D entity
    /// elements read, the live scene's local player and the current player
    /// level.
    fn place_scene_actors(&mut self) {
        let (active_target, draw_order) = self.core.session.as_ref().map_or((0, 0), |s| {
            (
                s.ui.engine.scene.active_target,
                s.ui.engine.scene.draw_order,
            )
        });
        let npc_store_for_flags = self
            .core
            .session
            .ui()
            .and_then(|ui| ui.engine.configs.npcs.clone());
        let hint_npcs: Vec<usize> = self
            .minimap
            .hint_arrow_state
            .iter()
            .flatten()
            .filter(|a| a.hint_type == 1)
            .filter_map(|a| a.npc_index)
            .collect();
        let (Some(game), Some(scene), Some(live)) = (
            self.core.session.game_mut(),
            self.scene.graph.as_mut(),
            self.scene.live.as_mut(),
        ) else {
            return;
        };
        if game.game.runtime.map_request.is_some() || !game.game.runtime.feed.state.initialized {
            return;
        }
        let c = crate::player_scene::Settings {
            local: game.game.runtime.map.local,
            idle_detail: game
                .ui_variables
                .queries
                .preferences
                .options
                .live()
                .idle_animation_detail,
            active_target,
            draw_order,
            cutscene: game.game.cutscene.scene_state == 0,
            ..Default::default()
        };
        if let Err(error) = crate::player_scene::insert(
            &mut game.game.runtime.feed.state.players,
            scene,
            game.game.runtime.terrain.as_ref(),
            game.game.cycle,
            &c,
        ) {
            crate::logging::warn_repeated!("[client910] player insertion: {error:#}");
        }
        // NPC draw priority and deferred scene adds, read by the 2D entity
        // elements.
        if let Some(store) = npc_store_for_flags.as_deref() {
            let varps = &game.game.runtime.feed.state.varps;
            let bits = &game.game.inputs.bits;
            let read = |bit: bool, id: i32| -> Option<i32> {
                let varps = varps.as_ref()?;
                if bit {
                    varps.get_bit(&bits.get(id, false).ok()?).ok()
                } else {
                    varps.get(id).ok()
                }
            };
            let types = |n: &crate::entities910::Npc| {
                let t = store.get(u32::try_from(n.type_id).ok()?)?;
                (t.multinpc.is_empty() || t.multi_npc(&read).is_some()).then_some(
                    crate::entity_elements::NpcSceneType {
                        follower: t.follower,
                        drawabove: t.drawabove,
                        drawbelow: t.drawbelow,
                    },
                )
            };
            let size = [
                game.game.runtime.map.width as usize,
                game.game.runtime.map.height as usize,
            ];
            crate::entity_elements::npc_scene_flags(
                &game.game.runtime.feed.state.players,
                &mut game.game.runtime.feed.state.npcs,
                &types,
                size,
                draw_order,
                active_target,
                &hint_npcs,
            );
        }
        live.local_player = game.game.runtime.feed.state.players.players[c.local]
            .as_ref()
            .map(|p| [p.fine_x, p.fine_z]);
        self.scene.focus_level = game.game.runtime.feed.state.players.current_level as u8;
    }

    /// R11: the transient entities (NPC, player and effect models, whose
    /// animation state is written back into the game), the temporary
    /// install, loc replacements and their meshes, the roof/draw plan and
    /// floor selection, loc animations and the dynamic scenery.
    fn build_entities(&mut self, env_frame: &crate::env::EnvFrame) {
        let scene_black = self.drawscene_black();
        if self.faithful_scene_required()
            && self.renderer.is_some()
            && self
                .scene
                .floors
                .iter()
                .zip(&self.scene.meshes.floor_meshes)
                .any(|(floor, mesh)| floor.is_some() && mesh.is_none())
        {
            self.upload_floors();
        }
        if let Err(error) = self.add_transient_entities() {
            crate::logging::warn_repeated!("[client910] transient scene insertion: {error:#}");
        }
        if let (Some(scene), Some(live)) = (self.scene.graph.as_ref(), self.scene.live.as_mut()) {
            live.install_temporary(scene);
        }
        if let Err(error) = self.apply_pending_location_models(&env_frame.sun) {
            crate::logging::warn_repeated!(
                "[client910] persistent location replacement: {error:#}"
            );
        }
        if self.faithful_scene_required() {
            if let Err(error) = self.upload_transient_meshes() {
                crate::logging::warn_repeated!("[client910] transient mesh upload: {error:#}");
            }
        }
        let faithful = self.faithful_scene_required();
        if let (Some(live), Some(scene), Some(renderer)) = (
            self.scene.live.as_mut(),
            self.scene.graph.as_ref(),
            self.renderer.as_ref(),
        ) {
            let mut server_roof = [-1, -1];
            if let Some(session) = self.core.session.as_ref() {
                if let Some(game) = session.game.as_ref() {
                    live.roof_mode = game
                        .ui_variables
                        .queries
                        .preferences
                        .options
                        .live()
                        .roof_mode;
                    server_roof = session.ui.engine.scene.server_roof;
                }
            }
            // A blacked-out drawScene returns before the roof/draw passes:
            // nothing is dispatched this frame.
            if scene_black {
                let floors = std::mem::take(&mut live.draw.plan.floors);
                live.draw.plan = Default::default();
                live.draw.plan.floors = floors;
            } else {
                live.update(
                    scene,
                    self.view.camera.scene_camera(renderer.scene_size()),
                    self.scene.floor_base,
                    self.scene.focus_level as usize,
                    server_roof,
                );
            }
            if faithful {
                self.scene.meshes.select_floors(
                    renderer.faithful_ref(),
                    &self.scene.floors,
                    &live.draw.plan.floors,
                );
            }
        }
        self.apply_pending_loc_animations();
        if let Err(error) = self.refresh_dynamic_scene(&env_frame.sun) {
            crate::logging::warn_repeated!("[client910] dynamic scenery: {error:#}");
        }
    }

    /// R12: the player and NPC bodies and the pick frame the next UI tick
    /// reads (`ui.engine.scene.player_picks`).
    fn refresh_bodies(&mut self) {
        let blacked_out = self.view.camera_unready || self.drawscene_black();
        let faithful_uploads = self.faithful_scene_required();
        let (Some(session), Some(scene), Some(live), Some(renderer), Some(materials), Some(_)) = (
            self.core.session.as_mut(),
            self.scene.graph.as_mut(),
            self.scene.live.as_mut(),
            self.renderer.as_mut(),
            self.scene.material_store.as_ref(),
            self.scene.pack_root.as_ref(),
        ) else {
            return;
        };
        let (Some(game), ui) = (session.game.as_mut(), &mut session.ui) else {
            return;
        };
        ui.engine.scene.player_picks = None;
        if self.entities.players.is_none() {
            match crate::player_renderer::PlayersRenderer::new(&self.pack) {
                Ok(r) => self.entities.players = Some(r),
                Err(error) => {
                    crate::logging::warn_repeated!("[client910] player resources: {error:#}")
                }
            }
        }
        let (Some(players), Some((viewport, _))) =
            (self.entities.players.as_mut(), ui.state.viewport)
        else {
            return;
        };
        players.set_model_detail(
            crate::rebuild::BuildPrefs::from_options(
                &game.ui_variables.queries.preferences.options,
            )
            .model_detail(),
        );
        let mut frame = crate::player_picking::Frame::new(
            &self.view.camera.scene_camera(renderer.scene_size()),
            [game.runtime.map.base_x << 9, game.runtime.map.base_z << 9],
            game.runtime.terrain_generation,
            viewport,
        );
        match players.refresh(
            renderer.faithful(),
            rs910_render_gpu::player_renderer::PlayerRefresh {
                game: &mut game.game,
                faithful_uploads,
                options: &game.ui_variables.queries.preferences.options,
                live,
                scene,
                materials,
                scene_cycle: self.core.scene_cycle,
                pick_frame: &mut frame,
                hint_arrows: &self.minimap.hint_arrow_state,
                npc_store: ui.engine.configs.npcs.as_deref(),
            },
        ) {
            Ok(()) => {
                append_npc_picks(scene, &self.entities.npc_picks, &mut frame);
                let occupants =
                    slot_occupants(&self.scene.loc_changes, self.scene.loc_slots.as_ref());
                frame.collect_scene(scene, live, ui.engine.configs.locs.as_deref(), &occupants);
                let (locs, objs) =
                    ui.active_scene_keys([game.runtime.map.base_x, game.runtime.map.base_z]);
                frame.collect_active_heights(scene, &locs, &objs);
                // The ground pick reads the matrices of the last drawn frame,
                // whichever camera produced it; a blacked-out frame keeps the
                // previous ones.
                if !blacked_out {
                    ui.engine.scene.drawn_view =
                        Some(crate::ui_scene_options::DrawnView::of_frame(&frame));
                }
                ui.engine.scene.player_picks = Some(frame)
            }
            Err(error) => crate::logging::warn_repeated!("[client910] player bodies: {error:#}"),
        }
    }

    /// R14 of a full redraw: the headless screenshot requests, then
    /// the scene draw's toolkit handoff ([`ViewerApp::frame_scene`]),
    /// and the frame is kept for the presents until the next logic cycle.
    fn draw_scene_frame(&mut self, env_frame: &crate::env::EnvFrame) {
        let software_blackout = self.drawscene_black();
        let software_local = self.core.session.game().map(|g| g.runtime.map.local);
        let (Some(renderer), Some(_)) = (self.renderer.as_mut(), self.window.as_ref()) else {
            return;
        };
        // Diagnostic PNG sequence (`CLIENT910_SCREENSHOT_SERIES=c1,c2,..`):
        // one `<stem>_<cycle>.png` beside `--screenshot` per listed logic cycle.
        if let (Some(cycles), Some((path, _))) = (screenshot_series(), &self.diag.screenshot) {
            let next = self.diag.screenshot_series_next;
            if let Some(&cycle) = cycles.get(next).filter(|&&c| self.core.cycle >= c) {
                let stem = path
                    .file_stem()
                    .map_or("frame".into(), |s| s.to_string_lossy().into_owned());
                let shot = path.with_file_name(format!("{stem}_{cycle:05}.png"));
                log::info!(
                    "[client910] screenshot series {} at logic cycle {}",
                    shot.display(),
                    self.core.cycle
                );
                renderer.request_screenshot(shot);
                self.diag.screenshot_series_next = next + 1;
                if let Some(ui) = self.core.session.ui() {
                    let d = &ui.diagnostics;
                    log::info!(
                        "[client910] ui diagnostics: hooks {} failures {} unsupported {:?} errors {:?}",
                        d.hooks, d.failures, ui.engine.unsupported, d.errors
                    );
                }
            }
        } else if let Some((path, frame)) = &self.diag.screenshot {
            let due = crate::debug_flags::flags()
                .screenshot_cycle
                .map_or(self.core.scene_cycle as u32 == *frame, |cycle| {
                    self.core.cycle >= cycle
                });
            if due {
                renderer.request_screenshot(path.clone());
                if let Some(ui) = self.core.session.ui() {
                    let d = &ui.diagnostics;
                    log::info!(
                        "[client910] ui diagnostics: hooks {} failures {} unsupported {:?} errors {:?}",
                        d.hooks, d.failures, ui.engine.unsupported, d.errors
                    );
                }
            }
        }
        let camera = self.view.camera.clone();
        self.frame_scene(
            RedrawKind::Full,
            env_frame,
            &camera,
            software_blackout,
            software_local,
        );
        self.frame.retained = Some(Retained {
            env: *env_frame,
            camera,
            blackout: software_blackout,
            local: software_local,
        });
    }

    /// R14 of a present: the last full redraw's frame drawn again (its
    /// environment, camera and installed scene); nothing when no frame is
    /// kept (a logic cycle ran since, or the full redraw stopped early).
    fn present_retained(&mut self) {
        let Some(Retained {
            env,
            camera,
            blackout,
            local,
        }) = self.frame.retained.take()
        else {
            return;
        };
        if self.renderer.is_some() && self.window.is_some() {
            self.frame_scene(RedrawKind::Present, &env, &camera, blackout, local);
        }
        self.frame.retained = Some(Retained {
            env,
            camera,
            blackout,
            local,
        });
    }

    /// The scene draw's toolkit handoff: the renderer-neutral scene
    /// (renderer plan A1) from the installed scene, meshes, bodies and
    /// particles, drawn by the active backend with `env_frame` and `camera`
    /// (a present: `ActiveToolkit::present_scene`). Reads only.
    fn frame_scene(
        &mut self,
        kind: RedrawKind,
        env_frame: &crate::env::EnvFrame,
        camera: &OrbitCamera,
        software_blackout: bool,
        software_local: Option<usize>,
    ) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        renderer.set_frame_identity(u64::from(self.clock.frames), self.core.cycle);
        let snapshot = crate::scene_snapshot::SceneSnapshot {
            owned: None,
            time_ms: None,
            camera: camera.scene_camera(renderer.scene_size()),
            env: env_frame,
            live: self.scene.live.as_ref(),
            scene: self.scene.graph.as_ref(),
            floors: &self.scene.floors,
            lights: &self.scene.lights,
            players: self.entities.players.as_ref().map(|p| p as _),
            floor_base: [self.scene.floor_base.0, self.scene.floor_base.1],
            materials: self.scene.material_store.as_ref(),
            pack: self.scene.pack_root.as_ref().map(|_| &self.pack),
            blackout: software_blackout,
            local_player: software_local,
            particles: self.particles.runtime.as_ref(),
            underwater: self.scene.underwater_floor.as_ref().map(|floor| {
                crate::scene_snapshot::Underwater {
                    floor,
                    models: &self.scene.underwater_models,
                }
            }),
            sky: self
                .environment
                .sky
                .as_ref()
                .and_then(|sky| sky.cache.frame()),
        };
        // The NXT backend's sun shadows read the shadow options (renderer
        // plan M3; the defaults until a session has options).
        let live = self
            .core
            .session
            .as_ref()
            .and_then(|s| s.game.as_ref())
            .map(|g| g.ui_variables.queries.preferences.options.live())
            .unwrap_or_default();
        renderer.set_modern_shadows(
            live.scenery_shadows,
            live.shadow_quality,
            i32::from(live.character_shadows),
        );
        let players = self.entities.players.as_ref();
        let result = match kind {
            RedrawKind::Full => {
                renderer.frame_scene(&snapshot, &self.scene.meshes, players, camera)
            }
            RedrawKind::Present => {
                renderer.present_scene(&snapshot, &self.scene.meshes, players, camera)
            }
        };
        if let Err(err) = result {
            crate::logging::warn_repeated!("[client910] frame failed: {err:#}");
        }
        self.clock.frames += 1;
    }
}

impl RedrawShell for ViewerApp {
    fn core(&mut self) -> &mut ClientCore {
        &mut self.core
    }
    /// R0: the loading screen draw in the loading states.
    fn loading_frame(&mut self) -> bool {
        if self.lifecycle.loading.is_none() {
            return false;
        }
        rs910_core::profile::scope!("R0 loading", self.render_loading_frame());
        true
    }
    fn keepalive(&mut self) {
        self.frame.keepalive =
            rs910_core::profile::scope!("R1 keep-alive", self.redraw_keepalive());
    }
    /// R2: draws the message box loading text (`crate::message_box`) over
    /// the retained frame, not the game, in the rebuild states.
    fn message_box(&mut self, progress: crate::login_state::RebuildProgress) {
        let text = loading_host::rebuild_message_text(&progress);
        self.present_message_box(&text);
    }
    fn camera_tile(&mut self) -> [i32; 2] {
        self.camera_window_tile()
    }
    fn environment(&mut self, current: (crate::env::Environment, [f32; 3])) {
        self.frame.env = Some(self.build_env_frame(current));
    }
    fn scene_overlays(&mut self) {
        let Some(env_frame) = self.frame.env else {
            return;
        };
        self.frame.overlays = Some(self.collect_scene_overlays(&env_frame));
    }
    fn tick_particles(&mut self) {
        ViewerApp::tick_particles(self);
    }
    fn paint_interfaces(&mut self) -> bool {
        let (Some(env_frame), Some(overlays)) = (self.frame.env, self.frame.overlays.take()) else {
            return false;
        };
        self.paint_interface_pass(&env_frame, overlays)
    }
    fn update_view(&mut self) {
        self.update_view_state();
    }
    fn place_actors(&mut self) {
        self.place_scene_actors();
    }
    fn build_scene_entities(&mut self) {
        if let Some(env_frame) = self.frame.env {
            self.build_entities(&env_frame);
        }
    }
    fn refresh_player_bodies(&mut self) {
        self.refresh_bodies();
    }
    fn prepare_frame_resources(&mut self) {
        if let Some(env_frame) = self.frame.env {
            rs910_core::profile::scope!("R13 materials", self.prepare_frame_materials(&env_frame));
            rs910_core::profile::scope!("R13 particles", self.update_particles());
            rs910_core::profile::scope!("R13 skybox", self.prepare_skybox(&env_frame));
        }
    }
    fn present(&mut self, kind: RedrawKind) {
        match kind {
            RedrawKind::Full => {
                if let Some(env_frame) = self.frame.env {
                    rs910_core::profile::scope!(
                        "R14 scene handoff",
                        self.draw_scene_frame(&env_frame)
                    );
                }
            }
            RedrawKind::Present => self.present_retained(),
        }
    }
    fn end_frame(&mut self) {
        self.frame.keepalive = None;
        self.frame.env = None;
        self.frame.overlays = None;
    }
}
