//! One cache of the UI toolkit's render pipelines, keyed by
//! `(samples, colour format, variant)`.
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
//! | [`Variant::Quads`] | the interface models (`ui_model_gpu::Models::set_samples`, rebuilt whenever a frame's interface switches between having models and not while MSAA is on) |
//! | [`Variant::SceneCopy`] (`scene_target`'s present and seed pipelines) | every scene target (per resize or sample change) and the interface models' MSAA target |
//!
//! Owners keep their per-instance state (buffers, bind groups, per-frame
//! draws) and take their pipelines from here through their `cached`
//! constructors.
//!
//! The cache is a shared handle ([`Clone`]): the renderer hands clones to the
//! owners it creates, so one renderer has one cache. Entries live as long as
//! the renderer (toolkit lifetime, see `render::resources`).

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The depth format of every toolkit pass (`Depth24Plus`).
pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

/// Which owner's pipelines an entry holds (each variant has one value type).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Variant {
    /// `text_render_gpu::Pipeline`.
    Batch2d,
    /// `[wgpu::RenderPipeline; 4]` of `floor_render::floor_pipelines`.
    FloorModels,
    /// `quad_render::QuadPipelines`.
    Quads,
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
