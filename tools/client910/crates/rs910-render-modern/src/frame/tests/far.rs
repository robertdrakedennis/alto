//! The far scene: its squares meet the near terrain without seams; its
//! locs are the classic placement's outside the window, never the window's;
//! streamed on workers it becomes the synchronous frame; the near
//! extension's containers draw the per-loc frame with fewer draws.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rs910_far_scene::far_ring::TileRect;
use rs910_far_scene::far_terrain::read_square;

use super::*;
use crate::terrain::TerrainScene;

/// The far scene meets the near one without seams: a far square built by `crate::terrain::build_square` (quiet
/// reads, its neighbours around it) has, on every tile inside the classic
/// window, exactly the near terrain's vertices (positions, materials and
/// normals, the near grid's one-sided east and north edge normals included:
/// the build takes the window), and on the window's boundary the colours;
/// every far vertex outside the window that lies
/// on the grid of the window's closed rectangle has the near terrain's
/// height there (no crack at the window edge); and neighbouring far squares
/// share their edge heights.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn far_squares_meet_the_near_terrain_at_equal_heights() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let offline = OfflineScene::new(&pack, (3222, 3218), (480, 300));
    let snapshot = offline.snapshot(&pack);
    let materials = snapshot.materials.unwrap();
    let near = TerrainScene::build(&pack, materials, &snapshot);
    assert!(near.usable, "{}", near.reason);
    let base = snapshot.floor_base;
    let window = TileRect::at(base, near.tiles);
    let near0 = near.levels[0].as_ref().unwrap();
    let flo = rs910_config::flo::FloStore::load(&pack).unwrap();
    let (lo, hi) = (
        [window.x0.div_euclid(64) - 1, window.z0.div_euclid(64) - 1],
        [
            (window.x1 - 1).div_euclid(64) + 1,
            (window.z1 - 1).div_euclid(64) + 1,
        ],
    );
    let mut squares = HashMap::new();
    for sx in lo[0] - 1..=hi[0] + 1 {
        for sz in lo[1] - 1..=hi[1] + 1 {
            if let Some(t) = read_square(&pack, (sx, sz)) {
                squares.insert((sx, sz), t);
            }
        }
    }
    let mut far_materials: Vec<i32> = Vec::new();
    let mut built = HashMap::new();
    for sx in lo[0]..=hi[0] {
        for sz in lo[1]..=hi[1] {
            let rect = Some([window.x0, window.z0, window.x1, window.z1]);
            let levels =
                crate::terrain::build_square(&squares, (sx, sz), &flo, materials, rect, &mut |m| {
                    if let Some(i) = far_materials.iter().position(|&x| x == m) {
                        return i as u16;
                    }
                    far_materials.push(m);
                    (far_materials.len() - 1) as u16
                });
            built.insert((sx, sz), levels);
        }
    }
    let material = |table: &[i32], slot: u16| (slot != 0xFFFF).then(|| table[usize::from(slot)]);
    let (mut same_tiles, mut vertices, mut colour_diffs) = (0, 0, 0);
    let (mut boundary_colours, mut edge_normals) = (0, 0);
    let (mut seam_vertices, mut far_edges) = (0, 0);
    let (tx, tz) = (near.tiles[0] as i32, near.tiles[1] as i32);
    for (&(sx, sz), levels) in &built {
        let Some(far0) = levels[0].as_ref() else {
            continue;
        };
        let shift = [
            ((sx * 64 - base[0]) * 512) as f32,
            ((sz * 64 - base[1]) * 512) as f32,
        ];
        for tile in 0..64 * 64 {
            let (lx, lz) = ((tile % 64) as i32, (tile / 64) as i32);
            let (x, z) = (sx * 64 + lx - base[0], sz * 64 + lz - base[1]);
            let (start, count) = far0.tile_ranges[tile];
            let far_vs = &far0.vertices[start as usize..(start + count) as usize];
            if window.contains(x + base[0], z + base[1]) {
                let (ns, nc) = near0.tile_ranges[(z * tx + x) as usize];
                let near_vs = &near0.vertices[ns as usize..(ns + nc) as usize];
                assert_eq!(far_vs.len(), near_vs.len(), "tile {x},{z}: vertex count");
                let edge = x == tx - 1 || z == tz - 1;
                for (f, n) in far_vs.iter().zip(near_vs) {
                    let p = [f.pos[0] + shift[0], f.pos[1], f.pos[2] + shift[1]];
                    assert_eq!(p, n.pos, "tile {x},{z}: position");
                    for k in 0..3 {
                        assert_eq!(
                            material(&far_materials, f.slots[k]),
                            material(&near.layer_materials, n.slots[k]),
                            "tile {x},{z}: material slot {k}"
                        );
                    }
                    assert_eq!((f.scale, f.weight), (n.scale, n.weight));
                    edge_normals += usize::from(edge);
                    assert_eq!(f.normal, n.normal, "tile {x},{z}: normal");
                    // On the window's boundary the classic shade map is 0 (its
                    // border vertices stay unblurred), so the colours meet.
                    let (gx, gz) = (p[0] / 512.0, p[2] / 512.0);
                    if gx == 0.0 || gz == 0.0 || gx == tx as f32 || gz == tz as f32 {
                        assert_eq!(f.colour, n.colour, "boundary vertex {gx},{gz}: colour");
                        boundary_colours += 1;
                    }
                    colour_diffs += usize::from(f.colour != n.colour);
                    vertices += 1;
                }
                same_tiles += 1;
                continue;
            }
            // Outside: vertices on the window rectangle's grid points take
            // the near terrain's height there.
            for f in far_vs {
                let (gx, gz) = (f.pos[0] + shift[0], f.pos[2] + shift[1]);
                if gx.rem_euclid(512.0) != 0.0 || gz.rem_euclid(512.0) != 0.0 {
                    continue;
                }
                let (gx, gz) = ((gx / 512.0) as i32, (gz / 512.0) as i32);
                if gx < 0 || gz < 0 || gx > tx || gz > tz {
                    continue;
                }
                let h = near0.grid[(gx * (tz + 1) + gz) as usize].0;
                assert_eq!(f.pos[1], h, "seam vertex {gx},{gz}");
                seam_vertices += 1;
            }
        }
        // Far-far seams: the east and north neighbours' shared edge.
        for (dx, dz) in [(1, 0), (0, 1)] {
            let Some(Some(other)) = built.get(&(sx + dx, sz + dz)).map(|l| l[0].as_ref()) else {
                continue;
            };
            for k in 0..=64 {
                let (a, b) = if dx == 1 {
                    (64 * 65 + k, k)
                } else {
                    (k * 65 + 64, k * 65)
                };
                assert_eq!(
                    far0.grid[a].0, other.grid[b].0,
                    "square edge {sx},{sz} +{dx},{dz}"
                );
                far_edges += 1;
            }
        }
    }
    eprintln!(
        "{same_tiles} window tiles equal ({vertices} vertices; {edge_normals} of them on the near grid's clamped east/north edge tiles; {colour_diffs} colours differ inside by the near terrain's classic shade map, {boundary_colours} boundary colours equal), {seam_vertices} seam vertices at the near height, {far_edges} far-far edge points"
    );
    assert!(same_tiles > 5000 && seam_vertices > 400 && far_edges > 400 && boundary_colours > 400);
}

