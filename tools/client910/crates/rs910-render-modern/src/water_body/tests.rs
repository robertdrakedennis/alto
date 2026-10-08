//! Tests of the water's CPU half (renderer plan M7): the reflection maths,
//! the flow clock, and (pack) the map-file evidence the module docs cite.
use super::*;

/// A camera-local frame like `crate::frame::frame_uniforms_lit`'s: the
/// camera orbiting its target (the origin).
fn camera_frame() -> (glam::Mat4, glam::Mat4, glam::Vec3) {
    let mut camera = crate::camera::SceneCamera::new([0, 0, 0]);
    camera.viewport = (320, 200);
    let view = camera.view_entries();
    let vp = crate::camera::multiply(&view, &camera.projection());
    let view_proj = crate::camera::gl_to_wgpu_depth() * crate::camera::to_glam(&vp);
    let eye = camera.eye();
    (
        crate::camera::to_glam(&view),
        view_proj,
        glam::Vec3::new(eye[0] as f32, eye[1] as f32, eye[2] as f32),
    )
}

fn clip(m: glam::Mat4, p: glam::Vec3) -> glam::Vec3 {
    let c = m * p.extend(1.0);
    c.truncate() / c.w
}

/// The mirrored camera sees a point on the plane where the real camera
/// does, keeps the points above the plane in the depth range and clips the
/// ones below it (the oblique near plane of `UpdateWaterReflection`).
#[test]
fn reflection_camera_mirrors_about_the_plane_and_clips_below_it() {
    let (view, view_proj, eye) = camera_frame();
    assert!(
        eye.y < 0.0,
        "the eye is above the target (classic y down): {eye}"
    );
    let h = 120.0;
    let (reflected_view, reflected) = reflection_view_proj(view, view_proj, h, 0.0);
    let mirrored_eye = mirror(h).transform_point3(eye);
    assert!((mirrored_eye.y - (2.0 * h - eye.y)).abs() < 1e-3);
    let origin = reflected_view.inverse().transform_point3(glam::Vec3::ZERO);
    assert!(
        (origin - mirrored_eye).length() < 0.5,
        "{origin} vs {mirrored_eye}"
    );
    // On the plane: the same screen point.
    for p in [
        glam::Vec3::new(0.0, h, 0.0),
        glam::Vec3::new(300.0, h, 200.0),
        glam::Vec3::new(-400.0, h, 600.0),
    ] {
        let a = clip(view_proj, p);
        let b = clip(reflected, p);
        assert!(
            (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3,
            "{p}: {a} vs {b}"
        );
    }
    // Above the plane (y < h): kept, depth inside [0, 1]; below: clipped.
    for p in [
        glam::Vec3::new(0.0, h - 200.0, 0.0),
        glam::Vec3::new(500.0, h - 50.0, 800.0),
        glam::Vec3::new(-300.0, h - 1000.0, 300.0),
    ] {
        let c = reflected * p.extend(1.0);
        assert!(c.w > 0.0 && c.z >= 0.0 && c.z <= c.w, "above {p}: {c}");
    }
    for p in [
        glam::Vec3::new(0.0, h + 200.0, 0.0),
        glam::Vec3::new(500.0, h + 50.0, 800.0),
    ] {
        let c = reflected * p.extend(1.0);
        assert!(c.z < 0.0, "below {p}: {c}");
    }
}

#[test]
fn reflection_plane_is_the_water_nearest_the_target() {
    let h = reflection_height([
        [900.0, 10.0, 0.0],
        [100.0, 30.0, -50.0],
        [-2000.0, 50.0, 0.0],
    ]);
    assert_eq!(h, Some(30.0));
    assert_eq!(reflection_height([]), None);
}

/// The bed lies `32 h` below the water tile's own surface
/// (M10's rule), not `32 h` below classic height 0 (M7's), so water whose
/// surface is not at height 0 gets its true depth; land is its own height;
/// between vertices the ground is bilinear.
#[test]
fn bed_lies_below_the_tiles_own_surface() {
    // A 2 x 2 vertex grid: a raised pool (surface code 10, i.e. 320 up),
    // beds 2 and 4 steps down, and two land vertices at code 10.
    let tiles = vec![
        TileWater {
            water: true,
            bed: 2,
            surface: Some(10),
        },
        TileWater {
            water: true,
            bed: 4,
            surface: Some(10),
        },
        TileWater {
            water: false,
            bed: 10,
            surface: None,
        },
        TileWater {
            water: false,
            bed: 10,
            surface: None,
        },
    ];
    let map = WaterMap {
        dims: [2, 2],
        tiles,
        ..WaterMap::default()
    };
    // Vertex (0, 0): surface 320 up, bed 64 below it: classic y -256.
    assert_eq!(map.tiles[0].ground_y(), -(320.0 - 64.0));
    assert_eq!(map.tiles[2].ground_y(), -320.0);
    assert_eq!(map.bed_y(0.0, 0.0), Some(-256.0));
    // Its depth under the classic surface (y -320) is 64, not M7's 64 + 320.
    assert_eq!(map.bed_y(0.0, 0.0).unwrap() - -320.0, 64.0);
    // Vertex (0, 1) (x * dims + z) is 128 below: halfway, 96.
    assert_eq!(map.bed_y(0.0, 256.0), Some(-(320.0 - 96.0)));
    // Surface code 1 (height 0): the old and new rules agree there.
    let flat = TileWater {
        water: true,
        bed: 5,
        surface: Some(1),
    };
    assert_eq!(flat.ground_y(), 160.0);
}

/// Squares 48..=51 x 49..=51: the Lumbridge river, the swamp coast, the
/// lake west of Lumbridge, as one scene.
fn lumbridge_map(pack: &crate::cache::Pack) -> WaterMap {
    WaterMap::load(pack, [48 * 64, 49 * 64], [4 * 64 - 1, 3 * 64 - 1])
}

/// The data behind the depth: a water tile's two height bytes are the classic
/// land height (the surface) and the classic underwater land height (the
/// bed; 0 where the classic map has none), on every water tile of those squares.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn water_tiles_carry_the_classic_surface_and_bed_heights() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    fn land(b: &[u8], levels: usize) -> Vec<i32> {
        let mut r = rs910_core::reader::Reader::new(b);
        let mut heights = Vec::new();
        for _ in 0..levels * 64 * 64 {
            let op = r.g1().unwrap();
            if op & 1 != 0 {
                r.g1().unwrap();
                r.gsmart1or2().unwrap();
            }
            if op & 2 != 0 {
                r.g1().unwrap();
            }
            if op & 4 != 0 {
                r.gsmart1or2().unwrap();
            }
            heights.push(if op & 8 != 0 {
                i32::from(r.g1().unwrap())
            } else {
                -1
            });
        }
        heights
    }
    let (mut water, mut beds) = (0, 0);
    for sx in 48..=51_u32 {
        for sz in 49..=51_u32 {
            let group = map_group(sx, sz);
            let files = pack.read_group(MAP_ARCHIVE, group).unwrap();
            let map = WaterMap::load(&pack, [sx as i32 * 64, sz as i32 * 64], [63, 63]);
            let surface = land(&files[&3], 4);
            let bed = files.get(&4).map(|b| land(b, 1));
            for x in 0..64 {
                for z in 0..64 {
                    let t = map.tiles[x * 64 + z];
                    if !t.water {
                        continue;
                    }
                    water += 1;
                    assert_eq!(
                        t.surface.map(i32::from),
                        Some(surface[x * 64 + z]),
                        "{sx},{sz} {x},{z}"
                    );
                    let classic_bed = bed.as_ref().map_or(-1, |b| b[x * 64 + z]);
                    assert_eq!(i32::from(t.bed), classic_bed.max(0), "{sx},{sz} {x},{z}");
                    beds += usize::from(classic_bed > 0);
                }
            }
        }
    }
    assert!(
        water > 1000 && beds > 500,
        "{water} water tiles, {beds} with a bed"
    );
}

