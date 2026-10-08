//! Unanimated NPC body and head model owners.
//!
//! One builder serves the scene NPC renderer, the interface model kinds 2/6 and transformed
//! players. It resolves `multinpc` through the live player var domain, then applies the recolour
//! list (including the `recol_d_palette` indirection), the retexture list and the type tint, in
//! that order. Animation, head turn and the `resizeh`/`resizev` scale stay with the caller: they
//! apply to the per-frame copy after the cached base model.
use crate::{
    billboard::BillboardStore,
    cache::Pack,
    config::{Npc, NpcStore},
    gpumodel::GpuModel,
    modelunlit::ModelUnlit,
    npc_customisation::NpcCustomisation,
    particle::EmitterStore,
    protocol910::bas_types::Bas,
    texture::MaterialStore,
};
use anyhow::{Context, Result};
use std::{borrow::Cow, collections::BTreeMap};

/// The client palette table. Nothing ever fills it, so every palette-indexed recolour
/// destination is zero.
pub const CLIENT_PALETTE: [i16; 256] = [0; 256];

/// Model sources shared by every NPC model owner.
pub struct Assets<'a> {
    pub pack: &'a Pack,
    pub materials: &'a MaterialStore,
    pub billboards: &'a BillboardStore,
    pub emitters: &'a EmitterStore,
    /// The loaded BAS types; `None` skips slot transforms like a missing BAS.
    pub bases: Option<&'a BTreeMap<i32, Bas>>,
    /// Model detail flags for every model built here.
    pub detail: i32,
}

/// Look a type up by id: a missing file yields a default-constructed type, never null.
pub fn list(store: &NpcStore, id: u32) -> Result<Cow<'_, Npc>> {
    Ok(match store.get(id) {
        Some(npc) => Cow::Borrowed(npc),
        None => Cow::Owned(crate::config::decode_npc(id, &[0])?),
    })
}

/// Follow `multinpc` selection the way the body and head builders recurse: each selected type
/// is checked for its own `multinpc` again. `None` is the null model (no type selected).
pub fn resolve<'s>(
    store: &'s NpcStore,
    npc: Cow<'s, Npc>,
    read: &dyn Fn(bool, i32) -> Option<i32>,
) -> Result<Option<Cow<'s, Npc>>> {
    let mut current = npc;
    // A cyclic multinpc chain would recurse forever; bound it so a malformed cache reports
    // instead of hanging the client.
    for _ in 0..64 {
        if current.multinpc.is_empty() {
            return Ok(Some(current));
        }
        let Some(id) = current.multi_npc(read) else {
            return Ok(None);
        };
        current = list(store, id)?;
    }
    anyhow::bail!("NPC multinpc chain does not terminate from {}", current.id)
}

/// Apply the recolour, retexture and tint lists, in that order.
fn recolour(
    model: &mut GpuModel,
    npc: &Npc,
    custom: Option<&NpcCustomisation>,
    materials: &MaterialStore,
) -> Result<()> {
    if !npc.recol_s.is_empty() {
        let recol_d: Vec<i16> = custom
            .and_then(|c| c.custom_recol_d.clone())
            .unwrap_or_else(|| npc.recol_d.iter().map(|&v| v as i16).collect());
        for (index, &src) in npc.recol_s.iter().enumerate() {
            if index >= npc.recol_d_palette.len() {
                let dst = *recol_d
                    .get(index)
                    .context("NPC recol_d shorter than recol_s")?;
                model.recolor(src as i16, dst);
            } else {
                let slot = (npc.recol_d_palette[index] as i32 & 0xFF) as usize;
                model.recolor(src as i16, CLIENT_PALETTE[slot]);
            }
        }
    }
    if !npc.retex_s.is_empty() {
        let retex_d: Vec<i16> = custom
            .and_then(|c| c.custom_retex_d.clone())
            .unwrap_or_else(|| npc.retex_d.iter().map(|&v| v as i16).collect());
        for (index, &src) in npc.retex_s.iter().enumerate() {
            let dst = *retex_d
                .get(index)
                .context("NPC retex_d shorter than retex_s")?;
            model.retexture(materials, src as i16, dst)?;
        }
    }
    if npc.tint_weight != 0 {
        model.tint(
            i32::from(npc.tint_hue),
            i32::from(npc.tint_saturation),
            i32::from(npc.tint_luminence),
            i32::from(npc.tint_weight) & 0xFF,
        );
    }
    Ok(())
}

