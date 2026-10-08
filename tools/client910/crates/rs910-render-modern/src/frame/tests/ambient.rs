//! The per-square ambient capture (`frame::gpu::ambient`): under the
//! verified look each map square's irradiance block comes from a capture of
//! the scene, one face a frame, and the frame converges to it.
use super::*;
use crate::lighting::ambient::Irradiance;
use crate::lighting::ambient_schedule::Square;
use crate::settings::LookMode;

const T0: i64 = 1_700_000_000_000;
/// The clock step of a frame (25 fps).
const STEP_MS: i64 = 40;

fn verified() -> ModernSettings {
    ModernSettings {
        look: LookMode::Verified,
        ..ModernSettings::DEFAULT
    }
}

/// Frames of `snapshot` with the clock stepping until the ambient has
/// settled (every square has its block and the blends are over), at most
/// `limit` frames; returns the frames drawn, the time and the last frame.
fn settle_ambient(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    r: &mut ModernRenderer,
    snapshot: &SceneSnapshot<'_>,
    size: [u32; 2],
    limit: i64,
) -> (i64, i64, Frame) {
    let mut faces = 0;
    for k in 0..limit {
        let now = T0 + STEP_MS * k;
        crate::logic_clock::set_test_now(Some(now));
        let frame = render(device, queue, r, snapshot, size);
        let drawn = r.ambient_stats().faces;
        assert!(drawn - faces <= 1, "{} faces in one frame", drawn - faces);
        faces = drawn;
        if k > 10 && r.ambient_settled(now) {
            return (k, now, frame);
        }
    }
    panic!(
        "the ambient did not settle in {limit} frames: {:?}",
        r.ambient_stats()
    );
}

fn aerial(pack: &crate::cache::Pack, size: [u32; 2]) -> OfflineScene {
    aerial_at(pack, (3222, 3222), size)
}

fn aerial_at(pack: &crate::cache::Pack, tile: (i32, i32), size: [u32; 2]) -> OfflineScene {
    OfflineScene::with_camera(pack, tile, (size[0] as i32, size[1] as i32), |c| {
        c.pitch = 1700.0;
        c.yaw = 12_000.0;
        c.distance_scale = 2.0;
    })
}

/// Under the verified look the ambient is captured one face a frame, each
/// of the window's squares gets a block of its own that is not the default,
/// the squares' blocks differ (they see different surroundings), and the
/// frame differs from the one showing the default block. The calibrated
/// look captures nothing.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn squares_get_their_own_captured_ambient() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [320, 200];
    let offline = aerial(&pack, size);
    let sky = offline.sky(&pack);
    let mut snapshot = offline.snapshot(&pack);
    snapshot.sky = sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);

    let classic = ModernSettings {
        look: LookMode::ClassicCalibrated,
        ..ModernSettings::DEFAULT
    };
    let mut calibrated = renderer(&device, &queue, 4, classic);
    settled(&device, &queue, &mut calibrated, &snapshot, size);
    assert_eq!(calibrated.ambient_stats().faces, 0);

    let mut r = renderer(&device, &queue, 4, verified());
    crate::logic_clock::set_test_now(Some(T0));
    let first = render(&device, &queue, &mut r, &snapshot, size);
    let (frames, now, last) = settle_ambient(&device, &queue, &mut r, &snapshot, size, 600);
    let stats = r.ambient_stats();
    let squares = r.ambient.schedule.wanted().to_vec();
    eprintln!(
        "settled after {frames} frames: {stats:?} over {} squares",
        squares.len()
    );
    assert!(squares.len() >= 4, "{squares:?}");
    assert_eq!(
        stats.faces as usize,
        6 * squares.len(),
        "six faces a square"
    );
    assert_eq!(stats.pending, 0);
    assert_eq!(r.ambient_stats().cached, squares.len());

    // Each square shows its own block, none the default.
    let up = [0.0_f32, -1.0, 0.0];
    let side = [0.0_f32, 0.0, 1.0];
    let shown: Vec<[[f32; 3]; 2]> = squares
        .iter()
        .map(|&s| {
            let b = r.ambient_block(s, now);
            assert_ne!(b, Irradiance::DEFAULT, "{s:?}");
            [b.evaluate_classic(up), b.evaluate_classic(side)]
        })
        .collect();
    for v in shown.iter().flatten().flatten() {
        assert!(v.is_finite() && *v >= -0.2 && *v < 1.6, "{shown:?}");
    }
    // The sky is above: facing up the block is bluer than red and brighter
    // than facing down (the capture's up is the renderer's -y).
    for &square in &squares {
        let b = r.ambient_block(square, now);
        let (up_light, down_light) = (
            b.evaluate_classic([0.0, -1.0, 0.0]),
            b.evaluate_classic([0.0, 1.0, 0.0]),
        );
        assert!(up_light[2] > up_light[0], "{square:?}: up {up_light:?}");
        assert!(
            luma(&up_light) > luma(&down_light),
            "{square:?}: up {up_light:?}, down {down_light:?}"
        );
    }
    // Open sky above: up is lit by the sky, and the surroundings differ
    // between squares.
    let mut widest = 0.0_f32;
    for a in &shown {
        for b in &shown {
            for ch in 0..3 {
                widest = widest
                    .max((a[0][ch] - b[0][ch]).abs())
                    .max((a[1][ch] - b[1][ch]).abs());
            }
        }
    }
    eprintln!("blocks {shown:?}; widest difference between squares {widest:.3}");
    assert!(
        widest > 0.01,
        "every square shows the same light: {shown:?}"
    );

    // The frame moved from the default block's to the captured ones.
    let moved = Noise::of(&first.pixels, &last.pixels);
    eprintln!("first frame to settled frame: {moved:?}");
    assert!(
        moved.values * 20 > (size[0] * size[1] * 4) as usize,
        "{moved:?}"
    );
    dump_frame("ambient-default", size, &first.pixels);
    dump_frame("ambient-captured", size, &last.pixels);
}

