//! Short forms for the wgpu descriptors the renderers build over and over:
//! bind group layout entries, bind groups, buffers, 2D textures, render
//! passes and full-screen pipelines. Each returns the plain wgpu value, so a
//! caller that needs one more field uses struct update syntax
//! (`wgpu::TextureDescriptor { mip_level_count: n, ..texture_2d(..) }`).

/// One bind group layout entry, never an array.
pub const fn entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty,
        count: None,
    }
}

/// A uniform buffer binding with a fixed offset and no minimum size.
pub const fn uniform(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    uniform_sized(binding, visibility, false, 0)
}

/// A uniform buffer binding, optionally with a dynamic offset, whose bound
/// range is at least `min_size` bytes (0 for no minimum).
pub const fn uniform_sized(
    binding: u32,
    visibility: wgpu::ShaderStages,
    has_dynamic_offset: bool,
    min_size: u64,
) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        visibility,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset,
            min_binding_size: wgpu::BufferSize::new(min_size),
        },
    )
}

/// A read-only storage buffer binding.
pub const fn storage(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        visibility,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
    )
}

/// A sampled texture binding.
pub const fn texture(
    binding: u32,
    visibility: wgpu::ShaderStages,
    sample_type: wgpu::TextureSampleType,
    view_dimension: wgpu::TextureViewDimension,
    multisampled: bool,
) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        visibility,
        wgpu::BindingType::Texture {
            sample_type,
            view_dimension,
            multisampled,
        },
    )
}

/// A single-sampled float texture binding.
pub const fn float_texture(
    binding: u32,
    visibility: wgpu::ShaderStages,
    filterable: bool,
    view_dimension: wgpu::TextureViewDimension,
) -> wgpu::BindGroupLayoutEntry {
    texture(
        binding,
        visibility,
        wgpu::TextureSampleType::Float { filterable },
        view_dimension,
        false,
    )
}

/// A sampler binding.
pub const fn sampler(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::SamplerBindingType,
) -> wgpu::BindGroupLayoutEntry {
    entry(binding, visibility, wgpu::BindingType::Sampler(ty))
}

/// A bind group layout.
pub fn layout(
    device: &wgpu::Device,
    label: &str,
    entries: &[wgpu::BindGroupLayoutEntry],
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries,
    })
}

/// A bind group from `(binding, resource)` pairs.
pub fn bind_group(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    resources: &[(u32, wgpu::BindingResource<'_>)],
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry<'_>> = resources
        .iter()
        .map(|(binding, resource)| wgpu::BindGroupEntry {
            binding: *binding,
            resource: resource.clone(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &entries,
    })
}

/// A pipeline layout over the given bind group layouts.
pub fn pipeline_layout(
    device: &wgpu::Device,
    label: &str,
    groups: &[Option<&wgpu::BindGroupLayout>],
) -> wgpu::PipelineLayout {
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: groups,
        immediate_size: 0,
    })
}

/// An unmapped buffer.
pub fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    })
}

/// A uniform buffer the queue writes into.
pub fn uniform_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    buffer(
        device,
        label,
        size,
        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    )
}

/// The descriptor of a single-sampled 2D texture with one mip level and one
/// layer.
pub fn texture_2d(
    label: &str,
    [width, height]: [u32; 2],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::TextureDescriptor<'_> {
    wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    }
}

/// The descriptor of a single-sampled 2D texture with one mip level and
/// `layers` layers (an array, or the six faces of a cube).
pub fn texture_layers(
    label: &str,
    [width, height, layers]: [u32; 3],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::TextureDescriptor<'_> {
    wgpu::TextureDescriptor {
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: layers,
        },
        ..texture_2d(label, [width, height], format, usage)
    }
}

/// A colour attachment that stores what the pass draws.
pub fn colour(
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> Option<wgpu::RenderPassColorAttachment<'_>> {
    Some(wgpu::RenderPassColorAttachment {
        view,
        resolve_target: None,
        depth_slice: None,
        ops: wgpu::Operations {
            load,
            store: wgpu::StoreOp::Store,
        },
    })
}

