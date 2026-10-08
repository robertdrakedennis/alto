//! Lumbridge scene-build fingerprint against the recording of the original client.
//!
//! The original client built the 104x104 window around 3222,3222 and dumped
//! its floors, scene graph, static lights and environment once.
//! [`fingerprint`] projects those dumps into named sections —
//! per-level vertex/height/colour hashes, the floor batch order, loc counts
//! per level and layer, a canonical hash of every placed entity and the
//! positions of a few single-instance locs — committed as
//! `fixtures/recorded-goldens/lumbridge-scene.txt`. The test builds the same
//! window through the production `rebuild::rebuild_normal`, serialises it
//! with the same dump formats (`FloorGeometry::to_dump`, `Scene::to_dump`,
//! `floorlight::to_dump`, `EnvironmentGrid::to_dump`) and compares.
//!
//! The recorded floor dump carries a second, floor-sweep-only shadow mask that
//! the Rust dump does not model (see `bin/floordiff.rs`); only the full mask
//! is fingerprinted. Scene records are matched in a canonical order with NaN
//! payloads folded, as `bin/scenediff.rs` does.

#![cfg(test)]

use crate::recorded_golden;

/// Big-endian reader over a dump.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn bytes(&mut self, n: usize) -> &'a [u8] {
        let b = &self.data[self.pos..self.pos + n];
        self.pos += n;
        b
    }
    fn u8(&mut self) -> u8 {
        self.bytes(1)[0]
    }
    fn u16(&mut self) -> u16 {
        let b = self.bytes(2);
        u16::from_be_bytes([b[0], b[1]])
    }
    fn i32(&mut self) -> i32 {
        let b = self.bytes(4);
        i32::from_be_bytes([b[0], b[1], b[2], b[3]])
    }
    /// A float's bits with every NaN folded to one pattern.
    fn f32_bits(&mut self) -> i32 {
        let v = self.i32();
        if f32::from_bits(v as u32).is_nan() {
            0x7FC0_0000
        } else {
            v
        }
    }
    fn done(&self) -> bool {
        self.pos == self.data.len()
    }
}

/// Bytes packed big-endian four to a word (zero padded), after the length.
fn byte_words(bytes: &[u8]) -> Vec<i32> {
    let mut out = vec![bytes.len() as i32];
    out.extend(bytes.chunks(4).map(|c| {
        let mut w = [0; 4];
        w[..c.len()].copy_from_slice(c);
        i32::from_be_bytes(w)
    }));
    out
}

/// Sections of one floor dump (`floordiff` layout).
fn floor_sections(level: usize, data: &[u8], out: &mut Vec<(String, Vec<i32>)>) {
    let mut c = Cursor { data, pos: 0 };
    let vertex_count = c.i32() as usize;
    let stride = c.i32() as usize;
    let flags = c.i32();
    let stream: Vec<i32> = (0..vertex_count * stride).map(|_| c.f32_bits()).collect();
    let base: Vec<i32> = (0..vertex_count).map(|_| c.i32()).collect();
    let batch_count = c.i32() as usize;
    let mut order = Vec::new();
    let mut batch_words = Vec::new();
    for _ in 0..batch_count {
        let material = c.i32();
        let scale = c.f32_bits();
        let fog: Vec<i32> = (0..7).map(|_| c.i32()).collect();
        let node_hi = c.i32();
        let node_lo = c.i32();
        order.extend([material, node_hi, node_lo]);
        batch_words.push(scale);
        batch_words.extend(fog);
        batch_words.extend((0..vertex_count).map(|_| c.i32()));
    }
    let mut mask = vec![0];
    if !c.done() && c.u8() != 0 {
        let width = c.i32();
        let height = c.i32();
        mask = vec![width, height];
        mask.extend(byte_words(c.bytes((width * height) as usize)));
    }
    // The optional floor-sweep-only mask that follows is not compared.
    let heights: Vec<i32> = stream
        .iter()
        .skip(1)
        .step_by(stride.max(1))
        .copied()
        .collect();
    let p = format!("floor/L{level}");
    out.push((
        format!("{p}/header"),
        vec![
            vertex_count as i32,
            stride as i32,
            flags,
            batch_count as i32,
        ],
    ));
    out.push((format!("{p}/heights"), heights));
    out.push((format!("{p}/stream"), stream));
    out.push((format!("{p}/base_colours"), base));
    out.push((format!("{p}/batch_order"), order));
    out.push((format!("{p}/batches"), batch_words));
    out.push((format!("{p}/shadow_mask"), mask));
}

