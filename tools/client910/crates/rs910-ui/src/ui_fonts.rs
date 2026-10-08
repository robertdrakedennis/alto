//! Shared CPU font resources and original cache lifetime.
//! Atlas upload is a renderer boundary; metrics and inline icon dimensions are
//! also available to scripts without creating a second GPU device.
use crate::ui_cache::Cache;
use crate::{
    cache::{CacheError, Pack},
    font_layout::{Image, Images},
    font_metrics::Metrics,
    sprite_sheet::SpriteSheet,
};
use anyhow::{Context, Result};
pub use rs910_toolkit::font::*;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Archive {
    Sprites,
    Metrics,
}
impl Archive {
    fn name(self) -> &'static str {
        match self {
            Self::Sprites => "sprites",
            Self::Metrics => "fontmetrics",
        }
    }
}
pub trait Source {
    fn load(&mut self, archive: Archive, id: i32) -> Result<bool>;
    fn fetch(&mut self, archive: Archive, id: i32) -> Result<Option<Vec<u8>>>;
    fn sprite_file(&mut self, id: i32) -> Result<Option<Vec<u8>>>;
}
struct PackSource {
    pack: Pack,
    bytes: BTreeMap<(Archive, i32), Vec<u8>>,
}
impl PackSource {
    fn read(&mut self, a: Archive, id: i32) -> Result<Option<Vec<u8>>> {
        if let Some(b) = self.bytes.get(&(a, id)) {
            return Ok(Some(b.clone()));
        }
        let Ok(group) = u32::try_from(id) else {
            return Ok(None);
        };
        let bytes = match self.pack.read_group(a.name(), group) {
            Ok(mut g) => g.remove(&0),
            Err(CacheError::GroupMissing { .. } | CacheError::UnknownGroup { .. }) => None,
            Err(e) => return Err(e.into()),
        };
        // Missing files remain retryable during resource loading.
        if let Some(b) = &bytes {
            self.bytes.insert((a, id), b.clone());
        }
        Ok(bytes)
    }
}
impl Source for PackSource {
    fn load(&mut self, a: Archive, id: i32) -> Result<bool> {
        if self.bytes.contains_key(&(a, id)) {
            return Ok(true);
        }
        Ok(self.read(a, id)?.is_some())
    }
    fn fetch(&mut self, a: Archive, id: i32) -> Result<Option<Vec<u8>>> {
        self.read(a, id)
    }
    fn sprite_file(&mut self, id: i32) -> Result<Option<Vec<u8>>> {
        self.read(Archive::Sprites, id)
    }
}

