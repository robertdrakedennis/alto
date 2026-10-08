//! True from a query means OCCLUDED.
//! Static normal-scene construction and per-frame software depth queries.
use crate::camera::CpuProjection;
use crate::floor::FloorHeights;
use crate::occlusion_raster::{DepthRaster, RasterMode, RasterTriangle};
use crate::scene::Scene;

#[derive(Clone, Debug, PartialEq)]
pub struct Occluder {
    pub kind: i32,
    pub level: i32,
    pub tiles: [i32; 4],
    pub x: [i32; 4],
    pub y: [i32; 4],
    pub z: [i32; 4],
    pub projected: [[i16; 3]; 4],
}
impl Occluder {
    pub fn new(kind: i32, level: i32, shift: i32, x: [i32; 4], y: [i32; 4], z: [i32; 4]) -> Self {
        Self {
            kind: kind as i8 as i32,
            level: level as i8 as i32,
            tiles: [
                (x[0] >> shift) as i16 as i32,
                (x[2] >> shift) as i16 as i32,
                (z[0] >> shift) as i16 as i32,
                (z[2] >> shift) as i16 as i32,
            ],
            x,
            y,
            z,
            projected: [[0; 3]; 4],
        }
    }
    pub fn words(&self) -> Vec<i32> {
        let mut v = vec![self.kind, self.level];
        v.extend(self.tiles);
        v.extend(self.x);
        v.extend(self.y);
        v.extend(self.z);
        v
    }
}

/// Snapshot of the level occlude-map tile markers AFTER bridges have shifted
/// the tiles. Diagonals retain their original placement plane/heightmap.
/// Walls coalesce into runs, sweeping z in the outer loop and x in the inner one.
pub fn build_occluders(scene: &Scene, heights: &[FloorHeights]) -> Vec<Occluder> {
    let mut out: Vec<_> = scene
        .occluders
        .iter()
        .map(|r| {
            Occluder::new(
                r.kind,
                r.level,
                scene.size,
                [r.x0, r.x1, r.x1, r.x0],
                [r.y0, r.y1, r.y1, r.y0],
                [r.z0, r.z0, r.z1, r.z1],
            )
        })
        .collect();
    let (nx, nz) = (scene.max_x, scene.max_z);
    // Keep markers separate from immutable scene access, retaining i16 signs.
    let mut marks = vec![[[0_i16; 2]; 2]; scene.max_level * nx * nz];
    let index = |l: usize, x: usize, z: usize| (l * nx + x) * nz + z;
    for l in 0..scene.max_level {
        for x in 0..nx {
            for z in 0..nz {
                if let Some(t) = scene.tile(l, x, z) {
                    for (axis, (h, offset)) in [t.occlude_x, t.occlude_z].into_iter().enumerate() {
                        marks[index(l, x, z)][axis] =
                            [if h > 0 { h.wrapping_neg() } else { h }, offset];
                    }
                }
            }
        }
    }
    for l in 0..scene.max_level {
        for z in 0..nx {
            for x in 0..nz {
                // The outer z sweep is bounded by the x extent and the inner x sweep by the z extent.
                // Client map windows are square; reject unsupported rectangles.
                assert_eq!(
                    nx, nz,
                    "the wall occluder merge assumes square scene windows"
                );
                let bridge = scene.tile(0, x, z).is_some_and(|t| t.bridge.is_some());
                // Axis 0 then 1; the index also selects coordinates below.
                for axis in [0_usize, 1] {
                    let [height, offset] = marks[index(l, x, z)][axis];
                    if height >= 0 {
                        continue;
                    }
                    let (fixed, at, limit) = if axis == 0 { (x, z, nz) } else { (z, x, nx) };
                    let coords = |along: usize| {
                        if axis == 0 {
                            (fixed, along)
                        } else {
                            (along, fixed)
                        }
                    };
                    let base_h = heights[l].get_tile_height(x, z);
                    let mut low = at;
                    let mut high = at;
                    while low > 0 {
                        let (tx, tz) = coords(low - 1);
                        let (px, pz) = coords(low.saturating_sub(2));
                        if marks[index(l, tx, tz)][axis] != [height, offset]
                            || height >= 0
                            || heights[l].get_tile_height(tx, tz) != base_h
                            || (low > 1 && heights[l].get_tile_height(px, pz) != base_h)
                            || high - low > 10
                        {
                            break;
                        }
                        low -= 1;
                    }
                    while high + 1 < limit {
                        let (tx, tz) = coords(high + 1);
                        let (px, pz) = coords(high + 2);
                        if marks[index(l, tx, tz)][axis] != [height, offset]
                            || heights[l].get_tile_height(tx, tz) != base_h
                            || (high + 1 < limit && heights[l].get_tile_height(px, pz) != base_h)
                            || high - low > 10
                        {
                            break;
                        }
                        high += 1;
                    }
                    let hl = l + usize::from(bridge);
                    let (ax, az) = coords(low);
                    let (bx, bz) = coords(high + 1);
                    let (y0, y1) = (
                        heights[hl].get_tile_height(ax, az),
                        heights[hl].get_tile_height(bx, bz),
                    );
                    let f = ((fixed as i32) << scene.size) + i32::from(offset);
                    let a = (low as i32) << scene.size;
                    let b = ((high as i32) << scene.size) + scene.tile_size;
                    let (qx, qz) = if axis == 0 {
                        ([f; 4], [a, b, b, a])
                    } else {
                        ([a, b, b, a], [f; 4])
                    };
                    out.push(Occluder::new(
                        if axis == 0 { 1 } else { 2 },
                        l as i32,
                        scene.size,
                        qx,
                        [y0, y1, y1 + i32::from(height), y0 + i32::from(height)],
                        qz,
                    ));
                    for along in low..=high {
                        let (tx, tz) = coords(along);
                        marks[index(l, tx, tz)][axis][0] = height.wrapping_neg();
                    }
                }
            }
        }
    }
    for call in &scene.occlude_calls {
        if call.kind != 8 && call.kind != 16 {
            continue;
        }
        out.push(diagonal_occluder(scene, heights, call));
    }
    out
}

