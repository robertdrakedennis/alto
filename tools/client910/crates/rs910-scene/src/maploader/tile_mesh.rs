//! Tile shape selection, neighbour blending, vertex emission and occlusion marks.

use crate::floor::{FloorHeights, WaterFogData};

use crate::tileflags::SceneLevelTileFlags;

use super::{
    blend_colours, rotate_point, FloorSources, MapLoader, OcclusionTile, Ov, SweepPath, TileArrays,
    TileTables, Ul, UnderlayTile, WaterFogGrid, BLEND_A, BLEND_B, BLEND_C, BLEND_EDGE_TRI,
    EDGE_MASK_BLEND, EDGE_MASK_NOBLEND, OVERLAY_POINT, OVERLAY_TRIS_BLEND, OVERLAY_TRIS_SIMPLE,
    OVERLAY_TRIS_SPLIT, SIMPLE_A, SIMPLE_B, SIMPLE_C, SPLIT_A, SPLIT_B, SPLIT_C, SPLIT_EDGE_TRI,
    TILE_POINT_X, TILE_POINT_Z, UNDERLAY_POINT, UNDERLAY_TRIS_BLEND, UNDERLAY_TRIS_SIMPLE,
    UNDERLAY_TRIS_SPLIT,
};

impl<'a> MapLoader<'a> {
    /// Pick the diagonal for shape 0/12 by underlay votes,
    /// tie-broken by flatness.
    pub(super) fn resolve_diagonal(
        &self,
        ul_wire: i32,
        cell: [usize; 4],
        heights: &FloorHeights,
        ul_ids: &[i16],
    ) -> i32 {
        let [x, z, x1, z1] = cell;
        if (self.shape_id != 0 && self.shape_id != 12)
            || x == 0
            || z == 0
            || x >= self.max_x
            || z >= self.max_z
        {
            return self.angle;
        }
        let same = |xx: usize, zz: usize| i32::from(ul_ids[self.ti(xx, zz)]) == ul_wire;
        let vote = |b: bool| -> i32 {
            if b {
                1
            } else {
                -1
            }
        };
        let mut vote_sw = vote(same(x - 1, z - 1));
        let mut vote_se = vote(same(x1, z - 1));
        let mut vote_ne = vote(same(x1, z1));
        let mut vote_nw = vote(same(x - 1, z1));
        if same(x, z - 1) {
            vote_sw += 1;
            vote_se += 1;
        } else {
            vote_sw -= 1;
            vote_se -= 1;
        }
        if same(x1, z) {
            vote_se += 1;
            vote_ne += 1;
        } else {
            vote_se -= 1;
            vote_ne -= 1;
        }
        if same(x, z1) {
            vote_ne += 1;
            vote_nw += 1;
        } else {
            vote_ne -= 1;
            vote_nw -= 1;
        }
        if same(x - 1, z) {
            vote_nw += 1;
            vote_sw += 1;
        } else {
            vote_nw -= 1;
            vote_sw -= 1;
        }
        let mut d1 = (vote_sw - vote_ne).abs();
        let mut d2 = (vote_se - vote_nw).abs();
        if d1 == d2 {
            d1 = (heights.get_tile_height(x, z) - heights.get_tile_height(x1, z1)).abs();
            d2 = (heights.get_tile_height(x1, z) - heights.get_tile_height(x, z1)).abs();
        }
        if d1 < d2 {
            1
        } else {
            0
        }
    }
    /// Choose the tile's shape, angle and edge flags from its overlay and underlay.
    pub(super) fn prepare_tile(
        &mut self,
        overlay: Option<Ov>,
        underlay: Option<Ul>,
        tile: [usize; 2],
        tables: &TileTables<'_>,
        edges: &mut [bool; 4],
    ) {
        let mask: &[bool; 4] = match overlay {
            Some(o) if o.blend => &EDGE_MASK_BLEND[self.shape_id as usize],
            _ => &EDGE_MASK_NOBLEND[self.shape_id as usize],
        };
        let _ = underlay;
        self.sample_neighbours(overlay, tile, tables, edges);
        self.use_vertex_colours = overlay.is_some_and(|o| o.averagecolour != o.rgb);
        if !self.use_vertex_colours {
            for i in 0..8 {
                if self.slots.priority[i] >= 0 && self.slots.average[i] != self.slots.rgb[i] {
                    self.use_vertex_colours = true;
                    break;
                }
            }
        }
        let a = self.angle;
        if !mask[((a + 1) & 0x3) as usize] {
            edges[1] |= (self.slots.edges[2] & self.slots.edges[4]) == 0;
        }
        if !mask[((a + 3) & 0x3) as usize] {
            edges[3] |= (self.slots.edges[6] & self.slots.edges[0]) == 0;
        }
        if !mask[(a & 0x3) as usize] {
            edges[0] |= (self.slots.edges[0] & self.slots.edges[2]) == 0;
        }
        if !mask[((a + 2) & 0x3) as usize] {
            edges[2] |= (self.slots.edges[4] & self.slots.edges[6]) == 0;
        }
        if self.blend_overlay || (self.shape_id != 0 && self.shape_id != 12) {
            return;
        }
        if edges[0] && !edges[1] && !edges[2] && edges[3] {
            edges[3] = false;
            edges[0] = false;
            self.shape_id = if self.shape_id == 0 { 13 } else { 14 };
            self.angle = 0;
        } else if edges[0] && edges[1] && !edges[2] && !edges[3] {
            edges[1] = false;
            edges[0] = false;
            self.shape_id = if self.shape_id == 0 { 13 } else { 14 };
            self.angle = 3;
        } else if !edges[0] && edges[1] && edges[2] && !edges[3] {
            edges[2] = false;
            edges[1] = false;
            self.shape_id = if self.shape_id == 0 { 13 } else { 14 };
            self.angle = 2;
        } else if !edges[0] && !edges[1] && edges[2] && edges[3] {
            edges[3] = false;
            edges[2] = false;
            self.shape_id = if self.shape_id == 0 { 13 } else { 14 };
            self.angle = 1;
        }
    }
    /// Select the triangle tables and per-shape counts for the tile's shape.
    pub(super) fn select_shape_tables(&mut self, overlay: Option<Ov>, underlay: Option<Ul>) {
        let s = self.shape_id as usize;
        if self.use_simple_triangles {
            self.vertex_table_a = SIMPLE_A[s];
            self.vertex_table_b = SIMPLE_B[s];
            self.vertex_table_c = SIMPLE_C[s];
            self.overlay_vertex_count = if overlay.is_none() {
                0
            } else {
                OVERLAY_TRIS_SIMPLE[s]
            };
            self.underlay_vertex_count = if underlay.is_none() {
                0
            } else {
                UNDERLAY_TRIS_SIMPLE[s]
            };
        } else if self.blend_overlay {
            self.vertex_table_a = BLEND_A[s];
            self.vertex_table_b = BLEND_B[s];
            self.vertex_table_c = BLEND_C[s];
            self.overlay_vertex_count = if overlay.is_none() {
                0
            } else {
                OVERLAY_TRIS_BLEND[s]
            };
            self.underlay_vertex_count = if underlay.is_none() {
                0
            } else {
                UNDERLAY_TRIS_BLEND[s]
            };
            self.shape_vertex_counts = Some(&BLEND_EDGE_TRI[s]);
        } else {
            self.vertex_table_a = SPLIT_A[s];
            self.vertex_table_b = SPLIT_B[s];
            self.vertex_table_c = SPLIT_C[s];
            self.overlay_vertex_count = if overlay.is_none() {
                0
            } else {
                OVERLAY_TRIS_SPLIT[s]
            };
            self.underlay_vertex_count = if underlay.is_none() {
                0
            } else {
                UNDERLAY_TRIS_SPLIT[s]
            };
            self.shape_vertex_counts = Some(&SPLIT_EDGE_TRI[s]);
        }
    }
    /// The triangle-or-fan selection shared by the overlay and underlay vertex writers.
    /// Fills `triangle_points` and returns how many indices to emit.
    pub(super) fn select_triangle(&mut self, edges: &[bool; 4]) -> usize {
        let idx = self.shape_vertex_index;
        let counts = self.shape_vertex_counts;
        let a = self.vertex_table_a[idx];
        let b = self.vertex_table_b[idx];
        let c = self.vertex_table_c[idx];
        // `shape_vertex_counts` is `None` on the simple path, but then no edge is
        // set and the lookup is never reached.
        let at = |edge: usize| counts.is_some_and(|cs| cs[edge] == idx as i32);
        let mid = if edges[((-self.angle) & 0x3) as usize] && at(0) {
            Some(1)
        } else if edges[((2 - self.angle) & 0x3) as usize] && at(2) {
            Some(5)
        } else if edges[((1 - self.angle) & 0x3) as usize] && at(1) {
            Some(3)
        } else if edges[((3 - self.angle) & 0x3) as usize] && at(3) {
            Some(7)
        } else {
            None
        };
        if let Some(mid) = mid {
            self.triangle_points = [a, mid, c, mid, b, c];
            6
        } else {
            self.triangle_points[0] = a;
            self.triangle_points[1] = b;
            self.triangle_points[2] = c;
            3
        }
    }
    /// Write the overlay part of the tile's vertices into `arrays`.
    pub(super) fn add_overlay_vertices(
        &mut self,
        tile: [usize; 3],
        overlay: Option<Ov>,
        edges: &[bool; 4],
        arrays: &mut TileArrays,
        ground: FloorSources<'_>,
        water_fog: Option<&mut WaterFogGrid>,
    ) {
        let [level, x, z] = tile;
        let FloorSources {
            heights,
            surface: surface_heights,
            underwater: underwater_heights,
        } = ground;
        self.tile_rgb = -1;
        self.tile_material = -1;
        self.material_scale = 256;
        let Some(o) = overlay else {
            if self.use_simple_triangles {
                self.shape_vertex_index += OVERLAY_TRIS_SIMPLE[self.shape_id as usize] as usize;
            } else if self.blend_overlay {
                self.shape_vertex_index += OVERLAY_TRIS_BLEND[self.shape_id as usize] as usize;
            } else {
                self.shape_vertex_index += OVERLAY_TRIS_SPLIT[self.shape_id as usize] as usize;
            }
            return;
        };
        self.tile_rgb = o.rgb;
        self.tile_material = o.material;
        self.material_scale = o.materialscale;
        let average = self.resolve_average_colour(&o);
        let shape = self.shape_id as usize;
        for _ in 0..self.overlay_vertex_count {
            let count = self.select_triangle(edges);
            for k in 0..count {
                let point = self.triangle_points[k];
                let slot = ((point - self.angle * 2) & 0x7) as usize;
                let (rx, rz) = rotate_point(
                    TILE_POINT_X[point as usize],
                    TILE_POINT_Z[point as usize],
                    self.angle,
                );
                let v = self.vertex_count;
                arrays.xs[v] = rx;
                arrays.zs[v] = rz;
                let fx = ((x as i32) << 9) + rx;
                let fz = ((z as i32) << 9) + rz;
                if let (Some(d), Some(surface)) = (arrays.depth.as_mut(), surface_heights) {
                    if OVERLAY_POINT[shape][point as usize] {
                        d[v] = surface.get_fine_height(fx, fz) - heights.get_fine_height(fx, fz);
                    }
                }
                if let Some(off) = arrays.offset.as_mut() {
                    if let (Some(surface), false) =
                        (surface_heights, OVERLAY_POINT[shape][point as usize])
                    {
                        off[v] = heights.get_fine_height(fx, fz) - surface.get_fine_height(fx, fz);
                    } else if let (Some(seabed), false) =
                        (underwater_heights, UNDERLAY_POINT[shape][point as usize])
                    {
                        off[v] = seabed.get_fine_height(fx, fz) - heights.get_fine_height(fx, fz);
                    }
                }
                if point < 8 && self.slots.priority[slot] > o.priority {
                    if let Some(alt) = arrays.alt.as_mut() {
                        alt[v] = self.slots.average[slot];
                    }
                    arrays.scale[v] = self.slots.material_scale[slot];
                    arrays.material[v] = self.slots.material[slot];
                    arrays.rgb[v] = self.slots.rgb[slot];
                } else {
                    if let Some(alt) = arrays.alt.as_mut() {
                        alt[v] = average;
                    }
                    arrays.material[v] = o.material;
                    arrays.scale[v] = o.materialscale;
                    arrays.rgb[v] = self.tile_rgb;
                }
                self.vertex_count += 1;
            }
            self.shape_vertex_index += 1;
        }
        if !self.underwater && level == 0 {
            if let Some(fog) = water_fog {
                fog.set(
                    x,
                    z,
                    WaterFogData {
                        colour: o.waterfogcolour,
                        scale: o.waterfogscale,
                        offset: o.waterfogoffset,
                        reserved: 0,
                        extra_a: o.water_fog_extra_a,
                        extra_b: o.water_fog_extra_b,
                        extra_c: o.water_fog_extra_c,
                    },
                );
            }
        }
        if self.shape_id != 12 && o.rgb != -1 && o.hardshadow {
            self.hard_shadow = true;
        }
    }
    /// Write the underlay part of the tile's vertices into `arrays`.
    pub(super) fn add_underlay_vertices(
        &mut self,
        cell: [usize; 4],
        tile: UnderlayTile,
        edges: &[bool; 4],
        arrays: &mut TileArrays,
        blended: &[i32],
        ground: FloorSources<'_>,
    ) {
        let [x, z, x1, z1] = cell;
        let UnderlayTile {
            underlay,
            here: ul_here,
            north: mut ul_north,
            ne: mut ul_ne,
            east: mut ul_east,
        } = tile;
        let FloorSources {
            heights,
            surface: surface_heights,
            underwater: underwater_heights,
        } = ground;
        let Some(u) = underlay else {
            return;
        };
        if ul_north == 0 {
            ul_north = ul_here;
        }
        if ul_ne == 0 {
            ul_ne = ul_here;
        }
        if ul_east == 0 {
            ul_east = ul_here;
        }
        let u_sw = self.flo.underlay(ul_here - 1);
        let u_nw = self.flo.underlay(ul_north - 1);
        let u_ne = self.flo.underlay(ul_ne - 1);
        let u_se = self.flo.underlay(ul_east - 1);
        let shape = self.shape_id as usize;
        let c_sw = blended[self.ti(x, z)];
        let c_nw = blended[self.ti(x, z1)];
        let c_ne = blended[self.ti(x1, z1)];
        let c_se = blended[self.ti(x1, z)];
        for _ in 0..self.underlay_vertex_count {
            let count = self.select_triangle(edges);
            for k in 0..count {
                let point = self.triangle_points[k];
                let slot = ((point - self.angle * 2) & 0x7) as usize;
                let (rx, rz) = rotate_point(
                    TILE_POINT_X[point as usize],
                    TILE_POINT_Z[point as usize],
                    self.angle,
                );
                let v = self.vertex_count;
                arrays.xs[v] = rx;
                arrays.zs[v] = rz;
                let fx = ((x as i32) << 9) + rx;
                let fz = ((z as i32) << 9) + rz;
                if let (Some(d), Some(surface)) = (arrays.depth.as_mut(), surface_heights) {
                    if OVERLAY_POINT[shape][point as usize] {
                        d[v] = surface.get_fine_height(fx, fz) - heights.get_fine_height(fx, fz);
                    }
                }
                if let Some(off) = arrays.offset.as_mut() {
                    if let (Some(surface), false) =
                        (surface_heights, OVERLAY_POINT[shape][point as usize])
                    {
                        off[v] = heights.get_fine_height(fx, fz) - surface.get_fine_height(fx, fz);
                    } else if let (Some(seabed), false) =
                        (underwater_heights, UNDERLAY_POINT[shape][point as usize])
                    {
                        off[v] = seabed.get_fine_height(fx, fz) - heights.get_fine_height(fx, fz);
                    }
                }
                if point < 8 && self.slots.priority[slot] >= 0 {
                    if let Some(alt) = arrays.alt.as_mut() {
                        alt[v] = self.slots.average[slot];
                    }
                    arrays.scale[v] = self.slots.material_scale[slot];
                    arrays.material[v] = self.slots.material[slot];
                    arrays.rgb[v] = self.slots.rgb[slot];
                } else {
                    if self.blend_overlay && OVERLAY_POINT[shape][point as usize] {
                        arrays.material[v] = self.tile_material;
                        arrays.scale[v] = self.material_scale;
                        arrays.rgb[v] = self.tile_rgb;
                    } else if rx == 0 && rz == 0 {
                        arrays.rgb[v] = c_sw;
                        arrays.material[v] = u_sw.material;
                        arrays.scale[v] = u_sw.materialscale;
                    } else if rx == 0 && rz == 512 {
                        arrays.rgb[v] = c_nw;
                        arrays.material[v] = u_nw.material;
                        arrays.scale[v] = u_nw.materialscale;
                    } else if rx == 512 && rz == 512 {
                        arrays.rgb[v] = c_ne;
                        arrays.material[v] = u_ne.material;
                        arrays.scale[v] = u_ne.materialscale;
                    } else if rx == 512 && rz == 0 {
                        arrays.rgb[v] = c_se;
                        arrays.material[v] = u_se.material;
                        arrays.scale[v] = u_se.materialscale;
                    } else {
                        if rx < 256 {
                            if rz < 256 {
                                arrays.material[v] = u_sw.material;
                                arrays.scale[v] = u_sw.materialscale;
                            } else {
                                arrays.material[v] = u_nw.material;
                                arrays.scale[v] = u_nw.materialscale;
                            }
                        } else if rz < 256 {
                            arrays.material[v] = u_se.material;
                            arrays.scale[v] = u_se.materialscale;
                        } else {
                            arrays.material[v] = u_ne.material;
                            arrays.scale[v] = u_ne.materialscale;
                        }
                        let south = blend_colours(c_sw, c_se, rx << 7 >> 9);
                        let north = blend_colours(c_nw, c_ne, rx << 7 >> 9);
                        arrays.rgb[v] = blend_colours(south, north, rz << 7 >> 9);
                    }
                    if let Some(alt) = arrays.alt.as_mut() {
                        alt[v] = arrays.rgb[v];
                    }
                }
                self.vertex_count += 1;
            }
            self.shape_vertex_index += 1;
        }
        if self.shape_id != 0 && u.hardshadow {
            self.hard_shadow = true;
        }
    }
    /// Mark the tile's occlusion from its underlay and overlay.
    pub(super) fn compute_occlusion(
        &mut self,
        heights: &FloorHeights,
        tile: OcclusionTile,
        level: usize,
        cell: [usize; 4],
        flags: &SceneLevelTileFlags,
    ) {
        let OcclusionTile {
            underlay,
            overlay,
            ul_wire,
            ov_wire,
        } = tile;
        let [x, z, x1, z1] = cell;
        let h00 = heights.get_tile_height(x, z);
        let h10 = heights.get_tile_height(x1, z);
        let h11 = heights.get_tile_height(x1, z1);
        let h01 = heights.get_tile_height(x, z1);
        let link = flags.is_link_below(x as i32, z as i32);
        if (!link || level <= 1) && (link || level == 0) {
            return;
        }
        // Else-if chain.
        #[allow(
            clippy::if_same_then_else,
            reason = "the branches stay separate so each rule reads on its own"
        )]
        let ok = if underlay.is_some_and(|u| !u.occlude) {
            false
        } else if ul_wire == 0 && self.shape_id != 0 {
            false
        } else {
            !(ov_wire > 0 && overlay.is_some_and(|o| !o.occlude))
        };
        if ok && h00 == h10 && h00 == h11 && h00 == h01 {
            let vi = self.vi(x, z);
            self.level_occludemap[level][vi] |= 0x4;
        }
    }
    /// Gather the highest-priority blending overlay around each perimeter point.
    pub(super) fn sample_neighbours(
        &mut self,
        overlay: Option<Ov>,
        tile: [usize; 2],
        tables: &TileTables<'_>,
        edges: &mut [bool; 4],
    ) {
        let [x, z] = tile;
        let TileTables {
            shapes,
            rots,
            ov_ids,
            ..
        } = *tables;
        let mask: &[bool; 4] = match overlay {
            Some(o) if o.blend => &EDGE_MASK_BLEND[self.shape_id as usize],
            _ => &EDGE_MASK_NOBLEND[self.shape_id as usize],
        };
        let max_x = self.max_x;
        let max_z = self.max_z;
        let ov_at = |this: &Self, xx: usize, zz: usize| -> Option<(Ov, usize, i32)> {
            let wire = i32::from(ov_ids[this.ti(xx, zz)]) & 0x7FFF;
            if wire > 0 {
                Some((
                    this.flo.overlay(wire - 1),
                    shapes[this.ti(xx, zz)] as usize,
                    i32::from(rots[this.ti(xx, zz)]),
                ))
            } else {
                None
            }
        };
        // Diagonal neighbours.
        if z > 0 {
            if x > 0 {
                if let Some((o, s, r)) = ov_at(self, x - 1, z - 1) {
                    if o.rgb != -1 && o.blend {
                        let p = ((r * 2 + 4) & 0x7) as usize;
                        let avg = self.resolve_average_colour(&o);
                        if OVERLAY_POINT[s][p] {
                            self.set_slot(0, &o, avg, 256);
                        }
                    }
                }
            }
            if x < max_x - 1 {
                if let Some((o, s, r)) = ov_at(self, x + 1, z - 1) {
                    if o.rgb != -1 && o.blend {
                        let p = ((r * 2 + 6) & 0x7) as usize;
                        let avg = self.resolve_average_colour(&o);
                        if OVERLAY_POINT[s][p] {
                            self.set_slot(2, &o, avg, 512);
                        }
                    }
                }
            }
        }
        if z < max_z - 1 {
            if x > 0 {
                if let Some((o, s, r)) = ov_at(self, x - 1, z + 1) {
                    if o.rgb != -1 && o.blend {
                        let p = ((r * 2 + 2) & 0x7) as usize;
                        let avg = self.resolve_average_colour(&o);
                        if OVERLAY_POINT[s][p] {
                            self.set_slot(6, &o, avg, 64);
                        }
                    }
                }
            }
            if x < max_x - 1 {
                if let Some((o, s, r)) = ov_at(self, x + 1, z + 1) {
                    if o.rgb != -1 && o.blend {
                        let p = ((r * 2) & 0x7) as usize;
                        let avg = self.resolve_average_colour(&o);
                        if OVERLAY_POINT[s][p] {
                            self.set_slot(4, &o, avg, 128);
                        }
                    }
                }
            }
        }
        // Orthogonal neighbours: south, north, west, east.
        let angle = self.angle;
        if z > 0 {
            if let Some((o, s, r)) = ov_at(self, x, z - 1) {
                if o.rgb != -1 {
                    if o.blend {
                        let avg = self.resolve_average_colour(&o);
                        self.sweep(
                            &o,
                            avg,
                            s,
                            0x20,
                            SweepPath {
                                slot: 2,
                                point: r * 2 + 4,
                                slot_step: -1,
                                point_step: 1,
                            },
                        );
                        if !mask[(angle & 0x3) as usize] {
                            edges[0] = EDGE_MASK_BLEND[s][((r + 2) & 0x3) as usize];
                        }
                    } else if !mask[(angle & 0x3) as usize] {
                        edges[0] = EDGE_MASK_NOBLEND[s][((r + 2) & 0x3) as usize];
                    }
                }
            }
        }
        if z < max_z - 1 {
            if let Some((o, s, r)) = ov_at(self, x, z + 1) {
                if o.rgb != -1 {
                    if o.blend {
                        let avg = self.resolve_average_colour(&o);
                        self.sweep(
                            &o,
                            avg,
                            s,
                            0x10,
                            SweepPath {
                                slot: 4,
                                point: r * 2 + 2,
                                slot_step: 1,
                                point_step: -1,
                            },
                        );
                        if !mask[((angle + 2) & 0x3) as usize] {
                            edges[2] = EDGE_MASK_BLEND[s][(r & 0x3) as usize];
                        }
                    } else if !mask[((angle + 2) & 0x3) as usize] {
                        edges[2] = EDGE_MASK_NOBLEND[s][(r & 0x3) as usize];
                    }
                }
            }
        }
        if x > 0 {
            if let Some((o, s, r)) = ov_at(self, x - 1, z) {
                if o.rgb != -1 {
                    if o.blend {
                        let avg = self.resolve_average_colour(&o);
                        self.sweep(
                            &o,
                            avg,
                            s,
                            0x8,
                            SweepPath {
                                slot: 6,
                                point: r * 2 + 4,
                                slot_step: 1,
                                point_step: -1,
                            },
                        );
                        if !mask[((angle + 3) & 0x3) as usize] {
                            edges[3] = EDGE_MASK_BLEND[s][((r + 1) & 0x3) as usize];
                        }
                    } else if !mask[((angle + 3) & 0x3) as usize] {
                        edges[3] = EDGE_MASK_NOBLEND[s][((r + 1) & 0x3) as usize];
                    }
                }
            }
        }
        if x < max_x - 1 {
            if let Some((o, s, r)) = ov_at(self, x + 1, z) {
                if o.rgb != -1 {
                    if o.blend {
                        let avg = self.resolve_average_colour(&o);
                        self.sweep(
                            &o,
                            avg,
                            s,
                            0x4,
                            SweepPath {
                                slot: 4,
                                point: r * 2 + 6,
                                slot_step: -1,
                                point_step: 1,
                            },
                        );
                        if !mask[((angle + 1) & 0x3) as usize] {
                            edges[1] = EDGE_MASK_BLEND[s][((r + 3) & 0x3) as usize];
                        }
                    } else if !mask[((angle + 1) & 0x3) as usize] {
                        edges[1] = EDGE_MASK_NOBLEND[s][((r + 3) & 0x3) as usize];
                    }
                }
            }
        }
        // The tile's own blending overlay.
        let Some(o) = overlay.filter(|o| o.blend) else {
            return;
        };
        let avg = self.resolve_average_colour(&o);
        for point in 0..8 {
            let slot = ((point - self.angle * 2) & 0x7) as usize;
            if OVERLAY_POINT[self.shape_id as usize][point as usize]
                && self.slots.priority[slot] <= o.priority
            {
                self.slots.rgb[slot] = o.rgb;
                self.slots.average[slot] = avg;
                self.slots.material[slot] = o.material;
                self.slots.material_scale[slot] = o.materialscale;
                if o.priority == self.slots.priority[slot] {
                    self.slots.edges[slot] |= 0x2;
                } else {
                    self.slots.edges[slot] = 2;
                }
                self.slots.priority[slot] = o.priority;
            }
        }
    }
    /// Diagonal-neighbour slot write.
    pub(super) fn set_slot(&mut self, slot: usize, o: &Ov, avg: i32, dir: i32) {
        self.slots.rgb[slot] = o.rgb;
        self.slots.average[slot] = avg;
        self.slots.material[slot] = o.material;
        self.slots.material_scale[slot] = o.materialscale;
        self.slots.priority[slot] = o.priority;
        self.slots.edges[slot] = dir;
    }
    /// Orthogonal-neighbour 3-point sweep:
    /// `slot` starts at `slot0` and steps by `slot_step`, the neighbour's
    /// point index starts at `point0` and steps by `point_step`; both are
    /// masked `& 0x7` each iteration.
    pub(super) fn sweep(&mut self, o: &Ov, avg: i32, shape: usize, dir: i32, path: SweepPath) {
        let SweepPath {
            mut slot,
            mut point,
            slot_step,
            point_step,
        } = path;
        for _ in 0..3 {
            point &= 0x7;
            slot &= 0x7;
            let s = slot as usize;
            if OVERLAY_POINT[shape][point as usize] && self.slots.priority[s] <= o.priority {
                self.slots.rgb[s] = o.rgb;
                self.slots.average[s] = avg;
                self.slots.material[s] = o.material;
                self.slots.material_scale[s] = o.materialscale;
                if o.priority == self.slots.priority[s] {
                    self.slots.edges[s] |= dir;
                } else {
                    self.slots.edges[s] = dir;
                }
                self.slots.priority[s] = o.priority;
            }
            point += point_step;
            slot += slot_step;
        }
    }
    /// The overlay's average colour, falling back to its material's or its rgb.
    pub(super) fn resolve_average_colour(&self, o: &Ov) -> i32 {
        if o.averagecolour != -1 {
            return o.averagecolour;
        }
        if o.material != -1 {
            if let Some(m) = self.materials.get(o.material as u32) {
                if !m.high_detail {
                    // The material's average colour is a 16-bit value, widened with sign.
                    return i32::from(m.average_colour as i16);
                }
            }
        }
        o.rgb
    }
}
