use super::*;

fn pack() -> Pack {
    crate::test_support::require_pack("client.loadingscreen.js5")
}

/// Records the host effects of `Loading::update`.
struct FakeHost {
    now: i64,
    state: i32,
    states: Vec<i32>,
    logins: Vec<(String, String)>,
    stages: Vec<usize>,
    /// Archive-index fetch averages reported before completion.
    indexes: Vec<i32>,
    pack: Pack,
}

impl FakeHost {
    fn new() -> Self {
        Self {
            now: 1_000,
            state: 5,
            states: Vec::new(),
            logins: Vec::new(),
            stages: Vec::new(),
            indexes: vec![40, 70],
            pack: Pack::open("/nonexistent-pack"),
        }
    }
}

impl Host for FakeHost {
    fn now(&self) -> i64 {
        self.now
    }
    fn client_state(&self) -> i32 {
        self.state
    }
    fn set_state(&mut self, state: i32) {
        self.state = state;
        self.states.push(state);
    }
    fn request_login(&mut self, username: String, password: String) {
        self.logins.push((username, password));
    }
    fn safe_mode_key(&mut self) {}
    fn pack(&self) -> Pack {
        self.pack.clone()
    }
    fn loading_screen_preference(&self) -> i32 {
        0
    }
    fn world(&self) -> (String, i32) {
        ("127.0.0.1".into(), 1)
    }
    fn default_fonts(&self) -> Option<Vec<i32>> {
        Some(Vec::new())
    }
    fn stage(&mut self, stage: usize) -> Result<Step> {
        self.stages.push(stage);
        if stage == 6 && !self.indexes.is_empty() {
            return Ok(Step::Stay(self.indexes.remove(0)));
        }
        if stage == 7 {
            self.set_state(1);
        }
        Ok(Step::Next)
    }
}

/// Stage texts, the published percent, the archive-index base/interpolation,
/// the 5 -> 11 -> 1 -> 4 states and the reload's login request.
#[test]
fn update_walks_every_stage() {
    let mut host = FakeHost::new();
    let mut loading = Loading::new(Language::En);
    loading.reload("user".into(), "pass".into());
    loading.update(&mut host).unwrap();
    // Stage 0 advanced: the finished stage's done text + its end percent.
    assert_eq!(loading.stage, 1);
    assert_eq!(loading.text_percent, "Checking for updates - 2%");
    assert_eq!(loading.presenter.as_ref().unwrap().snapshot.percent, 2);
    for _ in 0..5 {
        loading.update(&mut host).unwrap();
    }
    assert_eq!(host.states, [11]);
    assert_eq!(loading.stage, 6);
    // First index average 40 is the base: (40 - 40) * 100 / 60 = 0.
    loading.update(&mut host).unwrap();
    assert_eq!(loading.percent, 5);
    assert_eq!(loading.text_percent, "Checking for updates - 5%");
    // 70 -> (70 - 40) * 100 / 60 = 50 -> 5 + 93 * 50 / 100 = 51.
    loading.update(&mut host).unwrap();
    assert_eq!(loading.percent, 51);
    while loading.stage != DONE_STAGE {
        loading.update(&mut host).unwrap();
    }
    assert_eq!(host.states, [11, 1, 4]);
    assert_eq!(host.logins, [("user".into(), "pass".into())]);
    assert!(loading.presenter.is_none());
    // Stages 3 and 4 are Loading's own; 16 runs Loading's cleanup first.
    assert!(!host.stages.contains(&3) && !host.stages.contains(&4));
    assert!(host.stages.contains(&16));
}

/// The bar's next target follows the stage span.
#[test]
fn next_percent_follows_the_stage_span() {
    let mut s = Snapshot {
        percent: 40,
        stage: Some(STAGES[6]),
        ..Snapshot::default()
    };
    assert_eq!(s.next_percent(), 41);
    s.stage = Some(STAGES[10]);
    s.percent = 92;
    assert_eq!(s.next_percent(), 93);
    s.percent = 93;
    assert_eq!(s.next_percent(), 92);
    s.stage = Some(STAGES[17]);
    assert_eq!(s.next_percent(), 100);
    assert_eq!(Snapshot::default().next_percent(), 0);
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

/// A cache screen over the cache: every element becomes ready from
/// the JPEG loading sprites, creates, and paints the background, sprites,
/// the rotating ring and the status/percent text.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn cache_loading_screen_draws() {
    let pack = pack();
    let mut resources = Resources::new(pack.clone(), None, ("127.0.0.1".into(), 1));
    let layout = ScreenLayout::decode(&pack.read_group("loadingscreen", 1).unwrap()[&0]).unwrap();
    let mut screen = CacheScreen::new(1, layout, 600_000, 500, &mut resources);
    assert_eq!(screen.ready_percent(&mut resources).unwrap(), 100);
    screen.create(&mut resources).unwrap();
    let snapshot = Snapshot {
        start_time: 0,
        text: "Checking for updates".into(),
        text_percent: "Checking for updates - 50%".into(),
        percent: 50,
        stage: Some(STAGES[6]),
    };
    let mut painter = Painter::new([800, 600]);
    let mut ctx = DrawContext {
        painter: &mut painter,
        canvas: [800, 600],
        frame: DEFAULT_FRAME,
        now: 5_000,
        snapshot: &snapshot,
        draws: 1,
        b12: None,
    };
    screen.draw(&mut ctx).unwrap();
    let plan = painter.finish();
    // 4 sprites + glyphs of "50%" x5, "Checking for updates" x3 and the
    // title x3.
    assert!(plan.quads.len() > 60, "{} quads", plan.quads.len());
    // The rotating ring advanced by its speed.
    let angle = screen.elements[3].as_ref().unwrap().angle;
    assert_eq!(angle, -1755);
}

