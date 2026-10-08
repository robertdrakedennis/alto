//! Loading-screen font and sprite resources and asynchronous news loading.

use crate::{
    cache::Pack,
    ui_fonts::{Archive, Font, Fonts, Source},
    ui_sprites::Sprite,
};

use anyhow::{Context as _, Result};

use rs910_core::fault::Fault;

use std::{collections::BTreeMap, rc::Rc};

use super::{decode_loading_image, loading_sprites_archive, loading_sprites_raw};

/// Group loads/fetches over the loading sprites + font metrics archives,
/// feeding the font provider.
pub(super) struct LoadingSource {
    pack: Pack,
    bytes: BTreeMap<(Archive, i32), Vec<u8>>,
}

impl LoadingSource {
    pub(super) fn read(&mut self, archive: Archive, id: i32) -> Result<Option<Vec<u8>>> {
        if let Some(bytes) = self.bytes.get(&(archive, id)) {
            return Ok(Some(bytes.clone()));
        }
        let Ok(group) = u32::try_from(id) else {
            return Ok(None);
        };
        let name = match archive {
            Archive::Sprites => loading_sprites_archive(),
            Archive::Metrics => "fontmetrics",
        };
        let bytes = match self.pack.read_group(name, group) {
            Ok(mut files) => files.remove(&0),
            Err(
                crate::cache::CacheError::GroupMissing { .. }
                | crate::cache::CacheError::UnknownGroup { .. },
            ) => None,
            Err(error) => return Err(error.into()),
        };
        if let Some(bytes) = &bytes {
            self.bytes.insert((archive, id), bytes.clone());
        }
        Ok(bytes)
    }
}

impl Source for LoadingSource {
    fn load(&mut self, archive: Archive, id: i32) -> Result<bool> {
        Ok(self.read(archive, id)?.is_some())
    }
    fn fetch(&mut self, archive: Archive, id: i32) -> Result<Option<Vec<u8>>> {
        self.read(archive, id)
    }
    fn sprite_file(&mut self, id: i32) -> Result<Option<Vec<u8>>> {
        self.read(Archive::Sprites, id)
    }
}

/// The element resources: the loading sprites and font metrics archives, the
/// loading font provider and the created-sprite cache.
pub struct Resources {
    pub fonts: Fonts,
    /// The same archives without default-font slots: the `b12_full` font
    /// the pre-loading bar and the news display draw with before
    /// the font provider's `load_fonts` (stage 4) fills the default slots.
    pub(super) plain: Fonts,
    sprites: BTreeMap<i32, Rc<Sprite>>,
    news: Option<Rc<std::cell::RefCell<NewsManager>>>,
    /// World host and node for the news fetch.
    news_host: (String, i32),
}

impl Resources {
    /// Font provider over the loading archives: `default_fonts` are the
    /// graphics-defaults p11/p12/b12 ids once stage 1 decoded them.
    #[must_use]
    pub fn new(pack: Pack, default_fonts: Option<Vec<i32>>, news_host: (String, i32)) -> Self {
        Self {
            fonts: Fonts::new(
                Box::new(LoadingSource {
                    pack: pack.clone(),
                    bytes: BTreeMap::new(),
                }),
                default_fonts,
                None,
            ),
            plain: Fonts::new(
                Box::new(LoadingSource {
                    pack,
                    bytes: BTreeMap::new(),
                }),
                None,
                None,
            ),
            sprites: BTreeMap::new(),
            news: None,
            news_host,
        }
    }

    /// Whether the loading sprite `id` is loaded.
    pub(super) fn sprite_ready(&mut self, id: i32) -> Result<bool> {
        self.fonts.source_load(Archive::Sprites, id)
    }

    /// Whether both the sprite and the metrics of font `id` are loaded.
    pub(super) fn font_ready(&mut self, id: i32) -> Result<bool> {
        let sprites = self.sprite_ready(id)?;
        let metrics = self.fonts.source_load(Archive::Metrics, id)?;
        Ok(sprites && metrics)
    }

