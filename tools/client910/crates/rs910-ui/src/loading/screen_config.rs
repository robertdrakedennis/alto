//! Cache loading-screen layouts, element configs and screen-set selection.

use anyhow::{Context as _, Result};

use rs910_core::fault::Fault;

use rs910_core::reader::{Eof, Reader as CoreReader};

// ---------------------------------------------------------------------------
// Packet reader (g1/g2/g2s/g3/g3s/g4s/gSmart2or4s/gjstr; the arithmetic is
// `rs910_core::reader`'s)
// ---------------------------------------------------------------------------

pub(super) struct Packet<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Packet<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    /// One `rs910_core::reader` read at `pos`, all or nothing.
    pub(super) fn read<T>(
        &mut self,
        read: impl FnOnce(&mut CoreReader<'a>) -> std::result::Result<T, Eof>,
    ) -> Result<T> {
        let mut r = CoreReader::at(self.data, self.pos);
        let value = r
            .atomic(read)
            .ok()
            .with_context(|| Fault::IndexOutOfRange.message("loading screen packet"))?;
        self.pos = r.pos();
        Ok(value)
    }
    pub(super) fn g1(&mut self) -> Result<i32> {
        self.read(CoreReader::g1).map(i32::from)
    }
    pub(super) fn g2(&mut self) -> Result<i32> {
        self.read(CoreReader::g2).map(i32::from)
    }
    pub(super) fn g2s(&mut self) -> Result<i32> {
        self.read(CoreReader::g2s).map(i32::from)
    }
    pub(super) fn g3(&mut self) -> Result<i32> {
        self.read(CoreReader::g3).map(|v| v as i32)
    }
    pub(super) fn g3s(&mut self) -> Result<i32> {
        self.read(CoreReader::g3s)
    }
    pub(super) fn g4s(&mut self) -> Result<i32> {
        self.read(CoreReader::g4s)
    }
    pub(super) fn g_smart2or4s(&mut self) -> Result<i32> {
        self.data
            .get(self.pos)
            .with_context(|| Fault::IndexOutOfRange.message("loading screen smart"))?;
        self.read(CoreReader::gsmart2or4s)
    }
    pub(super) fn gjstr(&mut self) -> Result<String> {
        let mut r = CoreReader::at(self.data, self.pos);
        let bytes = r.gjstr_bytes();
        self.pos = r.pos();
        let bytes = bytes
            .ok()
            .with_context(|| Fault::IndexOutOfRange.message("loading screen packet"))?;
        Ok(crate::iface::decode_cp1252(bytes))
    }
}

// ---------------------------------------------------------------------------
// Loading-screen configs
// ---------------------------------------------------------------------------

/// Horizontal / vertical placement of an element: left/top, centre,
/// right/bottom, in encoding order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Centre,
    End,
}

impl Align {
    pub(super) fn decode(p: &mut Packet<'_>) -> Result<Self> {
        Ok(match p.g1()? {
            0 => Self::Start,
            1 => Self::Centre,
            2 => Self::End,
            v => anyhow::bail!(Fault::IndexOutOfRange.message(format!("screen alignment {v}"))),
        })
    }
    /// Position along one axis: the larger of the canvas and the frame size
    /// is the reference extent.
    #[must_use]
    pub fn compute(self, size: i32, canvas: i32, frame: i32) -> i32 {
        let extent = if canvas > frame { canvas } else { frame };
        match self {
            Self::Start => 0,
            Self::End => extent - size,
            Self::Centre => (extent - size) / 2,
        }
    }
    /// Alignment for a message box setup command's index argument.
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        [Self::Start, Self::Centre, Self::End].get(index).copied()
    }
}

/// Shared header of every progress-bar config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgressConfig {
    pub halign: Align,
    pub valign: Align,
    /// Offset from the aligned position.
    pub offset: [i32; 2],
    /// Width, height.
    pub size: [i32; 2],
    /// Text baseline shift.
    pub text_offset: i32,
    /// Font id in the loading sprites + font metrics archives.
    pub font: i32,
    /// Text colour.
    pub colour: i32,
}

impl ProgressConfig {
    pub(super) fn decode(p: &mut Packet<'_>) -> Result<Self> {
        Ok(Self {
            halign: Align::decode(p)?,
            valign: Align::decode(p)?,
            offset: [p.g2s()?, p.g2s()?],
            size: [p.g2()?, p.g2()?],
            text_offset: p.g2s()?,
            font: p.g_smart2or4s()?,
            colour: p.g4s()?,
        })
    }
}

/// Sprite ids of a six-sprite bar: fill, empty, left cap, right cap, top
/// edge, bottom edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarSprites {
    pub fill: i32,
    pub empty: i32,
    pub left: i32,
    pub right: i32,
    pub top: i32,
    pub bottom: i32,
}

impl BarSprites {
    pub(super) fn decode(p: &mut Packet<'_>) -> Result<Self> {
        Ok(Self {
            fill: p.g_smart2or4s()?,
            empty: p.g_smart2or4s()?,
            left: p.g_smart2or4s()?,
            right: p.g_smart2or4s()?,
            top: p.g_smart2or4s()?,
            bottom: p.g_smart2or4s()?,
        })
    }
    pub(super) fn ids(&self) -> [i32; 6] {
        [
            self.fill,
            self.empty,
            self.left,
            self.right,
            self.top,
            self.bottom,
        ]
    }
}

