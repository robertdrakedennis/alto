//! Revision caches and conservative visibility for temporary scene preparation.
use super::*;

pub(crate) type GroundStack = (i32, i32, i32, i32, i32, [Option<crate::obj_stack::Obj>; 3]);

/// Ranked stacks are stable until a packet changes them or the map changes.
#[derive(Default)]
pub(crate) struct GroundStacks {
    pub(crate) revision: Option<(u64, u64, i32, i32, i32, i32)>,
    pub(crate) rows: Vec<GroundStack>,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub(crate) struct NpcDefinitionRevision {
    pub(crate) base: u32,
    pub(crate) resolved: u32,
    pub(crate) bas: i32,
    pub(crate) seeds: [i32; 4],
    pub(crate) shadows: bool,
    pub(crate) textures: bool,
    pub(crate) shadow_texture: (i32, i32),
}

#[derive(Clone)]
pub(crate) struct NpcDefinition {
    pub(crate) look: crate::npc_draw::Look,
    pub(crate) fade_in: i32,
    pub(crate) bas: Option<std::rc::Rc<crate::protocol910::bas_types::Bas>>,
}

#[derive(Default)]
pub(crate) struct NpcDefinitions {
    pub(crate) store: Option<std::rc::Rc<crate::config::NpcStore>>,
    pub(crate) rows: SoftMap<NpcDefinitionRevision, NpcDefinition>,
}

/// The same vertical tile columns the scene planner tests, conservatively kept
/// near the eye so culled particle owners still receive their normal update.
pub(crate) struct TransientVisibility {
    pub(crate) cpu: crate::camera::CpuProjection,
    pub(crate) eye: [i32; 2],
    pub(crate) distance: i32,
    pub(crate) size_shift: i32,
    pub(crate) tile_size: i32,
    pub(crate) columns: HashMap<(i32, i32), i32>,
}

impl TransientVisibility {
    const PARTICLE_UPDATE_TILES: i32 = 16;
    const COLUMN_HEIGHT_MARGIN: i32 = 1000;
    const MARGIN_BASE_SHIFT: i32 = 7;

    /// Only omit a temporary when its planned tiles and its posed box are
    /// both outside the view. The modern off-screen caster pass excludes
    /// temporaries; a planner-rejected temporary cannot be a shadow draw.
    pub(crate) fn visible_model(
        &mut self,
        tiles: [i32; 4],
        model: &mut crate::gpumodel::GpuModel,
        position: [f32; 3],
        floors: &[Option<crate::floor::FloorGeometry>],
    ) -> bool {
        if model.has_particles || self.visible(tiles, floors) {
            return true;
        }
        model.min_y();
        let Some([min_x, max_x, min_y, max_y, min_z, max_z, _]) = model.cached_bounds() else {
            return true;
        };
        let [px, py, pz] = position;
        self.box_visible([
            (px + min_x as f32).floor() as i32,
            (px + max_x as f32).ceil() as i32,
            (py + min_y as f32).floor() as i32,
            (py + max_y as f32).ceil() as i32,
            (pz + min_z as f32).floor() as i32,
            (pz + max_z as f32).ceil() as i32,
        ])
    }

    fn box_visible(&self, bounds: [i32; 6]) -> bool {
        let [min_x, max_x, min_y, max_y, min_z, max_z] = bounds;
        let mut planes = -1;
        for x in [min_x, max_x] {
            for z in [min_z, max_z] {
                planes &= self.cpu.segment_code([x, min_y, z], [x, max_y, z]);
            }
        }
        planes == 0
    }

    pub(crate) fn visible(
        &mut self,
        bounds: [i32; 4],
        floors: &[Option<crate::floor::FloorGeometry>],
    ) -> bool {
        let Some(first) = floors.first().and_then(Option::as_ref) else {
            return true;
        };
        let Some(last) = floors.last().and_then(Option::as_ref) else {
            return true;
        };
        let [ex, ez] = self.eye;
        for x in bounds[0]..=bounds[1] {
            for z in bounds[2]..=bounds[3] {
                if (x - ex).abs() <= Self::PARTICLE_UPDATE_TILES
                    && (z - ez).abs() <= Self::PARTICLE_UPDATE_TILES
                {
                    return true;
                }
                if x < ex - self.distance
                    || x >= ex + self.distance
                    || z < ez - self.distance
                    || z >= ez + self.distance
                {
                    continue;
                }
                let mut planes = -1;
                for (x, z) in [(x, z), (x + 1, z), (x, z + 1), (x + 1, z + 1)] {
                    let code = *self.columns.entry((x, z)).or_insert_with(|| {
                        if x < 0
                            || z < 0
                            || x >= first.heights.tiles_x as i32
                            || z >= first.heights.tiles_z as i32
                        {
                            return -1;
                        }
                        let margin = Self::COLUMN_HEIGHT_MARGIN
                            << (self.size_shift - Self::MARGIN_BASE_SHIFT);
                        let top = last.heights.get_tile_height(x as usize, z as usize) - margin;
                        let bottom = first.heights.get_tile_height(x as usize, z as usize)
                            + self.tile_size
                            + margin;
                        self.cpu.segment_code(
                            [x << self.size_shift, top, z << self.size_shift],
                            [x << self.size_shift, bottom, z << self.size_shift],
                        )
                    });
                    planes &= code;
                }
                if planes == 0 {
                    return true;
                }
            }
        }
        false
    }
}

impl NpcDefinitions {
    pub(crate) fn definition(
        &mut self,
        revision: NpcDefinitionRevision,
        create: impl FnOnce() -> NpcDefinition,
    ) -> NpcDefinition {
        if let Some(definition) = self.rows.get(&revision) {
            return definition.clone();
        }
        let definition = create();
        self.rows.insert(revision, definition.clone());
        definition
    }

    pub(crate) fn clean(&mut self, age: u32) {
        self.rows.clean(age);
    }
    pub(crate) fn clear_soft(&mut self) -> usize {
        self.rows.clear_soft()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_posed_box_crossing_the_view_is_kept() {
        const VIEWPORT: [i32; 4] = [0, 0, 64, 48];
        const DRAW_DISTANCE: i32 = 1;
        const TILE_SHIFT: i32 = 9;
        const TILE_SIZE: i32 = 1 << TILE_SHIFT;
        const OUTSIDE: [i32; 6] = [10, 12, 10, 12, 0, 1];
        const TALL_CROSSING: [i32; 6] = [0, 1, -12, 12, 0, 1];
        const WIDE_CROSSING: [i32; 6] = [-12, 12, 0, 1, 0, 1];
        let visibility = TransientVisibility {
            cpu: crate::camera::CpuProjection::new(
                glam::Mat4::IDENTITY.to_cols_array(),
                glam::Mat4::IDENTITY.to_cols_array(),
                VIEWPORT,
            ),
            eye: [0; 2],
            distance: DRAW_DISTANCE,
            size_shift: TILE_SHIFT,
            tile_size: TILE_SIZE,
            columns: HashMap::new(),
        };
        assert!(!visibility.box_visible(OUTSIDE));
        assert!(visibility.box_visible(TALL_CROSSING));
        assert!(visibility.box_visible(WIDE_CROSSING));
    }
}
