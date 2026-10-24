//! Client-state class predicates: which client states count as loading,
//! map-rebuild, title, lobby and in-game.
//!
//! Pure `i32 -> bool` tests shared by the session lifecycle
//! (`client910::login_state`, which re-exports them) and the JS5 client
//! (`rs910_js5::js5net`: its error handling and the login status it sends).
//! The state constants stay in `login_state`; the class membership below is
//! the revision data.

/// States of the loading class.
const LOADING: &[i32] = &[5, 11, 1];
/// States of the map-rebuild class.
const REBUILD: &[i32] = &[10, 6, 3, 16, 8];
/// States of the title class.
const TITLE: &[i32] = &[4, 10, 17, 7, 0, 12, 8];
/// States of the lobby class.
const LOBBY: &[i32] = &[13, 6, 15, 16];
/// States of the in-game class.
const GAME: &[i32] = &[18, 3, 9];

/// Whether the state is a loading state.
#[must_use]
pub fn is_loading(state: i32) -> bool {
    LOADING.contains(&state)
}

/// Whether the state is a map-rebuild state.
#[must_use]
pub fn is_rebuild(state: i32) -> bool {
    REBUILD.contains(&state)
}

/// Whether the state is a title state.
#[must_use]
pub fn is_title(state: i32) -> bool {
    TITLE.contains(&state)
}

/// Whether the state is a lobby state.
#[must_use]
pub fn is_lobby(state: i32) -> bool {
    LOBBY.contains(&state)
}

/// Whether the state is an in-game state.
#[must_use]
pub fn is_game(state: i32) -> bool {
    GAME.contains(&state)
}
