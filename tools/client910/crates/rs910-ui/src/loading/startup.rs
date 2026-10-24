//! Staged startup and reload progress through the application host.

use crate::{cache::Pack, ui_paint::Painter, ui_text_compare::Language};

use anyhow::{Context as _, Result};

use rs910_core::fault::Fault;

use super::{
    draw_pre_loading, BoundedRandom, CacheScreen, DrawContext, Presenter, Resources, ScreenIndex,
    ScreenLayout, Shown, Snapshot, Text, DONE_STAGE, FADE_FRAMEBUFFER_ID, STAGES,
};

// ---------------------------------------------------------------------------
// Loader
// ---------------------------------------------------------------------------

/// Result of one app-owned stage body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Stay in the stage, reporting `n` percent.
    Stay(i32),
    /// Advance to the next stage.
    Next,
}

/// The app side of the stage bodies: everything they create that belongs to
/// other client owners (caches, toolkit, audio, session).
pub trait Host {
    /// The monotonic clock.
    fn now(&self) -> i64;
    /// The client state.
    fn client_state(&self) -> i32;
    /// Sets the client state.
    fn set_state(&mut self, state: i32);
    /// Requests a login for the reload credentials once loading is done.
    fn request_login(&mut self, username: String, password: String);
    /// With safe mode off, an 's'/'S' key event turns safe mode on and
    /// selects the safe toolkit type.
    fn safe_mode_key(&mut self);
    /// The pack all archives read.
    fn pack(&self) -> Pack;
    /// The `loadingScreen` preference.
    fn loading_screen_preference(&self) -> i32;
    /// The world host and node (loading-screen news).
    fn world(&self) -> (String, i32);
    /// The graphics-defaults p11/p12/b12 full font ids, once stage 1
    /// decoded them.
    fn default_fonts(&self) -> Option<Vec<i32>>;
    /// The app-owned body of `stage`.
    fn stage(&mut self, stage: usize) -> Result<Step>;
}

/// The loader state.
pub struct Loading {
    /// Whether a load is running.
    started: bool,
    /// The current stage index into [`STAGES`].
    pub stage: usize,
    /// When the current stage (or progress span) started.
    pub start_time: i64,
    /// The status text and the status text with percent.
    pub text: String,
    pub text_percent: String,
    /// The published percent.
    pub percent: i32,
    /// The credentials of a pending reload.
    pub reload: Option<(String, String)>,
    /// The first archive-index fetch average.
    index_base: i32,
    /// The cache screens.
    pub screens: Option<Vec<CacheScreen>>,
    /// Index of the last screen switched to.
    pub screen_index: i32,
    /// Whether the screen index has a fixed first screen.
    has_first: bool,
    /// The presenter.
    pub presenter: Option<Presenter>,
    /// The loading sprites archive, element factory and loading font
    /// provider (the client's font provider until the index fetch).
    pub resources: Option<Resources>,
    pub language: Language,
}

impl Loading {
    #[must_use]
    pub fn new(language: Language) -> Self {
        Self {
            started: false,
            stage: 0,
            start_time: 0,
            text: String::new(),
            text_percent: String::new(),
            percent: 0,
            reload: None,
            index_base: -1,
            screens: None,
            screen_index: -1,
            has_first: false,
            presenter: None,
            resources: None,
            language,
        }
    }

    /// Restarts loading for a reload, after the caller's logout and resource
    /// reset; the caller then enters state 5.
    pub fn reload(&mut self, username: String, password: String) {
        self.reload = Some((username, password));
        self.started = false;
    }

    /// Creates the presenter at the current stage.
    pub(super) fn create_presenter(&mut self) {
        if !self.started {
            return;
        }
        let stage = STAGES[self.stage];
        let text = stage.text.display(self.language).to_owned();
        let mut presenter = Presenter::new();
        presenter.snapshot = Snapshot {
            start_time: self.start_time,
            text: text.clone(),
            text_percent: text,
            percent: stage.start,
            stage: Some(stage),
        };
        self.presenter = Some(presenter);
    }

    /// Moves to the next stage (the last one stays).
    pub(super) fn advance(&mut self) -> i32 {
        if self.stage < STAGES.len() - 1 {
            self.stage += 1;
        }
        100
    }

