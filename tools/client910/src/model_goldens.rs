//! Tests of `rs910-model` modules that also need client910 modules or
//! fixtures (`recorded_golden`, `draw_trace`, `env`, `rebuild`, `maploader`,
//! `test_support::oracle_env`), so they stay in this package (tools/README.md
//! "Tests"). Moved from each module's `tests` (Phase 2.7); each submodule
//! globs the module it tests, like the `use super::*` it came from.

mod particle {
    use crate::cache::Pack;
    use crate::particle::*;
    use std::sync::Arc;

    /// The recorded scenario: one system with one emitter of type
    /// 356 (the torch emitter of model 2288, loc 724) on a fixed triangle,
    /// `tick(cycle)` then rebind every cycle, seeded like the
    /// recording's random source. Per cycle: live count and the FNV of the slot list
    /// (in slot order) as `x, y, z, colour, size, angle, texture`; and
    /// the last cycle's list in full.
    fn recorded_trace(rt: &mut Runtime, cycles: i64) -> (Vec<i32>, Vec<i32>) {
        let key = 1;
        let a = [EmitterAnchor {
            id: 1,
            particle: 356,
            vertices: [[5000, -300, 5000], [5064, -300, 5000], [5000, -300, 5064]],
        }];
        rt.bind(key, 0, 0, &a, &[]);
        let (mut per_cycle, mut last) = (Vec::new(), Vec::new());
        for cycle in 1..=cycles {
            rt.tick(cycle);
            rt.bind(key, cycle, 0, &a, &[]);
            let list = rt.slot_list(key);
            last = list
                .iter()
                .flat_map(|p| {
                    [
                        p.pos[0],
                        p.pos[1],
                        p.pos[2],
                        p.colour,
                        p.size,
                        i32::from(p.angle),
                        p.texture,
                    ]
                })
                .collect();
            let h = crate::recorded_golden::fnv(&last);
            per_cycle.extend([list.len() as i32, h as i32, (h >> 32) as i32]);
        }
        (per_cycle, last)
    }

    fn cache_runtime(pack: &Pack, rate: Option<i32>, seed: u64) -> Runtime {
        let mut emitters = EmitterStore::load(pack).unwrap();
        if let Some(rate) = rate {
            let mut t = emitters.get(356).clone();
            t.rate_min = rate;
            t.rate_max = rate;
            emitters.insert(356, Arc::new(t));
        }
        Runtime::new(emitters, EffectorStore::load(pack).unwrap(), seed)
    }

    /// 300 cycles of the cache torch emitter (spawn rate accumulator,
    /// barycentric spawn points, speed/size/colour ranges, the constant
    /// effector 9, colour/size fades, expiry) vs the recording
    /// (`fixtures/recorded-goldens/particles-torch.txt`).
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn torch_emitter_trace_matches_the_recording() {
        let pack = crate::test_support::require_pack("client.particles.js5");
        let golden = crate::recorded_golden::Golden::load("particles-torch.txt");
        let (per_cycle, last) = recorded_trace(&mut cache_runtime(&pack, None, 0x910), 300);
        golden.check("particles/torch/per_cycle", &per_cycle);
        golden.check("particles/torch/last", &last);
    }

    /// The torch type at 300 particles a cycle: from cycle 28 every spawn
    /// evicts the oldest slot occupant into the 1024-entry released ring, and
    /// each new particle takes the next released one's colour fraction. Slot
    /// order, live count and colours over 60 cycles vs the recording.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn slot_ring_eviction_and_pool_reuse_match_the_recording() {
        let pack = crate::test_support::require_pack("client.particles.js5");
        let golden = crate::recorded_golden::Golden::load("particles-torch.txt");
        let (per_cycle, last) =
            recorded_trace(&mut cache_runtime(&pack, Some(64 * 300), 0x911), 60);
        assert_eq!(last.len() / 7, SYSTEM_SLOTS);
        golden.check("particles/evict/per_cycle", &per_cycle);
        golden.check("particles/evict/last", &last);
    }
}

