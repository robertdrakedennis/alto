use super::*;

/// Opcode stream covering every emitter opcode branch.
fn full_emitter_bytes() -> Vec<u8> {
    let mut b = vec![1, 0, 10, 0, 20, 0, 30, 0, 40]; // angles
    b.extend([2, 9]);
    b.extend([3, 0, 0, 0, 100, 0, 0, 1, 0]); // speed 100..256
    b.extend([4, 1, 0xFE]); // damping mode 1, -2
    b.extend([6, 0x80, 0xFF, 0x00, 0x00, 0xFF, 0x00, 0xFF, 0x00]);
    b.extend([7, 0, 10, 0, 20]);
    b.extend([8, 0, 64, 0, 128]);
    b.extend([9, 2, 0, 1, 0, 2]);
    b.extend([10, 1, 0, 3]);
    b.extend([12, 0xFF, 13, 1, 14, 0, 5, 15, 0, 77]);
    b.extend([16, 0, 0, 5, 0, 50, 1]);
    b.extend([17, 0, 9, 18, 0x40, 0x00, 0x00, 0xFF, 19, 1, 20, 50, 21, 25]);
    b.extend([22, 0, 0, 0, 200, 23, 10, 24, 25, 1, 0, 4, 26]);
    b.extend([27, 0, 2, 28, 40, 29, 1, 0xFF, 0xFF, 0, 2, 30]);
    b.extend([31, 0, 1, 0, 3, 32, 33, 34, 35, 1, 0, 1, 0, 9, 3, 36, 0]);
    b
}

#[test]
fn emitter_decode_covers_every_opcode_and_derived_field() {
    let t = EmitterType::decode(&full_emitter_bytes()).unwrap();
    assert_eq!(
        (t.yaw_min, t.yaw_max, t.pitch_min, t.pitch_max),
        (80, 160, 240, 320)
    );
    assert_eq!((t.speed_min, t.speed_max), (100, 256));
    assert_eq!((t.damping_mode, t.damping), (1, -2));
    assert_eq!(
        (t.colour_min, t.colour_max),
        (0x80FF_0000_u32 as i32, 0xFF00_FF00_u32 as i32)
    );
    assert_eq!(
        (t.lifetime_min, t.lifetime_max, t.rate_min, t.rate_max),
        (10, 20, 64, 128)
    );
    assert_eq!(t.local_effectors.as_deref(), Some(&[1, 2][..]));
    assert_eq!(t.constant_effectors.as_deref(), Some(&[3][..]));
    assert_eq!(t.global_effectors.as_deref(), Some(&[4][..]));
    assert_eq!(
        (t.ceiling_level, t.floor_level, t.initial_burst, t.texture),
        (-1, 1, 5, 77)
    );
    assert_eq!(
        (
            t.active_before_threshold,
            t.period_threshold,
            t.period,
            t.period_repeat
        ),
        (false, 5, 50, true)
    );
    assert_eq!(
        (t.low_detail_type, t.target_colour, t.min_setting),
        (9, 0x4000_00FF, 1)
    );
    assert_eq!(
        (
            t.colour_percent,
            t.alpha_percent,
            t.target_speed,
            t.target_speed_percent
        ),
        (50, 25, 200, 10)
    );
    assert!(!t.uniform_colour && !t.unused_flag && t.keep_texture && !t.lit);
    assert!(t.entity_collision && !t.ground_collision && t.remove_face);
    assert_eq!((t.target_size, t.target_size_percent), (2 << 14, 40));
    assert_eq!((t.spin_min, t.spin_max), (-8, 16));
    assert_eq!((t.size_min, t.size_max), (1 << 14, 3 << 14));
    assert_eq!((t.angle_min, t.angle_max, t.angle_steps), (8, 72, 3));
    // The derived fields, hand-computed.
    assert!(t.level_collision);
    assert_eq!(
        (t.red, t.red_range, t.green, t.green_range),
        (0xFF, -0xFF, 0, 0xFF)
    );
    assert_eq!((t.alpha, t.alpha_range), (0x80, 0x7F));
    assert_eq!((t.colour_cycles, t.alpha_cycles), (10, 5));
    // red: ((0 - (-127 + 255)) << 8) / 10 = -3276 -> +4.
    assert_eq!(t.red_step, -3272);
    // green: ((0 - (127 + 0)) << 8) / 10 = -3251 -> +4.
    assert_eq!(t.green_step, -3247);
    // blue: ((0xFF - 0) << 8) / 10 = 6528 -> -4.
    assert_eq!(t.blue_step, 6524);
    // alpha: ((0x40 - (63 + 128)) << 8) / 5 = -6502 (truncating division) -> +4.
    assert_eq!(t.alpha_step, -6498);
    assert_eq!((t.speed_cycles, t.speed_step), (2, (200 - (78 + 100)) / 2));
    // (target - ((max - min) / 2 + min)) / cycles;
    // the fixture's target 2 << 14 is exactly the size midpoint
    // ((3 << 14) - (1 << 14)) / 2 + (1 << 14), so the step is zero.
    assert_eq!((t.size_cycles, t.size_step), (8, 0));
    assert_eq!((t.angle_range, t.spin_range), (64, 24));
}