/// F3: the far locs of the squares around the classic window are the classic
/// placement's own: none of them is a loc the window's scene graph holds
/// (the census per loc and tile finds no loc drawn twice), the window's
/// share of each square is left to it, and every loc drawn from RT7 draws
/// exactly its classic model's faces at LOD 0, most of them from RT7.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn far_locs_are_classic_faces_outside_the_window() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let offline = OfflineScene::new(&pack, (3222, 3218), (480, 300));
    let snapshot = offline.snapshot(&pack);
    let materials = snapshot.materials.unwrap();
    let scene = snapshot.scene.unwrap();
    let base = snapshot.floor_base;
    let g = snapshot.floors[0].as_ref().unwrap();
    let window = TileRect::at(base, [g.tiles_x, g.tiles_z]);
    // The window's locs by (loc, plane, tile).
    let tile = |x: i32, z: i32| ((x >> 9) + base[0], (z >> 9) + base[1]);
    let mut classic: HashSet<(u32, i32, (i32, i32))> = HashSet::new();
    classic.extend(
        scene
            .scenery
            .iter()
            .map(|e| (e.loc_id, e.level, tile(e.x, e.z))),
    );
    classic.extend(
        scene
            .walls
            .iter()
            .map(|e| (e.loc_id, e.level, tile(e.x, e.z))),
    );
    classic.extend(
        scene
            .wall_decors
            .iter()
            .map(|e| (e.loc_id, e.level, tile(e.x, e.z))),
    );
    classic.extend(
        scene
            .ground_decors
            .iter()
            .map(|e| (e.loc_id, e.level, tile(e.x, e.z))),
    );
    let flo = rs910_config::flo::FloStore::load(&pack).unwrap();
    let assets = Arc::new(crate::far::jobs::FarAssets::load(&pack, materials, &flo));
    let mut placer = rs910_far_scene::far_locs::Placer::new(
        pack.clone(),
        Arc::clone(&assets.place),
        rs910_config::config::LocStore::load(&pack).unwrap(),
    );
    let mut worker = crate::far::jobs::LocWorker::new(Arc::clone(&assets));
    let (lo, hi) = (
        [window.x0.div_euclid(64), window.z0.div_euclid(64)],
        [
            (window.x1 - 1).div_euclid(64),
            (window.z1 - 1).div_euclid(64),
        ],
    );
    // The categories each container keeps at level 2 seen from the
    // window's centre.
    let focus = [(window.x0 + window.x1) * 256, (window.z0 + window.z1) * 256];
    let level = rs910_far_scene::far_level::FarLevel::DEFAULT;
    let (mut placed, mut in_window, mut twice) = (0, 0, 0);
    let (mut rt7, mut classic_meshes, mut mismatches, mut containers) = (0, 0, 0, 0);
    let (mut categories, mut bytes, mut index_bytes) = ([0; 5], 0, 0);
    for sx in lo[0] - 1..=hi[0] + 1 {
        for sz in lo[1] - 1..=hi[1] + 1 {
            let square =
                rs910_far_scene::far_locs::place_square(&mut placer, (sx, sz), window, &|_, _| {
                    true
                });
            placed += square.stats.placed;
            in_window += square.stats.in_window;
            twice += square
                .locs
                .iter()
                .filter(|l| classic.contains(&(l.loc_id, l.level, l.anchor)))
                .count();
            let built = worker.square((sx, sz), window, focus, level);
            rt7 += built.stats.rt7;
            classic_meshes += built.stats.classic;
            mismatches += built.stats.face_mismatches;
            containers += built.containers.len();
            bytes += built.stats.bytes;
            index_bytes += built.stats.index_bytes;
            for (c, n) in categories.iter_mut().zip(built.stats.categories) {
                *c += n;
            }
        }
    }
    eprintln!(
        "{placed} far locs placed ({in_window} left to the window, {twice} of them the window's); {rt7} from RT7 ({mismatches} with other faces), {classic_meshes} classic meshes, {containers} containers ({:.1} MB, {:.1} MB of it indices; locs by category {categories:?})",
        bytes as f64 / 1e6,
        index_bytes as f64 / 1e6
    );
    assert!(
        placed > 5_000 && in_window > 2_000,
        "{placed} placed, {in_window} in the window"
    );
    assert_eq!(twice, 0, "far locs the window also holds");
    assert_eq!(
        mismatches, 0,
        "RT7 far locs drawing other faces than their classic models"
    );
    assert!(
        rt7 * 10 > (rt7 + classic_meshes) * 8,
        "{rt7} RT7 of {} built",
        rt7 + classic_meshes
    );
    assert!(containers > 16);
}

