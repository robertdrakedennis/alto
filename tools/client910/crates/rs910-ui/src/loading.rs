//! The staged startup loader and the cache-defined loading screens.
//!
//! - [`Loading`] keeps the stage cursor, the status texts and the published
//!   percent, and can restart for a reload. The per-stage work is the app's
//!   [`Host`], because it creates the app's owners.
//! - [`STAGES`] is the stage table: status texts, percent spans and which
//!   stages report progress.
//! - [`Presenter`] holds the published [`Snapshot`], screen switching and the
//!   cross-fade. The app draws it once per redraw on the winit thread.
//! - [`ScreenIndex`] lists the loading screens per `loadingScreen`
//!   preference; [`ScreenLayout`] decodes a screen's elements ([`ElementConfig`]:
//!   clear, sprites, bars, news, text, status) and [`CacheScreen`] draws them.
//! - [`draw_pre_loading`]: the plain bar shown before the cache screens load.
//!
//! Drawing goes through the shared [`crate::ui_paint::Painter`] (the GPU
//! toolkit's batch painter), so the same sprite/text/primitive semantics as
//! the interface presenter apply.

#[cfg(test)]
use crate::{cache::Pack, ui_paint::Painter, ui_sprites::Sprite};
#[cfg(test)]
use anyhow::Result;
#[cfg(test)]
use images::{gunzip, IMAGE_PROBE};

mod screen_config;
pub use screen_config::{
    Align, BarSprites, ElementConfig, NewsConfig, ProgressConfig, ScreenIndex, ScreenLayout,
    ScreenRef, ScreenSet, SpriteConfig, StatusConfig, TextConfig, ELEMENT_VERSIONS,
};
mod images;
pub use images::{
    decode_loading_image, image_probe_decodes, loading_sprites_archive, loading_sprites_raw,
};
mod resources;
pub use resources::{NewsManager, Resources};
mod elements;
use elements::utf16;
pub use elements::{
    draw_string, draw_string_center, draw_string_taggable, CacheScreen, DrawContext, Element,
};
mod progress_bar;
pub use progress_bar::draw_pre_loading;
mod startup;
pub use startup::{Host, Loading, Step};

use crate::{
    font_layout::{self},
    ui_text_compare::Language,
};

use Text::{CheckingForUpdates as Check, DownloadingUpdates as Fetch};

// ---------------------------------------------------------------------------
// Localised texts
// ---------------------------------------------------------------------------

/// The localised texts the loading/session presentation draws. Only EN, DE,
/// FR, PT and ES_MX exist; other languages have none ([`Text::display`]
/// yields the string "null" for them, as the 910 client does).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Text {
    /// "Checking for updates".
    CheckingForUpdates,
    /// "Fetching Updates".
    DownloadingUpdates,
    /// "Loading - please wait.".
    Loading,
    /// "Connection lost.".
    ConnectionLost,
    /// "Please wait - attempting to reestablish.".
    AttemptToReestablish,
    /// "Please wait...".
    PleaseWait,
}

impl Text {
    #[must_use]
    pub fn for_lang(self, language: Language) -> Option<&'static str> {
        use rs910_core::texts::Msg;
        match self {
            Self::CheckingForUpdates => Msg::CheckingForUpdates,
            Self::DownloadingUpdates => Msg::DownloadingUpdates,
            Self::Loading => Msg::Loading,
            Self::ConnectionLost => Msg::ConnectionLost,
            Self::AttemptToReestablish => Msg::AttemptToReestablish,
            Self::PleaseWait => Msg::PleaseWait,
        }
        .for_lang(language)
    }

    /// The text as concatenated into a message: a missing language is `"null"`.
    #[must_use]
    pub fn display(self, language: Language) -> &'static str {
        self.for_lang(language).unwrap_or("null")
    }
}

/// The client language from the launcher parameters.
#[must_use]
pub fn client_language() -> Language {
    rs910_core::texts::client_language()
}

// ---------------------------------------------------------------------------
// Stage table
// ---------------------------------------------------------------------------

/// One loading stage: id, text while running, text once finished, percent
/// span (`start`..`end`), whether the text carries a percent suffix, and
/// whether the stage reports progress within its span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stage {
    pub id: usize,
    pub text: Text,
    pub done_text: Text,
    pub start: i32,
    pub end: i32,
    pub show_percent: bool,
    pub progress: bool,
}

const fn stage(id: usize, text: Text, percent: i32) -> Stage {
    // A single-percent stage: start and end coincide.
    Stage {
        id,
        text,
        done_text: text,
        start: percent,
        end: percent,
        show_percent: true,
        progress: false,
    }
}

