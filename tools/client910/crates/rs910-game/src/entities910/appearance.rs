//! Player appearance model and its customisation hash.
// Moved to rs910-config (Phase 2.6).
pub use rs910_config::types910::customisation::*;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub bas: i32,
    pub kits: Vec<i32>,
    pub custom: Vec<Option<Customisation>>,
    pub colours: [i32; 10],
    pub textures: [i32; 10],
    pub female: bool,
    pub npc: i32,
    pub hash: u64,
}
impl Model {
    /// Recompute the appearance hash. Custom model IDs hash ONLY their low 16
    /// bits; the NPC transformation ID is not hashed. The hash is a CRC-64.
    pub fn update_hash(&mut self) {
        let mut bytes = vec![];
        bytes.extend((self.bas as u16).to_be_bytes());
        for v in &self.kits {
            bytes.extend(v.to_be_bytes())
        }
        for c in self.custom.iter().flatten() {
            let (body, head) = if self.female {
                (&c.woman, &c.woman_head)
            } else {
                (&c.man, &c.man_head)
            };
            for &v in body.iter().chain(head) {
                bytes.extend((v as u16).to_be_bytes())
            }
            for a in [&c.recolour, &c.retexture].into_iter().flatten() {
                for v in a {
                    bytes.extend(v.to_be_bytes())
                }
            }
        }
        bytes.extend(self.colours.map(|v| v as u8));
        bytes.extend(self.textures.map(|v| v as u8));
        bytes.push(self.female as u8);
        let mut h = u64::MAX;
        for b in bytes {
            let mut table = (h ^ b as u64) & 255;
            for _ in 0..8 {
                table = if table & 1 != 0 {
                    (table >> 1) ^ 0xC96C5795D7870F42
                } else {
                    table >> 1
                }
            }
            h = (h >> 8) ^ table;
        }
        self.hash = h;
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub base_size: i32,
    pub gender: i8,
    pub team: i32,
    pub title_id: i32,
    pub title: Option<String>,
    pub visibility: Option<i8>,
    pub name: Option<String>,
    pub combat: i32,
    pub max_combat: i32,
    pub wilderness: i32,
    pub skill: i32,
    pub bas: i32,
    pub model: Option<Model>,
    pub head_ids: [i32; 8],
    pub head_groups: [i32; 8],
    pub sound_range: i32,
    pub sound_ids: [i32; 4],
    pub sound_volume: i32,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            base_size: 1,
            gender: 0,
            team: 0,
            title_id: 0,
            title: None,
            visibility: Some(0),
            name: None,
            combat: 0,
            max_combat: 0,
            wilderness: -1,
            skill: 0,
            bas: 0,
            model: None,
            head_ids: [-1; 8],
            head_groups: [-1; 8],
            sound_range: 0,
            sound_ids: [-1; 4],
            sound_volume: 255,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedPacket {
    pub data: Vec<u8>,
    pub consumed: usize,
}
