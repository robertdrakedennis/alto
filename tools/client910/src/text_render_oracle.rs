use crate::protocol910::pack_defaults as defaults;
use crate::{
    cache::Pack,
    font_atlas::Atlas,
    font_layout::{self, Draw, Images, Style},
    font_metrics::Metrics,
    text_render::{self, Vertex},
};
use std::io::Write;
struct Case {
    size: [u32; 2],
    clip: [i32; 4],
    atlas: Atlas,
    quads: Vec<([f32; 4], [f32; 4], u32)>,
    glyph_quads: Option<Vec<Option<[Vertex; 4]>>>,
}
fn cases() -> anyhow::Result<Vec<Case>> {
    let root = rs910_core::test_support::repo_root();
    let pack = Pack::open(root.join("server/data/pack"));
    let mut cases = Vec::new();
    for scale in [1, 2] {
        for alpha in [0, 1, 127, 255] {
            for clip in [[0, 0, 64, 64], [9, 7, 51, 49]] {
                let atlas = Atlas {
                    width: 8,
                    height: 8,
                    scale,
                    argb: (0..64u32)
                        .map(|i| ((i * 13 % 256) << 24) | ((i * 3) << 16) | ((i * 2) << 8) | i)
                        .collect(),
                };
                let colour = (alpha << 24) | 0xc48e59;
                let q = vec![
                    (
                        [-5.25, -2.75, 60.25, 55.75],
                        [-0.13, 0.07, 1.17, 1.05],
                        colour,
                    ),
                    ([21.125, 25.25, 75.875, 81.75], [0., 0., 1., 1.], 0x90bbdd44),
                    ([64., 0., 72., 10.], [0., 0., 1., 1.], 0xffffffff),
                    ([-20., 0., -1., 10.], [0., 0., 1., 1.], colour),
                ];
                cases.push(Case {
                    size: [64, 64],
                    clip,
                    atlas,
                    quads: q,
                    glyph_quads: None,
                });
            }
        }
    }
    let defaults = defaults::load(&pack)?.graphics.scalars;
    for id in [defaults.p11_full, defaults.p12_full, defaults.b12_full] {
        let m = Metrics::decode(
            &crate::js5_fetch::fetch_file(&pack, "fontmetrics", id as u32)?.unwrap(),
        )?;
        let sheet = crate::sprite_sheet::SpriteSheet::decode(
            &crate::js5_fetch::fetch_file(&pack, "sprites", id as u32)?.unwrap(),
        )?;
        for mono in [false, true] {
            for cropped in [false, true] {
                let mut draws = Vec::new();
                let mut style = Style::new(0xffeedd88u32 as i32, 0xff000000u32 as i32);
                font_layout::line(
                    &m,
                    font_layout::LineAt {
                        // The recorded sample text; the recording depends on its exact glyphs.
                        text: &"Java 910: Ag fi 012 € <col=4aaacc>text</col>" // provenance: recorded-data (text-render recording input)
                            .encode_utf16()
                            .collect::<Vec<_>>(),
                        x: 5,
                        baseline: 32,
                    },
                    &mut style,
                    &Images::default(),
                    false,
                    &mut draws,
                )?;
                let clip = if cropped {
                    [14, 24, 159, 31]
                } else {
                    [0, 0, 320, 64]
                };
                let glyph_quads = draws
                    .iter()
                    .map(|d| text_render::glyph(&m, d, clip, [320, 64]))
                    .collect();
                let quads = draws
                    .into_iter()
                    .filter_map(|d| {
                        if let Draw::Glyph {
                            code, x, y, colour, ..
                        } = d
                        {
                            let c = code as usize;
                            let yy = y + m.bearings[c] as i32;
                            let v = m.glyph_vertices(c);
                            Some((
                                [
                                    x as f32,
                                    yy as f32,
                                    (x + m.advances[c] as i32) as f32,
                                    (yy + m.widths[c] as i32) as f32,
                                ],
                                [v[0][3], v[0][4], v[2][3], v[2][4]],
                                colour as u32,
                            ))
                        } else {
                            None
                        }
                    })
                    .collect();
                cases.push(Case {
                    size: [320, 64],
                    clip: if cropped {
                        [14, 24, 159, 31]
                    } else {
                        [0, 0, 320, 64]
                    },
                    atlas: Atlas::new(&m, &sheet, mono)?,
                    quads,
                    glyph_quads: Some(glyph_quads),
                });
            }
        }
    }
    Ok(cases)
}
fn int(w: &mut impl Write, v: u32) -> std::io::Result<()> {
    w.write_all(&v.to_be_bytes())
}
/// The directory of the GPU pixel comparison (`tools/oracle/run-gpu-pixels.sh`
/// sets `CLIENT910_TEXT_RENDER_REPLAY` and prepares the reference in it).
fn gpu_dir() -> std::path::PathBuf {
    std::env::var_os("CLIENT910_TEXT_RENDER_REPLAY").map_or_else(
        || panic!("CLIENT910_TEXT_RENDER_REPLAY is not set: run this test through tools/oracle/run-gpu-pixels.sh"),
        std::path::PathBuf::from,
    )
}
/// Quads of every text draw case against the frozen recording of the original
/// client's text renderer. With `CLIENT910_TEXT_RENDER_REPLAY` set the case
/// inputs are also written there for the GPU pixel comparison.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn export() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("text-render");
    let out = std::env::var_os("CLIENT910_TEXT_RENDER_REPLAY")
        .map_or_else(|| scratch.dir().to_path_buf(), std::path::PathBuf::from);
    let cases = cases()?;
    let mut input = std::io::BufWriter::new(std::fs::File::create(out.join("input.bin"))?);
    let mut result = std::io::BufWriter::new(std::fs::File::create(out.join("rust-quads.bin"))?);
    int(&mut input, cases.len() as u32)?;
    let mut quads = 0;
    for c in cases {
        for v in c.size {
            int(&mut input, v)?;
        }
        for v in c.clip {
            int(&mut input, v as u32)?;
        }
        for v in [
            c.atlas.width as u32,
            c.atlas.height as u32,
            c.atlas.scale as u32,
        ] {
            int(&mut input, v)?;
        }
        for p in c.atlas.argb {
            int(&mut input, p)?;
        }
        int(&mut input, c.quads.len() as u32)?;
        for (i, (rect, uv, colour)) in c.quads.into_iter().enumerate() {
            for v in rect.into_iter().chain(uv) {
                int(&mut input, v.to_bits())?;
            }
            int(&mut input, colour)?;
            match c.glyph_quads.as_ref().map_or_else(
                || text_render::quad(rect, uv, colour, c.clip, c.size),
                |q| q[i],
            ) {
                None => int(&mut result, 0)?,
                Some(v) => {
                    int(&mut result, 1)?;
                    for p in v {
                        for v in p.position.into_iter().chain(p.uv) {
                            int(&mut result, v.to_bits())?;
                        }
                    }
                    int(&mut result, colour)?;
                }
            }
            quads += 1;
        }
    }
    input.flush()?;
    result.flush()?;
    rs910_core::test_support::frozen::assert_stream(
        "text-render/quads",
        &std::fs::read(out.join("rust-quads.bin"))?,
    );
    println!("Text render: {quads} quad inputs");
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
    let root = rs910_core::test_support::pack_root();
    let pack = Pack::open(root);
    let id = defaults::load(&pack)?.graphics.scalars.p11_full;
    let metrics = std::rc::Rc::new(Metrics::decode(
        &crate::js5_fetch::fetch_file(&pack, "fontmetrics", id as u32)?.unwrap(),
    )?);
    let mut renderer = crate::ui_paint_gpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    let mut actual = Vec::new();
    for (frame, c) in cases()?.into_iter().enumerate() {
        let font = std::rc::Rc::new(crate::ui_fonts::Font {
            metrics: metrics.clone(),
            atlas: c.atlas.clone(),
            monochrome: false,
            paletted: true,
            translucent: false,
        });
        let quads: Vec<[Vertex; 4]> = match c.glyph_quads {
            Some(q) => q.into_iter().flatten().collect(),
            None => c
                .quads
                .into_iter()
                .filter_map(|(r, u, col)| text_render::quad(r, u, col, c.clip, c.size))
                .collect(),
        };
        renderer.prepare(
            &device,
            &queue,
            crate::ui_paint::Plan {
                size: c.size,
                quads: quads
                    .into_iter()
                    .map(|vertices| crate::ui_paint::Quad {
                        image: crate::ui_paint::Image::Font(font.clone()),
                        vertices,
                        clip: c.clip,
                        mask: None,
                    })
                    .collect(),
            },
        )?;
        let size = wgpu::Extent3d {
            width: c.size[0],
            height: c.size[1],
            depth_or_array_layers: 1,
        };
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("text oracle"),
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
                label: Some("text proof"),
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
        if [18, 22, 26].contains(&frame) {
            let start = actual.len() - c.size[0] as usize * c.size[1] as usize * 4;
            let rgb: Vec<u8> = actual[start..]
                .chunks_exact(4)
                .flat_map(|p| p[..3].iter().copied())
                .collect();
            crate::png_out::write_rgb_png(
                &out.join(format!("font-{frame}.png")),
                c.size[0],
                c.size[1],
                &rgb,
            )?;
        }
    }
    std::fs::write(out.join("rust.rgba"), &actual)?;
    anyhow::ensure!(actual.len() == reference.len(), "pixel lengths");
    let mut max = 0;
    let mut differing = 0;
    for (a, b) in actual.iter().zip(&reference) {
        let d = a.abs_diff(*b);
        max = max.max(d);
        differing += (d != 0) as usize;
    }
    // Exact nearest-texel ties stay in the corpus. Clipped f32 varyings can
    // interpolate to either side of an integer edge on GL and Metal. Validate
    // both results from the adjacent texels and ordered alpha composition.
    let mut at = 0;
    let mut boundary_pixels = 0;
    let mut ordinary_max = 0;
    for c in cases()? {
        for y in 0..c.size[1] {
            for x in 0..c.size[0] {
                let a = &actual[at..at + 4];
                let b = &reference[at..at + 4];
                at += 4;
                let difference = a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
                if difference <= 2 {
                    ordinary_max = ordinary_max.max(difference);
                    continue;
                }
                let candidates = nearest_boundary_colours(&c, x, y);
                let possible = |p: &[u8]| {
                    candidates
                        .iter()
                        .any(|c| c.iter().zip(p).all(|(a, b)| a.abs_diff(*b) <= 2))
                };
                anyhow::ensure!(
                    possible(a) && possible(b),
                    "text GPU mismatch at {x},{y}: actual {a:?}, GL {b:?}, allowed {candidates:?}"
                );
                boundary_pixels += 1;
            }
        }
    }
    std::fs::write(
        out.join("pixels.json"),
        format!(
            "{{\"bytes\":{},\"max_channel_error\":{max},\"max_nonboundary_error\":{ordinary_max},\"texel_boundary_pixels\":{boundary_pixels},\"differing_channels\":{differing}}}\n",
            actual.len()
        ),
    )?;
    println!("PASS text GPU {} bytes ordinary max {ordinary_max}/255, {boundary_pixels} verified nearest-texel ties", actual.len());
    Ok(())
}

