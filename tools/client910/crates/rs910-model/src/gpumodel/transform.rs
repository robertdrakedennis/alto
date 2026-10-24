//! Placement transforms of a lit model: turns, mirror, scale, translate, the
//! replacement transform and the terrain draping of loc models.

use super::GpuModel;
use crate::floor::FloorHeights;
use crate::trig;

/// The ground a model is draped over: the floor heights of its level and,
/// for the two "hang from above" modes, of the level above.
#[derive(Clone, Copy)]
pub struct TerrainHeights<'a> {
    pub floor: &'a FloorHeights,
    pub above: Option<&'a FloorHeights>,
}

impl GpuModel {
    /// Turns the positions about Y by `angle` (0 to 16383, a full turn is
    /// 16384). The normals are left as they are.
    pub fn rotate_y_keep_normals(&mut self, angle: i32) {
        let sine = trig::sin(angle);
        let cosine = trig::cos(angle);
        for i in 0..self.vertex_count as usize {
            let x = self.vz[i]
                .wrapping_mul(sine)
                .wrapping_add(self.vx[i].wrapping_mul(cosine))
                >> 14;
            self.vz[i] = self.vz[i]
                .wrapping_mul(cosine)
                .wrapping_sub(self.vx[i].wrapping_mul(sine))
                >> 14;
            self.vx[i] = x;
        }
        self.bounds_valid = false;
    }

    /// Turns positions and normals about Y.
    pub fn rotate_y(&mut self, angle: i32) {
        let sine = trig::sin(angle);
        let cosine = trig::cos(angle);
        for i in 0..self.vertex_count as usize {
            let x = self.vz[i]
                .wrapping_mul(sine)
                .wrapping_add(self.vx[i].wrapping_mul(cosine))
                >> 14;
            self.vz[i] = self.vz[i]
                .wrapping_mul(cosine)
                .wrapping_sub(self.vx[i].wrapping_mul(sine))
                >> 14;
            self.vx[i] = x;
        }
        for unique in 0..self.unique_count as usize {
            let nx = i32::from(self.nx[unique]);
            let nz = i32::from(self.nz[unique]);
            let turned_x = (nx * cosine + nz * sine) >> 14;
            self.nz[unique] = ((nz * cosine - nx * sine) >> 14) as i16;
            self.nx[unique] = turned_x as i16;
        }
        self.bounds_valid = false;
    }

    /// Turns the positions about X.
    pub fn rotate_x(&mut self, angle: i32) {
        let sine = trig::sin(angle);
        let cosine = trig::cos(angle);
        for i in 0..self.vertex_count as usize {
            let y = self.vy[i]
                .wrapping_mul(cosine)
                .wrapping_sub(self.vz[i].wrapping_mul(sine))
                >> 14;
            self.vz[i] = self.vy[i]
                .wrapping_mul(sine)
                .wrapping_add(self.vz[i].wrapping_mul(cosine))
                >> 14;
            self.vy[i] = y;
        }
        self.bounds_valid = false;
    }

    /// Turns the positions about Z.
    pub fn rotate_z(&mut self, angle: i32) {
        let sine = trig::sin(angle);
        let cosine = trig::cos(angle);
        for i in 0..self.vertex_count as usize {
            let y = self.vy[i]
                .wrapping_mul(sine)
                .wrapping_add(self.vx[i].wrapping_mul(cosine))
                >> 14;
            self.vy[i] = self.vy[i]
                .wrapping_mul(cosine)
                .wrapping_sub(self.vx[i].wrapping_mul(sine))
                >> 14;
            self.vx[i] = y;
        }
        self.bounds_valid = false;
    }

    /// Moves every vertex by `(dx, dy, dz)`.
    pub fn translate(&mut self, dx: i32, dy: i32, dz: i32) {
        for i in 0..self.vertex_count as usize {
            if dx != 0 {
                self.vx[i] += dx;
            }
            if dy != 0 {
                self.vy[i] += dy;
            }
            if dz != 0 {
                self.vz[i] += dz;
            }
        }
        self.bounds_valid = false;
    }

