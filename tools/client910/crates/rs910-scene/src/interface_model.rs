//! One interface model draw (type-6 interface components) as the retained UI
//! hands it to a toolkit: the lit `GpuModel`, the matrices and lighting
//! (`DrawSpace`) and the particles, with the cache resources they read. Split
//! out of client910's
//! `ui_models` in Phase 3.2 (the renderers consume
//! it without naming the UI crate. It is the `M` of
//! `rs910_toolkit::FramePlan<M>` and `rs910_toolkit::ui_output::Output<M>`.
//!
//! The draw is renderer-neutral (A4, lane Q-PREM1): each backend derives its
//! own pipeline state from [`DrawSpace`]. The faithful GPU toolkit computes
//! its `FloorUniforms` when it prepares the draw
//! (`rs910_render_gpu::ui_model_gpu::interface_uniforms`, the former
//! `ui_models::uniforms`, a pure function of the same inputs).
use crate::{cache::Pack, gpumodel::GpuModel};
use std::{any::Any, collections::BTreeMap, rc::Rc, rc::Weak};

/// The component that owns an interface model, by `Rc` identity: the key the
/// GPU mesh cache reuses a model's buffers under while the component lives
/// (it is keyed on the component object). The `Weak` is type-erased so this
/// draw can live below the UI crate that defines the component;
/// [`ModelOwner::strong_count`] and [`ModelOwner::as_ptr`] give the count and
/// the address the `Weak<RefCell<Component>>` it wraps gives.
pub struct ModelOwner(Weak<dyn Any>);
impl ModelOwner {
    /// `Weak::strong_count` of the owner.
    pub fn strong_count(&self) -> usize {
        self.0.strong_count()
    }
    /// The owner's address (`Weak::as_ptr`, without the vtable).
    pub fn as_ptr(&self) -> *const () {
        self.0.as_ptr() as *const ()
    }
}
impl<T: 'static> From<Weak<T>> for ModelOwner {
    fn from(owner: Weak<T>) -> Self {
        Self(owner)
    }
}
pub struct Resources {
    pub pack: Pack,
    pub materials: crate::texture::MaterialStore,
    pub billboards: crate::billboard::BillboardStore,
    pub emitters: crate::particle::EmitterStore,
    pub appearance: crate::entity_runtime::appearance_pack::Inputs,
    pub bases: BTreeMap<i32, crate::protocol910::bas_types::Bas>,
    pub avatar_idks: crate::avatar::IdkStore,
    pub avatar_defaults: crate::avatar::GraphicsDefaults,
    /// Varbit definitions used to resolve multi-NPC models.
    pub varbits: crate::entity_runtime::bits_pack::Inputs,
}
pub struct Draw {
    pub owner: ModelOwner,
    pub quad: usize,
    pub before_scene: bool,
    pub clip: [i32; 4],
    pub model: GpuModel,
    pub depth_write: bool,
    pub resources: Rc<Resources>,
    /// The model's emitters/effectors after the model matrix is applied, keyed
    /// by the component's particle system.
    pub particles: crate::particle::Binding,
    /// Only the particle system of a perspective (non-orthographic) model is
    /// drawn.
    pub draw_particles: bool,
    /// The particle draw list after the bind, filled by the particle owner.
    pub particle_list: Vec<crate::particle::DrawParticle>,
    /// The inputs of the draw every backend reads: the model matrix as
    /// `Matrix4x4` entries, the projection and the interface lighting.
    pub space: DrawSpace,
}

/// Bind every interface model's particles and take the list each one draws.
pub fn bind_particles(draws: &mut [Draw], runtime: &mut crate::particle::Runtime, cycle: i64) {
    for d in draws {
        let b = &d.particles;
        runtime.bind_interface(b.key, cycle, &b.emitters, &b.effectors);
        d.particle_list = if d.draw_particles {
            runtime.slot_list(b.key)
        } else {
            Vec::new()
        };
    }
}
/// The matrices and lighting of a type-6 draw.
#[derive(Clone, Debug)]
pub struct DrawSpace {
    /// The model matrix (4x3) as `Matrix4x4` entries
    /// (`actor_matrix::Matrix::entries`).
    pub matrix: [f32; 16],
    /// The projection (the view is the identity).
    pub projection: [f32; 16],
    /// The interface environment the draw is lit and fogged by (sun, sun
    /// colour, fog, the environment sampler), `ui_models::lighting`.
    pub lighting: crate::env::EnvFrame,
}
