use super::*;
/// Executes the cache GLSL through offscreen OpenGL for the reference;
/// this path renders the same fixtures through the production WGSL/Metal pipelines.
#[test]
#[ignore = "requires tools/oracle/run-gpu-pixels.sh and a desktop GPU"]
fn material_pixels_match_cache_glsl() {
    let path = std::env::var_os("CLIENT910_MATERIAL_REFERENCE").expect(
        "CLIENT910_MATERIAL_REFERENCE is not set: run this test through tools/oracle/run-gpu-pixels.sh",
    );
    let reference = std::fs::read(path).unwrap();

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: None,
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        memory_hints: Default::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        trace: wgpu::Trace::Off,
    }))
    .unwrap();
    let mut pipeline = FloorPipeline::new(
        &device,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Depth24Plus,
    );
    let cube = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 6,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for face in 0..6u8 {
        let mut pixels = Vec::new();
        for y in 0..4u8 {
            for x in 0..4u8 {
                pixels.extend([
                    40 + 45 * x + 10 * face,
                    30 + 50 * y + 5 * face,
                    20 + 35 * face,
                    255,
                ]);
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &cube,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: face as u32,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(16),
                rows_per_image: Some(4),
            },
            wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
        );
    }
    pipeline.environment_view = cube.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    pipeline.uniform_bind_group = material_frame_bindings(
        &device,
        &pipeline.uniform_layout,
        &pipeline.uniform_buffer,
        FrameViews {
            cube: &pipeline.environment_view,
            sampler: &pipeline.environment_sampler,
            noise: &pipeline.noise_view,
            noise_sampler: &pipeline.noise_sampler,
            water_normals: &pipeline.water_normal_view,
        },
    );
    pipeline.upload_noise(&queue);
    let mut uniforms = FloorUniforms {
        wvp: glam::Mat4::IDENTITY.to_cols_array_2d(),
        shadow_wvp: glam::Mat4::IDENTITY.to_cols_array_2d(),
        sun_dir: [0., 0., -1., 0.],
        sun_colour: [0.3, 0.4, 0.5, 0.],
        anti_sun_colour: [0.; 4],
        ambient_colour: [0.2, 0.2, 0.2, 0.],
        height_fog_plane: [0.; 4],
        height_fog_colour: [0.; 4],
        distance_fog_plane: [0., 0., 0., 0.25],
        distance_fog_colour: [0.1, 0.2, 0.3, 0.],
        scene_origin: [0.; 4],
        eye_time: [0., 0., -2., 0.],
        scene_base: [0.; 4],
        model_world: glam::Mat4::IDENTITY.to_cols_array_2d(),
        sun_rgb: [1.0, 1.0, 1.0, 0.0],
    };
    queue.write_buffer(&pipeline.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    let size = wgpu::Extent3d {
        width: 32,
        height: 32,
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth24Plus,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let depth_view = depth.create_view(&Default::default());
    let cases = [
        [0, 0, 255, 0, 0, 0],
        [0, 0, 0, 0, 0, 0],
        [0, 0, 127, 127, 0, 0],
        [0, 0, 128, 127, 0, 0],
        [0, 0, 127, 128, 0, 0],
        [0, 0, 255, 0, 0, 1],
        [1, 32, 128, 0, 0, 0],
        [1, 4, 128, 0, 0, 0],
        [1, 1, 128, 0, 0, 0],
        [1, 0, 128, 0, 0, 0],
        [2, 0, 128, 0, 0, 0],
        [3, 0, 128, 0, 0, 0],
        [6, 0, 0, 0, 0, 0],
        [0, 0, 255, 0, 1, 0],
        [1, 4, 128, 0, 1, 0],
        [5, 0, 255, 0, 0, 0],
        [0, 0, 255, 0, 4, 0],
        [2, 0, 128, 0, 0, 0],
        [2, 0, 128, 0, 0, 0],
        [2, 0, 128, 0, 0, 0],
        [2, 0, 128, 0, 0, 0],
        [2, 0, 128, 0, 0, 0],
        [2, 0, 128, 0, 0, 0],
        [2, 0, 128, 0, 0, 0],
        [2, 0, 128, 0, 0, 0],
        [2, 0, 128, 0, 0, 0],
        [2, 0, 128, 0, 0, 0],
        [2, 0, 128, 0, 0, 0],
        [5, 0, 255, 0, 0, 0],
        [5, 0, 255, 0, 0, 0],
    ];
    assert_eq!(reference.len(), cases.len() * 32 * 32 * 4);
    let mut max_error = 0;
    for (case, c) in cases.iter().enumerate() {
        if case >= 17 {
            let eyes = [
                [-8., 0., 0.25],
                [8., 0., 0.25],
                [0., 8., 0.25],
                [0., -8., 0.25],
                [0., 0., 8.],
                [2.1, 2.2, 0.25],
                [0.1, 2.1, 2.1],
            ];
            uniforms.eye_time[..3].copy_from_slice(&eyes[(case - 17) % 7]);
            queue.write_buffer(&pipeline.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        }
        if case >= 24 {
            let rotations = [
                [
                    0., 0., -1., 0., 0., 1., 0., 0., 1., 0., 0., 0., 0., 0., 0., 1.,
                ],
                [
                    1., 0., 0., 0., 0., 0., 1., 0., 0., -1., 0., 0., 0., 0., 0., 1.,
                ],
                [
                    0., 1., 0., 0., -1., 0., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
                ],
                [
                    0., 0., 1., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 0., 1.,
                ],
            ];
            let mut world = rotations[(case - 24) % 4];
            if c[0] == 5 {
                world[12] = 0.125;
                world[13] = 0.5;
                world[14] = -0.25;
            }
            uniforms.model_world = glam::Mat4::from_cols_array(&world).to_cols_array_2d();
        }
        let texture = FloorTexture {
            view: upload_argb(
                &device,
                &queue,
                "fixture",
                1,
                1,
                vec![(c[2] as u32) << 24 | 160 << 16 | 120 << 8 | 80],
                false,
            ),
            sampler: std::sync::Arc::new(make_sampler(&device, true, true, false)),
        };
        let alpha_ref = if c[3] > 0 { c[3] as f32 / 255. } else { -1. };
        let (bind_group, buffer, mut values) = batch_bind_group_full(
            &device,
            &pipeline,
            &texture,
            BatchUniforms::initial(1., alpha_ref, false, [0.; 3]),
            "fixture",
        );
        values.shader = [c[0] as f32, c[1] as f32, 1., c[4] as f32];
        values.water = [1., 1., 0., 0.25];
        for slot in 0..c[4] as usize {
            values.light_pos[slot] = [0., 0., -2., 0.25];
            values.light_colour[slot] = [0.1, 0.2, 0.3, 1.];
        }
        queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&values));
        let vertices =
            [[-1., -1., 0.25], [3., -1., 0.25], [-1., 3., 0.25]].map(|pos| FloorVertex {
                pos,
                uv: [0.5; 2],
                depth: 0.,
                normal: [0., 0., -1.],
            });
        let mut mesh = FloorMesh {
            render_id: next_render_id(),
            model_uniform: None,
            depth_write: true,
            billboards: None,
            vertex_buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            shared_colours: Some(
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: &[200, 180, 160, 255].repeat(3),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
            ),
            batches: vec![FloorGpuBatch {
                colour_buffer: None,
                index_buffer: index_buffer(
                    &device,
                    "fixture",
                    if c[5] != 0 { &[0, 2, 1] } else { &[0, 1, 2] },
                ),
                index_count: 3,
                bind_group,
                material: -1,
                alpha_test: c[3] > 0,
                floor_batch: None,
                payload: None,
            }],
            vertex_count: 3,
        };
        if case >= 24 {
            mesh.set_model_uniforms(&device, &queue, &pipeline, &uniforms);
        }
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * 32,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1,
                            g: 0.2,
                            b: 0.3,
                            a: 0.4,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &pipeline.uniform_bind_group, &[]);
            mesh.draw_model(&mut pass, &pipeline);
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(32),
                },
            },
            size,
        );
        queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().unwrap().unwrap();
        let mapped = readback.slice(..).get_mapped_range().expect("mapped range");
        let mut bad = 0;
        for y in 0..32 {
            for x in 0..128 {
                let actual = mapped[y * 256 + x];
                let expected = reference[case * 4096 + y * 128 + x];
                let error = actual.abs_diff(expected);
                max_error = max_error.max(error);
                if error > 2 {
                    bad += 1;
                }
            }
        }
        if bad > 0 {
            let raw: Vec<u8> = (0..32)
                .flat_map(|y| mapped[y * 256..y * 256 + 128].to_vec())
                .collect();
            std::fs::write(format!("/tmp/material-actual-{case}.rgba"), raw).unwrap();
        }
        assert_eq!(
            bad, 0,
            "GLSL/Metal case {case} {c:?}, maximum channel error {max_error}"
        );
    }
    eprintln!(
        "[material-pixels] 30 GLSL/Metal fixtures matched; maximum channel error {max_error}/255"
    );
}