/// One canonical scene record: its sort key and every word.
struct Record {
    key: (u8, u16, u16, u8, i32),
    shape_angle: (i32, i32),
    words: Vec<i32>,
}

/// Sections of the scene dump (`scenediff` layout).
fn scene_sections(data: &[u8], out: &mut Vec<(String, Vec<i32>)>) {
    let mut c = Cursor { data, pos: 0 };
    assert_eq!(c.bytes(4), b"SCN1");
    let levels = c.i32();
    let max_x = c.i32();
    let max_z = c.i32();
    let tiles = c.bytes((levels * max_x * max_z) as usize).to_vec();
    let count = c.i32() as usize;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let layer = c.u8();
        let plane = c.u8();
        let x = c.u16();
        let z = c.u16();
        let ints: Vec<i32> = (0..12).map(|_| c.i32()).collect();
        let mut words = vec![
            i32::from(layer),
            i32::from(plane),
            i32::from(x),
            i32::from(z),
        ];
        words.extend(&ints);
        words.extend((0..7).map(|_| c.f32_bits()));
        words.push(i32::from(c.u8()));
        let has_model = c.u8();
        words.push(i32::from(has_model));
        if has_model == 1 {
            words.push(i32::from(c.u8()));
            words.push(i32::from(c.u8()));
            let head: Vec<i32> = (0..6).map(|_| c.i32()).collect();
            let unique = head[3] as usize;
            let draw = head[5] as usize;
            words.extend(&head);
            words.extend((0..unique * 3).map(|_| c.f32_bits()));
            words.extend((0..unique).map(|_| c.i32()));
            words.extend((0..unique * 3).map(|_| c.f32_bits()));
            for _ in 0..unique {
                words.extend((0..3).map(|_| i32::from(c.u16() as i16)));
                words.push(i32::from(c.u8() as i8));
            }
            words.extend((0..unique * 2).map(|_| c.f32_bits()));
            words.extend((0..draw * 3).map(|_| i32::from(c.u16())));
            let nb = c.i32() as usize;
            words.push(nb as i32);
            words.extend((0..nb * 5).map(|_| c.i32()));
        }
        records.push(Record {
            key: (plane, x, z, layer, ints[0]),
            shape_angle: (ints[1], ints[2]),
            words,
        });
    }
    assert!(c.done(), "trailing scene dump bytes");
    records.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.words.cmp(&b.words)));
    out.push((
        "scene/header".into(),
        vec![levels, max_x, max_z, count as i32],
    ));
    out.push((
        "scene/tiles".into(),
        tiles.iter().map(|&t| i32::from(t)).collect(),
    ));
    // Placed entities per (level, layer); layers are the scene dump kinds.
    let mut counts = vec![0; levels as usize * 8];
    for r in &records {
        let (plane, _, _, layer, _) = r.key;
        assert!(layer < 8, "scene dump layer {layer}");
        counts[plane as usize * 8 + layer as usize] += 1;
    }
    out.push(("scene/layer_counts".into(), counts));
    for plane in 0..levels as u8 {
        let words: Vec<i32> = records
            .iter()
            .filter(|r| r.key.0 == plane)
            .flat_map(|r| r.words.iter().copied())
            .collect();
        out.push((format!("scene/records/L{plane}"), words));
    }
    // The first sixteen loc ids placed exactly once: where they stand.
    let mut per_id = std::collections::BTreeMap::<i32, Vec<&Record>>::new();
    for r in &records {
        per_id.entry(r.key.4).or_default().push(r);
    }
    let named: Vec<i32> = per_id
        .values()
        .filter(|v| v.len() == 1)
        .take(16)
        .flat_map(|v| {
            let r = v[0];
            let (plane, x, z, layer, id) = r.key;
            [
                id,
                i32::from(plane),
                i32::from(x),
                i32::from(z),
                i32::from(layer),
                r.shape_angle.0,
                r.shape_angle.1,
            ]
        })
        .collect();
    out.push(("scene/single_locs".into(), named));
}

