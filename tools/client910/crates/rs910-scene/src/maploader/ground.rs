//! Level floor allocation, underlay blending and ground-build orchestration.

use crate::floor::{FlatTile, FloorBuilder, FloorHeights, IndexedTile, SunLighting, WaterFogData};

use crate::tileflags::SceneLevelTileFlags;

use super::{
    rotate_point, FloorSources, LevelBuild, MapLoader, OcclusionTile, SceneFloors, TileArrays,
    TileTables, UnderlayTile, OVERLAY_POINT, OVERLAY_TRIS_SIMPLE, SIMPLE_A, SIMPLE_B, SIMPLE_C,
    TILE_POINT_X, TILE_POINT_Z, UNDERLAY_POINT, UNDERLAY_TRIS_SIMPLE,
};

impl<'a> MapLoader<'a> {
    // -- floors -------------------------------------------------------------

    /// Create one
    /// [`FloorBuilder`] per level in the scene's current (normal / underwater)
    /// set. `normal_source` is the underwater loader's heightmaps when the
    /// normal floors should take seabed normals.
    pub fn build_floors(&mut self, scene: &mut SceneFloors, normal_source: Option<&[Vec<i32>]>) {
        rs910_core::profile::scope!("build floors");
        for level in 0..self.levels {
            let mut flags_a = 0;
            let mut flags_b = 0;
            if !self.underwater {
                if self.is_water_detail {
                    flags_b |= 0x8;
                }
                if self.is_lighting_detail {
                    flags_a |= 0x2;
                }
                if self.scenery_shadows != 0 {
                    flags_a |= 0x1;
                    flags_b |= 0x10;
                }
            }
            if self.is_lighting_detail {
                flags_b |= 0x7;
            }
            if !self.is_texturing {
                flags_b |= 0x20;
            }
            let source: &[i32] = match normal_source {
                Some(src) if level < src.len() => &src[level],
                _ => &self.level_heightmap[level],
            };
            let builder = FloorBuilder::new(
                flags_a,
                flags_b,
                self.max_x,
                self.max_z,
                self.level_heightmap[level].clone(),
                source,
                512,
            );
            let mut builder = builder;
            builder.underwater = self.underwater;
            let set = if self.underwater {
                &mut scene.underwater_builders
            } else {
                &mut scene.normal_builders
            };
            set[level] = Some(builder);
        }
    }
    /// Blend, mesh every level, finalise. `other_a` is the underwater floor while building
    /// the normal set, `other_b` is the finished normal floor 0 while building the
    /// underwater set.
    pub fn build_ground(
        &mut self,
        scene: &mut SceneFloors,
        flags: &SceneLevelTileFlags,
        other_a: Option<&FloorHeights>,
        other_b: Option<&FloorHeights>,
        sun: &SunLighting,
        lighting: Option<(&[crate::env::StaticLight], &crate::scene::Scene)>,
    ) -> anyhow::Result<()> {
        rs910_core::profile::scope!("build ground");
        let mut blended = vec![0_i32; self.max_x * self.max_z];
        if self.blend_hue.len() != self.max_z {
            self.blend_hue = vec![0; self.max_z];
            self.blend_saturation = vec![0; self.max_z];
            self.blend_lightness = vec![0; self.max_z];
            self.blend_chroma = vec![0; self.max_z];
            self.blend_magnitude = vec![0; self.max_z];
        }
        let SceneFloors {
            normal_builders,
            normal,
            underwater_builders,
            underwater,
            water_fog,
            normal_tiles,
            underwater_tiles,
            lights: scene_lights,
            ..
        } = scene;
        let (builders, finished, tiles) = if self.underwater {
            (underwater_builders, underwater, underwater_tiles)
        } else {
            (normal_builders, normal, normal_tiles)
        };
        for (level, builder) in builders[..self.levels].iter_mut().enumerate() {
            blended.fill(0);
            for z in 0..self.max_z {
                self.blend_hue[z] = 0;
                self.blend_saturation[z] = 0;
                self.blend_lightness[z] = 0;
                self.blend_chroma[z] = 0;
                self.blend_magnitude[z] = 0;
            }
            let mx = self.max_x as i32;
            let mz = self.max_z as i32;
            for x in -5..mx {
                for z in 0..self.max_z {
                    let ahead = x + 5;
                    if ahead < mx {
                        let id = i32::from(
                            self.level_tile_underlay_ids[level][self.ti(ahead as usize, z)],
                        ) & 0x7FFF;
                        if id > 0 {
                            let u = self.flo.underlay(id - 1);
                            self.blend_hue[z] += u.hue;
                            self.blend_saturation[z] += u.saturation;
                            self.blend_lightness[z] += u.lightness;
                            self.blend_chroma[z] += u.chroma;
                            self.blend_magnitude[z] += 1;
                        }
                    }
                    let behind = x - 5;
                    if behind >= 0 {
                        let id = i32::from(
                            self.level_tile_underlay_ids[level][self.ti(behind as usize, z)],
                        ) & 0x7FFF;
                        if id > 0 {
                            let u = self.flo.underlay(id - 1);
                            self.blend_hue[z] -= u.hue;
                            self.blend_saturation[z] -= u.saturation;
                            self.blend_lightness[z] -= u.lightness;
                            self.blend_chroma[z] -= u.chroma;
                            self.blend_magnitude[z] -= 1;
                        }
                    }
                }
                if x >= 0 {
                    let (mut hue, mut sat, mut lum, mut chroma, mut mag) = (0, 0, 0, 0, 0);
                    for z in -5..mz {
                        let ahead = z + 5;
                        if ahead < mz {
                            hue += self.blend_hue[ahead as usize];
                            sat += self.blend_saturation[ahead as usize];
                            lum += self.blend_lightness[ahead as usize];
                            chroma += self.blend_chroma[ahead as usize];
                            mag += self.blend_magnitude[ahead as usize];
                        }
                        let behind = z - 5;
                        if behind >= 0 {
                            hue -= self.blend_hue[behind as usize];
                            sat -= self.blend_saturation[behind as usize];
                            lum -= self.blend_lightness[behind as usize];
                            chroma -= self.blend_chroma[behind as usize];
                            mag -= self.blend_magnitude[behind as usize];
                        }
                        if z >= 0 && chroma > 0 && mag > 0 {
                            blended[self.ti(x as usize, z as usize)] =
                                crate::colour::hsl24to16(hue * 256 / chroma, sat / mag, lum / mag);
                        }
                    }
                }
            }
            let floor = builder.as_mut().ok_or_else(|| {
                anyhow::anyhow!("build ground: level {level} has no floor (build floors not run)")
            })?;
            let (a, b) = if level == 0 {
                (other_a, other_b)
            } else {
                (None, None)
            };
            if self.is_ground_blending {
                self.build_tiles_blended(LevelBuild {
                    floor,
                    level,
                    blended: &blended,
                    underwater_heights: a,
                    surface_heights: b,
                    flags,
                    water_fog: water_fog.as_mut(),
                    tiles,
                })?;
            } else {
                self.build_tiles_unblended(LevelBuild {
                    floor,
                    level,
                    blended: &blended,
                    underwater_heights: a,
                    surface_heights: b,
                    flags,
                    water_fog: water_fog.as_mut(),
                    tiles,
                })?;
            }
            // The per-level tile arrays are no longer needed.
            self.level_tile_underlay_ids[level] = Vec::new();
            self.level_tile_overlay_ids[level] = Vec::new();
            self.level_tile_overlay_shape[level] = Vec::new();
            self.level_tile_overlay_rotation[level] = Vec::new();
        }
        // The floor hard-shadow sweep.
        if !self.underwater && self.scenery_shadows != 0 {
            crate::hardshadow::sweep_shadows(builders.as_mut_slice(), sun);
        }
        // The static point lights baked onto the (not yet finalised) floors.
        if !self.underwater && self.is_lighting_detail {
            if let Some((static_lights, graph)) = lighting {
                *scene_lights = crate::floorlight::build_static_lighting(
                    static_lights,
                    graph,
                    builders.as_slice(),
                    9,
                    256,
                    self.max_x as i32,
                    self.max_z as i32,
                );
            }
        }
        for level in 0..self.levels {
            let builder = builders[level].take().ok_or_else(|| {
                anyhow::anyhow!("build ground: level {level} floor missing at finalise")
            })?;
            finished[level] = Some(builder.finalise(sun)?);
        }
        Ok(())
    }
    /// Flat per-triangle colours, no neighbour blending.
    pub(super) fn build_tiles_unblended(&mut self, build: LevelBuild<'_>) -> anyhow::Result<()> {
        let LevelBuild {
            floor,
            level,
            blended,
            underwater_heights,
            surface_heights,
            flags,
            mut water_fog,
            tiles,
        } = build;
        for x in 0..self.max_x {
            for z in 0..self.max_z {
                let ti = self.ti(x, z);
                let mut shape = i32::from(self.level_tile_overlay_shape[level][ti]);
                let rot = i32::from(self.level_tile_overlay_rotation[level][ti]);
                let ov_wire = i32::from(self.level_tile_overlay_ids[level][ti]) & 0x7FFF;
                let ul_wire = i32::from(self.level_tile_underlay_ids[level][ti]) & 0x7FFF;
                let mut overlay = if ov_wire == 0 {
                    None
                } else {
                    Some(self.flo.overlay(ov_wire - 1))
                };
                let underlay = if ul_wire == 0 {
                    None
                } else {
                    Some(self.flo.underlay(ul_wire - 1))
                };
                if shape == 0 && overlay.is_none() {
                    shape = 12;
                }
                let occlude_overlay = overlay;
                if let Some(o) = overlay {
                    if o.rgb == -1 && o.averagecolour == -1 {
                        overlay = None;
                    }
                }
                if overlay.is_none() && underlay.is_none() {
                    continue;
                }
                let shape_u = shape as usize;
                self.underlay_vertex_count = UNDERLAY_TRIS_SIMPLE[shape_u];
                self.overlay_vertex_count = OVERLAY_TRIS_SIMPLE[shape_u];
                let total = (if overlay.is_none() {
                    0
                } else {
                    self.overlay_vertex_count
                }) + (if underlay.is_none() {
                    0
                } else {
                    self.underlay_vertex_count
                });
                let total = total as usize;
                let mut k = 0;
                self.angle = 0;
                self.tile_material = overlay.map_or(-1, |o| o.material);
                let ul_material = underlay.map_or(-1, |u| u.material);
                let mut tri_a = vec![0; total];
                let mut tri_b = vec![0; total];
                let mut tri_c = vec![0; total];
                let mut tri_rgb = vec![0; total];
                let mut tri_mat = vec![0; total];
                let mut tri_scale = vec![0; total];
                let mut tri_alt: Option<Vec<i32>> = match overlay {
                    Some(o) if o.averagecolour != -1 => Some(vec![0; total]),
                    _ => None,
                };
                match overlay {
                    None => self.angle += self.overlay_vertex_count,
                    Some(o) => {
                        for _ in 0..self.overlay_vertex_count {
                            let a = self.angle as usize;
                            tri_a[k] = SIMPLE_A[shape_u][a];
                            tri_b[k] = SIMPLE_B[shape_u][a];
                            tri_c[k] = SIMPLE_C[shape_u][a];
                            tri_mat[k] = self.tile_material;
                            tri_scale[k] = o.materialscale;
                            tri_rgb[k] = o.rgb;
                            if let Some(alt) = tri_alt.as_mut() {
                                alt[k] = o.averagecolour;
                            }
                            k += 1;
                            self.angle += 1;
                        }
                        if !self.underwater && level == 0 {
                            if let Some(fog) = water_fog.as_deref_mut() {
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
                    }
                }
                if let Some(u) = underlay {
                    for _ in 0..self.underlay_vertex_count {
                        let a = self.angle as usize;
                        tri_a[k] = SIMPLE_A[shape_u][a];
                        tri_b[k] = SIMPLE_B[shape_u][a];
                        tri_c[k] = SIMPLE_C[shape_u][a];
                        tri_mat[k] = ul_material;
                        tri_scale[k] = u.materialscale;
                        tri_rgb[k] = blended[ti];
                        if let Some(alt) = tri_alt.as_mut() {
                            alt[k] = tri_rgb[k];
                        }
                        k += 1;
                        self.angle += 1;
                    }
                }
                let points = TILE_POINT_X.len();
                let mut px = vec![0; points];
                let mut pz = vec![0; points];
                let mut depth: Option<Vec<i32>> = surface_heights.map(|_| vec![0; points]);
                let mut offset: Option<Vec<i32>> =
                    if surface_heights.is_none() && underwater_heights.is_none() {
                        None
                    } else {
                        Some(vec![0; points])
                    };
                for p in 0..points {
                    let (rx, rz) = rotate_point(TILE_POINT_X[p], TILE_POINT_Z[p], rot);
                    px[p] = rx;
                    pz[p] = rz;
                    let fx = ((x as i32) << 9) + rx;
                    let fz = ((z as i32) << 9) + rz;
                    if let (Some(d), Some(a5)) = (depth.as_mut(), surface_heights) {
                        if OVERLAY_POINT[shape_u][p] {
                            d[p] =
                                a5.get_fine_height(fx, fz) - floor.heights.get_fine_height(fx, fz);
                        }
                    }
                    if let Some(o) = offset.as_mut() {
                        if let (Some(a5), false) = (surface_heights, OVERLAY_POINT[shape_u][p]) {
                            o[p] =
                                floor.heights.get_fine_height(fx, fz) - a5.get_fine_height(fx, fz);
                        } else if let (Some(a4), false) =
                            (underwater_heights, UNDERLAY_POINT[shape_u][p])
                        {
                            o[p] =
                                a4.get_fine_height(fx, fz) - floor.heights.get_fine_height(fx, fz);
                        }
                    }
                }
                let h00 = floor.heights.get_tile_height(x, z);
                let h10 = floor.heights.get_tile_height(x + 1, z);
                let h11 = floor.heights.get_tile_height(x + 1, z + 1);
                let h01 = floor.heights.get_tile_height(x, z + 1);
                let link = flags.is_link_below(x as i32, z as i32);
                if (link && level > 1) || (!link && level > 0) {
                    // Else-if chain.
                    #[allow(
                        clippy::if_same_then_else,
                        reason = "the branches stay separate so each rule reads on its own"
                    )]
                    let ok = if underlay.is_some_and(|u| !u.occlude) {
                        false
                    } else if ul_wire == 0 && shape != 0 {
                        false
                    } else {
                        !(ov_wire > 0 && occlude_overlay.is_some_and(|o| !o.occlude))
                    };
                    if ok && h00 == h10 && h00 == h11 && h00 == h01 {
                        let vi = self.vi(x, z);
                        self.level_occludemap[level][vi] |= 0x4;
                    }
                }
                let fog = if self.underwater {
                    water_fog
                        .as_deref()
                        .map_or_else(WaterFogData::default, |f| f.get(x, z))
                } else {
                    WaterFogData::default()
                };
                floor.add_tile_unblended(
                    self.materials,
                    x,
                    z,
                    IndexedTile {
                        point_x: &px,
                        point_offset: depth.as_deref(),
                        point_z: &pz,
                        point_depth: offset.as_deref(),
                        tri_a: &tri_a,
                        tri_b: &tri_b,
                        tri_c: &tri_c,
                        tri_rgb: &tri_rgb,
                        tri_alt: tri_alt.as_deref(),
                        tri_material: &tri_mat,
                        tri_scale: &tri_scale,
                        water_fog: fog,
                        hard_shadow: false,
                    },
                )?;
                tiles.create_tile(level, x, z);
            }
        }
        Ok(())
    }
    /// The ground-blending path with neighbour sampling.
    pub(super) fn build_tiles_blended(&mut self, build: LevelBuild<'_>) -> anyhow::Result<()> {
        let level = build.level;
        let shapes = std::mem::take(&mut self.level_tile_overlay_shape[level]);
        let rots = std::mem::take(&mut self.level_tile_overlay_rotation[level]);
        let ul_ids = std::mem::take(&mut self.level_tile_underlay_ids[level]);
        let ov_ids = std::mem::take(&mut self.level_tile_overlay_ids[level]);
        let tables = TileTables {
            shapes: &shapes,
            rots: &rots,
            ul_ids: &ul_ids,
            ov_ids: &ov_ids,
        };
        let result = self.build_tiles_blended_inner(build, &tables);
        self.level_tile_overlay_shape[level] = shapes;
        self.level_tile_overlay_rotation[level] = rots;
        self.level_tile_underlay_ids[level] = ul_ids;
        self.level_tile_overlay_ids[level] = ov_ids;
        result
    }
    pub(super) fn build_tiles_blended_inner(
        &mut self,
        build: LevelBuild<'_>,
        tables: &TileTables<'_>,
    ) -> anyhow::Result<()> {
        let LevelBuild {
            floor,
            level,
            blended,
            underwater_heights,
            surface_heights,
            flags,
            mut water_fog,
            tiles,
        } = build;
        let TileTables {
            shapes,
            rots,
            ul_ids,
            ov_ids,
        } = *tables;
        let mut edges: [bool; 4];
        for x in 0..self.max_x {
            let x1 = if x < self.max_x - 1 { x + 1 } else { x };
            for z in 0..self.max_z {
                let z1 = if z < self.max_z - 1 { z + 1 } else { z };
                let ti = self.ti(x, z);
                self.shape_id = i32::from(shapes[ti]);
                self.angle = i32::from(rots[ti]);
                let ov_wire = i32::from(ov_ids[ti]) & 0x7FFF;
                let ul_wire = i32::from(ul_ids[ti]) & 0x7FFF;
                if ov_wire == 0 && ul_wire == 0 {
                    continue;
                }
                let mut overlay = if ov_wire == 0 {
                    None
                } else {
                    Some(self.flo.overlay(ov_wire - 1))
                };
                let underlay = if ul_wire == 0 {
                    None
                } else {
                    Some(self.flo.underlay(ul_wire - 1))
                };
                if self.shape_id == 0 && overlay.is_none() {
                    self.shape_id = 12;
                }
                self.hard_shadow = false;
                self.blend_overlay = false;
                edges = [false; 4];
                let occlude_overlay = overlay;
                if let Some(o) = overlay {
                    if o.rgb == -1 && o.averagecolour == -1 {
                        overlay = None;
                    } else if underlay.is_some() && self.shape_id != 0 {
                        self.blend_overlay = o.blend;
                    }
                }
                self.angle = self.resolve_diagonal(ul_wire, [x, z, x1, z1], &floor.heights, ul_ids);
                for i in 0..13 {
                    self.slots.priority[i] = -1;
                    self.slots.edges[i] = 1;
                }
                self.prepare_tile(overlay, underlay, [x, z], tables, &mut edges);
                self.use_simple_triangles =
                    !self.blend_overlay && !edges[0] && !edges[2] && !edges[1] && !edges[3];
                self.select_shape_tables(overlay, underlay);
                let mut tri_count = self.overlay_vertex_count + self.underlay_vertex_count;
                if tri_count <= 0 {
                    tiles.create_tile(level, x, z);
                    continue;
                }
                if edges[0] {
                    tri_count += 1;
                }
                if edges[2] {
                    tri_count += 1;
                }
                if edges[1] {
                    tri_count += 1;
                }
                if edges[3] {
                    tri_count += 1;
                }
                self.shape_vertex_index = 0;
                self.vertex_count = 0;
                let n = tri_count as usize * 3;
                let mut arrays = TileArrays {
                    alt: if self.use_vertex_colours {
                        Some(vec![0; n])
                    } else {
                        None
                    },
                    xs: vec![0; n],
                    zs: vec![0; n],
                    rgb: vec![0; n],
                    material: vec![-1; n],
                    scale: vec![0; n],
                    depth: surface_heights.map(|_| vec![0; n]),
                    offset: if surface_heights.is_none() && underwater_heights.is_none() {
                        None
                    } else {
                        Some(vec![0; n])
                    },
                };
                let ground = FloorSources {
                    heights: &floor.heights,
                    surface: surface_heights,
                    underwater: underwater_heights,
                };
                self.add_overlay_vertices(
                    [level, x, z],
                    overlay,
                    &edges,
                    &mut arrays,
                    ground,
                    water_fog.as_deref_mut(),
                );
                let ul_north = i32::from(ul_ids[self.ti(x, z1)]) & 0x7FFF;
                let ul_ne = i32::from(ul_ids[self.ti(x1, z1)]) & 0x7FFF;
                let ul_east = i32::from(ul_ids[self.ti(x1, z)]) & 0x7FFF;
                self.add_underlay_vertices(
                    [x, z, x1, z1],
                    UnderlayTile {
                        underlay,
                        here: ul_wire,
                        north: ul_north,
                        ne: ul_ne,
                        east: ul_east,
                    },
                    &edges,
                    &mut arrays,
                    blended,
                    ground,
                );
                self.compute_occlusion(
                    &floor.heights,
                    OcclusionTile {
                        underlay,
                        overlay: occlude_overlay,
                        ul_wire,
                        ov_wire,
                    },
                    level,
                    [x, z, x1, z1],
                    flags,
                );
                let fog = if self.underwater {
                    water_fog
                        .as_deref()
                        .map_or_else(WaterFogData::default, |f| f.get(x, z))
                } else {
                    WaterFogData::default()
                };
                // The arrays are sized at `sized_tri_count` from the edge flags
                // but the fan for an edge is only emitted when the shape's
                // per-edge triangle exists (`shape_vertex_counts[edge] != -1`),
                // so `vertex_count` can fall short of `sized_tri_count`. `add_tile`
                // still consumes every colour entry: the trailing zero
                // vertices (x 0, z 0, rgb 0, material -1, scale 0) become
                // real degenerate triangles. Varrock hits this; keep them.
                // The surface-heights-only array (`depth`) becomes the tile's `offsets`; the
                // surface-or-underwater array (`offset`) becomes its `depths`, the
                // water-depth stream (`TEX_COORD_1`).
                floor.add_tile(
                    self.materials,
                    x,
                    z,
                    FlatTile {
                        xs: arrays.xs,
                        offsets: arrays.depth,
                        zs: arrays.zs,
                        depths: arrays.offset,
                        rgb: arrays.rgb,
                        alt: arrays.alt,
                        material_ids: arrays.material,
                        material_scales: arrays.scale,
                        water_fog: fog,
                        hard_shadow: self.hard_shadow,
                    },
                )?;
                tiles.create_tile(level, x, z);
            }
        }
        Ok(())
    }
}
