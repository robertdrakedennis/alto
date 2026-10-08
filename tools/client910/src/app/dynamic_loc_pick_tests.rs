//! Locs the server adds, changes and removes while the client runs.
//!
//! A zone loc packet reaches the scene through the redraw (the transient
//! entities, the static-slot swap, the draw plan) and the menu reads the pick
//! list the redraw collected, re-tested at the mouse by every logic cycle. The
//! logic cycles run after the last redraw's temporary entities have left the
//! scene, so what a pick needs from its drawn model has to outlive them. The
//! tests drive the real zone packets (a loopback socket, as the session replay
//! does) and the app's own redraw passes without a window, then read the pick
//! list the way the UI tick does.
use super::session_replay::{server_frame, Replay, Trace, FIXTURE};
use super::*;
use crate::player_picking::Frame;
use crate::scene::{EntityRef, Scene};

/// The scene viewport the picks are projected into.
const VIEWPORT: [i32; 4] = [4, 4, 512, 334];
/// An iron rock the dev server adds with `locadd`: a centrepiece with a
/// `Mine` op and no clickbox, so it picks through its model.
const ROCK: i32 = 113_158;
/// A tree stump: its only op is `Examine`.
const STUMP: i32 = 40_350;
/// The shape of a centrepiece (scenery layer).
const CENTREPIECE: u8 = 10;

/// A headless client in the recorded Lumbridge session.
struct World {
    app: ViewerApp,
    peer: std::net::TcpStream,
    base: [i32; 2],
    _keep: Box<dyn std::any::Any>,
}

/// The `UPDATE_ZONE_PARTIAL_FOLLOWS` frame that makes the zone of scene tile
/// `(x, z)` the target of the loc frames after it.
fn zone_frame(level: u8, tile: [i32; 2]) -> Vec<u8> {
    use crate::proto::server as sp;
    server_frame(
        sp::UPDATE_ZONE_PARTIAL_FOLLOWS,
        &[
            level,
            (-((tile[1] & !7) >> 3)) as u8,
            ((tile[0] & !7) >> 3) as u8,
        ],
    )
}

/// The in-zone coordinate byte of scene tile `(x, z)`.
fn zone_coord(tile: [i32; 2]) -> u8 {
    (((tile[0] & 7) << 4) | (tile[1] & 7)) as u8
}

/// `LOC_ADD_CHANGE`: `p1(shape << 2 | angle)`, `p4_alt3(id)`, `p1_alt2(coord)`.
fn loc_add_frame(tile: [i32; 2], id: i32, shape: u8, angle: u8) -> Vec<u8> {
    use crate::proto::server as sp;
    let id = id as u32;
    server_frame(
        sp::LOC_ADD_CHANGE,
        &[
            shape << 2 | angle,
            (id >> 16) as u8,
            (id >> 24) as u8,
            id as u8,
            (id >> 8) as u8,
            zone_coord(tile).wrapping_neg(),
        ],
    )
}

/// `LOC_DEL`: `p1(shape << 2 | angle)`, `p1_alt3(coord)`.
fn loc_del_frame(tile: [i32; 2], shape: u8, angle: u8) -> Vec<u8> {
    use crate::proto::server as sp;
    server_frame(
        sp::LOC_DEL,
        &[shape << 2 | angle, 128u8.wrapping_sub(zone_coord(tile))],
    )
}

impl World {
    fn start() -> anyhow::Result<Self> {
        let root = &rs910_core::test_support::client_dir();
        let trace = Trace::load(&root.join(FIXTURE))?;
        let (mut app, (peer, dir, clock)) = Replay::start(&trace)?.into_app();
        // A pack-backed scene without a window or device.
        app.scene.pack_root = Some(std::path::PathBuf::new());
        let game = app.core.session.game().context("game")?;
        let base = [game.runtime.map.base_x, game.runtime.map.base_z];
        Ok(Self {
            app,
            peer,
            base,
            _keep: Box::new((dir, clock)),
        })
    }