#[test]
fn effector_decode_and_derived_fields() {
    let bytes = [
        1, 0, 128, 2, 7, 3, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0, 0, 4, 1, 0, 0, 0, 2, 6, 1, 8, 9, 10, 0,
    ];
    let t = EffectorType::decode(12, Some(&bytes)).unwrap();
    assert_eq!((t.id, t.kind, t.cone, t.force), (12, 1, 128, [3, 4, 0]));
    assert_eq!(
        (t.falloff_mode, t.falloff, t.positional, t.radial),
        (1, 2, 1, 1)
    );
    assert!(t.invert);
    assert_eq!(t.magnitude, -5);
    // (5 * 8 / 2)^2
    assert_eq!(t.range, 400);
    assert_eq!(t.cone_cos, crate::trig::cos(1024));
    let d = EffectorType::decode(3, None).unwrap();
    assert_eq!((d.range, d.falloff, d.cone_cos), (2_147_483_647, 1, 16384));
}

struct Flat;
impl ParticleScene for Flat {
    fn size(&self) -> i32 {
        9
    }
    fn max_tile_x(&self) -> i32 {
        104
    }
    fn max_tile_z(&self) -> i32 {
        104
    }
    fn max_level(&self) -> i32 {
        4
    }
    fn tile_height(&self, level: i32, _: i32, _: i32) -> i32 {
        -240 * level
    }
    fn tile_level(&self, level: i32, _: i32, _: i32) -> Option<i32> {
        Some(level)
    }
    fn bridged(&self, _: i32, _: i32) -> bool {
        false
    }
    fn create_tile(&mut self, _: i32, _: i32, _: i32, _: i32) {}
    fn bounds_contain(&self, _: i32, _: i32, _: i32, _: [i32; 3]) -> bool {
        false
    }
}

/// Records the tiles `allocate_tiles` creates over a sparse tile set.
#[derive(Default)]
struct Sparse {
    tiles: std::collections::BTreeMap<i32, i32>,
    bridge: bool,
    created: Vec<(i32, i32)>,
}
impl ParticleScene for Sparse {
    fn size(&self) -> i32 {
        9
    }
    fn max_tile_x(&self) -> i32 {
        104
    }
    fn max_tile_z(&self) -> i32 {
        104
    }
    fn max_level(&self) -> i32 {
        4
    }
    fn tile_height(&self, level: i32, _: i32, _: i32) -> i32 {
        -240 * level
    }
    fn tile_level(&self, level: i32, _: i32, _: i32) -> Option<i32> {
        self.tiles.get(&level).copied()
    }
    fn bridged(&self, _: i32, _: i32) -> bool {
        self.bridge
    }
    fn create_tile(&mut self, plane: i32, _: i32, _: i32, level: i32) {
        self.tiles.insert(plane, level);
        self.created.push((plane, level));
    }
    fn bounds_contain(&self, _: i32, _: i32, _: i32, _: [i32; 3]) -> bool {
        false
    }
}

