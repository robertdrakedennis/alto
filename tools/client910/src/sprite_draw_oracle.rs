//! Recorded batch geometry and cache GLSL versus the production sprite renderer.
use crate::{
    sprite_draw::{self, Painter},
    ui_component_fields::Fields,
    ui_sprites::Sprite,
};
use std::{io::Write, rc::Rc};
#[derive(Clone)]
struct Op {
    kind: i32,
    sprite: usize,
    a: [i32; 16],
}
fn op(kind: i32, sprite: usize, a: &[i32]) -> Op {
    let mut b = [0; 16];
    b[..a.len()].copy_from_slice(a);
    Op { kind, sprite, a: b }
}
fn bits(f: f32) -> i32 {
    f.to_bits() as i32
}
#[derive(Clone)]
struct Case {
    gpu: bool,
    size: [u32; 2],
    clip: [i32; 4],
    sprites: Vec<Rc<Sprite>>,
    ops: Vec<Op>,
}
fn synthetic(padding: [i32; 4], variant: i32) -> Rc<Sprite> {
    Rc::new(Sprite {
        paletted: None,
        size: [7, 5],
        padding,
        argb: (0..35)
            .map(|i| {
                let alpha = match variant {
                    0 => 255,
                    1 => {
                        if i % 3 == 0 {
                            0
                        } else {
                            255
                        }
                    }
                    _ => i * 37 % 256,
                };
                alpha << 24 | (i * 31 % 256) << 16 | (i * 13 % 256) << 8 | (i * 7 % 256)
            })
            .collect(),
    })
}
fn suite(sprites: Vec<Rc<Sprite>>, clip: [i32; 4], variant: i32, gpu: bool) -> Case {
    let mut ops = vec![];
    let colour = [
        -1,
        0x009abcde,
        0x019abcde,
        0x7fc48e59,
        0xfe55aa22u32 as i32,
        0xff3399eeu32 as i32,
    ][variant as usize % 6];
    let angle = [0, 1, 16384, 32767, 49152, 71317][variant as usize % 6];
    let xy = [-7, 3, 55, 0, 13, -20][variant as usize % 6];
    for (i, sprite) in sprites.iter().enumerate() {
        let full = sprite.full_size();
        ops.push(op(0, i, &[xy, 9, colour, variant % 3, variant % 4]));
        ops.push(op(
            1,
            i,
            &[-3, 5, 57, 37, colour, variant % 3, variant % 4, 1],
        ));
        // Bounded positive periods: divergent tiling is not executed.
        if full.iter().all(|n| *n > 0) {
            ops.push(op(2, i, &[-4, 3, 69, 43, colour, variant % 3, variant % 4]));
        }
        ops.push(op(
            3,
            i,
            &[
                bits(-8.125),
                bits(7.375),
                bits(50.625),
                bits(-3.25),
                bits(9.75),
                bits(51.875),
                colour,
                variant % 3,
                variant % 4,
                1,
            ],
        ));
        for kind in [4, 5] {
            ops.push(op(
                kind,
                i,
                &[
                    bits(31.125),
                    bits(23.625),
                    bits(full[0] as f32 / 2.),
                    bits(full[1] as f32 / 2.),
                    4096,
                    3072,
                    angle,
                    colour,
                    variant % 3,
                    variant % 4,
                ],
            ));
        }
        for tiling in [0, 1] {
            for tint in [-1, 0, 0xffabcdefu32 as i32] {
                ops.push(op(
                    6,
                    i,
                    &[
                        3,
                        7,
                        49,
                        31,
                        tint,
                        tiling,
                        angle,
                        variant * 51,
                        clip[0],
                        clip[1],
                        clip[2],
                        clip[3],
                    ],
                ));
            }
        }
    }
    // Retained clip transitions, including the bounds reset's width-1 shortcut.
    ops.push(op(8, 0, &[0, 0, 63, 47]));
    ops.push(op(
        3,
        0,
        &[
            bits(-1.),
            bits(-1.),
            bits(65.),
            bits(2.),
            bits(1.),
            bits(49.),
            -1,
            1,
            1,
            1,
        ],
    ));
    ops.push(op(0, 0, &[61, 41, -1, 1, 1]));
    ops.push(op(7, 0, &[-1, -1, 63, 47]));
    Case {
        gpu,
        size: [64, 48],
        clip,
        sprites,
        ops,
    }
}
/// The sprite draw corpus: synthetic sprites and clips, degenerate extents, and
/// every cached sprite that the recorded login interfaces draw.
fn cases() -> anyhow::Result<Vec<Case>> {
    let mut cases = vec![];
    for (pi, pad) in [
        [0; 4],
        [2, 3, 4, 1],
        [-1, -2, 3, 4],
        [0, 0, 8, 9],
        [8, 9, 0, 0],
    ]
    .into_iter()
    .enumerate()
    {
        for (ci, clip) in [
            [0, 0, 64, 48],
            [9, 7, 51, 39],
            [0, 0, 63, 47],
            [60, 44, 64, 48],
        ]
        .into_iter()
        .enumerate()
        {
            for variant in 0..6 {
                cases.push(suite(
                    vec![synthetic(pad, variant % 3), synthetic([1, 2, 3, 4], 2)],
                    clip,
                    variant,
                    pi < 2 && ci < 3,
                ));
            }
        }
    }
    // Negative/zero extents, 32-bit integer overflow, zero scale, invalid clip and
    // division failures. No unbounded tiling loops or undefined GPU triangles.
    for pad in [[0; 4], [-7, 0, 0, 0], [0, -5, 0, 0], [2, 3, 4, 1]] {
        let mut c = Case {
            gpu: false,
            size: [63, 47],
            clip: [0, 0, 63, 47],
            sprites: vec![synthetic(pad, 2)],
            ops: vec![],
        };
        for v in [i32::MIN, -4096, -1, 0, 1, 4096, i32::MAX] {
            c.ops.push(op(0, 0, &[v, v, -1, 0, 1]));
            c.ops.push(op(1, 0, &[4, 5, v, v, -1, 0, 1, 1]));
            c.ops.push(op(
                4,
                0,
                &[bits(31.), bits(23.), bits(0.), bits(-0.), v, v, v, -1, 1, 1],
            ));
            for tint in [-1, 0x336699] {
                c.ops
                    .push(op(6, 0, &[4, 5, v, v, tint, 0, 1, 0, 0, 0, 63, 47]));
            }
        }
        c.ops.push(op(8, 0, &[50, 40, 3, 2]));
        c.ops.push(op(0, 0, &[0, 0, -1, 1, 1]));
        cases.push(c);
    }
    // The graphics of the sprite components a recorded login creates (third
    // column of the recorded requests).
    let ids: std::collections::BTreeSet<u32> =
        rs910_core::test_support::frozen::text("ui-sprites/requests.tsv")
            .lines()
            .map(|line| line.split('\t').nth(2).unwrap().parse().unwrap())
            .collect();
    let pack = crate::cache::Pack::open(rs910_core::test_support::pack_root());
    let mut actual_ids = vec![];
    for id in ids {
        let Some(bytes) = crate::js5_fetch::fetch_file(&pack, "sprites", id)? else {
            continue;
        };
        let frames = crate::sprite_data::Data::decode(&bytes)?;
        let Some(data) = frames.first() else { continue };
        let sprite = Rc::new(Sprite::new(data)?);
        let n = actual_ids.len();
        actual_ids.push(id);
        cases.push(suite(
            vec![sprite, synthetic([1, 2, 3, 4], 2)],
            [5, 3, 61, 45],
            (n % 6) as i32,
            n % 17 == 0,
        ));
    }
    for clip in [[0, 0, 64, 48], [9, 7, 51, 39], [0, 0, 63, 47]] {
        for variant in 0..12 {
            let colour = [-1, 0, 0x7fabcd12, 0x01ffffff][variant % 4];
            let sprite = Rc::new(Sprite {
                paletted: None,
                size: [1, 1],
                padding: [0; 4],
                argb: vec![-1],
            });
            let mut c = Case {
                gpu: true,
                size: [64, 48],
                clip,
                sprites: vec![sprite],
                ops: vec![],
            };
            for width in [0, 1, 2, 3, 7] {
                for (from, to) in [
                    ([3, 5], [51, 5]),
                    ([17, 39], [17, 2]),
                    ([13, 41], [57, 11]),
                    ([61, 4], [7, 37]),
                    ([31, 7], [31, 7]),
                ] {
                    c.ops.push(op(
                        9,
                        0,
                        &[
                            from[0],
                            from[1],
                            to[0],
                            to[1],
                            colour,
                            width,
                            (variant % 3) as i32,
                        ],
                    ));
                }
            }
            for rect in [
                [-4, 5, 31, 17],
                [9, 9, 0, 0],
                [5, 13, 1, 1],
                [9, 9, 11, 19],
                [60, 44, 5, 5],
            ] {
                for kind in [10, 11] {
                    c.ops.push(op(
                        kind,
                        0,
                        &[
                            rect[0],
                            rect[1],
                            rect[2],
                            rect[3],
                            colour,
                            (variant % 3) as i32,
                        ],
                    ));
                }
            }
            cases.push(c);
        }
    }
    // Isolate each leaf as well as retaining overlapping mixed-texture frames.
    // A later opaque draw must not hide an earlier geometry/filtering error.
    let isolated: Vec<_> = cases
        .iter()
        .filter(|c| c.gpu)
        .flat_map(|c| {
            c.ops
                .iter()
                .filter(|op| op.kind <= 6 || op.kind >= 9)
                .map(|op| {
                    let mut one = c.clone();
                    one.ops = vec![op.clone()];
                    one
                })
        })
        .collect();
    cases.extend(isolated);
    Ok(cases)
}
fn apply(p: &mut Painter, sprites: &[Rc<Sprite>], op: &Op) -> anyhow::Result<()> {
    let a = op.a;
    let s = &sprites[op.sprite];
    let f = |i: usize| f32::from_bits(a[i] as u32);
    match op.kind {
        0 => p.sprite(s, [a[0], a[1]], a[2]),
        1 => p.scaled(s, [a[0], a[1], a[2], a[3]], a[4])?,
        2 => p.tiled(s, [a[0], a[1], a[2], a[3]], a[4])?,
        3 => p.affine(s, [f(0), f(1), f(2), f(3), f(4), f(5)], a[6]),
        4 | 5 => p.rotated(
            s,
            [f(0), f(1)],
            [f(2), f(3)],
            [a[4], if op.kind == 5 { a[4] } else { a[5] }],
            a[6],
            a[7],
        ),
        6 => {
            let fields = Fields {
                width: a[2],
                height: a[3],
                colour: a[4],
                tiling: a[5] != 0,
                angle2d: a[6],
                ..Default::default()
            };
            p.component(s, &fields, [a[0], a[1]], a[7], [a[8], a[9], a[10], a[11]])?;
        }
        7 => p.reset_bounds([a[0], a[1], a[2], a[3]]),
        8 => p.set_bounds([a[0], a[1], a[2], a[3]]),
        9..=11 => {
            let mut painter = crate::ui_paint::Painter::new(p.size);
            painter.sprite.clip = p.clip;
            match op.kind {
                9 => painter.line([a[0], a[1]], [a[2], a[3]], a[4], a[5]),
                10 => painter.fill([a[0], a[1], a[2], a[3]], a[4])?,
                _ => painter.outline([a[0], a[1], a[2], a[3]], a[4]),
            }
            p.clip = painter.sprite.clip;
            p.quads.extend(
                painter
                    .finish()
                    .quads
                    .into_iter()
                    .map(|q| crate::sprite_draw::Quad {
                        sprite: s.clone(),
                        vertices: q.vertices,
                        clip: q.clip,
                    }),
            );
        }
        _ => unreachable!(),
    }
    Ok(())
}
fn int(w: &mut impl Write, v: i32) -> std::io::Result<()> {
    w.write_all(&v.to_be_bytes())
}
fn float(w: &mut impl Write, v: f32) -> std::io::Result<()> {
    int(w, if v.is_nan() { 0x7fc00000 } else { bits(v) })
}
/// The directory of the GPU pixel comparison (`tools/oracle/run-gpu-pixels.sh`
/// sets `CLIENT910_SPRITE_DRAW_REPLAY` and prepares the reference in it).
fn gpu_dir() -> std::path::PathBuf {
    std::env::var_os("CLIENT910_SPRITE_DRAW_REPLAY").map_or_else(
        || panic!("CLIENT910_SPRITE_DRAW_REPLAY is not set: run this test through tools/oracle/run-gpu-pixels.sh"),
        std::path::PathBuf::from,
    )
}
/// Quads of every sprite draw case against the frozen recording of the
/// original client's sprite renderer. With `CLIENT910_SPRITE_DRAW_REPLAY` set
/// the case inputs are also written there for the GPU pixel comparison.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn export() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("sprite-draw");
    let out = std::env::var_os("CLIENT910_SPRITE_DRAW_REPLAY")
        .map_or_else(|| scratch.dir().to_path_buf(), std::path::PathBuf::from);
    let cases = cases()?;
    let mut input = std::io::BufWriter::new(std::fs::File::create(out.join("input.bin"))?);
    let mut result = std::io::BufWriter::new(std::fs::File::create(out.join("rust-quads.bin"))?);
    for a in 0..65536 {
        for f in sprite_draw::trig(a) {
            float(&mut result, f)?;
        }
    }
    int(&mut input, cases.len() as i32)?;
    let mut count = 0;
    for c in cases {
        int(&mut input, c.gpu as i32)?;
        for n in c.size {
            int(&mut input, n as i32)?;
        }
        for n in c.clip {
            int(&mut input, n)?;
        }
        int(&mut input, c.sprites.len() as i32)?;
        for s in &c.sprites {
            for n in s.size.into_iter().chain(s.padding) {
                int(&mut input, n)?;
            }
            for n in &s.argb {
                int(&mut input, *n)?;
            }
        }
        int(&mut input, c.ops.len() as i32)?;
        let mut p = Painter::new(c.size);
        p.reset_bounds(c.clip);
        for op in c.ops {
            int(&mut input, op.kind)?;
            int(&mut input, op.sprite as i32)?;
            for a in op.a {
                int(&mut input, a)?;
            }
            let ok = apply(&mut p, &c.sprites, &op).is_ok();
            int(&mut result, ok as i32)?;
            for a in p.clip {
                int(&mut result, a)?;
            }
            int(&mut result, p.quads.len() as i32)?;
            count += p.quads.len();
            for q in p.quads.drain(..) {
                int(
                    &mut result,
                    c.sprites
                        .iter()
                        .position(|s| Rc::ptr_eq(s, &q.sprite))
                        .unwrap() as i32,
                )?;
                for n in q.clip {
                    int(&mut result, n)?;
                }
                for v in q.vertices {
                    for n in v.position.into_iter().chain(v.uv) {
                        float(&mut result, n)?;
                    }
                }
                let c = q.vertices[0].colour;
                int(&mut result, i32::from_be_bytes([c[3], c[0], c[1], c[2]]))?;
            }
        }
    }
    input.flush()?;
    result.flush()?;
    rs910_core::test_support::frozen::assert_stream(
        "sprite-draw/quads",
        &std::fs::read(out.join("rust-quads.bin"))?,
    );
    println!("Rust sprite draw: {count} quads");
    Ok(())
}

