//! Owned render inputs. The main thread captures them after its authoritative
//! scene phases; the render thread borrows only this packet, never game owners.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{EntityKey, SceneSnapshot, Underwater};
use crate::actor_matrix::Matrix;
use crate::draw::{DrawEntity, DrawPlan};
use crate::gpumodel::GpuModel;
use crate::live_scene::{LiveScene, RoofRemoval};
use crate::player_draw::{PlayerDraw, PlayerDraws, ShadowDraw};
use crate::scene::EntityRef;

#[derive(Clone, Copy)]
pub struct DrawFrame<'a> {
    pub plan: &'a DrawPlan,
}

#[derive(Clone, Copy)]
enum Keys<'a> {
    Borrowed(&'a LiveScene),
    Owned(&'a [EntityKey]),
}

/// The read-only live-scene fields consumed by render backends.
#[derive(Clone, Copy)]
pub struct LiveFrame<'a> {
    pub entities: &'a [DrawEntity],
    pub static_entity_count: usize,
    pub draw: DrawFrame<'a>,
    pub model_lights: &'a crate::model_lights::ModelLights,
    flags: &'a crate::tileflags::SceneLevelTileFlags,
    roof: Option<RoofRemoval<'a>>,
    keys: Keys<'a>,
}

impl<'a> LiveFrame<'a> {
    pub fn borrowed(live: &'a LiveScene) -> Self {
        Self {
            entities: &live.entities,
            static_entity_count: live.static_entity_count,
            draw: DrawFrame {
                plan: &live.draw.plan,
            },
            model_lights: &live.model_lights,
            flags: live.tile_flags(),
            roof: live.roof_removal(),
            keys: Keys::Borrowed(live),
        }
    }
    pub fn owned(live: &'a LiveData) -> Self {
        Self {
            entities: &live.entities,
            static_entity_count: live.static_entity_count,
            draw: DrawFrame { plan: &live.plan },
            model_lights: &live.model_lights,
            flags: &live.flags,
            roof: live.roof.as_ref().map(|roof| RoofRemoval {
                stamps: &roof.stamps,
                stamp: roof.stamp,
                first_plane: roof.first_plane,
            }),
            keys: Keys::Owned(&live.keys),
        }
    }
    pub fn entity_key(&self, id: usize) -> Option<EntityKey> {
        match self.keys {
            Keys::Owned(keys) => keys.get(id).copied(),
            Keys::Borrowed(live) => {
                let entity = live.entities.get(id)?;
                Some(EntityKey {
                    id,
                    source: entity.source,
                    changes: live.slot_changes(id),
                    revision: live
                        .dynamic
                        .as_ref()
                        .filter(|dynamic| dynamic.contains(id))
                        .map(|dynamic| dynamic.revision(id)),
                })
            }
        }
    }
    pub fn has_dynamic(&self, id: usize) -> bool {
        self.entity_key(id)
            .is_some_and(|key| key.revision.is_some())
    }
    pub fn roof_removal(&self) -> Option<RoofRemoval<'a>> {
        self.roof
    }
    pub fn tile_flags(&self) -> &'a crate::tileflags::SceneLevelTileFlags {
        self.flags
    }
}

struct RoofData {
    stamps: Vec<Vec<Vec<i8>>>,
    stamp: i8,
    first_plane: usize,
}

pub struct LiveData {
    entities: Vec<DrawEntity>,
    static_entity_count: usize,
    plan: DrawPlan,
    model_lights: crate::model_lights::ModelLights,
    flags: Arc<crate::tileflags::SceneLevelTileFlags>,
    roof: Option<RoofData>,
    keys: Vec<EntityKey>,
}

#[derive(Default)]
pub struct RenderData {
    pub live: Option<LiveData>,
    pub models: HashMap<EntityRef, Arc<GpuModel>>,
    /// Stable installed-scene identity even for recordings without floor calls.
    pub scene_identity: usize,
    _scene_token: Option<InstalledToken>,
}

struct ShadowData {
    model: GpuModel,
    matrix: Matrix,
    visible: bool,
}
impl ShadowData {
    fn draw(&self) -> ShadowDraw<'_> {
        ShadowDraw {
            model: &self.model,
            matrix: &self.matrix,
            visible: self.visible,
        }
    }
}
struct PlayerData {
    matrix: Matrix,
    visible: bool,
    shadow: Option<ShadowData>,
    arrows: Vec<ShadowData>,
}
#[derive(Default)]
struct Players {
    entries: HashMap<usize, PlayerData>,
}
impl PlayerDraws for Players {
    fn player_draw(&self, player: usize) -> Option<PlayerDraw<'_>> {
        let entry = self.entries.get(&player)?;
        Some(PlayerDraw {
            matrix: &entry.matrix,
            visible: entry.visible,
            shadow: entry.shadow.as_ref().map(ShadowData::draw),
            hint_arrows: entry.arrows.iter().map(ShadowData::draw).collect(),
        })
    }
}