/// The records' boxes follow the water: with [`ROTATION_SENSE`] they hold
/// more of the squares' water tiles, and fewer dry tiles, than with the
/// other sense; the Lumbridge river's flow points south (-Z); water outside
/// every record is type 0.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn patch_rotation_sense_covers_the_river() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let map = lumbridge_map(&pack);
    assert!(map.patches.len() > 20, "{}", map.patches.len());
    let score = |patches: &[Patch]| {
        let (mut wet, mut dry) = (0, 0);
        for x in 0..map.dims[0] as i32 - 1 {
            for z in 0..map.dims[1] as i32 - 1 {
                let (cx, cz) = (x as f32 * 512.0 + 256.0, z as f32 * 512.0 + 256.0);
                if patches.iter().any(|p| p.contains(cx, cz)) {
                    if map.is_water_tile(x, z) {
                        wet += 1;
                    } else {
                        dry += 1;
                    }
                }
            }
        }
        (wet, dry)
    };
    let other: Vec<Patch> = map
        .patches
        .iter()
        .map(|p| Patch {
            cos_sin: [p.cos_sin[0], -p.cos_sin[1]],
            ..*p
        })
        .collect();
    let (wet, dry) = score(&map.patches);
    let (wet_other, dry_other) = score(&other);
    eprintln!("sense {ROTATION_SENSE}: {wet} wet / {dry} dry; other: {wet_other} / {dry_other}");
    assert!(wet > wet_other && dry < dry_other);
    // The river through square 50,50 (scene tiles 128..192 x 64..128).
    let (flow, water_type) = map.flow_at((128 + 36) as f32 * 512.0, (64 + 50) as f32 * 512.0);
    assert!(flow[1] < -1.0 && water_type == 0, "{flow:?} {water_type}");
    assert_eq!(map.default_type, 0);
}

