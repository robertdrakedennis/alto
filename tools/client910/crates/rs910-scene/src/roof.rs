//! Roof modes, camera ray and stamp flood fill.
//! Inputs are scene-local fine units, except cam2 world coordinates.
//! This produces the roof mask/exclusion boxes consumed by the E1/E2 planner.
use crate::{floor::FloorHeights, scene::Scene, tileflags::SceneLevelTileFlags};

pub struct RoofWorld<'a> {
    pub scene: Option<&'a Scene>,
    pub flags: &'a SceneLevelTileFlags,
    pub heights: &'a [FloorHeights],
}

#[derive(Clone, Debug)]
pub struct RoofInput {
    pub cycle: i32,
    pub level: usize,
    pub camera_state: i32,
    pub camera: [i32; 3],
    pub pitch: i32,
    pub player: [f32; 2],
    pub server: [i32; 2],
    pub base: [i32; 2],
    pub cam2_look: [f32; 2],
    pub cam2_eye: [f32; 2],
}

/// Where a roof flood starts: the level, the tile and the index of the
/// exclusion box it grows.
#[derive(Clone, Copy, Debug)]
pub struct FloodSeed {
    pub level: usize,
    pub tile: [usize; 2],
    pub box_index: usize,
}

pub struct RoofState {
    pub mode: i32,
    pub setup_level: i32,
    pub stamps: Option<Vec<Vec<Vec<i8>>>>,
    /// Exclusion boxes as max height, min x, max x, max z, min z. Preserves the
    /// zero/sentinel states of the legacy behaviour.
    pub boxes: Vec<[i32; 5]>,
    pub hide_roof: bool,
    /// Diagnostic equivalent of the invalid classic-camera ray report;
    /// callers may surface this locally.
    pub invalid_ray: bool,
}

impl Default for RoofState {
    fn default() -> Self {
        // Default roof-removal mode is 2 (the preference default); no level set up yet.
        Self {
            mode: 2,
            setup_level: -1,
            stamps: None,
            boxes: vec![[0; 5]; 2],
            hide_roof: false,
            invalid_ray: false,
        }
    }
}

impl RoofWorld<'_> {
    fn roof(&self, level: usize, x: i32, z: i32) -> bool {
        self.flags.get(level, x as usize, z as usize) & 4 != 0
    }
    /// Floor height at a world position, including bridge sampling.
    fn height(&self, x: i32, z: i32, mut level: usize) -> i32 {
        if self.scene.is_none()
            || x < 0
            || z < 0
            || x >> 9 >= self.flags.size_x() as i32
            || z >> 9 >= self.flags.size_z() as i32
        {
            return 0;
        }
        if level < 3 && self.flags.is_link_below(x >> 9, z >> 9) {
            level += 1;
        }
        self.heights[level].get_fine_height(x, z)
    }
}