/// Two runs capture the same blocks and draw the same settled frame, and the
/// settled frame repeats: no capture or blend is left moving.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn captured_ambient_is_deterministic_and_converged() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [320, 200];
    let offline = aerial(&pack, size);
    let sky = offline.sky(&pack);
    let mut snapshot = offline.snapshot(&pack);
    snapshot.sky = sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
    let run = || {
        let mut r = renderer(&device, &queue, 4, verified());
        let (_, now, frame) = settle_ambient(&device, &queue, &mut r, &snapshot, size, 600);
        let blocks: Vec<Irradiance> = r
            .ambient
            .schedule
            .wanted()
            .iter()
            .map(|&s| r.ambient_block(s, now))
            .collect();
        // Another frame at the same time: nothing is left moving (a capture
        // or a blend would show).
        let later = render(&device, &queue, &mut r, &snapshot, size);
        (frame, later, blocks, r.ambient_stats().faces)
    };
    let (a, a_later, a_blocks, a_faces) = run();
    let (b, _, b_blocks, b_faces) = run();
    assert_eq!(a_faces, b_faces);
    for (x, y) in a_blocks.iter().zip(&b_blocks) {
        for (p, q) in x.0.iter().flatten().zip(y.0.iter().flatten()) {
            assert!((p - q).abs() < 2e-3, "{x:?} vs {y:?}");
        }
    }
    let (n_run, n_later) = (
        Noise::of(&a.pixels, &b.pixels),
        Noise::of(&a.pixels, &a_later.pixels),
    );
    eprintln!("run to run {n_run:?}, settled to later {n_later:?}");
    assert!(n_run.is_repeat_noise(size), "{n_run:?}");
    assert!(n_later.is_repeat_noise(size), "{n_later:?}");
}

/// The shader picks the block by the map square under each fragment: with
/// the western square's block replaced by black, the ground in it loses its
/// ambient while the ground in the square to the east is untouched.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn fragments_take_the_block_of_their_square() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [480, 300];
    // The camera over the edge between map squares 49 and 50 (tile 3200).
    let offline = aerial_at(&pack, (3201, 3222), size);
    let snapshot = offline.snapshot(&pack);
    let mut r = renderer(&device, &queue, 4, verified());
    let (_, now, normal) = settle_ambient(&device, &queue, &mut r, &snapshot, size, 600);
    let zero = Irradiance([[0.0; 4]; 7]);
    let west: Vec<Square> = r
        .ambient
        .schedule
        .wanted()
        .iter()
        .copied()
        .filter(|s| s.0 == 49)
        .collect();
    assert!(!west.is_empty());
    for s in west {
        r.ambient.test_blocks.insert(s, zero);
    }
    crate::logic_clock::set_test_now(Some(now));
    let dark = render(&device, &queue, &mut r, &snapshot, size);

    // Ground points five tiles either side of the edge, on screen.
    let base = snapshot.floor_base;
    let g0 = snapshot.floors[0].as_ref().unwrap();
    let origin = [
        (snapshot.camera.target[0] - base[0] * 512) as f32,
        snapshot.camera.target[1] as f32,
        (snapshot.camera.target[2] - base[1] * 512) as f32,
    ];
    let vp = glam::Mat4::from_cols_array_2d(
        &frame_uniforms(&snapshot, (size[0] as i32, size[1] as i32)).view_proj,
    );
    let mean = |f: &Frame, (px, py): (f32, f32)| {
        let mut sum = 0.0;
        let mut n = 0.0;
        for dy in -3..=3 {
            for dx in -3..=3 {
                let (x, y) = ((px as i32 + dx) as usize, (py as i32 + dy) as usize);
                let i = (y * size[0] as usize + x) * 4;
                sum += luma(&f.hdr[i..i + 3]);
                n += 1.0;
            }
        }
        sum / n
    };
    let point = |tile_x: i32, tile_z: i32| {
        let (lx, lz) = (
            (tile_x - base[0]) * 512 + 256,
            (tile_z - base[1]) * 512 + 256,
        );
        let y = g0.heights.get_fine_height_clamped(lx, lz) as f32;
        let p = [lx as f32 - origin[0], y - origin[1], lz as f32 - origin[2]];
        pixel_of(&vp, size, p)
    };
    let (mut west_ratio, mut east_ratio) = (None, None);
    for (tile, slot) in [(3195, &mut west_ratio), (3206, &mut east_ratio)] {
        for dz in [0, 4, -4, 8, -8] {
            let (px, py) = point(tile, 3222 + dz);
            if px > 4.0 && py > 4.0 && px < size[0] as f32 - 4.0 && py < size[1] as f32 - 4.0 {
                *slot = Some(mean(&dark, (px, py)) / mean(&normal, (px, py)));
                break;
            }
        }
    }
    eprintln!(
        "ground luminance with the west square black: west {west_ratio:?}, east {east_ratio:?}"
    );
    let (west_ratio, east_ratio) = (
        west_ratio.expect("west ground on screen"),
        east_ratio.expect("east ground on screen"),
    );
    assert!(
        west_ratio < 0.92,
        "the western ground kept its ambient: {west_ratio}"
    );
    assert!(
        (east_ratio - 1.0).abs() < 0.02,
        "the eastern ground changed: {east_ratio}"
    );
    dump_frame("ambient-west-black", size, &dark.pixels);
}