/// The element configs of a loading screen, decoded by element-type index
/// (the order of [`ELEMENT_VERSIONS`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElementConfig {
    /// 0: clear the canvas (g4s colour).
    Clear { colour: i32 },
    /// 1: flat colour bar (`fill`/`outline` g4s).
    ColourBar {
        bar: ProgressConfig,
        fill: i32,
        outline: i32,
    },
    /// 2: tiled sprite bar.
    SpriteBar {
        bar: ProgressConfig,
        outline: i32,
        unused: i32,
        sprite: i32,
    },
    /// 3: six-sprite bar.
    SlicedBar {
        bar: ProgressConfig,
        sprites: BarSprites,
    },
    /// 4: news display.
    News(NewsConfig),
    /// 5: sprite.
    Sprite(SpriteConfig),
    /// 6: sprite rotating by `speed`.
    RotatingSprite { sprite: SpriteConfig, speed: i32 },
    /// 7: paragraph text.
    Text(TextConfig),
    /// 8: aspect-filling background sprite.
    Background { sprite: i32 },
    /// 9: six-sprite bar whose fill scrolls by `speed`.
    ScrollingBar {
        bar: ProgressConfig,
        sprites: BarSprites,
        speed: i32,
    },
    /// 10: status/percent text.
    Status(StatusConfig),
}

/// A positioned sprite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpriteConfig {
    pub sprite: i32,
    pub halign: Align,
    pub valign: Align,
    pub offset: [i32; 2],
}

impl SpriteConfig {
    pub(super) fn decode(p: &mut Packet<'_>) -> Result<Self> {
        Ok(Self {
            sprite: p.g_smart2or4s()?,
            halign: Align::decode(p)?,
            valign: Align::decode(p)?,
            offset: [p.g2s()?, p.g2s()?],
        })
    }
}

/// A news display box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewsConfig {
    pub entry: i32,
    pub halign: Align,
    pub valign: Align,
    pub offset: [i32; 2],
    pub size: [i32; 2],
    pub colour: i32,
    pub shadow: i32,
    pub border: i32,
    pub draw_border: bool,
}

/// A paragraph of text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextConfig {
    pub text: String,
    pub halign: Align,
    pub valign: Align,
    pub offset: [i32; 2],
    /// Paragraph alignment and line height.
    pub text_halign: i32,
    pub text_valign: i32,
    pub line_height: i32,
    pub size: [i32; 2],
    pub font: i32,
    pub colour: i32,
    pub shadow: i32,
}

/// A status/percent text line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusConfig {
    pub halign: Align,
    pub valign: Align,
    pub offset: [i32; 2],
    pub font: i32,
    pub colour: i32,
    /// 0 stage text, 1 percent, 2 text with percent.
    pub kind: i32,
}

impl ElementConfig {
    /// Decodes one element config by element-type index.
    pub(super) fn decode(p: &mut Packet<'_>, kind: i32) -> Result<Option<Self>> {
        Ok(Some(match kind {
            0 => Self::Clear { colour: p.g4s()? },
            1 => {
                let bar = ProgressConfig::decode(p)?;
                Self::ColourBar {
                    bar,
                    fill: p.g4s()?,
                    outline: p.g4s()?,
                }
            }
            2 => {
                let bar = ProgressConfig::decode(p)?;
                Self::SpriteBar {
                    bar,
                    outline: p.g4s()?,
                    unused: p.g4s()?,
                    sprite: p.g_smart2or4s()?,
                }
            }
            3 => Self::SlicedBar {
                bar: ProgressConfig::decode(p)?,
                sprites: BarSprites::decode(p)?,
            },
            4 => Self::News(NewsConfig {
                entry: p.g1()?,
                halign: Align::decode(p)?,
                valign: Align::decode(p)?,
                offset: [p.g2s()?, p.g2s()?],
                size: [p.g2()?, p.g2()?],
                colour: p.g4s()?,
                shadow: p.g4s()?,
                border: p.g4s()?,
                draw_border: p.g1()? == 1,
            }),
            5 => Self::Sprite(SpriteConfig::decode(p)?),
            6 => Self::RotatingSprite {
                sprite: SpriteConfig::decode(p)?,
                speed: p.g3s()?,
            },
            7 => Self::Text(TextConfig {
                text: p.gjstr()?,
                halign: Align::decode(p)?,
                valign: Align::decode(p)?,
                offset: [p.g2s()?, p.g2s()?],
                text_halign: p.g1()?,
                text_valign: p.g1()?,
                line_height: p.g1()?,
                size: [p.g2()?, p.g2()?],
                font: p.g_smart2or4s()?,
                colour: p.g4s()?,
                shadow: p.g4s()?,
            }),
            8 => Self::Background {
                sprite: p.g_smart2or4s()?,
            },
            9 => {
                let bar = ProgressConfig::decode(p)?;
                let sprites = BarSprites::decode(p)?;
                Self::ScrollingBar {
                    bar,
                    sprites,
                    speed: p.g2s()?,
                }
            }
            10 => Self::Status(StatusConfig {
                halign: Align::decode(p)?,
                valign: Align::decode(p)?,
                offset: [p.g2s()?, p.g2s()?],
                font: p.g_smart2or4s()?,
                colour: p.g4s()?,
                kind: p.g1()?,
            }),
            // The 11-entry type table is indexed: anything else is out of range.
            _ => anyhow::bail!(Fault::IndexOutOfRange.message(format!("element type {kind}"))),
        }))
    }
}

