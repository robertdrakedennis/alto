//! The underwater scene: the seabed floor and the locs standing on it, drawn
//! when the snapshot carries them (water detail high with an underwater map
//! square). The water surface's refraction and depth read this geometry, so
//! the shallows show the bed and what lies on it; without it (the default
//! water detail) the frame is unchanged.
//!
//! The bed is one more floor: level [`BED_LEVEL`] of [`ModernRenderer::floors`]
//! with every tile selected, drawn after the ordinary floors and never a
//! shadow caster. The locs are cached like the scene's static locs (one loc
//! page range per model, dropped with the scene) and drawn in the entity
//! phases: opaque models before the floors, transparent ones with the
//! transparent entities.

use crate::draw::FloorSelection;
use crate::frame::*;

/// The [`ModernRenderer::floors`] slot of the seabed: after the four levels.
pub(crate) const BED_LEVEL: usize = 4;

/// One underwater loc's cached mesh: the model it was built for and its
/// range in a loc page.
pub(crate) struct UnderwaterMesh {
    fingerprint: (usize, i32, i32),
    mesh: Option<crate::frame::resources::Mesh>,
}

impl UnderwaterMesh {
    #[cfg(test)]
    /// Whether the model has geometry in a loc page.
    pub(crate) fn has_mesh(&self) -> bool {
        self.mesh.is_some()
    }
}

/// The renderer's underwater state.
#[derive(Default)]
pub(crate) struct UnderwaterState {
    /// The selection of every tile of the bed, for its size.
    selection: Option<((usize, usize), FloorSelection)>,
    /// One entry per model of the snapshot's list.
    pub(crate) meshes: Vec<Option<UnderwaterMesh>>,
}

/// A selection covering every tile of a `tiles_x` by `tiles_z` floor.
fn whole_floor(tiles_x: usize, tiles_z: usize) -> FloorSelection {
    let distance = tiles_x.max(tiles_z) as i32;
    let side = 2 * distance as usize + 1;
    FloorSelection {
        whole: true,
        origin: [0, 0],
        distance,
        mask: vec![vec![true; side]; side],
    }
}

/// A translation as column-vector entries (the layout of the draw list).
fn translation(p: [i32; 3]) -> [f32; 16] {
    [
        1.,
        0.,
        0.,
        0.,
        0.,
        1.,
        0.,
        0.,
        0.,
        0.,
        1.,
        0.,
        p[0] as f32,
        p[1] as f32,
        p[2] as f32,
        1.,
    ]
}

impl ModernRenderer {
    /// The seabed draws of the last frame (tests).
    #[cfg(test)]
    pub(crate) fn underwater_bed_draws(&self) -> usize {
        self.draws
            .iter()
            .filter(|d| matches!(d.geometry, Geometry::Floor { level, .. } if level == BED_LEVEL))
            .count()
    }

    /// Record the seabed's floor draws (after the ordinary floors).
    pub(crate) fn prepare_underwater_bed(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        origin: [f32; 3],
    ) {
        let Some(underwater) = snapshot.underwater.as_ref() else {
            return;
        };
        let geometry = underwater.floor;
        if geometry.vertex_count == 0 {
            return;
        }
        let size = (geometry.tiles_x, geometry.tiles_z);
        let selection = match self.underwater.selection.take() {
            Some((have, selection)) if have == size => selection,
            _ => whole_floor(size.0, size.1),
        };
        let draw = crate::models::draw_list::FloorDraw {
            level: BED_LEVEL,
            geometry,
            selection: &selection,
        };
        self.prepare_floor(device, queue, snapshot, &draw);
        let matrix = local_matrix(
            &[
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
            origin,
        );
        let n = self.floors[BED_LEVEL]
            .as_ref()
            .map_or(0, |floor| floor.batches.len());
        for batch in 0..n {
            let (material, uv_scale, count) = {
                let b = &self.floors[BED_LEVEL].as_ref().expect("bed").batches[batch];
                (b.material, b.uv_scale, b.count)
            };
            if count == 0 {
                continue;
            }
            let instance = self.instances.len() as u32;
            // The point lights of the surface level's grid.
            let mut record = self.instance(matrix, material, uv_scale, FLAG_FLOOR);
            record.p2 = [0.0; 4];
            self.instances.push(record);
            self.draws.push(Draw {
                geometry: Geometry::Floor {
                    level: BED_LEVEL,
                    batch,
                },
                material,
                first_index: 0,
                count,
                instance,
                pass: Pass::Opaque,
                casts: false,
                indirect: None,
            });
        }
        self.underwater.selection = Some((size, selection));
    }

    /// Record the draws of the underwater locs that are (`transparent`) or
    /// are not transparent; a transparent one also joins `sorted` with its
    /// camera-local matrix, for the depth order of the blended draws.
    pub(crate) fn prepare_underwater_locs(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        origin: [f32; 3],
        transparent: bool,
        mut sorted: Option<&mut Vec<(std::ops::Range<usize>, [f32; 16])>>,
    ) {
        let (Some(underwater), Some(materials)) =
            (snapshot.underwater.as_ref(), snapshot.materials)
        else {
            return;
        };
        let models = underwater.models;
        if self.underwater.meshes.len() != models.len() {
            self.underwater.meshes.clear();
            self.underwater.meshes.resize_with(models.len(), || None);
        }
        for (index, entry) in models.iter().enumerate() {
            if entry.transparent != transparent {
                continue;
            }
            let fingerprint = (
                std::ptr::from_ref(&entry.model) as usize,
                entry.model.unique_count,
                entry.model.draw_face_count,
            );
            let stale = self.underwater.meshes[index]
                .as_ref()
                .is_none_or(|m| m.fingerprint != fingerprint);
            if stale {
                let mesh =
                    crate::models::mesh::model_streams(&entry.model, materials, Colour::Classic)
                        .map(|streams| {
                            let alloc = self.loc_arena.store(device, queue, None, &streams);
                            crate::frame::resources::Mesh {
                                alloc,
                                batches: streams.batches.clone(),
                                bounds: crate::models::bounds::Bounds::of(&streams.vertices),
                            }
                        });
                self.underwater.meshes[index] = Some(UnderwaterMesh { fingerprint, mesh });
            }
            let Some(mesh) = self.underwater.meshes[index]
                .as_ref()
                .and_then(|m| m.mesh.as_ref())
            else {
                continue;
            };
            let matrix = local_matrix(&translation(entry.position), origin);
            let first = mesh.alloc.first_index();
            let geometry = Geometry::Loc {
                page: mesh.alloc.page,
                base_vertex: mesh.alloc.vertex as i32,
            };
            let start = self.draws.len();
            for &(material, first_index, count) in &mesh.batches {
                self.textures
                    .ensure(device, queue, snapshot.pack, Some(materials), material);
                let instance = self.instances.len() as u32;
                self.instances.push(self.instance(matrix, material, 1.0, 0));
                self.draws.push(Draw {
                    geometry,
                    material,
                    first_index: first_index + first,
                    count,
                    instance,
                    pass: Pass::Opaque,
                    casts: false,
                    indirect: None,
                });
            }
            let bounds = mesh.bounds.map(|b| b.transformed(&matrix));
            self.note_bounds(start, bounds);
            if let Some(sorted) = sorted.as_deref_mut() {
                sorted.push((start..self.draws.len(), matrix));
            }
        }
    }
}
