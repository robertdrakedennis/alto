use super::*;

fn remapper(id: i32) -> Option<Remapper> {
    Some(Remapper {
        id,
        argb: vec![0; 256 * 16].into(),
    })
}

#[test]
fn chain_orders_effects_by_position_key() {
    // Levels and colour remapping are installed up front; bloom is added
    // later and still sorts first (position key 0).
    let mut chain = Chain::with_filters();
    assert_eq!(chain.effects(), [Effect::Levels, Effect::ColourRemapping]);
    assert_eq!(chain.data_type, 0);
    assert!(chain.add(Effect::Bloom));
    assert_eq!(
        chain.effects(),
        [Effect::Bloom, Effect::Levels, Effect::ColourRemapping]
    );
    assert_eq!(chain.data_type, 1);
    // Adding an effect that is already enabled is refused.
    assert!(!chain.add(Effect::Bloom));
    chain.remove(Effect::Bloom);
    assert_eq!(chain.effects(), [Effect::Levels, Effect::ColourRemapping]);
    // Removing an effect never lowers the chain's data type.
    assert_eq!(chain.data_type, 1);
    let mut reverse = Chain::default();
    for e in [Effect::ColourRemapping, Effect::Bloom, Effect::Levels] {
        reverse.add(e);
    }
    assert_eq!(
        reverse.effects(),
        [Effect::Bloom, Effect::Levels, Effect::ColourRemapping]
    );
}

#[test]
fn capture_gate_follows_live_effects() {
    let chain = Chain::with_filters();
    let mut params = Params::default();
    // Defaults: levels identity, remapping count 1 with no remapper.
    assert!(!chain.capture(&params));
    params.levels.gamma = 1.2;
    assert!(chain.capture(&params));
    assert_eq!(chain.live(&params), [Effect::Levels]);
    params.levels = LevelsParams::default();
    params.remap = ColourRemapParams::set(remapper(7), 0.5, None, 0.0, None, 0.0);
    assert_eq!(chain.live(&params), [Effect::ColourRemapping]);
    let mut bloom = chain.clone();
    bloom.add(Effect::Bloom);
    params.remap = ColourRemapParams::default();
    // Bloom is never a no-op.
    assert_eq!(bloom.live(&params), [Effect::Bloom]);
}

#[test]
fn set_colour_remapping_shifts_and_weights() {
    // Plain three-slot set: count positive weights, base = 1 - sum.
    let p = ColourRemapParams::set(remapper(1), 0.25, remapper(2), 0.5, remapper(3), 0.0);
    assert_eq!(p.count, 2);
    assert_eq!(p.base, 0.25);
    assert_eq!(
        p.remappers.map(|r| r.map(|r| r.id)),
        [Some(1), Some(2), Some(3)]
    );
    // A weighted empty slot 2 loses its weight.
    let p = ColourRemapParams::set(remapper(1), 0.25, remapper(2), 0.25, None, 0.5);
    assert_eq!(p.weights, [0.25, 0.25, 0.0]);
    assert_eq!(p.base, 0.5);
    // Empty weighted slots shift down.
    let p = ColourRemapParams::set(None, 0.5, None, 0.25, remapper(3), 0.125);
    assert_eq!(p.remappers.map(|r| r.map(|r| r.id)), [Some(3), None, None]);
    assert_eq!(p.weights, [0.125, 0.0, 0.0]);
    assert_eq!(p.count, 1);
    assert_eq!(p.base, 0.875);
    // Non-contiguous weights keep their slots: sampleCount 1 reads slot 0
    // only (a quirk of the original client), so slot 1's remapper is unused.
    let p = ColourRemapParams::set(remapper(1), 0.0, remapper(2), 0.5, None, 0.0);
    assert_eq!(p.count, 1);
    assert_eq!(p.weights, [0.0, 0.5, 0.0]);
    // No-op detection.
    assert!(ColourRemapParams::default().is_noop());
    assert!(ColourRemapParams::set(None, 0.0, None, 0.0, None, 0.0).is_noop());
    assert!(ColourRemapParams::set(None, 0.5, None, 0.0, None, 0.0).is_noop());
    assert!(!ColourRemapParams::set(remapper(1), 0.5, None, 0.0, None, 0.0).is_noop());
}

