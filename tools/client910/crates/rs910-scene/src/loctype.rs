//! Assemble the lit model of one loc placement (base model build plus the dynamic model).
//!
//! The 500-entry raw-model cache retains the render capabilities of the geometry it holds;
//! the 50-entry animated placement cache lives in `dynamic_scene`. Models returned
//! to placements own their arrays. The client palette table is all zero, so palette
//! recolours map to 0.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::billboard::BillboardStore;
use crate::cache::Pack;
use crate::config::Loc;
use crate::floor::FloorHeights;
use crate::gpumodel::{GpuModel, TerrainHeights};
use crate::modelunlit::ModelUnlit;
use crate::particle::EmitterStore;
use crate::protocol910::zone_state::LocCustom;
use crate::texture::MaterialStore;

/// Loc shape ids.
pub mod shape {
    pub const WALL_STRAIGHT: i32 = 0;
    pub const WALL_DIAGONAL_CORNER: i32 = 1;
    pub const WALL_L: i32 = 2;
    pub const WALL_SQUARE_CORNER: i32 = 3;
    pub const WALLDECOR_STRAIGHT_NOOFFSET: i32 = 4;
    pub const WALLDECOR_STRAIGHT_OFFSET: i32 = 5;
    pub const WALLDECOR_DIAGONAL_OFFSET: i32 = 6;
    pub const WALLDECOR_DIAGONAL_NOOFFSET: i32 = 7;
    pub const WALLDECOR_DIAGONAL_BOTH: i32 = 8;
    pub const WALL_DIAGONAL: i32 = 9;
    pub const CENTREPIECE_STRAIGHT: i32 = 10;
    pub const CENTREPIECE_DIAGONAL: i32 = 11;
    pub const ROOF_STRAIGHT: i32 = 12;
    pub const ROOF_DIAGONAL_WITH_ROOFEDGE: i32 = 13;
    pub const ROOF_FLAT: i32 = 17;
    pub const ROOFEDGE_STRAIGHT: i32 = 18;
    pub const ROOFEDGE_SQUARE_CORNER: i32 = 21;
    pub const GROUND_DECOR: i32 = 22;

    /// Whether the shape is a wall decoration.
    #[must_use]
    pub fn is_wall_decor(s: i32) -> bool {
        (WALLDECOR_STRAIGHT_NOOFFSET..=WALLDECOR_DIAGONAL_BOTH).contains(&s)
    }
    /// Whether the shape is a roof.
    #[must_use]
    pub fn is_roof(s: i32) -> bool {
        (ROOF_STRAIGHT..=ROOF_FLAT).contains(&s)
    }
    /// Whether the shape is a roof edge.
    #[must_use]
    pub fn is_roof_edge(s: i32) -> bool {
        (ROOFEDGE_STRAIGHT..=ROOFEDGE_SQUARE_CORNER).contains(&s)
    }
}

/// Raw lit geometry cache, 500-entry LRU. Flags
/// survive cache hits; face sorting depends on its retained alpha capability.
#[derive(Debug, Default)]
pub struct ModelCache {
    raw: HashMap<i64, GpuModel>,
    order: VecDeque<i64>,
}
pub type SharedModelCache = Arc<Mutex<ModelCache>>;

/// Model source + decode cache.
pub struct ModelSource<'a> {
    pub pack: &'a Pack,
    pub materials: &'a MaterialStore,
    pub billboards: &'a BillboardStore,
    pub emitters: &'a EmitterStore,
    /// `modelDetailFlags` (`gpumodel::MODEL_DETAIL_FLAGS`).
    pub detail_flags: i32,
    cache: RefCell<HashMap<i32, Option<ModelUnlit>>>,
    pub model_cache: SharedModelCache,
}

impl<'a> ModelSource<'a> {
    #[must_use]
    pub fn new(
        pack: &'a Pack,
        materials: &'a MaterialStore,
        billboards: &'a BillboardStore,
        emitters: &'a EmitterStore,
        detail_flags: i32,
    ) -> Self {
        rs910_core::profile::scope!("model source new");
        Self {
            pack,
            materials,
            billboards,
            emitters,
            detail_flags,
            cache: RefCell::new(HashMap::new()),
            model_cache: Default::default(),
        }
    }

    pub fn with_model_cache(mut self, cache: SharedModelCache) -> Self {
        self.model_cache = cache;
        self
    }

