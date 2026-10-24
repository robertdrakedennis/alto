//! Sprite/mask resources, keys and lifetime.
//! Factory calls are synchronous toolkit boundaries; the default CPU factory
//! produces upload-ready pixels and row masks on the shared renderer's behalf.
use crate::{
    cache::{CacheError, Pack},
    sprite_data::Data,
    ui_cache::Cache,
    ui_component_fields::Fields,
};
use anyhow::{Context, Result};
pub use rs910_model::sprite::*;
use std::rc::Rc;
#[derive(Debug)]
pub struct Mask {
    pub size: [i32; 2],
    pub starts: Vec<i32>,
    pub lengths: Vec<i32>,
    pub graphic: i32,
}
impl Mask {
    pub fn new(data: &Data, graphic: i32) -> Result<Self> {
        let size = data.full_size();
        let rows = usize::try_from(size[1]).context("negative graphic rows")?;
        let mut starts = vec![0; rows];
        let mut lengths = vec![0; rows];
        for y in 0..data.height {
            let mut start = 0;
            let mut end = data.width;
            for x in 0..data.width {
                if data.pixel(x, y)? != 0 {
                    start = x;
                    break;
                }
            }
            for x in (start..data.width).rev() {
                if data.pixel(x, y)? != 0 {
                    end = x + 1;
                    break;
                }
            }
            let row = y.wrapping_add(data.padding[1]) as usize;
            *starts.get_mut(row).context("graphic row index")? =
                start.wrapping_add(data.padding[0]);
            *lengths.get_mut(row).context("graphic row index")? = end.wrapping_sub(start);
        }
        Ok(Self {
            size,
            starts,
            lengths,
            graphic,
        })
    }
    /// Hit testing includes the right endpoint, even for an empty
    /// row. The geometry is based on palette/ARGB values, not alpha coverage.
    #[cfg_attr(not(test), allow(dead_code, reason = "exercised by tests only"))]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        y >= 0
            && (y as usize) < self.starts.len()
            && x >= self.starts[y as usize]
            && x <= self.starts[y as usize].wrapping_add(self.lengths[y as usize])
    }
}
pub trait Source {
    fn file(&mut self, id: i32) -> Result<Option<Vec<u8>>>;
}
struct Disk(Pack);
impl Source for Disk {
    fn file(&mut self, id: i32) -> Result<Option<Vec<u8>>> {
        let Ok(id) = u32::try_from(id) else {
            return Ok(None);
        };
        match self.0.read_group("sprites", id) {
            Ok(mut v) => Ok(v.remove(&0)),
            Err(CacheError::GroupMissing { .. } | CacheError::UnknownGroup { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
pub trait Factory {
    fn sprite(&mut self, data: &Data) -> Result<Rc<Sprite>>;
    fn mask(&mut self, mask: Mask) -> Result<Option<Rc<Mask>>>;
}
pub struct Cpu;
impl Factory for Cpu {
    fn sprite(&mut self, data: &Data) -> Result<Rc<Sprite>> {
        Ok(Rc::new(Sprite::new(data)?))
    }
    fn mask(&mut self, mask: Mask) -> Result<Option<Rc<Mask>>> {
        Ok(Some(Rc::new(mask)))
    }
}
pub struct Resources {
    source: Box<dyn Source>,
    sprites: Cache<Sprite>,
    masks: Cache<Mask>,
}
impl Resources {
    pub fn new(source: Box<dyn Source>) -> Self {
        Self {
            source,
            sprites: Cache::new(6_000_000),
            masks: Cache::new(8),
        }
    }
    /// The remaining and total weight of the sprite cache.
    pub fn sprite_cache_weights(&self) -> [i32; 2] {
        [self.sprites.available(), self.sprites.capacity()]
    }
    pub fn from_pack(pack: Pack) -> Result<Self> {
        pack.read_archive_index("sprites")?;
        Ok(Self::new(Box::new(Disk(pack))))
    }
    fn data(&mut self, id: i32) -> Result<Option<Data>> {
        let Some(bytes) = self.source.file(id)? else {
            return Ok(None);
        };
        let mut all = Data::decode(&bytes)?;
        anyhow::ensure!(!all.is_empty(), "empty sprite array has no first frame");
        Ok(Some(all.remove(0)))
    }
    /// The original client uses addition, not OR, for this key. Signed shifts and overlap of
    /// unusually large property values are observable cache collisions.
    pub fn sprite_key(f: &Fields) -> i64 {
        (f.graphicshadow as i64)
            .wrapping_shl(40)
            .wrapping_add((f.outline as i64).wrapping_shl(36))
            .wrapping_add(f.graphic as i64)
            .wrapping_add((f.alpha as i64) << 35)
            .wrapping_add((f.vflip as i64) << 38)
            .wrapping_add((f.hflip as i64) << 39)
    }
    pub fn sprite(
        &mut self,
        f: &Fields,
        dirty: &mut bool,
        factory: &mut impl Factory,
    ) -> Result<Option<Rc<Sprite>>> {
        *dirty = false;
        let key = Self::sprite_key(f);
        if let Some(sprite) = self.sprites.get(key) {
            return Ok(Some(sprite));
        }
        let Some(mut data) = self.data(f.graphic)? else {
            *dirty = true;
            return Ok(None);
        };
        if f.vflip {
            data.flip(true);
        }
        if f.hflip {
            data.flip(false);
        }
        if f.outline > 0 {
            data.expand(f.outline)?;
        } else if f.graphicshadow != 0 {
            data.expand(1)?;
        }
        if f.outline >= 1 {
            data.outline(1)?;
        }
        if f.outline >= 2 {
            data.outline(0xffffff)?;
        }
        if f.graphicshadow != 0 {
            data.shadow(f.graphicshadow | 0xff000000u32 as i32)?;
        }
        let sprite = factory.sprite(&data)?;
        self.sprites.insert(
            key,
            sprite.clone(),
            sprite.size[0].wrapping_mul(sprite.size[1]).wrapping_mul(4),
        )?;
        Ok(Some(sprite))
    }
    pub fn mask(&mut self, f: &Fields, factory: &mut impl Factory) -> Result<Option<Rc<Mask>>> {
        let key = (f.parentlayer as i64).wrapping_shl(32) | f.id as u32 as i64;
        if let Some(mask) = self.masks.get(key) {
            if mask.graphic == f.graphic {
                return Ok(Some(mask));
            }
            self.masks.remove(key);
        }
        let Some(data) = self.data(f.graphic)? else {
            return Ok(None);
        };
        let mask = Mask::new(&data, f.graphic)?;
        let Some(mask) = factory.mask(mask)? else {
            return Ok(None);
        };
        self.masks.put(key, mask.clone())?;
        Ok(Some(mask))
    }
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "cache-control operation kept with its owner; the frame schedule drives `clean` and `clear_soft` (tested)"
        )
    )]
    pub fn reset(&mut self) {
        self.sprites.reset();
        self.masks.reset();
    }
    pub fn clean(&mut self, age: i32) {
        self.sprites.clean(age);
        self.masks.clean(age);
    }
    pub fn clear_soft(&mut self) {
        self.sprites.clear_soft();
        self.masks.clear_soft();
    }
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "cache-control operation kept with its owner; the frame schedule drives `clean` and `clear_soft` (tested)"
        )
    )]
    pub fn clear_soft_referents(&mut self) {
        self.sprites.clear_soft_referents();
        self.masks.clear_soft_referents();
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn snapshot(&self) -> [(i32, Vec<crate::ui_cache::WeightedEntry>); 2] {
        [
            (self.sprites.available(), self.sprites.weighted_snapshot()),
            (self.masks.available(), self.masks.weighted_snapshot()),
        ]
    }
}