fn fixed_type() -> EmitterType {
    // 4 << 20: ((4 << 22) * 32767) >> 23 = 16383 fixed = 4 fine units a cycle.
    let mut t = EmitterType {
        speed_min: 4 << 20,
        speed_max: 4 << 20,
        lifetime_min: 30,
        lifetime_max: 30,
        rate_min: 64,
        rate_max: 64,
        size_min: 4 << 14,
        size_max: 4 << 14,
        colour_min: 0xFF80_4020_u32 as i32,
        colour_max: 0xFF80_4020_u32 as i32,
        ..EmitterType::default()
    };
    t.finish();
    t
}

fn anchor(y: i32) -> EmitterAnchor {
    // Counter-clockwise in X/Z with Y down: the normal points -Y (up).
    EmitterAnchor {
        id: 1,
        particle: 0,
        vertices: [[5000, y, 5000], [5064, y, 5000], [5000, y, 5064]],
    }
}

fn runtime_with(t: EmitterType) -> Runtime {
    let mut emitters = EmitterStore::default();
    emitters.entries.insert(0, Arc::new(t));
    Runtime::new(emitters, EffectorStore::default(), 7)
}

#[test]
fn simulation_step_matches_fixed_point() {
    // One particle, straight integration.
    let t = fixed_type();
    let mut p = MovingParticle {
        pos: [5000 << 12, -300 << 12, 5000 << 12],
        colour: 0,
        colour_frac: 0,
        lifetime: 30,
        remaining: 30,
        size: 4 << 14,
        angle: 0,
        spin: 0,
        texture: -1,
        lit: true,
        dir: [0, -32767, 0],
        speed: 1000,
        slot: 0,
        serial: 1,
    };
    let mut rng = AnimationRandom::new(1);
    let mut pool = ParticlePool::new();
    let ctx = Tick {
        detail: 2,
        previous_total: 0,
        current_total: 0,
        globals: &[],
        constants: &[],
        rng: &mut rng,
        pool: &mut pool,
    };
    assert!(p.advance(2, &t, [5021, -300, 5021], &[], &[], &ctx));
    // ((1000 << 2) * -32767 >> 23) * 2 = (-131068000 >> 23) * 2 = -16 * 2.
    assert_eq!(p.pos[1], (-300 << 12) - 32);
    assert_eq!(p.remaining, 28);
    // Constant gravity (kind 2, velocity) renormalises the direction.
    let gravity = Arc::new(EffectorType {
        kind: 2,
        force: [0, 20000, 0],
        ..EffectorType::default()
    });
    let consts = [gravity];
    let ctx = Tick {
        detail: 2,
        previous_total: 0,
        current_total: 0,
        globals: &[],
        constants: &consts,
        rng: &mut rng,
        pool: &mut pool,
    };
    let before = p.pos[1];
    assert!(p.advance(1, &t, [0; 3], &[], &[0], &ctx));
    // -32767 + 20000 = -12767 stays inside +-32767: no halving.
    assert_eq!(p.dir, [0, -12767, 0]);
    assert_eq!(p.speed, 1000);
    assert_eq!(p.pos[1], before + ((4000_i64 * -12767) >> 23) as i32);
    // Colour fade towards the target.
    let mut faded = EmitterType {
        lifetime_max: 10,
        colour_min: 0xFF00_0000_u32 as i32,
        colour_max: 0xFF00_0000_u32 as i32,
        target_colour: 0x00FF_0000,
        ..EmitterType::default()
    };
    faded.finish();
    // red: ((255 - 0) << 8) / 10 = 6528 -> 6524; alpha: ((0 - 255) << 8)/10 = -6528 -> -6524.
    assert_eq!((faded.red_step, faded.alpha_step), (6524, -6524));
    let mut q = p;
    q.colour = 0xFF00_0000_u32 as i32;
    q.colour_frac = 0;
    q.lifetime = 10;
    q.remaining = 10;
    let ctx = Tick {
        detail: 2,
        previous_total: 0,
        current_total: 0,
        globals: &[],
        constants: &[],
        rng: &mut rng,
        pool: &mut pool,
    };
    assert!(q.advance(1, &faded, [0; 3], &[], &[], &ctx));
    // red 8.8 = 6524 -> 0x19 whole, 0x7C fraction; alpha 0xFF00 - 6524 = 58756.
    assert_eq!(q.colour, (58756 & 0xFF00) << 16 | 0x19 << 16);
    // green/blue steps are 0 -> +4.
    assert_eq!(
        q.colour_frac,
        (58756 & 0xFF) << 24 | 0x7C << 16 | 0x04 << 8 | 0x04
    );
}

