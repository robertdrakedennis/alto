//! Interface type-6 model components: cache models, player heads and bodies, NPC,
//! object and inventory models, each lit and placed without world tilt.
use crate::animation_assets::Pose;
use crate::{
    cache::Pack,
    config::{NpcStore, ObjStore},
    entities910::appearance::{Appearance, Model},
    gpumodel::GpuModel,
    modelunlit::ModelUnlit,
    player_model,
    ui_component_fields::Fields,
    ui_components::{field_snapshot, Ref},
};
use anyhow::{Context, Result};
pub use rs910_config::npc_customisation::*;
use rs910_core::fault::Fault;
use rs910_core::soft_cache::SoftMap;
use std::{collections::BTreeMap, collections::VecDeque, rc::Rc};
field_snapshot!(
    /// The scalars `Models::draw` branches and finishes on.
    ModelFields {
        modelkind: i32,
        model: i32,
        modelNameHash: i32,
        invobject: i32,
        invcount: i32,
        usePlayerModel: bool,
        tint_weight: i32,
        tint_hue: i32,
        tint_saturation: i32,
        tint_luminence: i32,
        disableDepthTest: bool,
        modelorthog: bool,
    }
);
field_snapshot!(
    /// The scalars of `Models::inventory`, read before its animator borrow.
    InventoryFields {
        model: i32,
        modelkind: i32,
        modelNameHash: i32,
        usePlayerModel: bool,
        tint_weight: i32,
    }
);
pub use crate::interface_model::*;
/// Where and when one model component draws this frame.
pub struct ModelPlacement {
    /// Component origin on the canvas.
    pub at: [i32; 2],
    /// Canvas size.
    pub canvas: [i32; 2],
    /// Clip rectangle.
    pub clip: [i32; 4],
    /// Painter quad count at the moment of the draw (orders the model among
    /// sprites).
    pub quad: usize,
    /// True when no world scene is behind the interface.
    pub before_scene: bool,
}

pub struct Models {
    pack: Pack,
    resources: Option<Rc<Resources>>,
    bodies: Option<player_model::Models>,
    players: BTreeMap<i32, Appearance>,
    snapshot_players: BTreeMap<i32, Appearance>,
    object_models: SoftMap<ObjectKey, GpuModel>,
    /// Models of model kind 1 (a cache model id on the component), keyed by the
    /// id and the component's recolour/retexture arrays.
    cache_models: SoftMap<CacheModelKey, GpuModel>,
    /// Model kind 7 heads, keyed by the three kit ids and the local palette.
    kit_head_models: SoftMap<KitHeadKey, GpuModel>,
    /// NPC head and body model caches, keyed by NPC id plus the
    /// customisation cache-key salt.
    npc_head_models: SoftMap<(i32, i64), GpuModel>,
    npc_body_models: SoftMap<(i32, i64), GpuModel>,
    player_head_models: SoftMap<(i32, u64), GpuModel>,
    last_keys: BTreeMap<i32, i64>,
    local: i32,
    map_width: i32,
    pub camera_planes: Option<[f32; 2]>,
    model_detail: i32,
    /// The live local player's varps, used to resolve the `multinpc`
    /// variant of an NPC type.
    player_varps: Vec<i32>,
    /// Primary inventory slot object ids mirrored for model kinds 8/9.
    inventories: BTreeMap<i32, Vec<i32>>,
    /// Inventory model cache (ten entries, least recently used evicted),
    /// keyed by [`inventory_crc64`].
    inventory_models: BTreeMap<i64, GpuModel>,
    inventory_order: VecDeque<i64>,
}

/// A retained object model: the item type that supplies the mesh (after the
/// stack-size substitution) and, when the model wears the local player's
/// colours, those colour and texture indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ObjectKey {
    id: i32,
    palette: Option<([i32; 10], [i32; 10])>,
}

/// A model of kind 1: the component's own recolour and retexture arrays decide
/// the result, so they are part of the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct CacheModelKey {
    model: i32,
    recolour: Option<([i16; 5], [i16; 5])>,
    retexture: Option<([i16; 5], [i16; 5])>,
}

/// A model of kind 7: three kit ids (first, second, third) plus the local
/// player's palette indices that colour it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct KitHeadKey {
    kits: [i32; 3],
    colours: [i32; 10],
    textures: [i32; 10],
}

/// Which model the object builder makes: the item, the stack size that picks
/// its model variant, and whether the local player's colours apply.
struct ObjectRequest {
    id: i32,
    count: i32,
    wear: bool,
}

/// Cap of the kind-1 model cache; the whole cache restarts past it.
const CACHE_MODEL_LIMIT: usize = 128;

impl Models {
    pub fn new(pack: Pack) -> Self {
        Self {
            pack,
            resources: None,
            bodies: None,
            players: BTreeMap::new(),
            snapshot_players: BTreeMap::new(),
            object_models: SoftMap::new(),
            cache_models: SoftMap::new(),
            kit_head_models: SoftMap::new(),
            npc_head_models: SoftMap::new(),
            npc_body_models: SoftMap::new(),
            player_head_models: SoftMap::new(),
            last_keys: BTreeMap::new(),
            local: -1,
            map_width: 0,
            camera_planes: None,
            model_detail: crate::gpumodel::MODEL_DETAIL_FLAGS,
            player_varps: Vec::new(),
            inventories: BTreeMap::new(),
            inventory_models: BTreeMap::new(),
            inventory_order: VecDeque::new(),
        }
    }
    /// Age the cached interface models by one clean (see
    /// `rs910_core::soft_cache`).
    pub fn clean(&mut self, age: u32) {
        self.object_models.clean(age);
        self.cache_models.clean(age);
        self.kit_head_models.clean(age);
        self.npc_head_models.clean(age);
        self.npc_body_models.clean(age);
        self.player_head_models.clean(age);
    }

    /// Drop the interface models nothing has drawn for a while; how many
    /// went.
    pub fn clear_soft(&mut self) -> usize {
        self.object_models.clear_soft()
            + self.cache_models.clear_soft()
            + self.kit_head_models.clear_soft()
            + self.npc_head_models.clear_soft()
            + self.npc_body_models.clear_soft()
            + self.player_head_models.clear_soft()
    }
    /// Retain this tick's primary inventory slot objects for the
    /// kind-8/9 builder; unchanged inventories keep their vectors.
    pub fn sync_inventories(&mut self, cache: &crate::ui_inv::InvCache) {
        self.inventories
            .retain(|id, _| cache.inventory(*id, false).is_some());
        for (id, ids) in cache.primary_obj_ids() {
            if self.inventories.get(&id).map(Vec::as_slice) != Some(ids) {
                self.inventories.insert(id, ids.to_vec());
            }
        }
    }
    /// Retain this tick's local player varps for model selection.
    pub fn sync_player_varps(&mut self, varps: Option<&[i32]>) {
        let varps = varps.unwrap_or(&[]);
        if self.player_varps != varps {
            self.player_varps = varps.to_vec();
        }
    }
    /// Read a varbit or varp over the retained local player varps; `None` is
    /// a missing type (the caller leaves the selector at -1).
    fn read_player_var(
        varps: &[i32],
        varbits: &crate::entity_runtime::bits_pack::Inputs,
        bit: bool,
        id: i32,
    ) -> Option<i32> {
        if bit {
            let t = varbits.get(id, false).ok()?;
            let base = t.binding.as_ref()?;
            t.get(*varps.get(usize::try_from(base.id).ok()?)?).ok()
        } else {
            varps.get(usize::try_from(id).ok()?).copied()
        }
    }

    /// Apply the model-detail flags. Existing UI model draws
    /// are rebuilt on their next draw; unchanged settings keep their caches.
    pub fn set_model_detail(&mut self, detail: i32) -> bool {
        if self.model_detail == detail {
            return false;
        }
        self.model_detail = detail;
        self.bodies = None;
        self.object_models.clear();
        self.cache_models.clear();
        self.kit_head_models.clear();
        self.npc_head_models.clear();
        self.npc_body_models.clear();
        self.player_head_models.clear();
        self.last_keys.clear();
        // The inventory model cache is detail-dependent too.
        self.inventory_models.clear();
        self.inventory_order.clear();
        true
    }
    pub fn sync(&mut self, scene: &crate::ui_cam2::SceneInput<'_>) {
        self.local = scene.local_player.map_or(-1, |p| p.index);
        self.map_width = scene.map_width;
        let Some(players) = scene.players else {
            self.players.clear();
            self.last_keys.clear();
            return;
        };
        self.players.retain(|id, _| {
            players
                .players
                .get(*id as usize)
                .is_some_and(Option::is_some)
        });
        self.last_keys.retain(|id, _| self.players.contains_key(id));
        for (id, p) in players
            .players
            .iter()
            .enumerate()
            .filter_map(|(id, p)| p.as_ref().map(|p| (id as i32, p)))
        {
            if self.players.get(&id) != Some(&p.appearance) {
                self.players.insert(id, p.appearance.clone());
            }
        }
    }
    /// The cache stores the model builders read, loaded on first use.
    pub fn resources(&mut self) -> Result<Rc<Resources>> {
        if self.resources.is_none() {
            self.resources = Some(Rc::new(Resources {
                pack: self.pack.clone(),
                materials: crate::texture::MaterialStore::load(&self.pack)?,
                billboards: crate::billboard::BillboardStore::load(&self.pack)?,
                emitters: crate::particle::EmitterStore::load(&self.pack)?,
                appearance: crate::entity_runtime::appearance_pack::load_inputs(
                    &self.pack, true, false,
                )?,
                bases: crate::ui_bas::load(&self.pack)?,
                avatar_idks: crate::avatar::IdkStore::load(&self.pack)?,
                avatar_defaults: crate::avatar::GraphicsDefaults::load(&self.pack)?,
                varbits: crate::entity_runtime::bits_pack::load(&self.pack)?,
            }));
        }
        if self.bodies.is_none() {
            self.bodies = Some(player_model::Models::load(&self.pack, self.model_detail)?);
        }
        Ok(self.resources.as_ref().unwrap().clone())
    }
    /// The local player's appearance: the live player once a world is
    /// installed, else the lobby appearance kept under id -1.
    fn local_appearance(&self) -> Option<&Appearance> {
        self.players
            .get(&self.local)
            .or_else(|| self.snapshot_players.get(&-1))
    }

