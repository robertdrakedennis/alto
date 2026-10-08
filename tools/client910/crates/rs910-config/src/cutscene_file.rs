//! The `cutscenes` JS5 file format: [`Definition`] is the decoded file with
//! its templates, splines, entity and location definitions, routes and the
//! scrambled action list. client910's `cutscene` re-exports it and keeps the
//! logic half (the cutscene manager state and the action clock).
use crate::ui_bytes::Cursor;
use anyhow::{bail, Context, Result};
use rs910_core::fault::Fault;
/// JS5 archive name of the cutscenes archive (id 35).
pub const ARCHIVE: &str = "cutscenes";

/// A source region copied into the cutscene build area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    pub src_level: i32,
    /// Source tile coordinates (`srcMapX/srcMapZ`, 14 bits each).
    pub src_x: i32,
    pub src_z: i32,
    /// Size and destination in 8x8 chunks.
    pub size_x: i32,
    pub size_z: i32,
    pub dest_level: i32,
    pub dest_x: i32,
    pub dest_z: i32,
    pub rotation: i32,
}
impl Template {
    fn decode(p: &mut Cursor<'_>) -> Result<Self> {
        let packed = p.g4s()?;
        Ok(Self {
            src_level: ((packed as u32) >> 28) as i32,
            src_x: ((packed as u32) >> 14 & 0x3FFF) as i32,
            src_z: packed & 0x3FFF,
            size_x: i32::from(p.g1()?),
            size_z: i32::from(p.g1()?),
            dest_level: i32::from(p.g1()?),
            dest_x: i32::from(p.g1()?),
            dest_z: i32::from(p.g1()?),
            rotation: i32::from(p.g1()?),
        })
    }
}

/// Keyframe pairs in fine units relative to the cutscene area (`g2 - 5120`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spline {
    pub from: Vec<[i32; 3]>,
    pub roll: Vec<i32>,
    pub to: Vec<[i32; 3]>,
}
impl Spline {
    fn decode(p: &mut Cursor<'_>) -> Result<Self> {
        let n = p.gsmart1or2()?;
        let n =
            usize::try_from(n).with_context(|| Fault::NegativeSize.message("cutscene spline"))?;
        let mut s = Self {
            from: Vec::with_capacity(n),
            roll: Vec::with_capacity(n),
            to: Vec::with_capacity(n),
        };
        for _ in 0..n {
            // from x, z, y, to x, z, y, roll.
            let fx = i32::from(p.g2()?) - 5120;
            let fz = i32::from(p.g2()?) - 5120;
            let fy = i32::from(p.g2()? as i16);
            let tx = i32::from(p.g2()?) - 5120;
            let tz = i32::from(p.g2()?) - 5120;
            let ty = i32::from(p.g2()? as i16);
            let roll = i32::from(p.g2()? as i16);
            s.from.push([fx, fy, fz]);
            s.to.push([tx, ty, tz]);
            s.roll.push(roll);
        }
        Ok(s)
    }
    /// The uploaded spline rows `{x, y, z, roll}` for every from/to pair.
    #[must_use]
    pub fn rows(&self) -> Vec<Vec<i32>> {
        let mut rows = vec![vec![0; 4]; self.from.len() << 1];
        for i in 0..self.from.len() {
            let [x, y, z] = self.from[i];
            rows[i * 2] = vec![x, y, z, self.roll[i]];
            let [x, y, z] = self.to[i];
            rows[i * 2 + 1] = vec![x, y, z, self.roll[i]];
        }
        rows
    }
}

/// Entity definition half.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityDef {
    pub index: usize,
    /// `-1` for a player entity dressed from the cutscene appearance.
    pub npc_id: i32,
    /// The trailing `gjstr`, read and discarded by the original client.
    pub name: String,
}

/// Location definition half.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocationDef {
    pub loc_id: i32,
    /// The loc shape id; `layer` is the layer of that shape.
    pub shape: i32,
    pub layer: i32,
}

/// A route: per-step speeds and tile waypoints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub speeds: Vec<i32>,
    /// `(x << 16) + z` tile waypoints.
    pub waypoints: Vec<i32>,
}

/// One decoded cutscene action, tagged with its `startTick` (`g2`).
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub start_tick: i32,
    pub kind: ActionKind,
}