    /// One server tick of `frames` on the world socket, read by the client's
    /// logic cycle the way the session replay reads its recorded ticks.
    fn serve(&mut self, frames: Vec<Vec<u8>>) -> anyhow::Result<()> {
        use crate::proto::server as sp;
        use std::io::Write;
        let mut bytes: Vec<u8> = frames.concat();
        bytes.extend(server_frame(sp::SERVER_TICK_END, &[]));
        self.peer.write_all(&bytes)?;
        let session = self.app.core.session.as_mut().context("session")?;
        let started = std::time::Instant::now();
        let stream = session.io.world.stream.as_ref().context("world stream")?;
        let mut probe = vec![0u8; bytes.len()];
        while !matches!(stream.peek(&mut probe), Ok(n) if n >= bytes.len()) {
            anyhow::ensure!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "loopback delivery stalled"
            );
            std::thread::yield_now();
        }
        loop {
            match poll_live_session(session, &mut LiveIo) {
                LivePollOutcome::Lost(reason) => anyhow::bail!("connection lost: {reason}"),
                LivePollOutcome::Idle | LivePollOutcome::Polled(_) => {}
            }
            let game = session.game.as_ref().context("game")?;
            if session.io.world.pending.is_empty() && game.runtime.feed.front().is_none() {
                return Ok(());
            }
            anyhow::ensure!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "the tick was not read"
            );
            std::thread::yield_now();
        }
    }

    /// The camera over scene tile `tile`, looking north-east down at it.
    fn camera_over(&self, tile: [i32; 2]) -> OrbitCamera {
        let height = self
            .app
            .scene
            .floors
            .first()
            .and_then(Option::as_ref)
            .map_or(0, |floor| {
                floor
                    .heights
                    .get_fine_height(tile[0] * 512 + 256, tile[1] * 512 + 256)
            });
        let mut camera = OrbitCamera::new(glam::Vec3::new(
            (self.base[0] + tile[0]) as f32 + 0.5,
            -(height as f32) / 512.0,
            (self.base[1] + tile[1]) as f32 + 0.5,
        ));
        camera.pitch = 1500.0;
        camera.yaw = 1000.0;
        camera.zoom = Some(crate::camera::camera_zoom(VIEWPORT[2], VIEWPORT[3]));
        camera.scene_viewport = Some(VIEWPORT);
        camera
    }

    /// A full redraw without the GPU: the app's own entity passes (transient
    /// entities, the static-slot swap, the draw plan), then the pick list the
    /// UI tick reads, collected as the redraw collects it.
    fn redraw(&mut self, camera: &OrbitCamera) -> anyhow::Result<Frame> {
        let sun = crate::floor::SunLighting::environment_default(3, 0.0);
        self.app.add_transient_entities()?;
        {
            let scene = self.app.scene.graph.as_ref().context("scene")?;
            self.app
                .scene
                .live
                .as_mut()
                .context("live scene")?
                .install_temporary(scene);
        }
        self.app.apply_pending_location_models(&sun)?;
        let size = (VIEWPORT[2] as u32, VIEWPORT[3] as u32);
        let scene_camera = camera.scene_camera(size);
        {
            let scene = self.app.scene.graph.as_mut().context("scene")?;
            let live = self.app.scene.live.as_mut().context("live scene")?;
            live.update(
                scene,
                scene_camera.clone(),
                self.app.scene.floor_base,
                self.app.scene.focus_level as usize,
                [-1, -1],
            );
        }
        // The animated locs' models are posed after the plan, before the
        // bodies and the pick list (the order of the redraw).
        self.app.apply_pending_loc_animations();
        self.app.refresh_dynamic_scene(&sun)?;
        {
            let scene = self.app.scene.graph.as_mut().context("scene")?;
            let live = self.app.scene.live.as_mut().context("live scene")?;
            // What the body refresh keeps in the plan: a transient entity
            // drawn from its own model, a promoted replacement not at all (its
            // slot draws it), the rest by their model and the view planes.
            let planes = live.draw.plan.model_planes;
            let drawn = |live: &crate::live_scene::LiveScene, scene: &Scene, id: &usize| {
                let entity = &live.entities[*id];
                match entity.source {
                    EntityRef::Temporary(index) => {
                        scene.temporary[index].transient
                            && crate::dynamic_scene::model(scene, entity.source).is_some()
                    }
                    source => {
                        crate::dynamic_scene::model(scene, source).is_some()
                            && crate::draw::entity_model_visible(entity, &planes)
                    }
                }
            };
            let plan = &live.draw.plan;
            let opaque: Vec<usize> = plan
                .dispatch_opaque
                .iter()
                .copied()
                .filter(|id| drawn(live, scene, id))
                .collect();
            let transparent: Vec<usize> = plan
                .dispatch_transparent
                .iter()
                .copied()
                .filter(|id| drawn(live, scene, id))
                .collect();
            live.draw.plan.opaque = opaque;
            live.draw.plan.transparent = transparent;
        }
        self.app.apply_pending_location_models(&sun)?;
        let game = self.app.core.session.game().context("game")?;
        let mut frame = Frame::new(
            &scene_camera,
            [self.base[0] << 9, self.base[1] << 9],
            game.runtime.terrain_generation,
            VIEWPORT,
        );
        let ui = self.app.core.session.ui().context("ui")?;
        let scene = self.app.scene.graph.as_mut().context("scene")?;
        let live = self.app.scene.live.as_ref().context("live scene")?;
        let occupants = scene_host::slot_occupants(
            &self.app.scene.loc_changes,
            self.app.scene.loc_slots.as_ref(),
        );
        frame.collect_scene(scene, live, ui.engine.configs.locs.as_deref(), &occupants);
        Ok(frame)
    }

    /// The logic cycles that follow a redraw: its temporary entities leave the
    /// scene before the mouse is tested against the pick list again.
    fn logic_cycle(&mut self, frame: &mut Frame, mouse: [i32; 2]) {
        self.app.end_presentation();
        if let Some(scene) = self.app.scene.graph.as_mut() {
            frame.refresh_locs(scene, mouse);
        }
    }

    /// The pick of the loc on scene tile `tile` in `frame`, if one is drawn.
    fn pick_on(frame: &Frame, tile: [i32; 2]) -> Option<usize> {
        frame.loc_picks.iter().position(|pick| pick.tile == tile)
    }

    /// A canvas point the pick `k` is hit at, nearest the middle of its
    /// capsule, tested with the redraw's own models.
    fn point_on(&mut self, frame: &mut Frame, k: usize) -> anyhow::Result<[i32; 2]> {
        let scene = self.app.scene.graph.as_mut().context("scene")?;
        let c = frame.loc_picks[k].capsule;
        let mid = [(c.a[0] + c.b[0]) / 2, (c.a[1] + c.b[1]) / 2];
        let r = c.radius + (c.a[1] - c.b[1]).abs();
        let mut best: Option<(i32, [i32; 2])> = None;
        for y in (mid[1] - r..=mid[1] + r).step_by(2) {
            for x in (mid[0] - r..=mid[0] + r).step_by(2) {
                frame.refresh_locs(scene, [x, y]);
                if frame.loc_picks[k].hit {
                    let d = (x - mid[0]).pow(2) + (y - mid[1]).pow(2);
                    if best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, [x, y]));
                    }
                }
            }
        }
        best.map(|(_, point)| point)
            .context("the loc is never hit in its own redraw")
    }
}