/// Occlude-map kinds 8/16: the
/// one-tile diagonal quad appended to the scene's diagonal calls.
fn diagonal_occluder(
    scene: &Scene,
    heights: &[FloorHeights],
    call: &crate::scene::OccludeMapCall,
) -> Occluder {
    let (x, z) = (call.x as usize, call.z as usize);
    let h = &heights[call.level as usize];
    let (x0, x1, z0, z1) = (
        (x as i32) << scene.size,
        ((x + 1) as i32) << scene.size,
        (z as i32) << scene.size,
        ((z + 1) as i32) << scene.size,
    );
    let (qx, y0, y1) = if call.kind == 8 {
        (
            [x0, x1, x1, x0],
            h.get_tile_height(x, z),
            h.get_tile_height(x + 1, z + 1),
        )
    } else {
        (
            [x1, x0, x0, x1],
            h.get_tile_height(x + 1, z),
            h.get_tile_height(x, z + 1),
        )
    };
    Occluder::new(
        call.kind,
        call.level,
        scene.size,
        qx,
        [y0, y1, y1 - call.height, y0 - call.height],
        [z0, z1, z1, z0],
    )
}

/// What the frame's occlusion pass reads of the camera and the draw planner:
/// the projection, the eye, the raster surface size, the draw distance in
/// tiles, which tiles are inside the view, the roof level and the roof
/// exclusion boxes.
#[derive(Clone, Copy)]
pub struct OcclusionView<'a> {
    pub cpu: &'a CpuProjection,
    pub eye: [i32; 3],
    pub surface: [i32; 2],
    pub distance: i32,
    pub visibility: &'a [Vec<bool>],
    pub roof_level: i32,
    pub boxes: &'a [Exclusion],
}

