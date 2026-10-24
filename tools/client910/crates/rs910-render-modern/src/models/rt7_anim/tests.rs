//! Tests over the pack (CPU): NPC bodies (the shared sequenced-model
//! builder), player bodies (the identity-kit merges) and spot-anim models
//! drawn from RT7, unposed and through several animation frames.
use super::*;
use crate::models::rt7::Rt7Cache;
use rs910_scene::animation_assets::AnimationAssets;

/// The render flags a classic animation asks a model for: vertex label
/// groups (0x20) and face label groups (0x80, 0x100).
pub(crate) const ANIMATED: i32 = 0x20 | 0x180;

/// The pack's stores the model builders need.
pub(crate) struct Stores {
    pub pack: crate::cache::Pack,
    pub materials: MaterialStore,
    pub billboards: crate::billboard::BillboardStore,
    pub emitters: rs910_model::particle::EmitterStore,
    pub bases: std::collections::BTreeMap<i32, rs910_config::types910::bas_types::Bas>,
    pub npcs: rs910_config::config::NpcStore,
    pub animations: AnimationAssets,
}

impl Stores {
    pub(crate) fn load() -> Self {
        let pack = crate::test_support::require_pack("client.modelsrt7.js5");
        let raw = pack.read_group("config", 32).unwrap();
        let bases = raw
            .iter()
            .filter_map(|(&id, b)| {
                rs910_config::types910::bas_types::Bas::decode(id as i32, b)
                    .ok()
                    .map(|bas| (id as i32, bas))
            })
            .collect();
        Self {
            materials: MaterialStore::load(&pack).unwrap(),
            billboards: crate::billboard::BillboardStore::load(&pack).unwrap(),
            emitters: rs910_model::particle::EmitterStore::load(&pack).unwrap(),
            npcs: rs910_config::config::NpcStore::load(&pack).unwrap(),
            animations: AnimationAssets::load(&pack).unwrap(),
            bases,
            pack,
        }
    }

    /// NPC `npc`'s unanimated body (`npc_type_model::body`).
    pub(crate) fn npc_body(&self, npc: &rs910_config::config::Npc) -> Option<GpuModel> {
        let assets = rs910_scene::npc_type_model::Assets {
            pack: &self.pack,
            materials: &self.materials,
            billboards: &self.billboards,
            emitters: &self.emitters,
            bases: Some(&self.bases),
            detail: crate::gpumodel::MODEL_DETAIL_FLAGS,
        };
        rs910_scene::npc_type_model::body(
            npc,
            None,
            npc.bas,
            // With the label groups (0x20) an animation needs (lane
            // F-NPCANIM's `sequenced_flags`).
            rs910_scene::npc_type_model::sequenced_flags(0x20),
            &assets,
        )
        .ok()
        .flatten()
    }

    /// NPC `npc`'s body as the sequenced-model builder makes it for an
    /// animation (the flags are widened by the frame sets' render flags:
    /// [`ANIMATED`], the vertex and face label groups), for an NPC without
    /// per-slot offsets or BAS slot transforms. The shared builder
    /// ([`Self::npc_body`]) passes the plain flag set, without the label groups.
    pub(crate) fn npc_body_animated(&self, npc: &rs910_config::config::Npc) -> Option<GpuModel> {
        let parts: Vec<Option<ModelUnlit>> = npc
            .model_slots
            .iter()
            .map(|&id| {
                let mut raw = ModelUnlit::load(&self.pack, u32::try_from(id).ok()?).ok()?;
                if raw.version < 13 {
                    raw.scale_by_power_of_two(2);
                }
                Some(raw)
            })
            .collect();
        let raw = if parts.len() == 1 {
            parts.into_iter().next()??
        } else {
            ModelUnlit::merge_slots(&parts.iter().map(Option::as_ref).collect::<Vec<_>>())
        };
        GpuModel::new(
            &crate::gpumodel::ModelStores {
                materials: &self.materials,
                billboards: &self.billboards,
                emitters: &self.emitters,
            },
            &raw,
            crate::gpumodel::BuildParams {
                flags: 0x1F01F | ANIMATED,
                ambient: npc.ambient + 64,
                contrast: npc.contrast * 5 + 850,
                detail: crate::gpumodel::MODEL_DETAIL_FLAGS,
            },
        )
        .ok()
    }