/// One font-icon array identity, shared by layout and painting.
/// Dimensions and upload-ready pixels are created together on cache miss.
pub struct Icons {
    pub dimensions: Rc<Vec<Image>>,
    pub sprites: Vec<Rc<crate::ui_sprites::Sprite>>,
}
pub struct Fonts {
    source: RefCell<Box<dyn Source>>,
    pub ids: Option<Vec<i32>>,
    defaults: RefCell<Option<BTreeMap<usize, Rc<Font>>>>,
    metrics: RefCell<Cache<Metrics>>,
    fonts: RefCell<Cache<Font>>,
    icons: RefCell<Option<Cache<Icons>>>,
    toolkit: RefCell<Option<u64>>,
    pub images: Images,
    inline_id: Option<i32>,
    pub inline_sprites: Option<Vec<Rc<crate::ui_sprites::Sprite>>>,
}
impl Fonts {
    pub fn new(source: Box<dyn Source>, ids: Option<Vec<i32>>, toolkit: Option<u64>) -> Self {
        Self {
            source: RefCell::new(source),
            ids,
            defaults: RefCell::new(None),
            metrics: RefCell::new(Cache::default()),
            fonts: RefCell::new(Cache::default()),
            icons: RefCell::new(Some(Cache::default())),
            toolkit: RefCell::new(toolkit),
            images: Images::default(),
            inline_id: None,
            inline_sprites: None,
        }
    }
    /// Built from the default sprites. Recolouring font icons does not affect
    /// dimensions. The graphics consumer supplies a toolkit generation token.
    pub fn from_pack(pack: Pack, toolkit: Option<u64>) -> Result<Self> {
        pack.read_archive_index("fontmetrics")?;
        pack.read_archive_index("sprites")?;
        let defaults = crate::protocol910::defaults::Graphics::decode(
            &crate::js5_fetch::fetch_file(&pack, "defaults", 3)?
                .context("graphics defaults missing")?,
        )
        .map_err(|e| anyhow::anyhow!("graphics defaults: {e:?}"))?
        .value
        .scalars;
        let mut fonts = Self::new(
            Box::new(PackSource {
                pack,
                bytes: BTreeMap::new(),
            }),
            Some(vec![
                defaults.p11_full,
                defaults.p12_full,
                defaults.b12_full,
            ]),
            toolkit,
        );
        fonts.load_fonts()?;
        let bytes = fonts
            .source
            .borrow_mut()
            .sprite_file(defaults.font_icons)?
            .context("default font icons missing")?;
        fonts.images.images = Some(sprite_dimensions(&bytes)?);
        fonts.inline_id = Some(defaults.font_icons);
        Ok(fonts)
    }
    /// The loading owner supplies the random samples; every RGB sample is
    /// consumed before each sprite is created.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "cache-control operation kept with its owner; the frame schedule drives `clean` and `clear_soft` (tested)"
        )
    )]
    pub fn load_inline_sprites(&mut self, random: &mut dyn FnMut() -> f64) -> Result<()> {
        let id = self
            .inline_id
            .context("default inline sprite id not installed")?;
        let bytes = self
            .source
            .borrow_mut()
            .sprite_file(id)?
            .context("default font icons missing")?;
        let mut data = crate::sprite_data::Data::decode(&bytes)?;
        let mut sprites = vec![];
        for d in &mut data {
            let shifts = std::array::from_fn::<_, 3, _>(|_| {
                (-12i32).wrapping_add((random() * 12.0 * 2.0) as i32)
            });
            d.recolour(shifts);
            sprites.push(Rc::new(crate::ui_sprites::Sprite::new(d)?));
        }
        self.images.images = Some(
            sprites
                .iter()
                .map(|s| {
                    let [width, height] = s.full_size();
                    Image { width, height }
                })
                .collect(),
        );
        self.inline_sprites = Some(sprites);
        Ok(())
    }
    pub fn load_fonts(&self) -> Result<()> {
        let ids = self.ids.as_ref().context("null default font ids")?;
        *self.defaults.borrow_mut() = Some(BTreeMap::new());
        for (index, &id) in ids.iter().enumerate() {
            let metrics = self.decode_metrics(id)?;
            let bytes = self.source.borrow_mut().fetch(Archive::Sprites, id)?;
            let font = Font::create(bytes, metrics, true)?;
            self.defaults
                .borrow_mut()
                .as_mut()
                .unwrap()
                .insert(index, font);
        }
        Ok(())
    }
    /// Loads a file from the provider's own archives (the loading-screen
    /// element readiness checks share the loading font archives).
    pub fn source_load(&self, archive: Archive, id: i32) -> Result<bool> {
        self.source.borrow_mut().load(archive, id)
    }
    /// `Js5.fetchFile` on the provider's own archives.
    pub fn source_fetch(&self, archive: Archive, id: i32) -> Result<Option<Vec<u8>>> {
        self.source.borrow_mut().fetch(archive, id)
    }
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "cache-control operation kept with its owner; the frame schedule drives `clean` and `clear_soft` (tested)"
        )
    )]
    pub fn clear_fonts(&self) {
        *self.defaults.borrow_mut() = None;
    }
    pub fn count(&self) -> i32 {
        self.ids
            .as_ref()
            .map_or(0, |v| (v.len() as i32).wrapping_mul(2))
    }
    pub fn loaded_count(&self, force: bool) -> Result<i32> {
        let Some(ids) = &self.ids else {
            return Ok(0);
        };
        if !force && self.defaults.borrow().is_some() {
            return Ok(self.count());
        }
        let mut n = 0i32;
        for &id in ids {
            for a in [Archive::Sprites, Archive::Metrics] {
                if self.source.borrow_mut().load(a, id)? {
                    n = n.wrapping_add(1);
                }
            }
        }
        Ok(n)
    }
    fn default(&self, id: i32) -> Option<Result<Rc<Font>>> {
        self.ids.as_ref()?.iter().position(|v| *v == id).map(|i| {
            self.defaults
                .borrow()
                .as_ref()
                .and_then(|m| m.get(&i))
                .cloned()
                .context("default font not loaded")
        })
    }
    fn decode_metrics(&self, id: i32) -> Result<Option<Rc<Metrics>>> {
        self.source
            .borrow_mut()
            .fetch(Archive::Metrics, id)?
            .map(|b| Metrics::decode(&b).map(Rc::new))
            .transpose()
    }
    pub fn get_metrics(
        &self,
        id: i32,
        cache: bool,
        request_sprites: bool,
    ) -> Result<Option<Rc<Metrics>>> {
        if id == -1 {
            return Ok(None);
        }
        if let Some(font) = self.default(id) {
            return Ok(Some(font?.metrics.clone()));
        }
        if request_sprites {
            self.source.borrow_mut().load(Archive::Sprites, id)?;
        }
        if let Some(m) = self.metrics.borrow_mut().get(id as i64) {
            return Ok(Some(m));
        }
        let m = self.decode_metrics(id)?;
        if cache {
            if let Some(m) = &m {
                self.metrics.borrow_mut().put(id as i64, m.clone())?;
            }
        }
        Ok(m)
    }
    pub fn get_font(&self, id: i32, cache_metrics: bool, mono: bool) -> Result<Option<Rc<Font>>> {
        if id == -1 {
            return Ok(None);
        }
        if let Some(font) = self.default(id) {
            return font.map(Some);
        }
        let key = (id.wrapping_shl(1) | mono as i32) as i64;
        if let Some(f) = self.fonts.borrow_mut().get(key) {
            return Ok(Some(f));
        }
        let bytes = self.source.borrow_mut().fetch(Archive::Sprites, id)?;
        if bytes.is_none() {
            return Ok(None);
        }
        let metrics = self.get_metrics(id, cache_metrics, false)?;
        if metrics.is_none() {
            return Ok(None);
        }
        let f = Font::create(bytes, metrics, mono)?;
        self.fonts.borrow_mut().put(key, f.clone())?;
        Ok(Some(f))
    }
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "cache-control operation kept with its owner; the frame schedule drives `clean` and `clear_soft` (tested)"
        )
    )]
    pub fn reset(&self) {
        *self.metrics.borrow_mut() = Cache::default();
        *self.fonts.borrow_mut() = Cache::default();
        if let Some(c) = self.icons.borrow_mut().as_mut() {
            *c = Cache::default();
        }
    }
    pub fn clean(&self, age: i32) {
        self.metrics.borrow_mut().clean(age);
        self.fonts.borrow_mut().clean(age);
        if let Some(c) = self.icons.borrow_mut().as_mut() {
            c.clean(age);
        }
    }
    pub fn clear_soft(&self) {
        self.metrics.borrow_mut().clear_soft();
        self.fonts.borrow_mut().clear_soft();
        if let Some(c) = self.icons.borrow_mut().as_mut() {
            c.clear_soft();
        }
    }
    /// Clear the values of the soft entries but keep their nodes for the
    /// next get or clean. Memory pressure drops them outright
    /// ([`Fonts::clear_soft`]); this is the explicit form.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "cache-control operation kept with its owner; the frame schedule drives `clean` and `clear_soft` (tested)"
        )
    )]
    pub fn clear_soft_referents(&self) {
        self.metrics.borrow_mut().clear_soft_referents();
        self.fonts.borrow_mut().clear_soft_referents();
        if let Some(c) = self.icons.borrow_mut().as_mut() {
            c.clear_soft_referents();
        }
    }
    pub fn icon_dimensions(&self, toolkit: Option<u64>, id: i32) -> Result<Option<Rc<Vec<Image>>>> {
        Ok(self
            .icon_sprites(toolkit, id)?
            .map(|v| v.dimensions.clone()))
    }
    /// getIconSprites. Measurement and paint share the
    /// same array/cache: a dimensions-only lookup still creates every sprite.
    pub fn icon_sprites(&self, toolkit: Option<u64>, id: i32) -> Result<Option<Rc<Icons>>> {
        let mut icons = self.icons.borrow_mut();
        let Some(cache) = icons.as_mut() else {
            return Ok(None);
        };
        if let Some(t) = toolkit {
            if *self.toolkit.borrow() != Some(t) {
                *cache = Cache::default();
            }
            *self.toolkit.borrow_mut() = Some(t);
        }
        if self.toolkit.borrow().is_none() {
            return Ok(None);
        }
        if let Some(v) = cache.get(id as i64) {
            return Ok(Some(v));
        }
        let Some(b) = self.source.borrow_mut().sprite_file(id)? else {
            return Ok(None);
        };
        let sprites = crate::sprite_data::Data::decode(&b)?
            .iter()
            .map(|d| crate::ui_sprites::Sprite::new(d).map(Rc::new))
            .collect::<Result<Vec<_>>>()?;
        if sprites.is_empty() {
            return Ok(None);
        }
        let dimensions = Rc::new(
            sprites
                .iter()
                .map(|s| {
                    let [width, height] = s.full_size();
                    Image { width, height }
                })
                .collect(),
        );
        let v = Rc::new(Icons {
            dimensions,
            sprites,
        });
        cache.put(id as i64, v.clone())?;
        Ok(Some(v))
    }
    pub fn icon_width(&self, id: i32) -> Result<i32> {
        Ok(self.icon_dimensions(None, id)?.map_or(0, |v| v[0].width))
    }
    pub fn width(&self, metrics: &Metrics, text: Option<&[u16]>) -> Result<i32> {
        let widths = self
            .images
            .images
            .as_ref()
            .map(|v| v.iter().map(|i| i.width).collect::<Vec<_>>());
        metrics.width_with_provider(
            text.unwrap_or_default(),
            widths.as_deref(),
            Some(&|id| self.icon_width(id)),
        )
    }
    /// FontMetrics-590: paragraph scratch capacity is 100 lines.
    pub fn lines(
        &self,
        metrics: &Metrics,
        text: Option<&[u16]>,
        width: i32,
        drop_space: bool,
    ) -> Result<(Vec<Vec<u16>>, usize)> {
        crate::font_layout::split_with_provider(
            metrics,
            text.unwrap_or_default(),
            Some(&[width]),
            &self.images,
            drop_space,
            100,
            Some(&|id| self.icon_width(id)),
        )
    }
    /// The `[height, width]` of `text` set as a paragraph at most `wrap` pixels wide
    /// in font `id`; `None` while the font is not loaded.
    pub fn paragraph_size(&self, id: i32, text: &str, wrap: i32) -> Option<[i32; 2]> {
        let font = self.get_font(id, true, true).ok().flatten()?;
        let units: Vec<u16> = text.encode_utf16().collect();
        let (lines, count) = self.lines(&font.metrics, Some(&units), wrap, true).ok()?;
        let mut width = 0;
        for line in &lines[..count] {
            width = width.max(self.width(&font.metrics, Some(line)).ok()?);
        }
        let m = &font.metrics;
        Some([
            m.descent + m.ascent + (count as i32 - 1) * m.space_width,
            width,
        ])
    }
    /// FontMetrics. Text remains UTF-16 here, including a line break
    /// inside a surrogate pair; only the current VM adapter has a String limit.
    pub fn measure(
        &self,
        m: &Metrics,
        name: &str,
        text: Option<&[u16]>,
        args: &[i32],
    ) -> Result<Measurement> {
        if name == "stringwidth" {
            return Ok(Measurement::Int(self.width(m, text)?));
        }
        let (lines, n) = self.lines(m, text, args[0], true)?;
        Ok(match name {
            "paraheight" => Measurement::Int(n as i32),
            "parawidth" => {
                let mut w = 0;
                for line in &lines[..n] {
                    w = w.max(self.width(m, Some(line))?);
                }
                Measurement::Int(w)
            }
            "paraline" => Measurement::Text(if args[2] >= 0 && (args[2] as usize) < n {
                Some(lines[args[2] as usize].clone())
            } else {
                None
            }),
            _ => anyhow::bail!("unknown font command {name}"),
        })
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn snapshot(&self) -> Vec<Vec<(i64, bool, i64, bool)>> {
        vec![
            self.metrics.borrow().snapshot(),
            self.fonts.borrow().snapshot(),
            self.icons
                .borrow()
                .as_ref()
                .map_or_else(Vec::new, Cache::snapshot),
        ]
    }
}

