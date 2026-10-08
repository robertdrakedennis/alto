//! The modern renderer's saved display choice: the render scale a player set
//! with the `renderscale` console command, kept in a small file beside the
//! preferences (`players/modern-renderer.conf`). It is the client's own file:
//! nothing in `ClientOptions`, the preferences or a packet changes with it.
//!
//! The file holds one line, `render_scale=auto` or `render_scale=50`..`200`
//! (percent of the scene viewport's pixels per axis). A missing or unreadable
//! file is automatic ([`rs910_render_modern::settings::RenderScale::auto`]:
//! full resolution on a low-DPI display, about 1600 x 1000 pixels of scene on
//! a high-DPI one). `CLIENT910_MODERN_RENDER_SCALE` overrides the saved
//! choice for a run.

use std::path::{Path, PathBuf};

use rs910_render_modern::settings::RenderScale;

/// The file beside the preferences file.
pub fn path(pack_root: &Path) -> PathBuf {
    rs910_client::client_core::preferences_path(pack_root).with_file_name("modern-renderer.conf")
}

/// The saved scale (`None`: automatic, or no usable file).
pub fn load(path: &Path) -> Option<RenderScale> {
    parse(&std::fs::read_to_string(path).ok()?)
}

/// Save `scale` (`None`: automatic).
pub fn save(path: &Path, scale: Option<RenderScale>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format(scale))?;
    Ok(())
}

fn parse(text: &str) -> Option<RenderScale> {
    let value = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("render_scale="))?;
    RenderScale::parse(value.trim())
}

fn format(scale: Option<RenderScale>) -> String {
    match scale {
        Some(s) => format!("render_scale={}\n", s.as_percent()),
        None => "render_scale=auto\n".to_string(),
    }
}

/// The console command's argument: `auto` or `50`..`200` (a percentage or a
/// factor, `RenderScale::parse`). `Err` holds the usage line.
pub fn parse_argument(argument: &str) -> Result<Option<RenderScale>, &'static str> {
    const USAGE: &str = "Usage: renderscale auto|50..200";
    match argument.trim() {
        "auto" => Ok(None),
        other => RenderScale::parse(other).map(Some).ok_or(USAGE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_saved_choice_round_trips_and_a_bad_file_is_automatic() {
        for scale in [None, RenderScale::percent(65), RenderScale::percent(200)] {
            assert_eq!(parse(&format(scale)), scale);
        }
        assert_eq!(parse("render_scale=auto\n"), None);
        assert_eq!(parse("render_scale=10\n"), None);
        assert_eq!(parse("garbage"), None);
        assert_eq!(parse_argument("75"), Ok(RenderScale::percent(75)));
        assert_eq!(parse_argument("auto"), Ok(None));
        assert!(parse_argument("7").is_err());
    }
}
