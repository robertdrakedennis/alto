//! Modern renderer performance hook (the modern renderer; never compiled into the
//! product). `tools/perf/modern_stage.py` copies this file into a build copy of
//! the tree as `rs910_render_modern::modern_perf_hook` and:
//!
//! - rewrites the wgpu calls of `rs910-render-modern` to the `nx_*` twins below
//!   (counts: passes, draws, indices, instances, pipeline/bind-group/vertex/
//!   index-buffer sets and how many of those repeat the bound state, queue
//!   writes and bytes, objects created and their bytes, the CPU time of
//!   shader-module and pipeline creation);
//! - gives every render and compute pass with a descriptor a begin/end
//!   timestamp (`ts_render`/`ts_compute`: per-pass GPU times on adapters with
//!   `TIMESTAMP_QUERY`);
//! - marks CPU phases in `ModernRenderer::draw`/`encode` (`phase`), opening the
//!   frame with `frame_begin` and resolving the timestamps in `frame_end`, and
//!   times each encode unit on its thread (`unit_timer`).
//!
//! Nothing here changes what is drawn. Records: [`take_records`] (the
//! headless bench) or, with `MODERN_PERF_OUT=<file.tsv>`, one row per frame
//! appended to that file (the client).
#![allow(dead_code, clippy::all, clippy::pedantic, missing_docs)]
use std::cell::Cell;
use std::ops::{Bound, Range, RangeBounds};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

// ------------------------------------------------------------- counters

/// Buffers created per label (`MODERN_PERF_LABELS=1`: printed to stderr every
/// 300 frames and reset; finds what still creates buffers per frame).
static LABELS: Mutex<Vec<(String, u64, u64)>> = Mutex::new(Vec::new());
fn tally(label: Option<&str>, bytes: u64) {
    if std::env::var_os("MODERN_PERF_LABELS").is_none() {
        return;
    }
    let label = label.unwrap_or("?");
    let mut v = LABELS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match v.iter_mut().find(|e| e.0 == label) {
        Some(e) => {
            e.1 += 1;
            e.2 += bytes;
        }
        None => v.push((label.to_string(), 1, bytes)),
    }
}
fn print_labels(frame: u64) {
    if frame % 300 != 0 || std::env::var_os("MODERN_PERF_LABELS").is_none() {
        return;
    }
    let mut v = LABELS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for (label, n, bytes) in v.iter() {
        eprintln!("[modern-perf] frames to {frame}: {n} buffers ({bytes} bytes) \"{label}\"");
    }
    v.clear();
}

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum K {
    Passes,
    ComputePasses,
    Draws,
    DrawCommands,
    IndirectCommands,
    Indices,
    Instances,
    Dispatches,
    SetPipeline,
    SetPipelineSame,
    SetBindGroup,
    SetBindGroupSame,
    SetVertexBuffer,
    SetVertexBufferSame,
    SetIndexBuffer,
    SetIndexBufferSame,
    WriteBuffer,
    WriteBufferBytes,
    WriteTexture,
    WriteTextureBytes,
    CreateBuffer,
    CreateBufferBytes,
    CreateTexture,
    CreateTextureBytes,
    CreateBindGroup,
    CreateSampler,
    CreateShader,
    CreatePipeline,
    ShaderPipelineNs,
    Copies,
    Allocs,
    AllocBytes,
    /// Gauges (counted once per `draw`, so the row's delta is the value):
    /// the loc mesh cache's size, and the light-probe captures started.
    MeshCache,
    ProbeCaptures,
    /// Shader modules and pipelines created off the render thread (lane
    /// P5: the background compiles) and their CPU time; `create_pipeline`
    /// and `shader_pipeline_ns` count every thread.
    BgPipelines,
    BgCompileNs,
    N,
}
pub const NAMES: [&str; K::N as usize] = [
    "passes",
    "compute_passes",
    "draws",
    "draw_commands",
    "indirect_commands",
    "indices",
    "instances",
    "dispatches",
    "set_pipeline",
    "set_pipeline_same",
    "set_bind_group",
    "set_bind_group_same",
    "set_vertex_buffer",
    "set_vertex_buffer_same",
    "set_index_buffer",
    "set_index_buffer_same",
    "write_buffer",
    "write_buffer_bytes",
    "write_texture",
    "write_texture_bytes",
    "create_buffer",
    "create_buffer_bytes",
    "create_texture",
    "create_texture_bytes",
    "create_bind_group",
    "create_sampler",
    "create_shader",
    "create_pipeline",
    "shader_pipeline_ns",
    "copies",
    "allocs",
    "alloc_bytes",
    "mesh_cache",
    "probe_captures",
    "bg_pipelines",
    "bg_compile_ns",
];
/// Per-thread counters (the renderer encodes on several threads,
/// `frame::jobs`: one shared array would bounce its cache lines between
/// them and slow the encode it measures); [`counts`] sums every thread's.
/// Fixed slots, so counting never allocates (the bench's allocator counts
/// through here).
type Counters = [AtomicU64; K::N as usize];
const COUNTER_SLOTS: usize = 64;
static COUNTS: [Counters; COUNTER_SLOTS] = [const { [const { AtomicU64::new(0) }; K::N as usize] }; COUNTER_SLOTS];
static NEXT_SLOT: AtomicU64 = AtomicU64::new(0);
thread_local! {
    static SLOT: Cell<usize> = const { Cell::new(usize::MAX) };
}