/// Sprite dimensions as the toolkit reports them: cropped zero-size sprites
/// allocate a 1x1 texture before restoring padding; the position includes
/// padding.
pub fn sprite_dimensions(bytes: &[u8]) -> Result<Vec<Image>> {
    // The sprite data provider permits an empty sprite array. The
    // general sheet consumer requires a first frame; icon lookup does not.
    if bytes.len() >= 2 {
        let tail = u16::from_be_bytes([bytes[bytes.len() - 2], bytes[bytes.len() - 1]]);
        if tail & 0x7fff == 0 {
            if tail >> 15 == 0 {
                anyhow::ensure!(bytes.len() >= 7, "empty sprite metadata EOF");
                anyhow::ensure!(
                    bytes.len() >= 7 + bytes[bytes.len() - 3] as usize * 3,
                    "empty sprite palette EOF"
                );
            } else {
                anyhow::ensure!(
                    bytes.len() >= 6 && bytes[0] == 0,
                    "empty full sprite header"
                );
            }
            return Ok(vec![]);
        }
    }
    match SpriteSheet::decode(bytes)? {
        SpriteSheet::Paletted {
            canvas_w,
            canvas_h,
            sprites,
            ..
        } => Ok(sprites
            .iter()
            .map(|s| {
                let empty = s.width == 0 || s.height == 0;
                Image {
                    width: canvas_w as i32 + if empty { 1 - s.width as i32 } else { 0 },
                    height: canvas_h as i32 + if empty { 1 - s.height as i32 } else { 0 },
                }
            })
            .collect()),
        SpriteSheet::Full {
            width,
            height,
            sprites,
            ..
        } => Ok(vec![
            Image {
                width: if width == 0 || height == 0 {
                    1
                } else {
                    width as i32
                },
                height: if width == 0 || height == 0 {
                    1
                } else {
                    height as i32
                }
            };
            sprites.len()
        ]),
    }
}

