//! Real-cache body construction through the recorded player-model builder.
use crate::{
    cache::Pack,
    gpumodel::GpuModel,
    player_model::{Inputs, Models},
    protocol910::sequence_types::Sequence,
};
use std::{collections::BTreeSet, io::Write};
pub(super) fn words(m: &GpuModel) -> Vec<i32> {
    let mut w = vec![
        m.flags,
        m.vertex_count_all,
        m.vertex_count,
        m.unique_count,
        m.face_count,
        m.draw_face_count,
        m.ambient as i32,
        m.contrast as i32,
        m.has_transparency as i32,
        m.has_animated_uvs as i32,
    ];
    for v in 0..m.vertex_count_all as usize {
        w.extend([
            m.vx[v],
            m.vy[v],
            m.vz[v],
            m.vertex_source_models.as_ref().map_or(-1, |p| p[v] as i32),
        ]);
    }
    for v in 0..m.unique_count as usize {
        w.extend([
            m.unique_vertex[v] as i32,
            m.unique_face[v] as i32,
            m.nx[v] as i32,
            m.ny[v] as i32,
            m.nz[v] as i32,
            m.ncount[v] as i32,
            m.u[v].to_bits() as i32,
            m.v[v].to_bits() as i32,
        ]);
    }
    for f in 0..m.face_count as usize {
        w.extend([
            m.idx1[f] as i32,
            m.idx2[f] as i32,
            m.idx3[f] as i32,
            m.face_colour[f] as i32,
            m.face_alpha[f] as i32,
            m.face_material[f] as i32,
            m.face_part.as_ref().map_or(-1, |p| p[f] as i32),
        ]);
    }
    for groups in [&m.vertex_groups, &m.face_groups] {
        w.push(groups.as_ref().map_or(-1, |g| g.len() as i32));
        if let Some(g) = groups {
            for group in g {
                w.push(group.len() as i32);
                w.extend(group.iter().map(|&v| v as i32));
            }
        }
    }
    for batch in m.batches() {
        w.extend([batch.0 as i32, batch.1, batch.2, batch.3, batch.4]);
    }
    let mut bounds = m.clone();
    w.extend([
        bounds.min_x(),
        bounds.max_x(),
        bounds.min_y(),
        bounds.max_y(),
        bounds.min_z(),
        bounds.max_z(),
        bounds.horizontal_radius(),
        bounds.radius(),
    ]);
    bounds.translate(0, 73, 0);
    w.extend([bounds.min_y(), bounds.height()]);
    w
}
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn real_cache_player_bodies() -> anyhow::Result<()> {
    let root = rs910_core::test_support::repo_root();
    let scratch = rs910_core::test_support::frozen::Scratch::new("player-models");
    let out = scratch.dir().to_path_buf();
    std::fs::create_dir_all(&out)?;
    let pack = Pack::open(root.join("server/data/pack"));
    let mut inputs = crate::entity_runtime::Inputs::load(&pack, true, false, 50)?;
    // Explicit BAS inputs cover rotations absent on the current server stance.
    // The recording received these same fields; all body model bytes still come from cache.
    for (i, angles) in [
        [0, 0, 0],
        [256, 0, 0],
        [0, 256, 0],
        [0, 0, 256],
        [127, 511, 901],
        [2047, 1, 2046],
    ]
    .into_iter()
    .enumerate()
    {
        let mut b = crate::protocol910::bas_types::Bas::default();
        let mut slots = vec![None; inputs.appearance.defaults.wear.positions.len()];
        for slot in [8, 9, 16] {
            slots[slot] = Some(vec![-37, 73, 129, angles[0], angles[1], angles[2]]);
        }
        b.slot_transforms = Some(slots);
        inputs.bas.insert(900001 + i as i32, b);
    }
    let defaults = &inputs.appearance.defaults;
    let c = Inputs {
        items: &inputs.appearance.types.items,
        bases: &inputs.bas,
        wear: &defaults.wear,
        recolour: defaults.graphics.recolour.as_ref().unwrap(),
        retexture: defaults.graphics.retexture.as_ref().unwrap(),
    };
    let materials = crate::texture::MaterialStore::load(&pack)?;
    let billboards = crate::billboard::BillboardStore::load(&pack)?;
    let emitters = crate::particle::EmitterStore::load(&pack)?;
    let mut models = Models::load(&pack, 0x37)?;
    // Exact appearance bytes currently emitted by the server's PLAYER_INFO encoder.
    // Decode with the production cache-backed appearance context; do not guess
    // which wear slots the eight leading zero bytes occupy in this cache.
    let mut server = vec![0u8; 10];
    for id in [18u16, 26, 36, 0, 33, 42, 10] {
        server.extend((id | 256).to_be_bytes());
    }
    server.extend([0; 23]);
    server.extend(2699u16.to_be_bytes());
    server.extend(b"2004Scape\0");
    server.extend([138, 0, 0, 0]);
    let mut packet = crate::entities910::appearance::CachedPacket {
        data: server,
        consumed: 0,
    };
    let mut actor = crate::entities910::Player::default();
    crate::protocol910::appearance::apply(&mut packet, &mut actor, &inputs.appearance.appearance)
        .map_err(|e| anyhow::anyhow!("server appearance: {e:?}"))?;
    anyhow::ensure!(packet.consumed == packet.data.len(), "appearance tail");
    let base = actor.appearance.model.unwrap();
    println!("Server body: BAS {} kits {:?}", base.bas, base.kits);
    let identity: Vec<_> = models
        .identity
        .iter()
        .filter(|(_, i)| i.models.as_ref().is_some_and(|m| !m.is_empty()))
        .map(|(&id, _)| id)
        .collect();
    let items: Vec<_> = c
        .items
        .iter()
        .filter(|(_, i)| {
            i.custom.man[0] != -1 && i.wear[0] >= 0 && i.wear[0] < base.kits.len() as i32
        })
        .step_by(127)
        .map(|(&id, _)| id)
        .take(60)
        .collect();
    let mut cases = vec![(base.clone(), -1, -1, 0x820, true)];
    for &id in identity.iter().step_by(17) {
        let mut a = base.clone();
        a.kits[4] = i32::MIN | id;
        cases.push((a, -1, -1, 0x820, true));
    }
    for id in items {
        let item = &c.items[&id];
        let mut a = base.clone();
        a.kits[item.wear[0] as usize] = id | 0x40000000;
        cases.push((a.clone(), -1, -1, 0x820, true));
        a.female = true;
        cases.push((a.clone(), -1, -1, 0x100920, true));
        a.female = false;
        let mut custom = item.custom.clone();
        if let Some(recol) = &mut custom.recolour {
            for v in recol {
                *v = v.wrapping_add(127);
            }
        }
        a.custom[item.wear[0] as usize] = Some(custom);
        cases.push((a, -1, -1, 0x820, true));
    }
    for main in [-1, 65535, 1277, 4151] {
        for off in [-1, 65535, 1540] {
            let mut a = base.clone();
            a.kits[5] = 1277 | 0x40000000;
            a.kits[3] = 1540 | 0x40000000;
            cases.push((a, main, off, 0x820, true));
        }
    }
    for v in 0..20 {
        let mut a = base.clone();
        a.bas = if v % 2 == 0 { 0 } else { 2699 };
        for i in 0..10 {
            a.colours[i] = (v as usize % c.recolour.destinations[i][0].len().max(1)) as i32;
            a.textures[i] = (v as usize % c.retexture.destinations[i][0].len().max(1)) as i32;
        }
        cases.push((a, -1, -1, 0x820, v % 3 != 0));
    }
    for id in 900001..=900006 {
        let mut a = base.clone();
        a.bas = id;
        cases.push((a, -1, -1, 0x820, true));
    }
    // Cache fallback: a valid prior model, then an unavailable replacement.
    cases.push((base.clone(), -1, -1, 0x100920, true));
    let mut missing = base.clone();
    missing.kits[5] = 1277 | 0x40000000;
    let mut custom = c.items[&1277].custom.clone();
    custom.man[0] = 999999;
    missing.custom[5] = Some(custom);
    cases.push((missing.clone(), -1, -1, 0x820, true));
    cases.push((missing, -1, -1, 0x40000000, true));
    let mut request = std::io::BufWriter::new(std::fs::File::create(out.join("requests.bin"))?);
    let mut result = std::io::BufWriter::new(std::fs::File::create(out.join("rust.bin"))?);
    let mut last = 0i64;
    let mut required = BTreeSet::new();
    request.write_all(&(cases.len() as i32).to_be_bytes())?;
    for (index, (mut a, main, off, flags, cache)) in cases.into_iter().enumerate() {
        a.update_hash();
        let mut seq = Sequence::empty(900000);
        seq.mainhand = main;
        seq.offhand = off;
        let mut q = vec![
            a.bas,
            a.female as i32,
            main,
            off,
            flags,
            cache as i32,
            a.kits.len() as i32,
        ];
        q.extend(&a.kits);
        q.extend(a.colours);
        q.extend(a.textures);
        for custom in &a.custom {
            q.push(custom.is_some() as i32);
            if let Some(custom) = custom {
                q.extend(custom.man);
                q.extend(custom.woman);
                q.extend(custom.man_head);
                q.extend(custom.woman_head);
                for a in [&custom.recolour, &custom.retexture] {
                    q.push(a.as_ref().map_or(-1, |a| a.len() as i32));
                    if let Some(a) = a {
                        q.extend(a.iter().map(|&v| v as i32));
                    }
                }
            }
        }
        if a.bas >= 900001 {
            let slots = c.bases[&a.bas].slot_transforms.as_ref().unwrap();
            q.push(slots.len() as i32);
            for slot in slots {
                q.push(slot.is_some() as i32);
                if let Some(t) = slot {
                    q.extend(t);
                }
            }
        } else {
            q.push(-1);
        }
        request.write_all(&(q.len() as i32).to_be_bytes())?;
        for v in q {
            request.write_all(&v.to_be_bytes())?;
        }
        // Export the exact resource closure, including all configured replacement
        // models used by hand overrides, customisations and identity-kit parts.
        let selection = crate::player_model::select(&a, Some(&seq), c.wear)?;
        for &id in a.kits.iter().chain(&selection.kits) {
            if id & 0x40000000 != 0 {
                if let Some(t) = c.items.get(&(id & 0x3fffffff)) {
                    required.extend(t.custom.man);
                    required.extend(t.custom.woman);
                }
            } else if id & i32::MIN != 0 {
                if let Some(m) = models
                    .identity
                    .get(&(id & 0x3fffffff))
                    .and_then(|i| i.models.as_ref())
                {
                    required.extend(m);
                }
            }
        }
        for custom in a.custom.iter().flatten() {
            required.extend(custom.man);
            required.extend(custom.woman);
        }
        let built = models.body(
            &pack,
            &a,
            Some(&seq),
            &c,
            &crate::gpumodel::ModelStores {
                materials: &materials,
                billboards: &billboards,
                emitters: &emitters,
            },
            crate::player_model::BodyBuild {
                flags,
                cache,
                last_key: &mut last,
            },
        )?;
        let w = built.as_ref().map(words).unwrap_or_default();
        result.write_all(&(w.len() as i32).to_be_bytes())?;
        for v in w {
            result.write_all(&v.to_be_bytes())?;
        }
        result.write_all(&last.to_be_bytes())?;
        println!(
            "body {index}: {}",
            built.as_ref().map_or(0, |m| m.unique_count)
        );
    }
    request.flush()?;
    result.flush()?;
    scratch.finish("player-models", &[("rust.bin", "recording")]);
    Ok(())
}