/// A loc the server adds on an empty tile is drawn as a transient entity of
/// the redraw. Its pick keeps hitting through the logic cycles, which run
/// after the transient entities left the scene; the same loc removed again is
/// no longer picked.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn a_loc_added_at_run_time_is_picked_until_it_is_removed() -> anyhow::Result<()> {
    let mut world = World::start()?;
    let tile = [3224 - world.base[0], 3224 - world.base[1]];
    let camera = world.camera_over(tile);

    let mut frame = world.redraw(&camera)?;
    assert_eq!(World::pick_on(&frame, tile), None, "an empty tile");
    world.app.end_presentation();

    world.serve(vec![
        zone_frame(0, tile),
        loc_add_frame(tile, ROCK, CENTREPIECE, 0),
    ])?;
    frame = world.redraw(&camera)?;
    let k = World::pick_on(&frame, tile).context("the added rock is not in the pick list")?;
    let pick = &frame.loc_picks[k];
    assert_eq!(
        (pick.id, pick.shape, pick.angle),
        (ROCK, i32::from(CENTREPIECE), 0)
    );
    let point = world.point_on(&mut frame, k)?;
    world.logic_cycle(&mut frame, point);
    assert!(
        frame.loc_picks[k].hit,
        "the added rock is hit at {point:?} by the logic cycle after its redraw"
    );
    world.logic_cycle(&mut frame, [0, 0]);
    assert!(!frame.loc_picks[k].hit, "and not away from it");

    world.serve(vec![
        zone_frame(0, tile),
        loc_del_frame(tile, CENTREPIECE, 0),
    ])?;
    frame = world.redraw(&camera)?;
    assert_eq!(World::pick_on(&frame, tile), None, "removed again");
    Ok(())
}

