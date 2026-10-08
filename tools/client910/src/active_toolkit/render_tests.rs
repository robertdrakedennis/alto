/// Preparing the changed next UI must not rewrite any geometry, instance,
/// sprite, mask, or framebuffer texture read by the deferred old frame.
#[test]
#[ignore = "needs a GPU adapter and the cache"]
fn deferred_ui_keeps_model_and_layer_bytes_until_submission() -> anyhow::Result<()> {
    use std::rc::Rc;
    const SIZE: [u32; 2] = [128, 96];
    const STRIDE: u32 = 512;
    const RGBA_BYTES: u32 = 4;
    const CORNER: i32 = 24;
    const MODEL_EXTENT: i32 = 32;
    const MODEL_SHIFT: i32 = 20;
    const VERTICES: i32 = 3;
    const TWO_SIDED_FACES: i32 = 2;
    const BRIGHTNESS: i32 = 3;
    const QUADS_BEFORE_MODEL: usize = 1;
    const MODEL_FLAGS: i32 = 2048;
    const AMBIENT: i32 = 64;
    const CONTRAST: i32 = 768;
    const FACE_COLOUR: i16 = 127;
    const LAYER_ID: u64 = 17;
    const MINIMAP_SIZE: u32 = 32;
    const ALTERNATING_UI_FRAMES: usize = 3;
    const OLD_MINIMAP_COLOUR: i32 = 0xff2040a0u32 as i32;
    const NEW_MINIMAP_COLOUR: i32 = 0xffa04020u32 as i32;
    const OLD_COLOUR: i32 = 0xff804010u32 as i32;
    const NEW_COLOUR: i32 = 0xff108040u32 as i32;
    const IDENTITY: [f32; 16] = [
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ];
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let resources = crate::ui_models::Models::new(pack).resources()?;
    let mut raw = crate::modelunlit::ModelUnlit::merge(&[]);
    raw.vertex_count = VERTICES;
    raw.used_vertex_count = VERTICES;
    raw.vertex_x = vec![-MODEL_EXTENT, MODEL_EXTENT, 0];
    raw.vertex_y = vec![-MODEL_EXTENT, -MODEL_EXTENT, MODEL_EXTENT];
    raw.vertex_z = vec![0; VERTICES as usize];
    raw.face_count = TWO_SIDED_FACES;
    raw.face_source_models = None;
    raw.vertex_source_models = None;
    raw.face_vertex1 = vec![0, 0];
    raw.face_vertex2 = vec![1, 2];
    raw.face_vertex3 = vec![2, 1];
    raw.face_colour = vec![FACE_COLOUR; raw.face_count as usize];
    let model = crate::gpumodel::GpuModel::new(
        &crate::gpumodel::ModelStores {
            materials: &resources.materials,
            billboards: &resources.billboards,
            emitters: &resources.emitters,
        },
        &raw,
        crate::gpumodel::BuildParams {
            flags: MODEL_FLAGS,
            ambient: AMBIENT,
            contrast: CONTRAST,
            detail: 0,
        },
    )?;
    let camera = crate::camera::SceneCamera::new([0; 3]);
    let lighting = crate::env::EnvFrame::build(
        &Default::default(),
        crate::env::SunSettings {
            direction: [0., 0., -1.],
            brightness_pref: BRIGHTNESS,
            anti_macro: 0.,
        },
        false,
        crate::env::FogReference {
            far: camera.fog_reference().0,
            near_min: camera.fog_reference().1,
            view: &IDENTITY,
        },
    );
    let owner = Rc::new(());
    let output = |changed: bool, minimap: u64| {
        let mut painter = crate::ui_paint::Painter::new(SIZE);
        painter
            .fill([0, 0, SIZE[0] as i32, SIZE[1] as i32], OLD_COLOUR)
            .unwrap();
        painter.affine_image(
            crate::ui_paint::Image::External(LAYER_ID),
            [0., 0., CORNER as f32, 0., 0., CORNER as f32],
            -1,
            None,
        );
        painter.affine_image(
            crate::ui_paint::Image::External(minimap),
            [
                (SIZE[0] as i32 - CORNER) as f32,
                0.,
                SIZE[0] as f32,
                0.,
                (SIZE[0] as i32 - CORNER) as f32,
                CORNER as f32,
            ],
            -1,
            None,
        );
        let mut layer = crate::ui_paint::Painter::new(SIZE);
        layer
            .fill(
                [0, 0, SIZE[0] as i32, SIZE[1] as i32],
                if changed { OLD_COLOUR } else { NEW_COLOUR },
            )
            .unwrap();
        let mut model = model.clone();
        if changed {
            for x in &mut model.vx {
                *x += MODEL_SHIFT;
            }
        }
        crate::ui_output::Output {
            models: vec![crate::interface_model::Draw {
                owner: Rc::downgrade(&owner).into(),
                quad: QUADS_BEFORE_MODEL,
                before_scene: true,
                clip: [0, 0, SIZE[0] as i32, SIZE[1] as i32],
                model,
                depth_write: true,
                resources: resources.clone(),
                particles: Default::default(),
                draw_particles: false,
                particle_list: Vec::new(),
                space: crate::interface_model::DrawSpace {
                    matrix: IDENTITY,
                    projection: glam::Mat4::orthographic_rh_gl(
                        -MODEL_EXTENT as f32 * 2.,
                        MODEL_EXTENT as f32 * 2.,
                        -MODEL_EXTENT as f32 * 2.,
                        MODEL_EXTENT as f32 * 2.,
                        -1.,
                        1.,
                    )
                    .to_cols_array(),
                    lighting,
                },
            }],
            paint: painter.finish(),
            scene: None,
            scene_quad: usize::MAX,
            recording: Default::default(),
            postprocess: None,
            layers: vec![(LAYER_ID, layer.finish())],
        }
    };
    let mut gpu = pollster::block_on(crate::gpu_device::Device::headless(
        (SIZE[0], SIZE[1]),
        Default::default(),
        Default::default(),
    ))?;
    let mut renderer = Renderer::new(&gpu);
    renderer.set_threaded_composition(true);
    let minimap_plan = |colour| crate::minimap::BasePlan {
        size: MINIMAP_SIZE,
        first_level: 0,
        level_tiles: Vec::new(),
        marks: vec![crate::minimap::Mark::Fill {
            rect: [0, 0, MINIMAP_SIZE as i32, MINIMAP_SIZE as i32],
            colour,
        }],
    };
    let minimap_world = || rs910_render_gpu::render::MinimapWorld {
        geometries: &[],
        meshes: &mut [],
        base: (0, 0),
        size_z: 0,
    };
    // Use the actual minimap rendering/registration owner, before the second
    // UI slot exists, rather than a test-only external texture insertion.
    let minimap = renderer.render_minimap_base(
        &mut gpu,
        &minimap_plan(OLD_MINIMAP_COLOUR),
        minimap_world(),
        &lighting,
    )?;
    let capture = |gpu: &crate::gpu_device::Device,
                   frame: &rs910_render_gpu::render::Composition|
     -> anyhow::Result<Vec<u8>> {
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("owned UI proof"),
            size: wgpu::Extent3d {
                width: SIZE[0],
                height: SIZE[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: gpu.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(STRIDE * SIZE[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        frame.encode_under(&gpu.device, &mut encoder, &view);
        frame.encode_over(&gpu.device, &mut encoder, &view);
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(STRIDE),
                    rows_per_image: Some(SIZE[1]),
                },
            },
            target.size(),
        );
        gpu.submit(Some(encoder.finish()));
        let (send, done) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                send.send(result).unwrap();
            });
        gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
        done.recv()?
            .map_err(|error| anyhow::anyhow!("map: {error}"))?;
        let bytes = buffer.slice(..).get_mapped_range()?;
        Ok(bytes
            .chunks(STRIDE as usize)
            .flat_map(|row| row[..(SIZE[0] * RGBA_BYTES) as usize].iter().copied())
            .collect())
    };
    renderer.prepare_ui(&gpu, output(false, minimap))?;
    let old = renderer.take_composition();
    let expected = capture(&gpu, &old)?;
    renderer.prepare_ui(&gpu, output(true, minimap))?;
    let (old, actual) = std::thread::scope(|scope| {
        let gpu = &gpu;
        scope
            .spawn(move || {
                let pixels = capture(gpu, &old)?;
                Ok::<_, anyhow::Error>((old, pixels))
            })
            .join()
            .unwrap()
    })?;
    assert_eq!(
        actual, expected,
        "next UI preparation overwrote a deferred resource"
    );
    renderer.restore_composition(old);
    let next = renderer.take_composition();
    let changed = capture(&gpu, &next)?;
    renderer.restore_composition(next);
    assert_ne!(
        actual, changed,
        "changed model/layer inputs must reach their own frame"
    );
    assert!(
        actual
            .chunks_exact(RGBA_BYTES as usize)
            .zip(changed.chunks_exact(RGBA_BYTES as usize))
            .enumerate()
            .any(|(index, (old, new))| {
                let x = index % SIZE[0] as usize;
                let y = index / SIZE[0] as usize;
                x >= CORNER as usize && y >= CORNER as usize && old != new
            }),
        "the interface model change must be visible outside the changed framebuffer corner"
    );
    assert!(
        actual
            .chunks_exact(RGBA_BYTES as usize)
            .zip(changed.chunks_exact(RGBA_BYTES as usize))
            .enumerate()
            .any(|(index, (old, new))| {
                let x = index % SIZE[0] as usize;
                let y = index / SIZE[0] as usize;
                x < CORNER as usize && y < CORNER as usize && old != new
            }),
        "the framebuffer layer change must visibly reach its own frame"
    );
    assert!(actual
        .chunks_exact(RGBA_BYTES as usize)
        .any(|pixel| pixel[..3].iter().any(|&channel| channel > 0)));

    let mut frozen = None;
    for attempt in 0..ALTERNATING_UI_FRAMES {
        renderer.prepare_ui(&gpu, output(false, minimap))?;
        let frame = renderer.take_composition();
        assert_eq!(
            capture(&gpu, &frame)?,
            expected,
            "minimap missing in an alternating UI slot"
        );
        if attempt + 1 == ALTERNATING_UI_FRAMES {
            frozen = Some(frame);
        } else {
            renderer.restore_composition(frame);
        }
    }
    let frozen = frozen.expect("retained old minimap frame");
    renderer.release_minimap_base(minimap);
    assert!(
        renderer.prepare_ui(&gpu, output(false, minimap)).is_err(),
        "released minimap must leave the next UI slot"
    );
    assert_eq!(
        capture(&gpu, &frozen)?,
        expected,
        "release invalidated a captured minimap bind group"
    );
    let replacement = renderer.render_minimap_base(
        &mut gpu,
        &minimap_plan(NEW_MINIMAP_COLOUR),
        minimap_world(),
        &lighting,
    )?;
    assert_ne!(
        replacement, minimap,
        "new base gets a new external identity"
    );
    assert_eq!(
        capture(&gpu, &frozen)?,
        expected,
        "replacement changed the retained minimap texture"
    );
    renderer.restore_composition(frozen);
    let mut replacement_pixels = None;
    for _ in 0..ALTERNATING_UI_FRAMES {
        assert!(
            renderer.prepare_ui(&gpu, output(false, minimap)).is_err(),
            "released minimap must leave both existing slots"
        );
        renderer.prepare_ui(&gpu, output(false, replacement))?;
        let frame = renderer.take_composition();
        let pixels = capture(&gpu, &frame)?;
        if let Some(expected) = &replacement_pixels {
            assert_eq!(&pixels, expected, "rebuilt minimap differs between slots");
        } else {
            assert!(
                pixels
                    .chunks_exact(RGBA_BYTES as usize)
                    .zip(expected.chunks_exact(RGBA_BYTES as usize))
                    .enumerate()
                    .any(|(index, (new, old))| {
                        let x = index % SIZE[0] as usize;
                        let y = index / SIZE[0] as usize;
                        x >= SIZE[0] as usize - CORNER as usize && y < CORNER as usize && new != old
                    }),
                "replacement minimap must visibly reach its own frame"
            );
            replacement_pixels = Some(pixels);
        }
        renderer.restore_composition(frame);
    }
    Ok(())
}