#[inline]
pub fn count(k: K, n: u64) {
    let slot = SLOT.try_with(|s| {
        if s.get() == usize::MAX {
            s.set(NEXT_SLOT.fetch_add(1, Relaxed) as usize % COUNTER_SLOTS);
        }
        s.get()
    });
    if let Ok(slot) = slot {
        COUNTS[slot][k as usize].fetch_add(n, Relaxed);
    }
}
pub fn counts() -> Vec<u64> {
    (0..K::N as usize)
        .map(|k| COUNTS.iter().map(|c| c[k].load(Relaxed)).sum())
        .collect()
}

/// Set while `ModernRenderer::draw` runs (the bench's counting allocator
/// counts only those allocations, on the render thread and the renderer's
/// worker threads, `frame::jobs`).
pub static IN_DRAW: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The bound state of the current pass: pipeline, 8 bind groups, 4
    /// vertex buffers (id, offset), the index buffer.
    static BOUND: Cell<[u64; 16]> = const { Cell::new([u64::MAX; 16]) };
}

/// For the bench's `#[global_allocator]`: count an allocation made while
/// the renderer draws.
#[inline]
pub fn note_alloc(bytes: usize) {
    if IN_DRAW.load(Relaxed) {
        count(K::Allocs, 1);
        count(K::AllocBytes, bytes as u64);
    }
}

fn bound_same(slot: usize, id: u64) -> bool {
    BOUND.with(|b| {
        let mut v = b.get();
        let same = v[slot] == id;
        v[slot] = id;
        b.set(v);
        same
    })
}
fn reset_bound() {
    BOUND.with(|b| b.set([u64::MAX; 16]));
}

/// `MODERN_PERF_PASSCOUNTS=1`: per render pass label, the draws, bind group
/// sets and the sets that changed nothing, summed over the run (printed by
/// [`print_pass_counts`]).
static PASS_COUNTS: Mutex<Vec<(String, [u64; 4])>> = Mutex::new(Vec::new());
thread_local! {
    static CUR_PASS: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}
fn pass_counts_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MODERN_PERF_PASSCOUNTS").is_some())
}
fn pass_tally(field: usize) {
    if !pass_counts_on() {
        return;
    }
    let label = CUR_PASS.with(|c| c.borrow().clone());
    let mut v = PASS_COUNTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match v.iter_mut().find(|e| e.0 == label) {
        Some(e) => e.1[field] += 1,
        None => {
            let mut c = [0; 4];
            c[field] = 1;
            v.push((label, c));
        }
    }
}
pub fn reset_pass_counts() {
    PASS_COUNTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
}
pub fn print_pass_counts(frames: u64) {
    if !pass_counts_on() {
        return;
    }
    let mut v = PASS_COUNTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for (label, c) in v.iter() {
        eprintln!(
            "[passcounts] {label}: passes {:.2} draws {:.1} set_bind_group {:.1} (same {:.1}) per frame",
            c[3] as f64 / frames as f64,
            c[0] as f64 / frames as f64,
            c[1] as f64 / frames as f64,
            c[2] as f64 / frames as f64
        );
    }
    v.clear();
}
fn range_key(r: &impl RangeBounds<u64>) -> u64 {
    let s = match r.start_bound() {
        Bound::Included(&v) => v,
        Bound::Excluded(&v) => v + 1,
        Bound::Unbounded => 0,
    };
    let e = match r.end_bound() {
        Bound::Included(&v) => v + 1,
        Bound::Excluded(&v) => v,
        Bound::Unbounded => u64::MAX,
    };
    s.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ e
}

// ------------------------------------------------------------- wgpu twins

fn tex_bytes(desc: &wgpu::TextureDescriptor<'_>) -> u64 {
    let (bw, bh) = desc.format.block_dimensions();
    let block = u64::from(desc.format.block_copy_size(Some(wgpu::TextureAspect::All)).unwrap_or(4));
    let mut total = 0u64;
    for mip in 0..desc.mip_level_count {
        let w = u64::from((desc.size.width >> mip).max(1).div_ceil(bw));
        let h = u64::from((desc.size.height >> mip).max(1).div_ceil(bh));
        let layers = if desc.dimension == wgpu::TextureDimension::D3 {
            u64::from((desc.size.depth_or_array_layers >> mip).max(1))
        } else {
            u64::from(desc.size.depth_or_array_layers)
        };
        total += w * h * layers * block;
    }
    total * u64::from(desc.sample_count)
}

/// The render thread (the first that draws, `frame_begin`, or the bench's
/// `mark_render_thread`); creations on other threads also count as `bg_*`.
static RENDER_THREAD: OnceLock<std::thread::ThreadId> = OnceLock::new();
pub fn mark_render_thread() {
    let _ = RENDER_THREAD.set(std::thread::current().id());
}
fn off_render_thread() -> bool {
    RENDER_THREAD.get().is_some_and(|t| *t != std::thread::current().id())
}

/// `MODERN_PERF_COMPILES=1`: every shader module and pipeline creation to
/// stderr (label, thread, ms): where the compiles happen.
fn timed<R>(label: Option<&str>, f: impl FnOnce() -> R) -> R {
    let t = Instant::now();
    let r = f();
    let ns = t.elapsed().as_nanos() as u64;
    count(K::ShaderPipelineNs, ns);
    let bg = off_render_thread();
    if bg {
        count(K::BgPipelines, 1);
        count(K::BgCompileNs, ns);
    }
    static LOG: OnceLock<bool> = OnceLock::new();
    if *LOG.get_or_init(|| std::env::var_os("MODERN_PERF_COMPILES").is_some()) {
        eprintln!(
            "[modern-perf] compile {:?} {} {:.2} ms",
            label.unwrap_or("?"),
            if bg { "bg" } else { "render" },
            ns as f64 / 1e6
        );
    }
    r
}

