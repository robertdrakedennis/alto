use super::*;

fn pack() -> Pack {
    crate::test_support::require_pack("client.loadingscreen.js5")
}

/// The screen index and layouts over the 910 cache: preference 0
/// shows screen 2 (background, 500 ms fade) then screen 1 (the full
/// screen, shown for 600000 ms); screen 1's elements decode.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn cache_loading_screens_decode() {
    let pack = pack();
    let index = ScreenIndex::decode(
        pack.read_group("loadingscreen", 0)
            .unwrap()
            .get(&0)
            .map(Vec::as_slice),
    )
    .unwrap();
    assert!(!index.has_first());
    let refs = index.screens(0, &mut |_| 0);
    assert_eq!(
        refs,
        [
            ScreenRef {
                id: 2,
                min_time: 0,
                fade: 500
            },
            ScreenRef {
                id: 1,
                min_time: 600_000,
                fade: 500
            },
        ]
    );
    // An out-of-range preference falls back to the fixed-first-screen set.
    assert!(index.screens(9, &mut |_| 0).is_empty());
    let layout = ScreenLayout::decode(&pack.read_group("loadingscreen", 1).unwrap()[&0]).unwrap();
    assert_eq!(layout.elements.len(), 16);
    assert!(matches!(
        layout.elements[0],
        Some(ElementConfig::Background { sprite: 21231 })
    ));
    assert!(matches!(
        layout.elements[3],
        Some(ElementConfig::RotatingSprite { speed: -1755, .. })
    ));
    let Some(ElementConfig::Text(text)) = &layout.elements[15] else {
        panic!("text element");
    };
    assert_eq!(text.text, "Preparing Your Greatest Adventure");
}

/// Over the cache: each JPEG loading sprite decodes to the size (and,
/// within JPEG decoder rounding, the pixels) of its `loadingspritesraw`
/// counterpart.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn jpeg_loading_sprites_match_the_raw_archive() {
    let pack = pack();
    for id in [21231, 21380, 21381, 30460, 30462] {
        let jpeg = pack
            .read_group("loadingsprites", id)
            .unwrap()
            .remove(&0)
            .unwrap();
        let sprite = decode_loading_image(&jpeg).unwrap();
        let raw = pack
            .read_group("loadingspritesraw", id)
            .unwrap()
            .remove(&0)
            .unwrap();
        let raw = crate::sprite_data::Data::decode(&raw).unwrap().remove(0);
        let raw = Sprite::new(&raw).unwrap();
        assert_eq!(sprite.full_size(), raw.full_size(), "sprite {id}");
        assert_eq!(sprite.argb.len(), raw.argb.len(), "sprite {id}");
        // The raw archive holds its own pixels (not a decode of these
        // JPEGs): alpha, the fourth JPEG component, matches exactly; the
        // colour differs by the JPEG's compression noise, so only its
        // mean error is bounded.
        let channel = |p: i32, c: i32| (p >> (c * 8)) & 0xff;
        let mut alpha = 0u64;
        let mut colour = (0u64, 0u64);
        for (&a, &b) in sprite.argb.iter().zip(&raw.argb) {
            alpha += u64::from(channel(a, 3).abs_diff(channel(b, 3)));
            if channel(b, 3) == 0xff {
                colour.0 += (0..3)
                    .map(|c| u64::from(channel(a, c).abs_diff(channel(b, c))))
                    .sum::<u64>();
                colour.1 += 3;
            }
        }
        let alpha = alpha as f64 / sprite.argb.len() as f64;
        let colour = colour.0 as f64 / colour.1.max(1) as f64;
        eprintln!("sprite {id}: mean alpha error {alpha:.2}, opaque colour error {colour:.2}");
        assert!(alpha < 1.0 && colour < 8.0, "sprite {id}: {alpha} {colour}");
    }
}

/// The bounded draws over the legacy 48-bit generator seeded 42. The expected
/// values were recorded from the original algorithm on that generator: one
/// `next_int`, then six draws each for a power-of-two bound (the `>> 32`
/// multiply branch), 100 and 7 (the rejection loop and floor-mod branch), all
/// from one generator.
#[test]
fn bounded_random_matches_recorded_sequence() {
    let mut random = BoundedRandom::seeded(42);
    assert_eq!(random.next_int(), -1_170_105_035);
    let draws = |random: &mut BoundedRandom, bound| -> Vec<i32> {
        (0..6).map(|_| random.below(bound)).collect()
    };
    assert_eq!(draws(&mut random, 8), [0, 5, 0, 2, 7, 2]);
    assert_eq!(draws(&mut random, 100), [41, 42, 86, 69, 4, 53]);
    assert_eq!(draws(&mut random, 7), [0, 2, 2, 1, 6, 4]);
}
