//! Entity views of the item and NPC types: the fields entity code needs
//! (names, worn models, palettes, morph list, ambient sound), taken from the
//! types `crate::config` decodes.
use super::{appearance, npc, npc_custom, zone};
use crate::types910::customisation::Customisation;
use std::collections::BTreeMap;

/// One item type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: i32,
    pub name: String,
    pub cost: i32,
    pub stackable: i32,
    pub members: bool,
    pub team: i32,
    pub wear: [i32; 3],
    pub custom: Customisation,
    pub recolour_source: Option<Vec<i16>>,
    pub retexture_source: Option<Vec<i16>>,
    pub man_offset: [i32; 3],
    pub woman_offset: [i32; 3],
}

impl Item {
    pub fn empty(id: i32) -> Self {
        Self {
            id,
            name: "null".into(),
            cost: 1,
            stackable: 0,
            members: false,
            team: 0,
            wear: [-1; 3],
            custom: Customisation {
                man: [-1; 3],
                woman: [-1; 3],
                man_head: [-1; 2],
                woman_head: [-1; 2],
                recolour: None,
                retexture: None,
            },
            recolour_source: None,
            retexture_source: None,
            man_offset: [0; 3],
            woman_offset: [0; 3],
        }
    }

    /// The entity view of a resolved obj type. Outside a members world a
    /// members item loses its team.
    pub fn from_obj(obj: &crate::config::Obj, members: bool) -> Self {
        let present = |values: &[u16]| {
            (!values.is_empty()).then(|| values.iter().map(|&v| v as i16).collect())
        };
        let [heads, heads2] = obj.head_models;
        Self {
            id: obj.id as i32,
            name: obj.name.clone(),
            cost: obj.inventory.cost,
            stackable: obj.inventory.stackable,
            members: obj.members,
            team: if !members && obj.members { 0 } else { obj.team },
            wear: obj.inventory.wearpos,
            custom: Customisation {
                man: obj.worn[0],
                woman: obj.worn[1],
                man_head: [heads[0], heads2[0]],
                woman_head: [heads[1], heads2[1]],
                recolour: present(&obj.recol_d),
                retexture: present(&obj.retex_d),
            },
            recolour_source: present(&obj.recol_s),
            retexture_source: present(&obj.retex_s),
            man_offset: obj.worn_offsets[0],
            woman_offset: obj.worn_offsets[1],
        }
    }

    pub fn appearance(&self) -> appearance::Item {
        appearance::Item {
            team: self.team,
            defaults: self.custom.clone(),
        }
    }

    pub fn ground_type(&self) -> zone::ObjectType {
        zone::ObjectType {
            cost: self.cost,
            stackable: self.stackable,
        }
    }
}

/// One NPC type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Npc {
    pub id: i32,
    pub name: String,
    pub size: i32,
    pub turnspeed: i32,
    pub vislevel: i32,
    pub bas: i32,
    pub models: Vec<i32>,
    pub heads: Option<Vec<i32>>,
    pub modeloffset: Option<Vec<Option<[i32; 3]>>>,
    pub recolour: Option<Vec<i16>>,
    pub retexture: Option<Vec<i16>>,
    pub headicons: Option<Vec<(i32, i16)>>,
    pub covermarker: i32,
    pub multivarbit: i32,
    pub multivarp: i32,
    pub multinpc: Option<Vec<i32>>,
    pub sounds: [i32; 4],
    pub sound_range: i32,
    pub sound_dropoff: i32,
    pub sound_volume: i32,
    pub sound_rates: [i32; 2],
    pub active: bool,
    pub walksmoothing: bool,
    pub respawndir: Option<i32>,
    /// Bit 0 lets the client stand the NPC on a map square's spawn list.
    pub walkflags: i32,
    pub fade: i32,
    pub tint: [i8; 4],
}

impl Npc {
    pub fn empty(id: i32) -> Self {
        Self {
            id,
            name: "null".into(),
            size: 1,
            turnspeed: 32,
            vislevel: -1,
            bas: -1,
            models: vec![],
            heads: None,
            modeloffset: None,
            recolour: None,
            retexture: None,
            headicons: None,
            covermarker: -1,
            multivarbit: -1,
            multivarp: -1,
            multinpc: None,
            sounds: [-1; 4],
            sound_range: 0,
            sound_dropoff: 0,
            sound_volume: 255,
            sound_rates: [256; 2],
            active: true,
            walksmoothing: true,
            respawndir: Some(4),
            walkflags: 0,
            fade: 0,
            tint: [0; 4],
        }
    }

    /// The entity view of an NPC type.
    pub fn from_config(npc: &crate::config::Npc) -> Self {
        let present = |values: &[u16]| {
            (!values.is_empty()).then(|| values.iter().map(|&v| v as i16).collect())
        };
        Self {
            id: npc.id as i32,
            name: npc.name.clone(),
            size: i32::from(npc.size),
            turnspeed: npc.turnspeed,
            vislevel: npc.vislevel,
            bas: npc.bas,
            models: npc.model_slots.clone(),
            heads: npc.head_slots.clone(),
            modeloffset: npc.modeloffset.clone(),
            recolour: present(&npc.recol_d),
            retexture: present(&npc.retex_d),
            headicons: npc.head_icons.clone(),
            covermarker: npc.covermarker,
            multivarbit: npc.multivarbit,
            multivarp: npc.multivarp,
            multinpc: (!npc.multinpc.is_empty()).then(|| npc.multinpc.clone()),
            sounds: npc.sounds,
            sound_range: npc.sound_range,
            sound_dropoff: npc.sound_dropoff,
            sound_volume: npc.sound_volume,
            sound_rates: npc.sound_rates,
            active: npc.active,
            walksmoothing: npc.walksmoothing,
            respawndir: npc.respawndir,
            walkflags: npc.walkflags,
            fade: npc.fade_in,
            tint: [
                npc.tint_hue,
                npc.tint_saturation,
                npc.tint_luminence,
                npc.tint_weight,
            ],
        }
    }

    /// Whether the NPC plays a background sound of its own. The crawl-only
    /// sound is deliberately not counted.
    pub fn own_sound(&self) -> bool {
        self.sounds[0] != -1 || self.sounds[2] != -1 || self.sounds[3] != -1
    }
    /// Whether the NPC or, when it morphs, any direct morph target has a
    /// background sound; targets of targets are not followed.
    pub fn has_background_sound(&self, all: &BTreeMap<i32, Npc>) -> bool {
        if let Some(ids) = &self.multinpc {
            ids.iter()
                .filter(|&&id| id != -1)
                .any(|id| all.get(id).is_some_and(Npc::own_sound))
        } else {
            self.own_sound()
        }
    }
    pub fn packet_type(&self) -> npc::NpcType {
        npc::NpcType {
            size: self.size,
            turnspeed: self.turnspeed,
            vislevel: self.vislevel,
            name: self.name.clone(),
            head_icons: self.headicons.clone(),
            covermarker: self.covermarker,
            walkflags: self.walkflags,
            respawndir: self.respawndir,
            has_sound_or_multinpc: self.own_sound() || self.multinpc.is_some(),
        }
    }
    pub fn customisation(&self) -> npc_custom::Spec {
        npc_custom::Spec {
            recolour: self.recolour.as_ref().map(Vec::len),
            retexture: self.retexture.as_ref().map(Vec::len),
        }
    }
}
