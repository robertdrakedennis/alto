//! The submission core: how the frame's model draws reach a render pass.
//!
//! A [`Draw`] is a self-contained draw packet: its geometry is an offset in
//! a shared buffer set (a loc page of `frame::arenas`, the per-frame arena,
//! a floor batch), its per-draw data an instance record the instance stream
//! (vertex slot 2) addresses by the draw's first instance, its material the
//! bind group of group 1. [`Bound`] tracks what a pass has bound, so a draw
//! sets only what differs from the draw before it: the instance stream once
//! per pass, a loc page once per run of draws in it, the material once per
//! run of draws with it.
//!
//! The forward, geometry, reflection, point-shadow and capture passes draw
//! the frame's draw list in its (faithful) order. The depth-only passes do
//! not depend on draw order (their depth test keeps the nearest depth
//! whatever order the draws come in, and they write no colour), so their
//! lists, [`FramePackets`], are sorted by material, then buffer set, then
//! first index: each material's bind group and each page is then set once
//! per pass (per cascade). The geometry pass writes colour (normal,
//! position), where equal depths keep the draw that came last, so it keeps
//! the draw order.
//!
//! [`FramePackets`] are built once per frame at the end of the prepare and
//! only read by the encode: the depth pre-pass's list. The sun shadow
//! cascades' caster packets, sorted the same way, are the shadow cache's
//! (`frame::gpu::shadow_cache`: only what a cascade redraws this frame).

use crate::frame::*;

/// The buffer set a draw's geometry lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Buffers {
    /// A loc page (`frame::arenas`).
    Loc(u16),
    /// The per-frame arena.
    Arena,
    /// A floor level's batch.
    Floor(usize, usize),
    /// A page of the far scene's arena (`frame::gpu::far`).
    Far(u16),
}

impl Geometry {
    pub(crate) fn buffers(self) -> Buffers {
        match self {
            Geometry::Loc { page, .. } => Buffers::Loc(page),
            Geometry::Arena { .. } => Buffers::Arena,
            Geometry::Floor { level, batch } => Buffers::Floor(level, batch),
            Geometry::Far { page, .. } => Buffers::Far(page),
        }
    }

    fn base_vertex(self) -> i32 {
        match self {
            Geometry::Loc { base_vertex, .. }
            | Geometry::Arena { base_vertex }
            | Geometry::Far { base_vertex, .. } => base_vertex,
            Geometry::Floor { .. } => 0,
        }
    }
}

/// What a pass has bound for the model draws (see the module docs). Reset
/// it whenever the pass draws something else in between (terrain, sprites,
/// water, capture floors), which binds its own buffers and group 1.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Bound {
    instances: bool,
    buffers: Option<Buffers>,
    /// The material whose own textures group 1 holds ([`NEUTRAL`]: none is
    /// read).
    material: Option<i32>,
    /// The material arrays (group 4) are set.
    arrays: bool,
    /// Group 1's meta block is the zero block (an array material's draw
    /// reads the block: a material with an atlas must not stay bound).
    plain: bool,
}

/// `Bound::material` when group 1 holds the neutral material: what a pass of
/// array materials binds because the pipeline layout has the group.
const NEUTRAL: i32 = i32::MIN;