    /// Model ids that resolved to a model, ascending.
    #[must_use]
    pub fn loaded_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .cache
            .borrow()
            .iter()
            .filter_map(|(&id, m)| m.as_ref().and(u32::try_from(id).ok()))
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Load and decode the model, applying the `version < 13` scale by a power of two
    /// (`2`); `None` when the model is absent.
    fn model(&self, id: i32) -> Option<ModelUnlit> {
        if let Some(m) = self.cache.borrow().get(&id) {
            return m.clone();
        }
        let decoded = if id < 0 {
            None
        } else {
            match ModelUnlit::load(self.pack, id as u32) {
                Ok(mut m) => {
                    if m.version < 13 {
                        m.scale_by_power_of_two(2);
                    }
                    Some(m)
                }
                Err(_) => None,
            }
        };
        self.cache.borrow_mut().insert(id, decoded.clone());
        decoded
    }
}

/// Build the base model for the requested flags, shape and angle (no customisation).
/// `None` where the loc has no model.
pub fn build_base_model(
    src: &ModelSource<'_>,
    loc: &Loc,
    capability_flags: i32,
    loc_shape: i32,
    rotation: i32,
) -> anyhow::Result<Option<GpuModel>> {
    build_base_model_custom(src, loc, capability_flags, loc_shape, rotation, None)
}

/// The base model build with the retained zone customisation
/// applied before the lit model enters the shared raw-model cache.
pub fn build_base_model_custom(
    src: &ModelSource<'_>,
    loc: &Loc,
    capability_flags: i32,
    loc_shape: i32,
    rotation: i32,
    custom: Option<&LocCustom>,
) -> anyhow::Result<Option<GpuModel>> {
    let light_ambient = i32::from(loc.ambient) + 64;
    let light_contrast = i32::from(loc.contrast) + 850;
    let mirrored = loc.mirror || (shape::WALL_L == loc_shape && rotation > 3);
    let Some((_, base_ids)) = loc
        .shape_models
        .iter()
        .find(|(s, _)| i32::from(*s) == loc_shape)
    else {
        return Ok(None);
    };
    let ids = custom
        .and_then(|custom| custom.models.as_deref())
        .unwrap_or(base_ids.as_slice());
    if ids.is_empty() {
        return Ok(None);
    }
    let mut requested = capability_flags;
    if mirrored {
        requested |= 0x10;
    }
    if rotation == 0 {
        if loc.resizex != 128 || loc.xoff != 0 {
            requested |= 1;
        }
        if loc.resizez != 128 || loc.zoff != 0 {
            requested |= 4;
        }
    } else {
        requested |= 0xD;
    }
    if loc.resizey != 128 || loc.yoff != 0 {
        requested |= 2;
    }
    if !loc.recol_s.is_empty() {
        requested |= 0x4000;
    }
    if !loc.retex_s.is_empty() {
        requested |= 0x8000;
    }
    if loc.tint_weight != 0 {
        requested |= 0x80000;
    }
    let mut key = ids.iter().fold(0i64, |key, &id| {
        key.wrapping_mul(67783).wrapping_add(id as i64)
    });
    if let Some(custom) = custom {
        key = key.wrapping_add(custom.salt);
    }
    let old = {
        let mut cache = src.model_cache.lock().unwrap();
        let old = cache.raw.get(&key).cloned();
        if old.is_some() {
            cache.order.retain(|&k| k != key);
            cache.order.push_back(key);
        }
        old
    };
    if let Some(old) = &old {
        if i32::from(old.ambient) != light_ambient {
            requested |= 0x1000;
        }
        if i32::from(old.contrast) != light_contrast {
            requested |= 0x2000;
        }
    }
    let mut built_model =
        if let Some(old) = old.as_ref().filter(|m| m.flags & requested == requested) {
            old.clone()
        } else {
            let mut parts = Vec::with_capacity(ids.len());
            for &id in ids {
                let Some(m) = src.model(id) else {
                    return Ok(None);
                };
                parts.push(m);
            }
            let unlit = if parts.len() > 1 {
                let refs: Vec<&ModelUnlit> = parts.iter().collect();
                ModelUnlit::merge(&refs)
            } else {
                parts.pop().expect("one part")
            };
            let flags = requested | 0x1F01F | old.as_ref().map_or(0, |m| m.flags);
            let model = GpuModel::new(
                &crate::gpumodel::ModelStores {
                    materials: src.materials,
                    billboards: src.billboards,
                    emitters: src.emitters,
                },
                &unlit,
                crate::gpumodel::BuildParams {
                    flags,
                    ambient: light_ambient,
                    contrast: light_contrast,
                    detail: src.detail_flags,
                },
            )?;
            let mut cache = src.model_cache.lock().unwrap();
            cache.raw.insert(key, model.clone());
            cache.order.retain(|&k| k != key);
            cache.order.push_back(key);
            while cache.order.len() > 500 {
                let old = cache.order.pop_front().unwrap();
                cache.raw.remove(&old);
            }
            model
        };
    // The clone's capabilities differ from those retained by the shared raw geometry.
    built_model.has_transparency |= requested & 0x100 != 0;
    built_model.ambient = light_ambient as i16;
    built_model.contrast = light_contrast as i16;
    if mirrored {
        built_model.mirror();
    }
    if shape::WALLDECOR_STRAIGHT_NOOFFSET == loc_shape && rotation > 3 {
        built_model.rotate_y(2048);
        built_model.translate(180, 0, -180);
    }
    match rotation & 0x3 {
        1 => built_model.rotate_y(4096),
        2 => built_model.rotate_y(8192),
        3 => built_model.rotate_y(12288),
        _ => {}
    }
    for recolour_index in 0..loc.recol_s.len() {
        let from = loc.recol_s[recolour_index] as i16;
        if let Some(to) = custom
            .and_then(|custom| custom.colours.as_ref())
            .and_then(|colours| colours.get(recolour_index))
        {
            built_model.recolor(from, *to);
        } else if loc.recol_d_palette.is_empty() || recolour_index >= loc.recol_d_palette.len() {
            built_model.recolor(from, loc.recol_d[recolour_index] as i16);
        } else {
            // clientpalette[...] is all zeros.
            built_model.recolor(from, 0);
        }
    }
    for retexture_index in 0..loc.retex_s.len() {
        let to = custom
            .and_then(|custom| custom.textures.as_ref())
            .and_then(|textures| textures.get(retexture_index))
            .copied()
            .unwrap_or(loc.retex_d[retexture_index] as i16);
        built_model.retexture(src.materials, loc.retex_s[retexture_index] as i16, to)?;
    }
    if loc.tint_weight != 0 {
        built_model.tint(
            i32::from(loc.tint_hue),
            i32::from(loc.tint_saturation),
            i32::from(loc.tint_luminence),
            i32::from(loc.tint_weight) & 0xFF,
        );
    }
    if loc.resizex != 128 || loc.resizey != 128 || loc.resizez != 128 {
        built_model.scale(loc.resizex, loc.resizey, loc.resizez);
    }
    if loc.xoff != 0 || loc.yoff != 0 || loc.zoff != 0 {
        built_model.translate(loc.xoff, loc.yoff, loc.zoff);
    }
    built_model.flags = capability_flags;
    Ok(Some(built_model))
}