/// The 27 concrete action kinds (the command byte is the kind's id, mapped
/// through its scramble id).
#[derive(Clone, Debug, PartialEq)]
pub enum ActionKind {
    /// Entity hitmark action (id 15).
    EntityHitmark {
        entity: usize,
        hitmark: i32,
        damage: i32,
        secondary: i32,
        secondary_damage: i32,
    },
    /// Entity move action (id 10).
    EntityMove {
        entity: usize,
        x: i32,
        z: i32,
        level: i32,
        angle: i32,
    },
    /// Finish action (id 255).
    Finish,
    /// Sound effect action (id 31): an SFX created at decode.
    Sound31 {
        sound: i32,
        volume: i32,
        rate: i32,
        repeats: i32,
    },
    /// Set varbit action (id 61) / set varp action (id 60): the key carries a
    /// `0x100000000` tag for a varbit.
    SetVar { key: i64, value: i32 },
    /// Sound jingle action (id 32).
    SoundJingle { jingle: i32, volume: i32 },
    /// Entity route action (id 12).
    EntityRoute {
        entity: usize,
        route: usize,
        level: i32,
    },
    /// Text coord action (id 41).
    TextCoord {
        x: i32,
        z: i32,
        text: String,
        colour: i32,
        duration: i32,
    },
    /// Entity look action (id 16).
    EntityLook { entity: usize, direction: i32 },
    /// Proj anim action (ids 50-53); `-1` marks the absent side.
    ProjAnim {
        level: i32,
        src_entity: i32,
        src_x: i32,
        src_z: i32,
        dest_entity: i32,
        dest_x: i32,
        dest_z: i32,
        spot: i32,
        start_height: i32,
        target_height: i32,
        duration: i32,
        peak_pitch: i32,
        arc: i32,
    },
    /// Entity say action (id 13).
    EntitySay {
        entity: usize,
        text: String,
        colour: i32,
        time: i32,
    },
    /// Entity anim action (id 14).
    EntityAnim {
        entity: usize,
        seq: i32,
        slot_mask: i32,
    },
    /// Entity del action (id 11).
    EntityDel { entity: usize },
    /// Loc create action (id 20).
    LocCreate {
        location: usize,
        x: i32,
        z: i32,
        level: i32,
        angle: i32,
    },
    /// Fade action (id 40): `colour` is ARGB.
    Fade { duration: i32, colour: i32 },
    /// Loc del action (id 21).
    LocDel { location: usize },
    /// Loc anim action (id 22).
    LocAnim { location: usize, seq: i32 },
    /// Sound vorbis action (id 33).
    SoundVorbis {
        sound: i32,
        volume: i32,
        rate: i32,
        repeats: i32,
    },
    /// Cam move action (id 0).
    CamMove {
        x: i32,
        height: i32,
        z: i32,
        pitch: i32,
        yaw: i32,
    },
    /// Entity spot anim action (id 17).
    EntitySpot {
        spot: i32,
        orientation: i32,
        height: i32,
        entity: usize,
        slot: usize,
        delay: i32,
    },
    /// Sound song action (id 30).
    SoundSong { song: i32, volume: i32 },
    /// Map anim action (id 42).
    MapAnim {
        spot: i32,
        orientation: i32,
        height: i32,
        tile_x: i32,
        tile_z: i32,
        level: i32,
    },
    /// Cam move along action (id 1).
    CamMoveAlong {
        pos_spline: usize,
        target_spline: usize,
        pos_keyframe: usize,
        target_keyframe: usize,
        min_speed: i32,
        max_speed: i32,
    },
    /// Subtitle action (id 70).
    Subtitle { text: String, subtitle: i32 },
}

