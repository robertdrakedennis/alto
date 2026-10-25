//! Revision-owned actor meshes in persistent shared GPU pages. A presentation
//! reuses installed geometry; a new authoritative pose rewrites its allocation.
//! All passes consume the same mesh, including reflections and dynamic shadows.

use std::sync::Arc;

use crate::frame::arenas::LocArena;
use crate::frame::resources::Mesh;
use crate::frame::ModelStreams;
use crate::models::draw_list::{EntityDraw, Kind};

const UNUSED_PRESENTATIONS_BEFORE_RELEASE: u64 = 4;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct MeshKey {
    model: usize,
    kind: Kind,
}
struct Entry {
    revision: Arc<()>,
    mesh: Mesh,
    used: u64,
}
#[derive(Default)]
pub(crate) struct DynamicMeshes {
    meshes: rustc_hash::FxHashMap<MeshKey, Entry>,
}

pub(crate) struct MeshInput<'a> {
    pub(crate) device: &'a wgpu::Device,
    pub(crate) queue: &'a dyn rs910_gpu_device::uploads::Uploader,
    pub(crate) entity: &'a EntityDraw<'a>,
    pub(crate) streams: &'a ModelStreams,
    pub(crate) revision: &'a Arc<()>,
    pub(crate) frame: u64,
}

impl DynamicMeshes {
    pub(crate) fn resolve(&mut self, arena: &mut LocArena, input: MeshInput<'_>) -> &Mesh {
        let key = MeshKey {
            model: std::ptr::from_ref(input.entity.model) as usize,
            kind: input.entity.kind,
        };
        let entry = self.meshes.entry(key).or_insert_with(|| Entry {
            revision: input.revision.clone(),
            mesh: Mesh {
                alloc: arena.store(input.device, input.queue, None, input.streams),
                batches: input.streams.batches.clone(),
                bounds: crate::models::bounds::Bounds::of(&input.streams.vertices),
            },
            used: input.frame,
        });
        if !Arc::ptr_eq(&entry.revision, input.revision) {
            entry.mesh.alloc = arena.store(
                input.device,
                input.queue,
                Some(entry.mesh.alloc),
                input.streams,
            );
            entry.mesh.batches.clone_from(&input.streams.batches);
            entry.mesh.bounds = crate::models::bounds::Bounds::of(&input.streams.vertices);
            entry.revision = input.revision.clone();
        }
        entry.used = input.frame;
        &entry.mesh
    }

    /// Free every mesh (a new scene; from the next frame on).
    pub(crate) fn release_all(&mut self, arena: &mut LocArena) {
        for (_, entry) in self.meshes.drain() {
            arena.free(entry.mesh.alloc);
        }
    }

    /// Free only meshes absent from several presentations. Page ranges enter
    /// the arena's deferred free list and cannot alias this frame's draws.
    pub(crate) fn release_unused(&mut self, arena: &mut LocArena, frame: u64) {
        self.meshes.retain(|_, entry| {
            let keep = frame.saturating_sub(entry.used) < UNUSED_PRESENTATIONS_BEFORE_RELEASE;
            if !keep {
                arena.free(entry.mesh.alloc);
            }
            keep
        });
    }
}