/// During a cross-fade each screen is
/// drawn into its own framebuffer sprite and composited once at
/// 255 - a then a, instead of blending every quad of a screen.
#[test]
fn cross_fade_composites_framebuffer_sprites() {
    let clear = |colour| CacheScreen {
        id: 0,
        elements: vec![Some(Element {
            config: ElementConfig::Clear { colour },
            font: None,
            sprites: Vec::new(),
            angle: 0,
            shown_percent: 0,
            since: 0,
            news: None,
        })],
        min_time: 0,
        fade: 500,
    };
    let mut loading = Loading::new(Language::En);
    loading.screens = Some(vec![
        clear(0xffff_0000_u32 as i32),
        clear(0xff00_ff00_u32 as i32),
    ]);
    loading.presenter = Some(Presenter {
        current: Shown::Main(1),
        previous: Some(Shown::Main(0)),
        switched_at: 1_000,
        ..Presenter::new()
    });
    let mut painter = Painter::new([40, 30]);
    loading
        .draw(&mut painter, [40, 30], DEFAULT_FRAME, 1_250)
        .unwrap();
    let a = (250 * 255 / 500) as u8;
    assert_eq!(
        painter.layers.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        [FADE_FRAMEBUFFER_ID, FADE_FRAMEBUFFER_ID + 1]
    );
    // Each layer holds its whole screen, unblended.
    for (_, plan) in &painter.layers {
        assert!(!plan.quads.is_empty());
        assert!(plan.quads.iter().all(|q| q.vertices[0].colour[3] == 255));
    }
    let plan = painter.finish();
    let composites: Vec<_> = plan
        .quads
        .iter()
        .filter_map(|q| match q.image {
            crate::ui_paint::Image::External(id) => Some((id, q.vertices[0].colour)),
            _ => None,
        })
        .collect();
    assert_eq!(
        composites,
        [
            (FADE_FRAMEBUFFER_ID, [255, 255, 255, 255 - a]),
            (FADE_FRAMEBUFFER_ID + 1, [255, 255, 255, a]),
        ]
    );
    // After the fade the previous screen is dropped: no layers.
    let mut painter = Painter::new([40, 30]);
    loading
        .draw(&mut painter, [40, 30], DEFAULT_FRAME, 1_600)
        .unwrap();
    assert!(painter.layers.is_empty());
    assert!(loading.presenter.as_ref().unwrap().previous.is_none());
}

/// The shown percent is x100 and eased toward the next target over the previous step's duration.
#[test]
fn bar_percent_eases_toward_the_next_target() {
    let mut element = Element {
        config: ElementConfig::Clear { colour: 0 },
        font: None,
        sprites: Vec::new(),
        angle: 0,
        shown_percent: 0,
        since: 0,
        news: None,
    };
    let snapshot = Snapshot {
        start_time: 1_000,
        percent: 10,
        stage: Some(STAGES[6]),
        ..Snapshot::default()
    };
    let mut painter = Painter::new([10, 10]);
    let mut ctx = DrawContext {
        painter: &mut painter,
        canvas: [10, 10],
        frame: DEFAULT_FRAME,
        now: 2_000,
        snapshot: &snapshot,
        draws: 0,
        b12: None,
    };
    // A new percent is shown as is and starts the timer.
    assert_eq!(element.interpolated_percent(&ctx), 1_000);
    assert_eq!(element.since, 2_000);
    // The shown percent took 1000 ms from the stage start, so each
    // further percent is eased over 1000 / 10 ms.
    // 50 ms of the 100 ms the next percent is expected to take.
    ctx.now = 2_050;
    assert_eq!(element.interpolated_percent(&ctx), 1_050);
    ctx.now = 2_200;
    assert_eq!(element.interpolated_percent(&ctx), 1_100);
}

