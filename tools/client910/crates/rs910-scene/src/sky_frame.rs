//! The skybox of one frame in renderer-neutral form: the layer list, the sky models (faded
//! through their face alphas) and the material layers' texture pixels. Split out of
//! `sw_toolkit::model`; the CPU half of the sky resolution lives here rather than in the GPU
//! environment passes (`rs910_render_gpu::skybox_render::EnvPasses`), so every backend draws
//! the same [`SkyFrame`] from the scene snapshot.
//!
//! [`SkyCache`] is the shell's (one per client): [`SkyCache::resolve`] loads each box's
//! model and material texture on first use and applies the frame's fades at the point where
//! the scene is drawn. The faithful GPU toolkit then uploads what it has not uploaded yet
//! from the frame; a model whose upload fails is the toolkit failing to create the model
//! (the failure is swallowed and the box has no model, so it stays on the 2D path), which
//! the shell reports back through [`SkyCache::model_upload_failed`] before the frame is
//! drawn, as when the toolkit owned the load.
use crate::gpumodel::GpuModel;
use crate::sky_texture::load_texture;
pub use crate::sky_texture::{SkyAssets, SkyTexture};
use crate::skybox::{SkyLayer, SkyboxKey};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// A loaded sky model: the GPU model, the model's own face alphas and the fade last
/// applied to them.
#[derive(Clone)]
struct SkyModel {
    model: Arc<GpuModel>,
    own_alpha: Arc<Vec<i8>>,
    applied_fade: i32,
}

/// What a decor sprite was baked for: its size and the decor's pitch and yaw
/// (its light direction).
type DecorBake = (i32, i32, i32);

/// The sky models and material textures loaded so far, by skybox, and this
/// frame's layers (see the module docs).
#[derive(Clone, Default)]
pub struct SkyCache {
    models: BTreeMap<SkyboxKey, SkyModel>,
    sprites: BTreeMap<SkyboxKey, Arc<SkyTexture>>,
    failed_models: BTreeSet<SkyboxKey>,
    failed_sprites: BTreeSet<SkyboxKey>,
    /// The baked decor sprites by box and decor, with what they were baked
    /// for (sprite size, the decor's pitch and yaw).
    decors: BTreeMap<(SkyboxKey, usize), (DecorBake, Arc<SkyTexture>)>,
    failed_decors: BTreeSet<(SkyboxKey, usize, DecorBake)>,
    /// This frame's layers (`None`: no environment skybox, the scene clears to the fog
    /// colour).
    layers: Option<Vec<SkyLayer>>,
    /// The box the environment settles on (`None`: no skybox), whether the box
    /// changed through a cross-fade, and the cross-fade duration an
    /// environment override asks for ([`SkyFrame::target`]).
    target: Option<SkyboxKey>,
    fading: bool,
    fade_override_ms: Option<u32>,
}

impl SkyCache {
    /// Loading the sky model failed for this box (it has no model, so it keeps taking the
    /// 2D path).
    #[must_use]
    pub fn model_failed(&self, key: SkyboxKey) -> bool {
        self.failed_models.contains(&key)
    }

