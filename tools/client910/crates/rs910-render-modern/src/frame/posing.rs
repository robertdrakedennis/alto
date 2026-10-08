//! The frame's posed models on the renderer's threads (`frame::jobs`).
//!
//! The models posed this frame (the draw list's entities without a cache
//! key: NPC and player bodies, projectiles, spot anims, a player's spot
//! shadow and hint arrows) are re-posed on the CPU every frame: an animated
//! model's RT7 mesh posed as its classic model (`models::rt7_anim`), else the
//! classic mesh's streams. Before the draws are recorded, [`ModernRenderer::
//! pose_models`] finds each model's RT7 map in the draw order (the map
//! cache is the renderer's and is built here, on the render thread) and
//! poses them on the threads, each from its model and map alone. The draws
//! then borrow their streams in the draw order (`prepare_entity`), so the
//! per-frame arena, the instances and the counters are what posing each
//! model at its draw gives. A model drawn twice in a frame, and a posed
//! model drawn outside the list (an off-screen shadow caster), uses the same
//! frame cache. A cached dynamic loc is rebuilt only when its pose key changes.

use std::sync::Arc;

use crate::frame::*;
use crate::gpumodel::GpuModel;
use crate::models::rt7_anim::{AnimMap, PoseCount, PoseScratch};

/// Models per job (a job poses a run of the frame's models).
const MODELS_PER_JOB: usize = 4;

/// One posed model: its streams (`None`: it draws nothing) and, for an RT7
/// pose, what the pose adds to the animated models' counters.
pub(crate) struct Posed {
    streams: Option<Arc<ModelStreams>>,
    count: Option<PoseCount>,
    counted: bool,
}

/// This frame's posed models by model address, borrowed by every draw.
#[derive(Default)]
pub(crate) struct Posing {
    posed: HashMap<PoseIdentity, Posed>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PoseIdentity {
    model: usize,
    rt7: bool,
}

fn identity(entity: &EntityDraw<'_>) -> PoseIdentity {
    PoseIdentity {
        model: std::ptr::from_ref(entity.model) as usize,
        rt7: matches!(entity.kind, Kind::Model | Kind::Body),
    }
}

impl Posing {
    pub(crate) fn has(&self, entity: &EntityDraw<'_>) -> bool {
        self.posed.contains_key(&identity(entity))
    }
}

/// `model` posed with `map`, else its classic mesh.
fn pose(
    model: &GpuModel,
    map: Option<&AnimMap>,
    materials: &crate::texture::MaterialStore,
    scratch: &mut PoseScratch,
) -> Posed {
    let classic = || crate::models::mesh::model_streams(model, materials, Colour::Classic);
    match map {
        Some(map) => {
            let (streams, count) = map.pose_counted(model, materials, scratch);
            Posed {
                streams: streams.or_else(classic).map(Arc::new),
                count: Some(count),
                counted: false,
            }
        }
        None => Posed {
            streams: classic().map(Arc::new),
            count: None,
            counted: false,
        },
    }
}

impl ModernRenderer {
    /// Pose the transient models and stale dynamic locs `list` draws.
    pub(crate) fn pose_models(&mut self, snapshot: &SceneSnapshot<'_>, list: &DrawList<'_>) {
        self.posing.posed.clear();
        self.pose_entities(snapshot, list.opaque.iter().chain(&list.transparent));
    }

    /// Build each model's streams once in this frame, including stale
    /// dynamic locs. Later shadow/capture candidates extend the same cache.
    pub(crate) fn pose_entities<'e>(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        entities: impl IntoIterator<Item = &'e EntityDraw<'e>>,
    ) {
        let Some(materials) = snapshot.materials else {
            return;
        };
        // In the draw order: each distinct model and its map.
        let mut models: Vec<(&EntityDraw<'_>, Option<Arc<AnimMap>>)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for entity in entities {
            let identity = identity(entity);
            if self.posing.posed.contains_key(&identity) || !seen.insert(identity) {
                continue;
            }
            if entity.key.is_some()
                && (!self.loc_mesh_stale(entity)
                    || !crate::models::rt7::Rt7Cache::is_dynamic(snapshot, entity))
            {
                continue;
            }
            let map = self.rt7.anim_map(snapshot, entity, Some(self.frame));
            models.push((entity, map));
        }
        let jobs = models.len().div_ceil(MODELS_PER_JOB);
        let posed = self.jobs.map(jobs, |job| {
            let mut scratch = PoseScratch::default();
            let end = ((job + 1) * MODELS_PER_JOB).min(models.len());
            models[job * MODELS_PER_JOB..end]
                .iter()
                .map(|(entity, map)| pose(entity.model, map.as_deref(), materials, &mut scratch))
                .collect::<Vec<_>>()
        });
        for ((entity, _), posed) in models.iter().zip(posed.into_iter().flatten()) {
            self.posing.posed.insert(identity(entity), posed);
        }
    }

    /// The streams of posed `entity`: its pose from [`Self::pose_models`]
    /// (its counts added now, in the draw order), else posed here.
    pub(crate) fn posed_streams(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        materials: &crate::texture::MaterialStore,
        entity: &EntityDraw<'_>,
    ) -> Option<Arc<ModelStreams>> {
        let identity = identity(entity);
        if !self.posing.posed.contains_key(&identity) {
            let map = self.rt7.anim_map(snapshot, entity, Some(self.frame));
            let posed = pose(
                entity.model,
                map.as_deref(),
                materials,
                &mut PoseScratch::default(),
            );
            self.posing.posed.insert(identity, posed);
        }
        let posed = self.posing.posed.get_mut(&identity).expect("posed model");
        if let Some(mut count) = posed.count {
            if posed.counted {
                count.time = std::time::Duration::ZERO;
            }
            self.rt7.anim.count(&count);
            posed.counted = true;
        }
        posed.streams.clone()
    }
}