/// One reusable CPU slot. Immutable construction data is shared; mutable
/// frame state and transient model streams are owned by the slot.
pub struct OwnedSnapshot {
    camera: crate::camera::SceneCamera,
    env: crate::env::EnvFrame,
    data: RenderData,
    scene: Option<crate::scene::Scene>,
    floors: Arc<Vec<Option<crate::floor::FloorGeometry>>>,
    lights: Arc<Vec<Vec<crate::floorlight::BakedLight>>>,
    players: Players,
    floor_base: [i32; 2],
    materials: Option<Arc<crate::texture::MaterialStore>>,
    pack: Option<crate::cache::Pack>,
    blackout: bool,
    local_player: Option<usize>,
    underwater: Option<(
        crate::floor::FloorGeometry,
        Vec<crate::rebuild::UnderwaterModel>,
    )>,
    sky: Option<crate::sky_frame::SkyCache>,
    time_ms: i64,
    captured_at: std::time::Instant,
}

impl OwnedSnapshot {
    pub fn captured_age(&self) -> std::time::Duration {
        self.captured_at.elapsed()
    }
    pub fn snapshot(&self) -> SceneSnapshot<'_> {
        SceneSnapshot {
            owned: Some(&self.data),
            time_ms: Some(self.time_ms),
            camera: self.camera.clone(),
            env: &self.env,
            live: None,
            scene: self.scene.as_ref(),
            floors: &self.floors,
            lights: &self.lights,
            players: Some(&self.players),
            floor_base: self.floor_base,
            materials: self.materials.as_deref(),
            pack: self.pack.as_ref(),
            blackout: self.blackout,
            local_player: self.local_player,
            particles: None,
            underwater: self
                .underwater
                .as_ref()
                .map(|(floor, models)| Underwater { floor, models }),
            sky: self
                .sky
                .as_ref()
                .and_then(crate::sky_frame::SkyCache::frame),
        }
    }
}

struct CachedModel {
    key: EntityKey,
    address: usize,
    model: Arc<GpuModel>,
}

/// Main-thread immutable resource cache shared by the two owned slots.
#[derive(Default)]
pub struct SnapshotBuilder {
    scene_identity: Option<usize>,
    scene_token: Option<InstalledToken>,
    models: HashMap<EntityRef, CachedModel>,
    floors: Arc<Vec<Option<crate::floor::FloorGeometry>>>,
    lights: Arc<Vec<Vec<crate::floorlight::BakedLight>>>,
    flags: Option<Arc<crate::tileflags::SceneLevelTileFlags>>,
    materials: Option<(usize, Arc<crate::texture::MaterialStore>)>,
}

#[derive(Clone)]
enum InstalledToken {
    Scene(Arc<()>),
    Floor(Arc<crate::floor::FloorCalls>),
    Unkeyed(Arc<()>),
}
impl InstalledToken {
    fn of(snapshot: &SceneSnapshot<'_>) -> Self {
        if let Some(scene) = snapshot.scene {
            return Self::Scene(scene.render_identity().clone());
        }
        if let Some(calls) = snapshot
            .floors
            .iter()
            .flatten()
            .find_map(|floor| floor.calls.as_ref())
        {
            return Self::Floor(calls.clone());
        }
        // No retained construction identity: refresh conservatively instead
        // of trusting a struct/Vec address that an allocator can reuse.
        Self::Unkeyed(Arc::new(()))
    }
    fn identity(&self) -> usize {
        match self {
            Self::Scene(token) | Self::Unkeyed(token) => Arc::as_ptr(token) as usize,
            Self::Floor(calls) => Arc::as_ptr(calls) as usize,
        }
    }
}

fn shadow(draw: ShadowDraw<'_>) -> ShadowData {
    ShadowData {
        model: draw.model.clone(),
        matrix: *draw.matrix,
        visible: draw.visible,
    }
}

