//! E1/E2: CPU scene draw planner (draw-list building, culling and ordering).
//! Verified through committed decision traces, not screenshot order.
//! E2: optional normal-scene software occlusion, verified by frame traces.
//! Roof stamps come from `roof::RoofState` in the live viewer.
//! E4: live model rejection/order and floor selections share this planner.
//! E5: `material` also records normal programmable-GLX draw state and uniforms.
use crate::camera::{self, CpuProjection, Matrix4x3};
pub use crate::draw_entity::*;
use crate::draw_trace::Trace;
use crate::floor::FloorGeometry;
use crate::floor::FloorHeights;
use crate::material::{MaterialSpec, MaterialState};
use crate::scene::{EntityRef, Scene};
use std::collections::HashSet;
use std::path::Path;

/// Temporary primaries follow pending entities in forward array order.
/// Their bounds are `None`, so occlusion never rejects them.
pub fn append_temporary(scene: &Scene, out: &mut Vec<DrawEntity>) {
    for (i, e) in scene.temporary.iter().enumerate() {
        let [x, y, z] = e.position.map(|v| v as i32);
        let b = e.bounds;
        out.push(DrawEntity {
            source: EntityRef::Temporary(i),
            dynamic: false,
            bounds: None,
            wall_type: 0,
            cylinder: None,
            position: Some(e.position),
            precise_cylinder: None,
            id: out.len() as i32,
            bucket: 3,
            kind: 4,
            loc_id: e.player as i32,
            shape: 0,
            angle: 0,
            level: e.level,
            occlude_level: e.occlude_level,
            x,
            y,
            z,
            tiles: b,
            overlay_height: e.overlay_height,
            transparent: e.transparent,
        });
    }
}

/// Both bucket quicksorts, including index parity and wrapping pivot
/// addition. Pairs are (entity, depth).
pub fn sort_bucket(entries: &mut [(i32, i32)], transparent: bool) {
    fn sort(a: &mut [(i32, i32)], lo: i32, hi: i32, reverse: bool) {
        if lo >= hi {
            return;
        }
        let pivot = (lo + hi) / 2;
        a.swap(pivot as usize, hi as usize);
        let depth = a[hi as usize].1;
        let mut at = lo;
        for i in lo..hi {
            let boundary = depth.wrapping_add(i & 1);
            if if reverse {
                a[i as usize].1 > boundary
            } else {
                a[i as usize].1 < boundary
            } {
                a.swap(i as usize, at as usize);
                at += 1;
            }
        }
        a.swap(hi as usize, at as usize);
        sort(a, lo, at - 1, reverse);
        sort(a, at + 1, hi, reverse);
    }
    sort(entries, 0, entries.len() as i32 - 1, transparent);
}

/// GPU input produced by the same decision loop as the committed decision traces.
/// (`FramePlan` before renderer plan A1; renamed so it does not clash with
/// the toolkit's frame, `rs910_toolkit::FramePlan`.)
#[derive(Clone, Default)]
pub struct DrawPlan {
    pub dispatched: Vec<usize>,
    /// Projected depth for each `dispatched` entry; the pickable-entity list
    /// orders by it.
    pub dispatched_depth: Vec<i32>,
    pub dispatch_opaque: Vec<usize>,
    pub dispatch_transparent: Vec<usize>,
    /// Culled entities near the eye still get their particle update.
    pub culled_updates: Vec<usize>,
    pub model_planes: [[f32; 4]; 6],
    pub opaque: Vec<usize>,
    pub transparent: Vec<usize>,
    pub floors: Vec<FloorSelection>,
    /// The draw-list build over the underwater scene:
    /// indices into its entity list, opaque near-first and transparent
    /// far-first, after the frustum test.
    pub underwater_opaque: Vec<usize>,
    pub underwater_transparent: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloorSelection {
    pub whole: bool,
    pub origin: [i32; 2],
    pub distance: i32,
    pub mask: Vec<Vec<bool>>,
}
impl FloorSelection {
    pub fn visible(&self, x: i32, z: i32) -> bool {
        let [ox, oz] = self.origin;
        let (x, z) = (x - ox, z - oz);
        x >= 0
            && z >= 0
            && x <= 2 * self.distance
            && z <= 2 * self.distance
            && self
                .mask
                .get(x as usize)
                .and_then(|r| r.get(z as usize))
                .copied()
                .unwrap_or(false)
    }
    /// Visible tile indices: X outer, Z inner, inclusive range.
    pub fn tiles(&self, nx: usize, nz: usize) -> Vec<usize> {
        let mut out = Vec::new();
        for x in 0..nx {
            for z in 0..nz {
                if self.visible(x as i32, z as i32) {
                    out.push(z * nx + x);
                }
            }
        }
        out
    }
}

/// One planned frame's camera and switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawFrame {
    /// Frame id: the client cycle; it tags the sections of a trace.
    pub id: i32,
    /// Eye in scene fine units.
    pub eye: [i32; 3],
    /// Camera angles (14-bit).
    pub yaw: i32,
    pub pitch: i32,
    /// Surface size in pixels (the occlusion raster is a third of it).
    pub surface: [i32; 2],
    /// Viewport rectangle: x, y, width, height.
    pub viewport: [i32; 4],
    /// Near and far clip planes.
    pub near: i32,
    pub far: i32,
    /// The roof stamp value that marks a roofed tile as hidden.
    pub roof_stamp: i32,
    /// The lowest level the roof hides; levels below it are drawn whole.
    pub roof_level: i32,
    /// Whether the visibility pass runs (the frustum switch).
    pub cull: bool,
    /// Entities and floors left out: bit 1 the transparent entities, bit 2
    /// the opaque entities and the floors.
    pub hide: i32,
    /// Occlusion mode: 0 off, 1 on, 2 on with a whole-scene exclusion box.
    pub occlusion: i32,
}

impl DrawFrame {
    /// The frame as the 19 numbers a draw fixture row holds (frame id, eye,
    /// yaw, pitch, surface, viewport, clip planes, roof stamp and level,
    /// cull switch, hide bits, occlusion mode).
    pub fn from_words(w: [i32; 19]) -> Self {
        Self {
            id: w[0],
            eye: [w[1], w[2], w[3]],
            yaw: w[4],
            pitch: w[5],
            surface: [w[6], w[7]],
            viewport: [w[8], w[9], w[10], w[11]],
            near: w[12],
            far: w[13],
            roof_stamp: w[14],
            roof_level: w[15],
            cull: w[16] != 0,
            hide: w[17],
            occlusion: w[18],
        }
    }