/// A wall face test: the tile it stands on, which sides it closes (`kind`,
/// one of the wall type bits) and its height.
#[derive(Clone, Copy, Debug)]
pub struct WallQuery {
    pub level: usize,
    pub tile: [usize; 2],
    pub kind: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Exclusion {
    pub max_height: i32,
    pub min_x: i32,
    pub max_x: i32,
    pub max_z: i32,
    pub min_z: i32,
}

pub struct Occlusion {
    pub quads: Vec<Occluder>,
    pub active: Vec<usize>,
    pub raster: DepthRaster,
    pub cycles: Vec<Vec<Vec<i32>>>,
    pub enabled: bool,
    /// The manager-wide switch every query tests together with the per-frame flag;
    /// `EXECUTE_CLIENT_CHEAT` 24 toggles it.
    pub manager_enabled: bool,
    pub record: bool,
    pub triangles: Vec<i32>,
    pub tile_queries: Vec<i32>,
    eye: [i32; 3],
    shift: i32,
    size: i32,
    cycle: i32,
}
impl Occlusion {
    pub fn new(scene: &Scene, heights: &[FloorHeights]) -> Self {
        let mut raster = DepthRaster::new(0, 0);
        // Dimensions uninitialized until the
        // first enabled frame; the depth buffer is still absent/empty.
        raster.width = -1;
        raster.height = -1;
        Self {
            quads: build_occluders(scene, heights),
            active: vec![],
            raster,
            cycles: vec![vec![vec![0; scene.max_z + 1]; scene.max_x + 1]; scene.max_level],
            enabled: false,
            manager_enabled: true,
            record: true,
            triangles: vec![],
            tile_queries: vec![],
            eye: [0; 3],
            shift: scene.size,
            size: scene.tile_size,
            cycle: 0,
        }
    }
    /// Registers an occluder marker once the manager is built, as a runtime location
    /// replacement does. Kinds 1/2 store the tile marker and rebuild the wall quads;
    /// kinds 8/16 append a diagonal quad. The scene keeps the call so later rebuilds agree.
    pub fn set_level_occlude_map(
        &mut self,
        scene: &mut Scene,
        heights: &[FloorHeights],
        call: crate::scene::OccludeMapCall,
    ) {
        if call.kind != 8 && call.kind != 16 {
            scene.set_occlude_marker(
                call.kind,
                call.level as usize,
                call.x as usize,
                call.z as usize,
                call.height,
                call.offset,
            );
        }
        scene.occlude_calls.push(call);
        self.rebuild(scene, heights);
    }

    /// Removes an occluder when a location is removed. Kinds
    /// 1/2 zero the tile's wall height (both axes, keeping the
    /// offset) and rebuild the coalesced wall quads; kinds
    /// 8/16 drop the first diagonal quad of that kind and level whose
    /// min tile or max tile is `(x, z)`.
    pub fn remove_occluder(
        &mut self,
        scene: &mut Scene,
        heights: &[FloorHeights],
        kind: i32,
        level: i32,
        x: i32,
        z: i32,
    ) {
        if kind != 8 && kind != 16 {
            let (lx, tx, tz) = (level as usize, x as usize, z as usize);
            if let Some(tile) = scene.tile(lx, tx, tz) {
                // Only the height is zeroed; the tile exists, so the marker
                // setter creates nothing.
                let offset = if kind == 1 {
                    tile.occlude_x.1
                } else {
                    tile.occlude_z.1
                };
                scene.set_occlude_marker(kind, lx, tx, tz, 0, offset.into());
            }
            self.rebuild(scene, heights);
            return;
        }
        // The scene's occlude calls include exactly the kind 8/16 diagonals, in call order.
        let found = scene.occlude_calls.iter().position(|call| {
            if call.kind != 8 && call.kind != 16 {
                return false;
            }
            let q = diagonal_occluder(scene, heights, call);
            q.kind == kind
                && q.level == level
                && (q.tiles[0] == x && q.tiles[2] == z || q.tiles[1] == x && q.tiles[3] == z)
        });
        if let Some(i) = found {
            scene.occlude_calls.remove(i);
            self.rebuild(scene, heights);
        }
    }

