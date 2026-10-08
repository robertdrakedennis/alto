use super::*;
use crate::login_state::{LOBBY_LOGIN, LOBBY_RELOGIN, LOGIN_GAME};

/// A social sign-on's key belongs to the network it was given for: picking the
/// same network again keeps it, another network drops it, and a login by
/// username and password forgets the sign-on.
#[test]
fn the_social_key_belongs_to_its_network() {
    let mut sso = SsoState::default();
    assert!(sso.login().is_none());
    sso.choose(6);
    assert_eq!(sso.login().unwrap().social_key, None, "not signed on yet");
    sso.learn(0x5A00_0000_0000_0006, 77);
    sso.choose(6);
    let login = sso.login().unwrap();
    assert_eq!(
        (login.network, login.social_key, login.social_name),
        (6, Some(0x5A00_0000_0000_0006), 77)
    );
    sso.choose(2);
    assert_eq!(sso.login().unwrap().social_key, None);
    sso.learn(5, 9);
    sso.clear();
    assert_eq!(sso, SsoState::default());
}

/// What the login worker reports reaches the login screens: the reply that
/// keeps the login running goes to the lobby or the world reply by the
/// running login, the queue position is copied, and the pages the server
/// named are queued to open. Nothing is published to a login that is not
/// running.
#[test]
fn the_login_worker_reports_to_the_login_screens() {
    let progress = crate::net::LoginProgress::new();
    progress.set_interim_reply(49);
    progress.set_queue_position(12);
    progress.push_url("https://example.test/validate".into());
    let mut engine = crate::ui_runtime::Engine::default();
    publish_login_progress(&progress, LOBBY_LOGIN, &mut engine);
    assert_eq!(
        (engine.login.reply, engine.login.lobby_reply),
        (-2, -2),
        "not running"
    );
    assert_eq!(engine.login.queue_position, 12);
    // The page was taken by that publish; a running login gets the reply.
    engine.login.in_progress = true;
    publish_login_progress(&progress, LOBBY_RELOGIN, &mut engine);
    assert_eq!((engine.login.reply, engine.login.lobby_reply), (-2, 49));
    progress.set_interim_reply(42);
    publish_login_progress(&progress, LOGIN_GAME, &mut engine);
    assert_eq!((engine.login.reply, engine.login.lobby_reply), (42, 49));
    // The first publish queued the one page.
    let mut engine = crate::ui_runtime::Engine::default();
    let progress = crate::net::LoginProgress::new();
    progress.push_url("https://example.test/validate".into());
    publish_login_progress(&progress, LOBBY_LOGIN, &mut engine);
    assert!(matches!(
        engine.effects.browser_urls.as_slice(),
        [crate::session::UiEvent::UrlOpen { primary, .. }] if primary == "https://example.test/validate"
    ));
}