/// The capability flags requested for every NPC body and head before animation widening. The
/// flags the original build requests (including the resize flags) all lie inside this superset,
/// which the scene and interface owners have always used for NPC models.
pub const BASE_FLAGS: i32 = 0x1F01F;

/// The requested flags for body and head builds: the base flags widened by every active
/// animation's render flags (0x20 label transforms, 0x80/0x100/0x400 colour, alpha and
/// billboard ops, 0x200 extra normals). Without 0x20 the built model has no vertex label
/// groups and `apply_animation` cannot move a vertex.
/// `anim_flags` is the OR of the prepared poses' flags (`player_pose::
/// Prepared::flags`, `animation_assets::Pose::flags`).
pub fn sequenced_flags(anim_flags: i32) -> i32 {
    BASE_FLAGS | anim_flags
}

/// The model-cache reuse test for body and head builds: `None` reuses the cached model, which
/// carries every requested flag; `Some(flags)` rebuilds with `requested | cached.flags`, so a
/// type's cached model only ever gains capabilities.
pub fn rebuild_flags(cached: Option<&GpuModel>, requested: i32) -> Option<i32> {
    match cached {
        Some(model) if model.flags & requested == requested => None,
        Some(model) => Some(requested | model.flags),
        None => Some(requested),
    }
}

/// Widen the requested flags with the recolour, retexture and tint capability bits the type needs.
fn model_flags(npc: &Npc, base: i32) -> i32 {
    let mut flags = base;
    if !npc.recol_s.is_empty() {
        flags |= 0x4000;
    }
    if !npc.retex_s.is_empty() {
        flags |= 0x8000;
    }
    if npc.tint_weight != 0 {
        flags |= 0x80000;
    }
    flags
}

fn merge(parts: Vec<Option<ModelUnlit>>) -> Option<ModelUnlit> {
    if parts.is_empty() {
        return None;
    }
    if parts.len() == 1 {
        return parts.into_iter().next().unwrap();
    }
    let refs: Vec<Option<&ModelUnlit>> = parts.iter().map(Option::as_ref).collect();
    Some(ModelUnlit::merge_slots(&refs))
}