    /// The inverse of [`Self::from_words`].
    pub fn words(&self) -> [i32; 19] {
        let [ex, ey, ez] = self.eye;
        let [sw, sh] = self.surface;
        let [vx, vy, vw, vh] = self.viewport;
        [
            self.id,
            ex,
            ey,
            ez,
            self.yaw,
            self.pitch,
            sw,
            sh,
            vx,
            vy,
            vw,
            vh,
            self.near,
            self.far,
            self.roof_stamp,
            self.roof_level,
            i32::from(self.cull),
            self.hide,
            self.occlusion,
        ]
    }
}

/// The scene a frame is planned over: the scene graph, the floor heights of
/// every level and the drawable entities.
#[derive(Clone, Copy)]
pub struct PlannerScene<'a> {
    pub scene: &'a Scene,
    pub heights: &'a [FloorHeights],
    pub entities: &'a [DrawEntity],
}

/// What the live planner takes besides the frame: the roof exclusion boxes,
/// the view and projection of a free camera, and the underwater scene's
/// entities and floor heights when it exists.
#[derive(Clone, Copy)]
pub struct LiveInputs<'a> {
    pub boxes: &'a [crate::occlusion::Exclusion],
    pub cam2: Option<([f32; 16], [f32; 16])>,
    pub underwater: Option<(&'a [DrawEntity], &'a [FloorHeights])>,
}

/// How [`DrawState::frame_inner`] runs: with or without the trace and its
/// depth sections, and the live extras.
struct FrameMode<'a> {
    e2: bool,
    record: bool,
    live_boxes: Option<&'a [crate::occlusion::Exclusion]>,
    cam2: Option<([f32; 16], [f32; 16])>,
    underwater: Option<(&'a [DrawEntity], &'a [FloorHeights])>,
}

pub struct DrawState {
    pub plan: DrawPlan,
    pub distance: i32,
    pub visibility: Vec<Vec<bool>>,
    scratch: Vec<Vec<bool>>,
    row: Vec<i32>,
    cycle: i32,
}

fn mask_words(mask: &[Vec<bool>]) -> Vec<i32> {
    let mut out = vec![mask.len() as i32, mask[0].len() as i32];
    out.extend(mask.iter().flatten().map(|&v| i32::from(v)));
    out
}

impl DrawState {
    /// Scene constructor; masks survive across frame calls.
    pub fn new(distance: i32) -> Self {
        let n = (distance * 2 + 2) as usize;
        Self {
            plan: DrawPlan::default(),
            distance,
            visibility: vec![vec![false; n - 1]; n - 1],
            scratch: vec![vec![false; n]; n],
            row: vec![0; n],
            cycle: 0,
        }
    }

    /// Normal static scene, the frustum switch and a supplied roof mask.
    /// Fixture column 19 enables the depth occlusion; absent or zero keeps it off.
    pub fn frame(
        &mut self,
        world: PlannerScene<'_>,
        view: DrawFrame,
        roof: Option<&[Vec<Vec<i8>>]>,
        occlusion: &mut crate::occlusion::Occlusion,
        e2: bool,
    ) -> Trace {
        let mode = FrameMode {
            e2,
            record: true,
            live_boxes: None,
            cam2: None,
            underwater: None,
        };
        self.frame_inner(world, view, roof, occlusion, mode)
    }

    pub fn live_frame(
        &mut self,
        world: PlannerScene<'_>,
        view: DrawFrame,
        roof: Option<&[Vec<Vec<i8>>]>,
        occlusion: &mut crate::occlusion::Occlusion,
        live: LiveInputs<'_>,
    ) {
        let mode = FrameMode {
            e2: true,
            record: false,
            live_boxes: Some(live.boxes),
            cam2: live.cam2,
            underwater: live.underwater,
        };
        self.frame_inner(world, view, roof, occlusion, mode);
    }