mod water {
    use crate::water::*;

    /// The gradient noise (419684) permutation and one noise slice, the water
    /// heights and the RGBA normal volume vs the recording
    /// (`fixtures/recorded-goldens/floor-oracle.txt`).
    #[test]
    fn noise_heights_and_normal_volume_match_the_recording() {
        let golden = crate::recorded_golden::Golden::load("floor-oracle.txt");
        let n = GradientNoise::new(419_684);
        golden.check("water_perm", n.perm());
        let mut out = vec![0.0; 128 * 128];
        let size = VolumeSize {
            width: 128,
            height: 128,
            depth: 16,
        };
        n.slice(
            3,
            size,
            Octave {
                frequency: [4.0 / 128.0, 4.0 / 128.0, 1.0],
                amplitude: 63.5,
            },
            &mut out,
        );
        let bits: Vec<i32> = out.iter().map(|v| v.to_bits() as i32).collect();
        golden.check("water_slice", &bits);
        let heights = octave_volume(
            size,
            8,
            &n,
            Octave {
                frequency: [4.0, 4.0, 16.0],
                amplitude: 0.5,
            },
            0.6,
        );
        let heights: Vec<i32> = heights.iter().map(|&v| i32::from(v)).collect();
        golden.check("water_heights", &heights);
        let normals: Vec<i32> = normal_volume().iter().map(|&v| i32::from(v)).collect();
        golden.check("water_normals", &normals);
    }

    /// `waterDetail == 2` around Lumbridge: the surface floor carries the
    /// water depth stream and routes its water batches to the
    /// EnvMapped programs; the seabed floor is the underwater pass's.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn water_detail_rebuild_carries_depth_and_water_programs() {
        let pack = crate::test_support::require_pack("client.mapsv2.js5");
        let flo = crate::flo::FloStore::load(&pack).unwrap();
        let tables = crate::maploader::FloTables::from_store(&flo);
        let materials = crate::texture::MaterialStore::load(&pack).unwrap();
        let spec = |g: &crate::floor::FloorGeometry, b: &crate::floor::FloorBatch| {
            let water = if g.underwater {
                crate::material::FloorWater::Underwater
            } else if g.water_detail {
                crate::material::FloorWater::WaterDetail
            } else {
                crate::material::FloorWater::Normal
            };
            crate::material::MaterialSpec::new(
                materials.get(b.material as u32),
                g.has_normals,
                false,
            )
            .with_floor_water(water)
            .program()
        };
        let high = crate::rebuild::BuildPrefs {
            water_detail: 2,
            ..crate::rebuild::BuildPrefs::default()
        };
        let result =
            crate::rebuild::rebuild_normal(&pack, &tables, &materials, None, 3222, 3222, &high)
                .unwrap();
        let surface = result.scene.normal[0].as_ref().unwrap();
        assert!(surface.water_detail && surface.has_depth && !surface.underwater);
        assert!(surface
            .batches
            .iter()
            .any(|b| matches!(spec(surface, b), 7 | 8)));
        let seabed = result.scene.underwater[0].as_ref().unwrap();
        assert!(seabed.underwater && seabed.has_depth && !seabed.water_detail);
        assert!(seabed
            .batches
            .iter()
            .all(|b| spec(seabed, b) != 7 && spec(seabed, b) != 8));
        assert!(seabed.batches.iter().any(|b| spec(seabed, b) == 9));
        // Low detail: no underwater set, no depth stream, no water programs.
        let low = crate::rebuild::rebuild_normal(
            &pack,
            &tables,
            &materials,
            None,
            3222,
            3222,
            &crate::rebuild::BuildPrefs::default(),
        )
        .unwrap();
        let surface = low.scene.normal[0].as_ref().unwrap();
        assert!(!surface.water_detail && !surface.has_depth);
        assert!(low.scene.underwater.iter().all(Option::is_none));
        assert!(surface.batches.iter().all(|b| spec(surface, b) < 7));
    }
}