    /// The sprite `id`, cached: decoded from the raw archive, or from the
    /// JPEG file when the platform image decoder works.
    pub(super) fn sprite(&mut self, id: i32) -> Result<Rc<Sprite>> {
        if let Some(sprite) = self.sprites.get(&id) {
            return Ok(sprite.clone());
        }
        let bytes = self
            .fonts
            .source_fetch(Archive::Sprites, id)?
            .with_context(|| format!("loading sprite {id} missing"))?;
        let sprite = if loading_sprites_raw() {
            let data = crate::sprite_data::Data::decode(&bytes)?
                .into_iter()
                .next()
                .context("loading sprite has no frames")?;
            Rc::new(Sprite::new(&data)?)
        } else {
            Rc::new(decode_loading_image(&bytes)?)
        };
        self.sprites.insert(id, sprite.clone());
        Ok(sprite)
    }

    /// The loading font `id`: metrics archive plus sprite archive.
    pub(super) fn font(&mut self, id: i32) -> Result<Rc<Font>> {
        self.fonts
            .get_font(id, false, true)?
            .with_context(|| Fault::MissingValue.message(format!("loading font {id}")))
    }

    /// The shared news fetcher, created on first use.
    pub(super) fn news(&mut self) -> Rc<std::cell::RefCell<NewsManager>> {
        self.news
            .get_or_insert_with(|| {
                Rc::new(std::cell::RefCell::new(NewsManager::new(
                    self.news_host.0.clone(),
                    self.news_host.1,
                )))
            })
            .clone()
    }

    /// Drops the created sprites.
    pub fn reset_sprites(&mut self) {
        self.sprites.clear();
    }
}

// ---------------------------------------------------------------------------
// News fetch
// ---------------------------------------------------------------------------

/// One background fetch of `http://<world host>:<port>/news.ws?game=<mode>`,
/// three lines per entry. Any I/O failure leaves the entry list empty.
pub struct NewsManager {
    host: String,
    node: i32,
    entries: std::sync::Arc<std::sync::Mutex<Option<Vec<[String; 3]>>>>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    started: bool,
}

impl NewsManager {
    pub(super) fn new(host: String, node: i32) -> Self {
        Self {
            host,
            node,
            entries: Default::default(),
            done: Default::default(),
            started: false,
        }
    }

    /// Starts the fetch thread once; ready when it is done.
    pub(super) fn ready(&mut self) -> bool {
        use std::sync::atomic::Ordering;
        if self.done.load(Ordering::Acquire) {
            return true;
        }
        if !self.started {
            self.started = true;
            // Live worlds use port 80, others `node + 7000`.
            let live = crate::applet_params::get().mode_where().ok().flatten() == Some(0);
            let port = if live { 80 } else { self.node + 7000 };
            let game = crate::applet_params::get()
                .mode_game()
                .ok()
                .flatten()
                .map_or(0, |g| i32::from(g != "runescape"));
            let host = self.host.clone();
            let entries = self.entries.clone();
            let done = self.done.clone();
            let _ = std::thread::Builder::new()
                .name("client910-news".into())
                .spawn(move || {
                    if let Some(lines) = fetch_news(&host, port, game) {
                        if lines.len() % 3 == 0 {
                            *entries.lock().unwrap() = Some(
                                lines
                                    .chunks_exact(3)
                                    .map(|c| [c[0].clone(), c[1].clone(), c[2].clone()])
                                    .collect(),
                            );
                        } else {
                            // A line count that is no multiple of 3 leaves the fetch unfinished.
                            return;
                        }
                    }
                    done.store(true, Ordering::Release);
                });
        }
        self.done.load(Ordering::Acquire)
    }

    /// The news entry at `index`, once fetched.
    pub(super) fn entry(&self, index: i32) -> Option<[String; 3]> {
        let entries = self.entries.lock().unwrap();
        usize::try_from(index)
            .ok()
            .and_then(|i| entries.as_ref()?.get(i).cloned())
    }
}

/// The HTTP GET and line-reading loop of the news fetch.
pub(super) fn fetch_news(host: &str, port: i32, game: i32) -> Option<Vec<String>> {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect((host, u16::try_from(port).ok()?)).ok()?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .ok()?;
    write!(
        stream,
        "GET /news.ws?game={game} HTTP/1.0\r\nHost: {host}:{port}\r\n\r\n"
    )
    .ok()?;
    let mut body = Vec::new();
    stream.read_to_end(&mut body).ok()?;
    let text = String::from_utf8_lossy(&body);
    let (head, content) = text.split_once("\r\n\r\n")?;
    if head
        .split_whitespace()
        .nth(1)
        .is_none_or(|code| code != "200")
    {
        return None;
    }
    Some(content.lines().map(str::to_owned).collect())
}