/// A reference model of the schedule with texture objects and the
/// write-back going into the capture texture itself; returns, per pass, the
/// contents it reads as its input and its scene copy and whether it writes
/// the screen.
fn reference_schedule(live: &[Effect]) -> Vec<(u32, u32, bool)> {
    // Texture objects 0 (capture), 1 and 2 (ping-pong); content labels:
    // 0 = captured scene, n = output of pass n.
    let mut contents = [0u32, u32::MAX, u32::MAX];
    let (capture, mut ping, mut pong) = (0usize, 1usize, 2usize);
    let mut label = 0;
    let mut reads = Vec::new();
    for (i, &e) in live.iter().enumerate() {
        let passes = e.passes();
        let final_effect = live.len() - 1 == i;
        for pass in 0..passes {
            let target = if passes - 1 != pass {
                Some(pong)
            } else if final_effect {
                None
            } else {
                Some(capture)
            };
            let input = if pass == 0 { capture } else { ping };
            reads.push((contents[input], contents[capture], target.is_none()));
            label += 1;
            if let Some(t) = target {
                contents[t] = label;
            }
            std::mem::swap(&mut ping, &mut pong);
        }
    }
    reads
}

#[test]
fn schedule_matches_the_reference_ping_pong() {
    use Effect::*;
    let chains: [&[Effect]; 7] = [
        &[Bloom],
        &[Levels],
        &[ColourRemapping],
        &[Bloom, Levels],
        &[Bloom, ColourRemapping],
        &[Levels, ColourRemapping],
        &[Bloom, Levels, ColourRemapping],
    ];
    for live in chains {
        let reference = reference_schedule(live);
        let mut contents = [0u32, u32::MAX, u32::MAX];
        let mut ours = Vec::new();
        for (n, step) in schedule(live).iter().enumerate() {
            ours.push((
                contents[step.input],
                contents[step.scene],
                step.output == PassOutput::Screen,
            ));
            assert_ne!(
                PassOutput::Texture(step.input),
                step.output,
                "{live:?} feedback"
            );
            assert_ne!(
                PassOutput::Texture(step.scene),
                step.output,
                "{live:?} feedback"
            );
            if let PassOutput::Texture(t) = step.output {
                contents[t] = n as u32 + 1;
            }
        }
        assert_eq!(ours, reference, "{live:?}");
        let steps = schedule(live);
        assert_eq!(steps.iter().filter(|s| s.last).count(), 1);
        assert!(steps.last().unwrap().last);
    }
}

/// A reference model of the three-vertex position and texture-coordinate
/// array for a `fbo`/`surface` pair, before it is uploaded.
fn reference_pos_and_tex_coords(
    effect: Effect,
    pass: usize,
    fbo: [u32; 2],
    surface: [u32; 2],
    last: bool,
) -> [f32; 12] {
    let offset = 0.0f32;
    let width = fbo[0] as f32;
    let height = fbo[1] as f32;
    let offset_x = offset * 2.0 / width;
    let offset_y = -offset * 2.0 / height;
    let mut vertices = [
        offset_x + -1.0,
        offset_y + 1.0,
        0.0,
        0.0,
        offset_x + -1.0,
        offset_y + -3.0,
        0.0,
        2.0,
        offset_x + 3.0,
        offset_y + 1.0,
        2.0,
        0.0,
    ];
    let mut region_w = width as i32;
    let mut region_h = height as i32;
    let mut texture_w = if last { surface[0] as i32 } else { region_w };
    let mut texture_h = if last { surface[1] as i32 } else { region_h };
    if effect == Effect::Bloom {
        if pass == 0 {
            region_w = 256;
            region_h = 256;
        } else if pass == 1 || pass == 2 {
            region_w = 256;
            region_h = 256;
            texture_w = region_w;
            texture_h = region_h;
        }
    }
    let region_x = region_w as f32 / width;
    let region_y = region_h as f32 / height;
    let texture_x = texture_w as f32 / width;
    let texture_y = texture_h as f32 / height;
    vertices[8] = (vertices[8] + 1.0) * region_x - 1.0;
    vertices[5] = (vertices[5] - 1.0) * region_y + 1.0;
    vertices[10] *= texture_x;
    vertices[7] *= texture_y;
    vertices
}

