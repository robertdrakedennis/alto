//! Tests of `rs910-scene` modules that need client910 fixtures
//! (`recorded_golden`, `test_support::oracle_env`) or run in the replay gate
//! (`tools/refactor/replay-gate.txt`, client910's lib), so they stay in this
//! package (tools/README.md "Tests"). Moved from each module's `tests`
//! (Phase 2.8); each submodule globs the module it tests, like the
//! `use super::*` it came from.

mod draw_trace {
    use crate::draw_trace::*;

    #[test]
    fn detects_mutated_missing_reordered_and_malformed_sections() {
        let mut a = Trace::default();
        a.push("frame/0/corners", vec![0, -1, 32]);
        a.push("frame/0/opaque", vec![4, 123, 7, 123]);
        let encoded = a.encode();
        let mut b = Trace::decode(&encoded).unwrap();
        a.compare(&b).unwrap();
        b.0[0].1[1] = 0;
        assert!(a.compare(&b).unwrap_err().contains("corners word 1"));
        b = Trace::decode(&encoded).unwrap();
        b.0.swap(0, 1);
        assert!(a.compare(&b).is_err());
        b.0.pop();
        assert!(a.compare(&b).is_err());
        for len in 0..encoded.len() {
            assert!(Trace::decode(&encoded[..len]).is_err());
        }
        let mut bad = encoded.clone();
        bad.push(0);
        assert!(Trace::decode(&bad).is_err());
        let mut duplicate = Trace::default();
        duplicate.push("x", vec![]);
        duplicate.push("x", vec![]);
        assert!(Trace::decode(&duplicate.encode()).is_err());
    }
}

mod draw {
    use crate::draw::*;
    use crate::draw_trace::Trace;
    use crate::floor::FloorHeights;
    use crate::scene::Scene;

    /// A trace grouped by frame: `frame/<n>/...` sections concatenated in
    /// order under `<kind>:frame/<n>`, other sections under `<kind>:<name>`.
    fn by_frame(kind: &str, trace: &Trace) -> Vec<(String, Vec<i32>)> {
        let mut out: Vec<(String, Vec<i32>)> = Vec::new();
        for (name, words) in &trace.0 {
            let mut parts = name.split('/');
            let key = match (parts.next(), parts.next()) {
                (Some("frame"), Some(n)) => format!("{kind}:frame/{n}"),
                _ => format!("{kind}:{name}"),
            };
            // Section names are part of the golden: fold them into the words.
            let mut w: Vec<i32> = name.bytes().map(i32::from).collect();
            w.push(words.len() as i32);
            w.extend(words);
            match out.last_mut() {
                Some((k, v)) if *k == key => v.extend(w),
                _ => out.push((key, w)),
            }
        }
        out
    }

    fn recorded_draw_frames() -> std::path::PathBuf {
        crate::recorded_golden::path("scene-draw-cameras.txt")
    }

    /// Whole-scene draw decisions for Lumbridge 3222,3222 under the 21
    /// recorded cameras (yaw 0/4096/8192/12288,
    /// pitch 1077-2787, viewports, roofs, occlusion modes): per frame the
    /// projected/culled/sorted entity lists, occlusion raster, floor tile
    /// selection and the submitted models, floor/light/shadow indices, vs the
    /// recorded scene draw (committed as
    /// `fixtures/recorded-goldens/scene-draw-lumbridge.txt`).
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn lumbridge_draw_decisions_match_the_recording() {
        let pack = crate::test_support::require_pack("client.mapsv2.js5");
        let flo = crate::flo::FloStore::load(&pack).unwrap();
        let tables = crate::maploader::FloTables::from_store(&flo);
        let materials = crate::texture::MaterialStore::load(&pack).unwrap();
        let locs = crate::config::LocStore::load(&pack).unwrap();
        let mut world = crate::rebuild::rebuild_normal(
            &pack,
            &tables,
            &materials,
            Some(&locs),
            3222,
            3222,
            &crate::rebuild::BuildPrefs::default(),
        )
        .unwrap();
        let traces = oracle_traces(&mut world, &recorded_draw_frames(), &materials).unwrap();
        let golden = crate::recorded_golden::Golden::load("scene-draw-lumbridge.txt");
        let groups: Vec<_> = by_frame("frames", &traces.frames)
            .into_iter()
            .chain(by_frame("submissions", &traces.submissions))
            .collect();
        assert!(groups.len() > 40, "{} groups", groups.len());
        let errors: Vec<String> = groups
            .iter()
            .filter_map(|(name, words)| golden.compare(name, words).err())
            .collect();
        assert!(
            errors.is_empty(),
            "{} of {} draw groups differ from the recording:\n{}",
            errors.len(),
            groups.len(),
            errors.join("\n")
        );
    }