impl RoofState {
    /// Applies a roof mode for a level and (re)fills the per-tile stamp array.
    /// A world rebuild supplies a fresh RoofState (the stamp array restarts);
    /// level/preference changes retain this world's array.
    pub fn setup(&mut self, world: &RoofWorld<'_>, mode: i32, level: usize, cycle: i32) {
        self.mode = mode;
        if mode == 0 {
            self.stamps = None;
            self.boxes.clear();
        } else {
            let initial = if mode == 1 || mode == 3 {
                0
            } else {
                cycle.wrapping_sub(4) as i8
            };
            let stamps = self.stamps.get_or_insert_with(|| {
                vec![vec![vec![0; world.flags.size_z()]; world.flags.size_x()]; 4]
            });
            for plane in stamps {
                for column in plane {
                    column.fill(initial);
                }
            }
            self.boxes = vec![[0; 5]; if mode == 1 || mode == 3 { 512 } else { 2 }];
            if (mode == 1 || mode == 3) && world.scene.is_some() {
                let mut id = 0;
                'scan: for x in 0..world.flags.size_x() {
                    for z in 0..world.flags.size_z() {
                        let seed = FloodSeed {
                            level,
                            tile: [x, z],
                            box_index: id,
                        };
                        if self.flood(world, seed, cycle, true) {
                            id += 1;
                        }
                        if id >= 512 {
                            break 'scan;
                        }
                    }
                }
            }
        }
        self.setup_level = level as i32;
    }

    /// Frame branch and output byte.
    pub fn update(&mut self, world: &RoofWorld<'_>, input: &RoofInput) {
        self.invalid_ray = false;
        match self.mode {
            2 => {
                // Per-frame stamp update; level 3 retains old boxes.
                let x = (input.cycle % world.flags.size_x() as i32) as usize;
                for plane in self.stamps.as_mut().unwrap() {
                    plane[x].fill(input.cycle.wrapping_sub(4) as i8);
                }
                if input.level == 3 {
                    return;
                }
                self.boxes.fill([-1000000, 1000000, 0, 0, 1000000]);
                if let Some((id, x, z)) = self.target(world, input) {
                    let seed = FloodSeed {
                        level: input.level,
                        tile: [x as usize, z as usize],
                        box_index: id,
                    };
                    self.flood(world, seed, input.cycle, false);
                }
            }
            3 => {
                // Mode 3: hide the roof while the camera ray finds a target (never on level 3).
                self.hide_roof = false;
                if input.level != 3 {
                    self.hide_roof = self.target(world, input).is_some();
                }
            }
            _ => {}
        }
    }

    pub fn draw_stamp(&self, cycle: i32) -> i8 {
        if self.mode == 2 {
            cycle as i8
        } else if self.mode == 3 && !self.hide_roof {
            -1
        } else {
            1
        }
    }

    /// Shared branch/ray decisions of the mode-3 hide test and the stamp update.
    /// Returns the first flood seed and its exclusion-box index.
    fn target(&mut self, w: &RoofWorld<'_>, a: &RoofInput) -> Option<(usize, i32, i32)> {
        let [cx, cy, cz] = a.camera;
        let level = a.level;
        if a.camera_state != 2 && a.camera_state != 3 && a.server[0] == -1 {
            return (w.height(cx, cz, level).wrapping_sub(cy) < 3200
                && w.roof(level, cx >> 9, cz >> 9))
            .then_some((1, cx >> 9, cz >> 9));
        }
        let (tx, tz) = if a.camera_state == 3 {
            if a.cam2_look[0].is_nan() {
                return None;
            }
            let x = (a.cam2_look[0] as i32).wrapping_sub(a.base[0].wrapping_shl(9));
            let z = (a.cam2_look[1] as i32).wrapping_sub(a.base[1].wrapping_shl(9));
            if x < 0
                || z < 0
                || x >> 9 >= w.flags.size_x() as i32
                || z >> 9 >= w.flags.size_z() as i32
            {
                return None;
            }
            (x >> 9, z >> 9)
        } else if a.camera_state == 2 {
            (a.player[0] as i32 >> 9, a.player[1] as i32 >> 9)
        } else {
            (a.server[0] >> 9, a.server[1] >> 9)
        };
        if w.roof(level, tx, tz) {
            return Some((0, tx, tz));
        }
        let (mut x, mut z) = if a.camera_state == 3 {
            let x = (a.cam2_eye[0] as i32 >> 9).wrapping_sub(a.base[0]);
            let z = (a.cam2_eye[1] as i32 >> 9).wrapping_sub(a.base[1]);
            if x < 0 || z < 0 || x >= w.flags.size_x() as i32 || z >= w.flags.size_z() as i32 {
                return None;
            }
            (x, z)
        } else {
            (cx >> 9, cz >> 9)
        };
        if w.flags.is_roof_of_world(x, z) {
            return (cy >= w.height(cx, cz, 3)).then_some((1, x, z));
        }
        if a.pitch >= 2560 {
            return None;
        }
        let dx = (tx - x).abs();
        let dz = (tz - z).abs();
        if dx == 0 && dz == 0 || dx >= w.flags.size_x() as i32 || dz >= w.flags.size_z() as i32 {
            self.invalid_ray = a.camera_state != 3;
            return None;
        }
        if dx <= dz {
            let step = dx.wrapping_mul(65536) / dz;
            let mut error = 32768;
            while z != tz {
                z += (tz - z).signum();
                for nx in [x, x + 1, x - 1] {
                    if nx >= 0 && nx < w.flags.size_x() as i32 && w.roof(level, nx, z) {
                        return Some((1, nx, z));
                    }
                }
                error += step;
                if error >= 65536 {
                    error -= 65536;
                    let dir = (tx - x).signum();
                    x += dir;
                    let nx = x + dir;
                    if dir != 0 && nx >= 0 && nx < w.flags.size_x() as i32 && w.roof(level, nx, z) {
                        return Some((1, nx, z));
                    }
                }
            }
        } else {
            let step = dz.wrapping_mul(65536) / dx;
            let mut error = 32768;
            while x != tx {
                x += (tx - x).signum();
                for nz in [z, z + 1, z - 1] {
                    if nz >= 0 && nz < w.flags.size_z() as i32 && w.roof(level, x, nz) {
                        return Some((1, x, nz));
                    }
                }
                error += step;
                if error >= 65536 {
                    error -= 65536;
                    let dir = (tz - z).signum();
                    z += dir;
                    let nz = z + dir;
                    if dir != 0 && nz >= 0 && nz < w.flags.size_z() as i32 && w.roof(level, x, nz) {
                        return Some((1, x, nz));
                    }
                }
            }
        }
        None
    }

    /// Under-roof flood fill. Fixed 4096-entry packed-direction ring queue;
    /// marks on enqueue, retaining the legacy neighbour order and footprint quirk.
    pub fn flood(&mut self, w: &RoofWorld<'_>, seed: FloodSeed, cycle: i32, initial: bool) -> bool {
        let FloodSeed {
            level,
            tile: [x, z],
            box_index: id,
        } = seed;
        let stamp = if initial { 1 } else { cycle as i8 };
        let mask = self.stamps.as_mut().unwrap();
        if mask[level][x][z] == stamp || !w.roof(level, x as i32, z as i32) {
            return false;
        }
        let mut qx = [0_i32; 4096];
        let mut qz = [0_i32; 4096];
        qx[0] = x as i32;
        qz[0] = z as i32;
        let (mut read, mut write) = (0, 1);
        mask[level][x][z] = stamp;
        while read != write {
            let x = (qx[read] & 65535) as usize;
            let z = (qz[read] & 65535) as usize;
            let directions = [
                (qx[read] >> 16) & 255,
                (qx[read] >> 24) & 255,
                (qz[read] >> 16) & 255,
            ];
            read = (read + 1) & 4095;
            let boundary = !w.roof(level, x as i32, z as i32);
            let mut marked = false;
            if let Some(scene) = w.scene {
                #[allow(
                    clippy::needless_range_loop,
                    reason = "under-roof plane loop: `plane` also indexes flags and scene tiles"
                )]
                'planes: for plane in level + 1..=3 {
                    if w.flags.get(plane, x, z) & 8 != 0 {
                        continue;
                    }
                    let tile = scene.tile(plane, x, z);
                    if boundary {
                        if let Some(tile) = tile {
                            if let Some(wall) = tile.wall {
                                for (n, &direction) in directions.iter().enumerate() {
                                    if n != 0 && direction == 0 {
                                        continue;
                                    }
                                    let kind = wall_type(direction);
                                    if scene.walls[wall].wall_type == kind
                                        || tile
                                            .dynamic_wall
                                            .is_some_and(|i| scene.walls[i].wall_type == kind)
                                    {
                                        continue 'planes;
                                    }
                                }
                            }
                            for &entity in &tile.entities {
                                let crate::scene::PrimaryRef::Scenery(entity) = entity else {
                                    continue;
                                };
                                let e = &scene.scenery[entity];
                                let shape = if e.shape == 21 { 19 } else { e.shape };
                                let kind = shape | (e.angle << 6);
                                if directions[0] == kind
                                    || directions[1] != 0 && directions[1] == kind
                                    || directions[2] != 0 && directions[2] == kind
                                {
                                    continue 'planes;
                                }
                            }
                        }
                    }
                    if let Some(tile) = tile {
                        for &entity in &tile.entities {
                            let [min_tx, max_tx, min_tz, max_tz] = scene.primary_bounds(entity);
                            if max_tx != min_tx || max_tz != min_tz {
                                let min_x = min_tx.clamp(0, mask[plane].len() as i32 - 1) as usize;
                                let max_x = max_tx.clamp(0, mask[plane].len() as i32 - 1) as usize;
                                let mut nz =
                                    min_tz.clamp(0, mask[plane][0].len() as i32 - 1) as usize;
                                let max_z =
                                    max_tz.clamp(0, mask[plane][0].len() as i32 - 1) as usize;
                                // Nz is deliberately NOT reset.
                                for column in &mut mask[plane][min_x..=max_x] {
                                    while nz <= max_z {
                                        column[nz] = stamp;
                                        nz += 1;
                                    }
                                }
                            }
                        }
                    }
                    mask[plane][x][z] = stamp;
                    marked = true;
                }
            }
            if marked {
                let b = &mut self.boxes[id];
                let h = w.heights[level + 1].get_tile_height(x, z);
                if b[0] < h {
                    b[0] = h;
                }
                let fx = (x as i32) << 9;
                let fz = (z as i32) << 9;
                if b[1] > fx {
                    b[1] = fx;
                } else if b[2] < fx {
                    b[2] = fx;
                }
                if b[4] > fz {
                    b[4] = fz;
                } else if b[3] < fz {
                    b[3] = fz;
                }
            }
            if !boundary {
                let x = x as i32;
                let z = z as i32;
                // x delta, z delta, packed X direction bytes, packed Z byte.
                for (dx, dz, dir_x, dir_z) in [
                    (-1, 0, 0xD3120000_u32, 0x130000),
                    (-1, 1, 0x52120000, 0x130000),
                    (0, 1, 0x13520000, 0x530000),
                    (1, 1, 0x92520000, 0x530000),
                    (1, 0, 0x53920000, 0x930000),
                    (-1, -1, 0x12D20000, 0xD30000),
                    (0, -1, 0x93D20000, 0xD30000),
                    (1, -1, 0xD2920000, 0x930000),
                ] {
                    let nx = x + dx;
                    let nz = z + dz;
                    if nx < 0
                        || nz < 0
                        || nx >= w.flags.size_x() as i32
                        || nz >= w.flags.size_z() as i32
                    {
                        continue;
                    }
                    if mask[level][nx as usize][nz as usize] == stamp {
                        continue;
                    }
                    if dx != 0 && dz != 0 && (w.roof(level, x, nz) || w.roof(level, nx, z)) {
                        continue;
                    }
                    qx[write] = nx | dir_x as i32;
                    qz[write] = nz | dir_z;
                    write = (write + 1) & 4095;
                    mask[level][nx as usize][nz as usize] = stamp;
                }
            }
        }
        let b = &mut self.boxes[id];
        if b[0] != -1000000 {
            b[0] = b[0].wrapping_add(40);
            b[1] = b[1].wrapping_sub(512);
            b[2] = b[2].wrapping_add(512);
            b[3] = b[3].wrapping_add(512);
            b[4] = b[4].wrapping_sub(512);
        }
        true
    }
}

/// Wall-type mask for a packed direction value.
pub fn wall_type(direction: i32) -> i32 {
    match direction & 63 {
        18 => 1 << ((direction >> 6) & 3),
        19 | 21 => 16 << ((direction >> 6) & 3),
        _ => 0,
    }
}