    /// The local player's model, which carries the palette that recolours
    /// worn-colour models.
    fn local_model(&self) -> Option<&Model> {
        self.local_appearance().and_then(|a| a.model.as_ref())
    }

    /// Whether `appearance`, shown for player `id`, answers the component's
    /// name hash (the local player always does).
    fn owner_matches(&self, id: i32, appearance: &Appearance, name_hash: i32) -> bool {
        let hash = appearance.name.as_ref().map(|n| {
            n.encode_utf16()
                .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)))
        });
        id == self.local || hash == Some(name_hash)
    }

    /// The player a head component (kind 3) shows: a live player index only.
    fn head_owner(&self, f: &ModelFields) -> Option<(i32, Model)> {
        if !(0..2048).contains(&f.model) {
            return None;
        }
        let appearance = self.players.get(&f.model)?;
        if !self.owner_matches(f.model, appearance, f.modelNameHash) {
            return None;
        }
        Some((f.model, appearance.model.clone()?))
    }

    /// The player a body component (kind 5) shows: a live player index, the
    /// local player (-1) or an interface-only snapshot (below -1).
    fn body_owner(&self, f: &ModelFields) -> Option<(i32, Model)> {
        let (id, appearance) = if (0..2048).contains(&f.model) {
            let appearance = self.players.get(&f.model)?;
            if !self.owner_matches(f.model, appearance, f.modelNameHash) {
                return None;
            }
            (f.model, appearance)
        } else if f.model == -1 {
            (self.local, self.local_appearance()?)
        } else if f.model < -1 {
            (f.model, self.snapshot_players.get(&f.model)?)
        } else {
            return None;
        };
        Some((id, appearance.model.clone()?))
    }

    /// Install the decoded `PLAYER_SNAPSHOT` appearance for an
    /// interface-only model id (`-snapshot - 2`).
    fn install_snapshot(&mut self, id: i32, gender: i8, bytes: &[u8]) -> Result<()> {
        let resources = self.resources()?;
        let model = crate::protocol910::appearance::decode_snapshot(
            bytes,
            gender,
            &resources.appearance.appearance,
        )
        .map_err(|e| anyhow::anyhow!("PLAYER_SNAPSHOT: {e:?}"))?;
        let appearance = Appearance {
            gender,
            bas: model.bas,
            model: Some(model),
            ..Default::default()
        };
        self.snapshot_players.insert(id, appearance);
        self.last_keys.remove(&id);
        self.player_head_models
            .retain(|(player, _), _| *player != id);
        Ok(())
    }

    pub fn apply_snapshot(&mut self, id: i32, gender: i8, bytes: &[u8]) -> Result<()> {
        anyhow::ensure!(id < -1, "invalid player snapshot id {id}");
        self.install_snapshot(id, gender, bytes)
    }

    /// The lobby local player uses the live-player sentinel `-1` while no
    /// world actor table is installed, so keep it in the same model owner.
    pub fn apply_lobby_appearance(&mut self, gender: i8, bytes: &[u8]) -> Result<()> {
        self.install_snapshot(-1, gender, bytes)
    }

    /// `CLEAR_PLAYER_SNAPSHOT` removes the retained interface model and
    /// any head-model cache derived from it.
    pub fn clear_snapshot(&mut self, id: i32) {
        self.snapshot_players.remove(&id);
        self.last_keys.remove(&id);
        self.player_head_models
            .retain(|(player, _), _| *player != id);
    }

    /// The IDK head parts of one kit: its head models, recoloured and
    /// retextured as the kit says. `None` while a head model is unavailable.
    fn idk_head_parts(r: &Resources, idk: &crate::avatar::Idk) -> Option<Vec<ModelUnlit>> {
        let mut parts = Vec::new();
        for &head_id in idk.heads.iter().filter(|&&head| head >= 0) {
            let mut raw = ModelUnlit::load(&r.pack, head_id as u32).ok()?;
            if raw.version < 13 {
                raw.scale_by_power_of_two(2);
            }
            for (&src, &dst) in idk.recol_s.iter().zip(&idk.recol_d) {
                raw.recolor(src as i16, dst as i16);
            }
            for (&src, &dst) in idk.retex_s.iter().zip(&idk.retex_d) {
                raw.rematerial(src as i16, dst as i16);
            }
            parts.push(raw);
        }
        Some(parts)
    }

    /// Merge head parts into one lit model and colour it with a player's
    /// palette indices. `None` when there is nothing to show.
    fn head_from_parts(
        &self,
        r: &Resources,
        mut parts: Vec<ModelUnlit>,
        palette: crate::avatar::AvatarPalette,
        flags: i32,
    ) -> Result<Option<GpuModel>> {
        if parts.is_empty() {
            return Ok(None);
        }
        let raw = if parts.len() == 1 {
            parts.pop().expect("one head model")
        } else {
            let refs: Vec<&ModelUnlit> = parts.iter().collect();
            ModelUnlit::merge(&refs)
        };
        let mut model = GpuModel::new(
            &crate::gpumodel::ModelStores {
                materials: &r.materials,
                billboards: &r.billboards,
                emitters: &r.emitters,
            },
            &raw,
            crate::gpumodel::BuildParams {
                flags,
                ambient: 64,
                contrast: 768,
                detail: self.model_detail,
            },
        )?;
        for (src, dst) in palette.recolor_pairs_u16(&r.avatar_defaults) {
            model.recolor(src as i16, dst as i16);
        }
        for (src, dst) in palette.retexture_pairs_u16(&r.avatar_defaults) {
            model.retexture(&r.materials, src as i16, dst as i16)?;
        }
        model.height();
        Ok(Some(model))
    }

    /// The head of a player (model kind 3), animated by the component's
    /// animator: its render flags widen the cached head and it animates the
    /// copy. IDK head parts and worn-object heads merge independently of the
    /// body; a transformed player shows the head of its NPC type.
    fn player_head(
        &mut self,
        id: i32,
        appearance: &Model,
        r: &Rc<Resources>,
        state: &crate::ui_properties::State,
        poses: Option<&[Pose]>,
    ) -> Result<Option<GpuModel>> {
        let requested = crate::npc_type_model::sequenced_flags(pose_flags(poses));
        if appearance.npc != -1 {
            // A transformed player's head is the head model of its transmog
            // NPC type, with no customisation.
            let npcs: &NpcStore = state.npcs.as_deref().context("npc types not installed")?;
            let Some(npc) = self.npc_type(npcs, appearance.npc, r)? else {
                return Ok(None);
            };
            let key = (npc.id as i32, 0);
            let cached = self.npc_head_models.get(&key);
            let mut model = match crate::npc_type_model::rebuild_flags(cached, requested) {
                None => cached.cloned().context("cached NPC head")?,
                Some(flags) => {
                    let Some(model) =
                        crate::npc_type_model::head(&npc, None, flags, &self.npc_assets(r))?
                    else {
                        return Ok(None);
                    };
                    self.npc_head_models.insert(key, model.clone());
                    model
                }
            };
            animate(&mut model, poses);
            return Ok(Some(model));
        }
        let key = (id, appearance.hash);
        let cached = self.player_head_models.get(&key);
        let mut model = match crate::npc_type_model::rebuild_flags(cached, requested) {
            None => cached.cloned().context("cached player head")?,
            Some(flags) => {
                let Some(model) =
                    self.build_player_head(appearance, r, state.objs.as_deref(), flags)?
                else {
                    return Ok(None);
                };
                self.player_head_models.insert(key, model.clone());
                model
            }
        };
        animate(&mut model, poses);
        Ok(Some(model))
    }

    /// The unanimated head of an appearance built with `flags`: every kit
    /// slot contributes the head models of its identity kit (high bit set) or
    /// its worn item (bit 30 set), merged in slot order.
    fn build_player_head(
        &self,
        appearance: &Model,
        r: &Resources,
        objects: Option<&ObjStore>,
        flags: i32,
    ) -> Result<Option<GpuModel>> {
        let mut parts = Vec::new();
        for (slot, &raw_kit) in appearance.kits.iter().enumerate() {
            if raw_kit & 0x4000_0000 != 0 {
                let Some(obj) = objects.and_then(|store| store.get((raw_kit & 0x3fff_ffff) as u32))
                else {
                    continue;
                };
                let gender = usize::from(appearance.female);
                let custom = appearance.custom.get(slot).and_then(Option::as_ref);
                let mut head_ids = obj.head_models[gender];
                if let Some(custom) = custom {
                    let custom_ids = if appearance.female {
                        custom.woman_head
                    } else {
                        custom.man_head
                    };
                    if custom_ids[0] != -1 {
                        head_ids = custom_ids;
                    }
                }
                if head_ids[0] == -1 {
                    continue;
                }
                let mut worn_parts = Vec::with_capacity(2);
                for (part, &head_id) in head_ids.iter().enumerate() {
                    if part == 1 && head_id == -1 {
                        continue;
                    }
                    let mut raw = match ModelUnlit::load(&r.pack, head_id as u32) {
                        Ok(raw) => raw,
                        Err(_) => return Ok(None),
                    };
                    if raw.version < 13 {
                        raw.scale_by_power_of_two(2);
                    }
                    let custom_recol = custom.and_then(|c| c.recolour.as_deref());
                    for (index, (&src, &dst)) in obj.recol_s.iter().zip(&obj.recol_d).enumerate() {
                        let dst = custom_recol
                            .and_then(|values| values.get(index).copied())
                            .unwrap_or(dst as i16);
                        raw.recolor(src as i16, dst);
                    }
                    let custom_retex = custom.and_then(|c| c.retexture.as_deref());
                    for (index, (&src, &dst)) in obj.retex_s.iter().zip(&obj.retex_d).enumerate() {
                        let dst = custom_retex
                            .and_then(|values| values.get(index).copied())
                            .unwrap_or(dst as i16);
                        raw.rematerial(src as i16, dst);
                    }
                    worn_parts.push(raw);
                }
                parts.extend(worn_parts);
            } else if raw_kit & i32::MIN != 0 {
                let Some(idk) = r.avatar_idks.get((raw_kit & 0x3fff_ffff) as u32) else {
                    continue;
                };
                let Some(idk_parts) = Self::idk_head_parts(r, idk) else {
                    return Ok(None);
                };
                parts.extend(idk_parts);
            }
        }
        let palette = crate::avatar::AvatarPalette {
            recolours: appearance.colours.map(|v| v as u8),
            retextures: appearance.textures.map(|v| v as u8),
        };
        self.head_from_parts(r, parts, palette, flags)
    }

    /// The head of model kind 7: three kit ids, coloured with the local
    /// player's palette. The component packs the first kit in the high half of
    /// `model`, the second in the low half and the third in `modelNameHash`.
    fn kit_head(
        &mut self,
        f: &ModelFields,
        r: &Rc<Resources>,
        poses: Option<&[Pose]>,
    ) -> Result<Option<GpuModel>> {
        let Some(local) = self.local_model() else {
            return Ok(None);
        };
        let key = KitHeadKey {
            kits: [
                (f.model as u32 >> 16) as i32,
                f.model & 0xffff,
                f.modelNameHash,
            ],
            colours: local.colours,
            textures: local.textures,
        };
        let requested = crate::npc_type_model::sequenced_flags(pose_flags(poses));
        let cached = self.kit_head_models.get(&key);
        let mut model = match crate::npc_type_model::rebuild_flags(cached, requested) {
            None => cached.cloned().context("cached kit head")?,
            Some(flags) => {
                let mut parts = Vec::new();
                for kit in key.kits {
                    let Some(idk) = u32::try_from(kit).ok().and_then(|id| r.avatar_idks.get(id))
                    else {
                        continue;
                    };
                    let Some(idk_parts) = Self::idk_head_parts(r, idk) else {
                        return Ok(None);
                    };
                    parts.extend(idk_parts);
                }
                let palette = crate::avatar::AvatarPalette {
                    recolours: key.colours.map(|v| v as u8),
                    retextures: key.textures.map(|v| v as u8),
                };
                let Some(model) = self.head_from_parts(r, parts, palette, flags)? else {
                    return Ok(None);
                };
                self.kit_head_models.insert(key, model.clone());
                model
            }
        };
        animate(&mut model, poses);
        Ok(Some(model))
    }

    /// The model of kind 1: the cache model named by the component, with the
    /// component's recolour and retexture arrays, animated by its animator.
    fn cache_model(
        &mut self,
        c: &Ref,
        f: &ModelFields,
        r: &Rc<Resources>,
        poses: Option<&[Pose]>,
    ) -> Result<Option<GpuModel>> {
        let Ok(model_id) = u32::try_from(f.model) else {
            return Ok(None);
        };
        let key = {
            let c = c.borrow();
            CacheModelKey {
                model: f.model,
                recolour: c.recolour,
                retexture: c.retexture,
            }
        };
        let requested = crate::npc_type_model::sequenced_flags(pose_flags(poses));
        let cached = self.cache_models.get(&key);
        let mut model = match crate::npc_type_model::rebuild_flags(cached, requested) {
            None => cached.cloned().context("cached component model")?,
            Some(flags) => {
                let Ok(mut raw) = ModelUnlit::load(&r.pack, model_id) else {
                    return Ok(None);
                };
                if raw.version < 13 {
                    raw.scale_by_power_of_two(2);
                }
                let mut model = GpuModel::new(
                    &crate::gpumodel::ModelStores {
                        materials: &r.materials,
                        billboards: &r.billboards,
                        emitters: &r.emitters,
                    },
                    &raw,
                    crate::gpumodel::BuildParams {
                        flags,
                        ambient: 64,
                        contrast: 768,
                        detail: self.model_detail,
                    },
                )?;
                if let Some((src, dst)) = key.recolour {
                    for (src, dst) in src.into_iter().zip(dst) {
                        model.recolor(src, dst);
                    }
                }
                if let Some((src, dst)) = key.retexture {
                    for (src, dst) in src.into_iter().zip(dst) {
                        model.retexture(&r.materials, src, dst)?;
                    }
                }
                if self.cache_models.len() >= CACHE_MODEL_LIMIT {
                    self.cache_models.clear();
                }
                self.cache_models.insert(key, model.clone());
                model
            }
        };
        animate(&mut model, poses);
        Ok(Some(model))
    }

    /// An item model (the `IF_SETOBJECT` family and model kind 4): the model
    /// variant the stack size selects, lit like an inventory icon, resized,
    /// coloured by the item's own recolours and, for worn-colour models, the
    /// local player's palette, animated by the component's animator.
    fn object(
        &mut self,
        request: ObjectRequest,
        poses: Option<&[Pose]>,
        state: &crate::ui_properties::State,
        r: &Rc<Resources>,
    ) -> Result<Option<GpuModel>> {
        let objects: &ObjStore = state
            .objs
            .as_deref()
            .context("object types not installed")?;
        let Some(mut obj) = u32::try_from(request.id)
            .ok()
            .and_then(|id| objects.get(id))
        else {
            return Ok(None);
        };
        if request.count > 1 {
            if let Some(counts) = &obj.inventory.countobj {
                let mut variant = None;
                for &(item, count) in counts {
                    if count != 0 && request.count >= count {
                        variant = Some(item);
                    }
                }
                if let Some(item) = variant {
                    obj = u32::try_from(item)
                        .ok()
                        .and_then(|id| objects.get(id))
                        .context("stack variant item missing")?;
                }
            }
        }
        let palette = request
            .wear
            .then(|| self.local_model().map(|m| (m.colours, m.textures)))
            .flatten();
        let key = ObjectKey {
            id: obj.id as i32,
            palette,
        };
        let requested = crate::npc_type_model::sequenced_flags(pose_flags(poses));
        let cached = self.object_models.get(&key);
        let mut model = match crate::npc_type_model::rebuild_flags(cached, requested) {
            None => cached.cloned().context("cached object model")?,
            Some(flags) => {
                let Some(&model_id) = obj.models.first() else {
                    return Ok(None);
                };
                let Ok(mut raw) = ModelUnlit::load(&r.pack, model_id) else {
                    return Ok(None);
                };
                if raw.version < 13 {
                    raw.scale_by_power_of_two(2);
                }
                let mut model = GpuModel::new(
                    &crate::gpumodel::ModelStores {
                        materials: &r.materials,
                        billboards: &r.billboards,
                        emitters: &r.emitters,
                    },
                    &raw,
                    crate::gpumodel::BuildParams {
                        flags,
                        ambient: obj.inventory.ambient + 64,
                        contrast: obj.inventory.contrast * 5 + 850,
                        detail: self.model_detail,
                    },
                )?;
                let [sx, sy, sz] = obj.inventory.resize;
                if sx != 128 || sy != 128 || sz != 128 {
                    model.scale(sx, sy, sz);
                }
                for (index, (&src, &dst)) in obj.recol_s.iter().zip(&obj.recol_d).enumerate() {
                    let dst = match obj.inventory.recol_palette.get(index) {
                        Some(&slot) => crate::npc_type_model::CLIENT_PALETTE[usize::from(slot)],
                        None => dst as i16,
                    };
                    model.recolor(src as i16, dst);
                }
                for (&src, &dst) in obj.retex_s.iter().zip(&obj.retex_d) {
                    model.retexture(&r.materials, src as i16, dst as i16)?;
                }
                if let Some((colours, textures)) = palette {
                    let palette = crate::avatar::AvatarPalette {
                        recolours: colours.map(|v| v as u8),
                        retextures: textures.map(|v| v as u8),
                    };
                    for (src, dst) in palette.recolor_pairs_u16(&r.avatar_defaults) {
                        model.recolor(src as i16, dst as i16);
                    }
                    for (src, dst) in palette.retexture_pairs_u16(&r.avatar_defaults) {
                        model.retexture(&r.materials, src as i16, dst as i16)?;
                    }
                }
                self.object_models.insert(key, model.clone());
                model
            }
        };
        animate(&mut model, poses);
        Ok(Some(model))
    }

    /// The NPC type for `id`, followed through its `multinpc` variants over
    /// the retained local player varps (shared by head and body models).
    fn npc_type<'s>(
        &self,
        npcs: &'s NpcStore,
        id: i32,
        r: &Resources,
    ) -> Result<Option<std::borrow::Cow<'s, crate::config::Npc>>> {
        let base = crate::npc_type_model::list(npcs, id as u32)?;
        let varps = &self.player_varps;
        crate::npc_type_model::resolve(npcs, base, &|bit, var| {
            Self::read_player_var(varps, &r.varbits, bit, var)
        })
    }
    fn npc_assets<'a>(&self, r: &'a Resources) -> crate::npc_type_model::Assets<'a> {
        crate::npc_type_model::Assets {
            pack: &r.pack,
            materials: &r.materials,
            billboards: &r.billboards,
            emitters: &r.emitters,
            bases: Some(&r.bases),
            detail: self.model_detail,
        }
    }

    /// The NPC head model for model kind 2 (`if_setnpchead`): the
    /// component's `customisation` replaces head models and
    /// recolour/retexture targets. Heads are lit at 64/768 and are not
    /// resized. The cache is keyed by the resolved `multinpc` type.
    /// `poses` is the component's model animator: its render flags widen the
    /// cached head and it animates the copy.
    fn npc_head(
        &mut self,
        f: &Fields,
        custom: Option<&NpcCustomisation>,
        poses: Option<&[Pose]>,
        state: &crate::ui_properties::State,
        r: &Rc<Resources>,
    ) -> Result<Option<GpuModel>> {
        let npcs: &NpcStore = state.npcs.as_deref().context("npc types not installed")?;
        let Some(npc) = self.npc_type(npcs, f.model, r)? else {
            return Ok(None);
        };
        let salt = custom.map_or(0, |c| c.cache_key_salt);
        let key = (npc.id as i32, salt);
        let requested = crate::npc_type_model::sequenced_flags(pose_flags(poses));
        let cached = self.npc_head_models.get(&key);
        let mut model = match crate::npc_type_model::rebuild_flags(cached, requested) {
            None => cached.cloned().context("cached NPC head")?,
            Some(flags) => {
                let Some(model) =
                    crate::npc_type_model::head(&npc, custom, flags, &self.npc_assets(r))?
                else {
                    return Ok(None);
                };
                self.npc_head_models.insert(key, model.clone());
                model
            }
        };
        animate(&mut model, poses);
        Ok(Some(model))
    }

    /// The NPC body model for model kind 6 (`if_setnpcmodel`) with the
    /// component's model animator as the main node and no overlays,
    /// head-turn or wearpos transforms. The animator's render flags widen the
    /// cached base; the copy is animated, then resized.
    fn npc_body(
        &mut self,
        f: &Fields,
        custom: Option<&NpcCustomisation>,
        poses: Option<&[Pose]>,
        state: &crate::ui_properties::State,
        r: &Rc<Resources>,
    ) -> Result<Option<GpuModel>> {
        let npcs: &NpcStore = state.npcs.as_deref().context("npc types not installed")?;
        let Some(npc) = self.npc_type(npcs, f.model, r)? else {
            return Ok(None);
        };
        let salt = custom.map_or(0, |c| c.cache_key_salt);
        let key = (npc.id as i32, salt);
        let requested = crate::npc_type_model::sequenced_flags(pose_flags(poses));
        let cached = self.npc_body_models.get(&key);
        let mut model = match crate::npc_type_model::rebuild_flags(cached, requested) {
            None => cached.cloned().context("cached NPC body")?,
            Some(flags) => {
                // Interface NPC bodies use the type's own BAS.
                let Some(model) =
                    crate::npc_type_model::body(&npc, custom, npc.bas, flags, &self.npc_assets(r))?
                else {
                    return Ok(None);
                };
                self.npc_body_models.insert(key, model.clone());
                model
            }
        };
        animate(&mut model, poses);
        crate::npc_type_model::resize(&npc, &mut model);
        Ok(Some(model))
    }
    /// Model kinds 8/9: the worn-equipment model of a mirrored inventory
    /// (female for kind 9), optionally recoloured with the local player's
    /// palette (`usePlayerModel`). The worn models of the inventory's slots
    /// are remapped by the BAS worn-slot remap, moved by the raw BAS
    /// `slot_transforms`, merged and lit at 65/852, then animated on a copy.
    #[cfg(test)]
    fn inventory(
        &mut self,
        c: &Ref,
        f: &Fields,
        state: &crate::ui_properties::State,
        r: &Rc<Resources>,
    ) -> Result<Option<GpuModel>> {
        self.inventory_model(c, InventoryFields::of(f), state, r)
    }
    /// [`Self::inventory`] with the component scalars already read, so the
    /// caller holds no borrow of `c` across the animator's `borrow_mut`.
    fn inventory_model(
        &mut self,
        c: &Ref,
        f: InventoryFields,
        state: &crate::ui_properties::State,
        r: &Rc<Resources>,
    ) -> Result<Option<GpuModel>> {
        let Some(inv) = self.inventories.get(&f.model) else {
            return Ok(None);
        };
        let female = f.modelkind == 9;
        let bas_id = f.modelNameHash;
        let default_bas = crate::protocol910::bas_types::Bas::default();
        let bas = (bas_id != -1).then(|| r.bases.get(&bas_id).unwrap_or(&default_bas));
        let slots: Vec<i32> = match bas.and_then(|b| b.worn_slot_remap.as_ref()) {
            Some(map) => map
                .iter()
                .map(|&s| {
                    usize::try_from(s)
                        .ok()
                        .and_then(|s| inv.get(s))
                        .copied()
                        .unwrap_or(-1)
                })
                .collect(),
            None => inv.clone(),
        };
        let palette = if f.usePlayerModel {
            self.local_model().map(|m| (m.colours, m.textures))
        } else {
            None
        };
        let mut flags = 2048 | if f.tint_weight != 0 { 0x80000 } else { 0 };
        let pose = if let Some(playback) = c.borrow_mut().model_animator.as_mut() {
            let mut animations = state
                .model_animations
                .as_ref()
                .context("UI animation resources")?
                .borrow_mut();
            Some(animations.prepare(playback, 0)?)
        } else {
            None
        };
        if let Some(poses) = &pose {
            flags |= poses.iter().fold(0, |flags, p| flags | p.flags);
        }
        let key = inventory_crc64(&slots, bas_id, palette.as_ref().map(|p| &p.0), female);
        let cached = self.inventory_models.get(&key).cloned();
        let model = if let Some(model) = cached.filter(|m| m.flags & flags == flags) {
            self.inventory_order.retain(|&k| k != key);
            self.inventory_order.push_back(key);
            model
        } else {
            if let Some(old) = self.inventory_models.get(&key) {
                flags |= old.flags;
            }
            let default_item = crate::protocol910::config_types::Item::empty(-1);
            let items = &r.appearance.types.items;
            let bodies = self.bodies.as_mut().context("player body models")?;
            let item = |id: i32| items.get(&id).unwrap_or(&default_item);
            if slots
                .iter()
                .any(|&id| id != -1 && !bodies.wear_ready(item(id), female))
            {
                return Ok(None);
            }
            let mut parts = Vec::with_capacity(slots.len());
            for &id in &slots {
                parts.push(if id != -1 {
                    bodies.wear_model(&r.pack, item(id), female)?
                } else {
                    None
                });
            }
            if let Some(transforms) = bas.and_then(|b| b.slot_transforms.as_ref()) {
                for (slot, t) in transforms.iter().enumerate() {
                    let Some(t) = t else {
                        continue;
                    };
                    let Some(m) = parts
                        .get_mut(slot)
                        .context(Fault::IndexOutOfRange.message("animation slot transform"))?
                        .as_mut()
                    else {
                        continue;
                    };
                    anyhow::ensure!(t.len() == 6, "BAS slot transform length");
                    // Unlike the live player model, the rotation is not
                    // scaled by `<< 3`.
                    if t[3] != 0 || t[4] != 0 || t[5] != 0 {
                        anyhow::ensure!(
                            t[3..].iter().all(|a| (0..16384).contains(a)),
                            Fault::IndexOutOfRange.message("rotation table index")
                        );
                        m.rotate(t[3], t[4], t[5]);
                    }
                    if t[0] != 0 || t[1] != 0 || t[2] != 0 {
                        m.translate(t[0], t[1], t[2]);
                    }
                }
            }
            let raw =
                ModelUnlit::merge_slots(&parts.iter().map(Option::as_ref).collect::<Vec<_>>());
            let mut model = GpuModel::new(
                &crate::gpumodel::ModelStores {
                    materials: &r.materials,
                    billboards: &r.billboards,
                    emitters: &r.emitters,
                },
                &raw,
                crate::gpumodel::BuildParams {
                    flags: if palette.is_some() {
                        flags | 0x4000
                    } else {
                        flags
                    },
                    ambient: 65,
                    contrast: 852,
                    detail: self.model_detail,
                },
            )?;
            if let Some((colours, textures)) = &palette {
                let graphics = &r.appearance.defaults.graphics;
                for (table, indices, colour) in [
                    (graphics.recolour.as_ref(), colours, true),
                    (graphics.retexture.as_ref(), textures, false),
                ] {
                    let table = table.context("player palette")?;
                    for (i, &index) in indices.iter().enumerate() {
                        for j in 0..4 {
                            let index = usize::try_from(index)
                                .context(Fault::IndexOutOfRange.message("palette index"))?;
                            if let Some(&dst) = table.destinations[i][j].get(index) {
                                if colour {
                                    model.recolor(table.source[i][j], dst);
                                } else {
                                    model.retexture(&r.materials, table.source[i][j], dst)?;
                                }
                            }
                        }
                    }
                }
            }
            model.flags = flags;
            self.inventory_models.insert(key, model.clone());
            self.inventory_order.retain(|&k| k != key);
            self.inventory_order.push_back(key);
            while self.inventory_order.len() > 10 {
                if let Some(evicted) = self.inventory_order.pop_front() {
                    self.inventory_models.remove(&evicted);
                }
            }
            model
        };
        let mut model = model;
        model.flags = flags;
        if let Some(poses) = pose {
            for p in poses {
                model.apply_animation(&p.transforms);
            }
        }
        Ok(Some(model))
    }
    /// The body model of a player (model kind 5), animated by the
    /// component's animator. A transformed player shows the body of its NPC
    /// type instead.
    fn player_body(
        &mut self,
        c: &Ref,
        id: i32,
        appearance: &Model,
        state: &crate::ui_properties::State,
        r: &Rc<Resources>,
    ) -> Result<Option<GpuModel>> {
        let tinted = c.borrow().f.tint_weight != 0;
        let (pose, seq) = if let Some(playback) = c.borrow_mut().model_animator.as_mut() {
            let mut animations = state
                .model_animations
                .as_ref()
                .context("UI animation resources")?
                .borrow_mut();
            let seq = animations
                .assets
                .sequences
                .get(&playback.node.id())
                .cloned();
            (Some(animations.prepare(playback, 0)?), seq)
        } else {
            (None, None)
        };
        let defaults = &r.appearance.defaults;
        let inputs = player_model::Inputs {
            items: &r.appearance.types.items,
            bases: &r.bases,
            wear: &defaults.wear,
            recolour: defaults
                .graphics
                .recolour
                .as_ref()
                .context("player recolour palette")?,
            retexture: defaults
                .graphics
                .retexture
                .as_ref()
                .context("player retexture palette")?,
        };
        let flags = 2048
            | pose
                .as_ref()
                .map_or(0, |poses| poses.iter().fold(0, |flags, p| flags | p.flags))
            | if tinted { 0x80000 } else { 0 };
        if appearance.npc != -1 {
            // Transformed players draw the body model of their transmog NPC
            // type (no customisation) with the component's animation.
            let npcs: &NpcStore = state.npcs.as_deref().context("npc types not installed")?;
            let Some(npc) = self.npc_type(npcs, appearance.npc, r)? else {
                return Ok(None);
            };
            let key = (npc.id as i32, 0);
            // The animator's render flags widen the cached transmog base.
            let requested = crate::npc_type_model::sequenced_flags(pose_flags(pose.as_deref()));
            let cached = self.npc_body_models.get(&key);
            let mut model = match crate::npc_type_model::rebuild_flags(cached, requested) {
                None => cached.cloned().context("cached NPC body")?,
                Some(flags) => {
                    let Some(model) = crate::npc_type_model::body(
                        &npc,
                        None,
                        npc.bas,
                        flags,
                        &self.npc_assets(r),
                    )?
                    else {
                        return Ok(None);
                    };
                    self.npc_body_models.insert(key, model.clone());
                    model
                }
            };
            animate(&mut model, pose.as_deref());
            crate::npc_type_model::resize(&npc, &mut model);
            return Ok(Some(model));
        }
        let Some(mut model) = self.bodies.as_mut().unwrap().body(
            &r.pack,
            appearance,
            seq.as_ref(),
            &inputs,
            &crate::gpumodel::ModelStores {
                materials: &r.materials,
                billboards: &r.billboards,
                emitters: &r.emitters,
            },
            crate::player_model::BodyBuild {
                flags,
                cache: true,
                last_key: self.last_keys.entry(id).or_insert(-1),
            },
        )?
        else {
            return Ok(None);
        };
        animate(&mut model, pose.as_deref());
        Ok(Some(model))
    }

    pub fn draw(
        &mut self,
        c: &Ref,
        state: &mut crate::ui_properties::State,
        placement: ModelPlacement,
    ) -> Result<Option<Draw>> {
        let ModelPlacement {
            at,
            canvas,
            clip,
            quad,
            before_scene,
        } = placement;
        // The draw does not write `c.f`, so the `&Fields` consumers below
        // borrow it live; none of them reaches the component itself.
        let f = ModelFields::of(&c.borrow().f);
        let custom = c.borrow().npc_customisation.clone();
        let r = self.resources()?;
        // Item models sit half their height below the component origin.
        let mut y_shift = 0;
        let built = if f.invobject != -1 {
            let poses = animator_poses(c, state)?;
            let request = ObjectRequest {
                id: f.invobject,
                count: f.invcount,
                wear: f.usePlayerModel,
            };
            let mut model = self.object(request, poses.as_deref(), state, &r)?;
            if let Some(model) = &mut model {
                y_shift = model.min_y().wrapping_neg() >> 1;
            }
            model
        } else {
            match f.modelkind {
                1 => {
                    let poses = animator_poses(c, state)?;
                    self.cache_model(c, &f, &r, poses.as_deref())?
                }
                2 | 6 => {
                    let poses = animator_poses(c, state)?;
                    let fields = &c.borrow().f;
                    if f.modelkind == 2 {
                        self.npc_head(fields, custom.as_ref(), poses.as_deref(), state, &r)?
                    } else {
                        self.npc_body(fields, custom.as_ref(), poses.as_deref(), state, &r)?
                    }
                }
                3 => match self.head_owner(&f) {
                    Some((id, appearance)) => {
                        let poses = animator_poses(c, state)?;
                        self.player_head(id, &appearance, &r, state, poses.as_deref())?
                    }
                    None => None,
                },
                // An item picked by id, drawn as a stack of ten in the local
                // player's colours.
                4 => {
                    let poses = animator_poses(c, state)?;
                    let request = ObjectRequest {
                        id: f.model,
                        count: 10,
                        wear: true,
                    };
                    self.object(request, poses.as_deref(), state, &r)?
                }
                5 => match self.body_owner(&f) {
                    Some((id, appearance)) => self.player_body(c, id, &appearance, state, &r)?,
                    None => None,
                },
                7 => {
                    let poses = animator_poses(c, state)?;
                    self.kit_head(&f, &r, poses.as_deref())?
                }
                8 | 9 => self.inventory_model(c, InventoryFields::of(&c.borrow().f), state, &r)?,
                _ => None,
            }
        };
        let Some(mut model) = built else {
            return Ok(None);
        };
        if f.tint_weight != 0 {
            model.tint(
                f.tint_hue,
                f.tint_saturation,
                f.tint_luminence,
                f.tint_weight,
            );
        }
        // The extended draw distance is in effect.
        let far = if self.map_width == 0 {
            430
        } else {
            (self.map_width as f64 * 34.46) as i32
        };
        let component = c.borrow();
        let fields = &component.f;
        let (matrix, projection) = crate::ui_model_transform::matrices(
            fields,
            at,
            canvas,
            [200., ((far << 2) + 512) as f32],
            self.camera_planes,
            y_shift,
        );
        let mut particles = crate::particle::Binding {
            key: crate::particle::keys::component(component.particle_serial),
            ..Default::default()
        };
        if model.has_particles {
            particles.add_model(
                &model,
                &matrix,
                crate::particle::Rotation::IDENTITY,
                crate::particle::keys::BODY,
            );
        }
        let space = DrawSpace {
            matrix: matrix.entries(),
            projection,
            lighting: lighting(fields),
        };
        Ok(Some(Draw {
            space,
            owner: Rc::downgrade(c).into(),
            quad,
            before_scene,
            clip,
            model,
            depth_write: !f.disableDepthTest,
            resources: r,
            particles,
            draw_particles: !f.modelorthog,
            particle_list: Vec::new(),
        }))
    }
}
/// The component's model animator poses, prepared before the model lookup so
/// their render flags reach the model-cache test. `None` when the component
/// has none.
fn animator_poses(c: &Ref, state: &crate::ui_properties::State) -> Result<Option<Vec<Pose>>> {
    let mut component = c.borrow_mut();
    let Some(playback) = component.model_animator.as_mut() else {
        return Ok(None);
    };
    let mut animations = state
        .model_animations
        .as_ref()
        .context("UI animation resources")?
        .borrow_mut();
    Ok(Some(animations.prepare(playback, 0)?))
}