    fn frame_inner(
        &mut self,
        world: PlannerScene<'_>,
        view: DrawFrame,
        roof: Option<&[Vec<Vec<i8>>]>,
        occlusion: &mut crate::occlusion::Occlusion,
        mode: FrameMode<'_>,
    ) -> Trace {
        let PlannerScene {
            scene,
            heights,
            entities,
        } = world;
        let FrameMode {
            e2,
            record,
            live_boxes,
            cam2,
            underwater,
        } = mode;
        // Last frame's floor selections: their masks are reused for this
        // frame's (same shape; programme Phase 6).
        let mut recycled_floors = std::mem::take(&mut self.plan.floors).into_iter();
        self.plan = DrawPlan::default();
        occlusion.record = record;
        let mut view_matrix = Matrix4x3::translation(
            -view.eye[0] as f32,
            -view.eye[1] as f32,
            -view.eye[2] as f32,
        );
        view_matrix.rotate_around_axis(0.0, -1.0, 0.0, crate::trig::radians(-view.yaw & 16383));
        view_matrix.rotate_around_axis(-1.0, 0.0, 0.0, crate::trig::radians(-view.pitch & 16383));
        view_matrix.rotate_around_axis(0.0, 0.0, -1.0, crate::trig::radians(0));
        let zoom = camera::camera_zoom(view.viewport[2], view.viewport[3]) << 1;
        let projection = camera::perspective_pixels(camera::PixelLens {
            centre: [(view.viewport[2] / 2) as f32, (view.viewport[3] / 2) as f32],
            focal: [zoom as f32, zoom as f32],
            near: view.near as f32,
            far: view.far as f32,
            size: [view.viewport[2] as f32, view.viewport[3] as f32],
        });
        let (view_entries, projection) = cam2.unwrap_or((view_matrix.to_entries(), projection));
        let cpu = CpuProjection::new(view_entries, projection, view.viewport);
        let mut trace = Trace::default();
        let prefix = format!("frame/{}/", view.id);
        if record {
            trace.push(
                format!("{prefix}input"),
                if e2 {
                    view.words().to_vec()
                } else {
                    view.words()[..18].to_vec()
                },
            );
        }
        for (name, m) in [
            ("view", cpu.view),
            ("projection", cpu.projection),
            ("viewProjection", cpu.combined),
        ] {
            if record {
                trace.push(
                    format!("{prefix}{name}"),
                    m.iter().map(|v| v.to_bits() as i32).collect(),
                );
            }
        }
        let mut gpu = cpu.combined;
        camera::glx_flip_y(&mut gpu);
        if record {
            trace.push(
                format!("{prefix}gpuViewProjection"),
                gpu.iter().map(|v| v.to_bits() as i32).collect(),
            );
        }
        let d = self.distance;
        let n = (2 * d + 2) as usize;
        let (ex, ez) = (view.eye[0] >> scene.size, view.eye[2] >> scene.size);
        let (mut min_x, mut min_z) = ((ex - d).max(0), (ez - d).max(0));
        let (bx, bz) = ((d - ex).max(0), (d - ez).max(0));
        let (mut max_x, mut max_z) = (
            (ex + d).min(scene.max_x as i32),
            (ez + d).min(scene.max_z as i32),
        );
        let mut corners = Vec::new();
        let enabled = view.cull;
        if enabled {
            for x in 0..n {
                let mut previous = 0;
                let mut current = 0;
                for z in 0..n {
                    if z > 1 {
                        self.row[z - 2] = previous;
                    }
                    previous = current;
                    let (tx, tz) = (ex - d + x as i32, ez - d + z as i32);
                    current =
                        if tx >= 0 && tz >= 0 && tx < scene.max_x as i32 && tz < scene.max_z as i32
                        {
                            let margin = 1000 << (scene.size - 7);
                            let top = heights
                                .last()
                                .unwrap()
                                .get_tile_height(tx as usize, tz as usize)
                                - margin;
                            let bottom = heights[0].get_tile_height(tx as usize, tz as usize)
                                + scene.tile_size
                                + margin;
                            cpu.segment_code(
                                [tx << scene.size, top, tz << scene.size],
                                [tx << scene.size, bottom, tz << scene.size],
                            )
                        } else {
                            -1
                        };
                    self.scratch[x][z] = current == 0;
                    if record {
                        corners.push(current);
                    }
                    if x > 0 && z > 0 {
                        self.visibility[x - 1][z - 1] =
                            self.row[z - 1] & self.row[z] & previous & current == 0;
                    }
                }
                self.row[n - 2] = previous;
                self.row[n - 1] = current;
            }
        } else {
            min_x = 0;
            min_z = 0;
            max_x = scene.max_x as i32;
            max_z = scene.max_z as i32;
        }
        if record {
            trace.push(format!("{prefix}corners"), corners);
        }
        // Only read without the visibility pass (`!enabled`).
        let all = if enabled {
            Vec::new()
        } else {
            vec![vec![true; n]; n]
        };
        let visibility = if enabled { &self.visibility } else { &all };
        let boxes = if view.occlusion == 2 {
            vec![crate::occlusion::Exclusion {
                max_height: i32::MAX,
                min_x: i32::MIN,
                max_x: i32::MAX,
                min_z: i32::MIN,
                max_z: i32::MAX,
            }]
        } else {
            vec![]
        };
        occlusion.prepare(
            &crate::occlusion::OcclusionView {
                cpu: &cpu,
                eye: view.eye,
                surface: view.surface,
                distance: d,
                visibility,
                roof_level: view.roof_level,
                boxes: live_boxes.unwrap_or(&boxes),
            },
            enabled && view.occlusion != 0,
        );
        let visible_tile = |tx: i32, tz: i32| -> bool {
            let (x, z) = (tx - ex + d, tz - ez + d);
            x >= 0
                && z >= 0
                && (x as usize) < visibility.len()
                && (z as usize) < visibility.len()
                && visibility[x as usize][z as usize]
        };
        if let Some((uw_entities, uw_heights)) = underwater {
            // The underwater entities are depth-projected, frustum-tested and sorted like
            // the normal lists (max level 1, underwater heightmaps), without touching
            // them. This pass counts its own `cycle` before the normal lists, and its
            // culling test with `skip_particles = true` never updates particles.
            self.cycle = self.cycle.wrapping_add(1);
            occlusion.set_cycle(self.cycle);
            let mut opaque = Vec::new();
            let mut transparent = Vec::new();
            for e in uw_entities {
                if (e.bucket == 0 && view.hide & 2 != 0) || (e.bucket != 0 && view.hide & 1 != 0) {
                    continue;
                }
                let mut point = e.position.unwrap_or([e.x as f32, e.y as f32, e.z as f32]);
                point[1] = (point[1] as i32).wrapping_add(e.overlay_height >> 1) as f32;
                let depth = cpu.project(point)[2] as i32;
                if depth == -1 {
                    continue;
                }
                if enabled {
                    // The footprint's first drawable tile decides.
                    let mut anchor = None;
                    'footprint: for x in e.tiles[0]..=e.tiles[1] {
                        for z in e.tiles[2]..=e.tiles[3] {
                            if e.occlude_level < 1
                                && x >= min_x
                                && x < max_x
                                && z >= min_z
                                && z < max_z
                            {
                                anchor = Some((x, z));
                                break 'footprint;
                            }
                        }
                    }
                    let Some((x, z)) = anchor else { continue };
                    if let Some(roof) = roof {
                        if e.level >= view.roof_level
                            && roof[e.level as usize][x as usize][z as usize]
                                == view.roof_stamp as i8
                        {
                            continue;
                        }
                    }
                    let mut visible = false;
                    for x in e.tiles[0]..=e.tiles[1] {
                        for z in e.tiles[2]..=e.tiles[3] {
                            visible |= visible_tile(x, z);
                        }
                    }
                    if !visible {
                        continue;
                    }
                    let (level, tx, tz) = (
                        e.occlude_level as usize,
                        (e.x >> scene.size) as usize,
                        (e.z >> scene.size) as usize,
                    );
                    let hidden = match e.kind {
                        0 | 4 => occlusion.loc_occluded(&cpu, uw_heights, level, e.tiles, e.bounds),
                        1 => occlusion.wall_occluded(
                            &cpu,
                            uw_heights,
                            crate::occlusion::WallQuery {
                                level,
                                tile: [tx, tz],
                                kind: e.wall_type,
                                height: e.overlay_height,
                            },
                        ),
                        2 => occlusion.decor_occluded(
                            &cpu,
                            uw_heights,
                            level,
                            tx,
                            tz,
                            e.overlay_height,
                        ),
                        _ => occlusion.tile_occluded(&cpu, uw_heights, level, tx, tz),
                    };
                    if hidden {
                        continue;
                    }
                }
                if e.bucket == 1 || (e.bucket >= 2 && e.transparent) {
                    transparent.push((e.id, depth));
                } else {
                    opaque.push((e.id, depth));
                }
            }
            sort_bucket(&mut opaque, false);
            sort_bucket(&mut transparent, true);
            let planes = model_planes(cpu.combined);
            let visible = |id: &usize| entity_model_visible(&uw_entities[*id], &planes);
            self.plan.underwater_opaque = opaque
                .iter()
                .map(|e| e.0 as usize)
                .filter(visible)
                .collect();
            self.plan.underwater_transparent = transparent
                .iter()
                .map(|e| e.0 as usize)
                .filter(visible)
                .collect();
        }
        self.cycle = self.cycle.wrapping_add(1);
        occlusion.set_cycle(self.cycle);
        let mut projected = Vec::new();
        let mut opaque = Vec::new();
        let mut transparent = Vec::new();
        for e in entities {
            if (e.bucket == 0 && view.hide & 2 != 0) || (e.bucket != 0 && view.hide & 1 != 0) {
                continue;
            }
            let mut point = e.position.unwrap_or([e.x as f32, e.y as f32, e.z as f32]);
            // Keeps float X/Z but truncates Y before adding the integer overlay height.
            point[1] = (point[1] as i32).wrapping_add(e.overlay_height >> 1) as f32;
            let p = cpu.project(point);
            let depth = p[2] as i32;
            if record {
                projected.push(e.id);
                projected.extend(p.iter().map(|v| v.to_bits() as i32));
                projected.push(depth);
            }
            if depth == -1 {
                continue;
            }
            if enabled {
                let mut anchor = None;
                'footprint: for x in e.tiles[0]..=e.tiles[1] {
                    for z in e.tiles[2]..=e.tiles[3] {
                        if e.occlude_level < scene.max_level as i32
                            && x >= min_x
                            && x < max_x
                            && z >= min_z
                            && z < max_z
                        {
                            anchor = Some((x, z));
                            break 'footprint;
                        }
                    }
                }
                let Some((x, z)) = anchor else { continue };
                let nearby = x >= ex - 16 && x <= ex + 16 && z >= ez - 16 && z <= ez + 16;
                if let Some(roof) = roof {
                    if e.level >= view.roof_level
                        && roof[e.level as usize][x as usize][z as usize] == view.roof_stamp as i8
                    {
                        if nearby {
                            self.plan.culled_updates.push(e.id as usize);
                        }
                        continue;
                    }
                }
                let mut visible = false;
                for x in e.tiles[0]..=e.tiles[1] {
                    for z in e.tiles[2]..=e.tiles[3] {
                        visible |= visible_tile(x, z);
                    }
                }
                if !visible {
                    if nearby {
                        self.plan.culled_updates.push(e.id as usize);
                    }
                    continue;
                }
                let (level, tx, tz) = (
                    e.occlude_level as usize,
                    (e.x >> scene.size) as usize,
                    (e.z >> scene.size) as usize,
                );
                let hidden = match e.kind {
                    0 | 4 => occlusion.loc_occluded(&cpu, heights, level, e.tiles, e.bounds),
                    1 => occlusion.wall_occluded(
                        &cpu,
                        heights,
                        crate::occlusion::WallQuery {
                            level,
                            tile: [tx, tz],
                            kind: e.wall_type,
                            height: e.overlay_height,
                        },
                    ),
                    2 => occlusion.decor_occluded(&cpu, heights, level, tx, tz, e.overlay_height),
                    _ => occlusion.tile_occluded(&cpu, heights, level, tx, tz),
                };
                if hidden {
                    if nearby {
                        self.plan.culled_updates.push(e.id as usize);
                    }
                    continue;
                }
            }
            if e.bucket == 1 || (e.bucket >= 2 && e.transparent) {
                transparent.push((e.id, depth));
            } else {
                opaque.push((e.id, depth));
            }
        }
        sort_bucket(&mut opaque, false);
        sort_bucket(&mut transparent, true);
        if record {
            trace.push(format!("{prefix}projected"), projected);
        }
        let dispatch_index = trace.0.len();
        if record {
            trace.push(format!("{prefix}dispatch"), vec![]);
        }
        let mut dispatch = Vec::new();
        for &(id, depth) in &opaque {
            dispatch.extend([0, id, depth]);
        }
        if view.hide & 2 == 0 {
            for level in 0..scene.max_level {
                let whole = (level as i32) < view.roof_level || roof.is_none();
                if enabled {
                    let nx = visibility.len() as i32
                        - (min_x + visibility.len() as i32 - scene.max_x as i32).max(0);
                    let nz = visibility[0].len() as i32
                        - (min_z + visibility[0].len() as i32 - scene.max_z as i32).max(0);
                    for x in bx..nx {
                        for z in bz..nz {
                            let (tx, tz) = ((min_x + x - bx) as usize, (min_z + z - bz) as usize);
                            let mut visible = visibility[x as usize][z as usize];
                            if !whole {
                                visible = false;
                                if visibility[x as usize][z as usize] {
                                    for plane in (0..=level).rev() {
                                        if scene
                                            .tile(plane, tx, tz)
                                            .is_some_and(|t| t.level as usize == level)
                                        {
                                            visible = (plane as i32) < view.roof_level
                                                || roof.unwrap()[plane][tx][tz]
                                                    != view.roof_stamp as i8;
                                            break;
                                        }
                                    }
                                }
                            }
                            if visible {
                                visible = !occlusion.tile_occluded(&cpu, heights, level, tx, tz);
                            }
                            self.scratch[x as usize][z as usize] = visible;
                        }
                    }
                }
                let source = if enabled { &self.scratch } else { &all };
                let mask = match recycled_floors.next() {
                    Some(FloorSelection { mut mask, .. }) => {
                        mask.clone_from(source);
                        mask
                    }
                    None => source.clone(),
                };
                self.plan.floors.push(FloorSelection {
                    whole,
                    origin: [ex - d, ez - d],
                    distance: d,
                    mask,
                });
                dispatch.extend([1, level as i32, whole as i32]);
                if record {
                    let mut words = vec![whole as i32, view.hide];
                    words.extend(mask_words(source));
                    trace.push(format!("{prefix}floor/{level}"), words);
                }
            }
        }
        for &(id, depth) in &transparent {
            dispatch.extend([0, id, depth]);
        }
        if record {
            trace.0[dispatch_index].1 = dispatch;
        }
        self.plan.dispatched = opaque
            .iter()
            .chain(&transparent)
            .map(|e| e.0 as usize)
            .collect();
        self.plan.dispatched_depth = opaque.iter().chain(&transparent).map(|e| e.1).collect();
        let planes = model_planes(cpu.combined);
        self.plan.model_planes = planes;
        self.plan.dispatch_opaque = opaque.iter().map(|e| e.0 as usize).collect();
        self.plan.dispatch_transparent = transparent.iter().map(|e| e.0 as usize).collect();
        self.plan.opaque = opaque
            .iter()
            .map(|e| e.0 as usize)
            .filter(|&id| entity_model_visible(&entities[id], &planes))
            .collect();
        self.plan.transparent = transparent
            .iter()
            .map(|e| e.0 as usize)
            .filter(|&id| entity_model_visible(&entities[id], &planes))
            .collect();
        if record {
            trace.push(
                format!("{prefix}window"),
                vec![ex, ez, min_x, min_z, max_x, max_z, bx, bz, self.cycle],
            );
        }
        if record {
            trace.push(format!("{prefix}visibility"), mask_words(&self.visibility));
        }
        if record {
            trace.push(format!("{prefix}scratchMask"), mask_words(&self.scratch));
        }
        if record {
            trace.push(
                format!("{prefix}opaque"),
                opaque.iter().flat_map(|&(id, d)| [id, d]).collect(),
            );
        }
        if record {
            trace.push(
                format!("{prefix}transparent"),
                transparent.iter().flat_map(|&(id, d)| [id, d]).collect(),
            );
        }
        if e2 && record {
            if record {
                trace.push(
                    format!("{prefix}occlusion/triangles"),
                    occlusion.triangles.clone(),
                );
            }
            if record {
                trace.push(
                    format!("{prefix}occlusion/tileQueries"),
                    occlusion.tile_queries.clone(),
                );
            }
            let mut active = Vec::new();
            for &id in &occlusion.active {
                active.push(id as i32);
                for p in occlusion.quads[id].projected {
                    active.extend(p.map(i32::from));
                }
            }
            if record {
                trace.push(format!("{prefix}occlusion/active"), active);
            }
            if record {
                trace.push(
                    format!("{prefix}occlusion/state"),
                    vec![
                        occlusion.enabled as i32,
                        occlusion.raster.width,
                        occlusion.raster.height,
                        occlusion.raster.mode.code(),
                        occlusion.raster.coverage,
                    ],
                );
            }
            if record {
                trace.push(
                    format!("{prefix}occlusion/depth"),
                    occlusion.raster.depth.clone(),
                );
            }
            if record {
                trace.push(
                    format!("{prefix}occlusion/cycles"),
                    occlusion
                        .cycles
                        .iter()
                        .flatten()
                        .flatten()
                        .copied()
                        .collect(),
                );
            }
        }
        trace
    }
}