/// The capability flags requested for every spot-animation model before animation widening;
/// the resize, orientation and hill-skew flags are all inside it.
pub const EFFECT_BASE_FLAGS: i32 = 0x1F01F;

/// The requested flags, widened by the
/// animation's render flags (0x20 label groups and the
/// colour/alpha/billboard/normal bits of its frames).
pub fn effect_flags(anim_flags: i32) -> i32 {
    EFFECT_BASE_FLAGS | anim_flags
}

/// Build the cached base model of a spot animation: the
/// model lit with the type's ambient/contrast, recoloured and retextured.
/// `flags` is the requested set after the model-cache test
/// (`npc_type_model::rebuild_flags`). The per-draw copy is animated first,
/// then `effect_resize` and the `effect_yaw` turn apply.
pub fn build_effect_model(
    pack: &Pack,
    materials: &MaterialStore,
    billboards: &BillboardStore,
    emitters: &EmitterStore,
    effect: &crate::protocol910::effect_types::Effect,
    flags: i32,
    detail_flags: i32,
) -> anyhow::Result<Option<GpuModel>> {
    if effect.model < 0 {
        return Ok(None);
    }
    let mut raw = match ModelUnlit::load(pack, effect.model as u32) {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };
    if raw.version < 13 {
        raw.scale_by_power_of_two(2);
    }
    let mut model = GpuModel::new(
        &crate::gpumodel::ModelStores {
            materials,
            billboards,
            emitters,
        },
        &raw,
        crate::gpumodel::BuildParams {
            flags,
            ambient: effect.ambient + 64,
            contrast: effect.contrast + 850,
            detail: detail_flags,
        },
    )?;
    if let Some(pairs) = &effect.colours {
        for &[from, to] in pairs {
            model.recolor(from, to);
        }
    }
    if let Some(pairs) = &effect.textures {
        for &[from, to] in pairs {
            model.retexture(materials, from, to)?;
        }
    }
    Ok(Some(model))
}

