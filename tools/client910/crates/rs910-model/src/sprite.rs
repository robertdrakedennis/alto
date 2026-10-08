//! A toolkit sprite's CPU pixels, and the paletted data kept when the sprite
//! is created from paletted data. Split out of client910's `ui_sprites` so
//! the scene's minimap and entity overlays name it below the UI;
//! `ui_sprites` re-exports it.

use crate::sprite_data::Data;
use anyhow::Result;
use std::rc::Rc;

#[derive(Debug)]
pub struct Sprite {
    /// The paletted source data behind this sprite, if any: the
    /// sprite keeps the indices and palette when it is created from paletted
    /// data.
    pub paletted: Option<Rc<Paletted>>,
    pub size: [i32; 2],
    pub padding: [i32; 4],
    pub argb: Vec<i32>,
}

/// The unpadded palette indices and the `palette` of paletted sprite data.
#[derive(Debug)]
pub struct Paletted {
    pub indices: Vec<u8>,
    pub palette: Vec<i32>,
}

impl Sprite {
    pub fn new(data: &Data) -> Result<Self> {
        let empty = data.width == 0 || data.height == 0;
        let paletted = match &data.pixels {
            // `isPaletted() && !isTranslucent()` (no surviving alpha plane).
            crate::sprite_data::Pixels::Paletted {
                palette,
                indices,
                alpha: None,
            } if !empty => Some(Rc::new(Paletted {
                indices: indices.clone(),
                palette: palette.borrow().clone(),
            })),
            _ => None,
        };
        Ok(Self {
            paletted,
            size: if empty {
                [1, 1]
            } else {
                [data.width, data.height]
            },
            padding: data.padding,
            argb: if empty { vec![0] } else { data.argb(false)? },
        })
    }
    pub fn full_size(&self) -> [i32; 2] {
        [
            self.padding[0]
                .wrapping_add(self.size[0])
                .wrapping_add(self.padding[2]),
            self.padding[1]
                .wrapping_add(self.size[1])
                .wrapping_add(self.padding[3]),
        ]
    }
}
