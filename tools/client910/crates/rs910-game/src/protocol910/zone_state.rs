//! Zone packets: loc changes, customisations, ground objects, sounds, text
//! coords and animations.
//! Location replacements and customisations are retained for the live scene
//! owner, which promotes their models into the matching scene layer.
use super::{zone, Context, Error, Packet, Result};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub level: i32,
    pub layer: i32,
    pub x: i32,
    pub z: i32,
    pub old_id: i32,
    pub old_shape: i32,
    pub old_angle: i32,
    pub id: i32,
    pub shape: i32,
    pub angle: i32,
    pub transform: Option<[f32; 10]>,
    pub custom: Option<LocCustom>,
    pub pending: bool,
    pub remove: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub next_custom: i64,
    pub level: i32,
    pub x: i32,
    pub z: i32,
    pub objects: zone::Objects,
    pub transients: crate::entities910::transient::Transients,
    pub locations: Vec<Location>,
    pub customisations: Vec<Location>,
    /// Loc animation requests retained until the live scene applies them.
    pub loc_animations: Vec<LocAnimation>,
    pub sounds: Vec<SoundArea>,
    pub text_coords: Vec<TextCoord>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct LocCustom {
    pub salt: i64,
    pub models: Option<Vec<i32>>,
    pub colours: Option<Vec<i16>>,
    pub textures: Option<Vec<i16>>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct LocAnimation {
    pub level: i32,
    pub x: i32,
    pub z: i32,
    pub layer: i32,
    pub shape: i32,
    pub angle: i32,
    pub transform: Option<[f32; 10]>,
    pub sequence: i32,
    pub delay: i32,
}
/// A positional sound (the sound-area packet and its dialog variant),
/// retained until the audio owner applies its listener/range check and sends
/// it to the mixer. Field names follow the sound-play arguments they feed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundArea {
    pub level: i32,
    pub x: i32,
    pub z: i32,
    pub sound: i32,
    pub loops: i32,
    pub radius: i32,
    /// `g1` start delay.
    pub delay: i32,
    /// `g1` volume 0-255.
    pub volume: i32,
    /// `g2` playback rate.
    pub rate: i32,
    /// Dialog variant only: `g1() == 1` selects the dialog sub-bus over the
    /// sound-effects sub-bus.
    pub dialog: bool,
}
/// A temporary world-space text label, drawn until its loop-cycle expiry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextCoord {
    pub level: i32,
    pub x: i32,
    pub z: i32,
    pub height: i32,
    pub expiry_cycle: i32,
    pub colour: i32,
    pub text: String,
}
impl Default for State {
    fn default() -> Self {
        Self {
            level: 0,
            x: 0,
            z: 0,
            next_custom: 1,
            objects: Default::default(),
            transients: Default::default(),
            locations: vec![],
            customisations: vec![],
            loc_animations: vec![],
            sounds: vec![],
            text_coords: vec![],
        }
    }
}
#[derive(Clone, Copy)]
pub enum Input {
    PartialFollows,
    FullFollows,
    Enclosed,
    Atom(u8),
}
pub struct Config<'a> {
    pub map: &'a Context,
    pub cycle: i32,
    pub cutscene: bool,
    pub allow_outside: bool,
    pub objects: &'a BTreeMap<i32, zone::ObjectType>,
    pub scene: Option<&'a SceneLocs>,
    pub transients: Option<&'a super::transient::Config<'a>>,
}
/// Installed scene loc snapshots by location key.
pub type SceneLocs = BTreeMap<(i32, i32, i32, i32), Snapshot>;
#[derive(Debug)]
pub struct Snapshot {
    pub id: i32,
    pub shape: i32,
    pub angle: i32,
    pub transform: Option<[f32; 10]>,
}
#[derive(Debug)]
pub struct Decoded {
    pub state: State,
    #[allow(dead_code, reason = "consumed byte count retained; no reader yet")]
    pub bytes: usize,
    pub refresh: Vec<(i32, i32, i32)>,
}