/// The type's resize, applied to the
/// per-draw copy after its animation.
pub fn effect_resize(model: &mut GpuModel, effect: &crate::protocol910::effect_types::Effect) {
    if effect.resizeh != 128 || effect.resizev != 128 {
        model.scale(effect.resizeh, effect.resizev, effect.resizeh);
    }
}

/// The type's `orientation` (opcode 6,
/// degrees; only 90, 180 and 270 turn) adds a quarter-turn multiple to the
/// caller's `yaw`, and a non-zero result turns the animated, resized
/// copy by `yaw & 0x3FFF` (`rotate_y_keep_normals`).
pub fn effect_yaw(
    model: &mut GpuModel,
    effect: &crate::protocol910::effect_types::Effect,
    yaw: i32,
) {
    let yaw = yaw.wrapping_add(match effect.orientation {
        90 => 4096,
        180 => 8192,
        270 => 12288,
        _ => 0,
    });
    if yaw != 0 {
        model.rotate_y_keep_normals(yaw & 0x3FFF);
    }
}

/// Which model of a loc type is wanted: the render capability flags, the loc
/// shape and the rotation (quarter turns, 4..7 for the diagonal variants).
#[derive(Clone, Copy, Debug)]
pub struct ModelRequest {
    pub flags: i32,
    pub shape: i32,
    pub rotation: i32,
}

/// The floors a loc's model follows: the floor of its level, when the level
/// has one, and the floor of the level above.
#[derive(Clone, Copy)]
pub struct LocGround<'a> {
    pub floor: Option<&'a FloorHeights>,
    pub above: Option<&'a FloorHeights>,
}

/// The dynamic model of a loc placement (no customisation), minus the hard
/// shadow blob.
pub fn get_dynamic_model(
    src: &ModelSource<'_>,
    loc: &Loc,
    request: ModelRequest,
    ground: LocGround<'_>,
    origin: [i32; 3],
) -> anyhow::Result<Option<GpuModel>> {
    get_dynamic_model_custom(src, loc, request, ground, origin, None)
}

/// The dynamic model with a customisation, for live zone location updates.
pub fn get_dynamic_model_custom(
    src: &ModelSource<'_>,
    loc: &Loc,
    request: ModelRequest,
    ground: LocGround<'_>,
    origin: [i32; 3],
    custom: Option<&LocCustom>,
) -> anyhow::Result<Option<GpuModel>> {
    let ModelRequest {
        flags: capability_flags,
        shape: mut loc_shape,
        rotation,
    } = request;
    let LocGround {
        floor: floor_heights,
        above: above_heights,
    } = ground;
    let [origin_x, origin_y, origin_z] = origin;
    if shape::is_wall_decor(loc_shape) {
        loc_shape = shape::WALLDECOR_STRAIGHT_NOOFFSET;
    }
    let mut model_flags = capability_flags;
    if loc.hillchange == 3 {
        model_flags = capability_flags | 0x7;
    } else {
        if loc.hillchange != 0 || loc.post_yoff != 0 {
            model_flags = capability_flags | 0x2;
        }
        if loc.post_xoff != 0 {
            model_flags |= 0x1;
        }
        if loc.post_zoff != 0 {
            model_flags |= 0x4;
        }
    }
    let follows_terrain =
        loc.hillchange != 0 && (floor_heights.is_some() || above_heights.is_some());
    let has_post_offset = loc.post_xoff != 0 || loc.post_yoff != 0 || loc.post_zoff != 0;
    let mut base_flags = model_flags;
    if shape::CENTREPIECE_STRAIGHT == loc_shape && rotation > 3 {
        base_flags = model_flags | 0x5;
    }
    let Some(mut model) =
        build_base_model_custom(src, loc, base_flags, loc_shape, rotation, custom)?
    else {
        return Ok(None);
    };
    if shape::CENTREPIECE_STRAIGHT == loc_shape && rotation > 3 {
        model.rotate_y_keep_normals(2048);
    }
    if follows_terrain || has_post_offset {
        if follows_terrain {
            let floor =
                floor_heights.ok_or_else(|| anyhow::anyhow!("hillchange without a floor"))?;
            model.hill_change(
                i32::from(loc.hillchange),
                loc.hillchange_value,
                TerrainHeights {
                    floor,
                    above: above_heights,
                },
                [origin_x, origin_y, origin_z],
            );
        }
        if has_post_offset {
            model.translate(loc.post_xoff, loc.post_yoff, loc.post_zoff);
        }
    }
    Ok(Some(model))
}