    /// Mirrors the model in Z: positions and normals flip and every face
    /// swaps two corners to keep its winding.
    pub fn mirror(&mut self) {
        for vertex in 0..self.vertex_count as usize {
            self.vz[vertex] = -self.vz[vertex];
        }
        for unique in 0..self.unique_count as usize {
            self.nz[unique] = self.nz[unique].wrapping_neg();
        }
        for face in 0..self.face_count as usize {
            std::mem::swap(&mut self.idx1[face], &mut self.idx3[face]);
        }
        self.bounds_valid = false;
    }

    /// Scales by `(sx, sy, sz) / 128` per axis.
    pub fn scale(&mut self, sx: i32, sy: i32, sz: i32) {
        for i in 0..self.vertex_count as usize {
            if sx != 128 {
                self.vx[i] = self.vx[i].wrapping_mul(sx) >> 7;
            }
            if sy != 128 {
                self.vy[i] = self.vy[i].wrapping_mul(sy) >> 7;
            }
            if sz != 128 {
                self.vz[i] = self.vz[i].wrapping_mul(sz) >> 7;
            }
        }
        self.bounds_valid = false;
    }

    /// Applies a decoded scale, rotation (quaternion) and translation to a
    /// retained replacement model. Cache entries stay untransformed so
    /// several zone requests can share the same base model safely.
    /// `transform` is `[qx, qy, qz, qw, tx, ty, tz, sx, sy, sz]`.
    pub fn apply_srt(&mut self, transform: [f32; 10]) {
        let q = [transform[0], transform[1], transform[2], transform[3]];
        let translation = [transform[4], transform[5], transform[6]];
        let scale = [transform[7], transform[8], transform[9]];
        let rotate = |v: [f32; 3]| {
            let qv = [q[0], q[1], q[2]];
            let cross = |a: [f32; 3], b: [f32; 3]| {
                [
                    a[1] * b[2] - a[2] * b[1],
                    a[2] * b[0] - a[0] * b[2],
                    a[0] * b[1] - a[1] * b[0],
                ]
            };
            let t = cross(qv, v).map(|value| value * 2.0);
            let qt = cross(qv, t);
            [
                v[0] + q[3] * t[0] + qt[0],
                v[1] + q[3] * t[1] + qt[1],
                v[2] + q[3] * t[2] + qt[2],
            ]
        };
        for index in 0..self.vertex_count as usize {
            let rotated = rotate([
                self.vx[index] as f32 * scale[0],
                self.vy[index] as f32 * scale[1],
                self.vz[index] as f32 * scale[2],
            ]);
            self.vx[index] = (rotated[0] + translation[0]).round() as i32;
            self.vy[index] = (rotated[1] + translation[1]).round() as i32;
            self.vz[index] = (rotated[2] + translation[2]).round() as i32;
        }
        for index in 0..self.unique_count as usize {
            let rotated = rotate([
                self.nx[index] as f32,
                self.ny[index] as f32,
                self.nz[index] as f32,
            ]);
            self.nx[index] = rotated[0].round() as i16;
            self.ny[index] = rotated[1].round() as i16;
            self.nz[index] = rotated[2].round() as i16;
        }
        self.bounds_valid = false;
    }