    /// The underwater draw lists: the
    /// underwater entities are depth-projected, frustum-tested and sorted
    /// with the normal lists' quicksorts (opaque near-first, transparent
    /// far-first), without touching the normal lists.
    #[test]
    fn underwater_lists_follow_build_draw_lists() {
        let scene = Scene::new(9, 4, 104, 104);
        let heights = vec![FloorHeights::new(104, 104, 512, vec![0; 105 * 105]); 4];
        let uw_heights = vec![FloorHeights::new(104, 104, 512, vec![0; 105 * 105])];
        let mut occlusion = crate::occlusion::Occlusion::new(&scene, &heights);
        let eye = [52 * 512 + 256, -600, 52 * 512 + 256];
        let mut entities = Vec::new();
        // Four rays of three entities each, alternately opaque/transparent.
        let rays = [[1, 0], [-1, 0], [0, 1], [0, -1]];
        for (r, [dx, dz]) in rays.iter().enumerate() {
            for step in 1..=3 {
                let id = entities.len() as i32;
                let (x, z) = (eye[0] + dx * step * 1024, eye[2] + dz * step * 1024);
                let transparent = (step % 2 == 0) as i32;
                entities.push(DrawEntity {
                    source: crate::scene::EntityRef::Scenery(id as usize),
                    dynamic: false,
                    bounds: None,
                    wall_type: 0,
                    cylinder: Some([x, 0, z, -200, 0, 100]),
                    position: None,
                    precise_cylinder: None,
                    id,
                    bucket: transparent,
                    kind: 0,
                    loc_id: 0,
                    shape: 10,
                    angle: 0,
                    level: 0,
                    occlude_level: 0,
                    x,
                    y: 0,
                    z,
                    tiles: [x >> 9, x >> 9, z >> 9, z >> 9],
                    overlay_height: 0,
                    transparent: transparent != 0,
                });
                let _ = r;
            }
        }
        let a = [
            0, eye[0], eye[1], eye[2], 0, 0, 512, 334, 0, 0, 512, 334, 50, 3500, -1, 1, 0, 0, 0,
        ];
        let mut state = DrawState::new(32);
        state.live_frame(
            crate::draw::PlannerScene {
                scene: &scene,
                heights: &heights,
                entities: &[],
            },
            crate::draw::DrawFrame::from_words(a),
            None,
            &mut occlusion,
            crate::draw::LiveInputs {
                boxes: &[],
                cam2: None,
                underwater: Some((&entities, &uw_heights)),
            },
        );
        let plan = &state.plan;
        assert!(plan.opaque.is_empty() && plan.transparent.is_empty());
        let drawn: Vec<usize> = plan
            .underwater_opaque
            .iter()
            .chain(&plan.underwater_transparent)
            .copied()
            .collect();
        assert!(!drawn.is_empty());
        // Only the ray the camera faces is drawn.
        let ray = drawn[0] / 3;
        assert!(drawn.iter().all(|&id| id / 3 == ray), "{drawn:?}");
        assert!(plan
            .underwater_opaque
            .iter()
            .all(|&id| entities[id].bucket == 0));
        assert!(plan
            .underwater_transparent
            .iter()
            .all(|&id| entities[id].bucket == 1));
        // Opaque near-first, transparent far-first (step = id % 3 + 1).
        assert!(plan
            .underwater_opaque
            .windows(2)
            .all(|w| w[0] % 3 < w[1] % 3));
        assert!(plan
            .underwater_transparent
            .windows(2)
            .all(|w| w[0] % 3 > w[1] % 3));
        // Without an underwater scene the lists stay empty.
        state.live_frame(
            crate::draw::PlannerScene {
                scene: &scene,
                heights: &heights,
                entities: &[],
            },
            crate::draw::DrawFrame::from_words(a),
            None,
            &mut occlusion,
            crate::draw::LiveInputs {
                boxes: &[],
                cam2: None,
                underwater: None,
            },
        );
        assert!(state.plan.underwater_opaque.is_empty());
    }
}