/// The render flags of an interface animator: the OR over its primary and
/// secondary poses.
fn pose_flags(poses: Option<&[Pose]>) -> i32 {
    poses.map_or(0, |poses| poses.iter().fold(0, |flags, p| flags | p.flags))
}

/// Apply an interface animator to the per-draw copy: the primary pose, then
/// the secondary frames when loaded.
fn animate(model: &mut GpuModel, poses: Option<&[Pose]>) {
    for pose in poses.into_iter().flatten() {
        model.apply_animation(&pose.transforms);
    }
}

/// The inventory model cache key: a CRC-64 (reflected ECMA-182 table) over
/// the BAS id's low 16 bits, each slot object's four bytes, the first five
/// palette recolours and the female flag.
fn inventory_crc64(ids: &[i32], bas: i32, recolours: Option<&[i32; 10]>, female: bool) -> i64 {
    let table: [u64; 256] = std::array::from_fn(|i| {
        let mut v = i as u64;
        for _ in 0..8 {
            v = if v & 1 == 1 {
                (v >> 1) ^ 0xC96C_5795_D787_0F42
            } else {
                v >> 1
            };
        }
        v
    });
    let step = |crc: u64, b: i32| (crc >> 8) ^ table[((crc ^ i64::from(b) as u64) & 0xff) as usize];
    let mut crc = step(step(u64::MAX, bas >> 8), bas);
    for &id in ids {
        crc = step(step(step(step(crc, id >> 24), id >> 16), id >> 8), id);
    }
    if let Some(recolours) = recolours {
        for &v in &recolours[..5] {
            crc = step(crc, v);
        }
    }
    step(crc, i32::from(female)) as i64
}
/// Interface model lighting. The current desktop environment profile uses the
/// default brightness=3 and anti-macro=0.
fn lighting(f: &Fields) -> crate::env::EnvFrame {
    let mut env = crate::env::EnvFrame::build(
        &crate::env::Environment::default(),
        crate::env::SunSettings {
            direction: crate::env::DEFAULT_SUN_DIR,
            brightness_pref: 3,
            anti_macro: 0.,
        },
        false,
        crate::env::FogReference {
            far: 10000.,
            near_min: 200.,
            view: &glam::Mat4::IDENTITY.to_cols_array(),
        },
    );
    if f.customlighting {
        env.sun = crate::floor::SunLighting::from_set_sun(
            f.lightDirX as f32 / 256.,
            f.lightDirY as f32 / 256.,
            f.lightDirZ as f32 / 256.,
            f.lightExtraA.wrapping_shl(2) as f32,
            f.lightExtraB.wrapping_shl(2) as f32,
            f.lightExtraC.wrapping_shl(2) as f32,
        );
        env.sun_rgb = [16, 8, 0].map(|s| ((f.lightColour >> s) & 255) as f32 / 255.);
        env.sampler = f.lightExtraD;
    }
    env
}