#[derive(Debug)]
pub enum Measurement {
    Int(i32),
    Text(Option<Vec<u16>>),
}
impl Measurement {
    fn value(self) -> Result<native910::vm::Value> {
        Ok(match self {
            Self::Int(v) => native910::vm::Value::Int(v),
            // Isolated surrogates stay single code units (native910::jstr).
            Self::Text(s) => {
                native910::vm::Value::Str(native910::jstr::from_units(&s.unwrap_or_default()))
            }
        })
    }
}

pub fn dispatch(
    fonts: Option<&Fonts>,
    name: &str,
    ints: &mut Vec<i32>,
    strs: &mut Vec<String>,
) -> Option<Result<Option<native910::vm::Value>>> {
    let count = match name {
        "stringwidth" => 1,
        "paraheight" | "parawidth" => 2,
        "paraline" => 3,
        _ => return None,
    };
    Some((|| {
        let text = strs.pop().context("font string stack underflow")?;
        anyhow::ensure!(ints.len() >= count, "font integer stack underflow");
        let args = ints.split_off(ints.len() - count);
        let fonts = fonts.context("font provider not installed")?;
        let id = args[if name == "stringwidth" { 0 } else { 1 }];
        // The lookup precedes the output-stack increment.
        let metrics = fonts.get_metrics(id, true, true)?;
        let result = (|| {
            let metrics = metrics.context("missing script font metrics")?;
            fonts.measure(&metrics, name, Some(&native910::jstr::units(&text)), &args)
        })();
        // The original client increments isp before evaluating the measurement on the RHS.
        // On failure the old first argument still occupies that output slot.
        if result.is_err() && name != "paraline" {
            ints.push(args[0]);
        }
        result?.value().map(Some)
    })())
}

impl crate::font_layout::IconProvider for Fonts {
    fn width(&self, id: i32) -> Result<i32> {
        self.icon_width(id)
    }
    fn frames(&self, id: i32) -> Result<Option<Rc<Vec<Image>>>> {
        self.icon_dimensions(None, id)
    }
}