    /// `model` posed at frame `frame` of classic sequence `seq`
    /// (the classic label ops, `classic_transforms_selected`, as
    /// `AnimationAssets` prepares them; `false` when it does not load).
    pub(crate) fn pose(&self, model: &mut GpuModel, seq: i32, frame: i32) -> bool {
        let Some(id) = self
            .animations
            .sequences
            .get(&seq)
            .and_then(|s| s.frame_ids.as_ref())
            .and_then(|f| f.get(frame as usize))
            .copied()
        else {
            return false;
        };
        let (set, index) = rs910_config::anim::split_frame_id(id as u32);
        let Ok(data) = rs910_config::anim::load_frameset(&self.pack, set) else {
            return false;
        };
        let Some((base, frame)) = data.frame(index) else {
            return false;
        };
        let ops = crate::gpumodel::classic_transforms_selected(
            crate::gpumodel::ClassicPose {
                base,
                frame,
                next: None,
                tick: 0,
                duration: 0,
            },
            crate::gpumodel::PoseTarget {
                normals: true,
                ..Default::default()
            },
        );
        if ops.is_empty() {
            return false;
        }
        model.apply_animation(&ops);
        true
    }

    /// The frame count of sequence `seq`.
    pub(crate) fn frames(&self, seq: i32) -> usize {
        self.animations
            .sequences
            .get(&seq)
            .and_then(|s| s.frame_ids.as_ref())
            .map_or(0, Vec::len)
    }
}

/// The raw classic models and RT7 copies of a model's parts.
pub(crate) type Parts = (Vec<Arc<ModelUnlit>>, Vec<Arc<Rt7Model>>);

/// The RT7 parts and raw models of `model`'s provenance.
pub(crate) fn sources(
    cache: &mut Rt7Cache,
    pack: &crate::cache::Pack,
    model: &GpuModel,
) -> Option<Parts> {
    let ids = model.source_ids.clone()?;
    let mut raws = Vec::new();
    let mut parts = Vec::new();
    for &id in ids.iter() {
        raws.push(cache.raw_model(pack, id as i32)?);
        parts.push(cache.rt7_model(pack, id as i32)?);
    }
    Some((raws, parts))
}

/// Pose `map` over `model` and check it: the triangles are the classic faces
/// (same posed corners), normals and tangents unit and finite.
fn pose_checked(map: &AnimMap, model: &GpuModel, materials: &MaterialStore) -> ModelStreams {
    let colours = model.colour_stream(materials).unwrap();
    let mut scratch = Scratch::default();
    let mut stats = AnimStats::default();
    let out = map
        .pose(model, &colours, &mut scratch, &mut stats)
        .expect("posed streams");
    assert!(
        map.check(model, &out),
        "triangles are not the classic faces"
    );
    for v in &out.vertices {
        let n = Vec3::from(v.normal);
        let t = Vec3::new(v.tangent[0], v.tangent[1], v.tangent[2]);
        assert!(n.is_finite() && t.is_finite());
        assert!(n == Vec3::ZERO || (n.length() - 1.0).abs() < 1e-3, "{n:?}");
    }
    out
}

/// The classic draw faces of `model` whose raw corners are not one point (the
/// faces RT7 has): what the map must draw.
fn expected_faces(model: &GpuModel, raws: &[Arc<ModelUnlit>]) -> usize {
    let mut offsets = Vec::new();
    let mut sum = 0;
    for r in raws {
        offsets.push(sum);
        sum += r.face_count as usize;
    }
    (0..model.draw_face_count as usize)
        .filter(|&f| {
            let sf = model.face_source[f] as usize;
            let p = offsets.partition_point(|&o| o <= sf) - 1;
            let lf = sf - offsets[p];
            let r = &raws[p];
            let v = [r.face_vertex1[lf], r.face_vertex2[lf], r.face_vertex3[lf]]
                .map(us)
                .map(|v| [r.vertex_x[v], r.vertex_y[v], r.vertex_z[v]]);
            !(v[0] == v[1] && v[1] == v[2])
        })
        .count()
}

