//! Loc meshes and their materials built on the renderer's threads ahead of
//! their draws (performance plan P5).
//!
//! The first frame of a scene builds the mesh of every loc it draws, casts
//! from off screen or shows the probes (2,000-4,700 RT7 meshes online:
//! 0.4-1 s on the render thread) and loads their materials' maps. Before each
//! of those loops the renderer hands the locs whose meshes are not cached to
//! [`ModernRenderer::prebuild_locs`]: the shape models they need and do not
//! have are decoded on the pool (`frame::jobs`) and cached on the render
//! thread (the model caches are the renderer's), the streams are built on
//! the pool (`models::rt7::Rt7Plan::build` and the classic mesh fallback touch
//! no cache), and the new materials' maps are decoded on the pool
//! (`Textures::prefetch`). `prepare_entity` takes each mesh at the loc's
//! draw, where it counts and uploads it, and `Textures::ensure` uploads each
//! material at its first use, exactly as building them there would. So the
//! frame, its draw order, the arena layout and the counters are unchanged;
//! only the building moves. Results a frame did not take are dropped with
//! it.

use crate::frame::*;
use crate::models::rt7::{Rt7Outcome, Rt7Plan};

/// Locs per job.
const LOCS_PER_JOB: usize = 8;

/// One loc mesh built ahead: what it was built for and its outcome.
pub(crate) struct Prebuilt {
    key: EntityKey,
    fingerprint: (usize, i32, i32),
    /// The RT7 outcome, and the classic mesh when it has no RT7 streams.
    outcome: Rt7Outcome,
    classic: Option<ModelStreams>,
}

/// This frame's meshes built ahead, by loc slot.
#[derive(Default)]
pub(crate) struct Prebuilds {
    ready: crate::fast_hash::FastMap<LocSlot, Prebuilt>,
    /// Meshes built ahead and taken by their draws so far (tests).
    pub(crate) taken: u64,
}

impl Prebuilds {
    /// The start of a frame: what the last one did not take is dropped.
    pub(crate) fn clear(&mut self) {
        self.ready.clear();
    }
}

/// The model identity a loc mesh is cached under (`prepare_entity`).
pub(crate) fn fingerprint(entity: &EntityDraw<'_>) -> (usize, i32, i32) {
    (
        std::ptr::from_ref(entity.model) as usize,
        entity.model.unique_count,
        entity.model.draw_face_count,
    )
}

impl ModernRenderer {
    /// Whether nothing is built ahead (tests: the frame of building each
    /// at its draw).
    fn builds_inline(&self) -> bool {
        #[cfg(test)]
        if self.preparation.test_inline_builds {
            return true;
        }
        false
    }

    /// Whether `entity`'s cached loc mesh is missing or was built for
    /// another model (`prepare_entity` builds it then).
    pub(crate) fn loc_mesh_stale(&self, entity: &EntityDraw<'_>) -> bool {
        entity.key.is_some_and(|key| {
            self.scene_resources
                .statics
                .get(&crate::frame::resources::loc_slot(&key))
                .is_none_or(|s| s.key != key || s.fingerprint != fingerprint(entity))
        })
    }