/// F5: streamed on the workers, the far ring becomes the frame the
/// synchronous mode draws: the render thread builds nothing (no inline
/// job), the builds finish and upload over the next frames, and the
/// settled frame equals the synchronous one within the repeat noise. The
/// far locs draw fewer calls than the models they draw.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn streamed_far_scene_converges_to_the_synchronous_frame() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [480, 300];
    let offline = OfflineScene::with_camera(&pack, (3222, 3218), (480, 300), |c| {
        c.pitch = 1700.0;
        c.yaw = 12_000.0;
        c.distance_scale = 6.0;
    });
    let snapshot = offline.snapshot(&pack);
    let mut sync = renderer(&device, &queue, 4, ModernSettings::DEFAULT);
    let reference = settled(&device, &queue, &mut sync, &snapshot, size);
    let s = sync.far_stats();
    eprintln!("sync: {s:?}");
    // One draw per material batch and LOD run, not per loc.
    assert!(s.far_locs > 0 && s.far_draws < s.far_locs, "{s:?}");
    let mut streamed = renderer(&device, &queue, 4, ModernSettings::DEFAULT);
    streamed.scene_resources.far.sync = false;
    // (The far scene starts on the second frame: the first decides the
    // scene's terrain.)
    let (mut frames, mut streaming) = (0, 0);
    loop {
        render(&device, &queue, &mut streamed, &snapshot, size);
        frames += 1;
        let s = streamed.far_stats();
        let busy = s.pending + s.waiting + s.uploaded > 0;
        streaming += usize::from(busy);
        if streaming > 0 && !busy && s.containers > 0 {
            break;
        }
        assert!(frames < 20_000, "the ring never finished: {s:?}");
    }
    let inline = streamed
        .scene_resources
        .far
        .terrain_jobs
        .as_ref()
        .map_or(0, |j| j.inline_runs())
        + streamed
            .scene_resources
            .far
            .loc_jobs
            .as_ref()
            .map_or(0, |j| j.inline_runs());
    let frame = settled(&device, &queue, &mut streamed, &snapshot, size);
    let noise = Noise::of(&reference.pixels, &frame.pixels);
    eprintln!("{streaming} of {frames} frames streaming; settled: {noise:?}");
    assert_eq!(inline, 0, "the render thread built far squares");
    assert!(noise.is_repeat_noise(size), "{noise:?}");
}