mod occlusion_fixtures {
    pub use crate::occlusion_fixtures::*;

    mod tests {
        use crate::draw_trace::Trace;

        /// The raster corpus grouped per case: the five passes of `raster/<c>`
        /// (mode, inputs, result, coverage, 32x24 depth rows) as one array.
        fn by_case(trace: &Trace) -> Vec<(String, Vec<i32>)> {
            let mut out: Vec<(String, Vec<i32>)> = Vec::new();
            for (name, words) in &trace.0 {
                let key = match name.strip_prefix("raster/") {
                    Some(rest) => format!("raster/{}", rest.split('/').next().unwrap()),
                    None => name.clone(),
                };
                match out.last_mut() {
                    Some((k, w)) if *k == key => w.extend(words),
                    _ => out.push((key, words.clone())),
                }
            }
            out
        }

        /// The occlusion raster over the 304-case corpus (overflow,
        /// vertex order, clipping, five depth passes each) and the
        /// tile-visibility gates vs the recording
        /// (committed as
        /// `fixtures/recorded-goldens/occlusion-raster.txt`).
        #[test]
        fn raster_rows_match_the_recording() {
            let golden = crate::recorded_golden::Golden::load("occlusion-raster.txt");
            let cases = by_case(&super::raster_trace());
            assert_eq!(cases.len(), 305);
            let errors: Vec<String> = cases
                .iter()
                .filter_map(|(name, words)| golden.compare(name, words).err())
                .collect();
            assert!(
                errors.is_empty(),
                "{} of {} raster cases differ from the recording:\n{}",
                errors.len(),
                cases.len(),
                errors[..errors.len().min(8)].join("\n")
            );
        }
    }
}

mod minimap {
    use crate::config::LocStore;
    use crate::minimap::*;

    /// Six words per non-icon mark, as the recording of the original toolkit
    /// calls holds them: `{0, x, y, w, h, colour}` for a fill
    /// (vertical line, horizontal line or rectangle fill) and
    /// `{1, x0, y0, x1, y1, colour}` for a line. Icons (no sprite archive in
    /// the recorded run) and the final additive noise fill are not recorded.
    fn mark_words(plan: &BasePlan) -> (Vec<i32>, [i32; 2]) {
        let mut words = Vec::new();
        let mut counts = [0; 2];
        for m in &plan.marks {
            match m {
                Mark::Fill { rect, colour } => {
                    words.extend([0, rect[0], rect[1], rect[2], rect[3], *colour]);
                    counts[0] += 1;
                }
                Mark::Line { from, to, colour } => {
                    words.extend([1, from[0], from[1], to[0], to[1], *colour]);
                    counts[1] += 1;
                }
                Mark::Icon { .. } | Mark::Add { .. } => {}
            }
        }
        (words, counts)
    }

