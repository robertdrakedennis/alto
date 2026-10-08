//! The client's own texts follow the launcher's language (applet parameter 26).
//! This binary installs German for its one test, so it runs in its own process.
use rs910_core::ui_text_compare::Language;

#[test]
fn menu_item_and_social_texts_follow_the_launcher_language() {
    rs910_core::applet_params::install(&["26=1".to_string()]).expect("launcher parameters");
    assert_eq!(rs910_ui::loading::client_language(), Language::De);
    // The menu every client starts with offers the cancel option in German.
    let mut menu = rs910_ui::ui_minimenu::MiniMenu::default();
    menu.reset();
    let ops: Vec<String> = menu
        .entries
        .iter()
        .map(|&entry| menu.entry(entry).op.clone())
        .collect();
    assert_eq!(ops, ["Abbrechen"]);
    // Items start with the language's take and drop operations.
    let ground = rs910_config::config::obj_default_ops();
    let backpack = rs910_config::config::obj_default_iops();
    assert_eq!(ground[2].as_deref(), Some("Nehmen"));
    assert_eq!(backpack[4].as_deref(), Some("Fallen lassen"));
    // Friend notices and refusals.
    assert_eq!(
        rs910_ui::ui_social::SocialText::FriendLogin.text(),
        rs910_core::texts::Msg::FriendLogin
            .for_lang(Language::De)
            .unwrap()
    );
    assert_ne!(
        rs910_ui::ui_social::SocialText::FriendLogin.text(),
        " has logged in."
    );
}