/// Parse the shared, explicit-camera oracle input; rejects unsupported values.
pub fn fixtures(path: &Path, size: usize) -> anyhow::Result<Vec<DrawFrame>> {
    let mut out = Vec::new();
    let mut ids = HashSet::new();
    for line in std::fs::read_to_string(path)?.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let mut values: Vec<i32> = line
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()?;
        if values.len() == 18 {
            values.push(0);
        }
        let a: [i32; 19] = values
            .try_into()
            .map_err(|_| anyhow::anyhow!("expected 18 or 19 fixture fields"))?;
        let view = DrawFrame::from_words(a);
        anyhow::ensure!(ids.insert(view.id), "duplicate frame id {}", view.id);
        anyhow::ensure!(
            view.eye[0] >= 0
                && view.eye[2] >= 0
                && (view.eye[0] >> 9) < size as i32
                && (view.eye[2] >> 9) < size as i32,
            "eye outside scene"
        );
        anyhow::ensure!(
            view.surface.iter().all(|&v| v > 0)
                && view.viewport[2] > 0
                && view.viewport[3] > 0
                && view.near > 0
                && view.far > view.near,
            "invalid dimensions/clip planes"
        );
        anyhow::ensure!(
            (-1..=255).contains(&view.roof_stamp)
                && (0..=4).contains(&view.roof_level)
                && (0..=1).contains(&a[16])
                && (0..=3).contains(&view.hide),
            "unsupported fixture profile"
        );
        anyhow::ensure!(
            (0..=2).contains(&view.occlusion),
            "invalid occlusion profile"
        );
        out.push(view);
    }
    anyhow::ensure!(!out.is_empty(), "no frames");
    Ok(out)
}