#[cfg(test)]
mod inventory_model_tests {
    use super::*;
    use crate::ui_components::Component;
    use std::cell::RefCell;

    /// The inventory cache key folds the BAS low 16 bits, all four bytes of each
    /// slot, only the first five recolours and the female flag.
    #[test]
    fn inventory_crc64_inputs() {
        let base = inventory_crc64(&[1205, -1], 1426, None, false);
        assert_ne!(base, inventory_crc64(&[1205, -1], 1426, None, true));
        assert_ne!(base, inventory_crc64(&[1205, 0], 1426, None, false));
        assert_eq!(
            base,
            inventory_crc64(&[1205, -1], 1426 | 0x10000, None, false)
        );
        let mut colours = [0; 10];
        let a = inventory_crc64(&[1205], -1, Some(&colours), false);
        colours[9] = 3;
        assert_eq!(a, inventory_crc64(&[1205], -1, Some(&colours), false));
        colours[4] = 3;
        assert_ne!(a, inventory_crc64(&[1205], -1, Some(&colours), false));
    }

    /// Real cache: model kinds 8/9 build the worn models of a mirrored
    /// inventory (male/female), an absent inventory draws nothing, and an
    /// unknown kind still reaches the strict gap.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn real_pack_inventory_models_build_male_and_female() {
        let pack =
            Pack::open(rs910_core::test_support::client_dir().join("../../server/data/pack"));
        let mut models = Models::new(pack);
        let r = models.resources().unwrap();
        let (&id, _) = r
            .appearance
            .types
            .items
            .iter()
            .find(|(_, item)| {
                item.custom.man[0] != -1
                    && item.custom.woman[0] != -1
                    && item.custom.man[0] != item.custom.woman[0]
            })
            .expect("an item with distinct male/female wear models");
        let mut cache = crate::ui_inv::InvCache::default();
        cache.update(94, 3, id, 1, false);
        models.sync_inventories(&cache);
        let state = crate::ui_properties::State::default();
        let component = Rc::new(RefCell::new(Component::default()));
        let mut f = Fields {
            model: 94,
            modelkind: 8,
            modelNameHash: -1,
            ..Default::default()
        };
        let male = models
            .inventory(&component, &f, &state, &r)
            .unwrap()
            .expect("male inventory model");
        f.modelkind = 9;
        let female = models
            .inventory(&component, &f, &state, &r)
            .unwrap()
            .expect("female inventory model");
        assert!(male.vertex_count > 0 && female.vertex_count > 0);
        assert_ne!(male.position_stream(), female.position_stream());
        assert_eq!(models.inventory_models.len(), 2);
        f.model = 93;
        assert!(models
            .inventory(&component, &f, &state, &r)
            .unwrap()
            .is_none());
    }
}

