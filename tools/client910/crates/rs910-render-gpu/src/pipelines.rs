//! One cache of the faithful toolkit's render pipelines, keyed by
//! `(samples, colour format, variant)` (code-quality programme §6 "one
//! pipeline cache keyed by (samples, format, variant)", Phase 4.5).
//!
//! A pipeline's descriptor is fixed by its owner's code, the sample count
//! and the colour format: every other input is a crate constant
//! ([`DEPTH_FORMAT`], the WGSL sources, the process-wide profiling switches
//! `CLIENT910_PROFILE_FRAGMENT_UNIFORM`/`_SHARED_DISCARD`) or a bind group
//! layout whose entries are fixed too (wgpu pools layouts by their entries,
//! so bind groups made from one owner's layout bind to a pipeline made from
//! another's). So one entry per key serves every owner that used to build
//! its own identical copy:
//!
//! | Variant | Owners that built identical copies before |
//! |---|---|
//! | [`Variant::Batch2d`] (`text_render_gpu::Pipeline`: plain, masked and opaque 2D batch) | the retained UI painter, the message-box overlay, the framebuffer-sprite painter, every minimap base (one per `render_minimap_base`), every bounded canvas target (one per `set_game_canvas` change), the console |
//! | [`Variant::FloorModels`] (`floor_render`'s four model pipelines) | the renderer's and the interface models' `FloorPipeline` (base at one sample), and each `set_sample_count` back to a count seen before |
//! | [`Variant::FloorPasses`] (static lights and hard shadows) | `Renderer::new`, then every `set_scene_effects` (incl. `recreate_toolkit_targets`, which forces a rebuild with the same key) |
//! | [`Variant::Particles`], [`Variant::Billboards`] | as `FloorPasses`, plus the interface models (`ui_model_gpu::Models::set_samples`, rebuilt whenever a frame's interface switches between having models and not while MSAA is on) |
//! | [`Variant::SceneCopy`] (`scene_target`'s present and seed pipelines) | every scene target (per resize or sample change) and the interface models' MSAA target |
//!
//! Owners keep their per-instance state (buffers, bind groups, per-frame
//! draws) and take their pipelines from here through their `cached`
//! constructors. The post-process chain (`postprocess::PostProcessor`, by
//! entry and format) and the sky's flat pipelines (`skybox_render`, by
//! format and samples, with their own sprite bindings) keep their existing
//! one-owner caches.
//!
//! The cache is a shared handle ([`Clone`]): the renderer hands clones to the
//! owners it creates, so one renderer has one cache. Entries live as long as
//! the renderer (toolkit lifetime, see `render::resources`).

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The depth format of every faithful pass (`Depth24Plus`).
pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

/// Which owner's pipelines an entry holds (each variant has one value type).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Variant {
    /// `text_render_gpu::Pipeline`.
    Batch2d,
    /// `[wgpu::RenderPipeline; 4]` of `floor_render::floor_pipelines`.
    FloorModels,
    /// `floorpass::FloorPasses`.
    FloorPasses,
    /// `particle_render::ParticlePipelines`.
    Particles,
    /// `billboard_render::BillboardPipelines`.
    Billboards,
    /// `scene_target::CopyPipelines`.
    SceneCopy,
}

/// A cache key: the colour target's sample count and format, and the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub samples: u32,
    pub format: wgpu::TextureFormat,
    pub variant: Variant,
}

type Entries = HashMap<Key, Arc<dyn Any + Send + Sync>>;

/// The shared pipeline cache (module docs).
#[derive(Clone, Default)]
pub struct PipelineCache(Arc<Mutex<Entries>>);

impl PipelineCache {
    /// The entry for `(samples, format, variant)`, built by `create` on
    /// first use.
    pub fn get<T: Any + Send + Sync>(
        &self,
        samples: u32,
        format: wgpu::TextureFormat,
        variant: Variant,
        create: impl FnOnce() -> T,
    ) -> Arc<T> {
        let key = Key {
            samples,
            format,
            variant,
        };
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.get(&key) {
            return Arc::clone(entry)
                .downcast()
                .unwrap_or_else(|_| panic!("pipeline cache: {variant:?} holds another type"));
        }
        let entry = Arc::new(create());
        entries.insert(key, entry.clone());
        entry
    }

    /// The number of entries (tests).
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_entry_per_key_shared_by_clones() {
        let cache = PipelineCache::default();
        let other = cache.clone();
        let mut built = 0;
        let a = cache.get(1, wgpu::TextureFormat::Bgra8Unorm, Variant::Batch2d, || {
            built += 1;
            7_u32
        });
        let b = other.get(1, wgpu::TextureFormat::Bgra8Unorm, Variant::Batch2d, || {
            built += 1;
            8_u32
        });
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!((*b, built), (7, 1));
        // Each part of the key separates entries.
        let c = cache.get(4, wgpu::TextureFormat::Bgra8Unorm, Variant::Batch2d, || {
            9_u32
        });
        let d = cache.get(
            1,
            wgpu::TextureFormat::Rgba16Float,
            Variant::Batch2d,
            || 10_u32,
        );
        let e = cache.get(
            1,
            wgpu::TextureFormat::Bgra8Unorm,
            Variant::Particles,
            || 11_u32,
        );
        assert_eq!((*c, *d, *e, cache.len()), (9, 10, 11, 4));
    }
}
