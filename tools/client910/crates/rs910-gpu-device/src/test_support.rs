//! Test helpers of the GPU crates (`test-hooks`; rs910-render-gpu's and
//! client910's tests enable it through `[dev-dependencies]` only).
/// A wgpu device on the default adapter. GPU tests are `#[ignore]`d. When
/// one runs on a machine without an adapter, it fails here instead of
/// returning early and reporting `ok`.
#[track_caller]
pub fn require_gpu() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("no wgpu adapter: this test needs a desktop GPU");
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: crate::gpu_device::renderer_limits(&adapter),
        ..Default::default()
    }))
    .expect("wgpu request_device failed: this test needs a desktop GPU")
}