/// The fingerprint of a scene build: four floor dumps, the scene dump, the
/// four static-light dumps and the environment dump.
pub fn fingerprint(
    floors: &[Vec<u8>],
    scene: &[u8],
    lights: &[Vec<u8>],
    env: &[u8],
) -> Vec<(String, Vec<i32>)> {
    let mut out = Vec::new();
    for (level, f) in floors.iter().enumerate() {
        floor_sections(level, f, &mut out);
    }
    scene_sections(scene, &mut out);
    for (level, l) in lights.iter().enumerate() {
        out.push((format!("lights/L{level}"), byte_words(l)));
    }
    out.push(("env".into(), byte_words(env)));
    out
}

const GOLDEN: &str = "lumbridge-scene.txt";

/// `rebuild::rebuild_normal` over Lumbridge 3222,3222 with the default build
/// preferences (the `--dump-floor` path)
/// against the recorded fingerprint. Also checks the floor batch bookkeeping the
/// dumps do not carry: every drawn triangle has an opaque base batch.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn lumbridge_scene_build_matches_the_recording() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let flo = crate::flo::FloStore::load(&pack).unwrap();
    let tables = crate::maploader::FloTables::from_store(&flo);
    let materials = crate::texture::MaterialStore::load(&pack).unwrap();
    let locs = crate::config::LocStore::load(&pack).unwrap();
    let prefs = crate::rebuild::BuildPrefs::default();
    let world =
        crate::rebuild::rebuild_normal(&pack, &tables, &materials, Some(&locs), 3222, 3222, &prefs)
            .unwrap();
    let floors: Vec<Vec<u8>> = world
        .scene
        .normal
        .iter()
        .map(|g| g.as_ref().unwrap().to_dump())
        .collect();
    let scene = world
        .scene_graph
        .as_ref()
        .unwrap()
        .to_dump(&materials)
        .unwrap();
    let lights: Vec<Vec<u8>> = world
        .lights
        .iter()
        .map(|l| crate::floorlight::to_dump(l))
        .collect();
    let got = fingerprint(&floors, &scene, &lights, &world.env.to_dump());
    let golden = recorded_golden::Golden::load(GOLDEN);
    let errors: Vec<String> = got
        .iter()
        .filter_map(|(name, words)| golden.compare(name, words).err())
        .collect();
    assert!(
        errors.is_empty(),
        "{} of {} sections differ from the recording:\n{}",
        errors.len(),
        got.len(),
        errors.join("\n")
    );
    for (level, g) in world.scene.normal.iter().enumerate() {
        let g = g.as_ref().unwrap();
        for (tile, tris) in g.tile_tris.iter().enumerate() {
            let Some(tris) = tris else { continue };
            assert_eq!(tris.len() % 3, 0);
            for t in 0..tris.len() / 3 {
                let corner = |b: &crate::floor::FloorBatch, k: usize| {
                    (b.colours[tris[t * 3 + k] as usize] as u32) >> 24 == 0xFF
                };
                // Corners with `rgb == -1` (invisible overlays) have no
                // owner in the recording either.
                if !g.batches.iter().any(|b| (0..3).any(|k| corner(b, k))) {
                    continue;
                }
                assert!(
                    g.batches
                        .iter()
                        .any(|b| b.tri_mask[tile] & (1_u32 << t) != 0
                            && (0..3).all(|k| corner(b, k))),
                    "level {level} tile {tile} tri {t} has no opaque base batch"
                );
            }
        }
    }
}
