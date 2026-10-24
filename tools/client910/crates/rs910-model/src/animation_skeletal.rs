//! Skeletal poses. Skeletal curves produce the same integer transforms as
//! classic frames; there is no replacement skinning.
use crate::{
    anim::{AnimBase, KeyFrameSet},
    animation_curve::EvaluatedCurve,
    animation_matrix as matrix,
    gpumodel::Transform,
};

#[derive(Clone, Debug)]
pub struct JointPose {
    pub local: [f32; 16],
    pub inverse: [f32; 16],
    pub global: [f32; 16],
}
#[derive(Clone, Debug)]
pub struct SkeletalPose {
    pub set: KeyFrameSet,
    pub joints: Vec<JointPose>,
    curves: Vec<Option<Vec<Option<EvaluatedCurve>>>>,
}
impl SkeletalPose {
    pub fn new(set: KeyFrameSet) -> anyhow::Result<Self> {
        fn global(
            base: &AnimBase,
            id: usize,
            pose: usize,
            visiting: &mut Vec<usize>,
        ) -> anyhow::Result<[f32; 16]> {
            anyhow::ensure!(!visiting.contains(&id), "cyclic animation joint {id}");
            visiting.push(id);
            let j = &base.joints[id];
            let local = j
                .matrices
                .get(pose)
                .ok_or_else(|| anyhow::anyhow!("joint {id}: pose {pose} missing"))?;
            let parent = if let Some(p) = j.parent_link {
                global(base, p, pose, visiting)?
            } else {
                matrix::IDENTITY
            };
            visiting.pop();
            Ok(matrix::multiply(local, &parent))
        }
        let pose = set.loop_point as usize;
        let mut joints = Vec::new();
        for (id, j) in set.base.joints.iter().enumerate() {
            let local = *j
                .matrices
                .get(pose)
                .ok_or_else(|| anyhow::anyhow!("joint {id}: pose {pose} missing"))?;
            joints.push(JointPose {
                local,
                inverse: matrix::inverse(&local),
                global: global(&set.base, id, pose, &mut Vec::new())?,
            });
        }
        let curves = set
            .curves
            .iter()
            .map(|row| {
                row.as_ref().map(|row| {
                    row.iter()
                        .map(|curve| curve.as_ref().map(EvaluatedCurve::new))
                        .collect()
                })
            })
            .collect();
        Ok(Self {
            set,
            joints,
            curves,
        })
    }
    #[cfg(any(test, feature = "test-hooks"))] // test-only introspection
    pub fn transforms(&self, tick: i32, angle: i32, normals: bool) -> Vec<Transform> {
        self.transforms_masked(tick, angle, normals, None, 65535)
    }
    /// The transforms of the pose at `tick`: type-zero joint pivots always
    /// survive blend filtering.
    pub fn transforms_masked(
        &self,
        tick: i32,
        angle: i32,
        normals: bool,
        blend: Option<(&[bool], bool)>,
        part_mask: i32,
    ) -> Vec<Transform> {
        let base = &self.set.base;
        let mut joint = None;
        let mut at = 0;
        let mut out = Vec::new();
        for (skin, &kind) in base.op_types.iter().enumerate() {
            if kind != 0 && blend.is_some_and(|(mask, selected)| mask[skin] != selected) {
                continue;
            }
            if kind == 0 {
                joint = base
                    .skin_order
                    .get(at)
                    .copied()
                    .and_then(|i| usize::try_from(i).ok())
                    .filter(|&i| i < self.joints.len());
                at += 1;
                if let Some(id) = joint {
                    let j = &self.joints[id];
                    out.push(Transform::new(
                        base,
                        skin,
                        [
                            j.global[12] as i32,
                            (-j.global[13]) as i32,
                            (-j.global[14]) as i32,
                        ],
                        angle,
                        normals,
                        true,
                    ));
                }
                continue;
            }
            let Some(row) = &self.curves[skin] else {
                continue;
            };
            let j = joint.map(|i| &self.joints[i]);
            let mut v = match kind {
                1 => j.map_or([0.; 3], |j| [j.local[12], j.local[13], j.local[14]]),
                2 => j.map_or([0.; 3], |j| matrix::euler(&j.inverse)),
                3 => [1.; 3],
                _ => [0.; 3],
            };
            for (component, curve) in row.iter().enumerate() {
                if let Some(curve) = curve {
                    if (kind != 7 || component >= 3) && component < 6 {
                        v[component % 3] = curve.value(tick);
                    }
                }
            }
            if kind == 9 {
                v[0] = v[2];
            }
            let mut value = v.map(|x| x as i32);
            match kind {
                1 => {
                    let Some(j) = j else {
                        continue;
                    };
                    let local = matrix::vector(&j.inverse, v, true);
                    let global = matrix::vector(&j.global, local, false);
                    value = [
                        global[0] as i32,
                        (global[1] as i32).wrapping_neg(),
                        (global[2] as i32).wrapping_neg(),
                    ];
                }
                2 => {
                    let Some(j) = j else {
                        continue;
                    };
                    let mut rotation = matrix::rotation(v[1], v[0], v[2]);
                    if let Some(parent) = base.joints[joint.unwrap()].parent_link {
                        rotation = matrix::multiply(&rotation, &self.joints[parent].global);
                    }
                    rotation = matrix::inverse(&rotation);
                    rotation = matrix::multiply(&rotation, &j.global);
                    let angles = matrix::euler(&rotation);
                    value = [
                        (angles[0] * 2607.5945) as i32 & 16383,
                        (-angles[1] * 2607.5945) as i32 & 16383,
                        (angles[2] * 2607.5945) as i32 & 16383,
                    ];
                }
                3 | 10 => value = v.map(|x| (x * 128.) as i32),
                9 => value[0] = (v[0] * 2607.5945) as i32 & 16383,
                5 => value[0] = (32. - v[0] * 32.) as i32,
                _ => {}
            }
            out.push(Transform::new(base, skin, value, angle, normals, true));
        }
        for op in &mut out {
            op.mask &= part_mask;
        }
        out
    }
}