/// Draw the cutout BEFORE the background, as Scene does before floors.
/// Rejected fragments must neither colour nor occlude the later draw.
#[test]
#[ignore = "requires a desktop GPU; run explicitly"]
fn model_cutout_preserves_background_depth() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: None,
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        memory_hints: Default::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        trace: wgpu::Trace::Off,
    }))
    .unwrap();
    let mut pipeline = FloorPipeline::new(
        &device,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Depth24Plus,
    );
    let uniforms = FloorUniforms {
        wvp: glam::Mat4::IDENTITY.to_cols_array_2d(),
        eye_time: [0.; 4],
        scene_base: [0.; 4],
        model_world: glam::Mat4::IDENTITY.to_cols_array_2d(),
        scene_origin: [0.0; 4],
        shadow_wvp: glam::Mat4::IDENTITY.to_cols_array_2d(),
        sun_dir: [0.0; 4],
        sun_colour: [0.0; 4],
        anti_sun_colour: [0.0; 4],
        ambient_colour: [1.0; 4],
        height_fog_plane: [0.0; 4],
        height_fog_colour: [0.0; 4],
        distance_fog_plane: [0.0; 4],
        distance_fog_colour: [0.0; 4],
        sun_rgb: [1.0, 1.0, 1.0, 0.0],
    };
    queue.write_buffer(&pipeline.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    let make_mesh = |pipeline_ref: &FloorPipeline,
                     z: f32,
                     alpha: u8,
                     colour: [u8; 4],
                     reference: f32,
                     reflective: bool| {
        let verts = [[-1.0, -1.0, z], [3.0, -1.0, z], [-1.0, 3.0, z]].map(|pos| FloorVertex {
            pos,
            uv: [0.5; 2],
            depth: 0.0,
            normal: [0.0, -1.0, 0.0],
        });
        let texture = FloorTexture {
            view: upload_argb(
                &device,
                &queue,
                "test alpha",
                1,
                1,
                vec![(u32::from(alpha) << 24) | 0xffffff],
                false,
            ),
            sampler: std::sync::Arc::new(
                device.create_sampler(&wgpu::SamplerDescriptor::default()),
            ),
        };
        FloorMesh {
            render_id: next_render_id(),
            model_uniform: None,
            depth_write: true,
            billboards: None,
            vertex_buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            shared_colours: Some(
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&[colour; 3]),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
            ),
            batches: vec![FloorGpuBatch {
                colour_buffer: None,
                index_buffer: index_buffer(&device, "test", &[0, 1, 2]),
                index_count: 3,
                bind_group: batch_bind_group(
                    &device,
                    pipeline_ref,
                    &texture,
                    BatchUniforms::initial(1.0, reference, reflective, [0.0; 3]),
                    "test",
                ),
                material: -1,
                floor_batch: None,
                payload: None,
                alpha_test: reference >= 0.0,
            }],
            vertex_count: 3,
        }
    };
    let background = make_mesh(&pipeline, 0.8, 255, [0, 0, 255, 255], -1.0, false);
    let farthest = make_mesh(&pipeline, 0.9, 255, [0, 255, 0, 255], -1., false);
    // GL_GREATER at a deliberately non-0.5 threshold. Also cover vertex
    // alpha multiplication and ref=255 (no fragment can pass).
    for samples in [1, 4] {
        pipeline.set_sample_count(
            &device,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Depth24Plus,
            samples,
        );
        for depth_write in [true, false] {
            for bundled in [false, true] {
                for (tex_alpha, vertex_alpha, reference, reflective, survives) in [
                    (0, 255, 64, false, false),
                    (63, 255, 64, false, false),
                    (64, 255, 64, false, false),
                    (65, 255, 64, false, true),
                    (255, 255, 64, false, true),
                    (255, 64, 64, false, false),
                    (255, 65, 64, false, true),
                    (255, 255, 255, false, false),
                    // Reflective texture alpha=0 must still produce solid geometry.
                    (0, 255, -255, true, true),
                    (128, 255, -255, true, true),
                    // The alpha override occurs BEFORE vertex translucency and test.
                    (0, 64, 64, true, false),
                    (0, 65, 64, true, true),
                ] {
                    let mut front = make_mesh(
                        &pipeline,
                        0.2,
                        tex_alpha,
                        [255, 0, 0, vertex_alpha],
                        reference as f32 / 255.0,
                        reflective,
                    );
                    front.set_depth_write(depth_write);
                    let bundle = ModelBundle::new(
                        &device,
                        &pipeline,
                        &[&front, &background, &farthest],
                        wgpu::TextureFormat::Rgba8Unorm,
                        wgpu::TextureFormat::Depth24Plus,
                    );
                    assert!(bundle.matches(&[&front, &background, &farthest]));
                    assert!(!bundle.matches(&[&background, &front, &farthest]));
                    assert!(!bundle.matches(&[&front]));
                    let size = wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    };
                    let target = device.create_texture(&wgpu::TextureDescriptor {
                        label: None,
                        size,
                        mip_level_count: 1,
                        sample_count: samples,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    });
                    let depth = device.create_texture(&wgpu::TextureDescriptor {
                        label: None,
                        size,
                        mip_level_count: 1,
                        sample_count: samples,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Depth24Plus,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    });
                    let target_view = target.create_view(&Default::default());
                    let resolve = (samples > 1).then(|| {
                        device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("cutout resolve"),
                            size,
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                                | wgpu::TextureUsages::COPY_SRC,
                            view_formats: &[],
                        })
                    });
                    let resolve_view_owned = resolve
                        .as_ref()
                        .map(|texture| texture.create_view(&Default::default()));
                    let resolve_view = resolve_view_owned
                        .as_ref()
                        .map_or(&target_view, |view| view);
                    let depth_view = depth.create_view(&Default::default());
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: None,
                        size: 256,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: None,
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &target_view,
                                resolve_target: (samples > 1).then_some(resolve_view),
                                depth_slice: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: Some(
                                wgpu::RenderPassDepthStencilAttachment {
                                    view: &depth_view,
                                    depth_ops: Some(wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(1.0),
                                        store: wgpu::StoreOp::Store,
                                    }),
                                    stencil_ops: None,
                                },
                            ),
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });
                        pass.set_bind_group(0, &pipeline.uniform_bind_group, &[]);
                        if bundled {
                            pass.execute_bundles(std::iter::once(&bundle.bundle));
                        } else {
                            let mut current_alpha_test = None;
                            front.draw_model_cached(
                                &mut pass,
                                &pipeline,
                                &mut current_alpha_test,
                                true,
                            );
                            background.draw_model_cached(
                                &mut pass,
                                &pipeline,
                                &mut current_alpha_test,
                                true,
                            );
                            farthest.draw_model_cached(
                                &mut pass,
                                &pipeline,
                                &mut current_alpha_test,
                                true,
                            );
                        }
                    }
                    encoder.copy_texture_to_buffer(
                        if samples > 1 {
                            resolve.as_ref().unwrap().as_image_copy()
                        } else {
                            target.as_image_copy()
                        },
                        wgpu::TexelCopyBufferInfo {
                            buffer: &buffer,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(256),
                                rows_per_image: Some(1),
                            },
                        },
                        size,
                    );
                    queue.submit(Some(encoder.finish()));
                    let (tx, rx) = std::sync::mpsc::channel();
                    buffer
                        .slice(..)
                        .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
                    let _ = device.poll(wgpu::PollType::wait_indefinitely());
                    rx.recv().unwrap().unwrap();
                    let mapped = buffer.slice(..).get_mapped_range().expect("mapped range");
                    let expected = if survives && depth_write {
                        [255, 0, 0]
                    } else {
                        [0, 0, 255]
                    };
                    assert_eq!(
                &mapped[..3],
                &expected,
                "texture={tex_alpha} vertex={vertex_alpha} ref={reference} depth_write={depth_write} bundled={bundled}"
            );
                }
            }
        }
    }
}

