//! Cache of decoded animation resources (sequences, classic frame sets,
//! skeletal key-frame sets), retaining each node's loader readiness.
use crate::{
    anim,
    animation_skeletal::SkeletalPose,
    cache::Pack,
    entities910::animation_state::Node,
    gpumodel::{classic_transforms_selected, ClassicPose, PoseTarget, Transform},
    protocol910::sequence_types::Sequence,
};
use std::collections::BTreeMap;

pub struct AnimationAssets {
    pub sequences: BTreeMap<i32, Sequence>,
    pub groups: BTreeMap<i32, crate::protocol910::sequence_types::Group>,
    frames: BTreeMap<u32, anim::FrameSetData>,
    skeletal: BTreeMap<u32, SkeletalPose>,
}
#[derive(Clone, Debug, Default)]
pub struct Playback {
    pub node: Node,
    /// A resource in the shared cache is not necessarily loaded by this node.
    pub loaded_skeletal: Option<i32>,
}
pub struct Pose {
    pub base_id: Option<u32>,
    pub base_identity: Option<BaseIdentity>,
    pub flags: i32,
    pub transforms: Vec<Transform>,
}
/// A classic frame set shares its animation bases only within that one frame
/// set; a key-frame set allocates a separate base object. Tween checks compare
/// those base objects, not merely their archive IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseIdentity {
    Classic { frame_set: u32, base: u32 },
    Skeletal { keyframe_set: u32 },
}
/// Model pose inputs for player blending and wear-slot overlays.
#[derive(Clone, Copy)]
pub struct Filter<'a> {
    pub blend: Option<(&'a [bool], bool)>,
    pub part_mask: i32,
    pub normals: Option<bool>,
    pub wrap_skeletal: bool,
    /// The secondary tween frame is compared to the primary base.
    pub next_base: Option<BaseIdentity>,
}
impl Default for Filter<'_> {
    fn default() -> Self {
        Self {
            blend: None,
            part_mask: 65535,
            normals: None,
            wrap_skeletal: false,
            next_base: None,
        }
    }
}
impl AnimationAssets {
    #[cfg(any(test, feature = "test-hooks"))] // test-only introspection
    pub fn used_archives(&self) -> Vec<(&'static str, Vec<u32>)> {
        let mut bases = std::collections::BTreeSet::new();
        for set in self.frames.values() {
            bases.extend(set.bases.keys().copied());
        }
        for pose in self.skeletal.values() {
            bases.insert(pose.set.base_id);
        }
        vec![
            (anim::ANIMS_ARCHIVE, self.frames.keys().copied().collect()),
            (anim::BASES_ARCHIVE, bases.into_iter().collect()),
            (
                anim::ANIMS_KEYFRAMES_ARCHIVE,
                self.skeletal.keys().copied().collect(),
            ),
        ]
    }
    pub fn load(pack: &Pack) -> anyhow::Result<Self> {
        let loaded = crate::animation_sequences::load(pack)?;
        Ok(Self {
            sequences: loaded.sequences,
            groups: loaded.groups,
            frames: BTreeMap::new(),
            skeletal: BTreeMap::new(),
        })
    }
    pub fn range(&self, p: &Playback) -> Option<(i32, i32)> {
        let id = p.loaded_skeletal?;
        let set = &self.skeletal.get(&(id as u32))?.set;
        Some((i32::from(set.start), i32::from(set.end)))
    }
    fn frame_set(&mut self, pack: &Pack, id: u32) -> anyhow::Result<()> {
        if let std::collections::btree_map::Entry::Vacant(slot) = self.frames.entry(id) {
            slot.insert(anim::load_frameset(pack, id)?);
        }
        Ok(())
    }
    pub fn prepare(&mut self, pack: &Pack, p: &mut Playback, angle: i32) -> anyhow::Result<Pose> {
        self.prepare_filtered(pack, p, angle, Filter::default())
    }
    /// Actor-owned loader state: cache residency alone does not advance a node.
    pub fn actor_pose(
        &mut self,
        pack: &Pack,
        node: &mut Node,
        filter: Filter<'_>,
    ) -> anyhow::Result<Pose> {
        let mut playback = Playback {
            node: node.clone(),
            loaded_skeletal: None,
        };
        let pose = self.prepare_filtered(pack, &mut playback, 0, filter)?;
        node.skeletal_range = playback.node.skeletal_range;
        Ok(pose)
    }
    /// Only the current classic frame affects a character shadow. Player main/walk nodes disable secondary frame sets.
    pub fn shadow(
        &mut self,
        pack: &Pack,
        node: &Node,
        model: &mut crate::gpumodel::GpuModel,
    ) -> anyhow::Result<()> {
        let Some(id) = self
            .sequences
            .get(&node.id())
            .and_then(|s| s.frame_ids.as_ref())
            .and_then(|f| f.get(node.frame as usize))
            .copied()
        else {
            return Ok(());
        };
        let (set, index) = anim::split_frame_id(id as u32);
        self.frame_set(pack, set)?;
        let (base, frame) = self.frames[&set]
            .frame(index)
            .ok_or_else(|| anyhow::anyhow!("missing shadow frame {set}:{index}"))?;
        model.apply_shadow_animation(base, frame);
        Ok(())
    }
    pub fn prepare_filtered(
        &mut self,
        pack: &Pack,
        p: &mut Playback,
        angle: i32,
        filter: Filter<'_>,
    ) -> anyhow::Result<Pose> {
        let Some(seq) = self.sequences.get(&p.node.id()).cloned() else {
            return Ok(Pose {
                base_id: None,
                base_identity: None,
                flags: 0,
                transforms: vec![],
            });
        };
        self.prepare_sequence(pack, p, angle, filter, &seq)
    }
    /// Interface animation nodes enable the second classic frame loader. Each
    /// pose is applied separately, resetting the model's pivot state.
    pub fn interface_poses(
        &mut self,
        pack: &Pack,
        p: &mut Playback,
        angle: i32,
    ) -> anyhow::Result<Vec<Pose>> {
        let primary = self.prepare_filtered(pack, p, angle, Filter::default())?;
        let mut result = vec![primary];
        if let Some(mut seq) = self.sequences.get(&p.node.id()).cloned() {
            if seq.skeletal == -1 && result[0].base_id.is_some() {
                if let Some(ids) = seq.secondary.take() {
                    seq.frame_ids = Some(ids);
                    result.push(self.prepare_sequence(pack, p, angle, Filter::default(), &seq)?);
                }
            }
        }
        Ok(result)
    }
    fn prepare_sequence(
        &mut self,
        pack: &Pack,
        p: &mut Playback,
        angle: i32,
        filter: Filter<'_>,
        seq: &Sequence,
    ) -> anyhow::Result<Pose> {
        let extra = if seq.extra { 0x200 } else { 0 };
        if seq.skeletal != -1 {
            let id = seq.skeletal as u32;
            if let std::collections::btree_map::Entry::Vacant(slot) = self.skeletal.entry(id) {
                slot.insert(SkeletalPose::new(anim::load_keyframeset(pack, id)?)?);
            }
            p.loaded_skeletal = Some(seq.skeletal);
            let pose = &self.skeletal[&id];
            p.node.skeletal_range = Some((pose.set.start as i32, pose.set.end as i32));
            let tick = if filter.wrap_skeletal {
                p.node.time % (pose.set.end as i32 - pose.set.start as i32 + 1)
            } else {
                p.node.time
            };
            let flags = 0x20 | extra | pose.set.render_flags;
            return Ok(Pose {
                base_id: Some(pose.set.base.id),
                base_identity: Some(BaseIdentity::Skeletal { keyframe_set: id }),
                flags,
                transforms: pose.transforms_masked(
                    tick,
                    angle,
                    filter.normals.unwrap_or(seq.extra),
                    filter.blend,
                    filter.part_mask,
                ),
            });
        }
        let Some(ids) = &seq.frame_ids else {
            return Ok(Pose {
                base_id: None,
                base_identity: None,
                flags: 0,
                transforms: vec![],
            });
        };
        let Some(&frame_id) = ids.get(p.node.frame as usize) else {
            return Ok(Pose {
                base_id: None,
                base_identity: None,
                flags: 0,
                transforms: vec![],
            });
        };
        let (set_id, index) = anim::split_frame_id(frame_id as u32);
        self.frame_set(pack, set_id)?;
        let next = if seq.tween {
            ids.get(p.node.next as usize).copied()
        } else {
            None
        };
        if let Some(id) = next {
            self.frame_set(pack, (id as u32) >> 16)?;
        }
        let (base, frame) = self.frames[&set_id]
            .frame(index)
            .ok_or_else(|| anyhow::anyhow!("missing frame {set_id}:{index}"))?;
        let next = next.and_then(|id| {
            self.frames[&((id as u32) >> 16)]
                .frame(id as u32 & 65535)
                .map(|(_, f)| {
                    (
                        BaseIdentity::Classic {
                            frame_set: (id as u32) >> 16,
                            base: f.base_id,
                        },
                        f,
                    )
                })
        });
        let mut flags = 0x20 | extra;
        for f in std::iter::once(frame).chain(next.map(|(_, f)| f)) {
            if f.has_alpha_op {
                flags |= 0x100;
            }
            if f.has_colour_op {
                flags |= 0x80;
            }
            if f.has_billboard_op {
                flags |= 0x400;
            }
        }
        let identity = BaseIdentity::Classic {
            frame_set: set_id,
            base: base.id,
        };
        let next = next
            .filter(|(id, _)| *id == filter.next_base.unwrap_or(identity))
            .map(|(_, f)| f);
        Ok(Pose {
            base_id: Some(base.id),
            base_identity: Some(identity),
            flags,
            transforms: classic_transforms_selected(
                ClassicPose {
                    base,
                    frame,
                    next,
                    tick: p.node.time,
                    duration: seq.frames.as_ref().unwrap()[p.node.frame as usize],
                },
                PoseTarget {
                    angle,
                    normals: filter.normals.unwrap_or(seq.extra),
                    blend: filter.blend,
                    part_mask: filter.part_mask,
                },
            ),
        })
    }
}
impl Playback {
    pub fn start(
        &mut self,
        assets: &AnimationAssets,
        id: i32,
        delay: i32,
        mode: i32,
        random: bool,
        rng: &mut crate::animation_playback::AnimationRandom,
    ) -> anyhow::Result<()> {
        let seq = if id == -1 {
            None
        } else {
            Some(
                assets
                    .sequences
                    .get(&id)
                    .ok_or_else(|| anyhow::anyhow!("sequence {id} missing"))?,
            )
        };
        if self.node.id() != id {
            self.loaded_skeletal = None;
        }
        crate::animation_playback::start(&mut self.node, seq, delay, mode, random, rng)
    }
    pub fn advance(
        &mut self,
        assets: &AnimationAssets,
        ticks: i32,
        rng: &mut crate::animation_playback::AnimationRandom,
    ) -> bool {
        let Some(seq) = assets.sequences.get(&self.node.id()) else {
            return false;
        };
        let range = assets.range(self);
        crate::animation_playback::advance(&mut self.node, seq, ticks, range, rng)
    }
}
