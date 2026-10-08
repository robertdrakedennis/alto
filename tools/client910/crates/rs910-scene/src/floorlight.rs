//! Static point lights baked onto the floor: the per-light vertex/index buffers
//! that are appended to the floor and drawn after the floor batches (blend
//! `DST_COLOR, ONE`, depth write off, the `Standard_0PointLights` program,
//! `AmbientColour = (intensity, intensity, intensity)` and the distance fog
//! colour zeroed).
//!
//! The `f32` arithmetic order is kept so the colour bytes are reproducible.

use std::collections::HashMap;

use crate::env::StaticLight;
pub use crate::floor::LightFloor;
use crate::scene::Scene;

/// One baked light: the vertex/index buffers of a single static point light.
#[derive(Clone, Debug, PartialEq)]
pub struct BakedLight {
    /// Index in the scene's static lights; retained even when other lights bake empty.
    pub source_light: Option<usize>,
    /// Tile bounds.
    pub tile_x0: i32,
    pub tile_x1: i32,
    pub tile_z0: i32,
    pub tile_z1: i32,
    /// Vertices of 16 bytes each: `f32 x, y, z` (little-endian) then
    /// `r, g, b, 0xFF`.
    pub vertices: Vec<u8>,
    /// `u16` indices.
    pub indices: Vec<u16>,
    /// The `AmbientColour` of the light pass (1.0).
    pub intensity: f32,
    /// The light's radius.
    pub radius: i32,
}

impl BakedLight {
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / 16
    }
}

/// Bake the static lights of one window: returns, per level, the [`BakedLight`]
/// list in light order. `scene` supplies the first scenery entity whose min
/// tile is the queried tile; `floors[level]` is that level's floor; `size` is
/// the scene size in tiles (9), `half_tile` half a tile in fine units (256).
pub fn build_static_lighting<F: LightFloor>(
    lights: &[StaticLight],
    scene: &Scene,
    floors: &[Option<F>],
    size: i32,
    half_tile: i32,
    max_tile_x: i32,
    max_tile_z: i32,
) -> Vec<Vec<BakedLight>> {
    let mut out: Vec<Vec<BakedLight>> = (0..floors.len()).map(|_| Vec::new()).collect();
    for (source, light) in lights.iter().enumerate() {
        let level = light.level as usize;
        let reach = light.radius - half_tile;
        let grid_width = ((reach * 2) >> size) + 1;
        let mut span_row: i32 = 0;
        let mut tile_shapes = vec![0_i32; (grid_width * grid_width) as usize];
        let min_tile_x = (light.x - reach) >> size;
        let mut min_tile_z = (light.z - reach) >> size;
        let mut max_tile_z_clamped = (light.z + reach) >> size;
        if min_tile_z < 0 {
            span_row -= min_tile_z;
            min_tile_z = 0;
        }
        if max_tile_z_clamped >= max_tile_z {
            max_tile_z_clamped = max_tile_z - 1;
        }
        for tile_z in min_tile_z..=max_tile_z_clamped {
            let span_run = light.span_runs[span_row as usize];
            let run_start = i32::from(span_run >> 8);
            let mut shape_index = grid_width * span_row + run_start;
            let mut run_min_x = run_start + min_tile_x;
            let mut run_max_x = i32::from(span_run & 0xFF) + run_min_x - 1;
            if run_min_x < 0 {
                shape_index -= run_min_x;
                run_min_x = 0;
            }
            if run_max_x >= max_tile_x {
                run_max_x = max_tile_x - 1;
            }
            for tile_x in run_min_x..=run_max_x {
                let mut shape = 1;
                if let Some(diag) = entity_diag(scene, level, tile_x, tile_z) {
                    if diag != 0 {
                        if diag == 1 {
                            let mut left_lit = tile_x > run_min_x;
                            let mut right_lit = tile_x < run_max_x;
                            if !left_lit && tile_z < max_tile_z_clamped {
                                let next_run = light.span_runs[(span_row + 1) as usize];
                                let next_run_min_x = i32::from(next_run >> 8) + min_tile_x;
                                let next_run_max_x = i32::from(next_run & 0xFF) + next_run_min_x;
                                left_lit = tile_x > next_run_min_x && tile_x < next_run_max_x;
                            }
                            if !right_lit && tile_z > min_tile_z {
                                let prev_run = light.span_runs[(span_row - 1) as usize];
                                let prev_run_min_x = i32::from(prev_run >> 8) + min_tile_x;
                                let prev_run_max_x = i32::from(prev_run & 0xFF) + prev_run_min_x;
                                right_lit = tile_x > prev_run_min_x && tile_x < prev_run_max_x;
                            }
                            if left_lit && !right_lit {
                                shape = 4;
                            } else if right_lit && !left_lit {
                                shape = 2;
                            }
                        } else {
                            let mut left_lit_reverse = tile_x > run_min_x;
                            let mut right_lit_reverse = tile_x < run_max_x;
                            if !left_lit_reverse && tile_z > min_tile_z {
                                let previous_row_run = light.span_runs[(span_row - 1) as usize];
                                let previous_row_min_x =
                                    i32::from(previous_row_run >> 8) + min_tile_x;
                                let previous_row_max_x =
                                    i32::from(previous_row_run & 0xFF) + previous_row_min_x;
                                left_lit_reverse =
                                    tile_x > previous_row_min_x && tile_x < previous_row_max_x;
                            }
                            if !right_lit_reverse && tile_z < max_tile_z_clamped {
                                let following_row_run = light.span_runs[(span_row + 1) as usize];
                                let following_row_min_x =
                                    i32::from(following_row_run >> 8) + min_tile_x;
                                let following_row_max_x =
                                    i32::from(following_row_run & 0xFF) + following_row_min_x;
                                right_lit_reverse =
                                    tile_x > following_row_min_x && tile_x < following_row_max_x;
                            }
                            if left_lit_reverse && !right_lit_reverse {
                                shape = 3;
                            } else if right_lit_reverse && !left_lit_reverse {
                                shape = 5;
                            }
                        }
                    }
                }
                tile_shapes[shape_index as usize] = shape;
                shape_index += 1;
            }
            span_row += 1;
        }
        // Baked lighting is always applied.
        if let Some(Some(floor)) = floors.get(level) {
            let mut baked = bake(floor, light, &tile_shapes);
            baked.source_light = Some(source);
            out[level].push(baked);
        }
    }
    out
}