mod floor {
    use crate::floor::*;

    #[test]
    fn sun_defaults_match_render_constants() {
        let sun = SunLighting::environment_default(3, 0.0);
        // Direction (-200,-240,-200) normalised.
        assert!((sun.dir[0] + 0.539_163_9).abs() < 1e-6);
        assert!((sun.dir[1] + 0.646_996_6).abs() < 1e-6);
        assert!((sun.diffuse_half - 179.0 / 512.0).abs() < 1e-7);
        assert!((sun.shadow_half - 0.6).abs() < 1e-7);
        // The sun update pushes the manager direction << 2:
        // the default manager direction (-50,-60,-50) becomes this sun.
        let env_sun =
            crate::env::Environment::default().sun_lighting([-50.0, -60.0, -50.0], 3, 0.0);
        assert_eq!(env_sun, sun);
    }
}

/// Interface models with particles (type-6 components): the model's emitters
/// bind to the component's system, the system runs on the logic cycle, and a
/// perspective model draws its particles as quads while an orthographic one
/// draws none.
mod interface_particles {
    use crate::interface_model::{bind_particles, Draw, DrawSpace};
    use crate::particle::{keys, Binding, EffectorStore, EmitterStore, Rotation, Runtime};
    use std::rc::Rc;

    /// FNV-1a over 32-bit words.
    fn fnv(words: impl IntoIterator<Item = u32>) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325_u64;
        for word in words {
            for b in word.to_le_bytes() {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
        h
    }

    /// A view looking down +Z with nothing moved.
    fn identity_view() -> [f32; 16] {
        let mut v = [0.0; 16];
        v[0] = 1.0;
        v[5] = 1.0;
        v[10] = 1.0;
        v[15] = 1.0;
        v
    }

    /// Model 197 (six emitters) as an interface component's model in a 640 x
    /// 480 canvas, bound for 90 logic cycles: the perspective draw and the
    /// orthographic one, in that order, each with the list it draws.
    fn model_after_90_cycles(pack: &crate::cache::Pack) -> Vec<Draw> {
        let mut models = crate::ui_models::Models::new(pack.clone());
        let resources = models.resources().unwrap();
        let raw = crate::modelunlit::ModelUnlit::load(pack, 197).unwrap();
        let model = crate::gpumodel::GpuModel::new(
            &crate::gpumodel::ModelStores {
                materials: &resources.materials,
                billboards: &resources.billboards,
                emitters: &resources.emitters,
            },
            &raw,
            crate::gpumodel::BuildParams {
                flags: 0,
                ambient: 64,
                contrast: 850,
                detail: crate::gpumodel::MODEL_DETAIL_FLAGS,
            },
        )
        .unwrap();
        assert!(model.has_particles);
        // The component's own model matrix and projection, as `ui_models::draw`
        // builds them for a component filling the canvas.
        let fields = crate::ui_component_fields::Fields {
            width: 640,
            height: 480,
            modelzoom: 1300,
            modelangle_x: 1900,
            modelangle_y: 300,
            ..Default::default()
        };
        let mut model = model;
        let min_y = model.min_y();
        let (matrix, projection) = rs910_ui::ui_model_transform::matrices(
            &fields,
            [0, 0],
            [640, 480],
            [200.0, 20_000.0],
            None,
            min_y,
        );
        let mut binding = Binding {
            key: keys::component(1),
            ..Default::default()
        };
        binding.add_model(&model, &matrix, Rotation::IDENTITY, keys::BODY);
        assert!(!binding.emitters.is_empty());
        let owner = Rc::new(());
        let lighting = crate::env::EnvFrame::default_for(1000.0, 10.0, &identity_view());
        let draw = |draw_particles: bool| Draw {
            owner: Rc::downgrade(&owner).into(),
            quad: 0,
            before_scene: false,
            clip: [0, 0, 640, 480],
            model: model.clone(),
            depth_write: true,
            resources: resources.clone(),
            particles: binding.clone(),
            draw_particles,
            particle_list: Vec::new(),
            space: DrawSpace {
                matrix: matrix.entries(),
                projection,
                lighting,
            },
        };
        let mut runtime = Runtime::new(
            EmitterStore::load(pack).unwrap(),
            EffectorStore::load(pack).unwrap(),
            0x5eed_7a27,
        );
        let mut draws = vec![draw(true), draw(false)];
        for cycle in 1..=90 {
            runtime.tick(cycle);
            bind_particles(&mut draws, &mut runtime, cycle);
        }
        // Keep the component alive for the caller's draws.
        std::mem::forget(owner);
        draws
    }

    /// The model bound as a component's model for 90 logic cycles: the system
    /// fills its slots, the draw takes the list, the list builds quads, and an
    /// orthographic model takes none. The last cycle's list and its quads are
    /// pinned by digests.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn interface_model_particles_bind_run_and_build_quads() {
        let pack = crate::test_support::require_pack("client.particles.js5");
        let draws = model_after_90_cycles(&pack);
        assert!(draws[1].particle_list.is_empty());
        let list = &draws[0].particle_list;
        assert!(list.len() > 20, "{} particles", list.len());
        let frame =
            crate::particle_render::Builder::default().build(&[list], &identity_view(), [0; 3]);
        assert_eq!(frame.vertices.len(), 4 * list.len() - 4 * frame.dropped);
        let list_digest = fnv(list.iter().flat_map(|p| {
            [
                p.pos[0] as u32,
                p.pos[1] as u32,
                p.pos[2] as u32,
                p.colour as u32,
                p.size as u32,
                u32::from(p.angle as u16),
                p.texture as u32,
            ]
        }));
        let quad_digest = fnv(frame.vertices.iter().flat_map(|v| {
            [
                v.pos[0].to_bits(),
                v.pos[1].to_bits(),
                v.pos[2].to_bits(),
                v.uv[0].to_bits(),
                v.uv[1].to_bits(),
                u32::from_le_bytes(v.colour),
            ]
        }));
        assert_eq!(
            (list.len(), list_digest, quad_digest),
            (247, 0x3355_684b_19b7_495a, 0x273d_ee7a_18ac_7b79),
            "the model's particles after 90 cycles"
        );
    }