/// Bottleneck #8: the near extension drawn from its containers is the
/// per-loc frame (within the repeat noise) with less than half the draws.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn batched_near_extension_draws_the_per_loc_frame() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [480, 300];
    let offline = OfflineScene::with_camera(&pack, (3222, 3218), (480, 300), |c| {
        c.pitch = 1700.0;
        c.yaw = 12_000.0;
        c.distance_scale = 3.0;
    });
    let snapshot = offline.snapshot(&pack);
    let mut per_loc = renderer(&device, &queue, 4, ModernSettings::DEFAULT);
    per_loc.scene_resources.far.test_per_loc = true;
    let reference = settled(&device, &queue, &mut per_loc, &snapshot, size);
    let mut batched = renderer(&device, &queue, 4, ModernSettings::DEFAULT);
    let frame = settled(&device, &queue, &mut batched, &snapshot, size);
    let (a, b) = (per_loc.far_stats(), batched.far_stats());
    let noise = Noise::of(&reference.pixels, &frame.pixels);
    eprintln!(
        "per loc: {} locs, {} entity draws; batched: {} locs ({} batched, {} draws), {} entity draws; {noise:?}",
        a.ext_locs, per_loc.stats.draws, b.ext_locs, b.ext_batched, b.ext_draws, batched.stats.draws
    );
    assert_eq!(a.ext_locs, b.ext_locs, "the same extension");
    assert!(b.ext_batched * 2 > b.ext_locs, "{b:?}");
    // The batched locs' per-loc draws are gone; their containers draw
    // less than half as many calls.
    let per_loc_draws = per_loc.stats.draws - (batched.stats.draws - b.ext_draws);
    assert!(
        b.ext_draws * 2 < per_loc_draws,
        "{} container draws for {per_loc_draws} per-loc draws",
        b.ext_draws
    );
    assert!(noise.is_repeat_noise(size), "{noise:?}");
}

