//! The retained UI's string type: a string as its UTF-16 code units
//! (component text, script strings, param/enum/struct values). Split out of
//! client910's `ui_components` in Phase 3.2 so the config lists
//! (`ui_configs`) can name it below the UI crate.
pub type Text = Vec<u16>;