/// E1/E2 harness entry after the independently built Rust scene: writes
/// `output` (the decision trace), `submissions.bin` and `materials.bin`
/// beside it.
pub fn write_oracle(
    result: &mut crate::rebuild::Rebuild,
    input: &Path,
    output: &Path,
    materials: &crate::texture::MaterialStore,
) -> anyhow::Result<()> {
    let traces = oracle_traces(result, input, materials)?;
    std::fs::write(
        output.with_file_name("materials.bin"),
        traces.materials.encode(),
    )?;
    std::fs::write(
        output.with_file_name("submissions.bin"),
        traces.submissions.encode(),
    )?;
    std::fs::write(output, traces.frames.encode())?;
    Ok(())
}

/// The three traces [`write_oracle`] writes.
pub struct OracleTraces {
    /// `frames.bin`: build entities, sort fixtures and per-frame decisions.
    pub frames: Trace,
    /// `submissions.bin`: per-frame accepted models and floor/light/shadow indices.
    pub submissions: Trace,
    /// `materials.bin`: waterfall shader parameters.
    pub materials: Trace,
}

/// The E1/E2 decision traces for the frames in `input` (`draw-frames*.txt`).
pub fn oracle_traces(
    result: &mut crate::rebuild::Rebuild,
    input: &Path,
    materials: &crate::texture::MaterialStore,
) -> anyhow::Result<OracleTraces> {
    let frames = fixtures(input, result.map_size)?;
    frame_traces(result, frames, materials)
}