/// Far indexed multi-draw preserves nonzero first instances, base vertices,
/// index ranges and draw order with the real Device staging uploader.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn far_multi_draw_matches_direct_packets_with_staged_arguments() {
    use crate::frame::{Geometry, PrepareTarget};
    const SIZE: [u32; 2] = [480, 320];
    const PACKET_COUNT: usize = 64;
    const GRID_SIDE: usize = 8;
    const MARKER_SPACING: f32 = 180.0;
    const MARKER_HALF_WIDTH: f32 = 180.0;
    const GROUND_Y: f32 = -4.0;
    const COLOR_STEP: u32 = 3;
    let _clock = fixed_clock();
    let gpu = pollster::block_on(rs910_gpu_device::gpu_device::Device::headless(
        (SIZE[0], SIZE[1]),
        Default::default(),
        crate::device_features::optional_device_features(),
    ))
    .expect("headless modern device");
    assert!(gpu
        .adapter
        .get_downlevel_capabilities()
        .flags
        .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION));
    assert!(gpu
        .device
        .features()
        .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE));
    let (device, queue) = (&*gpu.device, &*gpu.queue);
    let (camera, env) = bare_camera(SIZE);
    let snapshot = bare(&camera, &env);
    let identity = glam::Mat4::IDENTITY.to_cols_array();
    let models: Vec<_> = (0..PACKET_COUNT)
        .map(|index| {
            let x = (index % GRID_SIDE) as f32 * MARKER_SPACING;
            let z = (index / GRID_SIDE) as f32 * MARKER_SPACING;
            let mut mesh = flat_quad(
                camera.target[0] as f32 + x,
                camera.target[2] as f32 + z,
                MARKER_HALF_WIDTH,
                GROUND_Y,
            );
            let colour = 0xff20_5040 + index as u32 * COLOR_STEP;
            mesh.colours.fill(colour);
            (mesh, identity)
        })
        .collect();
    let mut r = ModernRenderer::new(
        device,
        &gpu,
        wgpu::TextureFormat::Rgba8Unorm,
        1,
        ModernSettings {
            ao: crate::settings::AoMode::Off,
            far: None,
            volumetrics: false,
            ..ModernSettings::DEFAULT
        },
    );
    r.preparation.test_models = models.clone();
    let rect = [0, 0, SIZE[0] as i32, SIZE[1] as i32];
    let clip = [0, 0, SIZE[0] as i32, SIZE[1] as i32];
    let _prepared = r
        .prepare_frame(
            PrepareTarget {
                device,
                queue: &gpu,
                size: SIZE,
                rect,
                clip,
            },
            &snapshot,
        )
        .expect("visible frame");
    assert_eq!(r.frame_resources.draws.len(), PACKET_COUNT);
    for (draw, (mesh, _)) in r.frame_resources.draws.iter_mut().zip(&models) {
        let allocation = r.scene_resources.far.arena.store(device, &gpu, None, mesh);
        draw.geometry = Geometry::Far {
            page: allocation.page,
            base_vertex: allocation.vertex as i32,
        };
        draw.first_index = allocation.first_index();
    }
    r.scene_resources.far.arena.flush(&gpu);
    r.prepare_far_indirect(device, &gpu);
    r.frame_resources
        .packets
        .build(&r.frame_resources.draws, PACKET_COUNT, true);
    assert_eq!(r.scene_resources.far.stats.indirect_runs, 1);
    assert_eq!(r.scene_resources.far.stats.indirect_packets, PACKET_COUNT);
    assert!(r
        .scene_resources
        .far
        .indirect_args
        .iter()
        .skip(1)
        .all(|arg| arg.first_instance > 0 && arg.base_vertex > 0 && arg.first_index > 0));
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("far packet proof"),
        size: wgpu::Extent3d {
            width: SIZE[0],
            height: SIZE[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    let capture = |r: &ModernRenderer| {
        let mut encoder = device.create_command_encoder(&Default::default());
        let mut units = r.encode(device, &mut encoder, &view, rect, clip);
        units.push(encoder.finish());
        gpu.submit(units);
        read_back(device, queue, &output, 4)
    };
    // Warm the same prepared state, without advancing simulation or producer clocks.
    capture(&r);
    let indirect = capture(&r);
    let packets = r.frame_resources.draws.clone();
    for draw in &mut r.frame_resources.draws {
        draw.indirect = None;
    }
    r.frame_resources
        .packets
        .build(&r.frame_resources.draws, PACKET_COUNT, true);
    let direct = capture(&r);
    assert_eq!(
        indirect, direct,
        "multi-draw leaves the exact prepared frame unchanged"
    );
    assert!(direct
        .chunks_exact(4)
        .any(|pixel| pixel[0] > 0 && pixel[1] > 0 && pixel[2] > 0));
    assert!(
        direct.chunks_exact(4).any(|pixel| pixel != &direct[..4]),
        "varied visible geometry"
    );
    r.frame_resources.draws = packets;
    r.frame_resources
        .packets
        .build(&r.frame_resources.draws, PACKET_COUNT, true);
    assert_eq!(
        capture(&r),
        direct,
        "returning to multi-draw repeats the same frame"
    );
}