    /// `rebuildMinimapBase` wall/loc marks over the real Lumbridge window
    /// (production rebuild + LAND terrain flags) for player levels 0 and 1
    /// vs the recorded wall and loc marks over the same scene
    /// (`fixtures/recorded-goldens/minimap-lumbridge.txt`): mark order,
    /// geometry and the wall/active/diagonal colours.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn lumbridge_base_marks_match_the_recording() {
        let pack = crate::test_support::require_pack("client.mapsv2.js5");
        let flo = crate::flo::FloStore::load(&pack).unwrap();
        let tables = crate::maploader::FloTables::from_store(&flo);
        let materials = crate::texture::MaterialStore::load(&pack).unwrap();
        let locs = LocStore::load(&pack).unwrap();
        let world = crate::rebuild::rebuild_normal(
            &pack,
            &tables,
            &materials,
            Some(&locs),
            3222,
            3222,
            &crate::rebuild::BuildPrefs::default(),
        )
        .unwrap();
        let mut terrain = crate::protocol910::terrain::Terrain::new(104, 104).unwrap();
        for &square in &world.squares {
            let (mx, mz) = ((square >> 8) as i32, (square & 0xFF) as i32);
            let group = (square >> 8) | ((square & 0xFF) << 7);
            let land = pack
                .read_group("mapsv2", group)
                .unwrap()
                .remove(&3)
                .unwrap();
            terrain = terrain
                .read_normal(
                    &land,
                    mx * 64 - world.base_x,
                    mz * 64 - world.base_z,
                    world.base_x,
                    world.base_z,
                )
                .unwrap()
                .state;
        }
        // The recorded scene builds its loc list with the members flag set.
        locs.allow_members.set(true);
        let mut minimap = Minimap {
            locs: Some(locs),
            ..Minimap::default()
        };
        let scene = world.scene_graph.as_ref().unwrap();
        let golden = crate::recorded_golden::Golden::load("minimap-lumbridge.txt");
        for level in [0, 1] {
            let plan = minimap.base_plan(scene, &terrain, level, None);
            let (words, counts) = mark_words(&plan);
            golden.check(&format!("minimap/L{level}/counts"), &counts);
            golden.check(&format!("minimap/L{level}/ops"), &words);
        }
    }
}

mod maploader {
    use crate::maploader::*;
    use rs910_core::perlin::perlin;

    /// The perlin noise over a 200x200 window and `blendColours` over a
    /// lattice vs the original client (committed
    /// as `fixtures/recorded-goldens/floor-oracle.txt`).
    #[test]
    fn perlin_and_blend_colours_match_the_recording() {
        let golden = crate::recorded_golden::Golden::load("floor-oracle.txt");
        let mut noise = Vec::new();
        for z in 3100..3300 {
            for x in 3100..3300 {
                noise.push(perlin(x + 932_731, z + 556_238));
            }
        }
        golden.check("perlin", &noise);
        let mut blend = Vec::new();
        for a in (0..65536).step_by(1021) {
            for b in (0..65536).step_by(977) {
                for t in [0, 32, 64, 96, 128] {
                    blend.push(blend_colours(a, b, t));
                }
            }
        }
        golden.check("blendColours", &blend);
    }
}

mod camera {
    use crate::camera::*;

    fn entry_bits(m: &Matrix4x3) -> Vec<i32> {
        m.e.iter().map(|v| v.to_bits() as i32).collect()
    }

    /// Rotation about an axis, about four axes and angles after a
    /// translation and a yaw, vs the recording
    /// (`fixtures/recorded-goldens/floor-oracle.txt`, `matrix_axes`).
    #[test]
    fn rotate_around_axis_matches_the_recording() {
        let golden = crate::recorded_golden::Golden::load("floor-oracle.txt");
        let mut got = Vec::new();
        let axes = [
            [0.6, 0.0, 0.8],
            [0.0, 0.8, -0.6],
            [0.48, 0.6, 0.64],
            [-1.0, 0.0, 0.0],
        ];
        for [x, y, z] in axes {
            for angle in [0.3_f32, 1.7, -2.2, 3.0] {
                let mut m = Matrix4x3::translation(10.5, -20.25, 300.0);
                m.rotate_around_axis(0.0, 1.0, 0.0, 0.4);
                m.rotate_around_axis(x, y, z, angle);
                got.extend(entry_bits(&m));
            }
        }
        golden.check("matrix_axes", &got);
    }

    /// The classic camera view matrix for an explicit pose over a
    /// pitch/yaw/roll grid vs the recording (`matrix_camera`: camera
    /// x/y/z = 1650624, -1200, 1649856).
    #[test]
    fn classic_view_matrix_matches_the_recording() {
        let golden = crate::recorded_golden::Golden::load("floor-oracle.txt");
        let mut got = Vec::new();
        for pitch in [0, 128, 1500, 3000, 12000] {
            for yaw in [0, 1000, 5000, 9000, 15000] {
                for roll in [0, 700] {
                    let mut cam = SceneCamera::new([0, 0, 0]);
                    cam.legacy = Some(LegacyFrame {
                        eye: [1_650_624, -1200, 1_649_856],
                        pitch,
                        yaw,
                        roll,
                    });
                    got.extend(entry_bits(&cam.view_matrix()));
                }
            }
        }
        golden.check("matrix_camera", &got);
    }
}