/// `MODERN_BENCH_COLD=1`: each module's WGSL gets its structs and helper
/// functions renamed with a suffix unique to this process and module, so the
/// Metal source the module compiles to is new to Metal's shader cache (a cold
/// compile) while the frame stays the same. (An unused function appended to
/// the module is dropped before the Metal source is written, so it does not
/// make the source unique.)
pub fn cold_nonce(source: String) -> String {
    static COLD: OnceLock<bool> = OnceLock::new();
    if !*COLD.get_or_init(|| std::env::var_os("MODERN_BENCH_COLD").is_some()) {
        return source;
    }
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Relaxed);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
        ^ (u64::from(std::process::id()) << 20)
        ^ n;
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    // The names to suffix: every struct, and every function that is not an
    // entry point (an entry point's attributes precede its `fn`).
    let mut names: Vec<String> = Vec::new();
    let words: Vec<(usize, &str)> = {
        let mut out = Vec::new();
        let mut start = None;
        for (i, c) in source.char_indices() {
            match (start, is_word(c)) {
                (None, true) => start = Some(i),
                (Some(s), false) => {
                    out.push((s, &source[s..i]));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            out.push((s, &source[s..]));
        }
        out
    };
    for w in 0..words.len().saturating_sub(1) {
        let (at, word) = words[w];
        let name = words[w + 1].1;
        if word == "struct" {
            names.push(name.to_string());
        } else if word == "fn" {
            let before = &source[..at];
            let line = before.rsplit('\n').next().unwrap_or("");
            let prev_line = before.trim_end_matches(|c: char| c == ' ' || c == '\n' || c == '\r');
            let attrs = format!("{} {}", prev_line.rsplit('\n').next().unwrap_or(""), line);
            let entry = ["@vertex", "@fragment", "@compute"].iter().any(|a| attrs.contains(a))
                || prev_line.ends_with("@vertex")
                || prev_line.ends_with("@fragment")
                || prev_line.ends_with("@compute");
            if !entry {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    names.dedup();
    let mut out = String::with_capacity(source.len() + 64);
    let mut last = 0;
    for &(at, word) in &words {
        if names.binary_search(&word.to_string()).is_ok() && !source[..at].ends_with('@') {
            out.push_str(&source[last..at + word.len()]);
            out.push_str(&format!("_c{nonce:x}"));
            last = at + word.len();
        }
    }
    out.push_str(&source[last..]);
    out
}

pub trait NxDevice {
    fn nx_create_shader_module(&self, d: wgpu::ShaderModuleDescriptor<'_>) -> wgpu::ShaderModule;
    fn nx_create_render_pipeline(&self, d: &wgpu::RenderPipelineDescriptor<'_>) -> wgpu::RenderPipeline;
    fn nx_create_compute_pipeline(&self, d: &wgpu::ComputePipelineDescriptor<'_>) -> wgpu::ComputePipeline;
    fn nx_create_buffer(&self, d: &wgpu::BufferDescriptor<'_>) -> wgpu::Buffer;
    fn nx_create_buffer_init(&self, d: &wgpu::util::BufferInitDescriptor<'_>) -> wgpu::Buffer;
    fn nx_create_texture(&self, d: &wgpu::TextureDescriptor<'_>) -> wgpu::Texture;
    fn nx_create_texture_with_data(
        &self,
        q: &wgpu::Queue,
        d: &wgpu::TextureDescriptor<'_>,
        o: wgpu::util::TextureDataOrder,
        data: &[u8],
    ) -> wgpu::Texture;
    fn nx_create_bind_group(&self, d: &wgpu::BindGroupDescriptor<'_>) -> wgpu::BindGroup;
    fn nx_create_sampler(&self, d: &wgpu::SamplerDescriptor<'_>) -> wgpu::Sampler;
}
impl NxDevice for wgpu::Device {
    fn nx_create_shader_module(&self, d: wgpu::ShaderModuleDescriptor<'_>) -> wgpu::ShaderModule {
        count(K::CreateShader, 1);
        let label = d.label.map(str::to_string);
        timed(label.as_deref(), || self.create_shader_module(d))
    }
    fn nx_create_render_pipeline(&self, d: &wgpu::RenderPipelineDescriptor<'_>) -> wgpu::RenderPipeline {
        count(K::CreatePipeline, 1);
        timed(d.label, || self.create_render_pipeline(d))
    }
    fn nx_create_compute_pipeline(&self, d: &wgpu::ComputePipelineDescriptor<'_>) -> wgpu::ComputePipeline {
        count(K::CreatePipeline, 1);
        timed(d.label, || self.create_compute_pipeline(d))
    }
    fn nx_create_buffer(&self, d: &wgpu::BufferDescriptor<'_>) -> wgpu::Buffer {
        count(K::CreateBuffer, 1);
        count(K::CreateBufferBytes, d.size);
        tally(d.label, d.size);
        self.create_buffer(d)
    }
    fn nx_create_buffer_init(&self, d: &wgpu::util::BufferInitDescriptor<'_>) -> wgpu::Buffer {
        count(K::CreateBuffer, 1);
        count(K::CreateBufferBytes, d.contents.len() as u64);
        tally(d.label, d.contents.len() as u64);
        wgpu::util::DeviceExt::create_buffer_init(self, d)
    }
    fn nx_create_texture(&self, d: &wgpu::TextureDescriptor<'_>) -> wgpu::Texture {
        count(K::CreateTexture, 1);
        count(K::CreateTextureBytes, tex_bytes(d));
        self.create_texture(d)
    }
    fn nx_create_texture_with_data(
        &self,
        q: &wgpu::Queue,
        d: &wgpu::TextureDescriptor<'_>,
        o: wgpu::util::TextureDataOrder,
        data: &[u8],
    ) -> wgpu::Texture {
        count(K::CreateTexture, 1);
        count(K::CreateTextureBytes, tex_bytes(d));
        count(K::WriteTexture, 1);
        count(K::WriteTextureBytes, data.len() as u64);
        wgpu::util::DeviceExt::create_texture_with_data(self, q, d, o, data)
    }
    fn nx_create_bind_group(&self, d: &wgpu::BindGroupDescriptor<'_>) -> wgpu::BindGroup {
        count(K::CreateBindGroup, 1);
        self.create_bind_group(d)
    }
    fn nx_create_sampler(&self, d: &wgpu::SamplerDescriptor<'_>) -> wgpu::Sampler {
        count(K::CreateSampler, 1);
        self.create_sampler(d)
    }
}

pub trait NxQueue {
    fn nx_write_buffer(&self, b: &wgpu::Buffer, offset: u64, data: &[u8]);
    fn nx_write_texture(
        &self,
        t: wgpu::TexelCopyTextureInfo<'_>,
        data: &[u8],
        l: wgpu::TexelCopyBufferLayout,
        s: wgpu::Extent3d,
    );
}
impl<T: rs910_gpu_device::uploads::Uploader + ?Sized> NxQueue for T {
    fn nx_write_buffer(&self, b: &wgpu::Buffer, offset: u64, data: &[u8]) {
        count(K::WriteBuffer, 1);
        count(K::WriteBufferBytes, data.len() as u64);
        remember_indirect(b, offset, data);
        rs910_gpu_device::uploads::Uploader::write_buffer(self, b, offset, data);
    }
    fn nx_write_texture(
        &self,
        t: wgpu::TexelCopyTextureInfo<'_>,
        data: &[u8],
        l: wgpu::TexelCopyBufferLayout,
        s: wgpu::Extent3d,
    ) {
        count(K::WriteTexture, 1);
        count(K::WriteTextureBytes, data.len() as u64);
        rs910_gpu_device::uploads::Uploader::write_texture(self, t, data, l, s);
    }
}

pub trait NxEncoder {
    fn nx_begin_render_pass<'e>(&'e mut self, d: &wgpu::RenderPassDescriptor<'_>) -> wgpu::RenderPass<'e>;
    fn nx_begin_compute_pass<'e>(&'e mut self, d: &wgpu::ComputePassDescriptor<'_>) -> wgpu::ComputePass<'e>;
    fn nx_copy_texture_to_texture(
        &mut self,
        a: wgpu::TexelCopyTextureInfo<'_>,
        b: wgpu::TexelCopyTextureInfo<'_>,
        s: wgpu::Extent3d,
    );
    fn nx_copy_buffer_to_buffer(&mut self, a: &wgpu::Buffer, ao: u64, b: &wgpu::Buffer, bo: u64, n: u64);
}
impl NxEncoder for wgpu::CommandEncoder {
    fn nx_begin_render_pass<'e>(&'e mut self, d: &wgpu::RenderPassDescriptor<'_>) -> wgpu::RenderPass<'e> {
        count(K::Passes, 1);
        reset_bound();
        if pass_counts_on() {
            CUR_PASS.with(|c| *c.borrow_mut() = d.label.unwrap_or("?").to_string());
            pass_tally(3);
        }
        self.begin_render_pass(d)
    }
    fn nx_begin_compute_pass<'e>(&'e mut self, d: &wgpu::ComputePassDescriptor<'_>) -> wgpu::ComputePass<'e> {
        count(K::ComputePasses, 1);
        reset_bound();
        self.begin_compute_pass(d)
    }
    fn nx_copy_texture_to_texture(
        &mut self,
        a: wgpu::TexelCopyTextureInfo<'_>,
        b: wgpu::TexelCopyTextureInfo<'_>,
        s: wgpu::Extent3d,
    ) {
        count(K::Copies, 1);
        self.copy_texture_to_texture(a, b, s);
    }
    fn nx_copy_buffer_to_buffer(&mut self, a: &wgpu::Buffer, ao: u64, b: &wgpu::Buffer, bo: u64, n: u64) {
        count(K::Copies, 1);
        self.copy_buffer_to_buffer(a, ao, b, bo, n);
    }
}

/// CPU mirrors only for indirect buffers, so counters still describe the
/// same logical draws/indices while draw_commands counts front-end calls.
static INDIRECT_BYTES: Mutex<Vec<(u64, Vec<u8>)>> = Mutex::new(Vec::new());
fn remember_indirect(buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
    if !buffer.usage().contains(wgpu::BufferUsages::INDIRECT) { return; }
    let mut buffers = INDIRECT_BYTES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let key = hid(buffer);
    let at = match buffers.iter().position(|b| b.0 == key) {
        Some(at) => at,
        None => { buffers.push((key, Vec::new())); buffers.len() - 1 },
    };
    let bytes = &mut buffers[at].1;
    let end = offset as usize + data.len();
    if bytes.len() < end { bytes.resize(end, 0); }
    bytes[offset as usize..end].copy_from_slice(data);
}
fn indirect_counts(buffer: &wgpu::Buffer, offset: u64, draws: u32) -> (u64, u64) {
    const INDEXED_ARGUMENT_BYTES: usize = std::mem::size_of::<wgpu::util::DrawIndexedIndirectArgs>();
    const ARGUMENT_WORD_BYTES: usize = std::mem::size_of::<u32>();
    const INDEX_COUNT_OFFSET: usize = 0;
    const INSTANCE_COUNT_OFFSET: usize = 4;
    let buffers = INDIRECT_BYTES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let bytes = &buffers.iter().find(|b| b.0 == hid(buffer)).expect("uploaded indirect arguments").1;
    let end = offset as usize + draws as usize * INDEXED_ARGUMENT_BYTES;
    bytes[offset as usize..end].chunks_exact(INDEXED_ARGUMENT_BYTES).fold((0, 0), |(indices, instances), arg| {
        let word = |at| u32::from_le_bytes(arg[at..at + ARGUMENT_WORD_BYTES].try_into().expect("argument word")) as u64;
        let count = word(INSTANCE_COUNT_OFFSET);
        (indices + word(INDEX_COUNT_OFFSET) * count, instances + count)
    })
}

pub trait NxRenderPass<'a> {
    fn nx_set_pipeline(&mut self, p: &'a wgpu::RenderPipeline);
    fn nx_set_bind_group(&mut self, i: u32, g: &'a wgpu::BindGroup, o: &[u32]);
    fn nx_set_vertex_buffer<S: RangeBounds<u64>>(&mut self, slot: u32, b: &'a wgpu::Buffer, r: S);
    fn nx_set_vertex_buffer_slice(&mut self, slot: u32, s: wgpu::BufferSlice<'a>);
    fn nx_set_index_buffer<S: RangeBounds<u64>>(&mut self, b: &'a wgpu::Buffer, r: S, f: wgpu::IndexFormat);
    fn nx_set_index_buffer_slice(&mut self, s: wgpu::BufferSlice<'a>, f: wgpu::IndexFormat);
    fn nx_draw_indexed(&mut self, r: Range<u32>, base: i32, i: Range<u32>);
    fn nx_draw(&mut self, v: Range<u32>, i: Range<u32>);
    fn nx_multi_draw_indexed_indirect(&mut self, b: &wgpu::Buffer, offset: u64, draws: u32);
}
impl<'a> NxRenderPass<'a> for wgpu::RenderPass<'a> {
    fn nx_set_pipeline(&mut self, p: &'a wgpu::RenderPipeline) {
        count(K::SetPipeline, 1);
        if bound_same(0, hid(p)) {
            count(K::SetPipelineSame, 1);
        }
        self.set_pipeline(p);
    }
    fn nx_set_bind_group(&mut self, i: u32, g: &'a wgpu::BindGroup, o: &[u32]) {
        count(K::SetBindGroup, 1);
        let key = hid(g) ^ o.iter().fold(0u64, |h, v| h.wrapping_mul(31).wrapping_add(u64::from(*v) + 1));
        pass_tally(1);
        if bound_same(1 + (i as usize & 7), key) {
            count(K::SetBindGroupSame, 1);
            pass_tally(2);
        }
        self.set_bind_group(i, g, o);
    }
    fn nx_set_vertex_buffer<S: RangeBounds<u64>>(&mut self, slot: u32, b: &'a wgpu::Buffer, r: S) {
        count(K::SetVertexBuffer, 1);
        if bound_same(9 + (slot as usize & 3), hid(b) ^ range_key(&r)) {
            count(K::SetVertexBufferSame, 1);
        }
        self.set_vertex_buffer(slot, b.slice(r));
    }
    fn nx_set_vertex_buffer_slice(&mut self, slot: u32, s: wgpu::BufferSlice<'a>) {
        count(K::SetVertexBuffer, 1);
        bound_same(9 + (slot as usize & 3), u64::MAX - 1);
        self.set_vertex_buffer(slot, s);
    }
    fn nx_set_index_buffer<S: RangeBounds<u64>>(&mut self, b: &'a wgpu::Buffer, r: S, f: wgpu::IndexFormat) {
        count(K::SetIndexBuffer, 1);
        if bound_same(13, hid(b) ^ range_key(&r)) {
            count(K::SetIndexBufferSame, 1);
        }
        self.set_index_buffer(b.slice(r), f);
    }
    fn nx_set_index_buffer_slice(&mut self, s: wgpu::BufferSlice<'a>, f: wgpu::IndexFormat) {
        count(K::SetIndexBuffer, 1);
        bound_same(13, u64::MAX - 1);
        self.set_index_buffer(s, f);
    }
    fn nx_draw_indexed(&mut self, r: Range<u32>, base: i32, i: Range<u32>) {
        count(K::Draws, 1);
        count(K::DrawCommands, 1);
        pass_tally(0);
        count(K::Indices, u64::from(r.end.saturating_sub(r.start)) * u64::from(i.end.saturating_sub(i.start)));
        count(K::Instances, u64::from(i.end.saturating_sub(i.start)));
        self.draw_indexed(r, base, i);
    }
    fn nx_draw(&mut self, v: Range<u32>, i: Range<u32>) {
        count(K::Draws, 1);
        count(K::DrawCommands, 1);
        count(K::Indices, u64::from(v.end.saturating_sub(v.start)) * u64::from(i.end.saturating_sub(i.start)));
        count(K::Instances, u64::from(i.end.saturating_sub(i.start)));
        self.draw(v, i);
    }
    fn nx_multi_draw_indexed_indirect(&mut self, b: &wgpu::Buffer, offset: u64, draws: u32) {
        count(K::Draws, u64::from(draws));
        count(K::DrawCommands, 1);
        count(K::IndirectCommands, 1);
        let (indices, instances) = indirect_counts(b, offset, draws);
        count(K::Indices, indices);
        count(K::Instances, instances);
        for _ in 0..draws { pass_tally(0); }
        self.multi_draw_indexed_indirect(b, offset, draws);
    }
}

pub trait NxComputePass<'a> {
    fn nx_set_pipeline(&mut self, p: &'a wgpu::ComputePipeline);
    fn nx_set_bind_group(&mut self, i: u32, g: &'a wgpu::BindGroup, o: &[u32]);
    fn nx_dispatch_workgroups(&mut self, x: u32, y: u32, z: u32);
}
impl<'a> NxComputePass<'a> for wgpu::ComputePass<'a> {
    fn nx_set_pipeline(&mut self, p: &'a wgpu::ComputePipeline) {
        count(K::SetPipeline, 1);
        self.set_pipeline(p);
    }
    fn nx_set_bind_group(&mut self, i: u32, g: &'a wgpu::BindGroup, o: &[u32]) {
        count(K::SetBindGroup, 1);
        self.set_bind_group(i, g, o);
    }
    fn nx_dispatch_workgroups(&mut self, x: u32, y: u32, z: u32) {
        count(K::Dispatches, 1);
        self.dispatch_workgroups(x, y, z);
    }
}

// ------------------------------------------------------------- GPU timestamps

/// Passes timed per frame (two queries each).
pub const MAX_PASSES: usize = 96;
const SLOTS: usize = 4;

struct Slot {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
}
struct Gpu {
    slots: Vec<Slot>,
    period: f64,
}
static GPU: OnceLock<Option<Gpu>> = OnceLock::new();

#[derive(Default)]
struct SlotState {
    /// 0 idle, 1 resolved (awaiting its submit), 2 mapping, 3 mapped, 4 failed.
    state: u8,
    frame: u64,
    names: Vec<&'static str>,
}
struct State {
    frame: u64,
    active: Option<usize>,
    next: usize,
    slots: Vec<SlotState>,
    map_flags: Vec<std::sync::Arc<AtomicU64>>,
    phase: Option<(&'static str, Instant)>,
    phases: Vec<(&'static str, f64)>,
    frame_start: Option<Instant>,
    counts0: Vec<u64>,
    /// Frames drawn, awaiting their GPU times: (frame, cpu ms, phases, count deltas).
    pending: Vec<(u64, f64, Vec<(&'static str, f64)>, Vec<u64>)>,
    records: Vec<Record>,
}
static STATE: Mutex<Option<State>> = Mutex::new(None);
static ENABLED: AtomicBool = AtomicBool::new(true);

/// One drawn frame.
#[derive(Clone, Debug, Default)]
pub struct Record {
    pub frame: u64,
    /// `ModernRenderer::draw` wall time, ms.
    pub cpu_ms: f64,
    pub phases: Vec<(&'static str, f64)>,
    /// Counter deltas over the draw ([`NAMES`] order).
    pub counts: Vec<u64>,
    /// Per timed pass: (name, ms); empty without timestamps.
    pub gpu: Vec<(&'static str, f64)>,
    /// Per timed pass: (begin, end) ms after the frame's first begin.
    pub gpu_at: Vec<(f64, f64)>,
    /// First pass start to last pass end, ms.
    pub gpu_span_ms: f64,
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let s = g.get_or_insert_with(|| State {
        frame: 0,
        active: None,
        next: 0,
        slots: (0..SLOTS).map(|_| SlotState::default()).collect(),
        map_flags: (0..SLOTS).map(|_| std::sync::Arc::new(AtomicU64::new(0))).collect(),
        phase: None,
        phases: Vec::new(),
        frame_start: None,
        counts0: Vec::new(),
        pending: Vec::new(),
        records: Vec::new(),
    });
    f(s)
}

/// Turn GPU timestamps off (the bench's timing rounds without them).
pub fn set_gpu_timing(on: bool) {
    ENABLED.store(on, Relaxed);
}

fn gpu(device: &wgpu::Device, queue: &dyn rs910_gpu_device::uploads::Uploader) -> Option<&'static Gpu> {
    GPU.get_or_init(|| {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            log::warn!("[modern-perf] no TIMESTAMP_QUERY: no per-pass GPU times");
            return None;
        }
        let n = (MAX_PASSES * 2) as u32;
        let slots = (0..SLOTS)
            .map(|_| Slot {
                queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("modern perf passes"),
                    ty: wgpu::QueryType::Timestamp,
                    count: n,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("modern perf resolve"),
                    size: u64::from(n) * 8,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("modern perf readback"),
                    size: u64::from(n) * 8,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
            })
            .collect();
        Some(Gpu {
            slots,
            period: f64::from(queue.queue().get_timestamp_period()),
        })
    })
    .as_ref()
}

/// Start of `ModernRenderer::draw`: maps and collects earlier frames' GPU
/// times, picks this frame's timestamp slot and opens the first phase.
pub fn frame_begin(device: &wgpu::Device, queue: &dyn rs910_gpu_device::uploads::Uploader) {
    mark_render_thread();
    let g = if ENABLED.load(Relaxed) { gpu(device, queue) } else { None };
    collect(device, g, false);
    with(|s| {
        s.frame += 1;
        s.active = None;
        if let Some(_) = g {
            let slot = s.next;
            if s.slots[slot].state == 0 {
                s.slots[slot].frame = s.frame;
                s.slots[slot].names.clear();
                s.active = Some(slot);
            }
        }
        s.phases.clear();
        s.counts0 = counts();
        s.frame_start = Some(Instant::now());
        s.phase = Some(("setup", Instant::now()));
    });
    IN_DRAW.store(true, Relaxed);
}

/// Close the open CPU phase and open `name`.
pub fn phase(name: &'static str) {
    with(|s| {
        let now = Instant::now();
        if let Some((prev, t)) = s.phase.take() {
            s.phases.push((prev, (now - t).as_secs_f64() * 1000.0));
        }
        s.phase = Some((name, now));
    });
}

/// One encode unit's CPU time (`frame::units`; the units record on several
/// threads, so their times overlap the `encode: units` phase and each
/// other): recorded as the phase `unit: NAME` when the timer drops.
pub struct UnitTimer(&'static str, Instant);

pub fn unit_timer(name: &str) -> UnitTimer {
    UnitTimer(intern(&format!("unit: {}", name.trim_start_matches("modern "))), Instant::now())
}

impl Drop for UnitTimer {
    fn drop(&mut self) {
        let ms = self.1.elapsed().as_secs_f64() * 1000.0;
        let name = self.0;
        with(|s| s.phases.push((name, ms)));
    }
}

/// End of `ModernRenderer::draw`: close the phases, resolve this frame's
/// timestamps into its slot's readback buffer.
pub fn frame_end(encoder: &mut wgpu::CommandEncoder) {
    IN_DRAW.store(false, Relaxed);
    print_labels(with(|s| s.frame));
    let g = GPU.get().and_then(Option::as_ref);
    with(|s| {
        let now = Instant::now();
        if let Some((prev, t)) = s.phase.take() {
            s.phases.push((prev, (now - t).as_secs_f64() * 1000.0));
        }
        let cpu = s.frame_start.take().map_or(0.0, |t| (now - t).as_secs_f64() * 1000.0);
        let c = counts();
        let delta: Vec<u64> = c.iter().zip(&s.counts0).map(|(a, b)| a - b).collect();
        let phases = std::mem::take(&mut s.phases);
        s.pending.push((s.frame, cpu, phases, delta));
        if let (Some(g), Some(slot)) = (g, s.active.take()) {
            let n = s.slots[slot].names.len() as u32 * 2;
            if n > 0 {
                let sl = &g.slots[slot];
                encoder.resolve_query_set(&sl.queries, 0..n, &sl.resolve, 0);
                encoder.copy_buffer_to_buffer(&sl.resolve, 0, &sl.readback, 0, u64::from(n) * 8);
                s.slots[slot].state = 1;
                s.next = (slot + 1) % SLOTS;
            }
        }
    });
}

/// After the frame's submit: map resolved slots and collect the mapped ones
/// (`wait`: block until this frame's times are in; the bench).
pub fn after_submit(device: &wgpu::Device, wait: bool) {
    collect(device, GPU.get().and_then(Option::as_ref), wait);
}

fn collect(device: &wgpu::Device, g: Option<&'static Gpu>, wait: bool) {
    let Some(g) = g else {
        // A headless caller requesting completion still waits when timestamps
        // are disabled; capture readiness must not depend on the timing option.
        if wait {
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("required headless frame GPU completion");
        }
        // CPU records need no timestamps. A requested GPU wait succeeded above;
        // wait=false records no evidence of GPU completion.
        let done = with(|s| {
            let pending = std::mem::take(&mut s.pending);
            for (frame, cpu, phases, counts) in pending {
                s.records.push(Record { frame, cpu_ms: cpu, phases, counts, ..Record::default() });
            }
            s.records.len()
        });
        let _ = done;
        flush_file();
        return;
    };
    // Map every resolved slot (its submit has happened by now).
    with(|s| {
        for (i, st) in s.slots.iter_mut().enumerate() {
            if st.state == 1 {
                st.state = 2;
                let flag = s.map_flags[i].clone();
                flag.store(0, Relaxed);
                g.slots[i].readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                    flag.store(if r.is_ok() { 1 } else { 2 }, Relaxed);
                });
            }
        }
    });
    let _ = device.poll(if wait { wgpu::PollType::wait_indefinitely() } else { wgpu::PollType::Poll });
    with(|s| {
        let mut gpu_by_frame = Vec::new();
        for i in 0..SLOTS {
            if s.slots[i].state != 2 {
                continue;
            }
            match s.map_flags[i].load(Relaxed) {
                1 => {
                    let names = std::mem::take(&mut s.slots[i].names);
                    let n = names.len() * 2;
                    let data = g.slots[i].readback.slice(..).get_mapped_range().expect("mapped range");
                    let ticks: Vec<u64> = data
                        .chunks_exact(8)
                        .take(n)
                        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                        .collect();
                    drop(data);
                    g.slots[i].readback.unmap();
                    let ms = |a: u64, b: u64| b.saturating_sub(a) as f64 * g.period / 1e6;
                    let passes: Vec<(&'static str, f64)> = names
                        .iter()
                        .enumerate()
                        .map(|(k, name)| (*name, ms(ticks[2 * k], ticks[2 * k + 1])))
                        .collect();
                    let first = ticks.iter().step_by(2).copied().filter(|t| *t != 0).min().unwrap_or(0);
                    let last = ticks.iter().skip(1).step_by(2).copied().max().unwrap_or(0);
                    let at: Vec<(f64, f64)> = (0..names.len())
                        .map(|k| (ms(first, ticks[2 * k]), ms(first, ticks[2 * k + 1])))
                        .collect();
                    gpu_by_frame.push((s.slots[i].frame, passes, ms(first, last), at));
                    s.slots[i].state = 0;
                }
                2 => s.slots[i].state = 0,
                _ => {}
            }
        }
        // Complete the pending frames: those with GPU times, and those older
        // than every slot still in flight (they were not timed).
        let oldest_in_flight = s
            .slots
            .iter()
            .filter(|st| st.state == 1 || st.state == 2)
            .map(|st| st.frame)
            .min()
            .unwrap_or(u64::MAX);
        let pending = std::mem::take(&mut s.pending);
        for (frame, cpu, phases, counts) in pending {
            if let Some((_, passes, span, at)) = gpu_by_frame.iter().find(|(f, _, _, _)| *f == frame) {
                s.records.push(Record {
                    frame,
                    cpu_ms: cpu,
                    phases,
                    counts,
                    gpu: passes.clone(),
                    gpu_span_ms: *span,
                    gpu_at: at.clone(),
                });
            } else if frame < oldest_in_flight && !s.slots.iter().any(|st| st.frame == frame && st.state != 0) {
                s.records.push(Record { frame, cpu_ms: cpu, phases, counts, ..Record::default() });
            } else {
                s.pending.push((frame, cpu, phases, counts));
            }
        }
    });
    flush_file();
}

fn intern(name: &str) -> &'static str {
    static NAMES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut v = NAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(n) = v.iter().find(|n| **n == name) {
        return n;
    }
    let n: &'static str = Box::leak(name.to_string().into_boxed_str());
    v.push(n);
    n
}

/// The begin/end timestamp writes of a render pass named `name`.
pub fn ts_render(name: &str) -> Option<wgpu::RenderPassTimestampWrites<'static>> {
    let (slot, k) = claim(name)?;
    let g = GPU.get()?.as_ref()?;
    Some(wgpu::RenderPassTimestampWrites {
        query_set: &g.slots[slot].queries,
        beginning_of_pass_write_index: Some(2 * k),
        end_of_pass_write_index: Some(2 * k + 1),
    })
}

/// The begin/end timestamp writes of a compute pass named `name`.
pub fn ts_compute(name: &str) -> Option<wgpu::ComputePassTimestampWrites<'static>> {
    let (slot, k) = claim(name)?;
    let g = GPU.get()?.as_ref()?;
    Some(wgpu::ComputePassTimestampWrites {
        query_set: &g.slots[slot].queries,
        beginning_of_pass_write_index: Some(2 * k),
        end_of_pass_write_index: Some(2 * k + 1),
    })
}

fn claim(name: &str) -> Option<(usize, u32)> {
    let name = intern(name);
    with(|s| {
        let slot = s.active?;
        let names = &mut s.slots[slot].names;
        if names.len() >= MAX_PASSES {
            return None;
        }
        names.push(name);
        Some((slot, names.len() as u32 - 1))
    })
}

/// The completed records so far (the bench).
pub fn take_records() -> Vec<Record> {
    with(|s| std::mem::take(&mut s.records))
}

/// Drop every pending state (the bench, between configurations).
pub fn reset(device: &wgpu::Device) {
    after_submit(device, true);
    let _ = take_records();
}

// ------------------------------------------------------------- client file

fn flush_file() {
    static PATH: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    let Some(path) = PATH.get_or_init(|| std::env::var_os("MODERN_PERF_OUT").map(Into::into)).as_ref() else {
        return;
    };
    let records = take_records();
    if records.is_empty() {
        return;
    }
    use std::io::Write as _;
    let new = !path.exists();
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    if new {
        let _ = writeln!(f, "frame\tcpu_ms\tgpu_span_ms\t{}\tphases\tgpu", NAMES.join("\t"));
    }
    for r in records {
        let phases: Vec<String> = r.phases.iter().map(|(n, v)| format!("{n}={v:.4}")).collect();
        let gpu: Vec<String> = r.gpu.iter().map(|(n, v)| format!("{n}={v:.4}")).collect();
        let counts: Vec<String> = r.counts.iter().map(u64::to_string).collect();
        let _ = writeln!(
            f,
            "{}\t{:.4}\t{:.4}\t{}\t{}\t{}",
            r.frame,
            r.cpu_ms,
            r.gpu_span_ms,
            counts.join("\t"),
            phases.join(";"),
            gpu.join(";")
        );
    }
}

/// A stable identity for a wgpu resource (its `Hash`, which is its address:
/// one `write` of a word, so an identity hasher costs nothing).
fn hid<T: std::hash::Hash>(resource: &T) -> u64 {
    #[derive(Default)]
    struct Identity(u64);
    impl std::hash::Hasher for Identity {
        fn finish(&self) -> u64 {
            self.0
        }
        fn write(&mut self, bytes: &[u8]) {
            for &b in bytes {
                self.0 = self.0.wrapping_mul(0x100_0000_01b3).wrapping_add(u64::from(b));
            }
        }
        fn write_usize(&mut self, n: usize) {
            self.0 = n as u64;
        }
        fn write_u64(&mut self, n: u64) {
            self.0 = n;
        }
    }
    use std::hash::Hasher as _;
    let mut hasher = Identity::default();
    resource.hash(&mut hasher);
    hasher.finish()
}
