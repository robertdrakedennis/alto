//! Cursor definitions (config group 33) and the custom-cursor update.
//! Selection and cache pixels stay independent of the native window boundary.
use crate::{
    cache::{CacheError, Pack},
    sprite_data::Data,
    ui_bytes::Cursor,
    ui_cache::Cache,
};
use anyhow::{Context, Result};
use std::{collections::BTreeMap, rc::Rc};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Definition {
    pub graphic: i32,
    pub hotspot: [i32; 2],
}
impl Definition {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        let mut out = Self::default();
        loop {
            match c.g1()? {
                0 => return Ok(out),
                1 => out.graphic = c.gsmart2or4s()?,
                2 => out.hotspot = [i32::from(c.g1()?), i32::from(c.g1()?)],
                // Unknown opcodes carry no operand and are ignored.
                _ => {}
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub graphic: i32,
    pub size: [i32; 2],
    pub hotspot: [i32; 2],
    pub argb: Vec<i32>,
}
impl Image {
    pub fn new(def: &Definition, data: &Data) -> Result<Self> {
        Ok(Self {
            graphic: def.graphic,
            size: data.full_size(),
            hotspot: def.hotspot,
            argb: data.argb(true)?,
        })
    }
    pub fn rgba(&self) -> Vec<u8> {
        self.argb
            .iter()
            .flat_map(|&p| {
                [
                    (p >> 16) as u8,
                    (p >> 8) as u8,
                    p as u8,
                    (p as u32 >> 24) as u8,
                ]
            })
            .collect()
    }
}
pub trait Source {
    fn image(&mut self, id: i32) -> Result<Option<Image>>;
}
pub struct Resources {
    pack: Pack,
    raw: BTreeMap<u32, Vec<u8>>,
    definitions: Cache<Definition>,
    sprites: Cache<Data>,
}
impl Resources {
    pub fn new(pack: Pack) -> Result<Self> {
        let raw = pack.read_group("config", 33)?;
        Ok(Self {
            pack,
            raw,
            definitions: Cache::new(64),
            sprites: Cache::new(2),
        })
    }
    pub fn definition(&mut self, id: i32) -> Result<Rc<Definition>> {
        if let Some(value) = self.definitions.get(id as i64) {
            return Ok(value);
        }
        // A missing file yields a default definition.
        let value = Rc::new(match self.raw.get(&(id as u32)) {
            Some(raw) => Definition::decode(raw)?,
            None => Definition::default(),
        });
        self.definitions.put(id as i64, value.clone())?;
        Ok(value)
    }
}
impl Source for Resources {
    fn image(&mut self, id: i32) -> Result<Option<Image>> {
        let def = self.definition(id)?;
        let data = if let Some(data) = self.sprites.get(def.graphic as i64) {
            data
        } else {
            let Ok(graphic) = u32::try_from(def.graphic) else {
                return Ok(None);
            };
            let bytes = match self.pack.read_group("sprites", graphic) {
                Ok(mut group) => group.remove(&0),
                Err(CacheError::GroupMissing { .. } | CacheError::UnknownGroup { .. }) => None,
                Err(e) => return Err(e.into()),
            };
            let Some(bytes) = bytes else { return Ok(None) };
            let data = Rc::new(
                Data::decode(&bytes)?
                    .into_iter()
                    .next()
                    .context("cursor sprite has no frame0")?,
            );
            self.sprites.put(def.graphic as i64, data.clone())?;
            data
        };
        Ok(Some(Image::new(&def, &data)?))
    }
}
pub trait Sink {
    fn custom(&mut self, id: i32, image: &Image) -> Result<()>;
    fn system(&mut self) -> Result<()>;
}
pub struct State {
    pub current: i32,
}
impl Default for State {
    fn default() -> Self {
        Self { current: -1 }
    }
}
impl State {
    /// Identical IDs avoid loads; missing sprites restore
    /// the system cursor. Record a new ID only after the native call succeeds.
    pub fn update(
        &mut self,
        requested: i32,
        enabled: bool,
        source: &mut impl Source,
        sink: &mut impl Sink,
    ) -> Result<()> {
        let mut wanted = if enabled { requested } else { -1 };
        if self.current == wanted {
            return Ok(());
        }
        if wanted != -1 {
            if let Some(image) = source.image(wanted)? {
                sink.custom(wanted, &image)?;
                self.current = wanted;
            } else {
                wanted = -1;
            }
        }
        if wanted == -1 && self.current != -1 {
            sink.system()?;
            self.current = -1;
        }
        Ok(())
    }
}
/// The cursor to show: the menu's, else the hovered component's, else the
/// configured default.
pub fn select(menu: i32, component: i32, default: i32) -> i32 {
    if menu != -1 {
        menu
    } else if component != -1 {
        component
    } else {
        default
    }
}
#[cfg(test)]
#[path = "ui_cursor_tests.rs"]
mod tests;
