//! Borrow helpers over the app's session slot (code-quality programme
//! Phase 4.4): `self.session.game_mut()` instead of
//! `self.session.as_mut().and_then(|s| s.game.as_mut())`. Each helper
//! borrows only the `Option<Session>` it is called on.
use super::*;

/// The session's game and interface owners, when a session exists.
pub trait SessionSlot {
    /// The session's game owner.
    fn game(&self) -> Option<&crate::client_game::ClientGame>;
    fn game_mut(&mut self) -> Option<&mut crate::client_game::ClientGame>;
    /// The session's retained interface runtime.
    fn ui(&self) -> Option<&crate::ui_runtime::Runtime>;
    fn ui_mut(&mut self) -> Option<&mut crate::ui_runtime::Runtime>;
}

impl SessionSlot for Option<Session> {
    fn game(&self) -> Option<&crate::client_game::ClientGame> {
        self.as_ref().and_then(|s| s.game.as_ref())
    }
    fn game_mut(&mut self) -> Option<&mut crate::client_game::ClientGame> {
        self.as_mut().and_then(|s| s.game.as_mut())
    }
    fn ui(&self) -> Option<&crate::ui_runtime::Runtime> {
        self.as_ref().map(|s| &s.ui)
    }
    fn ui_mut(&mut self) -> Option<&mut crate::ui_runtime::Runtime> {
        self.as_mut().map(|s| &mut s.ui)
    }
}