#[test]
fn system_lifecycle_spawns_ages_and_times_out() {
    let mut rt = runtime_with(fixed_type());
    let a = [anchor(-300)];
    // Frame 0: bind creates the system.
    rt.bind(9, 100, 0, &a, &[]);
    assert!(rt.has_system(9));
    // Each frame: tick (bound last cycle -> spawn), then rebind.
    for cycle in 101..=110 {
        rt.tick(cycle);
        rt.bind(9, cycle, 0, &a, &[]);
    }
    // Rate 64/64 per cycle plus the random 0..63 accumulator start: 9 or 10.
    let live = rt.stats().live;
    assert!((9..=10).contains(&live), "live {live}");
    // Draw list culls nothing above flat ground at y = 0.
    rt.collect(&mut Flat);
    assert_eq!(rt.stats().drawn, live);
    let (_, list) = rt.lists().next().unwrap();
    // Particles rise (normal -Y): later particles are lower (older rise more).
    assert!(list[0].pos[1] < list[list.len() - 1].pos[1]);
    // The owner stops binding: spawning stops, particles age out.
    for cycle in 111..=150 {
        rt.tick(cycle);
    }
    assert_eq!(rt.stats().live, 0);
    assert!(rt.has_system(9));
    // More than 750 cycles since the last bind kills the system.
    rt.tick(861);
    assert!(!rt.has_system(9));
}

#[test]
fn detail_setting_and_limits_gate_spawning() {
    let mut t = fixed_type();
    t.min_setting = 1;
    let mut rt = runtime_with(t);
    rt.set_detail(0);
    let a = [anchor(-300)];
    rt.bind(1, 0, 0, &a, &[]);
    for cycle in 1..20 {
        rt.tick(cycle);
        rt.bind(1, cycle, 0, &a, &[]);
    }
    assert_eq!(rt.stats().live, 0);
    rt.set_detail(1);
    for cycle in 20..30 {
        rt.tick(cycle);
        rt.bind(1, cycle, 0, &a, &[]);
    }
    assert!(rt.stats().live > 0);
    rt.set_detail(7);
    assert_eq!(rt.detail(), 0);
}

#[test]
fn unmatched_emitters_die_and_ground_collision_kills() {
    let mut rt = runtime_with(fixed_type());
    let a = [anchor(-300)];
    rt.bind(2, 0, 0, &a, &[]);
    for cycle in 1..5 {
        rt.tick(cycle);
        rt.bind(2, cycle, 0, &a, &[]);
    }
    assert!(rt.stats().live > 0);
    // The model loses its emitter: the old one keeps its particles, dying.
    rt.tick(5);
    rt.bind(2, 5, 0, &[], &[]);
    assert_eq!(rt.stats().emitters, 1);
    for cycle in 6..40 {
        rt.tick(cycle);
        rt.bind(2, cycle, 0, &[], &[]);
    }
    assert_eq!(rt.stats().emitters, 0);
    // Particles spawned below ground (y > height 0) die in the kill test.
    let mut rt = runtime_with(fixed_type());
    let low = [anchor(3000)];
    rt.bind(3, 0, 0, &low, &[]);
    for cycle in 1..5 {
        rt.tick(cycle);
        rt.bind(3, cycle, 0, &low, &[]);
    }
    rt.collect(&mut Flat);
    assert_eq!(rt.stats().drawn, 0);
    assert_eq!(rt.stats().live, 0);
}