/// The loc-animation packet. It targets a location by
/// absolute packed tile; the scene consumer resolves the matching dynamic
/// location after the retained protocol state commits.
pub fn decode_loc_anim_specific(bytes: &[u8], prior: &State, c: &Config) -> Result<Decoded> {
    let mut p = Packet::new(bytes);
    let packed = p.g4_alt1()? as u32;
    let level = ((packed >> 28) & 3) as i32;
    let x = ((packed >> 14) & 0x3fff) as i32 - c.map.base_x;
    let z = (packed & 0x3fff) as i32 - c.map.base_z;
    let delay = p.byte()? as i32;
    let sequence = p.g4s()?;
    let info = p.byte()?;
    let shape = ((info >> 2) & 31) as i32;
    let angle = (info & 3) as i32;
    let transform = if info & 128 != 0 {
        Some(transform(&mut p)?)
    } else {
        None
    };
    if p.pos != bytes.len() {
        return Err(Error::Invalid("LOC_ANIM_SPECIFIC length"));
    }
    let layer = loc_layer(shape)?;
    let mut state = prior.clone();
    if x >= 0 && z >= 0 && x + 1 < c.map.width && z + 1 < c.map.height {
        state.loc_animations.push(LocAnimation {
            level,
            x,
            z,
            layer,
            shape,
            angle,
            transform,
            sequence,
            delay,
        });
    }
    Ok(Decoded {
        state,
        bytes: p.pos,
        refresh: vec![],
    })
}

fn loc_layer(shape: i32) -> Result<i32> {
    [
        0, 0, 0, 0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3,
    ]
    .get(shape as usize)
    .copied()
    .ok_or(Error::Invalid("loc shape"))
}