    /// Drapes or tilts the model onto the terrain. The model's origin sits at
    /// `origin = [x, y, z]` in world fine units (`y` is the height of the
    /// ground under it). `mode` is the loc's hill change:
    ///
    /// - 1: every vertex follows the ground height;
    /// - 2: vertices below the `value` fraction of the model's top follow it,
    ///   blending out towards that limit;
    /// - 3: the model tilts to the four corner heights of the footprint packed
    ///   in `value` (width, depth, then the pitch and roll limits);
    /// - 4: every vertex follows the level above, hanging by the model height;
    /// - 5: the model stretches between the floor and the level above, keeping
    ///   `value` clear.
    ///
    /// A model whose footprint leaves the height grid stays as it is, and so
    /// does a model with no vertices (nothing to move) or one the mode cannot
    /// scale: mode 2 with a zero `value` and mode 5 on a model of no height
    /// divide by zero in the original client, which throws before it has
    /// moved a vertex. The arithmetic wraps at 32 bits as it does there.
    pub fn hill_change(
        &mut self,
        mode: i32,
        value: i32,
        terrain: TerrainHeights<'_>,
        origin: [i32; 3],
    ) {
        let TerrainHeights { floor, above } = terrain;
        let [x, y, z] = origin;
        self.ensure_bounds();
        if self.vertex_count == 0 {
            return;
        }
        let world_min_x = self.min_x.wrapping_add(x);
        let world_max_x = self.max_x.wrapping_add(x);
        let world_min_z = self.min_z.wrapping_add(z);
        let world_max_z = self.max_z.wrapping_add(z);
        let outside = |heights: &FloorHeights| {
            world_min_x < 0
                || heights.tile_size.wrapping_add(world_max_x) >> heights.shift
                    >= heights.tiles_x as i32
                || world_min_z < 0
                || heights.tile_size.wrapping_add(world_max_z) >> heights.shift
                    >= heights.tiles_z as i32
        };
        if (mode == 1 || mode == 2 || mode == 3 || mode == 5) && outside(floor) {
            return;
        }
        if mode == 4 || mode == 5 {
            let Some(above) = above else { return };
            if outside(above) {
                return;
            }
        } else {
            let min_tile_x = world_min_x >> floor.shift;
            let max_tile_x = (floor.tile_size - 1).wrapping_add(world_max_x) >> floor.shift;
            let min_tile_z = world_min_z >> floor.shift;
            let max_tile_z = (floor.tile_size - 1).wrapping_add(world_max_z) >> floor.shift;
            // The model is left alone when every corner of its footprint
            // sits at the origin's height. A corner off the grid stops the
            // change, like a footprint that leaves it.
            let mut flat = true;
            for (tile_x, tile_z) in [
                (min_tile_x, min_tile_z),
                (max_tile_x, min_tile_z),
                (min_tile_x, max_tile_z),
                (max_tile_x, max_tile_z),
            ] {
                match floor.tile_height_checked(tile_x, tile_z) {
                    Some(height) if height == y => {}
                    Some(_) => {
                        flat = false;
                        break;
                    }
                    None => return,
                }
            }
            if flat {
                return;
            }
        }
        if mode == 1 {
            for i in 0..self.vertex_count as usize {
                let ground = floor.get_fine_height_clamped(
                    self.vx[i].wrapping_add(x),
                    self.vz[i].wrapping_add(z),
                );
                self.vy[i] = self.vy[i].wrapping_add(ground).wrapping_sub(y);
            }
        } else if mode == 2 {
            let top = self.min_y;
            if top == 0 || value == 0 {
                return;
            }
            for i in 0..self.vertex_count as usize {
                let fraction = (self.vy[i] << 16).wrapping_div(top);
                if fraction < value {
                    let ground = floor.get_fine_height_clamped(
                        self.vx[i].wrapping_add(x),
                        self.vz[i].wrapping_add(z),
                    );
                    let lift = ground
                        .wrapping_sub(y)
                        .wrapping_mul(value.wrapping_sub(fraction))
                        .wrapping_div(value);
                    self.vy[i] = self.vy[i].wrapping_add(lift);
                }
            }
        } else if mode == 3 {
            let width = (value & 0xFF) * 16;
            let depth = ((value >> 8) & 0xFF) * 16;
            let pitch_limit = ((value >> 16) & 0xFF) << 6;
            let roll_limit = ((value >> 24) & 0xFF) << 6;
            if x - (width >> 1) < 0
                || floor.tile_size + (width >> 1) + x >= (floor.tiles_x as i32) << floor.shift
                || z - (depth >> 1) < 0
                || floor.tile_size + (depth >> 1) + z >= (floor.tiles_z as i32) << floor.shift
            {
                return;
            }
            self.tilt_to_corner_heights(floor, origin, [width, depth], [pitch_limit, roll_limit]);
        } else if mode == 4 {
            let above = above.expect("checked");
            let height = self.max_y.wrapping_sub(self.min_y);
            for i in 0..self.vertex_count as usize {
                let ceiling = above.get_fine_height_clamped(
                    self.vx[i].wrapping_add(x),
                    self.vz[i].wrapping_add(z),
                );
                self.vy[i] = self.vy[i]
                    .wrapping_add(ceiling.wrapping_sub(y))
                    .wrapping_add(height);
            }
        } else if mode == 5 {
            let above = above.expect("checked");
            let height = self.max_y.wrapping_sub(self.min_y);
            if height == 0 {
                return;
            }
            for i in 0..self.vertex_count as usize {
                let world_x = self.vx[i].wrapping_add(x);
                let world_z = self.vz[i].wrapping_add(z);
                let ground = floor.get_fine_height_clamped(world_x, world_z);
                let ceiling = above.get_fine_height_clamped(world_x, world_z);
                let span = ground.wrapping_sub(ceiling).wrapping_sub(value);
                let stretched = (self.vy[i] << 8).wrapping_div(height).wrapping_mul(span) >> 8;
                self.vy[i] = stretched.wrapping_sub(y.wrapping_sub(ground));
            }
        }
        self.bounds_valid = false;
    }