/// Independent f64 texture lookup/composition for the exact nearest-filter
/// ambiguity. Candidate indices may differ only within 0.00002 texels of an
/// integer edge. Every other sample has one index, and both driver pixels must
/// agree with a complete candidate composition to within two channel units.
fn nearest_boundary_colours(c: &Case, x: u32, y: u32) -> Vec<[u8; 4]> {
    if c.atlas.scale != 1 {
        return vec![];
    }
    let mut states = vec![[26u8, 51, 77, 102]];
    let mut boundary = false;
    for &(rect, uv, colour) in &c.quads {
        let Some(v) = text_render::quad(rect, uv, colour, c.clip, c.size) else {
            continue;
        };
        let px = |a: f32| (a as f64 + 1.) * 0.5 * c.size[0] as f64;
        let py = |a: f32| (1. - a as f64) * 0.5 * c.size[1] as f64;
        let r = [
            px(v[0].position[0]),
            py(v[0].position[1]),
            px(v[3].position[0]),
            py(v[3].position[1]),
        ];
        let point = [x as f64 + 0.5, y as f64 + 0.5];
        if point[0] < r[0] || point[0] >= r[2] || point[1] < r[1] || point[1] >= r[3] {
            continue;
        }
        let sample = |axis: usize, size: u16| {
            let t = (v[0].uv[axis] as f64
                + (point[axis] - r[axis]) / (r[axis + 2] - r[axis])
                    * (v[3].uv[axis] as f64 - v[0].uv[axis] as f64))
                * size as f64;
            if (t - t.round()).abs() < 0.00002 {
                vec![
                    (t.round() as i32 - 1).rem_euclid(size as i32),
                    (t.round() as i32).rem_euclid(size as i32),
                ]
            } else {
                vec![(t.floor() as i32).rem_euclid(size as i32)]
            }
        };
        let xs = sample(0, c.atlas.width);
        let ys = sample(1, c.atlas.height);
        boundary |= xs.len() > 1 || ys.len() > 1;
        let tint = [
            (colour >> 16) as u8,
            (colour >> 8) as u8,
            colour as u8,
            (colour >> 24) as u8,
        ];
        let mut next = Vec::new();
        for &yy in &ys {
            for &xx in &xs {
                let p = c.atlas.argb[yy as usize * c.atlas.width as usize + xx as usize];
                let src = [(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8];
                let a = src[3] as f64 / 255. * tint[3] as f64 / 255.;
                for old in &states {
                    let mut pixel = *old;
                    if a > 0. {
                        for k in 0..3 {
                            pixel[k] = (src[k] as f64 * tint[k] as f64 / 255. * a
                                + old[k] as f64 * (1. - a))
                                .round() as u8;
                        }
                        pixel[3] = 0;
                    }
                    if !next.contains(&pixel) {
                        next.push(pixel);
                    }
                }
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