    /// Resolve this frame's layers: load the sky model and
    /// the material texture on first use and apply the fade to the model's
    /// face alphas.
    pub fn resolve(
        &mut self,
        assets: &SkyAssets<'_>,
        owner: &crate::skybox::SkyboxOwner,
        layers: Option<Vec<SkyLayer>>,
    ) {
        self.target = owner.target();
        self.fading = owner.is_fading();
        let Some(layers) = layers else {
            self.layers = None;
            return;
        };
        for layer in &layers {
            match layer {
                SkyLayer::Model { key, fade, .. } => {
                    let Some(sky) = owner.boxes.get(key) else {
                        continue;
                    };
                    if !self.models.contains_key(key) && !self.failed_models.contains(key) {
                        match load_model(assets, sky.model_id) {
                            Ok(model) => {
                                self.models.insert(*key, model);
                            }
                            Err(error) => {
                                // The failure is swallowed; the box has no model and
                                // stays on the 2D path.
                                rs910_core::log_repeat::warn_repeated!(
                                    "[client910] skybox {key:?} model {}: {error:#}",
                                    sky.model_id
                                );
                                self.failed_models.insert(*key);
                            }
                        }
                    }
                    let Some(sky_model) = self.models.get_mut(key) else {
                        continue;
                    };
                    if sky_model.applied_fade != *fade {
                        for (alpha, own) in Arc::make_mut(&mut sky_model.model)
                            .face_alpha
                            .iter_mut()
                            .zip(sky_model.own_alpha.iter())
                        {
                            *alpha =
                                crate::skybox::faded_alpha(Some(*own as u8), *fade as u8) as i8;
                        }
                        sky_model.applied_fade = *fade;
                    }
                }
                SkyLayer::Material { key, material, .. } => {
                    if !self.sprites.contains_key(key) && !self.failed_sprites.contains(key) {
                        match load_texture(assets, *material) {
                            Ok(texture) => {
                                self.sprites.insert(*key, Arc::new(texture));
                            }
                            Err(error) => {
                                rs910_core::log_repeat::warn_repeated!(
                                    "[client910] skybox {key:?} material {material}: {error:#}"
                                );
                                self.failed_sprites.insert(*key);
                            }
                        }
                    }
                }
                SkyLayer::Decor { key, decor, .. } => {
                    let Some(sky) = owner.boxes.get(key) else {
                        continue;
                    };
                    let Some(placed) = sky.decors.as_ref().and_then(|d| d.get(*decor)) else {
                        continue;
                    };
                    let bake: DecorBake = (placed.sprite_size, placed.pitch, placed.yaw);
                    let slot = (*key, *decor);
                    if self
                        .decors
                        .get(&slot)
                        .is_some_and(|(have, _)| *have == bake)
                        || self.failed_decors.contains(&(*key, *decor, bake))
                    {
                        continue;
                    }
                    let sun = sky
                        .sun
                        .and_then(|i| sky.decors.as_ref().and_then(|d| d.get(i)));
                    match crate::sky_decor::bake(placed, placed.sprite_size, sun, assets) {
                        Ok(texture) => {
                            self.decors.insert(slot, (bake, Arc::new(texture)));
                        }
                        Err(error) => {
                            rs910_core::log_repeat::warn_repeated!(
                                "[client910] skybox {key:?} decor {decor}: {error:#}"
                            );
                            self.failed_decors.insert((*key, *decor, bake));
                        }
                    }
                }
                SkyLayer::Fill { .. } | SkyLayer::Clear { .. } => {}
            }
        }
        self.layers = Some(layers);
    }

    /// The cross-fade duration (milliseconds) an active environment override
    /// asks for, taken by the next frames ([`SkyFrame::fade_override_ms`]);
    /// `None` without an override.
    pub fn set_fade_override(&mut self, milliseconds: Option<u32>) {
        self.fade_override_ms = milliseconds;
    }

    /// The faithful toolkit could not create the model of `key`: forget it, so the box
    /// keeps the 2D path and no backend draws it.
    pub fn model_upload_failed(&mut self, key: SkyboxKey) {
        self.models.remove(&key);
        self.failed_models.insert(key);
    }

    /// This frame's skybox (`None`: clear to the fog colour).
    #[must_use]
    pub fn frame(&self) -> Option<SkyFrame<'_>> {
        Some(SkyFrame {
            layers: self.layers.as_deref()?,
            cache: self,
        })
    }
}

/// One frame's skybox: the layers, the (faded) sky models the `Model` layers draw and the
/// material layers' texture pixels.
#[derive(Clone, Copy)]
pub struct SkyFrame<'a> {
    pub layers: &'a [SkyLayer],
    cache: &'a SkyCache,
}

