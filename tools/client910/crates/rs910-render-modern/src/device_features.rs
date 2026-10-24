//! Device feature policy shared by modern scene entry points.

/// Optional native features used by the modern material and far submission paths.
#[must_use]
pub fn optional_device_features() -> wgpu::Features {
    crate::models::materials::optional_device_features()
        | wgpu::Features::INDIRECT_FIRST_INSTANCE
        | wgpu::Features::MULTI_DRAW_INDIRECT_COUNT
}
