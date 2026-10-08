//! Player body model construction from identity kits and worn items.
//! Body construction keeps wear-slot holes and the model/cache capabilities.
use crate::{
    cache::Pack,
    entities910::appearance::{Customisation, Model as Appearance},
    gpumodel::GpuModel,
    modelunlit::ModelUnlit,
    protocol910::{
        bas_types::Bas,
        config_types::Item,
        defaults::{Palette, Wear},
        idk_types::IdentityKit,
        sequence_types::Sequence,
    },
};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

/// How one body model is built: the render capability flags, whether the
/// result enters the model cache, and the key of the last cached build (the
/// fallback when a part is not ready yet).
pub struct BodyBuild<'a> {
    pub flags: i32,
    pub cache: bool,
    pub last_key: &'a mut i64,
}

pub struct Inputs<'a> {
    pub items: &'a BTreeMap<i32, Item>,
    pub bases: &'a BTreeMap<i32, Bas>,
    pub wear: &'a Wear,
    pub recolour: &'a Palette,
    pub retexture: &'a Palette,
}
#[derive(Clone, Debug)]
pub struct Selection {
    pub key: u64,
    pub kits: Vec<i32>,
    pub main_override: bool,
    pub off_override: bool,
}
/// Animation hand overrides change both the model key and dependent wear slots.
/// The offhand XOR sign-extends its 32-bit operand.
pub fn select(a: &Appearance, seq: Option<&Sequence>, w: &Wear) -> Result<Selection> {
    let mut s = Selection {
        key: a.hash,
        kits: a.kits.clone(),
        main_override: false,
        off_override: false,
    };
    if let Some(seq) = seq {
        for (id, slot, extra, main) in [
            (
                seq.mainhand,
                w.mainhand,
                w.mainhand_override_slots.as_ref(),
                true,
            ),
            (
                seq.offhand,
                w.offhand,
                w.offhand_override_slots.as_ref(),
                false,
            ),
        ] {
            if id < 0 || slot == -1 {
                continue;
            }
            if main {
                s.main_override = true;
            } else {
                s.off_override = true;
            }
            let kit = if id == 65535 { 0 } else { id | 0x40000000 };
            *s.kits
                .get_mut(slot as usize)
                .context("animation hand slot")? = kit;
            for &slot in extra.context("animation hand-dependent slots")? {
                *s.kits
                    .get_mut(slot as usize)
                    .context("animation dependent wear slot")? = 0;
            }
            let xor = if id == 65535 {
                u64::MAX
            } else {
                kit as i64 as u64
            };
            s.key ^= if main {
                xor << 32
            } else {
                if id == 65535 {
                    0xffffffff
                } else {
                    xor
                }
            };
        }
    }
    Ok(s)
}
/// The readiness and build checks use subset capability tests. Cache models
/// are shared bases; callers receive owned copies for per-actor deformation.
pub struct Models {
    pub identity: BTreeMap<i32, IdentityKit>,
    available: BTreeSet<u32>,
    raw: HashMap<u32, ModelUnlit>,
    cache: HashMap<u64, GpuModel>,
    order: VecDeque<u64>,
    pub detail: i32,
    /// Unanimated NPC body bases for NPC-transformed
    /// players, keyed by resolved type and transmog BAS (the NPC factory's
    /// model cache).
    pub npc_bodies: HashMap<(u32, i32), GpuModel>,
}
impl Models {
    #[cfg(any(test, feature = "test-hooks"))] // test-only introspection
    pub fn used_models(&self) -> Vec<u32> {
        let mut ids: Vec<_> = self.raw.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Change the model detail flags. Returns whether the
    /// effective setting changed, so callers can avoid needless invalidation.
    pub fn set_detail(&mut self, detail: i32) -> bool {
        if self.detail == detail {
            return false;
        }
        self.detail = detail;
        self.cache.clear();
        self.order.clear();
        self.npc_bodies.clear();
        true
    }

    pub fn load(pack: &Pack, detail: i32) -> Result<Self> {
        let identity = pack
            .read_group("config", 3)?
            .into_iter()
            .map(|(id, b)| {
                IdentityKit::decode(id as i32, &b)
                    .map(|s| (id as i32, s))
                    .map_err(|e| anyhow::anyhow!("identity kit: {e:?}"))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            identity,
            available: pack
                .read_archive_index("models")?
                .group_id
                .into_iter()
                .collect(),
            raw: HashMap::new(),
            cache: HashMap::new(),
            order: VecDeque::new(),
            detail,
            npc_bodies: HashMap::new(),
        })
    }
    fn source(&mut self, pack: &Pack, id: i32) -> Result<ModelUnlit> {
        let id = u32::try_from(id).context("negative body model ID")?;
        if let std::collections::hash_map::Entry::Vacant(slot) = self.raw.entry(id) {
            let files = pack.read_group("models", id)?;
            let mut raw = ModelUnlit::decode(files.get(&0).context("body model file zero")?)?;
            // Provenance for the NXT renderer's RT7 posing (as `ModelUnlit::load`).
            raw.source_ids = Some(std::sync::Arc::from([id]));
            if raw.version < 13 {
                raw.scale_by_power_of_two(2);
            }
            slot.insert(raw);
        }
        Ok(self.raw[&id].clone())
    }
    fn touch(&mut self, key: u64) {
        self.order.retain(|&k| k != key);
        self.order.push_back(key);
    }
    fn ready(&self, id: i32) -> bool {
        u32::try_from(id)
            .ok()
            .is_some_and(|id| self.available.contains(&id))
    }
    fn idk(&mut self, pack: &Pack, id: i32) -> Result<Option<ModelUnlit>> {
        let default = IdentityKit::default();
        let kit = self.identity.get(&id).unwrap_or(&default).clone();
        let Some(ids) = kit.models else {
            return Ok(None);
        };
        let parts = ids
            .into_iter()
            .map(|id| self.source(pack, id))
            .collect::<Result<Vec<_>>>()?;
        let mut m = if parts.len() == 1 {
            parts.into_iter().next().unwrap()
        } else {
            ModelUnlit::merge(&parts.iter().collect::<Vec<_>>())
        };
        for (src, dst) in kit
            .recolour
            .into_iter()
            .flat_map(|(a, b)| a.into_iter().zip(b))
        {
            m.recolor(src, dst);
        }
        for (src, dst) in kit
            .retexture
            .into_iter()
            .flat_map(|(a, b)| a.into_iter().zip(b))
        {
            m.rematerial(src, dst);
        }
        Ok(Some(m))
    }
    /// Whether every model of the worn item is available.
    pub fn wear_ready(&self, item: &Item, female: bool) -> bool {
        let ids = Self::worn_ids(item, None, female);
        ids[0] == -1 || ids.iter().all(|&id| id == -1 || self.ready(id))
    }
    /// Build the worn item's model (no customisation).
    pub fn wear_model(
        &mut self,
        pack: &Pack,
        item: &Item,
        female: bool,
    ) -> Result<Option<ModelUnlit>> {
        self.worn(pack, item, None, female)
    }
    fn worn_ids(item: &Item, custom: Option<&Customisation>, female: bool) -> [i32; 3] {
        let c = custom.unwrap_or(&item.custom);
        if female {
            c.woman
        } else {
            c.man
        }
    }
    fn worn(
        &mut self,
        pack: &Pack,
        item: &Item,
        custom: Option<&Customisation>,
        female: bool,
    ) -> Result<Option<ModelUnlit>> {
        let [a, b, c] = Self::worn_ids(item, custom, female);
        if a == -1 {
            return Ok(None);
        }
        let mut parts = vec![self.source(pack, a)?];
        if b != -1 {
            parts.push(self.source(pack, b)?);
            if c != -1 {
                parts.push(self.source(pack, c)?);
            }
        }
        let mut m = if parts.len() == 1 {
            parts.remove(0)
        } else {
            ModelUnlit::merge(&parts.iter().collect::<Vec<_>>())
        };
        let [x, y, z] = if female {
            item.woman_offset
        } else {
            item.man_offset
        };
        m.translate(x, y, z);
        let recolour = custom
            .and_then(|c| c.recolour.as_ref())
            .or(item.custom.recolour.as_ref());
        let retexture = custom
            .and_then(|c| c.retexture.as_ref())
            .or(item.custom.retexture.as_ref());
        for (src, dst, colour) in [
            (&item.recolour_source, recolour, true),
            (&item.retexture_source, retexture, false),
        ] {
            if let Some(src) = src {
                let dst = dst.context("wear material destinations")?;
                anyhow::ensure!(dst.len() >= src.len(), "wear material destination count");
                for (&a, &b) in src.iter().zip(dst) {
                    if colour {
                        m.recolor(a, b);
                    } else {
                        m.rematerial(a, b);
                    }
                }
            }
        }
        Ok(Some(m))
    }
    /// Model preparation before animation. `last_key` is owned by the player model
    /// (default zero), and only changes on a successfully cached build.
    /// Missing resources return None or the compatible previous cached model.
    pub fn body(
        &mut self,
        pack: &Pack,
        a: &Appearance,
        seq: Option<&Sequence>,
        c: &Inputs,
        stores: &crate::gpumodel::ModelStores<'_>,
        build: BodyBuild<'_>,
    ) -> Result<Option<GpuModel>> {
        let BodyBuild {
            mut flags,
            cache,
            last_key,
        } = build;
        let materials = stores.materials;
        anyhow::ensure!(
            a.npc == -1,
            "NPC-transformed player body requires NPC model builder"
        );
        let selected = select(a, seq, c.wear)?;
        let key = selected.key;
        let previous = self.cache.get(&key).cloned();
        if previous.is_some() {
            self.touch(key);
        }
        let mut model = previous;
        if model.as_ref().is_none_or(|m| (m.flags & flags) != flags) {
            if let Some(m) = &model {
                flags |= m.flags;
            }
            let default_item = Item::empty(-1);
            let mut ready = true;
            for (slot, &id) in selected.kits.iter().enumerate() {
                if id & 0x40000000 != 0 {
                    let item = c.items.get(&(id & 0x3fffffff)).unwrap_or(&default_item);
                    let overridden = (selected.main_override
                        && (c.wear.mainhand == slot as i32
                            || c.wear
                                .mainhand_override_slots
                                .as_ref()
                                .is_some_and(|a| a.contains(&(slot as i32)))))
                        || (selected.off_override
                            && (c.wear.offhand == slot as i32
                                || c.wear
                                    .offhand_override_slots
                                    .as_ref()
                                    .is_some_and(|a| a.contains(&(slot as i32)))));
                    let custom = if overridden {
                        None
                    } else {
                        a.custom.get(slot).and_then(Option::as_ref)
                    };
                    let ids = Self::worn_ids(item, custom, a.female);
                    if ids[0] != -1 {
                        for id in ids {
                            if id != -1 && !self.ready(id) {
                                ready = false;
                            }
                        }
                    }
                } else if id & i32::MIN != 0 {
                    if let Some(ids) = self
                        .identity
                        .get(&(id & 0x3fffffff))
                        .and_then(|i| i.models.as_ref())
                    {
                        if ids.iter().any(|&id| !self.ready(id)) {
                            ready = false;
                        }
                    }
                }
            }
            if !ready {
                if *last_key != -1 {
                    model = self.cache.get(&(*last_key as u64)).cloned();
                    if model.is_some() {
                        self.touch(*last_key as u64);
                    }
                }
                return Ok(model.filter(|m| (m.flags & flags) == flags).map(|mut m| {
                    m.flags = flags;
                    m
                }));
            }
            let mut slots = Vec::with_capacity(selected.kits.len());
            for (slot, &id) in selected.kits.iter().enumerate() {
                slots.push(if id & 0x40000000 != 0 {
                    let item = c.items.get(&(id & 0x3fffffff)).unwrap_or(&default_item);
                    // The build pass uses literal slots 5/3 here, unlike the
                    // readiness pass's default wear-position indices.
                    let overridden = (slot == 5 && selected.main_override)
                        || (slot == 3 && selected.off_override);
                    let custom = if overridden {
                        None
                    } else {
                        a.custom.get(slot).and_then(Option::as_ref)
                    };
                    self.worn(pack, item, custom, a.female)?
                } else if id & i32::MIN != 0 {
                    self.idk(pack, id & 0x3fffffff)?
                } else {
                    None
                });
            }
            if a.bas != -1 {
                if let Some(transforms) =
                    &c.bases.get(&a.bas).context("player BAS")?.slot_transforms
                {
                    for (slot, t) in transforms.iter().enumerate() {
                        if let Some(m) = slots.get_mut(slot).context("BAS model slot")?.as_mut() {
                            if let Some(t) = t {
                                anyhow::ensure!(t.len() == 6, "BAS slot transform length");
                                let angles = [t[3] << 3, t[4] << 3, t[5] << 3];
                                anyhow::ensure!(
                                    angles.iter().all(|a| (0..16384).contains(a)),
                                    "BAS rotation index"
                                );
                                m.rotate(angles[0], angles[1], angles[2]);
                                m.translate(t[0], t[1], t[2]);
                            }
                        }
                    }
                }
            }
            let raw =
                ModelUnlit::merge_slots(&slots.iter().map(Option::as_ref).collect::<Vec<_>>());
            let mut m = GpuModel::new(
                stores,
                &raw,
                crate::gpumodel::BuildParams {
                    flags: flags | 0x4000,
                    ambient: 64,
                    contrast: 850,
                    detail: self.detail,
                },
            )?;
            for (palette, indices, colour) in [
                (c.recolour, &a.colours, true),
                (c.retexture, &a.textures, false),
            ] {
                for (i, &index) in indices.iter().enumerate() {
                    for j in 0..4 {
                        if index < palette.destinations[i][j].len() as i32 {
                            let dst = *palette.destinations[i][j]
                                .get(index as usize)
                                .context("player palette index")?;
                            if colour {
                                m.recolor(palette.source[i][j], dst);
                            } else {
                                m.retexture(materials, palette.source[i][j], dst)?;
                            }
                        }
                    }
                }
            }
            // The height is computed before any actor deformation: retain
            // the original height even when later poses change the bounds.
            m.height();
            if cache {
                m.flags = flags;
                self.cache.insert(key, m.clone());
                self.touch(key);
                while self.order.len() > 260 {
                    let evicted = self.order.pop_front().unwrap();
                    self.cache.remove(&evicted);
                }
                *last_key = key as i64;
            }
            model = Some(m);
        }
        let mut model = model.unwrap();
        model.flags = flags;
        Ok(Some(model))
    }
}