/// The GZip'd probe inflates to a JPEG that decodes, so the loading
/// screens read the JPEG `loadingsprites` archive.
#[test]
fn image_probe_selects_the_jpeg_loading_sprites() {
    let probe = gunzip(&IMAGE_PROBE).unwrap();
    assert_eq!(probe.len(), 622);
    assert_eq!(&probe[..2], &[0xff, 0xd8]);
    assert!(decode_loading_image(&probe).is_ok());
    assert!(image_probe_decodes());
    assert!(!loading_sprites_raw());
    assert_eq!(loading_sprites_archive(), "loadingsprites");
    // A corrupt header is rejected.
    assert!(gunzip(&IMAGE_PROBE[1..]).is_err());
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

/// The 304x34 outlined bar centred on the canvas with a 3 px/percent fill.
#[test]
fn pre_loading_bar_is_centred_with_a_3px_percent_fill() {
    let mut painter = Painter::new([800, 600]);
    draw_pre_loading(&mut painter, [800, 600], 50, "x", Some("y"), 0, None).unwrap();
    let plan = painter.finish();
    // clear, 4 outline lines, fill, 4 inner lines, black remainder.
    assert_eq!(plan.quads.len(), 11);
    let bounds = |q: &crate::ui_paint::Quad| {
        let xs = q.vertices.map(|v| (v.position[0] + 1.0) / 2.0 * 800.0);
        (
            xs.iter().copied().fold(f32::MAX, f32::min),
            xs.iter().copied().fold(f32::MIN, f32::max),
        )
    };
    // The fill spans x = 400 - 152 + 2 .. + 150.
    let (l, r) = bounds(&plan.quads[5]);
    assert!(
        (l - 250.0).abs() < 0.01 && (r - 400.0).abs() < 0.01,
        "{l} {r}"
    );
}

/// Both strings are placed by `length * 6`, whatever the glyph widths: the
/// bar text at `x + (304 - len * 6) / 2`, baseline `y + 22`, the loading text
/// at `canvas_width / 2 - len * 6 / 2`, baseline `canvas_height / 2 - 26`.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn pre_loading_text_uses_fixed_placement() {
    let pack = crate::test_support::require_pack("client.fontmetrics.js5");
    let fonts = crate::ui_fonts::Fonts::from_pack(pack, Some(0)).unwrap();
    let font = fonts
        .get_font(fonts.ids.as_ref().unwrap()[2], true, true)
        .unwrap()
        .unwrap();
    let text = "Checking for updates - 3%";
    let loading = Text::Loading.display(Language::En);
    let glyph_left = |painter: Painter| {
        let plan = painter.finish();
        plan.quads
            .iter()
            .filter(|q| matches!(q.image, crate::ui_paint::Image::Font(_)))
            .map(|q| {
                let xs = q.vertices.map(|v| (v.position[0] + 1.0) / 2.0 * 800.0);
                let ys = q.vertices.map(|v| (1.0 - v.position[1]) / 2.0 * 600.0);
                (
                    xs.iter().copied().fold(f32::MAX, f32::min),
                    ys.iter().copied().fold(f32::MAX, f32::min),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut bar = Painter::new([800, 600]);
    draw_pre_loading(&mut bar, [800, 600], 3, text, None, 0, Some(&font)).unwrap();
    let bar = glyph_left(bar);
    let mut both = Painter::new([800, 600]);
    draw_pre_loading(
        &mut both,
        [800, 600],
        3,
        text,
        Some(loading),
        0,
        Some(&font),
    )
    .unwrap();
    let both = glyph_left(both);
    assert_eq!(&both[..bar.len()], &bar[..]);
    let extra = &both[bar.len()..];
    assert!(!extra.is_empty());
    // The loading text's first glyph starts at its pen x (plus the glyph's
    // left bearing) and sits above the bar (baseline 300 - 26 = 274).
    let pen = (400 - loading.len() as i32 * 6 / 2) as f32;
    assert!(
        extra[0].0 >= pen && extra[0].0 < pen + 4.0,
        "{:?}",
        extra[0]
    );
    assert!(extra.iter().all(|&(_, y)| y < 274.0));
    let bar_pen = (400 - 152 + (304 - text.len() as i32 * 6) / 2) as f32;
    assert!(
        bar[0].0 >= bar_pen && bar[0].0 < bar_pen + 4.0,
        "{:?}",
        bar[0]
    );
}

/// Screens switch only after state 5 and the shown screen's minimum time.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn screens_cycle_after_loading_state_5() {
    let pack = pack();
    let mut host = FakeHost::new();
    host.pack = pack.clone();
    let mut loading = Loading::new(Language::En);
    loading.update(&mut host).unwrap(); // stage 0
    loading.update(&mut host).unwrap(); // stage 1: the screen list
    assert_eq!(loading.screens.as_ref().unwrap().len(), 2);
    assert_eq!(loading.presenter.as_ref().unwrap().current, Shown::Pre);
    loading.update(&mut host).unwrap();
    loading.update(&mut host).unwrap();
    assert_eq!(host.state, 5);
    assert_eq!(loading.presenter.as_ref().unwrap().current, Shown::Pre);
    // Stage 4 enters state 11 before this update's switch pass; screen
    // 2 has no minimum time, so the same pass reaches screen 1.
    loading.update(&mut host).unwrap();
    assert_eq!(host.state, 11);
    let presenter = loading.presenter.as_ref().unwrap();
    assert_eq!(presenter.current, Shown::Main(1));
    assert_eq!(presenter.previous, Some(Shown::Main(0)));
    assert_eq!(loading.screen_index, -1);
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
