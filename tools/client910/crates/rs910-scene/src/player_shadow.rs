//! Character shadows are separate, unlit-texture or concentric-ring models.
use crate::{
    animation_assets::AnimationAssets,
    billboard::BillboardStore,
    cache::Pack,
    entities910::{animation_state::Node, Player},
    gpumodel::GpuModel,
    modelunlit::ModelUnlit,
    particle::EmitterStore,
    player_body::Resources,
    texture::MaterialStore,
};
use anyhow::Result;
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Rings {
        size: i32,
        colours: [i32; 2],
        alpha: [i32; 2],
    },
    Texture {
        material: i16,
        alpha: i8,
    },
}
impl Shape {
    fn key(self) -> i64 {
        match self {
            Self::Rings {
                size,
                colours: [a, b],
                alpha: [c, d],
            } => ((b as i64) << 48)
                .wrapping_add((a as i64) << 32)
                .wrapping_add(
                    d.wrapping_shl(24)
                        .wrapping_add(c.wrapping_shl(16))
                        .wrapping_add(size) as i64,
                ),
            Self::Texture { material, alpha } => {
                (((material as u16 as i32) << 8) | alpha as i32) as i64
            }
        }
    }
}
/// Build the bare unlit model of a shadow shape. These models have allocated
/// zero labels/priorities even when no animation is requested.
fn raw(shape: Shape) -> ModelUnlit {
    let mut m = ModelUnlit {
        version: 12,
        vertex_label: Some(vec![]),
        face_type: Some(vec![]),
        face_priority: Some(vec![]),
        face_trans: Some(vec![]),
        face_mapping: Some(vec![]),
        face_material: Some(vec![]),
        face_label: Some(vec![]),
        ..Default::default()
    };
    fn vertex(m: &mut ModelUnlit, x: i32, z: i32) -> usize {
        if let Some(i) =
            (0..m.vertex_count as usize).find(|&i| m.vertex_x[i] == x && m.vertex_z[i] == z)
        {
            return i;
        }
        let i = m.vertex_count as usize;
        m.vertex_x.push(x);
        m.vertex_y.push(0);
        m.vertex_z.push(z);
        m.vertex_label.as_mut().unwrap().push(0);
        m.vertex_count += 1;
        m.used_vertex_count = m.vertex_count;
        i
    }
    fn face(
        m: &mut ModelUnlit,
        [a, b, c]: [usize; 3],
        colour: i16,
        alpha: i8,
        material: i16,
        mapping: i16,
    ) {
        m.face_vertex1.push(a as i16);
        m.face_vertex2.push(b as i16);
        m.face_vertex3.push(c as i16);
        m.face_type.as_mut().unwrap().push(1);
        m.face_priority.as_mut().unwrap().push(0);
        m.face_trans.as_mut().unwrap().push(alpha);
        m.face_mapping.as_mut().unwrap().push(mapping);
        m.face_material.as_mut().unwrap().push(material);
        m.face_label.as_mut().unwrap().push(0);
        m.face_colour.push(colour);
        m.face_count += 1;
    }
    match shape {
        Shape::Rings {
            size,
            colours: [a, b],
            alpha: [c, d],
        } => {
            let n = (size * 3 + 6) as usize;
            let center = vertex(&mut m, 0, 0);
            let mut rings = vec![vec![0; n]; 3];
            for (ring, radius) in [64, 96, 128].into_iter().enumerate() {
                for (i, slot) in rings[ring].iter_mut().enumerate() {
                    let angle = ((i << 14) / n) as i32;
                    *slot = vertex(
                        &mut m,
                        (crate::trig::sin(angle) * radius) >> 14,
                        (crate::trig::cos(angle) * radius) >> 14,
                    );
                }
            }
            for ring in 0..3 {
                let t = (ring as i32 * 256 + 128) / 3;
                let u = 256 - t;
                let alpha = ((c * u + d * t) >> 8) as i8;
                let colour = (((((a & 127) * u + (b & 127) * t) & 0x7f00)
                    + (((a & 0x380) * u + (b & 0x380) * t) & 0x38000)
                    + (((a & 0xfc00) * u + (b & 0xfc00) * t) & 0xfc0000))
                    >> 8) as i16;
                for i in 0..n {
                    let j = (i + 1) % n;
                    if ring == 0 {
                        face(
                            &mut m,
                            [center, rings[0][j], rings[0][i]],
                            colour,
                            alpha,
                            -1,
                            -1,
                        );
                    } else {
                        face(
                            &mut m,
                            [rings[ring - 1][i], rings[ring - 1][j], rings[ring][j]],
                            colour,
                            alpha,
                            -1,
                            -1,
                        );
                        face(
                            &mut m,
                            [rings[ring - 1][i], rings[ring][j], rings[ring][i]],
                            colour,
                            alpha,
                            -1,
                            -1,
                        );
                    }
                }
            }
        }
        Shape::Texture { material, alpha } => {
            for (x, z) in [(-128, -128), (128, -128), (128, 128), (-128, 128)] {
                vertex(&mut m, x, z);
            }
            face(&mut m, [0, 1, 2], 0, alpha, material, 0);
            face(&mut m, [0, 2, 3], 0, alpha, material, 0);
            m.texture_triangle_count = 2;
            m.texture_triangle_type = Some(vec![0, 0]);
            m.texture_triangle_vertex1 = Some(vec![1, 2]);
            m.texture_triangle_vertex2 = Some(vec![2, 3]);
            m.texture_triangle_vertex3 = Some(vec![0, 0]);
            m.texture_triangle_scale_x = Some(vec![0; 2]);
            m.texture_triangle_scale_y = Some(vec![0; 2]);
            m.texture_triangle_scale_z = Some(vec![0; 2]);
            m.texture_triangle_speed = Some(vec![0; 2]);
            m.texture_triangle_translation_u = Some(vec![0; 2]);
            m.texture_triangle_translation_v = Some(vec![0; 2]);
            m.texture_triangle_rotation = Some(vec![0; 2]);
            m.texture_triangle_direction = Some(vec![0; 2]);
        }
    }
    m
}
/// What a shadow model is built from: the cache and the model stores.
#[derive(Clone, Copy)]
pub struct ShadowSources<'a> {
    pub pack: &'a Pack,
    pub materials: &'a MaterialStore,
    pub billboards: &'a BillboardStore,
    pub emitters: &'a EmitterStore,
}
impl<'a> From<&Resources<'a>> for ShadowSources<'a> {
    fn from(r: &Resources<'a>) -> Self {
        Self {
            pack: r.pack,
            materials: r.materials,
            billboards: r.billboards,
            emitters: r.emitters,
        }
    }
}
pub struct Shadows {
    cache: HashMap<i64, GpuModel>,
    order: VecDeque<i64>,
    detail: i32,
}
impl Default for Shadows {
    fn default() -> Self {
        Self {
            cache: HashMap::new(),
            order: VecDeque::new(),
            detail: crate::gpumodel::MODEL_DETAIL_FLAGS,
        }
    }
}
impl Shadows {
    pub fn set_detail(&mut self, detail: i32) -> bool {
        if self.detail == detail {
            return false;
        }
        self.detail = detail;
        self.cache.clear();
        self.order.clear();
        true
    }
}
impl Shadows {
    pub fn build(
        &mut self,
        r: ShadowSources<'_>,
        assets: &mut AnimationAssets,
        body: &mut GpuModel,
        shape: Shape,
        ground: [i32; 3],
        mut node: Option<&mut Node>,
    ) -> Result<GpuModel> {
        let mut flags = 2055;
        if let Some(n) = node.as_deref_mut() {
            flags = (flags | assets.actor_pose(r.pack, n, Default::default())?.flags) & !512;
        }
        let key = shape.key();
        if self
            .cache
            .get(&key)
            .is_none_or(|m| m.flags & flags != flags)
        {
            if let Some(old) = self.cache.get(&key) {
                flags |= old.flags;
            }
            // The model detail is selected the same way as for the player model.
            self.cache.insert(
                key,
                GpuModel::new(
                    &crate::gpumodel::ModelStores {
                        materials: r.materials,
                        billboards: r.billboards,
                        emitters: r.emitters,
                    },
                    &raw(shape),
                    crate::gpumodel::BuildParams {
                        flags,
                        ambient: 64,
                        contrast: 768,
                        detail: self.detail,
                    },
                )?,
            );
        }
        self.order.retain(|&id| id != key);
        self.order.push_back(key);
        if self.order.len() > 32 {
            self.cache.remove(&self.order.pop_front().unwrap());
        }
        let mut model = self.cache[&key].clone();
        model.flags = flags;
        let (x0, x1, z0, z1) = (body.min_x(), body.max_x(), body.min_z(), body.max_z());
        model.scale(x1.wrapping_sub(x0) >> 1, 128, z1.wrapping_sub(z0) >> 1);
        model.translate(x0.wrapping_add(x1) >> 1, 0, z0.wrapping_add(z1) >> 1);
        if let Some(n) = node {
            assets.shadow(r.pack, n, &mut model)?;
        }
        if ground[0] != 0 {
            model.rotate_x(ground[0]);
        }
        if ground[1] != 0 {
            model.rotate_z(ground[1]);
        }
        if ground[2] != 0 {
            model.translate(0, ground[2], 0);
        }
        Ok(model)
    }
}
/// The player draw deliberately chooses a delayed main node for its shadow;
/// the body uses the opposite delay predicate. Idle-detail does not gate this.
pub fn animation(e: &mut Player) -> Option<&mut Node> {
    let main = e.animation.main.sequence.is_some() && e.animation.main.delay != 0;
    if e.actor.walk.node.sequence.is_some() && (!e.actor.walk.idle || !main) {
        Some(&mut e.actor.walk.node)
    } else if main {
        Some(&mut e.animation.main)
    } else {
        None
    }
}