/// The format version of each element type, in element-type order: the
/// versions the screen index file must list.
pub const ELEMENT_VERSIONS: [i32; 11] = [1, 2, 2, 2, 1, 1, 1, 2, 1, 2, 1];

/// The decoded element list of one loading screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenLayout {
    pub elements: Vec<Option<ElementConfig>>,
}

impl ScreenLayout {
    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut p = Packet::new(data);
        let count = p.g1()?;
        let mut elements = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let kind = p.g1()?;
            elements.push(ElementConfig::decode(&mut p, kind)?);
        }
        Ok(Self { elements })
    }
}

/// A screen reference: screen id, minimum display time (ms) and cross-fade
/// time (ms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenRef {
    pub id: i32,
    pub min_time: i32,
    pub fade: i32,
}

/// A `loadingScreen` preference slot: shuffle flag and its screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenSet {
    pub shuffle: bool,
    pub screens: Vec<ScreenRef>,
}

/// The loading-screens archive's group 0 index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenIndex {
    /// Indexed by the `loadingScreen` preference.
    pub sets: Vec<ScreenSet>,
    /// The fixed first screen, or -1.
    pub first: i32,
}

impl ScreenIndex {
    /// Decodes the index; a missing, old or mismatched index is empty.
    pub fn decode(data: Option<&[u8]>) -> Result<Self> {
        let empty = Self {
            sets: Vec::new(),
            first: -1,
        };
        let data = data.unwrap_or(&[]);
        let mut p = Packet::new(data);
        let version = if data.is_empty() { -1 } else { p.g1()? };
        if version < 4 {
            return Ok(empty);
        }
        let types = p.g1()?;
        if types as usize != ELEMENT_VERSIONS.len() {
            return Ok(empty);
        }
        for expected in ELEMENT_VERSIONS {
            if p.g1()? != expected {
                return Ok(empty);
            }
        }
        let count = p.g1()?;
        let max = p.g1()?;
        let (first, first_min, first_fade) = if version > 2 {
            (p.g2s()?, p.g3()?, p.g2()?)
        } else {
            (-1, 0, 0)
        };
        let mut sets: Vec<Option<ScreenSet>> = vec![None; max as usize + 1];
        for _ in 0..count {
            let slot = p.g1()? as usize;
            let shuffle = p.g1()? == 1;
            let n = p.g2()?;
            let mut screens = Vec::with_capacity(n as usize + 1);
            if first != -1 {
                screens.push(ScreenRef {
                    id: first,
                    min_time: first_min,
                    fade: first_fade,
                });
            }
            for _ in 0..n {
                screens.push(ScreenRef {
                    id: p.g2()?,
                    min_time: p.g3()?,
                    fade: p.g2()?,
                });
            }
            *sets
                .get_mut(slot)
                .with_context(|| Fault::IndexOutOfRange.message("loading screen slot"))? =
                Some(ScreenSet { shuffle, screens });
        }
        let sets = sets
            .into_iter()
            .map(|set| set.unwrap_or_else(|| Self::fallback(first, first_min, first_fade)))
            .collect();
        Ok(Self { sets, first })
    }

    /// The set used for a slot without an entry: just the fixed first screen.
    pub(super) fn fallback(first: i32, min_time: i32, fade: i32) -> ScreenSet {
        ScreenSet {
            shuffle: false,
            screens: if first == -1 {
                Vec::new()
            } else {
                vec![ScreenRef {
                    id: first,
                    min_time,
                    fade,
                }]
            },
        }
    }

    /// Whether a fixed first screen exists.
    #[must_use]
    pub fn has_first(&self) -> bool {
        self.first != -1
    }

    /// The screens for a preference slot, shuffled after the fixed first
    /// screen when the slot asks for it. `random(bound)` is
    /// [`super::BoundedRandom::below`].
    pub fn screens(&self, slot: i32, random: &mut impl FnMut(i32) -> i32) -> Vec<ScreenRef> {
        let Some(set) = usize::try_from(slot).ok().and_then(|s| self.sets.get(s)) else {
            return Self::fallback(self.first, 0, 0).screens;
        };
        if !set.shuffle || set.screens.len() <= 1 {
            return set.screens.clone();
        }
        let fixed = usize::from(self.first != -1);
        let mut out = set.screens.clone();
        for i in fixed..out.len() {
            let j = random((out.len() - fixed) as i32) as usize + fixed;
            let current = set.screens[i];
            out[i] = out[j];
            out[j] = current;
        }
        out
    }
}