#[cfg(test)]
mod npc_model_tests {
    use super::*;

    /// Real-cache check that interface model kinds 6/2 build NPC models,
    /// and that a customisation salt produces a separate cached model.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn real_pack_npc_body_and_head_models_build_with_customisation() {
        let pack =
            Pack::open(rs910_core::test_support::client_dir().join("../../server/data/pack"));
        let npcs = Rc::new(NpcStore::load(&pack).unwrap());
        let (id, npc) = npcs
            .iter()
            .find(|(_, n)| n.models.len() > 1 && !n.head_models.is_empty() && n.multinpc.is_empty())
            .map(|(id, n)| (*id as i32, n.clone()))
            .unwrap();
        let state = crate::ui_properties::State {
            npcs: Some(npcs),
            ..Default::default()
        };
        let mut models = Models::new(pack);
        let r = models.resources().unwrap();
        let mut f = Fields {
            model: id,
            modelkind: 6,
            ..Default::default()
        };
        let body = models
            .npc_body(&f, None, None, &state, &r)
            .unwrap()
            .expect("npc body");
        let mut custom = NpcCustomisation::new(&npc, true).unwrap();
        custom.cache_key_salt = 7;
        custom.set_model_transform(0, -1, 0.0, [0; 3], [0; 3]);
        let reduced = models
            .npc_body(&f, Some(&custom), None, &state, &r)
            .unwrap()
            .expect("customised npc body");
        assert_ne!(body.vertex_count, reduced.vertex_count);
        assert_eq!(models.npc_body_models.len(), 2);
        f.modelkind = 2;
        assert!(models
            .npc_head(&f, None, None, &state, &r)
            .unwrap()
            .is_some());
    }

    /// An interface NPC body drawn with the component's animator widens the
    /// cached base with the animator's render flags (0x20 label groups) and
    /// animates the per-draw copy, so two frames draw different vertices.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn real_pack_interface_npc_body_animates_with_the_component_animator() {
        use crate::animation_assets::{AnimationAssets, Playback};
        use crate::entities910::animation_state::{Node, Sequence};
        let pack =
            Pack::open(rs910_core::test_support::client_dir().join("../../server/data/pack"));
        let npcs = Rc::new(NpcStore::load(&pack).unwrap());
        let bases = crate::ui_bas::load(&pack).unwrap();
        let mut animations = AnimationAssets::load(&pack).unwrap();
        let (id, seq) = npcs
            .iter()
            .filter(|(_, n)| n.multinpc.is_empty() && !n.models.is_empty() && n.bas != -1)
            .find_map(|(id, n)| {
                let seq = animations.sequences.get(&bases.get(&n.bas)?.readyanim)?;
                (seq.skeletal == -1 && seq.frame_ids.as_ref()?.len() >= 3)
                    .then(|| (*id as i32, seq.clone()))
            })
            .expect("a cache NPC whose stand animation has classic frames");
        let frames = seq.frame_ids.as_ref().unwrap().len();
        let mut poses_at = |frame: usize| {
            let mut playback = Playback {
                node: Node {
                    sequence: Some(Sequence {
                        id: seq.id,
                        frames: Some(frames),
                        skeletal: false,
                        restart: seq.restart,
                        priority: seq.priority,
                        stationary: seq.stationary,
                        moving: seq.moving,
                    }),
                    frame: frame as i32,
                    next: -1,
                    ..Default::default()
                },
                loaded_skeletal: None,
            };
            animations.interface_poses(&pack, &mut playback, 0).unwrap()
        };
        let first = poses_at(0);
        let later = poses_at(frames / 2);
        let state = crate::ui_properties::State {
            npcs: Some(npcs),
            ..Default::default()
        };
        let mut models = Models::new(pack);
        let r = models.resources().unwrap();
        let f = Fields {
            model: id,
            modelkind: 6,
            ..Default::default()
        };
        let still = models
            .npc_body(&f, None, None, &state, &r)
            .unwrap()
            .unwrap();
        assert!(models.npc_body_models[&(id, 0)].vertex_groups.is_none());
        let a = models
            .npc_body(&f, None, Some(&first), &state, &r)
            .unwrap()
            .unwrap();
        let cached = &models.npc_body_models[&(id, 0)];
        assert!(cached.vertex_groups.is_some(), "rebuilt with label groups");
        assert_eq!(cached.flags & 0x20, 0x20);
        let b = models
            .npc_body(&f, None, Some(&later), &state, &r)
            .unwrap()
            .unwrap();
        assert_ne!((&a.vx, &a.vy, &a.vz), (&still.vx, &still.vy, &still.vz));
        assert_ne!((&a.vx, &a.vy, &a.vz), (&b.vx, &b.vy, &b.vz));
        // The widened base keeps serving unanimated draws unchanged.
        let again = models
            .npc_body(&f, None, None, &state, &r)
            .unwrap()
            .unwrap();
        assert_eq!(
            (&again.vx, &again.vy, &again.vz),
            (&still.vx, &still.vy, &still.vz)
        );
    }

    /// Real cache: `if_setnpcmodel`/`if_setnpchead` on a `multinpc` type draw
    /// the varp-selected variant, keyed in the model caches by the resolved
    /// type, and a null selection draws nothing.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn real_pack_interface_npc_models_resolve_multinpc() {
        let pack =
            Pack::open(rs910_core::test_support::client_dir().join("../../server/data/pack"));
        let npcs = Rc::new(NpcStore::load(&pack).unwrap());
        let (id, npc, index, target) = npcs
            .iter()
            .find_map(|(id, n)| {
                if n.multinpc.len() < 2 || n.multivarbit != -1 || n.multivarp < 0 {
                    return None;
                }
                let index = (0..n.multinpc.len() - 1).find(|&i| {
                    u32::try_from(n.multinpc[i])
                        .ok()
                        .and_then(|t| npcs.get(t))
                        .is_some_and(|t| t.multinpc.is_empty() && !t.models.is_empty())
                })?;
                Some((*id as i32, n.clone(), index, n.multinpc[index] as u32))
            })
            .expect("a varp-selected multinpc type");
        let state = crate::ui_properties::State {
            npcs: Some(npcs.clone()),
            ..Default::default()
        };
        let mut models = Models::new(pack);
        let r = models.resources().unwrap();
        let mut varps = vec![0; npc.multivarp as usize + 1];
        varps[npc.multivarp as usize] = index as i32;
        models.sync_player_varps(Some(&varps));
        let f = Fields {
            model: id,
            modelkind: 6,
            ..Default::default()
        };
        let selected = models
            .npc_body(&f, None, None, &state, &r)
            .unwrap()
            .expect("selected variant body");
        assert!(models.npc_body_models.contains_key(&(target as i32, 0)));
        assert!(!models.npc_body_models.contains_key(&(id, 0)));
        let direct = crate::npc_type_model::body(
            npcs.get(target).unwrap(),
            None,
            npcs.get(target).unwrap().bas,
            crate::npc_type_model::BASE_FLAGS,
            &models.npc_assets(&r),
        )
        .unwrap()
        .unwrap();
        assert_eq!(selected.vertex_count, direct.vertex_count);
        assert_eq!(selected.face_colour, {
            let mut d = direct;
            crate::npc_type_model::resize(npcs.get(target).unwrap(), &mut d);
            d.face_colour
        });
        // A -1 slot (or -1 default) is a null variant: nothing draws.
        if let Some(null_index) = npc.multinpc[..npc.multinpc.len() - 1]
            .iter()
            .position(|&v| v == -1)
        {
            varps[npc.multivarp as usize] = null_index as i32;
            models.sync_player_varps(Some(&varps));
            assert!(models
                .npc_body(&f, None, None, &state, &r)
                .unwrap()
                .is_none());
        }
    }
}

