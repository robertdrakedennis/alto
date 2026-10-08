//! Atomic persistence for local modern graphics choices. The versioned file
//! sits beside ClientOptions and never changes its codec or network bytes.

use rs910_config::renderer_preferences::{RenderScale, RendererPreferences};
use std::path::{Path, PathBuf};

pub fn path(pack_root: &Path) -> PathBuf {
    rs910_client::client_core::preferences_path(pack_root).with_file_name("modern-renderer.conf")
}

pub fn load_preferences(path: &Path) -> RendererPreferences {
    match read_preferences(path) {
        Ok(preferences) => preferences,
        Err(error) => {
            log::warn!("[client910] load modern graphics preferences: {error:#}");
            RendererPreferences::DEFAULT
        }
    }
}

fn read_preferences(path: &Path) -> anyhow::Result<RendererPreferences> {
    match std::fs::read_to_string(path) {
        Ok(text) => RendererPreferences::decode(&text).map_err(anyhow::Error::msg),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(RendererPreferences::DEFAULT)
        }
        Err(error) => Err(error.into()),
    }
}

pub fn save_preferences(path: &Path, preferences: RendererPreferences) -> anyhow::Result<()> {
    use std::io::Write;
    const FIRST_SAVE: u64 = 0;
    const NEXT_SAVE: u64 = 1;
    static SAVE_SEQUENCE: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(FIRST_SAVE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let sequence = SAVE_SEQUENCE.fetch_add(NEXT_SAVE, std::sync::atomic::Ordering::Relaxed);
    let temporary = path.with_extension(format!("conf.{}.{sequence}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| -> anyhow::Result<()> {
        file.write_all(preferences.encode().as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub fn load(path: &Path) -> Option<RenderScale> {
    load_preferences(path).render_scale
}

/// The legacy console command updates one field, retaining all quality choices.
#[cfg(test)]
fn save(path: &Path, scale: Option<RenderScale>) -> anyhow::Result<()> {
    let mut preferences = read_preferences(path)?;
    preferences.render_scale = scale;
    save_preferences(path, preferences)
}

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
        for value in ["auto", "65", "200"] {
            let text = format!("render_scale={value}\n");
            assert_eq!(
                RendererPreferences::decode(&text).unwrap().render_scale,
                parse_argument(value).unwrap()
            );
        }
        assert!(RendererPreferences::decode("render_scale=10\n").is_err());
        assert!(RendererPreferences::decode("garbage").is_err());
        assert_eq!(parse_argument("auto"), Ok(None));
        assert!(parse_argument("7").is_err());
    }
    #[test]
    fn scale_edits_preserve_saved_quality_and_replace_the_file_atomically() {
        let directory =
            std::env::temp_dir().join(format!("alto-graphics-prefs-{}", std::process::id()));
        let path = directory.join("modern-renderer.conf");
        let mut preferences =
            rs910_config::renderer_preferences::QualityPreset::Performance.preferences();
        save_preferences(&path, preferences).unwrap();
        let scale = RenderScale::parse("75");
        save(&path, scale).unwrap();
        preferences.render_scale = scale;
        assert_eq!(read_preferences(&path).unwrap(), preferences);
        const SAVED_FILES: usize = 1;
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), SAVED_FILES);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
