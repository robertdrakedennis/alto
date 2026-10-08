use crate::entities910::appearance::*;
use crate::protocol910::appearance::*;
pub fn config() -> Config {
    let defaults = Customisation {
        man: [10, 11, -1],
        woman: [20, -1, 22],
        man_head: [30, -1],
        woman_head: [40, 41],
        recolour: Some(vec![1, 2, 3, 4]),
        retexture: Some(vec![5, 6]),
    };
    Config {
        wear: vec![0, 1, 2, 0],
        colour_lengths: [4; 10],
        texture_lengths: [3; 10],
        items: [
            (
                1,
                Item {
                    team: 7,
                    defaults: defaults.clone(),
                },
            ),
            (2, Item { team: 14, defaults }),
        ]
        .into_iter()
        .collect(),
        npc_sizes: [(123, 3), (40000, 5)].into_iter().collect(),
        titles: Default::default(),
        default_titles: ["Sir <name>".into(), "Lady <name>".into()],
        staff_live_override: false,
    }
}
fn i(o: &mut Vec<u8>, v: i32) {
    o.extend(v.to_be_bytes())
}
fn string(o: &mut Vec<u8>, s: &Option<String>) {
    if let Some(s) = s {
        i(o, s.len() as i32);
        o.extend(s.as_bytes())
    } else {
        i(o, -1)
    }
}
fn shorts(o: &mut Vec<u8>, s: &Option<Vec<i16>>) {
    if let Some(s) = s {
        i(o, s.len() as i32);
        for v in s {
            o.extend(v.to_be_bytes())
        }
    } else {
        i(o, -1)
    }
}
pub fn dump(o: &mut Vec<u8>, state: &crate::protocol910::Players, index: usize) {
    for p in [&state.appearances[index], &state.head_icons[index]] {
        o.push(p.is_some() as u8);
        if let Some(p) = p {
            i(o, p.consumed as i32);
            i(o, p.data.len() as i32);
            o.extend(&p.data)
        }
    }
    i(o, state.low[index].as_ref().map_or(-1, |l| l.partner));
    let Some(e) = &state.players[index] else {
        return;
    };
    fields(o, e);
    i(o, e.partner);
    let a = &e.appearance;
    for v in [
        a.base_size,
        e.size,
        a.gender as i32,
        a.team,
        a.title_id,
        a.visibility.map_or(-1, |v| v as i32),
        a.combat,
        a.max_combat,
        a.wilderness,
        a.skill,
        a.bas,
        a.sound_range,
        a.sound_ids[0],
        a.sound_ids[1],
        a.sound_ids[2],
        a.sound_ids[3],
        a.sound_volume,
    ] {
        i(o, v)
    }
    string(o, &a.title);
    string(o, &a.name);
    for n in 0..8 {
        i(o, a.head_ids[n]);
        i(o, a.head_groups[n])
    }
    o.push(a.model.is_some() as u8);
    let Some(m) = &a.model else { return };
    i(o, m.bas);
    i(o, m.npc);
    o.push(m.female as u8);
    o.extend(m.hash.to_be_bytes());
    i(o, m.kits.len() as i32);
    for &v in m.kits.iter().chain(&m.colours).chain(&m.textures) {
        i(o, v)
    }
    for c in &m.custom {
        o.push(c.is_some() as u8);
        if let Some(c) = c {
            for &v in c
                .man
                .iter()
                .chain(&c.woman)
                .chain(&c.man_head)
                .chain(&c.woman_head)
            {
                i(o, v)
            }
            shorts(o, &c.recolour);
            shorts(o, &c.retexture)
        }
    }
}

pub fn fields(o: &mut Vec<u8>, e: &crate::entities910::Player) {
    for &v in e.forced.iter().chain(&e.tint) {
        i(o, v)
    }
    i(o, e.wear.as_ref().map_or(-1, |w| w.len() as i32));
    if let Some(w) = &e.wear {
        for &v in w {
            i(o, v)
        }
    }
}