/// A depth attachment without stencil.
pub fn depth(
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<f32>,
    store: wgpu::StoreOp,
) -> Option<wgpu::RenderPassDepthStencilAttachment<'_>> {
    Some(wgpu::RenderPassDepthStencilAttachment {
        view,
        depth_ops: Some(wgpu::Operations { load, store }),
        stencil_ops: None,
    })
}

/// A render pass with no queries or timestamps.
pub fn pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    colour_attachments: &[Option<wgpu::RenderPassColorAttachment<'_>>],
    depth_stencil_attachment: Option<wgpu::RenderPassDepthStencilAttachment<'_>>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: colour_attachments,
        depth_stencil_attachment,
        occlusion_query_set: None,
        multiview_mask: None,
        timestamp_writes: None,
    })
}

/// A pass that draws into one colour target.
pub fn colour_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPass<'e> {
    pass(encoder, label, &[colour(view, load)], None)
}

/// A pass that only writes depth.
pub fn depth_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<f32>,
) -> wgpu::RenderPass<'e> {
    pass(encoder, label, &[], depth(view, load, wgpu::StoreOp::Store))
}

/// The vertex entry point every full-screen shader module defines: one
/// triangle that covers the target.
pub const FULLSCREEN_VERTEX: &str = "vs_full";

/// A pipeline that draws one full-screen triangle into one colour target
/// with no depth.
pub fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    fragment: &str,
    target: wgpu::ColorTargetState,
    samples: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(FULLSCREEN_VERTEX),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fragment),
            targets: &[Some(target)],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

/// A depth test without stencil or bias.
pub fn depth_state(
    format: wgpu::TextureFormat,
    write: bool,
    compare: wgpu::CompareFunction,
) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: Default::default(),
        bias: Default::default(),
    }
}

/// Triangle-list assembly with the given winding and culling.
pub fn triangles(
    front_face: wgpu::FrontFace,
    cull_mode: Option<wgpu::Face>,
) -> wgpu::PrimitiveState {
    wgpu::PrimitiveState {
        topology: wgpu::PrimitiveTopology::TriangleList,
        front_face,
        cull_mode,
        ..Default::default()
    }
}

/// One bind group entry.
pub fn bind(binding: u32, resource: wgpu::BindingResource<'_>) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource }
}

/// Write mip level `mip_level` of layer `layer` from tightly packed `data`
/// covering `[width, height]` texels (rounded up to whole blocks for a
/// block-compressed format).
pub fn write_level(
    queue: &(impl crate::uploads::Uploader + ?Sized),
    texture: &wgpu::Texture,
    mip_level: u32,
    layer: u32,
    [width, height]: [u32; 2],
    data: &[u8],
) {
    let format = texture.format();
    let (block_width, block_height) = format.block_dimensions();
    let block_bytes = format
        .block_copy_size(None)
        .expect("a colour format the queue can write");
    let (blocks_x, blocks_y) = (width.div_ceil(block_width), height.div_ceil(block_height));
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer,
            },
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(blocks_x * block_bytes),
            rows_per_image: Some(blocks_y),
        },
        wgpu::Extent3d {
            width: blocks_x * block_width,
            height: blocks_y * block_height,
            depth_or_array_layers: 1,
        },
    );
}

/// Write a mip chain `(width, height, texels)`, level 0 first, into layer
/// `layer` (see [`write_level`]).
pub fn write_mips(
    queue: &(impl crate::uploads::Uploader + ?Sized),
    texture: &wgpu::Texture,
    layer: u32,
    levels: &[(u32, u32, Vec<u8>)],
) {
    for (mip, (width, height, data)) in levels.iter().enumerate() {
        write_level(queue, texture, mip as u32, layer, [*width, *height], data);
    }
}
