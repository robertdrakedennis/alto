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
//! the frame's draw list in plan order. The depth-only passes do
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

use crate::frame::encoding::EncodeInputs;
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
            Geometry::Loc { page, .. } | Geometry::Dynamic { page, .. } => Buffers::Loc(page),
            Geometry::Arena { .. } => Buffers::Arena,
            Geometry::Floor { level, batch } => Buffers::Floor(level, batch),
            Geometry::Far { page, .. } => Buffers::Far(page),
        }
    }

    pub(crate) fn base_vertex(self) -> i32 {
        match self {
            Geometry::Loc { base_vertex, .. }
            | Geometry::Dynamic { base_vertex, .. }
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
const FIRST_DRAW: usize = 0;
const SINGLE_DRAW: usize = 1;

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

/// Sort `list` by [`sort_key`], equal keys in list order (a stable sort that
/// computes each key once; `order` and `scratch` are kept for their capacity).
pub(crate) fn sort_depth_only(
    list: &mut Vec<Draw>,
    order: &mut Vec<(u128, u32)>,
    scratch: &mut Vec<Draw>,
) {
    order.clear();
    order.extend(
        list.iter()
            .enumerate()
            .map(|(i, d)| (sort_key(d), i as u32)),
    );
    order.sort_unstable();
    scratch.clear();
    scratch.extend(order.iter().map(|&(_, i)| list[i as usize]));
    std::mem::swap(list, scratch);
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
                .filter(|(_, d)| d.pass == Pass::Opaque && !d.batched)
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
    /// Draw `list` in order into a pass whose pipeline and groups 0, 2 and
    /// 3 are set.
    pub(crate) fn submit_all<'p>(&'p self, pass: &mut wgpu::RenderPass<'p>, list: &[Draw]) {
        self.encoding_inputs().submit_all(pass, list)
    }

    /// Pack each pass's final packet order into its own contiguous argument
    /// range after sorting and deferred caster preparation. Copies of scene
    /// packets use their pass's range, so sorting never fragments indirect runs.
    pub(crate) fn prepare_draw_commands(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
    ) {
        let frame = &mut self.frame_resources;
        let sun = &mut self.history.shadow.sun;
        frame.draw_commands.prepare(
            device,
            queue,
            frame
                .draws
                .iter_mut()
                .chain(frame.shadow_only.iter_mut().map(|(draw, _)| draw))
                .chain(frame.packets.prepass.iter_mut())
                .chain(sun.static_lists.iter_mut().flatten())
                .chain(sun.dynamic_lists.iter_mut().flatten()),
        );
        let far = &mut self.scene_resources.far;
        let mut first = 0;
        while first < self.frame_resources.draws.len() {
            let count = far_run_len(&self.frame_resources.draws[first..]);
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

impl<'a> EncodeInputs<'a> {
    /// Compatible array materials differ only in their per-instance records.
    /// Run order and every argument offset remain the original packet order.
    fn indirect_run_len(&self, list: &[Draw]) -> usize {
        let first = &list[FIRST_DRAW];
        let Some(index) = first.indirect else {
            return SINGLE_DRAW;
        };
        list.iter()
            .enumerate()
            .take_while(|(offset, draw)| {
                draw.indirect == Some(index + *offset as u32)
                    && draw.geometry.buffers() == first.geometry.buffers()
                    && draw.pass == first.pass
                    && (draw.material == first.material
                        || (self.textures.is_arrayed(first.material)
                            && self.textures.is_arrayed(draw.material)))
            })
            .count()
    }

    /// Draw `d`, setting only the state `bound` does not already hold (the
    /// pipeline and groups 0, 2 and 3 are the pass's).
    pub(crate) fn submit<'p>(&self, pass: &mut wgpu::RenderPass<'p>, d: &Draw, bound: &mut Bound) {
        if !self.bind_draw(pass, d, bound) {
            return;
        }
        pass.draw_indexed(
            d.first_index..d.first_index + d.count,
            d.geometry.base_vertex(),
            d.instance..d.instance + 1,
        );
    }

    fn bind_draw<'p>(&self, pass: &mut wgpu::RenderPass<'p>, d: &Draw, bound: &mut Bound) -> bool {
        // Ready geometry draws with the neutral white material while its maps
        // decode. A material with a layer of the arrays
        // (`models::material_arrays`) binds nothing per draw: the pass holds
        // the arrays, and the draw's instance record names the layer.
        let arrayed = self.textures.is_arrayed(d.material);
        let needs_material = !arrayed && bound.material != Some(d.material);
        let material = needs_material
            .then(|| self.textures.get(d.material))
            .flatten();
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
        } else if needs_material {
            pass.set_bind_group(1, self.textures.neutral(), &[]);
            bound.material = Some(d.material);
            bound.plain = true;
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

    /// Submit one ordinary packet or an ordered indirect run. Array materials
    /// share bindings; buffer page, pipeline and argument order stay exact.
    /// Sprites, water and other pipeline boundaries bound the caller's slice.
    pub(crate) fn submit_run<'p>(
        &self,
        pass: &mut wgpu::RenderPass<'p>,
        list: &[Draw],
        bound: &mut Bound,
    ) -> usize {
        let d = &list[0];
        let count = self.indirect_run_len(list);
        if count > SINGLE_DRAW {
            if let (Some(index), Some(buffer)) = (d.indirect, self.draw_commands.buffer()) {
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

    /// Draw `list` in order into a pass whose pipeline and groups 0, 2 and
    /// 3 are set.
    pub(crate) fn submit_all<'p>(&self, pass: &mut wgpu::RenderPass<'p>, list: &[Draw]) {
        let mut bound = Bound::default();
        let mut first = 0;
        while first < list.len() {
            first += self.submit_run(pass, &list[first..], &mut bound);
        }
    }
}