/// The component model kinds drawn through [`Models::draw`] over the real
/// cache: the default cache-model kind, kit heads, animated heads and items.
#[cfg(test)]
mod component_model_tests {
    use super::*;
    use crate::ui_components::Component;
    use crate::ui_model_animation::Animations;
    use std::cell::RefCell;

    struct Fixture {
        models: Models,
        state: crate::ui_properties::State,
        resources: Rc<Resources>,
    }

    fn fixture() -> Fixture {
        let pack =
            Pack::open(rs910_core::test_support::client_dir().join("../../server/data/pack"));
        let mut state = crate::ui_runtime::Runtime::load_state(&pack).expect("interface state");
        state.objs = Some(Rc::new(ObjStore::load(&pack).expect("item types")));
        state.npcs = Some(Rc::new(NpcStore::load(&pack).expect("npc types")));
        let mut models = Models::new(pack);
        let resources = models.resources().expect("model resources");
        Fixture {
            models,
            state,
            resources,
        }
    }

    fn component(fields: Fields) -> Ref {
        Rc::new(RefCell::new(Component {
            f: Fields {
                r#type: 6,
                width: 100,
                height: 100,
                modelzoom: 1000,
                ..fields
            },
            ..Component::default()
        }))
    }

    fn draw(fx: &mut Fixture, c: &Ref) -> Option<Draw> {
        fx.models
            .draw(
                c,
                &mut fx.state,
                ModelPlacement {
                    at: [10, 10],
                    canvas: [800, 600],
                    clip: [0, 0, 800, 600],
                    quad: 0,
                    before_scene: true,
                },
            )
            .expect("model draw")
    }