/// The WGSL `vertex` stage's three `(position, uv)` pairs for `geom`, with
/// GL's negated `v` folded back.
fn wgsl_vertices(geom: [f32; 4]) -> [f32; 12] {
    let mut out = [0.0; 12];
    for (i, (x, y)) in [(0.0f32, 0.0f32), (0.0, 2.0), (2.0, 0.0)]
        .iter()
        .enumerate()
    {
        out[i * 4] = x * 2.0 * geom[0] - 1.0;
        out[i * 4 + 1] = 1.0 - y * 2.0 * geom[1];
        out[i * 4 + 2] = x * geom[2];
        out[i * 4 + 3] = y * geom[3];
    }
    out
}

#[test]
fn pass_geometry_matches_reference_pos_and_tex_coords() {
    for fbo in [
        [128, 96],
        [256, 256],
        [765, 553],
        [1600, 1200],
        [1920, 1080],
    ] {
        for surface in [fbo, [fbo[0] + 17, fbo[1] + 3]] {
            for effect in [Effect::Bloom, Effect::Levels, Effect::ColourRemapping] {
                for pass in 0..effect.passes() {
                    for last in [false, true] {
                        let reference =
                            reference_pos_and_tex_coords(effect, pass, fbo, surface, last);
                        let ours = wgsl_vertices(pass_geometry(effect, pass, fbo, surface, last));
                        for (a, b) in reference.iter().zip(ours) {
                            assert!(
                                (a - b).abs() <= 1e-6,
                                "{effect:?} {pass} {fbo:?} {surface:?} {last}: {reference:?} vs {ours:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn step_uniforms_match_effect_parameters() {
    let params = Params {
        bloom: BloomParams {
            threshold: 0.75,
            white_point_sq: 2.0,
            intensity: 0.5,
            sample_scale: 1.0,
        },
        levels: LevelsParams::from_array([0.8, 0.1, 0.9, 0.05, 0.95]),
        remap: ColourRemapParams::set(remapper(4), 0.25, remapper(5), 0.5, None, 0.0),
    };
    let fbo = [800, 600];
    let step = |effect, pass| Step {
        effect,
        pass,
        input: 0,
        scene: 0,
        output: PassOutput::Texture(1),
        last: false,
    };
    // Bloom: params = (threshold, intensity, white point squared, 0);
    // sample size = sample scale / width or height.
    let b = step_uniforms(&step(Effect::Bloom, 1), &params, fbo, fbo);
    assert_eq!(b.params, [0.75, 0.5, 2.0, 0.0]);
    assert_eq!(b.extra, [0.0, 0.0, 256.0 / 800.0, 256.0 / 600.0]);
    assert_eq!(b.misc[2..], [1.0 / 800.0, 0.0]);
    let b = step_uniforms(&step(Effect::Bloom, 2), &params, fbo, fbo);
    assert_eq!(b.misc[2..], [0.0, 1.0 / 600.0]);
    let l = step_uniforms(&step(Effect::Levels, 0), &params, fbo, fbo);
    assert_eq!(l.ranges, [0.1, 0.9, 0.05, 0.95]);
    assert_eq!(l.misc[0], 0.8);
    let r = step_uniforms(&step(Effect::ColourRemapping, 0), &params, fbo, fbo);
    assert_eq!(r.params, [0.25, 0.25, 0.5, 0.0]);
    assert_eq!(r.misc[1], 2.0);
}

/// Sprite pixel `(16z + x, y)` of an identity remapping sprite.
fn identity_sprite() -> Vec<i32> {
    let mut argb = vec![0; 256 * 16];
    for y in 0..16 {
        for col in 0..256 {
            let (x, z) = (col % 16, col / 16);
            argb[y * 256 + col] = 0xff00_0000u32 as i32
                | ((x as i32 * 17) << 16)
                | ((y as i32 * 17) << 8)
                | (z as i32 * 17);
        }
    }
    argb
}

#[test]
fn lut_texels_follow_the_slice_layout() {
    let mut argb = vec![0; 256 * 16];
    for (i, p) in argb.iter_mut().enumerate() {
        *p = (i as i32).wrapping_mul(0x010203) ^ 0x5a5a5a;
    }
    let lut = lut_texels(&argb);
    // The layout written out literally, texel by texel.
    let mut expected = vec![0u8; 16384];
    for x in 0..16 {
        for y in 0..16 {
            for z in 0..16 {
                let pixel = argb[y * 256 + z * 16 + x];
                let at = (y * 16 + z * 256 + x) * 4;
                expected[at] = (pixel >> 16 & 0xff) as u8;
                expected[at + 1] = (pixel >> 8 & 0xff) as u8;
                expected[at + 2] = (pixel & 0xff) as u8;
                expected[at + 3] = 0xff;
            }
        }
    }
    assert_eq!(lut, expected);
    // glTexImage3D(16, 16, 16): texel (x, y, z) at ((z * 16 + y) * 16 + x).
    let id = lut_texels(&identity_sprite());
    let texel = |x: usize, y: usize, z: usize| &id[((z * 16 + y) * 16 + x) * 4..][..3];
    assert_eq!(texel(3, 7, 11), [3 * 17, 7 * 17, 11 * 17]);
}

#[test]
fn identity_remap_preserves_colour() {
    // The texel-centre mapping `0.03125 + c * 0.9375` makes an identity 16^3
    // volume exact under trilinear filtering.
    let lut = lut_texels(&identity_sprite());
    let p = ColourRemapParams::set(remapper(1), 1.0, None, 0.0, None, 0.0);
    for rgb in [
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        [0.2, 0.55, 0.9],
        [1.4, -0.2, 0.5],
    ] {
        let out = remap_reference(&p, std::slice::from_ref(&lut), rgb);
        for i in 0..3 {
            assert!(
                (out[i] - rgb[i].clamp(0.0, 1.0)).abs() < 1e-5,
                "{rgb:?} -> {out:?}"
            );
        }
    }
    // A half-weighted inverting LUT blends with the source.
    let inverted: Vec<i32> = identity_sprite()
        .iter()
        .map(|p| (p & 0xff00_0000u32 as i32) | (!p & 0xffffff))
        .collect();
    let inv = lut_texels(&inverted);
    let p = ColourRemapParams::set(remapper(2), 0.5, None, 0.0, None, 0.0);
    let out = remap_reference(&p, std::slice::from_ref(&inv), [0.2, 0.4, 0.6]);
    for (i, v) in [0.2f32, 0.4, 0.6].iter().enumerate() {
        assert!((out[i] - (v * 0.5 + (1.0 - v) * 0.5)).abs() < 1e-5);
    }
}

#[test]
fn levels_map_through_input_range_gamma_and_output_range() {
    let p = LevelsParams::from_array([2.0, 0.2, 0.8, 0.1, 0.9]);
    // x = clamp((c - 0.2) / 0.6), out = 0.1 + x^2 * 0.8.
    let out = levels_reference(&p, [0.5, 0.1, 1.2]);
    assert!((out[0] - (0.1 + 0.25 * 0.8)).abs() < 1e-6);
    assert!((out[1] - 0.1).abs() < 1e-6);
    assert!((out[2] - 0.9).abs() < 1e-6);
    assert!(LevelsParams::default().is_noop());
    assert_eq!(
        levels_reference(&LevelsParams::default(), [0.3, 0.6, 0.9]),
        [0.3, 0.6, 0.9]
    );
}

fn env_with(remap: [(i32, f32); 3]) -> crate::env::Environment {
    crate::env::Environment {
        colour_remap: remap,
        ..Default::default()
    }
}

#[test]
fn interpolation_fades_and_merges_remaps() {
    let mut a = env_with([(10, 0.5), (-1, 0.0), (-1, 0.0)]);
    a.bloom = [1.0, 0.25, 1.0];
    a.levels = [1.0, 0.0, 1.0, 0.0, 1.0];
    let mut b = env_with([(-1, 0.0); 3]);
    b.bloom = [3.0, 0.75, 0.5];
    b.levels = [2.0, 0.2, 0.8, 0.1, 0.9];
    let mut out = crate::env::Environment::default();
    interpolate(&mut out, &a, &b, 0.25);
    assert_eq!(out.bloom, [1.5, 0.375, 0.875]);
    assert_eq!(out.levels, [1.25, 0.05, 0.95, 0.025, 0.975]);
    // Fading out: `(1 - t) * weight` of the old maps.
    assert_eq!(out.colour_remap, [(10, 0.375), (-1, 0.0), (-1, 0.0)]);
    // Fading in: `weight * t` of the new maps.
    interpolate(&mut out, &b, &a, 0.25);
    assert_eq!(out.colour_remap, [(10, 0.125), (-1, 0.0), (-1, 0.0)]);
    // Nothing either side.
    interpolate(&mut out, &b, &b, 0.5);
    assert_eq!(out.colour_remap, [(-1, 0.0); 3]);
    // Merge: shared maps sum, new ones append.
    let c = env_with([(10, 0.5), (20, 0.25), (-1, 0.0)]);
    let d = env_with([(20, 0.5), (30, 0.5), (-1, 0.0)]);
    interpolate(&mut out, &c, &d, 0.5);
    assert_eq!(out.colour_remap, [(10, 0.25), (20, 0.375), (30, 0.25)]);
    // Four maps: sort by weight descending, keep three, rescale to the
    // total.
    let e = env_with([(1, 0.4), (2, 0.2), (-1, 0.0)]);
    let f = env_with([(3, 0.4), (4, 0.2), (-1, 0.0)]);
    interpolate(&mut out, &e, &f, 0.5);
    let total = 0.2 + 0.1 + 0.2 + 0.1f32;
    let kept = 0.2 + 0.2 + 0.1f32;
    let maps: Vec<i32> = out.colour_remap.iter().map(|(m, _)| *m).collect();
    assert_eq!(
        maps[..2]
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        [1, 3].into()
    );
    assert!(maps[2] == 2 || maps[2] == 4);
    for (i, w) in [0.2, 0.2, 0.1f32].iter().enumerate() {
        assert!((out.colour_remap[i].1 - w * total / kept).abs() < 1e-6);
    }
}

#[test]
fn sort_descending_orders_weights_and_ids_together() {
    let mut w = [0.1f32, 0.5, 0.3, 0.5, 0.2];
    let mut ids = [1, 2, 3, 4, 5];
    sort_descending(&mut w, &mut ids, 0, 4);
    assert_eq!(w, [0.5, 0.5, 0.3, 0.2, 0.1]);
    assert_eq!(ids[2..], [3, 5, 1]);
    assert!(ids[..2] == [2, 4] || ids[..2] == [4, 2]);
}

#[test]
fn update_full_feeds_current_environment() {
    let chain = Chain::with_filters();
    let mut params = Params::default();
    let mut cache = RemapperCache::new(None);
    let env = crate::env::Environment {
        bloom: [2.0, 0.5, 0.75],
        levels: [1.1, 0.0, 1.0, 0.0, 1.0],
        colour_remap: [(123, 0.5), (-1, 0.0), (-1, 0.0)],
        ..Default::default()
    };
    let frame = crate::env::EnvFrame::build(
        &env,
        crate::env::SunSettings {
            direction: env.sun_dir,
            brightness_pref: 3,
            anti_macro: 0.0,
        },
        true,
        crate::env::FogReference {
            far: 1.0,
            near_min: 1.0,
            view: &[0.0; 16],
        },
    );
    update_full(&chain, &mut params, &frame, &mut cache);
    // Bloom threshold, white point squared and intensity.
    assert_eq!(params.bloom.threshold, 0.75);
    assert_eq!(params.bloom.white_point_sq, 2.0);
    assert_eq!(params.bloom.intensity, 0.5);
    assert_eq!(params.levels.gamma, 1.1);
    // No pack: no remapper is created and the weight is dropped by the
    // remapping setter's shifting.
    assert_eq!(params.remap.count, 0);
    assert!(params.remap.is_noop());
}

#[test]
fn environment_override_decodes_post_process_bits() {
    let mut p = Vec::new();
    let mask: u64 = (1 << 12) | (1 << 14) | (1 << 15) | (1 << 18);
    p.extend_from_slice(&mask.to_be_bytes());
    p.extend_from_slice(&0.5f32.to_bits().to_be_bytes());
    p.extend_from_slice(&3.0f32.to_bits().to_be_bytes());
    p.extend_from_slice(&77u16.to_be_bytes());
    p.extend_from_slice(&4321u16.to_be_bytes());
    p.extend_from_slice(&0.25f32.to_bits().to_be_bytes());
    p.extend_from_slice(&1500u16.to_be_bytes());
    let e = crate::server_prot::parse_environment_override(&p).unwrap();
    assert_eq!(e.bloom_intensity, Some(0.5));
    assert_eq!(e.bloom_threshold, None);
    assert_eq!(e.bloom_white_point_sq, Some(3.0));
    assert_eq!(e.sampler, Some(77));
    assert_eq!(e.colour_remap, [None, Some((4321, 0.25)), None]);
    assert_eq!(e.duration_ms, 1500);
}

#[test]
fn bloom_passes_match_reference_observations() -> anyhow::Result<()> {
    let text = rs910_core::test_support::frozen::text("bloom-passes/passes.txt");
    let params = Params::default();
    for line in text.lines() {
        let parts: Vec<_> = line.split('|').collect();
        let key: Vec<usize> = parts[0].split(',').map(|s| s.parse().unwrap()).collect();
        let (w, h, pass) = (key[0] as u32, key[1] as u32, key[2]);
        let step = Step {
            effect: Effect::Bloom,
            pass,
            input: 0,
            scene: 0,
            output: PassOutput::Screen,
            last: pass == 3,
        };
        let u = step_uniforms(&step, &params, [w, h], [w, h]);
        for field in &parts[2..] {
            let (name, data) = field.split_once('=').unwrap();
            let expected: Vec<u32> = data.split(',').map(|s| s.parse().unwrap()).collect();
            let actual: Vec<f32> = match name {
                "params" => u.params.to_vec(),
                "sample" => u.misc[2..].to_vec(),
                "scale" => u.extra.to_vec(),
                "vertices" => wgsl_vertices(u.geom)
                    .chunks(4)
                    .flat_map(|v| [v[0], v[1], v[2], v[3]])
                    .collect(),
                _ => anyhow::bail!("unknown bloom observation {name}"),
            };
            assert_eq!(
                actual.iter().map(|f| f.to_bits()).collect::<Vec<_>>(),
                expected,
                "{} {name}",
                parts[0]
            );
        }
    }
    eprintln!("{} bloom pass observations matched", text.lines().count());
    Ok(())
}

fn readback(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let [w, h] = [texture.width() as usize, texture.height() as usize];
    let pitch = (w * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (pitch * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(pitch as u32),
                rows_per_image: Some(h as u32),
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv().unwrap().unwrap();
    let mapped = buffer.slice(..).get_mapped_range().expect("mapped range");
    (0..h)
        .flat_map(|y| mapped[y * pitch..y * pitch + w * 4].to_vec())
        .collect()
}

fn texture(device: &wgpu::Device, size: [u32; 2], format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

#[test]
#[ignore = "requires a desktop GPU"]
fn gpu_levels_and_remap_match_the_reference_equations() {
    let (device, queue) = crate::test_support::require_gpu();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let size = [96u32, 64];
    let mut scene_px = vec![0u8; (size[0] * size[1] * 4) as usize];
    for y in 0..size[1] {
        for x in 0..size[0] {
            let at = ((y * size[0] + x) * 4) as usize;
            scene_px[at..at + 4].copy_from_slice(&[
                (x * 255 / 95) as u8,
                (y * 4) as u8,
                ((x ^ y) * 2) as u8,
                255,
            ]);
        }
    }
    let scene = texture(&device, size, format);
    queue.write_texture(
        scene.as_image_copy(),
        &scene_px,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size[0] * 4),
            rows_per_image: Some(size[1]),
        },
        scene.size(),
    );
    let screen = texture(&device, size, format);
    let inverted: Vec<i32> = identity_sprite()
        .iter()
        .map(|p| (p & 0xff00_0000u32 as i32) | (!p & 0xffffff))
        .collect();
    let mut post = PostProcessor::new(&device);
    post.params.levels = LevelsParams::from_array([1.5, 0.1, 0.9, 0.05, 1.0]);
    post.params.remap = ColourRemapParams::set(
        Some(Remapper {
            id: 1,
            argb: inverted.clone().into(),
        }),
        0.4,
        None,
        0.0,
        None,
        0.0,
    );
    let scene_clip = [8, 4, 80, 56];
    let bounds = [4, 2, 88, 60];
    let mut encoder = device.create_command_encoder(&Default::default());
    // Pre-fill the screen so untouched pixels are visible.
    {
        let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &screen.create_view(&Default::default()),
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.2,
                        g: 0.4,
                        b: 0.6,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    assert!(post.encode(
        &device,
        &queue,
        &mut encoder,
        &Capture {
            scene: &scene.create_view(&Default::default()),
            format,
            size,
            scene_clip,
        },
        &Screen {
            view: &screen.create_view(&Default::default()),
            format,
            bounds,
        },
    ));
    queue.submit([encoder.finish()]);
    let out = readback(&device, &queue, &screen);
    let lut = lut_texels(&inverted);
    let inside =
        |r: [u32; 4], x: u32, y: u32| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3];
    let mut max_error = 0u8;
    for y in 0..size[1] {
        for x in 0..size[0] {
            let at = ((y * size[0] + x) * 4) as usize;
            let expected: [u8; 3] = if !inside(bounds, x, y) {
                [51, 102, 153]
            } else {
                // Outside the scene clip the capture is black.
                let src = if inside(scene_clip, x, y) {
                    [0, 1, 2].map(|i| scene_px[at + i] as f32 / 255.0)
                } else {
                    [0.0; 3]
                };
                // Levels (order 1), stored as 8-bit, then remapping (order 2).
                let l =
                    levels_reference(&post.params.levels, src).map(|v| (v * 255.0).round() / 255.0);
                let r = remap_reference(&post.params.remap, std::slice::from_ref(&lut), l);
                r.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
            };
            for i in 0..3 {
                let e = out[at + i].abs_diff(expected[i]);
                max_error = max_error.max(e);
                assert!(
                    e <= 2,
                    "pixel {x},{y}: {:?} vs {expected:?}",
                    &out[at..at + 3]
                );
            }
        }
    }
    eprintln!("levels+remap: max error {max_error}/255");
}

#[test]
#[ignore = "requires a desktop GPU"]
fn gpu_bloom_blooms_hdr_highlights_inside_bounds() {
    let (device, queue) = crate::test_support::require_gpu();
    let format = wgpu::TextureFormat::Rgba16Float;
    let size = [320u32, 240];
    // IEEE binary16 bit patterns of the few values used.
    let half = |v: f32| match v {
        0.125 => 0x3000u16,
        1.0 => 0x3c00,
        4.0 => 0x4400,
        _ => panic!("test half value"),
    };
    let mut px = vec![half(0.125); (size[0] * size[1] * 4) as usize];
    for y in 100..110 {
        for x in 150..160 {
            let at = ((y * size[0] + x) * 4) as usize;
            px[at..at + 3].copy_from_slice(&[half(4.0), half(4.0), half(4.0)]);
        }
    }
    for p in px.chunks_mut(4) {
        p[3] = half(1.0);
    }
    let scene = texture(&device, size, format);
    queue.write_texture(
        scene.as_image_copy(),
        bytemuck::cast_slice(&px),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size[0] * 8),
            rows_per_image: Some(size[1]),
        },
        scene.size(),
    );
    let screen = texture(&device, size, wgpu::TextureFormat::Rgba8Unorm);
    let mut post = PostProcessor::new(&device);
    post.chain.add(Effect::Bloom);
    let mut encoder = device.create_command_encoder(&Default::default());
    assert!(post.encode(
        &device,
        &queue,
        &mut encoder,
        &Capture {
            scene: &scene.create_view(&Default::default()),
            format,
            size,
            scene_clip: [0, 0, size[0], size[1]],
        },
        &Screen {
            view: &screen.create_view(&Default::default()),
            format: wgpu::TextureFormat::Rgba8Unorm,
            bounds: [0, 0, size[0], size[1]],
        },
    ));
    queue.submit([encoder.finish()]);
    let out = readback(&device, &queue, &screen);
    let at = |x: u32, y: u32| out[((y * size[0] + x) * 4) as usize];
    // Tone-mapped background: pre = 0.99 * 0.125 + 0.01, post/pre scaling
    // with white point squared 1.
    let pre = 0.99f32 * 0.125 + 0.01;
    let post_l = pre * (1.0 + pre) / (pre + 1.0);
    let bg = ((0.125 * post_l / pre) * 255.0).round() as u8;
    assert!(
        at(10, 10).abs_diff(bg) <= 2,
        "background {} vs {bg}",
        at(10, 10)
    );
    // The highlight saturates; its neighbourhood gains a two-axis halo.
    assert_eq!(at(155, 105), 255);
    assert!(at(155, 113) > bg + 2, "vertical halo {}", at(155, 113));
    assert!(at(164, 105) > bg + 2, "horizontal halo {}", at(164, 105));
}