pub fn decode(bytes: &[u8], prior: &State, c: &Config, input: Input) -> Result<Decoded> {
    let mut p = Packet::new(bytes);
    let mut s = prior.clone();
    let mut refresh = vec![];
    match input {
        Input::PartialFollows => {
            s.level = p.byte()? as i32;
            s.z = (p.byte()? as u8).wrapping_neg() as i8 as i32 * 8;
            s.x = p.byte()? as i8 as i32 * 8;
        }
        Input::FullFollows => {
            s.z = (p.byte()? as u8).wrapping_neg() as i8 as i32 * 8;
            s.level = (p.byte()?.wrapping_neg() & 255) as i32;
            s.x = 128u8.wrapping_sub(p.byte()? as u8) as i8 as i32 * 8;
            let mut keys = s.objects.key_order.clone();
            keys.sort_by_key(|k| k & 63);
            for key in keys {
                let level = (key >> 28 & 3) as i32;
                let x = (key & 16383) as i32 - c.map.base_x;
                let z = (key >> 14 & 16383) as i32 - c.map.base_z;
                if inside_zone(&s, level, x, z) {
                    s.objects.stacks.remove(&key);
                    s.objects.revision = s.objects.revision.wrapping_add(1);
                    s.objects.key_order.retain(|k| *k != key);
                    if in_scene(c.map, x, z) {
                        refresh.push((level, x, z));
                    }
                }
            }
            for r in s.locations.iter_mut().chain(&mut s.customisations) {
                if r.level == s.level && r.x >= s.x && r.x < s.x + 8 && r.z >= s.z && r.z < s.z + 8
                {
                    r.remove = true;
                }
            }
        }
        Input::Enclosed => {
            s.level = (p.byte()?.wrapping_sub(128) & 255) as i32;
            s.z = (p.byte()? as u8).wrapping_neg() as i8 as i32 * 8;
            s.x = (p.byte()? as u8).wrapping_neg() as i8 as i32 * 8;
            while p.pos < bytes.len() {
                let opcode = p.byte()? as u8;
                atom(opcode, &mut p, &mut s, c, &mut refresh)?;
            }
        }
        Input::Atom(op) => atom(op, &mut p, &mut s, c, &mut refresh)?,
    }
    if p.pos != bytes.len() {
        return Err(Error::Invalid("zone envelope length"));
    }
    Ok(Decoded {
        state: s,
        bytes: p.pos,
        refresh,
    })
}
fn inside_zone(s: &State, level: i32, x: i32, z: i32) -> bool {
    level == s.level && x >= s.x && x < s.x + 8 && z >= s.z && z < s.z + 8
}
fn in_scene(c: &Context, x: i32, z: i32) -> bool {
    x >= 0 && z >= 0 && x < c.width && z < c.height
}
fn atom(
    op: u8,
    p: &mut Packet,
    s: &mut State,
    c: &Config,
    refresh: &mut Vec<(i32, i32, i32)>,
) -> Result<()> {
    if let Some((op, n)) = match op {
        3 => Some((zone::Op::Count, 7)),
        8 => Some((zone::Op::Reveal, 7)),
        9 => Some((zone::Op::Delete, 3)),
        13 => Some((zone::Op::Add, 5)),
        _ => None,
    } {
        let end = p.pos + n;
        let data = p
            .data
            .get(p.pos..end)
            .ok_or(Error::Truncated { bit: p.pos * 8 })?;
        let cx = zone::ZoneContext {
            map: c.map,
            x: s.x,
            z: s.z,
            level: s.level,
            allow_outside: c.allow_outside,
            types: c.objects,
        };
        let out = zone::decode(op, data, &s.objects, &cx)?;
        s.objects = out.state;
        refresh.extend(out.refresh);
        p.pos = end;
        return Ok(());
    }
    if op == 14 || op == 6 {
        // Sound area and its dialog variant: coord, sound id, packed
        // radius/loops, delay, volume, rate; the dialog variant appends the
        // dialog-bus flag.
        let coord = p.byte()? as i32;
        let sound = p.g2()?;
        let packed = p.byte()? as i32;
        let delay = p.byte()? as i32;
        let volume = p.byte()? as i32;
        let rate = p.g2()?;
        let dialog = op == 6 && p.byte()? == 1;
        let x = s.x + ((coord >> 4) & 7);
        let z = s.z + (coord & 7);
        if c.allow_outside || in_scene(c.map, x, z) {
            s.sounds.push(SoundArea {
                level: s.level,
                x,
                z,
                sound: if sound == i32::from(u16::MAX) {
                    -1
                } else {
                    sound
                },
                loops: packed & 7,
                radius: (packed >> 4) & 15,
                delay,
                volume,
                rate,
                dialog,
            });
        }
        return Ok(());
    }
    if op == 2 {
        // TEXT_COORD: the first byte is only a protocol field and is
        // intentionally discarded.
        p.byte()?;
        let coord = p.byte()? as i32;
        let x = s.x + ((coord >> 4) & 7);
        let z = s.z + (coord & 7);
        let duration = p.g2()?;
        let height = p.byte()? as i32;
        let colour = (p.byte()? << 16 | p.byte()? << 8 | p.byte()?) as i32;
        let text = p.string()?;
        if !c.cutscene && (c.allow_outside || in_scene(c.map, x, z)) {
            s.text_coords.push(TextCoord {
                level: s.level,
                x,
                z,
                height,
                expiry_cycle: c.cycle.wrapping_add(duration),
                colour,
                text,
            });
        }
        return Ok(());
    }
    if let Some(n) = match op {
        1 => Some(10),
        7 => Some(18),
        12 => Some(21),
        _ => None,
    } {
        let data = p
            .data
            .get(p.pos..p.pos + n)
            .ok_or(Error::Truncated { bit: p.pos * 8 })?;
        s.transients = super::transient::decode(
            data,
            &s.transients,
            c.map,
            (s.x, s.z, s.level),
            op,
            c.transients
                .ok_or(Error::UnsupportedContext("zone transient context"))?,
        )?;
        p.pos += n;
        return Ok(());
    }
    if op == 11 {
        return customise(p, s);
    }
    if op != 5 && op != 10 {
        return Err(Error::UnsupportedZone(op));
    }
    let info = p.byte()?;
    let id = if op == 10 {
        let a = [p.byte()?, p.byte()?, p.byte()?, p.byte()?];
        ((a[1] << 24) | (a[0] << 16) | (a[3] << 8) | a[2]) as i32
    } else {
        -1
    };
    let coord = if op == 10 {
        p.byte()?.wrapping_neg() & 255
    } else {
        128u32.wrapping_sub(p.byte()?) & 255
    };
    let x = s.x + ((coord >> 4) & 7) as i32;
    let z = s.z + (coord & 7) as i32;
    let shape = (info >> 2) & 31;
    let angle = (info & 3) as i32;
    let transform = if info & 128 != 0 {
        Some(transform(p)?)
    } else {
        None
    };
    let layer = *[
        0, 0, 0, 0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3,
    ]
    .get(shape as usize)
    .ok_or(Error::Invalid("loc shape"))?;
    if c.allow_outside || in_scene(c.map, x, z) {
        let at = if let Some(i) = s
            .locations
            .iter()
            .position(|r| r.level == s.level && r.x == x && r.z == z && r.layer == layer)
        {
            i
        } else {
            let mut r = Location {
                level: s.level,
                layer,
                x,
                z,
                old_id: 0,
                old_shape: 0,
                old_angle: 0,
                id: 0,
                shape: 0,
                angle: 0,
                transform: None,
                custom: None,
                pending: true,
                remove: false,
            };
            if in_scene(c.map, x, z) {
                if let Some(scene) = c.scene {
                    r.old_id = -1;
                    if let Some(old) = scene.get(&(s.level, layer, x, z)) {
                        r.old_id = old.id;
                        r.old_shape = old.shape;
                        r.old_angle = old.angle;
                        r.transform = old.transform;
                    }
                }
            }
            s.locations.push(r);
            s.locations.len() - 1
        };
        let r = &mut s.locations[at];
        r.id = id;
        r.shape = shape as i32;
        r.angle = angle;
        if transform.is_some() {
            r.transform = transform;
        }
        r.pending = true;
        r.remove = false;
    }
    Ok(())
}
/// Compact scale/rotation/translation wire form; the quaternion is x, y, z, w.
fn transform(p: &mut Packet) -> Result<[f32; 10]> {
    let bits = p.byte()?;
    let mut v = [0., 0., 0., 1., 0., 0., 0., 1., 1., 1.];
    if bits & 1 != 0 {
        for x in &mut v[..4] {
            *x = p.g2()? as i16 as f32 / 32768.;
        }
    }
    for i in 0..3 {
        if bits & (2 << i) != 0 {
            v[4 + i] = p.g2()? as i16 as f32
        }
    }
    if bits & 16 != 0 {
        let x = p.g2()? as i16 as f32 / 128.;
        v[7..].fill(x)
    } else {
        for i in 0..3 {
            if bits & (32 << i) != 0 {
                v[7 + i] = p.g2()? as i16 as f32 / 128.
            }
        }
    }
    Ok(v)
}

