//! The far scene's development variable, read once (like each crate's
//! `*_debug_flags`). The draw-distance level itself is a renderer setting
//! (`rs910_render_modern::settings::ModernSettings::far`); nothing here is
//! read from or written to `ClientOptions` or the preferences, so the
//! CS2-visible profile does not change (design §6.2 "Level option").

use std::sync::OnceLock;

/// See the module docs.
#[derive(Debug)]
pub struct FarDebugFlags {
    /// `CLIENT910_MODERN_FAR_SYNC=1`: build the whole ring before the frame,
    /// in ring order (screenshots and tests); otherwise a few squares per
    /// frame ([`crate::far_world::FRAME_BUDGET`]).
    pub sync: bool,
}

/// The flags, parsed on first use.
pub fn flags() -> &'static FarDebugFlags {
    static FLAGS: OnceLock<FarDebugFlags> = OnceLock::new();
    FLAGS.get_or_init(|| FarDebugFlags {
        sync: std::env::var("CLIENT910_MODERN_FAR_SYNC").is_ok_and(|v| v == "1"),
    })
}