impl World {
    /// A map loc to change: a 1x1 level-0 centrepiece near the spawn with a
    /// first op, no clickbox and a model, as `(scene tile, id, angle)`.
    fn map_loc(&mut self) -> anyhow::Result<([i32; 2], i32, u8)> {
        let scene = self.app.scene.graph.as_ref().context("scene")?;
        let ui = self.app.core.session.ui().context("ui")?;
        let locs = ui.engine.configs.locs.as_deref().context("loc configs")?;
        let local = [3222 - self.base[0], 3222 - self.base[1]];
        scene
            .scenery
            .iter()
            .filter(|e| {
                e.level == 0
                    && e.shape == i32::from(CENTREPIECE)
                    && e.active
                    && !e.dynamic
                    && e.model.is_some()
                    && (e.min_tx, e.min_tz) == (e.max_tx, e.max_tz)
                    && (e.min_tx - local[0]).abs() <= 12
                    && (e.min_tz - local[1]).abs() <= 12
                    && locs
                        .get(e.loc_id)
                        .is_some_and(|loc| loc.ops[0].is_some() && loc.clickbox.is_none())
            })
            .map(|e| ([e.min_tx, e.min_tz], e.loc_id as i32, e.angle as u8))
            .next()
            .context("no centrepiece with an op near the spawn")
    }
}

/// A loc the map placed is changed by the server (a tree to its stump) and
/// then removed: each state is what the logic cycles pick.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn a_map_loc_changed_then_removed_is_picked_as_what_it_is_now() -> anyhow::Result<()> {
    let mut world = World::start()?;
    let (tile, original, angle) = world.map_loc()?;
    let camera = world.camera_over(tile);

    let mut frame = world.redraw(&camera)?;
    let k = World::pick_on(&frame, tile).context("the map loc is not in the pick list")?;
    assert_eq!(frame.loc_picks[k].id, original);
    let point = world.point_on(&mut frame, k)?;
    world.logic_cycle(&mut frame, point);
    assert!(frame.loc_picks[k].hit, "the map loc at {point:?}");

    world.serve(vec![
        zone_frame(0, tile),
        loc_add_frame(tile, STUMP, CENTREPIECE, angle),
    ])?;
    frame = world.redraw(&camera)?;
    let k = World::pick_on(&frame, tile).context("the stump is not in the pick list")?;
    let pick = &frame.loc_picks[k];
    assert_eq!(
        (pick.id, pick.shape, pick.angle),
        (STUMP, i32::from(CENTREPIECE), i32::from(angle)),
        "the changed slot is picked as the stump"
    );
    let revisions: Vec<_> = world
        .app
        .scene
        .live
        .as_ref()
        .context("live scene")?
        .entities
        .iter()
        .enumerate()
        .map(|(id, _)| world.app.scene.live.as_ref().unwrap().slot_changes(id))
        .collect();
    frame = world.redraw(&camera)?;
    assert_eq!(
        World::pick_on(&frame, tile).map(|index| frame.loc_picks[index].id),
        Some(STUMP)
    );
    let live = world.app.scene.live.as_ref().context("live scene")?;
    assert!(
        revisions
            .iter()
            .enumerate()
            .all(|(id, revision)| live.slot_changes(id) == *revision),
        "an unchanged static loc replacement must retain its scene revision"
    );
    let k = World::pick_on(&frame, tile).context("cached stump pick")?;
    let point = world.point_on(&mut frame, k)?;
    world.logic_cycle(&mut frame, point);
    assert!(frame.loc_picks[k].hit, "the stump at {point:?}");

    world.serve(vec![
        zone_frame(0, tile),
        loc_del_frame(tile, CENTREPIECE, angle),
    ])?;
    frame = world.redraw(&camera)?;
    assert_eq!(World::pick_on(&frame, tile), None, "removed");
    let live = world.app.scene.live.as_ref().context("live scene")?;
    let revisions: Vec<_> = live
        .entities
        .iter()
        .enumerate()
        .map(|(id, _)| live.slot_changes(id))
        .collect();
    frame = world.redraw(&camera)?;
    assert_eq!(World::pick_on(&frame, tile), None, "retained removal");
    let live = world.app.scene.live.as_ref().context("live scene")?;
    assert!(
        revisions
            .iter()
            .enumerate()
            .all(|(id, revision)| live.slot_changes(id) == *revision),
        "an unchanged loc removal must retain its scene revision"
    );
    world.serve(vec![
        zone_frame(0, tile),
        loc_add_frame(tile, STUMP, CENTREPIECE, angle),
    ])?;
    frame = world.redraw(&camera)?;
    assert_eq!(
        World::pick_on(&frame, tile).map(|i| frame.loc_picks[i].id),
        Some(STUMP)
    );
    world.serve(vec![
        zone_frame(0, tile),
        loc_del_frame(tile, CENTREPIECE, angle),
    ])?;
    frame = world.redraw(&camera)?;
    assert_eq!(World::pick_on(&frame, tile), None, "removed after re-add");
    Ok(())
}