/// The cached base model of an already resolved type. `bas` is the BAS the caller chose.
/// `Ok(None)` means no model (a model group not yet downloadable). `flags` is the requested
/// set (from `sequenced_flags` and the cache flags); the recolour/retexture/tint bits are added
/// here.
pub fn body(
    npc: &Npc,
    custom: Option<&NpcCustomisation>,
    bas: i32,
    flags: i32,
    assets: &Assets<'_>,
) -> Result<Option<GpuModel>> {
    let slots = custom.map_or_else(|| npc.model_slots.clone(), |c| c.models.clone());
    let mut parts = Vec::with_capacity(slots.len());
    for (slot, &model_id) in slots.iter().enumerate() {
        let Ok(model_id) = u32::try_from(model_id) else {
            parts.push(None);
            continue;
        };
        let Ok(mut raw) = ModelUnlit::load(assets.pack, model_id) else {
            return Ok(None);
        };
        if raw.version < 13 {
            raw.scale_by_power_of_two(2);
        }
        if let Some([x, y, z]) = npc
            .modeloffset
            .as_ref()
            .and_then(|offsets| offsets.get(slot).copied().flatten())
        {
            raw.translate(x, y, z);
        }
        parts.push(Some(raw));
    }
    if let Some(c) = custom {
        for (slot, raw) in parts.iter_mut().enumerate() {
            let Some(raw) = raw else { continue };
            if c.custom_scale.get(slot).is_some_and(|&s| s != 0.0) {
                raw.scale_by(c.custom_scale[slot]);
            }
            let [x, y, z] = c.custom_rotation.get(slot).copied().unwrap_or([0; 3]);
            anyhow::ensure!(
                [x, y, z].iter().all(|a| (0..16384).contains(a)),
                "NPC rotation out of range: {:?}",
                [x, y, z]
            );
            raw.rotate(x, y, z);
            let [x, y, z] = c.custom_offset.get(slot).copied().unwrap_or([0; 3]);
            raw.translate(x, y, z);
        }
    }
    if let Some(transforms) = assets
        .bases
        .and_then(|bases| bases.get(&bas))
        .and_then(|bas| bas.slot_transforms.as_ref())
    {
        for (slot, transform) in transforms.iter().enumerate() {
            let Some(Some(raw)) = parts.get_mut(slot) else {
                continue;
            };
            let t = transform.as_deref().unwrap_or(&[0; 6]);
            let rotation = [t[3] << 3, t[4] << 3, t[5] << 3];
            if rotation != [0; 3] {
                anyhow::ensure!(
                    rotation.iter().all(|a| (0..16384).contains(a)),
                    "NPC rotation out of range: {rotation:?}"
                );
                raw.rotate(rotation[0], rotation[1], rotation[2]);
            }
            if t[..3] != [0; 3] {
                raw.translate(t[0], t[1], t[2]);
            }
        }
    }
    let Some(raw) = merge(parts) else {
        return Ok(None);
    };
    let mut model = GpuModel::new(
        &crate::gpumodel::ModelStores {
            materials: assets.materials,
            billboards: assets.billboards,
            emitters: assets.emitters,
        },
        &raw,
        crate::gpumodel::BuildParams {
            flags: model_flags(npc, flags),
            ambient: npc.ambient + 64,
            contrast: npc.contrast * 5 + 850,
            detail: assets.detail,
        },
    )?;
    recolour(&mut model, npc, custom, assets.materials)?;
    Ok(Some(model))
}

/// The cached head model of an already resolved type. Heads (or the customisation models) are
/// lit at 64/768 and never resized. `Ok(None)` means no model. `flags` is the requested set,
/// widened here by the recolour/retexture bits.
pub fn head(
    npc: &Npc,
    custom: Option<&NpcCustomisation>,
    flags: i32,
    assets: &Assets<'_>,
) -> Result<Option<GpuModel>> {
    let slots: Vec<i32> = match custom {
        Some(c) => c.models.clone(),
        None => match &npc.head_slots {
            Some(heads) => heads.clone(),
            None => return Ok(None),
        },
    };
    let mut parts = Vec::with_capacity(slots.len());
    for &model_id in &slots {
        let raw = match u32::try_from(model_id) {
            Ok(model_id) => match ModelUnlit::load(assets.pack, model_id) {
                Ok(mut raw) => {
                    if raw.version < 13 {
                        raw.scale_by_power_of_two(2);
                    }
                    Some(raw)
                }
                // The model group is not downloadable yet: report no model and retry later.
                Err(_) => return Ok(None),
            },
            Err(_) => None,
        };
        parts.push(raw);
    }
    let Some(raw) = merge(parts) else {
        return Ok(None);
    };
    let mut model = GpuModel::new(
        &crate::gpumodel::ModelStores {
            materials: assets.materials,
            billboards: assets.billboards,
            emitters: assets.emitters,
        },
        &raw,
        crate::gpumodel::BuildParams {
            flags: model_flags(npc, flags),
            ambient: 64,
            contrast: 768,
            detail: assets.detail,
        },
    )?;
    recolour(&mut model, npc, custom, assets.materials)?;
    Ok(Some(model))
}

/// The NPC_INFO mask 0x400 customisation object in the shape `body` consumes. Absent arrays keep
/// their meaning: absent `models` selects the models of the (multinpc resolved) type, an
/// absent scale/rotation/offset array skips that transform (an empty list here), and absent
/// recolour/retexture lists fall back to the type's.
pub fn body_customisation(
    npc: &Npc,
    custom: &crate::entities910::npc_custom::Custom,
) -> NpcCustomisation {
    NpcCustomisation {
        cache_key_salt: custom.salt,
        models: custom
            .models
            .clone()
            .unwrap_or_else(|| npc.model_slots.clone()),
        custom_scale: custom.scale_bits.as_ref().map_or_else(Vec::new, |bits| {
            bits.iter().map(|&b| f32::from_bits(b)).collect()
        }),
        custom_rotation: custom.rotation.clone().unwrap_or_default(),
        custom_offset: custom.offset.clone().unwrap_or_default(),
        custom_recol_d: custom.colours.clone(),
        custom_retex_d: custom.textures.clone(),
    }
}