#[test]
#[ignore = "desktop GPU and real cache GLSL; run tools/oracle/run-gpu-pixels.sh"]
fn pixels() -> anyhow::Result<()> {
    let out = gpu_dir();
    let reference = std::fs::read(out.join("reference.rgba"))?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: None,
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        memory_hints: Default::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        trace: wgpu::Trace::Off,
    }))?;
    let mut renderer =
        crate::sprite_draw_gpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    let mut actual = Vec::new();
    for (frame, c) in cases()?.into_iter().filter(|c| c.gpu).enumerate() {
        let mut painter = Painter::new(c.size);
        painter.reset_bounds(c.clip);
        for op in &c.ops {
            apply(&mut painter, &c.sprites, op)?;
        }
        renderer.prepare(&device, &queue, painter)?;
        let size = wgpu::Extent3d {
            width: c.size[0],
            height: c.size[1],
            depth_or_array_layers: 1,
        };
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sprite oracle"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sprite proof"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            renderer.draw(&mut pass);
        }
        let stride = (c.size[0] * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: stride as u64 * c.size[1] as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(c.size[1]),
                },
            },
            size,
        );
        queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r).unwrap();
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv()??;
        let bytes = buffer.slice(..).get_mapped_range().expect("mapped range");
        for row in bytes.chunks(stride as usize) {
            actual.extend_from_slice(&row[..c.size[0] as usize * 4]);
        }
        drop(bytes);
        buffer.unmap();
        if [0, 36, 55].contains(&frame) {
            let start = actual.len() - c.size[0] as usize * c.size[1] as usize * 4;
            let rgb: Vec<u8> = actual[start..]
                .chunks_exact(4)
                .flat_map(|p| p[..3].iter().copied())
                .collect();
            crate::png_out::write_rgb_png(
                &out.join(format!("sprite-{frame}.png")),
                c.size[0],
                c.size[1],
                &rgb,
            )?;
        }
    }
    std::fs::write(out.join("rust.rgba"), &actual)?;
    anyhow::ensure!(actual.len() == reference.len(), "pixel lengths");
    let mut max = 0u8;
    let mut differing = 0;
    let mut ordinary_max = 0;
    let mut boundary_pixels = 0;
    let mut at = 0;
    let mut failures = vec![];
    for (frame, c) in cases()?.into_iter().filter(|c| c.gpu).enumerate() {
        let mut painter = Painter::new(c.size);
        painter.reset_bounds(c.clip);
        for op in &c.ops {
            apply(&mut painter, &c.sprites, op)?;
        }
        for y in 0..c.size[1] {
            for x in 0..c.size[0] {
                let a = &actual[at..at + 4];
                let b = &reference[at..at + 4];
                at += 4;
                let delta = a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
                max = max.max(delta);
                differing += a.iter().zip(b).filter(|(a, b)| a != b).count();
                if delta <= 2 {
                    ordinary_max = ordinary_max.max(delta);
                    continue;
                }
                let candidates = edge_colours(&painter, x, y);
                let possible = |p: &[u8]| {
                    candidates
                        .iter()
                        .any(|c| c.iter().zip(p).all(|(a, b)| a.abs_diff(*b) <= 2))
                };
                if possible(a) && possible(b) {
                    boundary_pixels += 1;
                } else if failures.len() < 20 {
                    failures.push(serde_json::json!({"frame":frame,"x":x,"y":y,"wgpu":a,"gl":b,"candidates":candidates}));
                }
            }
        }
    }
    std::fs::write(
        out.join("pixels.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"bytes":actual.len(),"max_channel_error":max,"max_nonboundary_error":ordinary_max,"raster_edge_pixels":boundary_pixels,"differing_channels":differing,"first_failures":failures}),
        )?,
    )?;
    anyhow::ensure!(
        failures.is_empty(),
        "sprite GPU first mismatches {failures:?}"
    );
    println!("PASS sprite GPU {} bytes, ordinary max {ordinary_max}/255, {boundary_pixels} independently verified raster edge pixels",actual.len());
    Ok(())
}