    /// Build the meshes of `entities` that are not cached, on the threads
    /// (see the module docs). Dynamic locs extend the frame pose cache.
    pub(crate) fn prebuild_locs<'e>(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        entities: impl IntoIterator<Item = &'e EntityDraw<'e>>,
    ) {
        let Some(materials) = snapshot.materials else {
            return;
        };
        if self.builds_inline() {
            return;
        }
        let start = std::time::Instant::now();
        let stale: Vec<&EntityDraw<'_>> = entities
            .into_iter()
            .filter(|entity| {
                entity.key.is_some_and(|key| {
                    self.loc_mesh_stale(entity)
                        && !self
                            .preparation
                            .prebuilds
                            .ready
                            .get(&crate::frame::resources::loc_slot(&key))
                            .is_some_and(|p| p.key == key && p.fingerprint == fingerprint(entity))
                })
            })
            .collect();
        if stale.is_empty() {
            return;
        }
        self.pose_entities(snapshot, stale.iter().copied());
        // Their shape models not decoded yet, decoded on the threads.
        if let Some(pack) = snapshot.pack {
            let mut seen = std::collections::HashSet::new();
            let mut missing = Vec::new();
            for entity in &stale {
                for id in self.rt7.missing_models(snapshot, entity) {
                    if seen.insert(id) {
                        missing.push(id);
                    }
                }
            }
            let decoded = self.jobs.map(missing.len(), |i| {
                crate::models::rt7::decode_models(pack, missing[i])
            });
            self.rt7.insert_models(&missing, decoded);
        }
        let mut work: Vec<(&EntityDraw<'_>, Rt7Plan)> = Vec::new();
        for entity in stale {
            let plan = self.rt7.plan(snapshot, entity);
            if plan.buildable() {
                work.push((entity, plan));
            }
        }
        if work.is_empty() {
            return;
        }
        let jobs = work.len().div_ceil(LOCS_PER_JOB);
        let built = self.jobs.map(jobs, |job| {
            let end = ((job + 1) * LOCS_PER_JOB).min(work.len());
            work[job * LOCS_PER_JOB..end]
                .iter()
                .map(|(entity, plan)| {
                    let outcome = plan.build(entity.model, materials);
                    let classic = match outcome {
                        Rt7Outcome::Built(..) => None,
                        _ => crate::models::mesh::model_streams(
                            entity.model,
                            materials,
                            Colour::Classic,
                        ),
                    };
                    (outcome, classic)
                })
                .collect::<Vec<_>>()
        });
        // Their materials' maps not loaded yet, decoded on the threads too.
        let mut ids = Vec::new();
        for (outcome, classic) in built.iter().flatten() {
            let streams = match outcome {
                Rt7Outcome::Built(streams, _) => Some(streams),
                _ => classic.as_ref(),
            };
            ids.extend(streams.iter().flat_map(|s| s.batches.iter().map(|b| b.0)));
        }
        self.prefetch_materials(snapshot, ids);
        for ((entity, _), (outcome, classic)) in work.iter().zip(built.into_iter().flatten()) {
            let key = entity.key.expect("a loc");
            self.preparation.prebuilds.ready.insert(
                crate::frame::resources::loc_slot(&key),
                Prebuilt {
                    key,
                    fingerprint: fingerprint(entity),
                    outcome,
                    classic,
                },
            );
        }
        self.rt7.build_time += start.elapsed();
    }

    /// Decode the maps of the materials `ids` names that are not loaded yet
    /// on the threads (`Textures::prefetch`; each is uploaded at its first
    /// use, as loading it there would).
    pub(crate) fn prefetch_materials(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        ids: impl IntoIterator<Item = i32>,
    ) {
        if self.builds_inline() {
            return;
        }
        let pack = snapshot.pack;
        let jobs = &self.jobs;
        self.device_resources
            .textures
            .prefetch(pack, snapshot.materials, ids, |work| {
                jobs.map(work.len(), |i| work[i].decode(pack))
            });
    }

    /// The streams of `entity`'s mesh built ahead ([`Self::prebuild_locs`]),
    /// counted as building it now counts them (`None`: not built ahead; the
    /// inner `None`: it draws nothing).
    pub(crate) fn take_prebuilt(
        &mut self,
        entity: &EntityDraw<'_>,
    ) -> Option<Option<ModelStreams>> {
        let key = entity.key?;
        let slot = crate::frame::resources::loc_slot(&key);
        let p = self.preparation.prebuilds.ready.get(&slot)?;
        if p.key != key || p.fingerprint != fingerprint(entity) {
            return None;
        }
        let p = self
            .preparation
            .prebuilds
            .ready
            .remove(&slot)
            .expect("a prebuilt mesh");
        self.preparation.prebuilds.taken += 1;
        let rt7 = self.rt7.count(entity, p.outcome);
        Some(rt7.or(p.classic))
    }
}