/// The decision traces of [`oracle_traces`] for explicit cameras (the rows
/// [`fixtures`] parses).
pub fn frame_traces(
    result: &mut crate::rebuild::Rebuild,
    frames: Vec<DrawFrame>,
    materials: &crate::texture::MaterialStore,
) -> anyhow::Result<OracleTraces> {
    let heights: Vec<_> = result
        .scene
        .normal
        .iter()
        .map(|g| {
            g.as_ref()
                .map(|g| g.heights.clone())
                .ok_or_else(|| anyhow::anyhow!("missing oracle floor"))
        })
        .collect::<Result<_, _>>()?;
    let scene = result
        .scene_graph
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("draw oracle requires locs"))?;
    let entities = entities(scene);
    let mut trace = Trace::default();
    trace.push(
        "build/entities",
        entities.iter().flat_map(DrawEntity::words).collect(),
    );
    for (case, depths) in [
        [42; 12],
        [
            i32::MAX,
            0,
            -1,
            1,
            i32::MIN,
            0,
            1,
            -1,
            42,
            42,
            i32::MAX,
            i32::MIN,
        ],
    ]
    .iter()
    .enumerate()
    {
        for (name, reverse) in [("opaque", false), ("transparent", true)] {
            let mut entries: Vec<_> = depths
                .iter()
                .enumerate()
                .map(|(id, &depth)| (id as i32, depth))
                .collect();
            sort_bucket(&mut entries, reverse);
            trace.push(
                format!("build/sort/{case}/{name}"),
                entries.iter().flat_map(|&(id, d)| [id, d]).collect(),
            );
        }
    }
    let e2 = frames.iter().any(|view| view.occlusion != 0);
    let mut occlusion = crate::occlusion::Occlusion::new(scene, &heights);
    if e2 {
        trace.push(
            "build/occluders",
            occlusion.quads.iter().flat_map(|q| q.words()).collect(),
        );
        trace.push(
            "build/entityBounds",
            entities
                .iter()
                .flat_map(|e| {
                    let mut v = vec![e.id, e.wall_type, i32::from(e.bounds.is_some())];
                    v.extend(e.bounds.unwrap_or([0; 6]));
                    v
                })
                .collect(),
        );
    }
    let mut state = DrawState::new(32); // Draw distance 4 * 8.
    let mut submissions = Trace::default();
    let mut material_trace = Trace::default();
    let mut material_state = crate::material::MaterialState::default();
    let mut model_lights =
        crate::model_lights::ModelLights::new(scene, &result.env.lights, entities.len());
    let mut live_state = DrawState::new(32);
    let mut live_occlusion = crate::occlusion::Occlusion::new(scene, &heights);
    for view in frames {
        let roof = if view.roof_stamp == -1 {
            None
        } else {
            Some(
                (0..scene.max_level)
                    .map(|l| {
                        (0..scene.max_x)
                            .map(|x| {
                                (0..scene.max_z)
                                    .map(|z| {
                                        if (x * 3 + z * 5 + l) % 11 == 0 {
                                            view.roof_stamp as i8
                                        } else {
                                            (view.roof_stamp - 4) as i8
                                        }
                                    })
                                    .collect()
                            })
                            .collect()
                    })
                    .collect::<Vec<Vec<Vec<i8>>>>(),
            )
        };
        let world = PlannerScene {
            scene,
            heights: &heights,
            entities: &entities,
        };
        trace.0.extend(
            state
                .frame(world, view, roof.as_deref(), &mut occlusion, e2)
                .0,
        );
        // Exercise the production (no tracing) entry point independently.
        let boxes = if view.occlusion == 2 {
            vec![crate::occlusion::Exclusion {
                max_height: i32::MAX,
                min_x: i32::MIN,
                max_x: i32::MAX,
                max_z: i32::MAX,
                min_z: i32::MIN,
            }]
        } else {
            vec![]
        };
        live_state.live_frame(
            world,
            view,
            roof.as_deref(),
            &mut live_occlusion,
            LiveInputs {
                boxes: &boxes,
                cam2: None,
                underwater: None,
            },
        );
        anyhow::ensure!(
            state.plan.opaque == live_state.plan.opaque
                && state.plan.transparent == live_state.plan.transparent
                && state.plan.floors == live_state.plan.floors,
            "live planner diverged"
        );
        let plan = &live_state.plan;
        material_trace.push(
            format!("frame/{}/materials", view.id),
            record_frame(
                &mut material_state,
                &CaptureScene {
                    scene,
                    entities: &entities,
                    plan,
                    floors: &result.scene.normal,
                    lights: &result.lights,
                    materials,
                },
                view.id,
                &mut model_lights,
                [view.eye[0] >> scene.size, view.eye[2] >> scene.size],
            ),
        );
        submissions.push(
            format!("frame/{}/models", view.id),
            plan.opaque
                .iter()
                .chain(&plan.transparent)
                .map(|&id| id as i32)
                .collect(),
        );
        for (level, selection) in plan.floors.iter().enumerate() {
            let g = result.scene.normal[level].as_ref().unwrap();
            // Empty floor builds have no index data and are skipped.
            if g.vertex_count == 0 {
                continue;
            }
            let tiles = selection.tiles(g.tiles_x, g.tiles_z);
            let prefix = format!("frame/{}/floor/{level}/", view.id);
            let mut words = Vec::new();
            for (i, batch) in g.batches.iter().enumerate() {
                let (indices, _, _) = batch.build_indices(g, &tiles);
                words.extend([i as i32, indices.len() as i32]);
                words.extend(indices.into_iter().map(i32::from));
            }
            submissions.push(format!("{prefix}batches"), words);
            let mut words = Vec::new();
            for (i, l) in result.lights[level].iter().enumerate() {
                if !l.indices.is_empty()
                    && light_visible([l.tile_x0, l.tile_x1, l.tile_z0, l.tile_z1], g, selection)
                {
                    words.extend([i as i32, l.indices.len() as i32]);
                    words.extend(l.indices.iter().copied().map(i32::from));
                }
            }
            submissions.push(format!("{prefix}lights"), words);
            let mut words = Vec::new();
            if g.hard_shadows.is_some() {
                for bz in 0..g.tiles_z / 8 {
                    for bx in 0..g.tiles_x / 8 {
                        let indices = shadow_indices(
                            [bx * 8, (bx + 1) * 8, bz * 8, (bz + 1) * 8],
                            g,
                            selection,
                        );
                        if !indices.is_empty() {
                            words.extend([bx as i32, bz as i32, indices.len() as i32]);
                            words.extend(indices.into_iter().map(i32::from));
                        }
                    }
                }
            }
            submissions.push(format!("{prefix}shadows"), words);
        }
    }
    for argument in 0..=255u8 {
        for millis in [0, 1, 999, 64000, 127999, 128000, -1] {
            let p = crate::material::waterfall_parameters(argument, millis);
            let values = [
                if argument & 128 != 0 { p[1] } else { 0. },
                0.,
                if argument & 128 == 0 { p[1] } else { 0. },
                0.,
                0.,
                p[1],
                0.,
                p[2],
                p[3],
            ];
            material_trace.push(
                format!("waterfall/{argument}/{millis}"),
                values.map(|v| v.to_bits() as i32).to_vec(),
            );
        }
    }
    Ok(OracleTraces {
        frames: trace,
        submissions,
        materials: material_trace,
    })
}