/// Finish an animated loc model after cloning its cached base model.
/// The pose precedes the diagonal rotation, terrain contour and post offsets.
pub fn finish_animated_model(
    model: &mut GpuModel,
    loc: &Loc,
    [shape, angle]: [i32; 2],
    pose: &crate::animation_assets::Pose,
    terrain: TerrainHeights<'_>,
    origin: [i32; 3],
) {
    // Alpha-animated clones are marked transparent.
    model.has_transparency |= pose.flags & 0x100 != 0;
    model.apply_animation(&pose.transforms);
    if shape == 10 && angle > 3 {
        model.rotate_y_keep_normals(2048);
    }
    if loc.hillchange != 0 {
        model.hill_change(
            i32::from(loc.hillchange),
            loc.hillchange_value,
            terrain,
            origin,
        );
    }
    if loc.post_xoff != 0 || loc.post_yoff != 0 || loc.post_zoff != 0 {
        model.translate(loc.post_xoff, loc.post_yoff, loc.post_zoff);
    }
}

#[cfg(test)]
mod effect_tests {
    use super::*;
    use crate::animation_assets::{AnimationAssets, Filter};
    use crate::entities910::animation_state::{Node, Sequence};

    /// A spot animation's model requested
    /// with its sequence's render flags carries label groups and two frames
    /// leave different vertices; the base flags alone (the port's request
    /// before lane F-NPCANIM) leave the model unanimatable. The type's
    /// orientation is a quarter-turn yaw, not a raw angle.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn real_pack_animated_spot_model_moves_and_orientation_turns_quarters() {
        let pack = Pack::open(rs910_core::test_support::pack_root());
        let materials = MaterialStore::load(&pack).unwrap();
        let billboards = BillboardStore::load(&pack).unwrap();
        let emitters = EmitterStore::load(&pack).unwrap();
        // The spot-animation types (`spot.config`), as `pack_animation::load`
        // decodes it.
        let (_, raw) = crate::protocol910::pack_types::records(&pack, "spot.config", 8).unwrap();
        let effects: Vec<crate::protocol910::effect_types::Effect> = raw
            .iter()
            .map(|(&id, b)| crate::protocol910::effect_types::Effect::decode(id, b).unwrap())
            .collect();
        let mut animations = AnimationAssets::load(&pack).unwrap();
        let (effect, seq) = effects
            .iter()
            .filter(|e| e.model >= 0)
            .find_map(|e| {
                let seq = animations.sequences.get(&e.sequence)?;
                (seq.skeletal == -1 && seq.frame_ids.as_ref()?.len() >= 3)
                    .then(|| (e.clone(), seq.clone()))
            })
            .expect("a cache spot animation with classic frames");
        let frames = seq.frame_ids.as_ref().unwrap().len();
        let mut pose_at = |frame: usize| {
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
                next: -1,
                ..Default::default()
            };
            animations
                .actor_pose(&pack, &mut node, Filter::default())
                .unwrap()
        };
        let poses: Vec<_> = (0..frames).map(&mut pose_at).collect();
        let anim_flags = poses.iter().fold(0, |flags, p| flags | p.flags);
        let build = |flags| {
            build_effect_model(
                &pack,
                &materials,
                &billboards,
                &emitters,
                &effect,
                flags,
                crate::gpumodel::MODEL_DETAIL_FLAGS,
            )
            .unwrap()
            .unwrap()
        };
        let unflagged = build(EFFECT_BASE_FLAGS);
        assert!(unflagged.vertex_groups.is_none());
        let base = build(effect_flags(anim_flags));
        assert!(base.vertex_groups.is_some());
        let shapes: std::collections::BTreeSet<_> = poses
            .iter()
            .map(|pose| {
                let mut m = base.clone();
                m.apply_animation(&pose.transforms);
                (m.vx, m.vy, m.vz)
            })
            .collect();
        assert!(
            shapes.len() > 1,
            "spot {} seq {}: frames leave different vertices",
            effect.model,
            seq.id
        );
        // 90 degrees is a 4096-unit yaw; other values do not turn.
        let mut quarter = effect.clone();
        quarter.orientation = 90;
        let mut turned = base.clone();
        effect_yaw(&mut turned, &quarter, 0);
        let mut expected = base.clone();
        expected.rotate_y_keep_normals(4096);
        assert_eq!((&turned.vx, &turned.vz), (&expected.vx, &expected.vz));
        quarter.orientation = 45;
        let mut unturned = base.clone();
        effect_yaw(&mut unturned, &quarter, 0);
        assert_eq!((&unturned.vx, &unturned.vz), (&base.vx, &base.vz));
    }
}