/// The scramble id of a command byte.
fn scramble_id(command: u8) -> Option<i32> {
    Some(match command {
        0 => 23,  // CAM_MOVE
        1 => 29,  // CAM_MOVEALONG
        2 => 18,  // UNKNOWN_18
        3 => 17,  // UNKNOWN_17
        10 => 1,  // ENTITY_MOVE
        11 => 13, // ENTITY_DEL
        12 => 6,  // ENTITY_ROUTE
        13 => 11, // ENTITY_SAY
        14 => 12, // ENTITY_ANIM
        15 => 0,  // ENTITY_HITMARK
        16 => 9,  // ENTITY_LOOK
        17 => 24, // ENTITY_SPOTANIM
        20 => 14, // LOC_CREATE
        21 => 20, // LOC_DEL
        22 => 21, // LOC_ANIM
        30 => 25, // SOUND_SONG
        31 => 3,  // OP31
        32 => 5,  // SOUND_JINGLE
        33 => 22, // SOUND_VORBIS
        40 => 19, // FADE
        41 => 8,  // TEXT_COROD
        42 => 28, // MAP_ANIM
        43 => 7,  // UNKNOWN_7
        50 => 10, // PROJANIM_ENTITY_ENTITY
        51 => 26, // PROJANIM_COORD_ENTITY
        52 => 15, // PROJANIM_COORD_COORD
        53 => 27, // PROJANIM_ENTITY_COORD
        60 => 16, // SET_VAR
        61 => 4,  // SET_VARBIT
        70 => 30, // SUBTITLE
        255 => 2, // FINISH
        _ => return None,
    })
}

fn index(v: u16) -> usize {
    usize::from(v)
}