/// The ordinary bounded worker returns canceled producer progress and all
/// owned slots before resize, backend and recovery barriers can mutate them.
#[test]
#[ignore = "needs a GPU adapter"]
fn headless_worker_returns_every_skipped_frame_before_barriers() -> anyhow::Result<()> {
    const SIZE: [u32; 2] = [128, 96];
    const RESIZED: [u32; 2] = [160, 120];
    const CANVAS_ORIGIN: [i32; 2] = [0; 2];
    const INSET_CANVAS_SIZE: [i32; 2] = [96, 72];
    const INSET_CANVAS_OFFSET: [i32; 2] = [12, 8];
    const ATTEMPTS: u64 = 3;
    const CAPTURE_TIME: i64 = 1_700_000_000_000;
    const TILE_SHIFT: i32 = 9;
    const LEVELS: usize = 1;
    const TILES: usize = 2;
    const PAINT_COLOUR: i32 = 0xff804010u32 as i32;
    const SCENE_QUAD: usize = 1;
    const SINGLE_SAMPLE: u32 = 1;
    const FIRST_ATTEMPT: u64 = 0;
    const SAMPLE_CHOICES: [u32; 2] = [2, 4];
    let mut toolkit = pollster::block_on(ActiveToolkit::headless(
        (SIZE[0], SIZE[1]),
        RendererKind::Modern,
    ))?;
    let canvas = rs910_toolkit::game_canvas::Canvas {
        size: SIZE.map(|value| value as i32),
        offset: CANVAS_ORIGIN,
    };
    let inset_canvas = rs910_toolkit::game_canvas::Canvas {
        size: INSET_CANVAS_SIZE,
        offset: INSET_CANVAS_OFFSET,
    };
    assert!(!toolkit.gpu.game_canvas_matches(&toolkit.device, canvas));
    toolkit.set_game_canvas(canvas)?;
    assert!(toolkit.gpu.game_canvas_matches(&toolkit.device, canvas));
    assert!(toolkit.gpu.scene_effects_match(SINGLE_SAMPLE, false));
    assert_eq!(toolkit.modern_samples, None);
    let changed_samples = SAMPLE_CHOICES
        .into_iter()
        .find(|&count| toolkit.supports_scene_samples(count))
        .expect("a multisample target");
    let meshes = crate::scene_meshes::SceneMeshes::default();
    let orbit = crate::render::OrbitCamera::new(glam::Vec3::ZERO);
    let scene = crate::scene::Scene::new(TILE_SHIFT, LEVELS, TILES, TILES);
    let mut camera = crate::camera::SceneCamera::new([0; 3]);
    camera.viewport = (SIZE[0] as i32, SIZE[1] as i32);
    let (far, near) = camera.fog_reference();
    let env = crate::env::EnvFrame::default_for(far, near, &camera.view_entries());
    let output = || {
        let mut painter = crate::ui_paint::Painter::new(SIZE);
        painter
            .fill([0, 0, SIZE[0] as i32, SIZE[1] as i32], PAINT_COLOUR)
            .unwrap();
        crate::ui_output::Output {
            paint: painter.finish(),
            models: Vec::new(),
            recording: Default::default(),
            postprocess: None,
            layers: Vec::new(),
            scene_quad: SCENE_QUAD,
            scene: Some(rs910_toolkit::frame_plan::Scene {
                rect: [0, 0, SIZE[0] as i32, SIZE[1] as i32],
                clip: [0, 0, SIZE[0] as i32, SIZE[1] as i32],
            }),
        }
    };
    for attempt in 0..ATTEMPTS {
        let snapshot = crate::scene_snapshot::SceneSnapshot {
            owned: None,
            time_ms: Some(CAPTURE_TIME + attempt as i64),
            camera: camera.clone(),
            env: &env,
            live: None,
            scene: Some(&scene),
            floors: &[],
            lights: &[],
            players: None,
            floor_base: [0; 2],
            materials: None,
            pack: None,
            blackout: false,
            local_player: None,
            particles: None,
            underwater: None,
            sky: None,
        };
        toolkit.prepare_ui(output())?;
        toolkit.set_frame_identity(attempt, attempt as i32);
        toolkit.frame_scene(&snapshot, &meshes, None, &orbit)?;
        assert!(matches!(toolkit.backend, Backend::Pending));
        if attempt == FIRST_ATTEMPT {
            // The faithful resources already match, but the modern desired
            // count still needs initialization while its first frame is owned.
            assert_eq!(toolkit.modern_samples, None);
            toolkit.set_scene_effects(SINGLE_SAMPLE, false)?;
            let Backend::Modern(modern) = &toolkit.backend else {
                panic!("initial sample selection must return the pending renderer")
            };
            assert_eq!(modern.samples(), SINGLE_SAMPLE);
            assert_eq!(toolkit.modern_samples, Some(SINGLE_SAMPLE));
            toolkit.prepare_ui(output())?;
            toolkit.frame_scene(&snapshot, &meshes, None, &orbit)?;
            assert!(matches!(toolkit.backend, Backend::Pending));
        }
        toolkit.set_scene_effects(SINGLE_SAMPLE, false)?;
        assert!(
            matches!(toolkit.backend, Backend::Pending),
            "unchanged resources must not drain the in-flight frame"
        );
        toolkit.set_game_canvas(canvas)?;
        assert!(
            matches!(toolkit.backend, Backend::Pending),
            "equal canvas must leave the captured frame pending"
        );
        // This uses the other writable UI slot while the captured one is owned
        // by the persistent runtime thread, rather than a test-only thread.
        toolkit.prepare_ui(output())?;
        toolkit.finish_render()?;
        let Backend::Modern(modern) = &toolkit.backend else {
            panic!("returned modern renderer")
        };
        assert_eq!(
            modern.frames(),
            0,
            "occluded prepare must not submit a scene"
        );
        assert_eq!(
            toolkit.snapshot_spare.as_ref().unwrap().snapshot().time_ms,
            snapshot.time_ms
        );
    }
    let snapshot = crate::scene_snapshot::SceneSnapshot {
        owned: None,
        time_ms: Some(CAPTURE_TIME),
        camera,
        env: &env,
        live: None,
        scene: Some(&scene),
        floors: &[],
        lights: &[],
        players: None,
        floor_base: [0; 2],
        materials: None,
        pack: None,
        blackout: false,
        local_player: None,
        particles: None,
        underwater: None,
        sky: None,
    };
    toolkit.frame_scene(&snapshot, &meshes, None, &orbit)?;
    assert!(!toolkit.gpu.game_canvas_matches(&toolkit.device, inset_canvas));
    toolkit.set_game_canvas(inset_canvas)?;
    assert!(
        matches!(toolkit.backend, Backend::Modern(_)),
        "changed canvas consumes the pending frame"
    );
    let [inset_width, inset_height] = INSET_CANVAS_SIZE;
    assert_eq!(toolkit.gpu.size(), (inset_width as u32, inset_height as u32));
    assert!(toolkit.gpu.game_canvas_matches(&toolkit.device, inset_canvas));
    toolkit.set_game_canvas(canvas)?;
    toolkit.prepare_ui(output())?;
    toolkit.frame_scene(&snapshot, &meshes, None, &orbit)?;
    toolkit.set_scene_effects(changed_samples, false)?;
    assert!(
        matches!(toolkit.backend, Backend::Modern(_)),
        "changed settings consume the pending frame"
    );
    assert!(toolkit.gpu.scene_effects_match(changed_samples, false));
    let Backend::Modern(modern) = &toolkit.backend else {
        unreachable!()
    };
    assert_eq!(modern.samples(), toolkit.modern_samples.unwrap());
    // MSAA changes offscreen ownership even with the same logical canvas.
    assert!(!toolkit.gpu.game_canvas_matches(&toolkit.device, canvas));
    toolkit.prepare_ui(output())?;
    toolkit.frame_scene(&snapshot, &meshes, None, &orbit)?;
    toolkit.set_game_canvas(canvas)?;
    assert!(matches!(toolkit.backend, Backend::Modern(_)));
    assert!(toolkit.gpu.game_canvas_matches(&toolkit.device, canvas));
    toolkit.set_scene_effects(SINGLE_SAMPLE, false)?;
    toolkit.set_game_canvas(canvas)?;
    toolkit.prepare_ui(output())?;
    toolkit.frame_scene(&snapshot, &meshes, None, &orbit)?;
    toolkit.resize(RESIZED[0], RESIZED[1]);
    assert!(
        matches!(toolkit.backend, Backend::Modern(_)),
        "resize consumes the pending event"
    );
    assert_eq!(toolkit.gpu.size(), (RESIZED[0], RESIZED[1]));
    assert!(!toolkit.gpu.game_canvas_matches(&toolkit.device, canvas));
    toolkit.set_game_canvas(canvas)?;
    assert!(toolkit.gpu.game_canvas_matches(&toolkit.device, canvas));
    toolkit.prepare_ui(output())?;
    toolkit.frame_scene(&snapshot, &meshes, None, &orbit)?;
    toolkit.set_toolkit0(true);
    assert!(matches!(toolkit.backend, Backend::Gpu));
    toolkit.set_toolkit0(false);
    toolkit.prepare_ui(output())?;
    toolkit.frame_scene(&snapshot, &meshes, None, &orbit)?;
    toolkit.recover(Recovery::Recreate)?;
    assert!(!matches!(toolkit.backend, Backend::Pending));
    assert!(!toolkit.gpu.game_canvas_matches(&toolkit.device, canvas));
    toolkit.set_game_canvas(canvas)?;
    assert!(toolkit.gpu.game_canvas_matches(&toolkit.device, canvas));
    toolkit.set_scene_effects(SINGLE_SAMPLE, false)?;
    assert_eq!(toolkit.modern_samples, Some(SINGLE_SAMPLE));
    toolkit.prepare_ui(output())?;
    toolkit.frame_scene(&snapshot, &meshes, None, &orbit)?;
    toolkit.set_scene_effects(SINGLE_SAMPLE, false)?;
    assert!(matches!(toolkit.backend, Backend::Pending));
    toolkit.set_game_canvas(canvas)?;
    assert!(matches!(toolkit.backend, Backend::Pending));
    toolkit.finish_render()?;
    let Backend::Modern(modern) = &toolkit.backend else {
        unreachable!()
    };
    assert_eq!(modern.samples(), SINGLE_SAMPLE);
    toolkit.wait_idle();
    assert_eq!(toolkit.device.health.validation_errors(), 0);
    Ok(())
}
