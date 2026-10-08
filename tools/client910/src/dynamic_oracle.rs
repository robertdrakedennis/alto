//! Full cache-backed dynamic loc model builds, including variables and shadows.
use crate::{draw_trace::Trace, dynamic_scene::DynamicScene};
fn packed(bytes: &[u8]) -> Vec<i32> {
    let mut out = vec![bytes.len() as i32];
    for c in bytes.chunks(4) {
        let mut b = [0; 4];
        b[..c.len()].copy_from_slice(c);
        out.push(i32::from_le_bytes(b));
    }
    out
}
/// The sign of a NaN depends on the platform that computed it, so the frozen
/// recording and the port's stream both carry the canonical quiet NaN.
fn canonical_nan(trace: &mut Trace) {
    for (_, words) in &mut trace.0 {
        for w in words {
            let bits = *w as u32;
            if bits & 0x7f80_0000 == 0x7f80_0000 && bits & 0x007f_ffff != 0 {
                *w = 0x7fc0_0000;
            }
        }
    }
}
/// Every dynamic loc of the Lumbridge scene over six cycles, variable states
/// and shadows, against the frozen recording of the original client's dynamic
/// loc model (NaN payloads canonicalised).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn export_dynamic_oracle() {
    let pack = crate::cache::Pack::open(rs910_core::test_support::pack_root());
    let materials = crate::texture::MaterialStore::load(&pack).unwrap();
    let locs = crate::config::LocStore::load(&pack).unwrap();
    let flo = crate::flo::FloStore::load(&pack).unwrap();
    let tables = crate::maploader::FloTables::from_store(&flo);
    let (cx, cz) = ("3222", "3222");
    let mut world = crate::rebuild::rebuild_normal(
        &pack,
        &tables,
        &materials,
        Some(&locs),
        cx.parse().unwrap(),
        cz.parse().unwrap(),
        &Default::default(),
    )
    .unwrap();
    let scene = world.scene_graph.as_mut().unwrap();
    let mut entities = crate::draw::entities(scene);
    let mut dynamic =
        DynamicScene::new(&pack, &locs, &entities, 910, world.model_cache.clone()).unwrap();
    let sun = crate::env::Environment::default().sun_lighting(world.env.sun_direction, 3, 0.);
    let mut trace = Trace::default();
    for (step, cycle) in [0, 1, 9, 33, 34, 200].into_iter().enumerate() {
        if step >= 2 {
            dynamic.variables.current.fill(if step == 2 {
                1
            } else if step == 3 {
                2
            } else {
                -1
            });
        }
        for (ordinal, (id, e)) in entities
            .iter_mut()
            .enumerate()
            .filter(|(_, e)| e.dynamic)
            .enumerate()
        {
            let seq = dynamic.animation_id(id);
            if step == 1 && seq != -1 && ordinal % 13 == 0 {
                dynamic.start_animation(id, e, cycle, seq, 3).unwrap();
            }
            let draw = step != 0 && (step != 3 || ordinal % 3 != 0);
            dynamic
                .refresh(
                    id,
                    draw,
                    cycle,
                    e,
                    crate::dynamic_scene::RefreshWorld {
                        scene,
                        floors: &mut world.scene.normal,
                        materials: &materials,
                        sun: &sun,
                    },
                )
                .unwrap();
            let mut words = dynamic.oracle_state(id);
            if words.pop() == Some(1) {
                let m = crate::dynamic_scene::model_mut(scene, e.source)
                    .as_mut()
                    .unwrap();
                words.extend([
                    1,
                    m.vertex_count_all,
                    m.vertex_count,
                    m.unique_count,
                    m.face_count,
                    m.draw_face_count,
                    i32::from(m.has_transparency),
                ]);
                words.extend(&m.vx[..m.vertex_count_all as usize]);
                words.extend(&m.vy[..m.vertex_count_all as usize]);
                words.extend(&m.vz[..m.vertex_count_all as usize]);
                if draw {
                    for a in [&m.nx, &m.ny, &m.nz, &m.face_colour] {
                        words.extend(a.iter().map(|&n| n as i32));
                    }
                    words.extend(m.face_alpha.iter().map(|&n| n as i32));
                }
                words.extend([
                    m.min_x(),
                    m.max_x(),
                    m.min_y(),
                    m.max_y(),
                    m.min_z(),
                    m.max_z(),
                    m.horizontal_radius(),
                    m.radius(),
                ]);
                if draw {
                    words.extend(m.colour_stream(&materials).unwrap());
                    for stream in [m.position_stream(), m.normal_stream()] {
                        words.extend(stream.iter().flatten().map(|v| v.to_bits() as i32));
                    }
                    words.extend(m.uv_stream().iter().flatten().map(|v| v.to_bits() as i32));
                    words.extend(m.index_stream().iter().map(|&v| i32::from(v)));
                    let batches = m.batches();
                    words.push(batches.len() as i32);
                    for (mat, start, count, min, span) in batches {
                        words.extend([i32::from(mat), start, count, min, span]);
                    }
                }
            } else {
                words.push(0);
            }
            trace.push(format!("step/{step}/loc/{ordinal}"), words);
        }
        for (level, f) in world.scene.normal.iter().enumerate() {
            trace.push(
                format!("step/{step}/shadow/{level}"),
                packed(
                    f.as_ref()
                        .and_then(|f| f.hard_shadows.as_ref())
                        .map_or(&[], |s| s.mask.as_slice()),
                ),
            );
        }
    }
    canonical_nan(&mut trace);
    rs910_core::test_support::frozen::assert_stream("dynamic/recording", &trace.encode());
}