impl World {
    /// An animated map loc (a furnace, an anvil, a fire): a level-0 scenery
    /// entity the dynamic scene poses, with an op and no clickbox, nearest the
    /// spawn, as `(scene tile, id, shape, angle)`.
    fn animated_map_loc(&self) -> anyhow::Result<([i32; 2], i32, u8, u8)> {
        let scene = self.app.scene.graph.as_ref().context("scene")?;
        let ui = self.app.core.session.ui().context("ui")?;
        let locs = ui.engine.configs.locs.as_deref().context("loc configs")?;
        let local = [3222 - self.base[0], 3222 - self.base[1]];
        scene
            .scenery
            .iter()
            .filter(|e| {
                e.level == 0
                    && e.dynamic
                    && e.active
                    && locs.get(e.loc_id).is_some_and(|loc| {
                        loc.has_anim
                            && loc.ops.iter().any(Option::is_some)
                            && loc.clickbox.is_none()
                    })
            })
            .min_by_key(|e| (e.min_tx - local[0]).abs() + (e.min_tz - local[1]).abs())
            .map(|e| {
                (
                    [e.min_tx, e.min_tz],
                    e.loc_id as i32,
                    e.shape as u8,
                    e.angle as u8,
                )
            })
            .context("no animated loc with an op in the scene")
    }
}

/// Animated locs are posed by the dynamic scene after the draw plan. The
/// pick list is collected over those poses, so an animated map loc picks like
/// a static one, and an animated loc the server adds picks through the logic
/// cycles after its redraw.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn animated_locs_are_picked_in_the_map_and_when_added() -> anyhow::Result<()> {
    let mut world = World::start()?;
    let (tile, id, shape, angle) = world.animated_map_loc()?;
    let camera = world.camera_over(tile);

    let mut frame = world.redraw(&camera)?;
    let k = World::pick_on(&frame, tile).context("the animated map loc is not in the pick list")?;
    assert_eq!(frame.loc_picks[k].id, id);
    let point = world.point_on(&mut frame, k)?;
    world.logic_cycle(&mut frame, point);
    assert!(frame.loc_picks[k].hit, "the animated map loc at {point:?}");

    // The same loc on the spawn's empty neighbour tile.
    let added = [3224 - world.base[0], 3224 - world.base[1]];
    let camera = world.camera_over(added);
    world.serve(vec![
        zone_frame(0, added),
        loc_add_frame(added, id, shape, angle),
    ])?;
    let mut frame = world.redraw(&camera)?;
    let k =
        World::pick_on(&frame, added).context("the added animated loc is not in the pick list")?;
    assert_eq!(frame.loc_picks[k].id, id);
    let point = world.point_on(&mut frame, k)?;
    world.logic_cycle(&mut frame, point);
    assert!(
        frame.loc_picks[k].hit,
        "the added animated loc at {point:?}"
    );
    Ok(())
}