    /// Re-derive the quad lists from the scene's markers and calls
    /// (walls are coalesced into runs; the level and diagonal lists are the
    /// scene's own). Selection is per frame, so `active` restarts empty.
    fn rebuild(&mut self, scene: &Scene, heights: &[FloorHeights]) {
        self.quads = build_occluders(scene, heights);
        self.active.clear();
    }

    /// Select this frame's occluder quads and rasterise them into the depth
    /// raster (each quad as the triangles 0-1-3 and 1-2-3). `enabled` is the
    /// per-frame switch; the manager-wide switch must also be on.
    pub fn prepare(&mut self, view: &OcclusionView<'_>, enabled: bool) {
        let OcclusionView {
            cpu,
            eye,
            surface,
            distance,
            visibility,
            roof_level,
            boxes,
        } = *view;
        self.eye = eye;
        self.enabled = enabled && self.manager_enabled;
        self.triangles.clear();
        self.tile_queries.clear();
        if !self.enabled {
            return;
        } // A disabled manager bypasses the tile queries entirely.
        let (width, height) = (
            (surface[0] as f32 / 3.0) as i32,
            (surface[1] as f32 / 3.0) as i32,
        );
        if width != self.raster.width || height != self.raster.height {
            let mode = self.raster.mode;
            self.raster = DepthRaster::new(width, height);
            self.raster.mode = mode;
        }
        self.active.clear();
        let (ex, ez) = (eye[0] >> self.shift, eye[2] >> self.shift);
        let max = distance * 2;
        for (id, q) in self.quads.iter_mut().enumerate() {
            if q.level >= roof_level
                && boxes.iter().any(|b| {
                    b.max_height != -1000000
                        && q.y.iter().any(|&v| v <= b.max_height)
                        && q.x.iter().any(|&v| v <= b.max_x)
                        && q.x.iter().any(|&v| v >= b.min_x)
                        && q.z.iter().any(|&v| v <= b.max_z)
                        && q.z.iter().any(|&v| v >= b.min_z)
                })
            {
                continue;
            }
            let [tx0, tx1, tz0, tz1] = q.tiles;
            let (x0, x1, z0, z1) = (
                distance + tx0 - ex,
                distance + tx1 - ex,
                distance + tz0 - ez,
                distance + tz1 - ez,
            );
            let dx = (eye[0].wrapping_sub(q.x[0]) as f32).abs();
            let dz = (eye[2].wrapping_sub(q.z[0]) as f32).abs();
            let selected = match q.kind {
                1 => {
                    x0 >= 0
                        && x0 <= max
                        && z0 <= max
                        && z1 >= 0
                        && dx >= self.size as f32
                        && (z0.max(0)..=z1.min(max)).any(|z| visibility[x0 as usize][z as usize])
                }
                2 => {
                    z0 >= 0
                        && z0 <= max
                        && x0 <= max
                        && x1 >= 0
                        && dz >= self.size as f32
                        && (x0.max(0)..=x1.min(max)).any(|x| visibility[x as usize][z0 as usize])
                }
                8 | 16 => {
                    x0 >= 0
                        && x0 <= max
                        && z0 >= 0
                        && z0 <= max
                        && visibility[x0 as usize][z0 as usize]
                        && (dx >= self.size as f32 || dz >= self.size as f32)
                }
                4 => {
                    (q.y[0].wrapping_sub(eye[1]) as f32) > self.size as f32
                        && x0 <= max
                        && x1 >= 0
                        && z0 <= max
                        && z1 >= 0
                        && (x0.max(0)..=x1.min(max)).any(|x| {
                            (z0.max(0)..=z1.min(max)).any(|z| visibility[x as usize][z as usize])
                        })
                }
                _ => false,
            };
            if !selected {
                continue;
            }
            let mut projected = true;
            for v in 0..4 {
                let p = cpu.project([q.x[v] as f32, q.y[v] as f32, q.z[v] as f32]);
                if p[2] < 50.0 {
                    projected = false;
                    break;
                }
                // The legacy conversion goes float -> int -> short (wrapping), not Rust's
                // direct saturating float-to-i16 cast.
                q.projected[v] = [
                    (p[0] / 3.0) as i32 as i16,
                    (p[1] / 3.0) as i32 as i16,
                    p[2] as i32 as i16,
                ];
            }
            if projected {
                self.active.push(id);
            }
        }
        self.raster.coverage = 0;
        if !self.active.is_empty() {
            self.raster.depth.fill(i32::MAX);
            self.raster.mode = RasterMode::Write;
            for id in self.active.clone() {
                let p = self.quads[id].projected;
                for [a, b, c] in [[0, 1, 3], [1, 2, 3]] {
                    self.triangle([
                        p[a][1] as i32,
                        p[b][1] as i32,
                        p[c][1] as i32,
                        p[a][0] as i32,
                        p[b][0] as i32,
                        p[c][0] as i32,
                        p[a][2] as i32,
                        p[b][2] as i32,
                        p[c][2] as i32,
                    ]);
                }
            }
            self.raster.mode = RasterMode::Test;
        }
    }
    pub fn set_cycle(&mut self, cycle: i32) {
        self.cycle = cycle;
    }
    fn triangle(&mut self, a: [i32; 9]) -> bool {
        let result = self.raster.triangle(RasterTriangle::from_words(a));
        if self.record {
            self.triangles.push(self.raster.mode.code());
            self.triangles.extend(a);
            self.triangles.push(result as i32);
        }
        result
    }
    /// Whether the projected triangle is occluded.
    fn test_triangle(&mut self, cpu: &CpuProjection, points: [[i32; 3]; 3]) -> bool {
        let mut p = [[0_i32; 3]; 3];
        for i in 0..3 {
            let v = cpu.project(points[i].map(|v| v as f32));
            if v[2] < 50.0 {
                return false;
            }
            p[i] = [(v[0] / 3.0) as i32, (v[1] / 3.0) as i32, v[2] as i32];
        }
        self.triangle([
            p[0][1], p[1][1], p[2][1], p[0][0], p[1][0], p[2][0], p[0][2], p[1][2], p[2][2],
        ])
    }
    fn ready(&self) -> bool {
        self.enabled && self.raster.coverage >= 101
    }
    /// Whether a tile is occluded, including signed cycle memoization.
    pub fn tile_occluded(
        &mut self,
        cpu: &CpuProjection,
        heights: &[FloorHeights],
        l: usize,
        x: usize,
        z: usize,
    ) -> bool {
        let result = self.tile_inner(cpu, heights, l, x, z);
        if self.record {
            self.tile_queries
                .extend([l as i32, x as i32, z as i32, result as i32]);
        }
        result
    }
    fn tile_inner(
        &mut self,
        cpu: &CpuProjection,
        heights: &[FloorHeights],
        l: usize,
        x: usize,
        z: usize,
    ) -> bool {
        if !self.ready() {
            return false;
        }
        let stamp = self.cycles[l][x][z];
        if stamp == self.cycle.wrapping_neg() {
            return false;
        }
        if stamp == self.cycle {
            return true;
        }
        let (fx, fz) = ((x as i32) << self.shift, (z as i32) << self.shift);
        let h = &heights[l];
        let a = [fx + 1, h.get_tile_height(x, z), fz + 1];
        let b = [
            fx + self.size - 1,
            h.get_tile_height(x + 1, z + 1),
            fz + self.size - 1,
        ];
        let result = self.test_triangle(
            cpu,
            [
                a,
                b,
                [fx + 1, h.get_tile_height(x, z + 1), fz + self.size - 1],
            ],
        ) && self.test_triangle(
            cpu,
            [
                a,
                [fx + self.size - 1, h.get_tile_height(x + 1, z), fz + 1],
                b,
            ],
        );
        self.cycles[l][x][z] = if result {
            self.cycle
        } else {
            self.cycle.wrapping_neg()
        };
        result
    }
    /// occluded box, arguments are min xyz and extents, even
    /// when the wall-corner caller supplies a negative y extent.
    pub fn bounds_occluded(&mut self, cpu: &CpuProjection, [x, y, z, w, h, d]: [i32; 6]) -> bool {
        let (xx, yy, zz) = (x.wrapping_add(w), y.wrapping_add(h), z.wrapping_add(d));
        if !self.test_triangle(cpu, [[x, yy, z], [xx, yy, zz], [x, yy, zz]])
            || !self.test_triangle(cpu, [[x, yy, z], [xx, yy, z], [xx, yy, zz]])
        {
            return false;
        }
        let sx = if x < self.eye[0] { x } else { xx };
        if !self.test_triangle(cpu, [[sx, y, zz], [sx, yy, z], [sx, yy, zz]])
            || !self.test_triangle(cpu, [[sx, y, zz], [sx, y, z], [sx, yy, z]])
        {
            return false;
        }
        let sz = if z < self.eye[2] { z } else { zz };
        self.test_triangle(cpu, [[x, y, sz], [xx, yy, sz], [x, yy, sz]])
            && self.test_triangle(cpu, [[x, y, sz], [xx, y, sz], [xx, yy, sz]])
    }
    /// visible, wall decoration box after the tile test.
    pub fn decor_occluded(
        &mut self,
        cpu: &CpuProjection,
        heights: &[FloorHeights],
        l: usize,
        x: usize,
        z: usize,
        height: i32,
    ) -> bool {
        self.ready()
            && self.tile_occluded(cpu, heights, l, x, z)
            && self.bounds_occluded(
                cpu,
                [
                    (x as i32) << self.shift,
                    heights[l].get_tile_height(x, z),
                    (z as i32) << self.shift,
                    self.size,
                    height,
                    self.size,
                ],
            )
    }
    /// Whether a location is occluded; multi-tile negative cache probes are not
    /// fresh tile queries and must not populate the cache.
    pub fn loc_occluded(
        &mut self,
        cpu: &CpuProjection,
        heights: &[FloorHeights],
        l: usize,
        [x0, x1, z0, z1]: [i32; 4],
        bounds: Option<[i32; 6]>,
    ) -> bool {
        if !self.ready()
            || x0 < 0
            || z0 < 0
            || x1 >= self.cycles[l].len() as i32
            || z1 >= self.cycles[l][x0 as usize].len() as i32
        {
            return false;
        }
        if x0 != x1 || z0 != z1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if self.cycles[l][x as usize][z as usize] == self.cycle.wrapping_neg() {
                        return false;
                    }
                }
            }
        } else if !self.tile_occluded(cpu, heights, l, x0 as usize, z0 as usize) {
            return false;
        }
        bounds.is_some_and(|b| self.bounds_occluded(cpu, b))
    }
    /// Whether a wall face is hidden. The vertex order of the tested
    /// triangles depends on the side the wall closes and is kept as is.
    pub fn wall_occluded(
        &mut self,
        cpu: &CpuProjection,
        heights: &[FloorHeights],
        wall: WallQuery,
    ) -> bool {
        let WallQuery {
            level: l,
            tile: [tx, tz],
            kind,
            height,
        } = wall;
        if !self.ready() || !self.tile_occluded(cpu, heights, l, tx, tz) {
            return false;
        }
        let (x, z) = ((tx as i32) << self.shift, (tz as i32) << self.shift);
        let y = heights[l].get_tile_height(tx, tz) - 1;
        let yy = y + height;
        let s = self.size;
        let triangles = match kind {
            1 => [
                [[x, y, z], [x, yy, z], [x, yy, z + s]],
                [[x, y, z], [x, yy, z + s], [x, y, z + s]],
            ],
            2 => [
                [[x, y, z + s], [x + s, yy, z + s], [x, yy, z + s]],
                [[x, y, z + s], [x + s, y, z + s], [x + s, yy, z + s]],
            ],
            4 => [
                [[x + s, y, z], [x + s, yy, z], [x + s, yy, z + s]],
                [[x + s, y, z], [x + s, yy, z + s], [x + s, y, z + s]],
            ],
            8 => [
                [[x, y, z], [x + s, yy, z], [x, yy, z]],
                [[x, y, z], [x + s, y, z], [x + s, yy, z]],
            ],
            16 | 32 | 64 | 128 => {
                let half = s >> 1;
                return self.bounds_occluded(
                    cpu,
                    [
                        x + if kind == 32 || kind == 64 { half } else { 0 },
                        y,
                        z + if kind == 16 || kind == 32 { half } else { 0 },
                        half,
                        yy,
                        half,
                    ],
                );
            }
            _ => return true,
        };
        self.test_triangle(cpu, triangles[0]) && self.test_triangle(cpu, triangles[1])
    }
}