/// A sloped receiver at Lumbridge world coordinates. Compare both floor
/// overlays against a reference with no receiver depth: 32 yaws x 4
/// pitches exposed shadow dropouts before local-frame projection.
#[test]
#[ignore = "requires a desktop GPU; run explicitly"]
fn shadow_depth_stable_under_camera_rotation() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: None,
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        memory_hints: Default::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        trace: wgpu::Trace::Off,
    }))
    .unwrap();
    let pipeline = FloorPipeline::new(
        &device,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Depth24Plus,
    );
    let passes = crate::floorpass::FloorPasses::new(
        &device,
        &pipeline,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Depth24Plus,
    );
    let origin = [3168.0 * 512.0, 0.0, 3168.0 * 512.0];
    let points = [
        [0.0, 0.0, 0.0],
        [4096.0, 721.0, 0.0],
        [4096.0, 978.0, 4096.0],
        [0.0, 257.0, 4096.0],
    ];
    let verts = points.map(|pos| FloorVertex {
        pos,
        uv: [pos[0], pos[2]],
        depth: 0.0,
        normal: [0.0, -1.0, 0.0],
    });
    let tex = white_texture(&device, &queue, &mut |s, t, m| {
        std::sync::Arc::new(make_sampler(&device, s, t, m))
    });
    let floor = FloorMesh {
        render_id: next_render_id(),
        model_uniform: None,
        depth_write: true,
        billboards: None,
        vertex_buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        }),
        shared_colours: Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: &[128, 128, 128, 255].repeat(4),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        ),
        batches: vec![FloorGpuBatch {
            colour_buffer: None,
            index_buffer: index_buffer(&device, "plane", &[0, 1, 2, 0, 2, 3]),
            index_count: 6,
            bind_group: batch_bind_group(
                &device,
                &pipeline,
                &tex,
                BatchUniforms::initial(1.0, -1.0, false, origin),
                "plane",
            ),
            material: -1,
            floor_batch: None,
            payload: None,
            alpha_test: false,
        }],
        vertex_count: 4,
    };
    let mut tile_tris = vec![None; 64];
    tile_tris[0] = Some(vec![0, 1, 2, 0, 2, 3]);
    let geometry = crate::floor::FloorGeometry {
        tiles_x: 8,
        tiles_z: 8,
        has_depth: false,
        has_normals: false,
        water_detail: false,
        underwater: false,
        stride_floats: 5,
        vertex_count: 4,
        index_count: 6,
        stream0: points
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], p[0], p[2]])
            .collect(),
        base_colours: vec![-1; 4],
        tile_tris,
        batches: vec![],
        hard_shadow: vec![0; 64],
        hard_shadows: None,
        min_y: -1.0,
        max_y: 1.0,
        heights: crate::floor::FloorHeights::new(8, 8, 512, vec![0; 81]),
        calls: None,
    };
    let mask_bytes = vec![1; 130 * 130];
    let mask = crate::floorpass::ShadowMaskView {
        width: 130,
        height: 130,
        mask: &mask_bytes,
        texel_shift: 5,
    };
    let shadow = crate::floorpass::ShadowMesh::build(
        &device,
        &queue,
        &passes,
        &geometry,
        &mask,
        origin,
        "constant shadow",
    );
    let mut light_vertices = Vec::new();
    for p in points {
        for value in p {
            light_vertices.extend_from_slice(&value.to_le_bytes());
        }
        light_vertices.extend_from_slice(&[64, 64, 64, 255]);
    }
    let baked = crate::floorlight::BakedLight {
        source_light: None,
        tile_x0: 0,
        tile_x1: 7,
        tile_z0: 0,
        tile_z1: 7,
        vertices: light_vertices,
        indices: vec![0, 1, 2, 0, 2, 3],
        intensity: 1.0,
        radius: 4096,
    };
    let light =
        crate::floorpass::LightMesh::build(&device, &passes, &baked, origin, "coplanar light")
            .unwrap();
    let size = wgpu::Extent3d {
        width: 256,
        height: 256,
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth24Plus,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let depth_view = depth.create_view(&Default::default());
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256 * 256 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let render = |with_floor: bool, light_mode: bool| {
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 128.0 / 255.0,
                            g: 128.0 / 255.0,
                            b: 128.0 / 255.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &pipeline.uniform_bind_group, &[]);
            if with_floor {
                pass.set_pipeline(pipeline.base_pipeline());
                floor.draw(&mut pass);
            }
            if light_mode {
                pass.set_pipeline(&passes.light_pipeline);
                light.draw(&mut pass);
            } else {
                pass.set_pipeline(&passes.shadow_pipeline);
                shadow.draw(&mut pass);
            }
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(1024),
                    rows_per_image: Some(256),
                },
            },
            size,
        );
        queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().unwrap().unwrap();
        let pixels = buffer
            .slice(..)
            .get_mapped_range()
            .expect("mapped range")
            .to_vec();
        buffer.unmap();
        pixels
    };
    let mut failures = Vec::new();
    let mut observed = 0;
    for light_mode in [false, true] {
        for pitch in [1077.0, 1500.0, 2200.0, 2787.0] {
            for yaw in (0..16384).step_by(512) {
                let mut camera = crate::camera::SceneCamera::new([
                    origin[0] as i32 + 2048,
                    -256,
                    origin[2] as i32 + 2048,
                ]);
                camera.viewport = (256, 256);
                camera.pitch = pitch;
                camera.yaw = yaw as f32;
                let env = crate::env::EnvFrame::default_for(1.0, 0.0, &camera.view_entries());
                let mut u = FloorUniforms::for_camera(&camera, &env);
                u.sun_colour = [0.0; 4];
                u.anti_sun_colour = [0.0; 4];
                u.ambient_colour = [1.0; 4];
                u.height_fog_plane = [0.0; 4];
                u.distance_fog_plane = [0.0; 4];
                queue.write_buffer(&pipeline.uniform_buffer, 0, bytemuck::bytes_of(&u));
                let reference = render(false, light_mode);
                let actual = render(true, light_mode);
                let mut missed = 0;
                for (want, got) in reference.chunks_exact(4).zip(actual.chunks_exact(4)) {
                    if if light_mode {
                        want[0] > 150
                    } else {
                        want[0] < 110
                    } {
                        observed += 1;
                        if if light_mode {
                            got[0] < 140
                        } else {
                            got[0] > 120
                        } {
                            missed += 1;
                        }
                    }
                }
                if missed != 0 {
                    failures.push((light_mode, yaw, pitch, missed));
                }
            }
        }
    }
    assert!(observed > 10000, "fixture must cover floor pixels");
    assert!(failures.is_empty(), "shadow depth dropouts: {failures:?}");
    // The baked mesh survives preference changes. Updating its uniform must
    // change the actual floor light pass, with DST_COLOR/ONE blending.
    light.set_intensity(&queue, 0.);
    let off = render(false, true);
    light.set_intensity(&queue, 0.5);
    let half = render(false, true);
    light.set_intensity(&queue, 1.);
    let full = render(false, true);
    let mut changed = 0;
    for ((off, half), full) in off
        .chunks_exact(4)
        .zip(half.chunks_exact(4))
        .zip(full.chunks_exact(4))
    {
        if full[0] > 150 {
            assert_eq!(&off[..3], &[128; 3]);
            assert!((half[0] as i32 - 144).abs() <= 1, "half intensity {half:?}");
            assert!((full[0] as i32 - 160).abs() <= 1, "full intensity {full:?}");
            changed += 1;
        }
    }
    assert!(
        changed > 100,
        "light uniform update must affect visible floor pixels"
    );
}

#[test]
fn mip_is_the_integer_box_average() {
    // 2x2 of 0xFF000000 | rgb: average of (10,20,30,40) = 25 per channel.
    let px = |v: u32| 0xFF00_0000 | (v << 16) | (v << 8) | v;
    let src = vec![px(10), px(20), px(30), px(40)];
    let out = box_mip(&src, 2, 2);
    assert_eq!(out, vec![px(25)]);
}

#[test]
fn gamma_lut_endpoints() {
    let lut = gamma_lut(0.7);
    assert_eq!(lut[0], 0);
    assert_eq!(lut[255], 255);
    // (128/255)^0.7 * 255 = 157.6 -> 157
    assert_eq!(lut[128], 157);
}