/// NPC bodies from RT7. Unposed, every RT7 vertex of an NPC
/// without per-slot offsets or BAS slot transforms sits on its classic vertex
/// (the correspondence is right, not only consistent); posed through their
/// stand and walk frames, every triangle is its classic face in every frame,
/// one per classic draw face; a whole-model yaw turns every normal exactly by
/// the yaw (the label-group fit is NXT's `mat3(bone)`), and no RT7 corner
/// carries another label than its classic vertex.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn rt7_npc_bodies_follow_their_classic_models_through_their_animations() {
    let stores = Stores::load();
    let mut cache = Rt7Cache::default();
    let npcs: Vec<rs910_config::config::Npc> = stores
        .npcs
        .iter()
        .map(|(_, n)| n)
        .filter(|n| !n.models.is_empty() && n.multinpc.is_empty() && n.bas >= 0)
        .step_by(97)
        .take(60)
        .cloned()
        .collect();
    let (mut built, mut exact, mut frames, mut rejected) = (0, 0, 0, Vec::new());
    let (mut fit, mut faces_rot) = (0, 0);
    let mut worst_yaw = 1.0_f32;
    let mut labels = 0;
    let mut moved = 0;
    for npc in &npcs {
        let Some(base) = stores.npc_body(npc) else {
            continue;
        };
        let (raws, parts) = sources(&mut cache, &stores.pack, &base).expect("npc sources");
        let map = match build_map(&base, &raws, &parts) {
            Ok(map) => map,
            Err(why) => {
                rejected.push((npc.id, why));
                continue;
            }
        };
        built += 1;
        labels += map.label_mismatches;
        assert_eq!(
            map.triangles(),
            expected_faces(&base, &raws),
            "npc {}",
            npc.id
        );
        let out = pose_checked(&map, &base, &stores.materials);
        // Unposed without offsets: the classic vertex is the RT7 vertex.
        let placed = npc.modeloffset.is_some()
            || stores
                .bases
                .get(&npc.bas)
                .is_some_and(|b| b.slot_transforms.is_some());
        if !placed {
            for (v, vertex) in out.vertices.iter().enumerate() {
                assert_eq!(
                    Vec3::from(vertex.pos),
                    map.base[v],
                    "npc {}: vertex {v} is not its RT7 vertex",
                    npc.id
                );
            }
            exact += 1;
        }
        // A whole-model yaw (`rotate_y_keep_normals`, 90 degrees): every normal turns by it.
        let mut turned = base.clone();
        turned.rotate_y_keep_normals(4096);
        let yawed = pose_checked(&map, &turned, &stores.materials);
        for (a, b) in out.vertices.iter().zip(&yawed.vertices) {
            let (a, b) = (Vec3::from(a.normal), Vec3::from(b.normal));
            if a == Vec3::ZERO {
                continue;
            }
            // The classic yaw by 4096: (x, z) -> (z, -x) or (-z, x); take the
            // better of the two senses.
            let want = [Vec3::new(a.z, a.y, -a.x), Vec3::new(-a.z, a.y, a.x)];
            let dot = want.map(|w| w.dot(b)).into_iter().fold(-1.0, f32::max);
            worst_yaw = worst_yaw.min(dot);
        }
        // Stand and walk frames, on the body built with the animation's
        // flags (its label groups); the plain NPCs' two builds hold the
        // same faces and vertices.
        let bas = stores.bases.get(&npc.bas).cloned();
        let base = if placed {
            continue;
        } else {
            stores.npc_body_animated(npc).expect("animated body")
        };
        let map = {
            let (raws, parts) = sources(&mut cache, &stores.pack, &base).unwrap();
            build_map(&base, &raws, &parts).expect("animated body map")
        };
        for seq in bas.iter().flat_map(|b| [b.readyanim, b.walkanim]) {
            let n = stores.frames(seq);
            for frame in (0..n).step_by((n / 3).max(1)).take(3) {
                let mut posed = base.clone();
                if !stores.pose(&mut posed, seq, frame as i32) {
                    continue;
                }
                moved += usize::from(posed.vx != base.vx || posed.vy != base.vy);
                let colours = posed.colour_stream(&stores.materials).unwrap();
                let mut scratch = Scratch::default();
                let mut stats = AnimStats::default();
                let out = map
                    .pose(&posed, &colours, &mut scratch, &mut stats)
                    .unwrap();
                assert!(
                    map.check(&posed, &out),
                    "npc {} seq {seq} frame {frame}",
                    npc.id
                );
                fit += stats.fit_groups;
                faces_rot += stats.face_groups;
                frames += 1;
            }
        }
    }
    eprintln!(
        "{moved} posed frames moved vertices; {built} of {} NPC bodies mapped ({exact} checked vertex for vertex unposed), {frames} animation frames posed ({fit} label groups by their fit, {faces_rot} by their triangles), yaw worst normal dot {worst_yaw:.5}, {labels} label mismatches; rejected {rejected:?}",
        npcs.len()
    );
    assert!(built * 100 >= npcs.len() * 95, "{built} of {}", npcs.len());
    assert!(
        exact > 10 && frames > 50 && moved * 10 >= frames * 9,
        "{moved} of {frames} moved"
    );
    assert!(worst_yaw > 0.999, "{worst_yaw}");
    assert_eq!(labels, 0);
}