#[cfg(test)]
mod loc_change_tests {
    use super::*;
    use crate::scene::OccludeMapCall;

    fn setup() -> (Scene, Vec<FloorHeights>) {
        let n = 12;
        let scene = Scene::new(9, 4, n, n);
        let heights = vec![FloorHeights::new(n, n, 512, vec![0; (n + 1) * (n + 1)]); 4];
        (scene, heights)
    }

    fn call(kind: i32, x: i32, z: i32) -> OccludeMapCall {
        OccludeMapCall {
            kind,
            level: 0,
            x,
            z,
            height: 240,
            offset: 16,
        }
    }

    /// A three-tile kind-1 wall run coalesces into one quad; `remove_occluder`
    /// on its middle tile splits it, and on every tile clears it. The
    /// cleared markers keep their offsets.
    #[test]
    fn wall_marker_removal_rebuilds_runs() {
        let (mut scene, heights) = setup();
        for z in 3..6 {
            scene.create_tile(0, 4, z);
        }
        let mut occ = Occlusion::new(&scene, &heights);
        for z in 3..6 {
            occ.set_level_occlude_map(&mut scene, &heights, call(1, 4, z));
        }
        let walls = |o: &Occlusion| o.quads.iter().filter(|q| q.kind == 1).count();
        assert_eq!(walls(&occ), 1);
        assert_eq!(occ.quads[0].tiles, [4, 4, 3, 6]);
        occ.remove_occluder(&mut scene, &heights, 1, 0, 4, 4);
        assert_eq!(scene.tile(0, 4, 4).unwrap().occlude_x, (0, 16));
        assert_eq!(walls(&occ), 2);
        occ.remove_occluder(&mut scene, &heights, 1, 0, 4, 3);
        occ.remove_occluder(&mut scene, &heights, 1, 0, 4, 5);
        assert_eq!(walls(&occ), 0);
        assert_eq!(occ.quads, build_occluders(&scene, &heights));
        // No tile: nothing to clear, the rebuild still runs.
        occ.remove_occluder(&mut scene, &heights, 2, 0, 9, 9);
        assert!(occ.quads.is_empty());
    }