impl<'a> SkyFrame<'a> {
    /// Own resolved layers and models; immutable sky pixels retain their identity.
    pub fn owned_cache(&self) -> SkyCache {
        self.cache.clone()
    }

    /// The box the environment settles on this frame (`None`: no skybox), for
    /// a backend that keeps its own cross-fade.
    #[must_use]
    pub fn target(&self) -> Option<SkyboxKey> {
        self.cache.target
    }

    /// Whether the environment reached [`Self::target`] through a cross-fade
    /// (false: installed at once, as a first environment or a teleport).
    #[must_use]
    pub fn fading(&self) -> bool {
        self.cache.fading
    }

    /// The cross-fade duration in milliseconds an active environment override
    /// asks for (`None`: the backend's default).
    #[must_use]
    pub fn fade_override_ms(&self) -> Option<u32> {
        self.cache.fade_override_ms
    }

    /// The sky model of `key` with this frame's fade applied (`None`: not loaded).
    #[must_use]
    pub fn model(&self, key: SkyboxKey) -> Option<&'a GpuModel> {
        self.cache.models.get(&key).map(|m| m.model.as_ref())
    }

    /// The sky model of `key` as first built (its own face alphas, no fade), for a
    /// backend's first upload.
    #[must_use]
    pub fn unfaded_model(&self, key: SkyboxKey) -> Option<GpuModel> {
        self.cache.models.get(&key).map(|m| {
            let mut model = m.model.as_ref().clone();
            model.face_alpha.clone_from(m.own_alpha.as_ref());
            model
        })
    }

    /// The baked sprite of decor `decor` of box `key` (`None`: it did not
    /// bake, and the decor draws nothing).
    #[must_use]
    pub fn decor_sprite(&self, key: SkyboxKey, decor: usize) -> Option<&'a Arc<SkyTexture>> {
        self.cache
            .decors
            .get(&(key, decor))
            .map(|(_, sprite)| sprite)
    }

    /// The material texture of `key` (`None`: it did not load).
    #[must_use]
    pub fn sprite(&self, key: SkyboxKey) -> Option<&'a Arc<SkyTexture>> {
        self.cache.sprites.get(&key)
    }
}