impl World {
    /// The loc the redraw built for the server's request on scene tile
    /// `tile`, and whether the draw plan draws it.
    fn added_loc(&self, tile: [i32; 2]) -> anyhow::Result<(&crate::scene::TemporaryEntity, bool)> {
        let scene = self.app.scene.graph.as_ref().context("scene")?;
        let live = self.app.scene.live.as_ref().context("live scene")?;
        let (index, entity) = scene
            .temporary
            .iter()
            .enumerate()
            .find(|(_, e)| e.location_key.is_some_and(|(_, _, x, z)| [x, z] == tile))
            .context("no loc was built for the request")?;
        let plan = &live.draw.plan;
        let drawn = live.entities.iter().enumerate().any(|(id, e)| {
            e.source == EntityRef::Temporary(index)
                && (plan.opaque.contains(&id) || plan.transparent.contains(&id))
        });
        Ok((entity, drawn))
    }
}

/// An energy rift is two locs that share a centre, as the map places them:
/// the rift, a 2x2 plane the player clicks whose faces are fully
/// transparent, and the 4x4 scenery one tile south-west of it that draws the
/// rift. Added by the server while the client runs, both are drawn and stand
/// as the map stands them, on the centre of their footprint at the floor
/// height there, so the plane lies under the drawn rift, and the scenery plays
/// its animation.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn an_energy_rift_added_at_run_time_stands_where_the_map_puts_one() -> anyhow::Result<()> {
    use rs910_symbols::loc::{ENERGY_RIFT, ENERGY_RIFT_SCENERY};
    let mut world = World::start()?;
    let rift = [3224 - world.base[0], 3224 - world.base[1]];
    let scenery = [rift[0] - 1, rift[1] - 1];
    let camera = world.camera_over(rift);
    world.serve(vec![
        zone_frame(0, rift),
        loc_add_frame(rift, ENERGY_RIFT.id(), CENTREPIECE, 0),
        zone_frame(0, scenery),
        loc_add_frame(scenery, ENERGY_RIFT_SCENERY.id(), CENTREPIECE, 0),
    ])?;
    world.redraw(&camera)?;

    // The shared centre is the tile corner north-east of the rift's tile.
    let corner = [rift[0] + 1, rift[1] + 1];
    let floor = world.app.scene.floors[0].as_ref().context("floor")?;
    let ground = floor
        .heights
        .get_tile_height(corner[0] as usize, corner[1] as usize);
    let centre = [
        (corner[0] << 9) as f32,
        ground as f32,
        (corner[1] << 9) as f32,
    ];

    let (plane, drawn) = world.added_loc(rift)?;
    assert!(drawn, "the rift's plane is in the draw plan");
    assert_eq!(
        plane.position, centre,
        "the rift stands on its footprint's centre"
    );
    assert_eq!(plane.bounds, [rift[0], rift[0] + 1, rift[1], rift[1] + 1]);
    let faces = &plane.model.as_ref().context("the rift's model")?.face_alpha;
    assert!(
        !faces.is_empty() && faces.iter().all(|&alpha| alpha as u8 == u8::MAX),
        "the rift's plane is fully transparent: {faces:?}"
    );

    let (body, drawn) = world.added_loc(scenery)?;
    assert!(drawn, "the rift's scenery is in the draw plan");
    assert_eq!(
        body.position, centre,
        "the scenery shares the rift's centre"
    );
    assert_eq!(
        body.bounds,
        [scenery[0], scenery[0] + 3, scenery[1], scenery[1] + 3]
    );
    let model = body.model.as_ref().context("the scenery's model")?;
    assert!(
        model.face_alpha.contains(&0),
        "the scenery draws opaque faces"
    );

    // It plays its type's animation, as the map's rift scenery does: half a
    // second later its model has moved.
    let posed = (model.vx.clone(), model.vy.clone(), model.vz.clone());
    world.app.end_presentation();
    world.app.core.cycle += 25;
    world.redraw(&camera)?;
    let (body, drawn) = world.added_loc(scenery)?;
    let model = body.model.as_ref().context("the scenery's model")?;
    assert!(drawn);
    assert_ne!(
        (model.vx.clone(), model.vy.clone(), model.vz.clone()),
        posed,
        "the scenery is animated"
    );
    Ok(())
}