    /// Diagonals: kind 8 at (x, z) spans min (x, z) / max (x + 1, z + 1),
    /// so removal matches it at either corner; kind 16 is mirrored in x
    /// (min (x + 1, z), max (x, z + 1)) and, as in the legacy behaviour, does not match
    /// its own tile. Only the first match goes.
    #[test]
    fn diagonal_removal_matches_either_corner() {
        let (mut scene, heights) = setup();
        let mut occ = Occlusion::new(&scene, &heights);
        occ.set_level_occlude_map(&mut scene, &heights, call(8, 2, 2));
        occ.set_level_occlude_map(&mut scene, &heights, call(8, 2, 2));
        occ.set_level_occlude_map(&mut scene, &heights, call(16, 6, 6));
        assert_eq!(occ.quads.len(), 3);
        occ.remove_occluder(&mut scene, &heights, 8, 0, 3, 3);
        assert_eq!(occ.quads.len(), 2);
        occ.remove_occluder(&mut scene, &heights, 8, 1, 2, 2); // other level
        occ.remove_occluder(&mut scene, &heights, 16, 0, 2, 2); // other kind
        occ.remove_occluder(&mut scene, &heights, 16, 0, 6, 6); // own tile
        assert_eq!(occ.quads.len(), 2);
        occ.remove_occluder(&mut scene, &heights, 16, 0, 7, 6);
        occ.remove_occluder(&mut scene, &heights, 8, 0, 2, 2);
        assert!(occ.quads.is_empty());
        assert!(scene.occlude_calls.is_empty());
    }
}