impl Action {
    /// Decode one action. Commands 17/18/43 (`UNKNOWN_*`, scramble 3/2/7)
    /// construct no action in the original client; the missing entry then fails
    /// the cutscene load at its readiness check, so they are load errors here.
    pub fn decode(p: &mut Cursor<'_>) -> Result<Self> {
        let command = p.g1()?;
        let scramble = scramble_id(command).with_context(|| {
            Fault::MissingValue.message(format_args!("cutscene command {command}"))
        })?;
        if matches!(scramble, 7 | 17 | 18) {
            bail!(
                "{}",
                Fault::MissingValue.message(format_args!(
                    "cutscene action for command {command} is null"
                ))
            );
        }
        let start_tick = i32::from(p.g2()?);
        let kind = match scramble {
            0 => {
                let entity = index(p.g2()?);
                let flags = p.g1()?;
                let (hitmark, damage) = if flags & 1 == 0 {
                    (-1, -1)
                } else {
                    (i32::from(p.g2()?), i32::from(p.g2()?))
                };
                let (secondary, secondary_damage) = if flags & 2 == 0 {
                    (-1, -1)
                } else {
                    (i32::from(p.g2()?), i32::from(p.g2()?))
                };
                if flags & 4 != 0 {
                    // The headbar fields are read and discarded; the client
                    // divides by the second value, so zero fails.
                    let current = i32::from(p.g2()?);
                    let max = i32::from(p.g2()?);
                    anyhow::ensure!(
                        max != 0,
                        "{}",
                        Fault::DivisionByZero.message("cutscene progress ratio")
                    );
                    let _ = current * 255 / max;
                }
                ActionKind::EntityHitmark {
                    entity,
                    hitmark,
                    damage,
                    secondary,
                    secondary_damage,
                }
            }
            1 => {
                let entity = index(p.g2()?);
                let packed = p.g4s()?;
                ActionKind::EntityMove {
                    entity,
                    x: ((packed as u32) >> 16) as i32,
                    z: packed & 0xFFFF,
                    level: i32::from(p.g1()?),
                    angle: gsmart1or2s(p)?,
                }
            }
            2 => ActionKind::Finish,
            3 | 22 => {
                let sound = i32::from(p.g2()?);
                let volume = i32::from(p.g1()?);
                let rate = i32::from(p.g1()?);
                let repeats = i32::from(p.g1()?);
                if scramble == 3 {
                    ActionKind::Sound31 {
                        sound,
                        volume,
                        rate,
                        repeats,
                    }
                } else {
                    ActionKind::SoundVorbis {
                        sound,
                        volume,
                        rate,
                        repeats,
                    }
                }
            }
            4 | 16 => {
                let id = i64::from(p.g2()?);
                let key = if scramble == 4 {
                    id | 0x1_0000_0000
                } else {
                    id
                };
                ActionKind::SetVar {
                    key,
                    value: p.g4s()?,
                }
            }
            5 => ActionKind::SoundJingle {
                jingle: i32::from(p.g2()?),
                volume: i32::from(p.g1()?),
            },
            6 => ActionKind::EntityRoute {
                entity: index(p.g2()?),
                route: index(p.g2()?),
                level: i32::from(p.g1()?),
            },
            8 => ActionKind::TextCoord {
                x: i32::from(p.g2()?),
                z: i32::from(p.g2()?),
                text: p.gjstr()?,
                colour: p.g4s()?,
                duration: i32::from(p.g2()?),
            },
            // Entity look.
            9 => ActionKind::EntityLook {
                entity: index(p.g2()?),
                direction: i32::from(p.g2()?),
            },
            // Projectile animation, by (source is entity, target is entity):
            // scrambles 10 (1,1), 15 (0,0), 26 (0,1), 27 (1,0).
            10 | 15 | 26 | 27 => {
                let (src_entity, dest_entity) = match scramble {
                    10 => (true, true),
                    15 => (false, false),
                    26 => (false, true),
                    _ => (true, false),
                };
                let (src_x, src_z, src) = if src_entity {
                    (-1, -1, i32::from(p.g2()?))
                } else {
                    let v = p.g4s()?;
                    (((v as u32) >> 16) as i32, v & 0xFFFF, -1)
                };
                let (dest_x, dest_z, dest) = if dest_entity {
                    (-1, -1, i32::from(p.g2()?))
                } else {
                    let v = p.g4s()?;
                    (((v as u32) >> 16) as i32, v & 0xFFFF, -1)
                };
                let level = if !src_entity && !dest_entity {
                    i32::from(p.g1()?)
                } else {
                    -1
                };
                ActionKind::ProjAnim {
                    level,
                    src_entity: src,
                    src_x,
                    src_z,
                    dest_entity: dest,
                    dest_x,
                    dest_z,
                    spot: i32::from(p.g2()?),
                    start_height: i32::from(p.g1()?),
                    target_height: i32::from(p.g1()?),
                    duration: p.g3()? as i32,
                    peak_pitch: i32::from(p.g2()?),
                    arc: i32::from(p.g1()?),
                }
            }
            11 => ActionKind::EntitySay {
                entity: index(p.g2()?),
                text: p.gjstr()?,
                colour: p.g4s()?,
                time: i32::from(p.g2()?),
            },
            12 => ActionKind::EntityAnim {
                entity: index(p.g2()?),
                seq: p.gsmart2or4s()?,
                slot_mask: p.g4s()?,
            },
            13 => ActionKind::EntityDel {
                entity: index(p.g2()?),
            },
            14 => {
                let location = index(p.g2()?);
                let packed = p.g4s()?;
                ActionKind::LocCreate {
                    location,
                    x: ((packed as u32) >> 16) as i32,
                    z: packed & 0xFFFF,
                    level: i32::from(p.g1()?),
                    angle: i32::from(p.g1()?),
                }
            }
            19 => ActionKind::Fade {
                duration: i32::from(p.g2()?),
                colour: p.g4s()?,
            },
            20 => ActionKind::LocDel {
                location: index(p.g2()?),
            },
            21 => ActionKind::LocAnim {
                location: index(p.g2()?),
                seq: p.gsmart2or4s()?,
            },
            23 => ActionKind::CamMove {
                x: i32::from(p.g2()?),
                height: i32::from(p.g2()?),
                z: i32::from(p.g2()?),
                pitch: i32::from(p.g2()?),
                yaw: i32::from(p.g2()?),
            },
            24 => {
                let spot = i32::from(p.g2()?);
                let orientation = i32::from(p.g2()?);
                let height = i32::from(p.g1()?);
                ActionKind::EntitySpot {
                    spot,
                    orientation,
                    height,
                    entity: index(p.g2()?),
                    slot: usize::from(p.g1()?),
                    delay: i32::from(p.g2()?),
                }
            }
            25 => ActionKind::SoundSong {
                song: i32::from(p.g2()?),
                volume: i32::from(p.g1()?),
            },
            28 => {
                let spot = i32::from(p.g2()?);
                let orientation = i32::from(p.g2()?);
                let height = i32::from(p.g1()?);
                let packed = p.g4s()?;
                ActionKind::MapAnim {
                    spot,
                    orientation,
                    height,
                    tile_x: ((packed as u32) >> 16) as i32,
                    tile_z: packed & 0xFFFF,
                    level: i32::from(p.g1()?),
                }
            }
            29 => ActionKind::CamMoveAlong {
                pos_spline: index(p.g2()?),
                target_spline: index(p.g2()?),
                pos_keyframe: index(p.g2()?),
                target_keyframe: index(p.g2()?),
                min_speed: i32::from(p.g2()?),
                max_speed: i32::from(p.g2()?),
            },
            30 => ActionKind::Subtitle {
                text: p.gjstr()?,
                subtitle: i32::from(p.g2()?),
            },
            other => bail!("cutscene action scramble {other} is unknown"),
        };
        Ok(Self { start_tick, kind })
    }
}