/// Six view-frustum planes from a matrix: f32 sums, then f64 normalisation.
fn model_planes(m: [f32; 16]) -> [[f32; 4]; 6] {
    [(2, 1.), (2, -1.), (0, 1.), (0, -1.), (1, 1.), (1, -1.)].map(|(c, s)| {
        let p = [
            m[3] + s * m[c],
            m[7] + s * m[c + 4],
            m[11] + s * m[c + 8],
            m[15] + s * m[c + 12],
        ];
        let length = ((p[2] * p[2] + p[0] * p[0] + p[1] * p[1]) as f64).sqrt();
        p.map(|v| (v as f64 / length) as f32)
    })
}
/// Translated vertical cylinder against the six frustum planes.
pub fn model_visible(cylinder: Option<[i32; 6]>, planes: &[[f32; 4]; 6]) -> bool {
    let Some([x, y, z, y0, y1, r]) = cylinder else {
        return false;
    };
    planes.iter().all(|p| {
        let d = |h: i32| {
            p[2] * z as f32 + p[0] * x as f32 + p[1] * (y as f32 + h as f32) + p[3] + r as f32
        };
        !(d(y0) < 0. && d(y1) < 0.)
    })
}
pub fn entity_model_visible(e: &DrawEntity, planes: &[[f32; 4]; 6]) -> bool {
    if let Some(c) = e.precise_cylinder {
        precise_cylinder_visible(c, planes)
    } else {
        model_visible(e.cylinder, planes)
    }
}
pub fn precise_cylinder_visible(c: [f32; 7], planes: &[[f32; 4]; 6]) -> bool {
    planes.iter().all(|p| {
        let d = |i| p[2] * c[i + 2] + p[0] * c[i] + p[1] * c[i + 1] + p[3] + c[6];
        !(d(0) < 0. && d(3) < 0.)
    })
}

// The draw oracle's per-frame material capture (`oracle_traces`'
// `frame/N/materials`): `material::model` and `material::record_frame`,
// moved here in Phase 2.7 so `material` (GLX material state) could leave for
// rs910-model; these walk `scene`/`draw` and call `floorpass`'s index
// selection.
pub fn model(
    scene: &crate::scene::Scene,
    source: crate::scene::EntityRef,
) -> Option<&crate::gpumodel::GpuModel> {
    use crate::scene::EntityRef::*;
    match source {
        Scenery(i) => scene.scenery[i].model.as_ref(),
        Wall(i) => scene.walls[i].model.as_ref(),
        WallDecor(i) => scene.wall_decors[i].model.as_ref(),
        GroundDecor(i) => scene.ground_decors[i].model.as_ref(),
        Temporary(i) => scene.temporary[i].model.as_ref(),
    }
}

/// What a material capture reads: the scene, the frame's drawable entities
/// and plan, the built floors and baked lights, and the materials.
#[derive(Clone, Copy)]
pub struct CaptureScene<'a> {
    pub scene: &'a crate::scene::Scene,
    pub entities: &'a [crate::draw::DrawEntity],
    pub plan: &'a crate::draw::DrawPlan,
    pub floors: &'a [Option<crate::floor::FloorGeometry>],
    pub lights: &'a [Vec<crate::floorlight::BakedLight>],
    pub materials: &'a crate::texture::MaterialStore,
}