/// The diagonal flag of the first scenery entity whose min tile is `(x, z)`,
/// `None` when the tile or entity is missing.
fn entity_diag(scene: &Scene, level: usize, x: i32, z: i32) -> Option<i8> {
    if x < 0 || z < 0 {
        return None;
    }
    let tile = scene.tile(level, x as usize, z as usize)?;
    tile.entities
        .iter()
        .filter_map(|&r| {
            if let crate::scene::PrimaryRef::Scenery(id) = r {
                Some(&scene.scenery[id])
            } else {
                None
            }
        })
        .find(|e| e.min_tx == x && e.min_tz == z)
        .map(|e| e.diag)
}

/// Bake one light onto its floor window.
fn bake<F: LightFloor>(floor: &F, light: &StaticLight, tile_shapes: &[i32]) -> BakedLight {
    let ts = floor.tile_size();
    let shift = floor.shift();
    let tiles_x = floor.tiles_x();
    let tiles_z = floor.tiles_z();
    let reach = light.radius - (ts >> 1);
    let x0 = (light.x - reach) >> shift;
    let x1 = (light.x + reach) >> shift;
    let z0 = (light.z - reach) >> shift;
    let z1 = (light.z + reach) >> shift;
    let span_x = x1 - x0 + 1;
    let span_z = z1 - z0 + 1;
    let stride = (span_z + 1) as usize;
    let mut normals = vec![[0.0_f32; 3]; ((span_x + 1) * (span_z + 1)) as usize];
    for normal_row in 0..=span_z {
        let normal_tile_z = z0 + normal_row;
        if normal_tile_z > 0 && normal_tile_z < tiles_z - 1 {
            for normal_column in 0..=span_x {
                let normal_tile_x = x0 + normal_column;
                if normal_tile_x > 0 && normal_tile_x < tiles_x - 1 {
                    let height_slope_x = floor.tile_height(normal_tile_x + 1, normal_tile_z)
                        - floor.tile_height(normal_tile_x - 1, normal_tile_z);
                    let height_slope_z = floor.tile_height(normal_tile_x, normal_tile_z + 1)
                        - floor.tile_height(normal_tile_x, normal_tile_z - 1);
                    let len_sq = height_slope_z
                        .wrapping_mul(height_slope_z)
                        .wrapping_add(height_slope_x.wrapping_mul(height_slope_x))
                        .wrapping_add(65536);
                    let normal_inv_length = (1.0_f64 / f64::from(len_sq).sqrt()) as f32;
                    normals[normal_column as usize * stride + normal_row as usize] = [
                        height_slope_x as f32 * normal_inv_length,
                        normal_inv_length * -256.0,
                        height_slope_z as f32 * normal_inv_length,
                    ];
                }
            }
        }
    }
    // Count pass.
    let mut count = 0_i32;
    let mut count_shape_index = 0_usize;
    for count_tile_z in z0..=z1 {
        if count_tile_z >= 0 && count_tile_z < tiles_z {
            for count_tile_x in x0..=x1 {
                if count_tile_x >= 0 && count_tile_x < tiles_x {
                    let count_shape = tile_shapes[count_shape_index];
                    if let Some((_, _, rgb)) = floor.tile_verts(count_tile_x, count_tile_z) {
                        if count_shape != 0 {
                            if count_shape == 1 {
                                for tri in rgb.chunks_exact(3) {
                                    if tri[0] != -1 && tri[1] != -1 && tri[2] != -1 {
                                        count += 3;
                                    }
                                }
                            } else {
                                count += 3;
                            }
                        }
                    }
                }
                count_shape_index += 1;
            }
        } else {
            // Advances one short of a full row here; kept as is.
            count_shape_index += (x1 - x0) as usize;
        }
    }
    let mut baked = BakedLight {
        source_light: None,
        tile_x0: x0,
        tile_x1: x1,
        tile_z0: z0,
        tile_z1: z1,
        vertices: Vec::new(),
        indices: Vec::new(),
        intensity: 1.0,
        radius: light.radius,
    };
    if count <= 0 {
        return baked;
    }
    let mut ctx = Emit {
        floor,
        light,
        normals: &normals,
        stride,
        lookup: HashMap::new(),
        vertices: Vec::with_capacity(count as usize * 16),
        indices: Vec::with_capacity(count as usize),
    };
    // emit_row: row, emit_column: column (counters beside the tile coordinates).
    let mut emit_shape_index = 0_usize; // tile_shapes index
    for (emit_tile_z, emit_row) in (z0..=z1).zip(0_i32..) {
        if emit_tile_z >= 0 && emit_tile_z < tiles_z {
            for (emit_tile_x, emit_column) in (x0..=x1).zip(0_i32..) {
                if emit_tile_x >= 0 && emit_tile_x < tiles_x {
                    let emit_shape = tile_shapes[emit_shape_index];
                    if let Some((xs, zs, rgb)) = floor.tile_verts(emit_tile_x, emit_tile_z) {
                        if emit_shape != 0 {
                            match emit_shape {
                                1 => {
                                    let mut i = 0;
                                    while i < rgb.len() {
                                        if rgb[i] == -1 || rgb[i + 1] == -1 || rgb[i + 2] == -1 {
                                            i += 3;
                                        } else {
                                            ctx.vertex(
                                                emit_column,
                                                emit_row,
                                                emit_tile_x,
                                                emit_tile_z,
                                                xs[i],
                                                zs[i],
                                            );
                                            i += 1;
                                            ctx.vertex(
                                                emit_column,
                                                emit_row,
                                                emit_tile_x,
                                                emit_tile_z,
                                                xs[i],
                                                zs[i],
                                            );
                                            i += 1;
                                            ctx.vertex(
                                                emit_column,
                                                emit_row,
                                                emit_tile_x,
                                                emit_tile_z,
                                                xs[i],
                                                zs[i],
                                            );
                                            i += 1;
                                        }
                                    }
                                }
                                3 => {
                                    ctx.vertex(
                                        emit_column,
                                        emit_row,
                                        emit_tile_x,
                                        emit_tile_z,
                                        0,
                                        0,
                                    );
                                    ctx.vertex(
                                        emit_column,
                                        emit_row,
                                        emit_tile_x,
                                        emit_tile_z,
                                        ts,
                                        0,
                                    );
                                    ctx.vertex(
                                        emit_column,
                                        emit_row,
                                        emit_tile_x,
                                        emit_tile_z,
                                        0,
                                        ts,
                                    );
                                }
                                2 => {
                                    ctx.vertex(
                                        emit_column,
                                        emit_row,
                                        emit_tile_x,
                                        emit_tile_z,
                                        ts,
                                        0,
                                    );
                                    ctx.vertex(
                                        emit_column,
                                        emit_row,
                                        emit_tile_x,
                                        emit_tile_z,
                                        ts,
                                        ts,
                                    );
                                    ctx.vertex(
                                        emit_column,
                                        emit_row,
                                        emit_tile_x,
                                        emit_tile_z,
                                        0,
                                        0,
                                    );
                                }
                                5 => {
                                    ctx.vertex(
                                        emit_column,
                                        emit_row,
                                        emit_tile_x,
                                        emit_tile_z,
                                        ts,
                                        ts,
                                    );
                                    ctx.vertex(
                                        emit_column,
                                        emit_row,
                                        emit_tile_x,
                                        emit_tile_z,
                                        0,
                                        ts,
                                    );
                                    ctx.vertex(
                                        emit_column,
                                        emit_row,
                                        emit_tile_x,
                                        emit_tile_z,
                                        ts,
                                        0,
                                    );
                                }
                                4 => {
                                    ctx.vertex(
                                        emit_column,
                                        emit_row,
                                        emit_tile_x,
                                        emit_tile_z,
                                        0,
                                        ts,
                                    );
                                    ctx.vertex(
                                        emit_column,
                                        emit_row,
                                        emit_tile_x,
                                        emit_tile_z,
                                        0,
                                        0,
                                    );
                                    ctx.vertex(
                                        emit_column,
                                        emit_row,
                                        emit_tile_x,
                                        emit_tile_z,
                                        ts,
                                        ts,
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                }
                emit_shape_index += 1;
            }
        } else {
            emit_shape_index += (x1 - x0) as usize;
        }
    }
    baked.vertices = ctx.vertices;
    baked.indices = ctx.indices;
    baked
}

struct Emit<'a, F: LightFloor> {
    floor: &'a F,
    light: &'a StaticLight,
    normals: &'a [[f32; 3]],
    stride: usize,
    /// Lattice vertex key → index.
    lookup: HashMap<u64, u16>,
    vertices: Vec<u8>,
    indices: Vec<u16>,
}

impl<F: LightFloor> Emit<'_, F> {
    fn normal(&self, col: i32, row: i32) -> [f32; 3] {
        self.normals[col as usize * self.stride + row as usize]
    }

    /// Emit one vertex at a tile-local position, with its grid normal.
    fn vertex(
        &mut self,
        grid_column: i32,
        grid_row: i32,
        tile_x: i32,
        tile_z: i32,
        local_x: i32,
        local_z: i32,
    ) {
        let ts = self.floor.tile_size();
        let world_x = (tile_x << self.floor.shift()) + local_x;
        let world_z = (tile_z << self.floor.shift()) + local_z;
        let world_y = self.floor.fine_height(world_x, world_z);
        let mut key: Option<u64> = None;
        if (local_x & 0x7F) == 0 || (local_z & 0x7F) == 0 {
            let k = ((world_z as i64 & 0xFFFF) << 16 | (world_x as i64 & 0xFFFF)) as u64;
            if let Some(&index) = self.lookup.get(&k) {
                self.indices.push(index);
                return;
            }
            key = Some(k);
        }
        let vertex_index = (self.vertices.len() / 16) as u16;
        if let Some(k) = key {
            self.lookup.insert(k, vertex_index);
        }
        let (normal_x, normal_y, normal_z) = if local_x == 0 && local_z == 0 {
            let n = self.normal(grid_column, grid_row);
            (n[0], n[1], n[2])
        } else if ts == local_x && local_z == 0 {
            let n = self.normal(grid_column + 1, grid_row);
            (n[0], n[1], n[2])
        } else if ts == local_x && ts == local_z {
            let n = self.normal(grid_column + 1, grid_row + 1);
            (n[0], n[1], n[2])
        } else if local_x == 0 && ts == local_z {
            let n = self.normal(grid_column, grid_row + 1);
            (n[0], n[1], n[2])
        } else {
            let frac_x = local_x as f32 / ts as f32;
            let frac_z = local_z as f32 / ts as f32;
            let n00 = self.normal(grid_column, grid_row);
            let n10 = self.normal(grid_column + 1, grid_row);
            let n01 = self.normal(grid_column, grid_row + 1);
            let n11 = self.normal(grid_column + 1, grid_row + 1);
            let (n00_x, n00_y, n00_z) = (n00[0], n00[1], n00[2]);
            let (n10_x, n10_y, n10_z) = (n10[0], n10[1], n10[2]);
            let edge_a_x = (n01[0] - n00_x) * frac_x + n00_x;
            let edge_a_y = (n01[1] - n00_y) * frac_x + n00_y;
            let edge_a_z = (n01[2] - n00_z) * frac_x + n00_z;
            let edge_b_x = (n11[0] - n10_x) * frac_x + n10_x;
            let edge_b_y = (n11[1] - n10_y) * frac_x + n10_y;
            let edge_b_z = (n11[2] - n10_z) * frac_x + n10_z;
            (
                (edge_b_x - edge_a_x) * frac_z + edge_a_x,
                (edge_b_y - edge_a_y) * frac_z + edge_a_y,
                (edge_b_z - edge_a_z) * frac_z + edge_a_z,
            )
        };
        let to_light_x = (self.light.x - world_x) as f32;
        let to_light_y = (self.light.y - world_y) as f32;
        let to_light_z = (self.light.z - world_z) as f32;
        let light_distance =
            f64::from(to_light_z * to_light_z + to_light_x * to_light_x + to_light_y * to_light_y)
                .sqrt() as f32;
        let inv_distance = 1.0_f32 / light_distance;
        let light_dir_x = to_light_x * inv_distance;
        let light_dir_y = to_light_y * inv_distance;
        let light_dir_z = to_light_z * inv_distance;
        let distance_ratio = light_distance / self.light.radius as f32;
        let mut attenuation = 1.0 - distance_ratio * distance_ratio;
        if attenuation < 0.0 {
            attenuation = 0.0;
        }
        let mut facing = normal_z * light_dir_z + normal_x * light_dir_x + normal_y * light_dir_y;
        if facing < 0.0 {
            facing = 0.0;
        }
        let mut intensity = attenuation * facing * 2.0;
        if intensity > 1.0 {
            intensity = 1.0;
        }
        let light_colour = self.light.colour;
        let chan = |c: i32| -> u8 {
            let v = (c as f32 * intensity) as i32;
            if v > 255 {
                255
            } else {
                v as u8
            }
        };
        let r = chan((light_colour >> 16) & 0xFF);
        let g = chan((light_colour >> 8) & 0xFF);
        let b = chan(light_colour & 0xFF);
        self.vertices
            .extend_from_slice(&(world_x as f32).to_le_bytes());
        self.vertices
            .extend_from_slice(&(world_y as f32).to_le_bytes());
        self.vertices
            .extend_from_slice(&(world_z as f32).to_le_bytes());
        self.vertices.extend_from_slice(&[r, g, b, 0xFF]);
        self.indices.push(vertex_index);
    }
}

/// Serialise per-level lists for byte comparison against an externally produced light dump:
/// `"LIT1", i32 count, per light: i32 x0, x1, z0, z1, i32 vertexCount, i32
/// indexCount, u8[vertexCount * 16] vertices (raw), u16[indexCount] (BE)`.
#[must_use]
pub fn to_dump(lights: &[BakedLight]) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(b"LIT1");
    o.extend_from_slice(&(lights.len() as i32).to_be_bytes());
    for l in lights {
        for v in [l.tile_x0, l.tile_x1, l.tile_z0, l.tile_z1] {
            o.extend_from_slice(&v.to_be_bytes());
        }
        o.extend_from_slice(&(l.vertex_count() as i32).to_be_bytes());
        o.extend_from_slice(&(l.indices.len() as i32).to_be_bytes());
        o.extend_from_slice(&l.vertices);
        for i in &l.indices {
            o.extend_from_slice(&i.to_be_bytes());
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat 8x8 floor at height -100 whose every tile is two triangles
    /// with colours.
    struct Flat;

    impl LightFloor for Flat {
        fn tiles_x(&self) -> i32 {
            8
        }
        fn tiles_z(&self) -> i32 {
            8
        }
        fn tile_size(&self) -> i32 {
            512
        }
        fn shift(&self) -> i32 {
            9
        }
        fn tile_height(&self, _x: i32, _z: i32) -> i32 {
            -100
        }
        fn fine_height(&self, _fx: i32, _fz: i32) -> i32 {
            -100
        }
        fn tile_verts(&self, _x: i32, _z: i32) -> Option<(&[i32], &[i32], &[i32])> {
            const XS: [i32; 6] = [0, 512, 0, 512, 512, 0];
            const ZS: [i32; 6] = [0, 0, 512, 0, 512, 512];
            const RGB: [i32; 6] = [1, 1, 1, 1, 1, 1];
            Some((&XS, &ZS, &RGB))
        }
    }

    fn light() -> StaticLight {
        StaticLight {
            level: 0,
            above: false,
            below: false,
            x: 4 * 512 + 256,
            y: -100 - 400,
            z: 4 * 512 + 256,
            radius: (1 << 9) + 256,
            colour: 0xFF8040,
            flicker: 0,
            phase: 0,
            wave: 0,
            offset: 0,
            amplitude: 2048,
            speed: 2048,
            group: -1,
            span_runs: vec![0x0003, 0x0003, 0x0003],
        }
    }

    #[test]
    fn full_span_bakes_every_tile_and_dedupes_lattice_vertices() {
        let scene = Scene::new(9, 1, 8, 8);
        let baked = build_static_lighting(&[light()], &scene, &[Some(Flat)], 9, 256, 8, 8);
        assert_eq!(baked[0].len(), 1);
        let l = &baked[0][0];
        // radius 768 - 256 = 512 → tiles 3..=5 on both axes.
        assert_eq!((l.tile_x0, l.tile_x1, l.tile_z0, l.tile_z1), (3, 5, 3, 5));
        // 9 tiles x 2 triangles x 3 corners = 54 indices; corners on the
        // lattice dedupe to the 16 grid points.
        assert_eq!(l.indices.len(), 54);
        assert_eq!(l.vertex_count(), 16);
        // The vertex under the light is brightest: attenuation 1 - (400/768)^2,
        // n·l with n = (0,-1,0)/|n| and l straight up.
        let idx = l.indices[0] as usize;
        let x = f32::from_le_bytes(l.vertices[idx * 16..idx * 16 + 4].try_into().unwrap());
        assert_eq!(x, 3.0 * 512.0);
        assert_eq!(l.vertices[idx * 16 + 15], 0xFF);
    }
}