mod skybox {
    use crate::skybox::*;

    /// The skybox model view over a pitch/yaw/roll grid vs the recorded
    /// matrix/trig sequence
    /// (`fixtures/recorded-goldens/floor-oracle.txt`, `matrix_skybox`).
    #[test]
    fn model_view_matches_the_recording() {
        let golden = crate::recorded_golden::Golden::load("floor-oracle.txt");
        let mut got = Vec::new();
        for pitch in [0, 128, 1500, 3000, 12000] {
            for yaw in [0, 1000, 5000, 9000, 15000] {
                for roll in [0, 700] {
                    let m = model_view(pitch, yaw, roll);
                    got.extend(m.e.iter().map(|v| v.to_bits() as i32));
                }
            }
        }
        golden.check("matrix_skybox", &got);
    }
}

mod ui_icon_model {
    use crate::cache::Pack;
    use crate::ui_icon_model::*;
    use anyhow::{Context, Result};

    /// Icon renders of ten inventory objects (three outline modes each) and the
    /// material colour table, against the frozen recording of the original
    /// client's model rasteriser.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn icon_models_match_the_recording() -> Result<()> {
        let pack = Pack::open(rs910_core::test_support::pack_root());
        let objs = crate::config::ObjStore::load(&pack)?;
        let models = Models::new(&pack)?;
        let mut colours = vec![];
        for id in 0..65536 {
            if let Some(mat) = models.materials.get(id) {
                for hsl in [0, 127, 1024, 12345, 32767, 65535] {
                    for bright in [2, 63, 126] {
                        colours.extend(material_colour(hsl, bright, mat).to_be_bytes());
                    }
                }
            }
        }
        let mut renders = vec![];
        for id in [995, 1004, 1205, 4151, 385, 554, 4152, 11694, 1050, 14484] {
            let obj = objs.get(id).context("obj fixture")?.clone();
            for outline in 0..3 {
                let pixels = models.render(&pack, &obj, outline, false, [1.; 3])?;
                renders.extend(pixels.iter().flat_map(|v| v.to_be_bytes()));
            }
        }
        use rs910_core::test_support::frozen::assert_stream;
        assert_stream("ui-icon-model/colours", &colours);
        assert_stream("ui-icon-model/renders", &renders);
        Ok(())
    }
}

mod light_animation {
    use crate::env::StaticLight;
    use rs910_scene::light_animation::*;

    fn light() -> StaticLight {
        StaticLight {
            level: 0,
            above: false,
            below: false,
            x: 0,
            y: 0,
            z: 0,
            radius: 1024,
            colour: 0xffffff,
            flicker: 2,
            phase: 0,
            wave: 1,
            offset: 0,
            amplitude: 2048,
            speed: 2048,
            group: -1,
            span_runs: vec![1],
        }
    }

    #[test]
    fn light_animation_matches_the_recording() -> anyhow::Result<()> {
        let expected: Vec<i32> =
            rs910_core::test_support::frozen::text("light-animation/noise.txt")
                .lines()
                .map(str::parse)
                .collect::<Result<_, _>>()?;
        assert_eq!(&noise()[..], expected);
        let mut count = 0;
        for line in rs910_core::test_support::frozen::text("light-animation/lights.csv").lines() {
            let a: Vec<i64> = line.split(',').map(str::parse).collect::<Result<_, _>>()?;
            let mut l = light();
            l.wave = a[0] as i32;
            l.phase = a[1] as i32;
            l.speed = a[2] as i32;
            l.amplitude = a[3] as i32;
            l.offset = a[4] as i32;
            assert_eq!(
                intensity(&l, a[5] as i32, a[6] != 0).to_bits(),
                a[7] as u32,
                "{line}"
            );
            count += 1;
        }
        for line in rs910_core::test_support::frozen::text("light-animation/cpu.csv").lines() {
            let a: Vec<u64> = line.split(',').map(str::parse).collect::<Result<_, _>>()?;
            assert_eq!(
                crate::graphics_runtime::cpu_sleeps(a[0] as i32),
                &a[1..],
                "{line}"
            );
        }
        eprintln!("2048 recorded noise samples and {count} light observations matched");
        Ok(())
    }
}