    /// One loader tick: runs the stage body, updates the texts and percent,
    /// publishes the snapshot and cycles the screens.
    pub fn update(&mut self, host: &mut dyn Host) -> Result<()> {
        if !self.started {
            self.started = true;
            self.stage = 0;
            self.start_time = host.now();
            self.index_base = -1;
        }
        if self.presenter.is_none() {
            self.create_presenter();
        }
        let before = self.stage;
        let progress = self.run_stage_body(host)?;
        if self.stage == before {
            let stage = STAGES[self.stage];
            self.text = stage.text.display(self.language).to_owned();
            self.text_percent = self.text.clone();
            if stage.progress {
                self.percent = (stage.end - stage.start) * progress / 100 + stage.start;
            }
            if stage.show_percent {
                self.text_percent = format!("{} - {}%", self.text_percent, self.percent);
            }
        } else if self.stage == DONE_STAGE {
            self.presenter = None;
            host.set_state(4);
            if let Some((username, password)) = self.reload.take() {
                host.request_login(username, password);
            }
        } else {
            let previous = STAGES[before];
            let stage = STAGES[self.stage];
            self.text = previous.done_text.display(self.language).to_owned();
            self.text_percent = self.text.clone();
            if stage.show_percent {
                self.text_percent = format!("{} - {}%", self.text_percent, previous.end);
            }
            self.percent = previous.end;
            if stage.progress || previous.progress {
                self.start_time = host.now();
            }
        }
        let Some(presenter) = self.presenter.as_mut() else {
            return Ok(());
        };
        presenter.snapshot = Snapshot {
            start_time: self.start_time,
            text: self.text.clone(),
            text_percent: self.text_percent.clone(),
            percent: self.percent,
            stage: Some(STAGES[self.stage]),
        };
        self.cycle_screens(host)
    }

    /// Advances to the next screen once it is ready, the client state left 5
    /// and the shown screen's minimum time passed.
    pub(super) fn cycle_screens(&mut self, host: &mut dyn Host) -> Result<()> {
        let (Some(screens), Some(resources), Some(presenter)) = (
            self.screens.as_mut(),
            self.resources.as_mut(),
            self.presenter.as_mut(),
        ) else {
            return Ok(());
        };
        let now = host.now();
        let len = screens.len();
        let mut next = (self.screen_index + 1) as usize;
        while next < len {
            let ready = screens[next].ready_percent(resources)? >= 100;
            let shown_long_enough = match presenter.current {
                Shown::Pre => true,
                Shown::Main(i) => screens[i].shown_long_enough(presenter.switched_at, now),
            };
            if ready
                && self.screen_index == next as i32 - 1
                && host.client_state() != 5
                && shown_long_enough
            {
                if let Err(error) = screens[next].create(resources) {
                    log::warn!(
                        "[client910] loading screen {} create failed: {error:#}",
                        screens[next].id
                    );
                    self.screens = None;
                    break;
                }
                // Switch: the shown screen becomes the previous one.
                presenter.previous = Some(presenter.current);
                presenter.current = Shown::Main(next);
                presenter.switched_at = now;
                self.screen_index += 1;
                if self.screen_index >= len as i32 - 1 && len > 1 {
                    self.screen_index = if self.has_first { 0 } else { -1 };
                }
            }
            next += 1;
        }
        Ok(())
    }

    /// The loader's own stage bodies around the app's [`Host::stage`];
    /// returns the stage progress percent.
    pub(super) fn run_stage_body(&mut self, host: &mut dyn Host) -> Result<i32> {
        host.safe_mode_key();
        let stage = self.stage;
        match stage {
            // Stage 1: the app fetches defaults + the loading archives, then
            // the loading-screen list for the preference.
            1 => {
                if let Step::Stay(v) = host.stage(stage)? {
                    return Ok(v);
                }
                let pack = host.pack();
                let mut resources =
                    Resources::new(pack.clone(), host.default_fonts(), host.world());
                let index = ScreenIndex::decode(
                    pack.read_group("loadingscreen", 0)
                        .ok()
                        .and_then(|mut files| files.remove(&0))
                        .as_deref(),
                )?;
                self.has_first = index.has_first();
                let mut random = BoundedRandom::seeded(host.now());
                let mut refs =
                    index.screens(host.loading_screen_preference(), &mut |b| random.below(b));
                if refs.is_empty() {
                    refs = index.screens(0, &mut |b| random.below(b));
                }
                if !refs.is_empty() {
                    let mut screens = Vec::with_capacity(refs.len());
                    for r in refs {
                        // The screen's layout file.
                        let bytes = u32::try_from(r.id)
                            .ok()
                            .and_then(|g| pack.read_group("loadingscreen", g).ok())
                            .and_then(|mut files| files.remove(&0))
                            .with_context(|| {
                                Fault::MissingValue.message("loading screen layout")
                            })?;
                        let layout = ScreenLayout::decode(&bytes)?;
                        screens.push(CacheScreen::new(
                            r.id,
                            layout,
                            r.min_time,
                            r.fade,
                            &mut resources,
                        ));
                    }
                    self.screens = Some(screens);
                }
                self.resources = Some(resources);
            }
            // Stage 3: wait for the fonts.
            3 => {
                if let Some(fonts) = self.resources.as_ref().map(|r| &r.fonts) {
                    let loaded = fonts.loaded_count(false)?;
                    let count = fonts.count();
                    if loaded < count {
                        return Ok(loaded * 100 / count);
                    }
                }
            }
            // Stage 4: the first screens must be ready.
            4 => {
                if let (Some(screens), Some(resources)) =
                    (self.screens.as_mut(), self.resources.as_mut())
                {
                    if !screens.is_empty() {
                        if screens[0].ready_percent(resources)? < 100 {
                            return Ok(0);
                        }
                        if screens.len() > 1
                            && self.has_first
                            && screens[1].ready_percent(resources)? < 100
                        {
                            return Ok(0);
                        }
                    }
                }
                if let Some(resources) = self.resources.as_ref() {
                    // Fill the default font slots.
                    resources.fonts.load_fonts()?;
                }
                host.set_state(11);
            }
            // Archive indexes: the app reports the provider average.
            6 => match host.stage(stage)? {
                Step::Stay(average) => {
                    if self.index_base < 0 {
                        self.index_base = average;
                    }
                    let span = 100 - self.index_base;
                    return Ok(if span == 0 {
                        0
                    } else {
                        (average - self.index_base) * 100 / span
                    });
                }
                Step::Next => {}
            },
            // Stage 16: stop the presenter and drop the loading archives, then
            // the app selects the real toolkit.
            16 => {
                self.presenter = None;
                if let Some(resources) = self.resources.as_mut() {
                    resources.reset_sprites();
                }
                self.resources = None;
                self.screens = None;
                if let Step::Stay(v) = host.stage(stage)? {
                    return Ok(v);
                }
            }
            _ => {
                if let Step::Stay(v) = host.stage(stage)? {
                    return Ok(v);
                }
            }
        }
        Ok(self.advance())
    }