/// The planar image obeys the law of reflection (Q-WATER2): for points
/// above the plane, the pixel the reflected camera puts a point on is the
/// pixel whose view ray, reflected where it meets the plane, runs through
/// the point. (The headless `water_reflection_is_the_mirror_image` checks
/// the same over the rendered river.)
#[test]
fn reflection_image_obeys_the_law_of_reflection() {
    let (view, view_proj, eye) = camera_frame();
    let h = 300.0;
    let (_, reflected) = reflection_view_proj(view, view_proj, h, 0.0);
    let inv = view_proj.inverse();
    let mut worst = 0.0_f32;
    for i in 0..200 {
        // Points above the plane (classic y down) around the target.
        let t = i as f32;
        let p = glam::Vec3::new(
            (t * 37.0).sin() * 1500.0,
            h - 50.0 - (t * 11.0).cos().abs() * 800.0,
            (t * 53.0).cos() * 1500.0,
        );
        let c = clip(reflected, p);
        if c.x.abs() > 1.0 || c.y.abs() > 1.0 || !(0.0..=1.0).contains(&c.z) {
            continue;
        }
        let far = inv.project_point3(glam::Vec3::new(c.x, c.y, 1.0));
        let d = (far - eye).normalize();
        let s = eye + d * ((h - eye.y) / d.y);
        let r = glam::Vec3::new(d.x, -d.y, d.z);
        worst = worst.max(
            (p - s)
                .normalize()
                .dot(r)
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees(),
        );
    }
    assert!(worst < 0.05, "worst angle {worst} degrees");
}

/// The water mesh (Q-WATER2) gives every triangle three vertices of its own
/// carrying all three corners' flows in corner order and their own corner,
/// so along an edge two triangles share, each corner's flow is the same in
/// both: the slot blend is continuous across the edge.
#[test]
fn water_mesh_carries_the_corner_flows() {
    // Four floor vertices, two triangles sharing the edge 1-2.
    let attrs = [
        [10.0, 1.0, 0.0, 0.0],
        [20.0, 0.0, 1.0, 0.0],
        [30.0, -1.0, 0.0, 0.0],
        [40.0, 0.0, -1.0, 0.0],
    ];
    let beds = [
        Some(Bed {
            colour: [0.1, 0.2, 0.3],
            distance: 0.0,
        }),
        None,
        None,
        None,
    ];
    let mesh = water_mesh(&[(7, vec![0, 1, 2]), (9, vec![1, 3, 2])], &attrs, &beds);
    assert_eq!(mesh.ranges, vec![(7, 0, 3), (9, 3, 3)]);
    assert_eq!(mesh.sources, vec![0, 1, 2, 1, 3, 2]);
    for (i, a) in mesh.attrs.iter().enumerate() {
        let tri = &mesh.sources[i / 3 * 3..i / 3 * 3 + 3];
        assert_eq!(a[0], attrs[mesh.sources[i] as usize][0], "own depth");
        assert_eq!(a[3], (i % 3) as f32, "corner");
        let slots = [[a[1], a[2]], [a[4], a[5]], [a[6], a[7]]];
        for k in 0..3 {
            let f = attrs[tri[k] as usize];
            assert_eq!(slots[k], [f[1], f[2]], "slot {k} of vertex {i}");
        }
    }
    // The bed: vertex 0's own, the rest none (red -1).
    assert_eq!(&mesh.attrs[0][8..12], &[0.1, 0.2, 0.3, 0.0]);
    assert_eq!(mesh.attrs[1][8], -1.0);
    // The shared edge 1-2: its corners' flows agree between the triangles.
    let flow_of = |tri: usize, v: u32| {
        let corner = mesh.sources[tri * 3..tri * 3 + 3]
            .iter()
            .position(|&s| s == v)
            .unwrap();
        let a = mesh.attrs[tri * 3];
        [[a[1], a[2]], [a[4], a[5]], [a[6], a[7]]][corner]
    };
    for v in [1, 2] {
        assert_eq!(flow_of(0, v), flow_of(1, v));
    }
}

/// The per-type look (Q-WATER2, inferred fields): ops 2/4 are the normal
/// maps' repeat in tiles, op 6 the opaque colour's tint; without a type,
/// type 0's values.
#[test]
fn type_look_reads_the_repeat_and_the_colour() {
    let sea = rs910_config::nxt::water_type::NxtWaterType {
        op2: Some(12.0),
        op4: Some(6.0),
        op6_rgb: Some(0x0038_738f),
        ..Default::default()
    };
    let look = TypeLook::of(Some(&sea));
    assert_eq!(
        look.texture_scales,
        [1.0 / (12.0 * 512.0), 1.0 / (6.0 * 512.0)]
    );
    let lin = |v: u32| (v as f32 / 255.0).powf(2.2);
    assert_eq!(look.tint, [lin(0x38), lin(0x73), lin(0x8f)]);
    let default = TypeLook::of(None);
    assert_eq!(default.texture_scales, [1.0 / 3072.0; 2]);
    assert_eq!(default.tint, [lin(0xb7), lin(0xdb), lin(0xda)]);
}