    /// The same draws through the faithful GPU's interface-model pass,
    /// offscreen: the perspective draw shows its particles over the model,
    /// the orthographic draw of the same model (empty list) does not.
    #[test]
    #[ignore = "needs a GPU (desktop adapter) and server/data/pack"]
    fn interface_model_particles_draw_on_the_gpu() -> anyhow::Result<()> {
        let pack = crate::test_support::require_pack("client.particles.js5");
        let mut with = model_after_90_cycles(&pack);
        let without = with.split_off(1);
        let out = |draws: Vec<Draw>| {
            let mut painter = crate::ui_paint::Painter::new([640, 480]);
            let recording = std::mem::take(&mut painter.recording);
            let layers = std::mem::take(&mut painter.layers);
            let paint = painter.finish();
            crate::ui_output::Output {
                models: draws,
                scene_quad: paint.quads.len(),
                paint,
                scene: None,
                recording,
                postprocess: None,
                layers,
            }
        };
        let mut capture = crate::ui_model_gpu::Capture::new([640, 480])?;
        let dir =
            std::env::var_os("CLIENT910_TEST_FRAMES").map_or_else(std::env::temp_dir, Into::into);
        let a = capture.render(out(with), &dir.join("interface-particles-with.png"))?;
        let b = capture.render(out(without), &dir.join("interface-particles-without.png"))?;
        // The particles are the difference between the two frames: the
        // model's own pixels are the same.
        let changed = a
            .chunks_exact(4)
            .zip(b.chunks_exact(4))
            .filter(|(p, q)| p.iter().zip(*q).any(|(x, y)| x.abs_diff(*y) > 16))
            .count();
        eprintln!("{changed} pixels differ with the particles");
        assert!(changed > 100, "{changed}");
        Ok(())
    }
}