    /// Draws one presenter iteration. `canvas` is the canvas size, `frame`
    /// the frame size.
    pub fn draw(
        &mut self,
        painter: &mut Painter,
        canvas: [i32; 2],
        frame: [i32; 2],
        now: i64,
    ) -> Result<()> {
        if self.presenter.is_none() {
            self.create_presenter();
        }
        let Some(presenter) = self.presenter.as_mut() else {
            return Ok(());
        };
        presenter.draws = presenter.draws.wrapping_add(1);
        // The b12 full font (third default font).
        let b12 = self.resources.as_ref().and_then(|r| {
            let id = r.fonts.ids.as_ref()?.get(2).copied()?;
            r.plain.get_font(id, false, true).ok().flatten()
        });
        match presenter.current {
            Shown::Pre => {
                let colour = crate::applet_params::get().loading_bar_colour();
                return draw_pre_loading(
                    painter,
                    canvas,
                    presenter.snapshot.percent,
                    &presenter.snapshot.text_percent,
                    Some(Text::Loading.display(self.language)),
                    colour,
                    b12.as_ref(),
                );
            }
            Shown::Main(current) => {
                let snapshot = presenter.snapshot.clone();
                let draws = presenter.draws;
                let fade = match presenter.previous {
                    Some(Shown::Main(p)) => self
                        .screens
                        .as_ref()
                        .and_then(|s| s.get(p))
                        .map_or(0, |s| s.fade),
                    // Fading from the plain bar: no cross-fade.
                    Some(Shown::Pre) | None => 0,
                };
                let switched_at = presenter.switched_at;
                let previous = presenter.previous;
                let Some(screens) = self.screens.as_mut() else {
                    return Ok(());
                };
                let mut ctx = DrawContext {
                    painter,
                    canvas,
                    frame,
                    now,
                    snapshot: &snapshot,
                    draws,
                    b12,
                };
                // Clear to opaque black before a full redraw.
                ctx.painter
                    .fill([0, 0, canvas[0], canvas[1]], 0xff00_0000_u32 as i32)?;
                if previous.is_none() || fade == 0 || switched_at < now - i64::from(fade) {
                    // Fade finished (or none): drop the previous screen.
                    if let Some(r) = self.presenter.as_mut() {
                        r.previous = None;
                    }
                    screens[current].draw(&mut ctx)?;
                } else {
                    // Cross-fade: each screen draws into a canvas-sized
                    // framebuffer sprite, composited with alpha `255 - a`
                    // (previous) then `a` (current).
                    let a = ((now - switched_at) * 255 / i64::from(fade)) as i32;
                    let size = ctx.painter.sprite.size;
                    let draw_layer = |screen: &mut CacheScreen, ctx: &mut DrawContext<'_>| {
                        let mut layer = Painter::new(size);
                        let mut sub = DrawContext {
                            painter: &mut layer,
                            canvas: ctx.canvas,
                            frame: ctx.frame,
                            now: ctx.now,
                            snapshot: ctx.snapshot,
                            draws: ctx.draws,
                            b12: ctx.b12.clone(),
                        };
                        screen.draw(&mut sub)?;
                        Ok::<_, anyhow::Error>(layer)
                    };
                    if let Some(Shown::Main(p)) = previous {
                        let layer = draw_layer(&mut screens[p], &mut ctx)?;
                        ctx.painter.framebuffer_sprite(
                            FADE_FRAMEBUFFER_ID,
                            layer,
                            (255 - a) << 24 | 0xFF_FFFF,
                        );
                    }
                    // The framebuffer is cleared and reused.
                    let layer = draw_layer(&mut screens[current], &mut ctx)?;
                    ctx.painter.framebuffer_sprite(
                        FADE_FRAMEBUFFER_ID + 1,
                        layer,
                        a << 24 | 0xFF_FFFF,
                    );
                }
            }
        }
        Ok(())
    }
}
