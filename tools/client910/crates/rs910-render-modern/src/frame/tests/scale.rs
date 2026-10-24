//! The render scale (`frame::scale`): a scaled scene is upscaled into the
//! frame's viewport where the full-resolution frame puts it.
use super::*;

/// One frame of `snapshot` with `models` into a `size` target first cleared
/// to magenta, the scene in `rect` and its scissor `clip`: the output
/// pixels.
pub(super) fn framed(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    r: &mut ModernRenderer,
    snapshot: &SceneSnapshot<'_>,
    models: &[(crate::models::mesh::ModelStreams, [f32; 16])],
    size: [u32; 2],
    (rect, clip): ([i32; 4], [i32; 4]),
) -> Vec<u8> {
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scaled frame"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: 1.0,
                    g: 0.0,
                    b: 1.0,
                    a: 1.0,
                }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        occlusion_query_set: None,
        multiview_mask: None,
        timestamp_writes: None,
    });
    r.preparation.test_models = models.to_vec();
    r.draw(
        Target {
            device,
            queue,
            encoder: &mut encoder,
            view: &view,
            format: wgpu::TextureFormat::Rgba8Unorm,
            size,
            rect,
            clip,
        },
        snapshot,
    );
    queue.submit(Some(encoder.finish()));
    read_back(device, queue, &output, 4)
}

/// At 50% the scene renders at half the viewport's pixels (its HDR frame
/// finite, a quarter of the pixels) and is upscaled into the frame's
/// viewport under its scissor: nothing outside the scissor is written, and a
/// marker on the ground lands where the full-resolution frame, and the
/// camera's projection of the viewport (what the client picks with), put
/// it, within a pixel.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn render_scale_upscales_into_the_viewport_where_full_resolution_draws() {
    let _clock = fixed_clock();
    let (device, queue) = crate::test_support::require_gpu();
    let size = [800u32, 500];
    // A viewport inside the frame, its scissor cut on the left and bottom
    // (the UI over it).
    let rect = [100, 60, 600, 380];
    let clip = [140, 60, 700, 420];
    let (camera, mut env) = bare_camera([rect[2] as u32, rect[3] as u32]);
    env.sun.dir = [-0.3, -0.9, 0.3];
    let snapshot = bare(&camera, &env);
    let uniforms = frame_uniforms(&snapshot, (rect[2], rect[3]));
    let view_proj = glam::Mat4::from_cols_array_2d(&uniforms.view_proj);
    let eye = uniforms.eye;
    let len = (eye[0] * eye[0] + eye[2] * eye[2]).sqrt();
    let d = [-eye[0] / len, -eye[2] / len];
    let (cx, cz) = (26.0 * 512.0, 26.0 * 512.0);
    let identity = glam::Mat4::IDENTITY.to_cols_array();
    // The marker: a bright quad on the ground ahead of the target and to
    // the side (camera-local `at`).
    let at = [
        d[0] * 200.0 - d[1] * 150.0,
        -4.0,
        d[1] * 200.0 + d[0] * 150.0,
    ];
    let mut marker = flat_quad(cx + at[0], cz + at[2], 96.0, at[1]);
    marker.colours = vec![0xff40_f0f0; 4];
    let ground = (flat_quad(cx, cz, 6000.0, 0.0), identity);
    let expected = {
        let (x, y) = pixel_of(&view_proj, [rect[2] as u32, rect[3] as u32], at);
        (x + rect[0] as f32, y + rect[1] as f32)
    };
    let mut centres = Vec::new();
    for percent in [100, 50] {
        let settings = ModernSettings {
            render_scale: Some(crate::settings::RenderScale::percent(percent).unwrap()),
            ..ModernSettings::DEFAULT
        };
        let mut r = renderer(&device, &queue, 4, settings);
        let mut frame = |models: &[_]| {
            framed(
                &device,
                &queue,
                &mut r,
                &snapshot,
                models,
                size,
                (rect, clip),
            )
        };
        let without = frame(std::slice::from_ref(&ground));
        let with = frame(&[ground.clone(), (marker.clone(), identity)]);
        // The scene's own HDR frame: finite, at the scaled viewport's size.
        let hdr_tex = r.hdr_target().expect("the HDR frame");
        let dims = [hdr_tex.width(), hdr_tex.height()];
        let hdr = halves(&read_back(&device, &queue, hdr_tex, 8));
        assert!(
            hdr.iter().all(|v| v.is_finite()),
            "{percent}%: non-finite HDR"
        );
        if percent == 50 {
            assert_eq!(dims, [300, 190], "the scaled scene's targets");
        }
        // Outside the scissor the frame keeps its magenta; inside, the scene.
        let (mut outside, mut inside) = (0, 0);
        for y in 0..size[1] as i32 {
            for x in 0..size[0] as i32 {
                let i = ((y * size[0] as i32 + x) * 4) as usize;
                let magenta = with[i..i + 4] == [255, 0, 255, 255];
                let within = x >= clip[0] && x < clip[2] && y >= clip[1] && y < clip[3];
                outside += usize::from(!within && !magenta);
                inside += usize::from(within && magenta);
            }
        }
        assert_eq!(
            (outside, inside),
            (0, 0),
            "{percent}%: pixels written outside the scissor, or left inside it"
        );
        // The marker's centre: the pixels it changes.
        let (mut n, mut sx, mut sy) = (0.0_f64, 0.0, 0.0);
        for i in 0..(size[0] * size[1]) as usize {
            let diff: i32 = (0..3)
                .map(|c| (i32::from(with[i * 4 + c]) - i32::from(without[i * 4 + c])).abs())
                .sum();
            if diff > 30 {
                n += 1.0;
                sx += (i % size[0] as usize) as f64 + 0.5;
                sy += (i / size[0] as usize) as f64 + 0.5;
            }
        }
        assert!(n > 50.0, "{percent}%: the marker is not drawn ({n} pixels)");
        let centre = ((sx / n) as f32, (sy / n) as f32);
        eprintln!("{percent}%: marker centre {centre:?} ({n} pixels), projected {expected:?}");
        centres.push(centre);
    }
    let dist = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).hypot(a.1 - b.1);
    assert!(
        dist(centres[0], expected) < 1.5,
        "full resolution {centres:?} vs {expected:?}"
    );
    assert!(
        dist(centres[1], centres[0]) < 1.0,
        "the scaled marker moved: {centres:?}"
    );
}