    /// Tilts the model to the ground heights at the four corners of its
    /// `extent = [width, depth]` footprint around `origin`: a pitch about X
    /// and a roll about Z (each limited by `limits`, `0` for none), then a
    /// lift so the model rests on the lower diagonal.
    fn tilt_to_corner_heights(
        &mut self,
        heights: &FloorHeights,
        origin: [i32; 3],
        extent: [i32; 2],
        limits: [i32; 2],
    ) {
        let [x, y, z] = origin;
        let [width, depth] = extent;
        let [pitch_limit, roll_limit] = limits;
        let corner = |dx: i32, dz: i32| heights.get_fine_height_clamped(x + dx, z + dz);
        let h00 = corner(-width / 2, -depth / 2);
        let h10 = corner(width / 2, -depth / 2);
        let h01 = corner(-width / 2, depth / 2);
        let h11 = corner(width / 2, depth / 2);
        let low_z_edge = h00.min(h10);
        let high_z_edge = h01.min(h11);
        let high_x_edge = h10.min(h11);
        let low_x_edge = h00.min(h01);
        if depth != 0 {
            let mut pitch = (f64::from(low_z_edge - high_z_edge).atan2(f64::from(depth))
                * 2607.5945876176133) as i32
                & 0x3FFF;
            if pitch != 0 {
                if pitch_limit != 0 {
                    if pitch > 8192 {
                        let floor_limit = 16384 - pitch_limit;
                        if pitch < floor_limit {
                            pitch = floor_limit;
                        }
                    } else if pitch > pitch_limit {
                        pitch = pitch_limit;
                    }
                }
                self.rotate_x(pitch);
            }
        }
        if width != 0 {
            let mut roll = (f64::from(low_x_edge - high_x_edge).atan2(f64::from(width))
                * 2607.5945876176133) as i32
                & 0x3FFF;
            if roll != 0 {
                if roll_limit != 0 {
                    if roll > 8192 {
                        let floor_limit = 16384 - roll_limit;
                        if roll < floor_limit {
                            roll = floor_limit;
                        }
                    } else if roll > roll_limit {
                        roll = roll_limit;
                    }
                }
                self.rotate_z(roll);
            }
        }
        let mut diagonal = h00 + h11;
        if h10 + h01 < diagonal {
            diagonal = h10 + h01;
        }
        let lift = (diagonal >> 1) - y;
        if lift != 0 {
            self.translate(0, lift, 0);
        }
    }

    /// Transforms the vertices of the parts selected by `mask` through
    /// `matrix` (or its inverse). Bounds and normals stay as they were.
    pub fn apply_part_matrix(
        &mut self,
        matrix: &crate::actor_matrix::Matrix,
        mask: i32,
        inverse: bool,
    ) {
        let Some(parts) = &self.vertex_source_models else {
            return;
        };
        let m = if inverse { matrix.inverse() } else { *matrix };
        for (v, &part) in parts[..self.vertex_count as usize].iter().enumerate() {
            if mask & part as i32 != 0 {
                let p = m.point(self.vx[v] as f32, self.vy[v] as f32, self.vz[v] as f32);
                self.vx[v] = p[0] as i32;
                self.vy[v] = p[1] as i32;
                self.vz[v] = p[2] as i32;
            }
        }
    }
}