/// Independent f64 triangle interpolation, linear-repeat texture lookup and
/// ordered ARGB composition. Only a quad edge within 1/256 pixel of the sample
/// may choose either coverage outcome. Both GPU outputs must match a complete
/// candidate composition within two channel units; other pixels get no waiver.
fn edge_colours(painter: &Painter, x: u32, y: u32) -> Vec<[u8; 4]> {
    let point = [x as f64 + 0.5, y as f64 + 0.5];
    let mut states = vec![[26, 51, 77, 102]];
    let mut boundary = false;
    for q in &painter.quads {
        if (x as i32) < q.clip[0]
            || (y as i32) < q.clip[1]
            || (x as i32) >= q.clip[2]
            || (y as i32) >= q.clip[3]
        {
            continue;
        }
        let v = q.vertices.map(|v| {
            [
                (v.position[0] as f64 + 1.) * painter.size[0] as f64 / 2.,
                (1. - v.position[1] as f64) * painter.size[1] as f64 / 2.,
                v.uv[0] as f64,
                v.uv[1] as f64,
            ]
        });
        let mut best = None;
        let mut cost = f64::INFINITY;
        for [a, b, c] in [[0, 2, 1], [2, 3, 1]] {
            let dx1 = v[b][0] - v[a][0];
            let dy1 = v[b][1] - v[a][1];
            let dx2 = v[c][0] - v[a][0];
            let dy2 = v[c][1] - v[a][1];
            let det = dx1 * dy2 - dx2 * dy1;
            if det == 0. {
                continue;
            }
            let t = ((point[0] - v[a][0]) * dy2 - (point[1] - v[a][1]) * dx2) / det;
            let u = (dx1 * (point[1] - v[a][1]) - dy1 * (point[0] - v[a][0])) / det;
            let score = (-t).max(0.) + (-u).max(0.) + (t + u - 1.).max(0.);
            if score < cost {
                cost = score;
                best = Some([
                    v[a][2] + t * (v[b][2] - v[a][2]) + u * (v[c][2] - v[a][2]),
                    v[a][3] + t * (v[b][3] - v[a][3]) + u * (v[c][3] - v[a][3]),
                ]);
            }
        }
        let Some(uv) = best else { continue };
        let near = [[0, 1], [1, 3], [3, 2], [2, 0]].iter().any(|&[a, b]| {
            let dx = v[b][0] - v[a][0];
            let dy = v[b][1] - v[a][1];
            let len = dx * dx + dy * dy;
            if len == 0. {
                return false;
            }
            let t = (((point[0] - v[a][0]) * dx + (point[1] - v[a][1]) * dy) / len).clamp(0., 1.);
            (point[0] - v[a][0] - t * dx).hypot(point[1] - v[a][1] - t * dy) <= 1. / 256.
        });
        if cost > 0. && !near {
            continue;
        }
        boundary |= near;
        let [w, h] = q.sprite.size;
        let tx = uv[0] * w as f64 - 0.5;
        let ty = uv[1] * h as f64 - 0.5;
        let ix = tx.floor() as i32;
        let iy = ty.floor() as i32;
        let fx = tx - tx.floor();
        let fy = ty - ty.floor();
        let mut sample = [0.; 4];
        for yy in 0..2 {
            for xx in 0..2 {
                let p = q.sprite.argb[(iy + yy).rem_euclid(h) as usize * w as usize
                    + (ix + xx).rem_euclid(w) as usize] as u32;
                let weight =
                    if xx == 0 { 1. - fx } else { fx } * if yy == 0 { 1. - fy } else { fy };
                for (k, shift) in [16, 8, 0, 24].into_iter().enumerate() {
                    sample[k] += ((p >> shift) & 255) as f64 * weight;
                }
            }
        }
        let tint = q.vertices[0].colour;
        for k in 0..4 {
            sample[k] *= tint[k] as f64 / 255.;
        }
        let alpha = sample[3] / 255.;
        let mut next = if near { states.clone() } else { vec![] };
        for old in &states {
            let mut colour = *old;
            if alpha > 0. {
                for k in 0..3 {
                    colour[k] = (sample[k] * alpha + old[k] as f64 * (1. - alpha)).round() as u8;
                }
                colour[3] = 0;
            }
            if !next.contains(&colour) {
                next.push(colour);
            }
        }
        states = next;
    }
    if boundary {
        states
    } else {
        vec![]
    }
}