impl Bound {
    /// Forget everything bound (another kind of draw came in between).
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The frame's sorted draw list for the depth pre-pass (see the module
/// docs), built once in `draw` and read by `encode`.
#[derive(Default)]
pub(crate) struct FramePackets {
    /// The depth pre-pass: the opaque entities that write depth, sorted.
    pub(crate) prepass: Vec<Draw>,
    /// Sort scratch: `(key, index)`.
    order: Vec<(u128, u32)>,
}

/// The depth-only passes' order: material, buffer set, first index (ties
/// broken by the draw's index, so the order is a function of the draw list
/// alone).
pub(crate) fn sort_key(d: &Draw) -> u128 {
    let buffers: u64 = match d.geometry.buffers() {
        Buffers::Loc(page) => u64::from(page),
        Buffers::Arena => 1 << 16,
        Buffers::Floor(level, batch) => (2 << 16) | ((level as u64) << 32) | batch as u64,
        Buffers::Far(page) => (3 << 16) | u64::from(page),
    };
    // Materials as unsigned: `-1` (untextured) last.
    (u128::from(d.material as u32) << 80)
        | (u128::from(buffers & 0xffff_ffff_ffff) << 32)
        | u128::from(d.first_index)
}

impl FramePackets {
    /// The sorted pre-pass list of this frame's `draws`: `opaque` the entity
    /// draws before the floors; `sorted` false keeps the draw order (the
    /// tests' reference).
    pub(crate) fn build(&mut self, draws: &[Draw], opaque: usize, sorted: bool) {
        let order = &mut self.order;
        order.clear();
        order.extend(
            draws[..opaque.min(draws.len())]
                .iter()
                .enumerate()
                .filter(|(_, d)| d.pass == Pass::Opaque)
                .map(|(i, d)| (sort_key(d), i as u32)),
        );
        if sorted {
            order.sort_unstable();
        }
        self.prepass.clear();
        self.prepass
            .extend(order.iter().map(|&(_, i)| draws[i as usize]));
    }
}

impl ModernRenderer {
    /// Draw `d`, setting only the state `bound` does not already hold (the
    /// pipeline and groups 0, 2 and 3 are the pass's).
    pub(crate) fn submit<'p>(
        &'p self,
        pass: &mut wgpu::RenderPass<'p>,
        d: &Draw,
        bound: &mut Bound,
    ) {
        if !self.bind_draw(pass, d, bound) {
            return;
        }
        pass.draw_indexed(
            d.first_index..d.first_index + d.count,
            d.geometry.base_vertex(),
            d.instance..d.instance + 1,
        );
    }

    fn bind_draw<'p>(
        &'p self,
        pass: &mut wgpu::RenderPass<'p>,
        d: &Draw,
        bound: &mut Bound,
    ) -> bool {
        // The material first: a draw whose material is not loaded draws
        // nothing (and binds nothing). A material with a layer of the arrays
        // (`models::material_arrays`) binds nothing per draw: the pass holds
        // the arrays, and the draw's instance record names the layer.
        let arrayed = self.textures.is_arrayed(d.material);
        let material = if arrayed || bound.material == Some(d.material) {
            None
        } else {
            match self.textures.get(d.material) {
                Some(m) => Some(m),
                None => return false,
            }
        };
        let buffers = d.geometry.buffers();
        if bound.buffers != Some(buffers) {
            let (vertices, colours, indices) = match buffers {
                Buffers::Loc(page) => {
                    let Some(p) = self.loc_arena.pages.get(usize::from(page)) else {
                        return false;
                    };
                    (&p.vertices, &p.colours, &p.indices)
                }
                Buffers::Arena => {
                    let Some((v, c, i, _)) = self.arena.gpu.as_ref() else {
                        return false;
                    };
                    (v, c, i)
                }
                Buffers::Floor(level, batch) => {
                    let Some(floor) = self.floors.get(level).and_then(Option::as_ref) else {
                        return false;
                    };
                    let b = &floor.batches[batch];
                    (&floor.vertices, &b.colours, &b.indices)
                }
                Buffers::Far(page) => {
                    let Some(p) = self.far.arena.pages.get(usize::from(page)) else {
                        return false;
                    };
                    (&p.vertices, &p.colours, &p.indices)
                }
            };
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_vertex_buffer(1, colours.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint16);
            bound.buffers = Some(buffers);
        }
        if !bound.instances {
            let Some((instances, _)) = self.instance_buffer.as_ref() else {
                return false;
            };
            pass.set_vertex_buffer(2, instances.slice(..));
            bound.instances = true;
        }
        if let Some(material) = material {
            pass.set_bind_group(1, &material.bind_group, &[]);
            bound.material = Some(d.material);
            bound.plain = !material.atlas;
        } else if arrayed && (bound.material.is_none() || !bound.plain) {
            pass.set_bind_group(1, self.textures.neutral(), &[]);
            bound.material = Some(NEUTRAL);
            bound.plain = true;
        }
        if !bound.arrays {
            pass.set_bind_group(4, &self.textures.arrays.bind, &[]);
            bound.arrays = true;
        }
        true
    }

    /// Draw `list` in order into a pass whose pipeline and groups 0, 2 and
    /// 3 are set.
    pub(crate) fn submit_all<'p>(&'p self, pass: &mut wgpu::RenderPass<'p>, list: &[Draw]) {
        let mut bound = Bound::default();
        let mut first = 0;
        while first < list.len() {
            first += self.submit_run(pass, &list[first..], &mut bound);
        }
    }
    /// Submit one ordinary packet or a consecutive far run. Pipeline, material,
    /// buffer page and argument order must all match; sprites bound the slice.
    pub(crate) fn submit_run<'p>(
        &'p self,
        pass: &mut wgpu::RenderPass<'p>,
        list: &[Draw],
        bound: &mut Bound,
    ) -> usize {
        let d = &list[0];
        let count = far_run_len(list);
        if count > 1 {
            if let (Some(index), Some((buffer, _))) = (d.indirect, self.far.indirect.as_ref()) {
                if self.bind_draw(pass, d, bound) {
                    pass.multi_draw_indexed_indirect(
                        buffer,
                        u64::from(index) * INDIRECT_STRIDE,
                        count as u32,
                    );
                }
                return count;
            }
        }
        self.submit(pass, d, bound);
        1
    }

    /// Far indirect arguments are frame resources, uploaded before recording.
    /// A device without nonzero indirect instances retains ordinary draws.
    pub(crate) fn prepare_far_indirect(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
    ) {
        let far = &mut self.far;
        far.indirect_args.clear();
        if !device
            .features()
            .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE)
        {
            return;
        }
        for d in &mut self.draws {
            if matches!(d.geometry, Geometry::Far { .. }) {
                d.indirect = Some(far.indirect_args.len() as u32);
                far.indirect_args.push(wgpu::util::DrawIndexedIndirectArgs {
                    index_count: d.count,
                    instance_count: 1,
                    first_index: d.first_index,
                    base_vertex: d.geometry.base_vertex(),
                    first_instance: d.instance,
                });
            }
        }
        if far.indirect_args.is_empty() {
            return;
        }
        let need = far.indirect_args.len() as u64 * INDIRECT_STRIDE;
        if far.indirect.as_ref().is_none_or(|(_, cap)| *cap < need) {
            let cap = need.next_power_of_two();
            far.indirect = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("modern far indirect"),
                    size: cap,
                    usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                cap,
            ));
        }
        queue.write_buffer(
            &far.indirect.as_ref().expect("far indirect").0,
            0,
            bytemuck::cast_slice(&far.indirect_args),
        );
        let mut first = 0;
        while first < self.draws.len() {
            let count = far_run_len(&self.draws[first..]);
            if count > 1 {
                far.stats.indirect_runs += 1;
                far.stats.indirect_packets += count;
            }
            first += count;
        }
    }
}