/// Capture logical index ranges alongside the state consumed by a draw.
/// Physical GPU buffers may pack the identical index stream differently.
pub fn record_frame(
    state: &mut MaterialState,
    world: &CaptureScene<'_>,
    frame: i32,
    model_lights: &mut crate::model_lights::ModelLights,
    eye: [i32; 2],
) -> Vec<i32> {
    let CaptureScene {
        scene,
        entities,
        plan,
        floors,
        lights,
        materials,
    } = *world;
    let mut counts = vec![0; entities.len()];
    for &id in &plan.dispatched {
        counts[id] = model_lights.collect(scene, &entities[id], eye).len() as i32;
    }
    let millis = [0, 1, 999, 64000, 127999, 128000, -1][frame.rem_euclid(7) as usize];
    let mut out = Vec::new();
    let mut uv = [1., 1., 0., 0.];
    fn capture(
        out: &mut Vec<i32>,
        state: &MaterialState,
        phase: i32,
        id: i32,
        program: i32,
        range: [i32; 4],
        uv: [f32; 4],
    ) {
        out.extend([phase, id, program, 0]);
        out.extend(range);
        out.extend(state.words());
        out.extend(uv.map(|v| v.to_bits() as i32));
        out.extend([state.exponent.to_bits() as i32, 0]);
        out.extend([0; 32]);
    }
    let models =
        |out: &mut Vec<i32>, state: &mut MaterialState, ids: &[usize], uv: &mut [f32; 4]| {
            for &id in ids {
                let Some(m) = model(scene, entities[id].source) else {
                    continue;
                };
                if m.draw_face_count == 0 {
                    continue;
                }
                state.begin_3d();
                *uv = [1., 1., 0., 0.];
                for (material, start, count, min, span) in m.batches() {
                    let spec = MaterialSpec::new(
                        if material == -1 {
                            None
                        } else {
                            materials.get(material as u16 as u32)
                        },
                        m.detail & 0x37 != 0,
                        true,
                    );
                    let program = state.material(spec);
                    let offset = spec.uv_offset(millis);
                    uv[2] = offset[0];
                    uv[3] = offset[1];
                    capture(
                        out,
                        state,
                        0,
                        id as i32,
                        program,
                        [min, span, start * 3, count],
                        *uv,
                    );
                    let at = out.len() - 56;
                    out[at + 3] = if matches!(program, 0..=2) {
                        counts[id]
                    } else {
                        0
                    };
                    if out[at + 3] > 0 {
                        for (slot, v) in model_lights
                            .parameters(&entities[id])
                            .into_iter()
                            .flatten()
                            .enumerate()
                        {
                            out[at + 24 + slot] = v.to_bits() as i32;
                        }
                    }
                }
            }
        };
    models(&mut out, state, &plan.opaque, &mut uv);
    for (level, selection) in plan.floors.iter().enumerate() {
        let Some(g) = &floors[level] else { continue };
        if g.vertex_count == 0 {
            continue;
        }
        state.begin_3d();
        let tiles = selection.tiles(g.tiles_x, g.tiles_z);
        let mut start = 0;
        for b in &g.batches {
            let (indices, min, max) = b.build_indices(g, &tiles);
            if indices.is_empty() {
                continue;
            }
            let spec = MaterialSpec::new(materials.get(b.material as u32), g.has_normals, false);
            let program = state.material(spec);
            uv = [1. / b.scale, 1. / b.scale, 0., 0.];
            capture(
                &mut out,
                state,
                1,
                level as i32,
                program,
                [min, max - min + 1, start, indices.len() as i32 / 3],
                uv,
            );
            start += indices.len() as i32;
        }
        if !lights[level].is_empty() {
            state.blend_mode(128);
            state.depth_secondary(false);
            for l in &lights[level] {
                if !l.indices.is_empty()
                    && light_visible([l.tile_x0, l.tile_x1, l.tile_z0, l.tile_z1], g, selection)
                {
                    capture(
                        &mut out,
                        state,
                        2,
                        level as i32,
                        0,
                        [0, l.vertex_count() as i32, 0, l.indices.len() as i32 / 3],
                        uv,
                    );
                }
            }
        }
        if g.hard_shadows.is_some() {
            state.blend_mode(1);
            state.depth_primary = false;
            let mut start = 0;
            for bz in 0..g.tiles_z / 8 {
                for bx in 0..g.tiles_x / 8 {
                    let bounds = [bx * 8, (bx + 1) * 8, bz * 8, (bz + 1) * 8];
                    let indices = shadow_indices(bounds, g, selection);
                    if indices.is_empty() {
                        continue;
                    }
                    let mut full = Vec::new();
                    for z in bounds[2]..bounds[3] {
                        for x in bounds[0]..bounds[1] {
                            if let Some(t) = &g.tile_tris[z * g.tiles_x + x] {
                                full.extend(t.iter().copied());
                            }
                        }
                    }
                    let min = *full.iter().min().unwrap() as i32;
                    let max = *full.iter().max().unwrap() as i32;
                    uv = [
                        1. / 4096.,
                        1. / 4096.,
                        (-(bx as i32)) as f32,
                        (-(bz as i32)) as f32,
                    ];
                    capture(
                        &mut out,
                        state,
                        3,
                        level as i32,
                        3,
                        [
                            min,
                            max - min + 1,
                            if selection.whole { 0 } else { start },
                            indices.len() as i32 / 3,
                        ],
                        uv,
                    );
                    start += indices.len() as i32;
                }
            }
            state.depth_primary = true;
        }
    }
    models(&mut out, state, &plan.transparent, &mut uv);
    out
}

// The CPU floor light/shadow visibility over `FloorSelection`
// moved here from `floorpass` (GPU) in Phase 2.8; `floorpass` re-exports them.
/// Partial mode requires a visible floor
/// tile with indices; whole mode accepts any visible tile in the light bounds.
pub fn light_visible(
    bounds: [i32; 4],
    geometry: &FloorGeometry,
    selection: &crate::draw::FloorSelection,
) -> bool {
    let [x0, x1, z0, z1] = bounds;
    for z in z0..=z1 {
        for x in x0..=x1 {
            if selection.visible(x, z)
                && (selection.whole
                    || (x >= 0
                        && z >= 0
                        && (x as usize) < geometry.tiles_x
                        && (z as usize) < geometry.tiles_z
                        && geometry.tile_tris[z as usize * geometry.tiles_x + x as usize]
                            .as_ref()
                            .is_some_and(|t| !t.is_empty())))
            {
                return true;
            }
        }
    }
    false
}

/// Whole mode draws a complete block if
/// any tile is visible; partial mode concatenates visible tiles in Z/X order.
pub fn shadow_indices(
    bounds: [usize; 4],
    geometry: &FloorGeometry,
    selection: &crate::draw::FloorSelection,
) -> Vec<u16> {
    let [x0, x1, z0, z1] = bounds;
    let whole = selection.whole
        && (x0..x1).any(|x| (z0..z1).any(|z| selection.visible(x as i32, z as i32)));
    let mut indices = Vec::new();
    for z in z0..z1 {
        for x in x0..x1 {
            if whole || (!selection.whole && selection.visible(x as i32, z as i32)) {
                if let Some(tris) = &geometry.tile_tris[z * geometry.tiles_x + x] {
                    indices.extend_from_slice(tris);
                }
            }
        }
    }
    indices
}
