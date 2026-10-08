use super::*;
use crate::{font_atlas::Atlas, font_metrics::Metrics, ui_paint::Quad};

fn sprite(size: [i32; 2], padding: [i32; 4], seed: i32) -> Rc<Sprite> {
    Rc::new(Sprite {
        paletted: None,
        size,
        padding,
        argb: (0..size[0] * size[1])
            .map(|i| (i.wrapping_mul(0x01f3_a5c7) ^ seed) | 0xff00_0000u32 as i32)
            .collect(),
    })
}

fn font() -> Rc<Font> {
    let mut metrics = Metrics::default();
    metrics.advances[b'A' as usize] = 7;
    metrics.widths[b'A' as usize] = 9;
    metrics.bearings[b'A' as usize] = 2;
    metrics.atlas_width = 16;
    metrics.atlas_height = 16;
    metrics.scale = 1;
    metrics.rects[b'A' as usize] = [1, 2, 7, 9];
    Rc::new(Font {
        metrics: Rc::new(metrics),
        atlas: Atlas {
            width: 16,
            height: 16,
            scale: 1,
            argb: vec![0xffff_ffff; 256],
        },
        monochrome: false,
        paletted: true,
        translucent: false,
    })
}

fn glyph(x: i32, y: i32) -> Draw {
    Draw::Glyph {
        code: b'A',
        x,
        y,
        colour: 0xff20_4060u32 as i32,
        shadow: false,
        masked: false,
    }
}

/// A frame that makes every recorded call through `Painter`'s own methods,
/// the way the UI does.
fn painted() -> Painter {
    let s = sprite([6, 5], [1, 2, 3, 1], 11);
    let t = sprite([4, 4], [0; 4], 23);
    let mask = MaskRef {
        sprite: sprite([8, 8], [0; 4], 5),
        origin: [3, 4],
    };
    let f = font();
    let mut p = Painter::new([64, 48]);
    p.reset_bounds([0, 0, 64, 48]);
    p.fill([2, 3, 10, 7], 0x80ff_0000u32 as i32).unwrap();
    p.set_bounds([1, 1, 60, 40]);
    p.outline([4, 5, 12, 9], -1);
    p.line([0, 0], [20, 13], 0xff00_ff00u32 as i32, 1);
    p.line([3, 30], [40, 2], 0xff00_00ffu32 as i32, 3);
    p.horizontal_line(5, 20, 30, 0xff11_2233u32 as i32);
    p.dashed_line([2, 2], [50, 30], -1, 4, 3, 1);
    p.dashed_line([50, 3], [2, 3], -1, 0, 0, 0);
    p.native(&s, [7, 8], -1);
    p.native(&s, [9, 10], 0x7f12_3456);
    p.scaled(&t, [3, 4, 17, 9], -1).unwrap();
    p.tiled(&t, [0, 0, 13, 11], 1).unwrap();
    let fields = Fields {
        tiling: true,
        angle2d: 0,
        width: 20,
        height: 14,
        colour: 0x0033_6699,
        ..Default::default()
    };
    p.component_sprite(&s, &fields, [5, 6], 40, [0, 0, 64, 48])
        .unwrap();
    let fields = Fields {
        tiling: false,
        angle2d: 4096,
        width: 12,
        height: 10,
        colour: -1,
        ..Default::default()
    };
    p.component_sprite(&s, &fields, [20, 16], 0, [0, 0, 64, 48])
        .unwrap();
    p.rotated_image(RotatedImage {
        image: Image::Sprite(s.clone()),
        size: [6, 5],
        origin: [30.0, 20.0],
        pivot: [3.0, 2.5],
        scale: 4096,
        angle: 12000,
        colour: -1,
        mask: None,
    });
    p.rotated_image(RotatedImage {
        image: Image::Sprite(t.clone()),
        size: [4, 4],
        origin: [10.0, 30.0],
        pivot: [2.0, 2.0],
        scale: 2048,
        angle: -3000,
        colour: 0x40ff_ffff,
        mask: Some(mask.clone()),
    });
    p.affine_image(
        Image::Sprite(s.clone()),
        [1.0, 2.0, 30.5, 4.0, 3.0, 25.0],
        -1,
        None,
    );
    p.affine_image(
        Image::White,
        [5.0, 5.0, 15.0, 5.0, 5.0, 15.0],
        0xff00_ff00u32 as i32,
        Some(mask.clone()),
    );
    p.masked_fill([6, 7, 9, 8], 0xffab_cdefu32 as i32, mask.clone());
    p.masked_native(&t, [11, 12], mask.clone());
    p.glyph(&f, &glyph(12, 30));
    p.glyph_masked(&f, &glyph(20, 31), Some(mask));
    p
}

/// A quad by value: image key, vertex bytes, clip and mask key.
type QuadKey = ((u8, usize), Vec<u8>, [i32; 4], Option<(usize, [i32; 2])>);

fn quad_key(q: &Quad) -> QuadKey {
    (
        q.image.key(),
        bytemuck::cast_slice(&q.vertices).to_vec(),
        q.clip,
        q.mask.as_ref().map(MaskRef::key),
    )
}

/// The GPU painter implements the trait with its own calls: replaying a
/// recording through `toolkit::replay` reproduces the recording and the
/// batch quads the direct calls produced.
#[test]
fn painter_replays_through_the_trait_like_its_own_calls() {
    let direct = painted();
    let mut replayed = Painter::new(direct.sprite.size);
    replay(&mut replayed, &direct.recording.ops).unwrap();
    assert_eq!(direct.recording.ops.len(), 23);
    assert_eq!(
        digest(&replayed.recording.ops),
        digest(&direct.recording.ops)
    );
    let direct = direct.finish();
    let replayed = replayed.finish();
    assert!(!direct.quads.is_empty());
    assert_eq!(
        replayed.quads.iter().map(quad_key).collect::<Vec<_>>(),
        direct.quads.iter().map(quad_key).collect::<Vec<_>>()
    );
}

/// `NullToolkit` receives every op kind with all its arguments: rebuilding
/// the recording through `toolkit::draw` gives the same op stream.
#[test]
fn null_toolkit_records_what_draw_dispatches() {
    let ops = painted().recording.ops;
    let mut null = NullToolkit::new();
    replay(&mut null, &ops).unwrap();
    assert_eq!(null.recording.ops.len(), ops.len());
    assert_eq!(null.digest(), digest(&ops));
    assert_eq!(null.counts.values().sum::<usize>(), ops.len());
    assert_eq!(null.counts.len(), 16, "every Op kind: {:?}", null.counts);
    let names: Vec<_> = null.recording.ops.iter().map(op_name).collect();
    assert_eq!(names, ops.iter().map(op_name).collect::<Vec<_>>());
}

/// The digest follows content, not `Rc` identity or order-independent
/// state: equal frames with fresh resources hash equal, any argument
/// changes it (no absolute value is pinned: only in-run comparisons use it,
/// `client910::active_toolkit`'s null backend logs it).
#[test]
fn digest_is_content_based() {
    let a = painted().recording.ops;
    let b = painted().recording.ops;
    assert!(!Rc::ptr_eq(
        match &a[9] {
            Op::Sprite(s, ..) => s,
            _ => unreachable!(),
        },
        match &b[9] {
            Op::Sprite(s, ..) => s,
            _ => unreachable!(),
        }
    ));
    assert_eq!(digest(&a), digest(&b));
    let mut c = a.clone();
    if let Op::Fill(_, colour) = &mut c[1] {
        *colour ^= 1;
    }
    assert_ne!(digest(&a), digest(&c));
    assert_ne!(digest(&a), digest(&a[1..]));
}