/// Load a sky model: the unlit model from the pack (no version rescale), built as
/// `GpuModel(flags 1099776, ambient 255, contrast 1, detail 0)`.
///
/// `detail & 0x37 == 0` bakes the sun into the colour stream at the first draw; that draw
/// happens after the frame sets ambient 1.0 and a white sun with zero diffuse/shadow, so the
/// baked colour is exactly `GpuModel::vertex_colour`, which the `(detail & 0x37) != 0`
/// stream already writes.
fn load_model(assets: &SkyAssets<'_>, model_id: i32) -> anyhow::Result<SkyModel> {
    let raw = crate::modelunlit::ModelUnlit::load(assets.pack, u32::try_from(model_id)?)?;
    let model = GpuModel::new(
        &crate::gpumodel::ModelStores {
            materials: assets.materials,
            billboards: assets.billboards,
            emitters: assets.emitters,
        },
        &raw,
        crate::gpumodel::BuildParams {
            flags: 1_099_776,
            ambient: 255,
            contrast: 1,
            detail: 0,
        },
    )?;
    let own_alpha = model.face_alpha.clone();
    Ok(SkyModel {
        model: Arc::new(model),
        own_alpha: Arc::new(own_alpha),
        applied_fade: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolving the next sky fade keeps the previous frame's model version.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn captured_sky_keeps_its_alpha_and_layer_version() -> anyhow::Result<()> {
        use crate::skybox::{SkyBoxType, SkyTypes, SkyboxOwner};
        const SYNTHETIC_SKY_TYPE: u32 = 0;
        const OLD_FADE: i32 = 0;
        const NEXT_FADE: i32 = 128;
        const OWN_ALPHA: i8 = 31;
        const MODEL_AMBIENT: i32 = 64;
        const MODEL_CONTRAST: i32 = 768;
        let pack = crate::test_support::require_pack("client.config.js5");
        let materials = crate::texture::MaterialStore::load(&pack)?;
        let billboards = crate::billboard::BillboardStore::load(&pack)?;
        let emitters = rs910_model::particle::EmitterStore::load(&pack)?;
        let assets = SkyAssets {
            pack: &pack,
            materials: &materials,
            billboards: &billboards,
            emitters: &emitters,
        };
        let mut types = SkyTypes::default();
        types
            .boxes_mut()
            .insert(SYNTHETIC_SKY_TYPE, SkyBoxType::default());
        let mut owner = SkyboxOwner::new(types);
        owner.select(Some(crate::env::SkyboxRef {
            kind: SYNTHETIC_SKY_TYPE as i32,
            a: 0,
            b: 0,
            c: 0,
            yaw_offset: 0,
        }));
        let key = owner.current.unwrap();
        let mut model = GpuModel::new(
            &crate::gpumodel::ModelStores {
                materials: &materials,
                billboards: &billboards,
                emitters: &emitters,
            },
            &crate::modelunlit::ModelUnlit::merge(&[]),
            crate::gpumodel::BuildParams {
                flags: 0,
                ambient: MODEL_AMBIENT,
                contrast: MODEL_CONTRAST,
                detail: 0,
            },
        )?;
        model.face_alpha = vec![OWN_ALPHA];
        let layer = |fade| SkyLayer::Model {
            key,
            pitch: 0,
            yaw: 0,
            roll: 0,
            fade,
        };
        let mut cache = SkyCache::default();
        cache.models.insert(
            key,
            SkyModel {
                model: Arc::new(model),
                own_alpha: Arc::new(vec![OWN_ALPHA]),
                applied_fade: OLD_FADE,
            },
        );
        cache.resolve(&assets, &owner, Some(vec![layer(OLD_FADE)]));
        let captured = cache.frame().unwrap().owned_cache();
        let previous = captured.models[&key].model.clone();
        assert!(Arc::ptr_eq(&previous, &cache.models[&key].model));
        cache.resolve(&assets, &owner, Some(vec![layer(NEXT_FADE)]));
        assert!(!Arc::ptr_eq(&previous, &cache.models[&key].model));
        assert_eq!(
            captured.frame().unwrap().model(key).unwrap().face_alpha,
            vec![OWN_ALPHA]
        );
        assert_eq!(captured.frame().unwrap().layers, &[layer(OLD_FADE)]);
        assert_eq!(cache.frame().unwrap().layers, &[layer(NEXT_FADE)]);
        assert_eq!(
            cache.frame().unwrap().model(key).unwrap().face_alpha,
            vec![crate::skybox::faded_alpha(Some(OWN_ALPHA as u8), NEXT_FADE as u8) as i8]
        );
        cache.resolve(&assets, &owner, Some(vec![layer(NEXT_FADE)]));
        assert_eq!(
            captured.frame().unwrap().model(key).unwrap().face_alpha,
            vec![OWN_ALPHA]
        );
        Ok(())
    }

    /// `rgba` inverts the ARGB packing of `load_texture` for every byte
    /// value in every channel.
    #[test]
    fn rgba_round_trips_the_packed_texels() {
        let px: Vec<u8> = (0..=255u8)
            .flat_map(|v| [v, v.wrapping_mul(7), v.wrapping_add(91), 255 - v])
            .collect();
        let argb = |p: &[u8]| {
            (u32::from(p[3]) << 24)
                | (u32::from(p[0]) << 16)
                | (u32::from(p[1]) << 8)
                | u32::from(p[2])
        };
        let texture = SkyTexture {
            argb: px.chunks_exact(4).map(|p| argb(p) as i32).collect(),
            size: [256, 1],
            first: 0,
            last: 0,
        };
        assert_eq!(texture.rgba(), px);
    }
}