impl SnapshotBuilder {
    /// Capture only after shared CPU posing and draw-cycle writeback finish.
    /// The returned slot has no references into mutable main-thread owners.
    pub fn capture(
        &mut self,
        source: &SceneSnapshot<'_>,
        recycled: Option<OwnedSnapshot>,
    ) -> OwnedSnapshot {
        let captured_at = std::time::Instant::now();
        let time_ms = source
            .time_ms
            .unwrap_or_else(crate::logic_clock::monotonic_millis);
        let token = InstalledToken::of(source);
        let identity = token.identity();
        self.scene_token = Some(token.clone());
        if self.scene_identity != Some(identity) {
            self.scene_identity = Some(identity);
            self.models.clear();
            self.floors = Arc::new(source.floors.to_vec());
            self.lights = Arc::new(source.lights.to_vec());
            self.flags = source
                .live_frame()
                .map(|live| Arc::new(live.tile_flags().clone()));
        }
        if let Some(materials) = source.materials {
            let address = std::ptr::from_ref(materials) as usize;
            if self
                .materials
                .as_ref()
                .is_none_or(|(previous, _)| *previous != address)
            {
                self.materials = Some((address, Arc::new(materials.clone())));
            }
        } else {
            self.materials = None;
        }
        let mut old_models = recycled.map(|slot| slot.data.models).unwrap_or_default();
        let mut data = RenderData {
            scene_identity: identity,
            _scene_token: Some(token),
            ..RenderData::default()
        };
        let mut players = Players::default();
        if let Some(live) = source.live_frame() {
            let mut keys = Vec::with_capacity(live.entities.len());
            let mut seen_players = HashSet::new();
            for (id, entity) in live.entities.iter().enumerate() {
                let key = live
                    .entity_key(id)
                    .expect("a live entity has a resource key");
                keys.push(key);
                if let Some(model) = source.model(entity.source) {
                    let stored = if matches!(entity.source, EntityRef::Temporary(_)) {
                        let mut stored = old_models
                            .remove(&entity.source)
                            .unwrap_or_else(|| Arc::new(model.clone()));
                        if let Some(unique) = Arc::get_mut(&mut stored) {
                            unique.copy_from(model);
                        } else {
                            stored = Arc::new(model.clone());
                        }
                        stored
                    } else {
                        let address = std::ptr::from_ref(model) as usize;
                        let cached =
                            self.models
                                .entry(entity.source)
                                .or_insert_with(|| CachedModel {
                                    key,
                                    address,
                                    model: Arc::new(model.clone()),
                                });
                        if cached.key != key || cached.address != address {
                            *cached = CachedModel {
                                key,
                                address,
                                model: Arc::new(model.clone()),
                            };
                        }
                        cached.model.clone()
                    };
                    data.models.insert(entity.source, stored);
                }
                if matches!(entity.source, EntityRef::Temporary(_))
                    && seen_players.insert(entity.loc_id as usize)
                {
                    if let Some(entry) = source
                        .players
                        .and_then(|players| players.player_draw(entity.loc_id as usize))
                    {
                        players.entries.insert(
                            entity.loc_id as usize,
                            PlayerData {
                                matrix: *entry.matrix,
                                visible: entry.visible,
                                shadow: entry.shadow.map(shadow),
                                arrows: entry.hint_arrows.into_iter().map(shadow).collect(),
                            },
                        );
                    }
                }
            }
            let roof = live.roof_removal().map(|roof| RoofData {
                stamps: roof.stamps.to_vec(),
                stamp: roof.stamp,
                first_plane: roof.first_plane,
            });
            data.live = Some(LiveData {
                entities: live.entities.to_vec(),
                static_entity_count: live.static_entity_count,
                plan: live.draw.plan.clone(),
                model_lights: live.model_lights.clone(),
                flags: self
                    .flags
                    .get_or_insert_with(|| Arc::new(live.tile_flags().clone()))
                    .clone(),
                roof,
                keys,
            });
        }
        OwnedSnapshot {
            camera: source.camera.clone(),
            env: *source.env,
            data,
            scene: source.scene.map(crate::scene::Scene::render_metadata),
            floors: self.floors.clone(),
            lights: self.lights.clone(),
            players,
            floor_base: source.floor_base,
            materials: self
                .materials
                .as_ref()
                .map(|(_, materials)| materials.clone()),
            pack: source.pack.cloned(),
            blackout: source.blackout,
            local_player: source.local_player,
            underwater: source
                .underwater
                .as_ref()
                .map(|underwater| (underwater.floor.clone(), underwater.models.to_vec())),
            sky: source.sky.map(|sky| sky.owned_cache()),
            time_ms,
            captured_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Installing a new scene at the same binding address must invalidate
    /// retained resource caches, including recordings without floor calls.
    #[test]
    fn scene_replacement_at_the_same_address_changes_owned_identity() {
        const TILE_SHIFT: i32 = 9;
        const LEVELS: usize = 1;
        const TILES: usize = 2;
        let mut scene = Box::new(crate::scene::Scene::new(TILE_SHIFT, LEVELS, TILES, TILES));
        let address = std::ptr::from_ref(scene.as_ref());
        let camera = crate::camera::SceneCamera::new([0; 3]);
        let (far, near) = camera.fog_reference();
        let env = crate::env::EnvFrame::default_for(far, near, &camera.view_entries());
        let capture = |builder: &mut SnapshotBuilder, scene: &crate::scene::Scene| {
            let snapshot = SceneSnapshot {
                owned: None,
                time_ms: Some(0),
                camera: camera.clone(),
                env: &env,
                live: None,
                scene: Some(scene),
                floors: &[],
                lights: &[],
                players: None,
                floor_base: [0; 2],
                materials: None,
                pack: None,
                blackout: false,
                local_player: None,
                particles: None,
                underwater: None,
                sky: None,
            };
            builder.capture(&snapshot, None)
        };
        let mut builder = SnapshotBuilder::default();
        let before = capture(&mut builder, scene.as_ref());
        *scene = crate::scene::Scene::new(TILE_SHIFT, LEVELS, TILES, TILES);
        assert_eq!(std::ptr::from_ref(scene.as_ref()), address);
        let after = capture(&mut builder, scene.as_ref());
        assert_ne!(before.data.scene_identity, after.data.scene_identity);
        drop(scene);
        assert!(before.snapshot().scene.is_some());
        assert!(after.snapshot().scene.is_some());
    }
}