/// Player bodies from RT7. The body merges the wear slots
/// (`merge_slots` with holes) of identity kits that are each a merge of
/// their models, then lights the result at 64/850 (`player_model.rs`
/// `Models::body`, reproduced here through the same public pieces: its
/// `Appearance` lives in rs910-game,
/// below this crate's layer). The default male and female kits map one to
/// one (every RT7 vertex on its classic vertex unposed) and follow a walk
/// cycle.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn rt7_player_bodies_follow_their_classic_models() {
    let stores = Stores::load();
    let mut cache = Rt7Cache::default();
    let models =
        rs910_scene::player_model::Models::load(&stores.pack, crate::gpumodel::MODEL_DETAIL_FLAGS)
            .unwrap();
    let kit = |id: i32| -> Option<ModelUnlit> {
        let ids = models.identity.get(&id)?.models.clone()?;
        let parts: Vec<ModelUnlit> = ids
            .iter()
            .map(|&m| {
                let mut raw = ModelUnlit::load(&stores.pack, m as u32).unwrap();
                if raw.version < 13 {
                    raw.scale_by_power_of_two(2);
                }
                raw
            })
            .collect();
        Some(if parts.len() == 1 {
            parts.into_iter().next().unwrap()
        } else {
            ModelUnlit::merge(&parts.iter().collect::<Vec<_>>())
        })
    };
    let mut checked = 0;
    // The classic default kits (hair, jaw, torso, arms, hands, legs, feet)
    // in their wear slots, the other slots holes.
    for ids in [[0, 10, 18, 26, 33, 36, 42], [45, -1, 56, 61, 67, 70, 79]] {
        let mut slots: Vec<Option<ModelUnlit>> = vec![None; 12];
        for (slot, id) in [8, 11, 4, 6, 9, 7, 10].into_iter().zip(ids) {
            if id >= 0 {
                slots[slot] = kit(id);
            }
        }
        let raw = ModelUnlit::merge_slots(&slots.iter().map(Option::as_ref).collect::<Vec<_>>());
        let body = GpuModel::new(
            &crate::gpumodel::ModelStores {
                materials: &stores.materials,
                billboards: &stores.billboards,
                emitters: &stores.emitters,
            },
            &raw,
            crate::gpumodel::BuildParams {
                flags: 0x1F01F | 0x4000 | ANIMATED,
                ambient: 64,
                contrast: 850,
                detail: crate::gpumodel::MODEL_DETAIL_FLAGS,
            },
        )
        .unwrap();
        let ids = body.source_ids.clone().expect("player provenance");
        assert!(ids.len() >= 6, "one or more models per kit: {ids:?}");
        let (raws, parts) = sources(&mut cache, &stores.pack, &body).unwrap();
        let map = build_map(&body, &raws, &parts).expect("player map");
        assert_eq!(map.label_mismatches, 0);
        assert_eq!(map.triangles(), expected_faces(&body, &raws));
        let out = pose_checked(&map, &body, &stores.materials);
        for (v, vertex) in out.vertices.iter().enumerate() {
            assert_eq!(Vec3::from(vertex.pos), map.base[v], "vertex {v}");
        }
        // The player walk (sequence 819, the human skeleton's walk).
        for frame in 0..stores.frames(819).min(4) {
            let mut posed = body.clone();
            if stores.pose(&mut posed, 819, frame as i32) {
                assert!(
                    posed.vx != body.vx || posed.vy != body.vy,
                    "the walk moves the body"
                );
                pose_checked(&map, &posed, &stores.materials);
                checked += 1;
            }
        }
    }
    eprintln!("{checked} player walk frames posed from RT7");
    assert!(checked >= 4);
}