const fn span(id: usize, text: Text, start: i32, end: i32, progress: bool) -> Stage {
    // A stage spanning `start`..`end`, optionally reporting progress.
    Stage {
        id,
        text,
        done_text: text,
        start,
        end,
        show_percent: true,
        progress,
    }
}

/// The stage table, in order.
pub const STAGES: [Stage; 18] = [
    // Archive client, master index, loading archives.
    stage(0, Check, 2),
    // Loading screens + defaults fetch.
    span(1, Check, 2, 3, false),
    // The loading-sprite font provider.
    stage(2, Check, 3),
    // Loading fonts.
    span(3, Check, 3, 4, false),
    // First loading screen ready, state 11.
    stage(4, Check, 4),
    // Open the archives.
    span(5, Check, 4, 5, false),
    // Fetch the archive indexes.
    span(6, Check, 5, 98, true),
    // Audio defaults, state 1.
    stage(7, Check, 99),
    // Hardware platform libraries.
    stage(8, Check, 100),
    // Download the game data.
    span(9, Fetch, 0, 92, true),
    // Set up the config decoders.
    span(10, Fetch, 92, 93, false),
    // Set up the static sprites.
    span(11, Fetch, 94, 95, false),
    // World map.
    span(12, Fetch, 96, 97, false),
    // Set up the client-var system.
    stage(13, Fetch, 97),
    // Login interface + its graphics.
    stage(14, Fetch, 97),
    // Show the login screen.
    stage(15, Fetch, 100),
    // Stop the loading presenter, install the real toolkit.
    stage(16, Fetch, 100),
    // Done (state 4).
    stage(17, Fetch, 100),
];

/// The final stage (id 17).
pub const DONE_STAGE: usize = 17;

/// Bounded random ints over the legacy 48-bit generator, seeded from the
/// clock.
pub struct BoundedRandom(font_layout::Random);

impl BoundedRandom {
    #[must_use]
    pub fn seeded(seed: i64) -> Self {
        let mut random = font_layout::Random::default();
        random.set_seed(seed);
        Self(random)
    }
    /// 32 bits of the 48-bit generator.
    fn next_int(&mut self) -> i32 {
        self.0.next_int()
    }
    /// A value in `0..bound` (0 for a non-positive bound).
    pub fn below(&mut self, bound: i32) -> i32 {
        if bound <= 0 {
            return 0;
        }
        if bound & bound.wrapping_neg() == bound {
            return (((i64::from(self.next_int()) & 0xFFFF_FFFF) * i64::from(bound)) >> 32) as i32;
        }
        let limit = i32::MIN.wrapping_sub((4_294_967_296_i64 % i64::from(bound)) as i32);
        loop {
            let v = self.next_int();
            if v < limit {
                return v.rem_euclid(bound);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Presenter
// ---------------------------------------------------------------------------

/// What the loader publishes to the presenter: stage start time, text, text
/// with percent, percent and stage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub start_time: i64,
    pub text: String,
    pub text_percent: String,
    pub percent: i32,
    pub stage: Option<Stage>,
}

impl Snapshot {
    fn stage_id(&self) -> usize {
        self.stage.map_or(0, |s| s.id)
    }

    /// The percent the bar is easing toward.
    #[must_use]
    pub fn next_percent(&self) -> i32 {
        let Some(stage) = self.stage else {
            return 0;
        };
        if stage.progress && self.percent < stage.end {
            self.percent + 1
        } else if stage.id < STAGES.len() - 1 {
            if stage.start == self.percent {
                stage.end
            } else {
                stage.start
            }
        } else {
            100
        }
    }
}

/// The screen the presenter shows: the plain pre-loading bar it starts with,
/// or one of the loader's cache screens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Pre,
    Main(usize),
}

/// The screen presenter: the 910 client runs it as a 50 fps thread; here
/// [`Loading::draw`] runs one iteration of its loop per window redraw.
#[derive(Clone, Debug)]
pub struct Presenter {
    pub snapshot: Snapshot,
    /// The screen being shown.
    pub current: Shown,
    /// The screen being faded out.
    pub previous: Option<Shown>,
    /// When the shown screen was switched to.
    pub switched_at: i64,
    /// Number of draws.
    pub draws: i32,
}

impl Presenter {
    fn new() -> Self {
        Self {
            snapshot: Snapshot::default(),
            current: Shown::Pre,
            previous: None,
            switched_at: 0,
            draws: 0,
        }
    }
}

/// `Image::External` ids of the loading cross-fade's framebuffer sprites
/// (previous, current). The 910 client reuses one framebuffer and sprite; the
/// two composites of a frame are separate GPU targets here.
pub const FADE_FRAMEBUFFER_ID: u64 = 0x4c4f_4144_0000;

/// Initial frame width and height.
pub const DEFAULT_FRAME: [i32; 2] = [765, 553];

#[cfg(test)]
mod tests;