/// The salt mixed into the body model-cache key (`id | toolkitId << 16`, plus the salt shifted
/// left 24 when a customisation is passed). Salts start at 1, so 0 is "none".
pub fn body_cache_salt(custom: Option<&crate::entities910::npc_custom::Custom>) -> i64 {
    custom.map_or(0, |c| c.salt)
}

/// The per-frame `resizeh`/`resizev` scale.
pub fn resize(npc: &Npc, model: &mut GpuModel) {
    if npc.resize_x != 128 || npc.resize_y != 128 {
        model.scale(npc.resize_x, npc.resize_y, npc.resize_x);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn npc(id: u32) -> Npc {
        crate::config::decode_npc(id, &[0]).unwrap()
    }

    /// The network customisation keeps its absent-array meanings.
    #[test]
    fn body_customisation_keeps_null_fallbacks() {
        use crate::entities910::npc_custom::Custom;
        let mut base = npc(1);
        base.model_slots = vec![100, 101];
        let colours_only = Custom {
            salt: 4,
            models: None,
            scale_bits: None,
            rotation: None,
            offset: None,
            colours: Some(vec![7, 8]),
            textures: None,
        };
        let c = body_customisation(&base, &colours_only);
        assert_eq!(c.models, vec![100, 101], "null models -> this.models");
        assert!(
            c.custom_scale.is_empty() && c.custom_rotation.is_empty() && c.custom_offset.is_empty()
        );
        assert_eq!(c.custom_recol_d, Some(vec![7, 8]));
        assert_eq!(c.custom_retex_d, None);
        assert_eq!(c.cache_key_salt, 4);
        let transformed = Custom {
            salt: 5,
            models: Some(vec![200, -1]),
            scale_bits: Some(vec![1.5f32.to_bits(), 0]),
            rotation: Some(vec![[0, 2048, 0], [0; 3]]),
            offset: Some(vec![[1, 2, 3], [0; 3]]),
            colours: None,
            textures: None,
        };
        let c = body_customisation(&base, &transformed);
        assert_eq!(c.models, vec![200, -1]);
        assert_eq!(c.custom_scale, vec![1.5, 0.0]);
        assert_eq!(c.custom_rotation[0], [0, 2048, 0]);
        assert_eq!(c.custom_offset[0], [1, 2, 3]);
        assert_eq!(body_cache_salt(Some(&transformed)), 5);
        assert_eq!(body_cache_salt(None), 0);
    }

    #[test]
    fn multinpc_selection_follows_null_and_default_rules() {
        let mut base = npc(1);
        base.multivarp = 7;
        // multinpc = [10, -1, 12, default 13]
        base.multinpc = vec![10, -1, 12, 13];
        let read = |value: i32| move |bit: bool, id: i32| (!bit && id == 7).then_some(value);
        assert_eq!(base.multi_npc(&read(0)), Some(10));
        // An explicit -1 slot selects no type, not the default.
        assert_eq!(base.multi_npc(&read(1)), None);
        assert_eq!(base.multi_npc(&read(2)), Some(12));
        // Index == length - 1 and out-of-range values use the default.
        assert_eq!(base.multi_npc(&read(3)), Some(13));
        assert_eq!(base.multi_npc(&read(99)), Some(13));
        assert_eq!(base.multi_npc(&read(-5)), Some(13));
        // A missing var type leaves the index at -1.
        assert_eq!(base.multi_npc(&|_, _| None), Some(13));
        base.multinpc = vec![10, -1];
        assert_eq!(base.multi_npc(&read(5)), None);
        // Varbit wins over varp when both are set.
        base.multivarbit = 3;
        base.multinpc = vec![20, 21, 22];
        assert_eq!(
            base.multi_npc(&|bit, id| (bit && id == 3).then_some(1)),
            Some(21)
        );
    }

    #[test]
    fn resolve_recurses_through_chained_multinpc_types() {
        let mut first = npc(1);
        first.multivarp = 1;
        first.multinpc = vec![2, -1];
        let mut second = npc(2);
        second.multivarp = 2;
        second.multinpc = vec![3, 3, -1];
        let third = npc(3);
        let store = NpcStore::from_map(BTreeMap::from([
            (1, first.clone()),
            (2, second),
            (3, third),
        ]));
        let read = |_: bool, id: i32| Some(if id == 1 { 0 } else { 1 });
        let resolved = resolve(&store, Cow::Borrowed(&first), &read)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.id, 3);
        let null = |_: bool, id: i32| Some(if id == 1 { 0 } else { 2 });
        assert!(resolve(&store, Cow::Borrowed(&first), &null)
            .unwrap()
            .is_none());
        // A missing type is list()'s default-constructed type.
        let mut missing = npc(9);
        missing.multinpc = vec![4000, -1];
        missing.multivarp = 1;
        let resolved = resolve(&store, Cow::Borrowed(&missing), &read)
            .unwrap()
            .unwrap();
        assert_eq!((resolved.id, resolved.name.as_str()), (4000, "null"));
    }

    #[test]
    fn decode_retains_recolour_palette_and_tint() {
        // 40: recol 0x1111->0x2222, 0x3333->0x4444. 42: palette [5].
        // 155: tint (-1, 3, 90, weight -128).
        let bytes = [
            40, 2, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33, 0x44, 0x44, 42, 1, 5, 155, 0xff, 3, 90,
            0x80, 0,
        ];
        let npc = crate::config::decode_npc(7, &bytes).unwrap();
        assert_eq!(npc.recol_d_palette, vec![5]);
        assert_eq!(
            (
                npc.tint_hue,
                npc.tint_saturation,
                npc.tint_luminence,
                npc.tint_weight
            ),
            (-1, 3, 90, -128)
        );
        assert_eq!(model_flags(&npc, 0), 0x4000 | 0x80000);
    }

    /// Reuse only a cached model that carries every requested flag, else rebuild with the union.
    #[test]
    fn rebuild_flags_follow_the_model_cache_test() {
        let mut cached = GpuModel::new(
            &crate::gpumodel::ModelStores {
                materials: &Default::default(),
                billboards: &Default::default(),
                emitters: &Default::default(),
            },
            &Default::default(),
            crate::gpumodel::BuildParams {
                flags: 0,
                ambient: 64,
                contrast: 768,
                detail: 0,
            },
        )
        .unwrap();
        assert_eq!(rebuild_flags(None, sequenced_flags(0)), Some(BASE_FLAGS));
        cached.flags = BASE_FLAGS | 0x4000;
        assert_eq!(rebuild_flags(Some(&cached), sequenced_flags(0)), None);
        // An animation adding label groups rebuilds with the cached flags kept.
        assert_eq!(
            rebuild_flags(Some(&cached), sequenced_flags(0x20 | 0x100)),
            Some(BASE_FLAGS | 0x4000 | 0x20 | 0x100)
        );
        cached.flags |= 0x20 | 0x100 | 0x80;
        assert_eq!(rebuild_flags(Some(&cached), sequenced_flags(0x20)), None);
    }

    /// A live NPC body requested with its stand animation's render flags carries the
    /// label groups (0x20), and two frames of that animation leave different
    /// vertices. The base flags alone (the port's request before lane
    /// F-NPCANIM) build a body the animation cannot move.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn real_pack_animated_npc_body_has_label_groups_and_moves_between_frames() {
        use crate::animation_assets::{AnimationAssets, Filter};
        use crate::entities910::animation_state::{Node, Sequence};
        let pack = pack();
        let store = NpcStore::load(&pack).unwrap();
        let materials = MaterialStore::load(&pack).unwrap();
        let billboards = BillboardStore::load(&pack).unwrap();
        let emitters = EmitterStore::load(&pack).unwrap();
        // The BAS type list (config group 32), as `pack_bas::load` decodes it.
        let bases: BTreeMap<i32, Bas> = pack
            .read_group("config", 32)
            .unwrap()
            .iter()
            .map(|(&id, raw)| (id as i32, Bas::decode(id as i32, raw).unwrap()))
            .collect();
        let mut animations = AnimationAssets::load(&pack).unwrap();
        let assets = Assets {
            pack: &pack,
            materials: &materials,
            billboards: &billboards,
            emitters: &emitters,
            bases: Some(&bases),
            detail: crate::gpumodel::MODEL_DETAIL_FLAGS,
        };
        let (npc, seq) = store
            .iter()
            .map(|(_, n)| n)
            .filter(|n| n.multinpc.is_empty() && !n.models.is_empty() && n.bas != -1)
            .find_map(|n| {
                let seq = animations.sequences.get(&bases.get(&n.bas)?.readyanim)?;
                (seq.skeletal == -1 && seq.frame_ids.as_ref()?.len() >= 3).then(|| (n, seq.clone()))
            })
            .expect("a cache NPC whose BAS stand animation has classic frames");
        let frames = seq.frame_ids.as_ref().unwrap().len();
        let mut node_at = |frame: usize| {
            let mut node = Node {
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
                next: if frame + 1 < frames {
                    frame as i32 + 1
                } else {
                    -1
                },
                ..Default::default()
            };
            animations
                .actor_pose(&pack, &mut node, Filter::default())
                .unwrap()
        };
        let first = node_at(0);
        let later = node_at(frames / 2);
        assert_ne!(first.flags & 0x20, 0, "classic frames request label groups");
        let unflagged = body(npc, None, npc.bas, BASE_FLAGS, &assets)
            .unwrap()
            .unwrap();
        assert!(unflagged.vertex_groups.is_none(), "npc {}", npc.id);
        let mut still = unflagged.clone();
        still.apply_animation(&first.transforms);
        assert_eq!(
            (&still.vx, &still.vy, &still.vz),
            (&unflagged.vx, &unflagged.vy, &unflagged.vz),
            "without 0x20 nothing moves"
        );
        let requested = sequenced_flags(first.flags | later.flags);
        let base = body(npc, None, npc.bas, requested, &assets)
            .unwrap()
            .unwrap();
        assert_eq!(base.flags & requested, requested);
        assert!(base.vertex_groups.is_some(), "npc {}", npc.id);
        let mut a = base.clone();
        a.apply_animation(&first.transforms);
        let mut b = base.clone();
        b.apply_animation(&later.transforms);
        assert_ne!(
            (&a.vx, &a.vy, &a.vz),
            (&base.vx, &base.vy, &base.vz),
            "npc {} frame 0 moves the body",
            npc.id
        );
        assert_ne!(
            (&a.vx, &a.vy, &a.vz),
            (&b.vx, &b.vy, &b.vz),
            "npc {} seq {} frames 0 and {} differ",
            npc.id,
            seq.id,
            frames / 2
        );
    }

    fn pack() -> Pack {
        Pack::open(rs910_core::test_support::pack_root())
    }

    /// Real cache: every NPC carrying a palette or a tint builds through the
    /// shared owner, and the resulting face colours equal the recolour-then-tint order applied to the untinted base.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn real_pack_palette_and_tint_reach_npc_face_colours() {
        let pack = pack();
        let store = NpcStore::load(&pack).unwrap();
        let materials = MaterialStore::load(&pack).unwrap();
        let billboards = BillboardStore::load(&pack).unwrap();
        let emitters = EmitterStore::load(&pack).unwrap();
        let assets = Assets {
            pack: &pack,
            materials: &materials,
            billboards: &billboards,
            emitters: &emitters,
            bases: None,
            detail: crate::gpumodel::MODEL_DETAIL_FLAGS,
        };
        let tinted = store
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.tint_weight != 0 && n.multinpc.is_empty() && !n.models.is_empty())
            .expect("a cache NPC with an opcode-155 tint");
        let model = body(tinted, None, tinted.bas, BASE_FLAGS, &assets)
            .unwrap()
            .expect("tinted body");
        let mut plain = tinted.clone();
        plain.tint_weight = 0;
        let mut expected = body(&plain, None, plain.bas, BASE_FLAGS, &assets)
            .unwrap()
            .unwrap();
        expected.tint(
            i32::from(tinted.tint_hue),
            i32::from(tinted.tint_saturation),
            i32::from(tinted.tint_luminence),
            i32::from(tinted.tint_weight) & 0xFF,
        );
        assert_eq!(model.face_colour, expected.face_colour, "npc {}", tinted.id);
        assert_ne!(
            model.face_colour,
            body(&plain, None, plain.bas, BASE_FLAGS, &assets)
                .unwrap()
                .unwrap()
                .face_colour,
            "npc {} tint changes colours",
            tinted.id
        );
        // Palette indirection: a real recoloured NPC whose first pair is
        // routed through `recol_d_palette` ends at clientpalette[slot] (0)
        // instead of its `recol_d` destination.
        let recoloured = store
            .iter()
            .map(|(_, n)| n)
            .find(|n| {
                n.recol_s.len() == 1
                    && n.recol_d[0] != 0
                    && n.tint_weight == 0
                    && n.multinpc.is_empty()
                    && !n.models.is_empty()
            })
            .expect("a cache NPC with one recolour pair");
        let mut plain = recoloured.clone();
        plain.recol_s.clear();
        plain.recol_d.clear();
        let base = body(&plain, None, plain.bas, BASE_FLAGS, &assets)
            .unwrap()
            .unwrap();
        let direct = body(recoloured, None, recoloured.bas, BASE_FLAGS, &assets)
            .unwrap()
            .unwrap();
        let mut paletted = recoloured.clone();
        paletted.recol_d_palette = vec![-3];
        let via_palette = body(&paletted, None, paletted.bas, BASE_FLAGS, &assets)
            .unwrap()
            .unwrap();
        let src = recoloured.recol_s[0] as i16;
        let changed: Vec<usize> = base
            .face_colour
            .iter()
            .enumerate()
            .filter(|(_, &c)| c == src)
            .map(|(face, _)| face)
            .collect();
        assert!(
            !changed.is_empty(),
            "npc {} has recoloured faces",
            recoloured.id
        );
        for &face in &changed {
            assert_eq!(direct.face_colour[face], recoloured.recol_d[0] as i16);
        }
        for face in changed {
            assert_eq!(via_palette.face_colour[face], CLIENT_PALETTE[253]);
        }
    }
    /// `GpuModel::copy_from` (the in-place copy the animated loc and actor
    /// paths use, programme Phase 6) leaves exactly `source.clone()`,
    /// whatever the destination held: real NPC bodies of different
    /// structure copied into each other, and into a model changed since.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn copy_from_equals_clone_on_real_models() {
        let pack = pack();
        let store = NpcStore::load(&pack).unwrap();
        let materials = MaterialStore::load(&pack).unwrap();
        let billboards = BillboardStore::load(&pack).unwrap();
        let emitters = EmitterStore::load(&pack).unwrap();
        let assets = Assets {
            pack: &pack,
            materials: &materials,
            billboards: &billboards,
            emitters: &emitters,
            bases: None,
            detail: crate::gpumodel::MODEL_DETAIL_FLAGS,
        };
        let models: Vec<GpuModel> = store
            .iter()
            .map(|(_, n)| n)
            .filter(|n| n.multinpc.is_empty() && !n.models.is_empty())
            .step_by(97)
            .take(24)
            .filter_map(|n| body(n, None, n.bas, BASE_FLAGS, &assets).ok().flatten())
            .collect();
        assert!(models.len() >= 12, "{} models", models.len());
        let show = |m: &GpuModel| format!("{m:?}");
        for source in &models {
            for dest in &models {
                let mut copy = dest.clone();
                copy.translate(3, -5, 7);
                copy.recolor(copy.face_colour.first().copied().unwrap_or(0), 17);
                copy.copy_from(source);
                assert_eq!(show(&copy), show(source));
            }
        }
    }
}
