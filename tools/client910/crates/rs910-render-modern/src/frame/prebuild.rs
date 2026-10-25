//! Owned background builds for static loc meshes. The frame queues bounded
//! requests, polls completions without waiting, then installs ready meshes
//! within one upload allowance shared by visible, shadow and probe passes.
//! Actors remain immediate; static locs appear when their final mesh is ready.
//! Scene generations and model fingerprints reject superseded results. Fully
//! resident controls retain the synchronous path for recorded comparisons.

use crate::frame::*;
use crate::models::rt7::{Rt7Outcome, Rt7Plan};
use rs910_jobs::assets::AssetPriority;

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
    pending: rustc_hash::FxHashMap<LocSlot, Pending>,
    generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    sources: std::sync::Arc<crate::models::rt7::StreamSources>,
    materials: Option<std::sync::Arc<crate::texture::MaterialStore>>,
    pub(crate) uploads: crate::streaming::UploadBudget,
    pub(crate) sync: bool,
    ready: rustc_hash::FxHashMap<LocSlot, Prebuilt>,
    /// Slots requested by earlier passes this frame, with visible draws first.
    requested: rustc_hash::FxHashMap<LocSlot, ()>,
    /// Meshes built ahead and taken by their draws so far (tests).
    pub(crate) taken: u64,
}

/// Outstanding meshes, including finished meshes waiting for installation.
const MAX_PENDING_MESHES: usize = 64;

struct Pending {
    key: EntityKey,
    fingerprint: (usize, i32, i32),
    completion: rs910_jobs::assets::AssetHandle<Option<Prebuilt>>,
}

impl Prebuilds {
    #[cfg(test)]
    pub(crate) fn pending_counts(&self) -> (usize, usize) {
        (self.pending.len(), self.ready.len())
    }

    pub(crate) fn reset(&mut self) {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.ready.clear();
        self.requested.clear();
        self.pending.clear();
        self.materials = None;
    }

    /// Drain completed owned jobs; this never waits for a worker.
    pub(crate) fn clear(&mut self) {
        self.requested.clear();
        if self.sync {
            self.ready.clear();
        }
        self.pending
            .retain(|&slot, pending| match pending.completion.try_recv() {
                Ok(Ok(Some(built))) => {
                    self.ready.insert(slot, built);
                    false
                }
                Ok(Ok(None)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
                Ok(Err(panic)) => std::panic::resume_unwind(panic),
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
            });
    }

    /// A changed view may leave completed meshes with no current consumer.
    /// Evict one unused result when capacity is needed, keeping this frame's
    /// earlier (visible) requests ahead of shadow and probe requests.
    fn make_room(&mut self) -> bool {
        let outstanding = self.pending.len() + self.ready.len();
        if outstanding < MAX_PENDING_MESHES {
            return true;
        }
        let unused = self
            .ready
            .keys()
            .find(|slot| !self.requested.contains_key(slot))
            .copied();
        if let Some(slot) = unused {
            self.ready.remove(&slot);
            return true;
        }
        false
    }

    pub(crate) fn ready_for(&self, entity: &EntityDraw<'_>) -> bool {
        entity.key.is_some_and(|key| {
            self.ready
                .get(&crate::frame::resources::loc_slot(&key))
                .is_some_and(|p| p.key == key && p.fingerprint == fingerprint(entity))
        }) && self.uploads.available()
    }

    fn waiting(&self, entity: &EntityDraw<'_>) -> bool {
        entity.key.is_some_and(|key| {
            let slot = crate::frame::resources::loc_slot(&key);
            let fingerprint = fingerprint(entity);
            self.pending
                .get(&slot)
                .is_some_and(|p| p.key == key && p.fingerprint == fingerprint)
                || self
                    .ready
                    .get(&slot)
                    .is_some_and(|p| p.key == key && p.fingerprint == fingerprint)
        })
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
        priority: AssetPriority,
    ) {
        if !self.preparation.prebuilds.sync && !self.builds_inline() {
            self.stream_locs(snapshot, entities, priority);
            return;
        }
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

    fn stream_locs<'e>(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        entities: impl IntoIterator<Item = &'e EntityDraw<'e>>,
        priority: AssetPriority,
    ) {
        let (Some(pack), Some(materials)) = (snapshot.pack, snapshot.materials) else {
            return;
        };
        for entity in entities {
            if !self.loc_mesh_stale(entity)
                || self.adopt_previous(snapshot, entity)
                || crate::models::rt7::Rt7Cache::is_dynamic(snapshot, entity)
            {
                continue;
            }
            let Some(key) = entity.key else {
                continue;
            };
            let slot = crate::frame::resources::loc_slot(&key);
            self.preparation.prebuilds.requested.insert(slot, ());
            if self.preparation.prebuilds.waiting(entity) {
                if let Some(pending) = self.preparation.prebuilds.pending.get(&slot) {
                    pending.completion.promote(priority);
                }
                continue;
            }
            let source = self.rt7.stream_source(snapshot, entity);
            if matches!(
                source,
                Err(Rt7Plan::Skip | Rt7Plan::NotStatic | Rt7Plan::Dynamic)
            ) {
                continue;
            }
            let fingerprint = fingerprint(entity);
            if self
                .preparation
                .prebuilds
                .pending
                .get(&slot)
                .is_some_and(|p| p.key != key || p.fingerprint != fingerprint)
            {
                self.preparation.prebuilds.pending.remove(&slot);
            }
            self.preparation
                .prebuilds
                .ready
                .retain(|&ready_slot, built| {
                    ready_slot != slot || (built.key == key && built.fingerprint == fingerprint)
                });
            if !self.preparation.prebuilds.make_room() {
                continue;
            }
            let model = entity.model.clone();
            let materials = self
                .preparation
                .prebuilds
                .materials
                .get_or_insert_with(|| std::sync::Arc::new(materials.clone()))
                .clone();
            let pack = pack.clone();
            let sources = self.preparation.prebuilds.sources.clone();
            let generation = self.preparation.prebuilds.generation.clone();
            let expected_generation = generation.load(std::sync::atomic::Ordering::Relaxed);
            let Some(completion) = self.jobs.assets().spawn(priority, move || {
                if generation.load(std::sync::atomic::Ordering::Relaxed) != expected_generation {
                    return None;
                }
                let plan = match source {
                    Ok((ids, scale)) => sources.plan(&pack, &ids, scale),
                    Err(plan) => plan,
                };
                let outcome = plan.build(&model, &materials);
                let classic = match outcome {
                    Rt7Outcome::Built(..) => None,
                    _ => crate::models::mesh::model_streams(&model, &materials, Colour::Classic),
                };
                Some(Prebuilt {
                    key,
                    fingerprint,
                    outcome,
                    classic,
                })
            }) else {
                continue;
            };
            self.preparation.prebuilds.pending.insert(
                crate::frame::resources::loc_slot(&key),
                Pending {
                    key,
                    fingerprint,
                    completion,
                },
            );
        }
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
        if !self.preparation.prebuilds.sync && !self.preparation.prebuilds.uploads.available() {
            return None;
        }
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