/// Packed indexed argument size required by wgpu's indirect ABI.
const INDIRECT_STRIDE: u64 = std::mem::size_of::<wgpu::util::DrawIndexedIndirectArgs>() as u64;

pub(crate) fn far_run_len(list: &[Draw]) -> usize {
    let first = &list[0];
    let Some(index) = first.indirect else {
        return 1;
    };
    if !matches!(first.geometry, Geometry::Far { .. }) {
        return 1;
    }
    list.iter()
        .enumerate()
        .take_while(|(offset, d)| {
            d.indirect == Some(index + *offset as u32)
                && d.geometry.buffers() == first.geometry.buffers()
                && d.material == first.material
                && d.pass == first.pass
        })
        .count()
}

#[cfg(test)]
mod run_tests {
    use super::*;

    #[test]
    fn far_runs_stop_at_page_material_pass_and_argument_order_boundaries() {
        const FIRST_INSTANCE: u32 = 4;
        const FIRST_ARGUMENT: u32 = 2;
        const PAGE: u16 = 1;
        const INDEX_COUNT: u32 = 6;
        let draw = Draw {
            geometry: Geometry::Far {
                page: PAGE,
                base_vertex: 0,
            },
            material: -1,
            first_index: 0,
            count: INDEX_COUNT,
            instance: FIRST_INSTANCE,
            pass: Pass::Opaque,
            casts: false,
            indirect: Some(FIRST_ARGUMENT),
        };
        let mut second = draw;
        second.indirect = Some(FIRST_ARGUMENT + 1);
        second.instance += 1;
        assert_eq!(far_run_len(&[draw, second]), 2);
        second.geometry = Geometry::Far {
            page: PAGE + 1,
            base_vertex: 0,
        };
        assert_eq!(far_run_len(&[draw, second]), 1);
        second.geometry = draw.geometry;
        second.material = i32::MIN;
        assert_eq!(far_run_len(&[draw, second]), 1);
        second.material = draw.material;
        second.pass = Pass::NoDepthWrite;
        assert_eq!(far_run_len(&[draw, second]), 1);
        second.pass = draw.pass;
        second.indirect = Some(FIRST_ARGUMENT + 2);
        assert_eq!(far_run_len(&[draw, second]), 1);
    }
}