/// `gSmart1or2s`: `g1() - 64` when the first
/// byte is below 128, otherwise `g2() - 49152`.
fn gsmart1or2s(p: &mut Cursor<'_>) -> Result<i32> {
    let first = i32::from(p.g1()?);
    if first < 128 {
        Ok(first - 64)
    } else {
        let second = i32::from(p.g1()?);
        Ok(((first << 8) | second) - 49152)
    }
}

/// Loc shape id -> layer; unknown ids decode to none in the original client
/// and fail when the models are checked for readiness.
fn shape_layer(shape: u8) -> Option<i32> {
    Some(match shape {
        0..=3 => 0,
        4..=8 => 1,
        9..=21 => 2,
        22 => 3,
        _ => return None,
    })
}

/// One decoded `cutscenes` file.
#[derive(Clone, Debug, PartialEq)]
pub struct Definition {
    /// The letterbox ratio (header opcode 0).
    pub viewport_width: i32,
    pub viewport_height: i32,
    pub templates: Vec<Template>,
    pub splines: Vec<Spline>,
    pub entities: Vec<EntityDef>,
    pub locations: Vec<LocationDef>,
    pub routes: Vec<Route>,
    pub actions: Vec<Action>,
}
impl Definition {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Ok(Self::decode_counted(bytes)?.0)
    }
    /// [`Self::decode`] plus the unread trailing byte count (the client never
    /// checks it; the cache test proves every file is consumed exactly).
    pub fn decode_counted(bytes: &[u8]) -> Result<(Self, usize)> {
        let mut p = Cursor::new(bytes);
        let (mut viewport_width, mut viewport_height) = (0, 0);
        // Header: unknown opcodes consume only their byte.
        loop {
            match p.g1()? {
                0 => {
                    viewport_width = i32::from(p.g2()?);
                    viewport_height = i32::from(p.g2()?);
                }
                255 => break,
                _ => {}
            }
        }
        let n = p.g1()?;
        let templates = (0..n)
            .map(|_| Template::decode(&mut p))
            .collect::<Result<_>>()?;
        let count = |p: &mut Cursor<'_>, what: &str| -> Result<usize> {
            usize::try_from(p.gsmart1or2()?).with_context(|| Fault::NegativeSize.message(what))
        };
        let n = count(&mut p, "splines")?;
        let splines = (0..n)
            .map(|_| Spline::decode(&mut p))
            .collect::<Result<_>>()?;
        let n = count(&mut p, "entities")?;
        let mut entities = Vec::with_capacity(n);
        for index in 0..n {
            // Type 0 is an NPC (a two-or-four byte id), otherwise a player.
            let npc_id = match p.g1()? {
                0 => p.gsmart2or4s()?,
                _ => -1,
            };
            entities.push(EntityDef {
                index,
                npc_id,
                name: p.gjstr()?,
            });
        }
        let n = count(&mut p, "locations")?;
        let mut locations = Vec::with_capacity(n);
        for _ in 0..n {
            let loc_id = p.gsmart2or4s()?;
            let shape = p.g1()?;
            let layer = shape_layer(shape).with_context(|| {
                Fault::MissingValue.message(format_args!("cutscene location shape {shape}"))
            })?;
            locations.push(LocationDef {
                loc_id,
                shape: i32::from(shape),
                layer,
            });
        }
        let n = count(&mut p, "routes")?;
        let mut routes = Vec::with_capacity(n);
        for _ in 0..n {
            let m = count(&mut p, "route")?;
            let mut route = Route {
                speeds: Vec::with_capacity(m),
                waypoints: Vec::with_capacity(m),
            };
            for _ in 0..m {
                route.speeds.push(i32::from(p.g1()?));
                let x = i32::from(p.g2()?);
                let z = i32::from(p.g2()?);
                route.waypoints.push((x << 16) + z);
            }
            routes.push(route);
        }
        let n = count(&mut p, "actions")?;
        let actions = (0..n)
            .map(|_| Action::decode(&mut p))
            .collect::<Result<_>>()?;
        Ok((
            Self {
                viewport_width,
                viewport_height,
                templates,
                splines,
                entities,
                locations,
                routes,
                actions,
            },
            p.remaining(),
        ))
    }

    /// Fetch and decode cutscene `id` from the cutscenes archive.
    pub fn load(pack: &crate::cache::Pack, id: i32) -> Result<Self> {
        let id = u32::try_from(id).context("negative cutscene id")?;
        let bytes =
            crate::anim::fetch_file(pack, ARCHIVE, id).with_context(|| format!("cutscene {id}"))?;
        Self::decode(&bytes).with_context(|| format!("decode cutscene {id}"))
    }

    /// The 26-bit region-data templates
    /// (plane, chunk x, chunk z order; `-1` empty) of a `mapSize`-tile area.
    pub fn region_templates(&self, map_size: i32) -> Result<Vec<i32>> {
        let chunks = usize::try_from(map_size >> 3).context("map size")?;
        let mut data = vec![-1; 4 * chunks * chunks];
        for t in &self.templates {
            let rotation = t.rotation;
            let odd = rotation & 1 == 1;
            let src_x = t.src_x >> 3;
            let src_z = t.src_z >> 3;
            let (mut z_offset, mut x_start, mut x_step, mut z_step) = (0, 0, 1, 1);
            if rotation == 1 {
                x_start = t.size_x - 1;
                x_step = -1;
            } else if rotation == 2 {
                x_start = t.size_x - 1;
                z_offset = t.size_z - 1;
                x_step = -1;
                z_step = -1;
            } else if rotation == 3 {
                z_offset = t.size_z - 1;
                x_step = 1;
                z_step = -1;
            }
            for sz in src_z..src_z + t.size_z {
                let mut x_offset = x_start;
                for sx in src_x..src_x + t.size_x {
                    let packed = (rotation << 1) + (sz << 3) + (t.src_level << 24) + (sx << 14);
                    let (cx, cz) = if odd {
                        (t.dest_x + z_offset, t.dest_z + x_offset)
                    } else {
                        (t.dest_x + x_offset, t.dest_z + z_offset)
                    };
                    let slot = usize::try_from(t.dest_level)
                        .ok()
                        .zip(usize::try_from(cx).ok())
                        .zip(usize::try_from(cz).ok())
                        .filter(|((l, x), z)| *l < 4 && *x < chunks && *z < chunks)
                        .map(|((l, x), z)| (l * chunks + x) * chunks + z)
                        .with_context(|| Fault::IndexOutOfRange.message("region data"))?;
                    data[slot] = packed;
                    x_offset += x_step;
                }
                z_offset += z_step;
            }
        }
        Ok(data)
    }

    /// The source map squares (`x << 8 | z`) in template order, filtered by
    /// whether the LAND group is valid, into an array of `capacity` entries.
    pub fn map_squares(
        &self,
        capacity: i32,
        land_groups: &std::collections::BTreeSet<i32>,
    ) -> Result<Vec<i32>> {
        let capacity = usize::try_from(capacity)
            .with_context(|| Fault::NegativeSize.message("map square capacity"))?;
        let mut squares: Vec<i32> = Vec::new();
        for t in &self.templates {
            let x0 = t.src_x >> 3;
            let z0 = t.src_z >> 3;
            let mut x1 = t.size_x + x0;
            if x1 & 7 == 0 {
                x1 -= 1;
            }
            let mut z1 = t.size_z + z0;
            if z1 & 7 == 0 {
                z1 -= 1;
            }
            for mx in (x0 >> 3)..=(x1 >> 3) {
                for mz in (z0 >> 3)..=(z1 >> 3) {
                    let square = (mx << 8) | mz;
                    if squares.contains(&square) {
                        continue;
                    }
                    if land_groups.contains(&(mx | (mz << 7))) {
                        anyhow::ensure!(
                            squares.len() < capacity,
                            "{}",
                            Fault::IndexOutOfRange.message("map squares to rebuild")
                        );
                        squares.push(square);
                    }
                }
            }
        }
        Ok(squares)
    }
}
