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

pub fn parse_argument(argument: &str) -> Result<Option<RenderScale>, &'static str> {
    const USAGE: &str = "Usage: renderscale auto|50..200";
    match argument.trim() {
        "auto" => Ok(None),
        other => RenderScale::parse(other).map(Some).ok_or(USAGE),
    }
}