/// Unlike loc placement, no cutscene or map-bounds filter applies.
fn customise(p: &mut Packet, s: &mut State) -> Result<()> {
    let flags = p.byte()?;
    let b = [p.byte()?, p.byte()?, p.byte()?, p.byte()?];
    let id = ((b[1] << 24) | (b[0] << 16) | (b[3] << 8) | b[2]) as i32;
    let info = p.byte()?.wrapping_neg() & 255;
    let coord = 128u32.wrapping_sub(p.byte()?) & 255;
    let x = s.x + ((coord >> 4) & 7) as i32;
    let z = s.z + (coord & 7) as i32;
    let shape = (info >> 2) & 31;
    // The position transform is consumed but not passed to the customisation request.
    if info & 128 != 0 {
        transform(p)?;
    }
    let layer = *[
        0, 0, 0, 0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3,
    ]
    .get(shape as usize)
    .ok_or(Error::Invalid("custom loc shape"))?;
    let shape = if shape == 11 { 10 } else { shape as i32 };
    let custom = if flags & 1 != 0 {
        None
    } else {
        let models = if flags & 2 != 0 {
            let n = p.byte()?;
            let mut a = vec![];
            for _ in 0..n {
                a.push(((p.g2()? as u32) << 16 | p.g2()? as u32) as i32)
            }
            Some(a)
        } else {
            None
        };
        let mut arrays = [None, None];
        for (i, a) in arrays.iter_mut().enumerate() {
            if flags & (4 << i) != 0 {
                let n = p.byte()?;
                let mut v = vec![];
                for _ in 0..n {
                    v.push(p.g2()? as i16)
                }
                *a = Some(v);
            }
        }
        let [colours, textures] = arrays;
        let custom = Some(LocCustom {
            salt: s.next_custom,
            models,
            colours,
            textures,
        });
        s.next_custom = s.next_custom.wrapping_add(1);
        custom
    };
    let at = if let Some(i) = s
        .customisations
        .iter()
        .position(|r| r.level == s.level && r.layer == layer && r.x == x && r.z == z)
    {
        i
    } else {
        s.customisations.push(Location {
            level: s.level,
            layer,
            x,
            z,
            old_id: 0,
            old_shape: 0,
            old_angle: 0,
            id: 0,
            shape: 0,
            angle: 0,
            transform: None,
            custom: None,
            pending: true,
            remove: false,
        });
        s.customisations.len() - 1
    };
    let r = &mut s.customisations[at];
    r.id = id;
    r.shape = shape;
    r.custom = custom;
    r.pending = true;
    r.remove = false;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_area_retains_zone_fields() {
        let map = Context {
            local: 0,
            base_x: 0,
            base_z: 0,
            width: 104,
            height: 104,
            bridges: vec![],
        };
        let objects = BTreeMap::new();
        let config = Config {
            map: &map,
            cycle: 100,
            cutscene: false,
            allow_outside: false,
            objects: &objects,
            scene: None,
            transients: None,
        };
        // coord 0x23 -> (2,3), sound 65535 -> -1, packed 0x45 ->
        // radius 4 / loop count 5, delay 7, volume 8, rate 0x1234.
        let bytes = [0x23, 0xff, 0xff, 0x45, 7, 8, 0x12, 0x34];
        let decoded = decode(&bytes, &State::default(), &config, Input::Atom(14)).unwrap();
        let expected = SoundArea {
            level: 0,
            x: 2,
            z: 3,
            sound: -1,
            loops: 5,
            radius: 4,
            delay: 7,
            volume: 8,
            rate: 0x1234,
            dialog: false,
        };
        assert_eq!(decoded.state.sounds, vec![expected.clone()]);
        // The dialog variant (ordinal 6, size 9): the same fields plus the
        // dialog-bus flag, also reachable inside UPDATE_ZONE_PARTIAL_ENCLOSED.
        for (flag, dialog) in [(1, true), (0, false), (2, false)] {
            let mut bytes = bytes.to_vec();
            bytes.push(flag);
            let decoded = decode(&bytes, &State::default(), &config, Input::Atom(6)).unwrap();
            assert_eq!(
                decoded.state.sounds,
                vec![SoundArea {
                    dialog,
                    ..expected.clone()
                }]
            );
        }
        let mut enclosed = vec![128, 0, 0, 6];
        enclosed.extend(bytes);
        enclosed.push(1);
        let decoded = decode(&enclosed, &State::default(), &config, Input::Enclosed).unwrap();
        assert!(decoded.state.sounds[0].dialog);
        assert!(decode(&bytes, &State::default(), &config, Input::Atom(6)).is_err());
    }

    #[test]
    fn text_coord_retains_overlay_fields_and_expiry() {
        let map = Context {
            local: 0,
            base_x: 0,
            base_z: 0,
            width: 104,
            height: 104,
            bridges: vec![],
        };
        let objects = BTreeMap::new();
        let config = Config {
            map: &map,
            cycle: 40,
            cutscene: false,
            allow_outside: false,
            objects: &objects,
            scene: None,
            transients: None,
        };
        // An unused leading byte is consumed, then coord 0x23, duration,
        // height, g3 colour and a NUL-terminated CP-1252 string.
        let bytes = [
            0x7f, 0x23, 0x00, 0x0a, 0x06, 0x12, 0x34, 0x56, b'H', b'i', 0,
        ];
        let decoded = decode(&bytes, &State::default(), &config, Input::Atom(2)).unwrap();
        assert_eq!(
            decoded.state.text_coords,
            vec![TextCoord {
                level: 0,
                x: 2,
                z: 3,
                height: 6,
                expiry_cycle: 50,
                colour: 0x12_34_56,
                text: "Hi".into(),
            }]
        );
    }
}