    /// Install `model` as the local player (id 5) of the model owner.
    fn local_player(fx: &mut Fixture, model: Model) {
        fx.models.local = 5;
        fx.models.players.insert(
            5,
            Appearance {
                name: Some("Tester".into()),
                model: Some(model),
                ..Default::default()
            },
        );
    }

    fn appearance(kits: Vec<i32>) -> Model {
        Model {
            bas: -1,
            kits,
            custom: vec![],
            colours: [0; 10],
            textures: [0; 10],
            female: false,
            npc: -1,
            hash: 0x1234,
        }
    }

    /// Three identity kits that own head models, one per body part.
    fn head_kits(fx: &Fixture) -> [i32; 3] {
        let mut picks: Vec<i32> = Vec::new();
        let mut parts = Vec::new();
        for id in 0..2000u32 {
            let Some(idk) = fx.resources.avatar_idks.get(id) else {
                continue;
            };
            if idk.heads[0] >= 0 && !parts.contains(&idk.bodypart) {
                parts.push(idk.bodypart);
                picks.push(id as i32);
                if picks.len() == 3 {
                    break;
                }
            }
        }
        picks.try_into().expect("three kits with head models")
    }

    /// The original draws the cache model a component names (kind 1) and
    /// applies the component's own recolour; a missing model id draws nothing.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn kind_one_draws_the_cache_model_and_applies_component_recolour() {
        let mut fx = fixture();
        // The two models of the lobby popup.
        let plain = component(Fields {
            model: 24207,
            ..Default::default()
        });
        let drawn = draw(&mut fx, &plain).expect("cache model draws");
        assert!(drawn.model.vertex_count > 0 && drawn.model.face_count > 0);
        let first_colour = drawn.model.face_colour[0];
        assert!(draw(
            &mut fx,
            &component(Fields {
                model: -1,
                ..Default::default()
            })
        )
        .is_none());
        assert!(draw(
            &mut fx,
            &component(Fields {
                model: 0x7fff_0000,
                ..Default::default()
            })
        )
        .is_none());
        let tinted = component(Fields {
            model: 24207,
            ..Default::default()
        });
        tinted.borrow_mut().recolour = Some(([first_colour, 0, 0, 0, 0], [12345, 0, 0, 0, 0]));
        let recoloured = draw(&mut fx, &tinted).expect("recoloured model");
        assert_eq!(recoloured.model.face_colour[0], 12345);
        assert_eq!(recoloured.model.face_count, drawn.model.face_count);
        // The plain component is unchanged by the other's colours.
        assert_eq!(
            draw(&mut fx, &plain).unwrap().model.face_colour[0],
            first_colour
        );
        // A cache model sits at the component origin: no half-height shift.
        assert_eq!(drawn.space.matrix[13], 0.0, "model matrix y translation");
    }

    /// Item models take half their height as a vertical shift; every other
    /// model kind keeps its own origin.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn only_item_models_are_shifted_by_half_their_height() {
        let mut fx = fixture();
        let item = component(Fields {
            invobject: 1205,
            invcount: 1,
            ..Default::default()
        });
        let mut drawn = draw(&mut fx, &item).expect("item model");
        let top = drawn.model.min_y();
        assert_eq!(
            drawn.space.matrix[13],
            (top.wrapping_neg() >> 1) as f32,
            "item shift is minus the top, halved"
        );
        let plain = component(Fields {
            model: 24207,
            ..Default::default()
        });
        assert_eq!(draw(&mut fx, &plain).unwrap().space.matrix[13], 0.0);
    }

    /// An item model follows the stack size: a large stack of coins draws the
    /// coin-pile model, not the single coin.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn item_models_follow_the_stack_size() {
        let mut fx = fixture();
        let coins = |count| {
            component(Fields {
                invobject: rs910_symbols::obj::COINS.id(),
                invcount: count,
                ..Default::default()
            })
        };
        let single = draw(&mut fx, &coins(1)).expect("single coin");
        let pile = draw(&mut fx, &coins(100_000)).expect("coin pile");
        assert_ne!(single.model.position_stream(), pile.model.position_stream());
    }

    /// Model kind 7 builds a head from the three kit ids the component packs
    /// (first in the high half of `model`, second in the low half, third in
    /// the name hash), coloured with the local player; with no local player
    /// nothing draws. It equals the head of a player wearing those kits.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn kind_seven_builds_the_head_from_three_kit_ids() {
        let mut fx = fixture();
        let [a, b, c] = head_kits(&fx);
        let kit_head = component(Fields {
            modelkind: 7,
            model: (a << 16) | b,
            modelNameHash: c,
            ..Default::default()
        });
        assert!(draw(&mut fx, &kit_head).is_none(), "no local player");
        let kits = vec![a | i32::MIN, b | i32::MIN, c | i32::MIN];
        local_player(&mut fx, appearance(kits));
        let from_kits = draw(&mut fx, &kit_head).expect("kit head draws");
        assert!(from_kits.model.vertex_count > 0);
        let worn = component(Fields {
            modelkind: 3,
            model: 5,
            modelNameHash: 0,
            ..Default::default()
        });
        let player_head = draw(&mut fx, &worn).expect("player head draws");
        assert_eq!(
            from_kits.model.position_stream(),
            player_head.model.position_stream()
        );
        assert_eq!(from_kits.model.face_colour, player_head.model.face_colour);
        // Different kits make a different head.
        let other = component(Fields {
            modelkind: 7,
            model: (b << 16) | b,
            modelNameHash: b,
            ..Default::default()
        });
        let other = draw(&mut fx, &other).expect("other kit head");
        assert_ne!(
            other.model.position_stream(),
            from_kits.model.position_stream()
        );
    }

    /// A player head (kind 3) takes the component's animator like the other
    /// model kinds: the cached head widens with the render flags and the
    /// per-draw copy moves between frames.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn player_heads_animate_with_the_component_animator() {
        let mut fx = fixture();
        let [a, b, c] = head_kits(&fx);
        local_player(
            &mut fx,
            appearance(vec![a | i32::MIN, b | i32::MIN, c | i32::MIN]),
        );
        let head = component(Fields {
            modelkind: 3,
            model: 5,
            ..Default::default()
        });
        let still = draw(&mut fx, &head).expect("still head").model;
        let service: Rc<RefCell<Animations>> = fx.state.model_animations.clone().unwrap();
        let candidates: Vec<i32> = service
            .borrow()
            .assets
            .sequences
            .values()
            .filter(|s| s.skeletal == -1 && s.frame_ids.as_ref().is_some_and(|f| f.len() >= 3))
            .take(400)
            .map(|s| s.id)
            .collect();
        let mut moved = None;
        for sequence in candidates {
            let mut playback = crate::animation_assets::Playback::default();
            service.borrow_mut().start(&mut playback, sequence).unwrap();
            head.borrow_mut().model_animator = Some(playback);
            let a = draw(&mut fx, &head).expect("animated head").model;
            if a.position_stream() != still.position_stream() {
                moved = Some((sequence, a));
                break;
            }
        }
        let (sequence, first) = moved.expect("an emote that moves the head");
        // A later frame of the same emote is another pose.
        let frames = service.borrow().assets.sequences[&sequence]
            .frame_ids
            .as_ref()
            .unwrap()
            .len();
        let mut later = None;
        for frame in 1..frames {
            head.borrow_mut()
                .model_animator
                .as_mut()
                .unwrap()
                .node
                .frame = frame as i32;
            let m = draw(&mut fx, &head).expect("later frame").model;
            if m.position_stream() != first.position_stream() {
                later = Some(m);
                break;
            }
        }
        assert!(later.is_some(), "a later frame differs from the first");
        // The cached head keeps serving unanimated draws unchanged.
        head.borrow_mut().model_animator = None;
        assert_eq!(
            draw(&mut fx, &head).unwrap().model.position_stream(),
            still.position_stream()
        );
        assert!(
            fx.models
                .player_head_models
                .get(&(5, 0x1234))
                .is_some_and(|m| m.vertex_groups.is_some()),
            "the cached head carries label groups after an animated draw"
        );
    }

    /// FNV-1a 64 of what a draw puts on screen: the animated vertex
    /// positions, the face colours and the model matrix.
    fn draw_digest(draw: &Draw) -> u64 {
        let mut bytes = Vec::new();
        for position in draw.model.position_stream() {
            for v in position {
                bytes.extend(v.to_bits().to_le_bytes());
            }
        }
        for colour in &draw.model.face_colour {
            bytes.extend(colour.to_le_bytes());
        }
        for v in draw.space.matrix {
            bytes.extend(v.to_bits().to_le_bytes());
        }
        rs910_core::test_support::frozen::fnv64(&bytes)
    }

    /// Every sixth model-kind-1 component of the cache's interfaces, deterministic
    /// order: `(interface, component, model id)`.
    fn kind_one_sample(pack: &Pack) -> Vec<(i32, i32, i32)> {
        let mut store = crate::ui_components::Store::from_pack(pack.clone()).expect("store");
        let mut found = Vec::new();
        for id in 0..4000 {
            if !store.open(id, None).unwrap_or(false) {
                continue;
            }
            let iface = store.interfaces[&id].borrow();
            for c in iface.components.borrow().iter().flatten() {
                let c = c.borrow();
                if c.f.r#type == 6 && c.f.modelkind == 1 && c.f.model >= 0 && c.f.invobject == -1 {
                    found.push((id, c.f.id, c.f.model));
                }
            }
        }
        found.into_iter().step_by(6).collect()
    }

    /// A sample of the cache's fixed-model components (about 5,000 of them
    /// carry a model id) draws, every model present, with a stable digest.
    /// Record with `RS910_RECORD=1` after an intended change.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn cache_model_components_across_the_cache_draw() {
        let mut fx = fixture();
        let sample = kind_one_sample(&fx.models.pack);
        assert!(sample.len() > 100, "{} sampled components", sample.len());
        let mut lines = String::new();
        let mut drawn = 0;
        for (iface, com, model) in sample {
            let c = component(Fields {
                model,
                ..Default::default()
            });
            match draw(&mut fx, &c) {
                Some(d) => {
                    assert!(
                        d.model.vertex_count > 0,
                        "{iface}:{com} model {model} empty"
                    );
                    drawn += 1;
                    lines.push_str(&format!("{iface}:{com} {model} {:016x}\n", draw_digest(&d)));
                }
                None => lines.push_str(&format!("{iface}:{com} {model} none\n")),
            }
        }
        assert!(drawn * 10 > lines.lines().count() * 9, "most models draw");
        let name = "ui-component-models/kind-one-sample";
        if std::env::var_os("RS910_RECORD").is_some() {
            let path = rs910_core::test_support::frozen::store().join(format!("{name}.bin"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, lines.as_bytes()).unwrap();
        }
        rs910_core::test_support::frozen::assert_stream(name, lines.as_bytes());
    }
}