#[test]
fn local_effector_pushes_particles() {
    let mut t = fixed_type();
    t.local_effectors = Some(vec![5]);
    let mut emitters = EmitterStore::default();
    emitters.entries.insert(0, Arc::new(t));
    let mut effectors = EffectorStore::default();
    // Cone 1024 (180 degrees: every direction), force (4096, 0, 0).
    let bytes = [1, 0x04, 0x00, 3, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    effectors
        .entries
        .insert(5, Arc::new(EffectorType::decode(5, Some(&bytes)).unwrap()));
    let mut rt = Runtime::new(emitters, effectors, 3);
    let a = [anchor(-300)];
    let e = [EffectorAnchor {
        id: 1,
        effector: 5,
        position: [5000, -300, 5000],
        rotation: Rotation::IDENTITY,
    }];
    rt.bind(5, 0, 0, &a, &e);
    for cycle in 1..=10 {
        rt.tick(cycle);
        rt.bind(5, cycle, 0, &a, &e);
    }
    rt.collect(&mut Flat);
    let (_, list) = rt.lists().next().unwrap();
    // The +X force (4096 * 1) bends every particle towards +X.
    assert!(list.iter().all(|p| (p.pos[0] >> 12) >= 5000));
    assert!(list.iter().any(|p| (p.pos[0] >> 12) > 5064));
}

#[test]
fn rotation_matches_model_rotations() {
    let r = Rotation::yaw(4096);
    let v = r.apply(100.0, 0.0, 0.0);
    // rotate_y_keep_normals at 90 degrees: x' = z*sin + x*cos, z' = z*cos - x*sin.
    assert!(v[0].abs() < 0.01 && (v[2] + 100.0).abs() < 0.01);
    let p = Rotation::pitch(4096).apply(0.0, 100.0, 0.0);
    assert!(p[1].abs() < 0.01 && (p[2] - 100.0).abs() < 0.01);
    let both = Rotation::pitch(4096)
        .then(&Rotation::yaw(4096))
        .apply(0.0, 100.0, 0.0);
    assert!((both[0] - 100.0).abs() < 0.01);
}

#[test]
fn collision_pass_allocates_empty_tiles() {
    // A missing plane-2 tile over an
    // unbuilt column allocates planes 0..=2 and tests plane 2.
    let mut scene = Sparse::default();
    assert_eq!(allocate_tiles(&mut scene, 2, 1, 1), 2);
    assert_eq!(scene.created, vec![(0, 0), (1, 1), (2, 2)]);
    // An existing tile is tested as is.
    let mut scene = Sparse::default();
    scene.tiles.insert(1, 1);
    assert_eq!(allocate_tiles(&mut scene, 1, 1, 1), 1);
    assert!(scene.created.is_empty());
    // Bridged column: plane 3 drops to 2, new tiles' level is bumped and
    // with plane 2 present but plane 1 missing, the plane-1 tile (the
    // last allocated) is the one tested.
    let mut scene = Sparse {
        bridge: true,
        ..Sparse::default()
    };
    scene.tiles.insert(0, 1);
    scene.tiles.insert(2, 3);
    assert_eq!(allocate_tiles(&mut scene, 3, 1, 1), 1);
    assert_eq!(scene.created, vec![(1, 2)]);
}

#[test]
fn released_particles_keep_their_colour_fraction() {
    let mut pool = ParticlePool::new();
    assert_eq!(pool.take(), 0, "empty ring: a new MovingParticle");
    pool.release(0x1234_5678);
    pool.release(7);
    assert_eq!(pool.take(), 0x1234_5678);
    assert_eq!(pool.take(), 7);
    assert_eq!(pool.take(), 0);
    // The write index laps the read index without a check: 1024 releases
    // leave the ring looking empty (write index == read index).
    for i in 0..PARTICLE_POOL as i32 {
        pool.release(i);
    }
    assert_eq!(pool.take(), 0);
    pool.release(99);
    assert_eq!(
        pool.take(),
        99,
        "only the entry written after the lap is readable"
    );
    // A timed-out system hands its occupants to the ring in slot order.
    let mut rt = runtime_with(fixed_type());
    let a = [anchor(0)];
    rt.bind(1, 0, 0, &a, &[]);
    for cycle in 1..=5 {
        rt.tick(cycle);
        rt.bind(1, cycle, 0, &a, &[]);
    }
    let live = rt.stats().live;
    assert!(live > 0);
    let before = rt.particles.write;
    rt.tick(5 + SYSTEM_TIMEOUT + 1);
    assert_eq!(rt.particles.write, (before + live) & (PARTICLE_POOL - 1));
}

#[test]
fn recycled_systems_stay_dead_until_the_pool_drains() {
    let mut rt = runtime_with(fixed_type());
    let a = [anchor(0)];
    rt.bind(1, 0, 0, &a, &[]);
    rt.tick(1);
    // The owner stops drawing: the system times out into the recycle ring.
    rt.tick(SYSTEM_TIMEOUT + 2);
    assert!(!rt.has_system(1));
    assert_eq!((rt.pool_read, rt.pool_write), (0, 1));
    // The recycle ring hands the dead system back: binding does nothing.
    let cycle = SYSTEM_TIMEOUT + 3;
    rt.bind(2, cycle, 0, &a, &[]);
    assert!(!rt.has_system(2));
    assert_eq!(rt.stats().emitters, 0);
    // Next draw: the ring is empty, so a new system binds.
    rt.tick(cycle + 1);
    rt.bind(2, cycle + 1, 0, &a, &[]);
    assert!(rt.has_system(2));
    assert_eq!(rt.stats().emitters, 1);
    // The replaced dead system stays listed until it times out.
    assert_eq!(rt.stats().systems, 2);
    rt.tick(cycle + SYSTEM_TIMEOUT + 1);
    rt.bind(2, cycle + SYSTEM_TIMEOUT + 1, 0, &a, &[]);
    assert_eq!(rt.stats().systems, 1);
    assert_eq!(rt.pool_write, 2);
}

#[test]
fn world_reset_kills_systems_until_owners_rebind() {
    let mut rt = runtime_with(fixed_type());
    let a = [anchor(0)];
    rt.bind(1, 0, 0, &a, &[]);
    rt.bind(2, 0, 0, &a, &[]);
    rt.tick(1);
    rt.reset();
    assert!(!rt.has_system(1) && !rt.has_system(2));
    assert_eq!(rt.stats().systems, 2, "dead systems stay listed");
    // A culled owner keeps its dead system alive.
    rt.touch(2, SYSTEM_TIMEOUT, 0);
    rt.bind(1, 2, 0, &a, &[]);
    assert!(rt.has_system(1));
    rt.tick(SYSTEM_TIMEOUT + 3);
    // Owner 1's replaced dead system timed out; owner 2's did not; owner
    // 1's new system was not rebound either and timed out as well.
    assert_eq!(rt.stats().systems, 1);
    assert_eq!(rt.pool_write, 2);
}

#[test]
fn interface_systems_tick_but_skip_the_scene_pass() {
    // An interface model's system is not a scene system: it draws the slot
    // ring.
    let mut rt = runtime_with(fixed_type());
    let key = keys::component(1);
    let a = [anchor(0)];
    rt.bind_interface(key, 0, &a, &[]);
    for cycle in 1..=5 {
        rt.tick(cycle);
        rt.bind_interface(key, cycle, &a, &[]);
    }
    rt.collect(&mut Flat);
    assert!(
        rt.list(key).is_empty(),
        "the scene pass skips non-scene systems"
    );
    assert_eq!(rt.lists().count(), 0);
    let slots = rt.slot_list(key);
    assert_eq!(slots.len(), rt.stats().live);
    assert!(!slots.is_empty());
}
