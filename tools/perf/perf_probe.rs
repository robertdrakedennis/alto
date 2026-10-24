//! Performance probe for the Phase 6 harness (tools/perf; never compiled
//! into the product: `tools/perf/instrument.py` copies this file into a build
//! copy of the tree as `rs910_gpu_device::perf_probe` and rewrites every wgpu
//! call of the faithful renderer to the `tr_*` twins below).
//!
//! Two modes, both off unless their variable is set:
//!
//! - `CLIENT910_PERF_STATS=<file.tsv>`: one row per redraw with the main
//!   thread's CPU time of the logic (`about_to_wait`) and the redraw, the
//!   GPU time of the frame's submits (each submit is waited for, so the GPU
//!   is idle before the next one: the sum is the frame's GPU work), the
//!   wgpu objects created, the queue writes, the submits, passes and draws,
//!   and the heap allocations of the main thread and of all threads.
//! - `CLIENT910_PERF_TRACE=<file>`: a content trace for equivalence checks.
//!   Resource creations are listed (lines not starting with `S `) but the
//!   compared stream (`S ` lines) names no object identity: every render
//!   pass by its attachments, every draw by its pipeline's canonical
//!   descriptor, viewport/scissor, the contents of each bound bind group
//!   (buffer bytes of the bound ranges as the GPU sees them at execution,
//!   texture views by descriptor and written content, samplers by
//!   descriptor) and the vertex stream it fetches (index buffer resolved,
//!   only the attribute bytes of every fetched vertex/instance). Queue
//!   writes are applied at submit, before the submit's commands, as wgpu
//!   does; bundles are expanded where they execute. Two builds whose `S `
//!   streams are equal drew the same things, whatever buffers, bind groups,
//!   bundles or offsets they used for it.
#![allow(dead_code, clippy::all, clippy::pedantic)]
use std::cell::Cell;
use std::collections::HashMap;
use std::io::Write as _;
use std::ops::{Bound, Range, RangeBounds};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

// ---------------------------------------------------------------- modes

struct Modes {
    stats: Option<std::path::PathBuf>,
    trace: Option<std::path::PathBuf>,
    gpu_time: bool,
}
fn modes() -> &'static Modes {
    static M: OnceLock<Modes> = OnceLock::new();
    M.get_or_init(|| Modes {
        stats: std::env::var_os("CLIENT910_PERF_STATS").map(Into::into),
        trace: std::env::var_os("CLIENT910_PERF_TRACE").map(Into::into),
        gpu_time: std::env::var_os("CLIENT910_PERF_NO_GPU_TIME").is_none(),
    })
}
fn tracing() -> bool {
    modes().trace.is_some()
}
fn stats() -> bool {
    modes().stats.is_some()
}

// ---------------------------------------------------------------- counters

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum K {
    Buffer,
    BufferBytes,
    BufferInit,
    BindGroup,
    Sampler,
    Texture,
    TextureBytes,
    View,
    BindGroupLayout,
    PipelineLayout,
    Pipeline,
    Shader,
    CommandEncoder,
    BundleEncoder,
    Bundle,
    WriteBuffer,
    WriteBufferBytes,
    WriteTexture,
    WriteTextureBytes,
    Submit,
    Pass,
    Draw,
    ExecuteBundles,
    SetPipeline,
    SetBindGroup,
    Copy,
    /// CPU nanoseconds inside the wgpu calls of each kind (the call only,
    /// not the probe's own bookkeeping or GPU waits).
    CreateBufferNs,
    CreateBindGroupNs,
    WriteBufferNs,
    WriteTextureNs,
    SubmitNs,
    N,
}
const NAMES: [&str; K::N as usize] = [
    "buffers",
    "buffer_bytes",
    "buffer_inits",
    "bind_groups",
    "samplers",
    "textures",
    "texture_bytes",
    "views",
    "bgls",
    "pipeline_layouts",
    "pipelines",
    "shaders",
    "encoders",
    "bundle_encoders",
    "bundles",
    "write_buffer",
    "write_buffer_bytes",
    "write_texture",
    "write_texture_bytes",
    "submits",
    "passes",
    "draws",
    "execute_bundles",
    "set_pipeline",
    "set_bind_group",
    "copies",
    "create_buffer_us",
    "create_bind_group_us",
    "write_buffer_us",
    "write_texture_us",
    "submit_us",
];
static COUNTS: [AtomicU64; K::N as usize] = [const { AtomicU64::new(0) }; K::N as usize];
#[inline]
fn count(k: K, n: u64) {
    COUNTS[k as usize].fetch_add(n, Ordering::Relaxed);
}
/// Run `f`, adding its wall time in nanoseconds to `k`.
#[inline]
fn timed<R>(k: K, f: impl FnOnce() -> R) -> R {
    let t0 = Instant::now();
    let r = f();
    count(k, t0.elapsed().as_nanos() as u64);
    r
}

// ---------------------------------------------------------------- allocator

/// Counts allocations (all threads, and the thread that renders). The
/// instrumented bin installs it as its `#[global_allocator]`.
pub struct CountingAlloc;
static ALL_ALLOCS: AtomicU64 = AtomicU64::new(0);
static ALL_BYTES: AtomicU64 = AtomicU64::new(0);
thread_local! {
    static MY_ALLOCS: Cell<u64> = const { Cell::new(0) };
    static MY_BYTES: Cell<u64> = const { Cell::new(0) };
}
#[inline]
fn note_alloc(bytes: usize) {
    ALL_ALLOCS.fetch_add(1, Ordering::Relaxed);
    ALL_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    let _ = MY_ALLOCS.try_with(|c| c.set(c.get() + 1));
    let _ = MY_BYTES.try_with(|c| c.set(c.get() + bytes as u64));
    let min = SITE_MIN.load(Ordering::Relaxed);
    if min != 0 && bytes as u64 >= min {
        site(bytes);
    }
}

// ---------------------------------------------------------------- alloc sites

/// `CLIENT910_PERF_ALLOC_SITES=<bytes>`: main-thread allocations of at
/// least that size are attributed to their call stack's client frames
/// (sampled every `CLIENT910_PERF_ALLOC_EVERY`-th, default 1) and the top
/// sites by bytes are printed (`PERF_ALLOC_SITE`) every 100 redraws.
static SITE_MIN: AtomicU64 = AtomicU64::new(0);
static SITE_EVERY: AtomicU64 = AtomicU64::new(1);
static SITE_SEEN: AtomicU64 = AtomicU64::new(0);
static SITES: Mutex<Option<HashMap<String, (u64, u64)>>> = Mutex::new(None);
thread_local! {
    static MAIN: Cell<bool> = const { Cell::new(false) };
    static IN_SITE: Cell<bool> = const { Cell::new(false) };
}
fn site(bytes: usize) {
    if !MAIN.try_with(Cell::get).unwrap_or(false) || IN_SITE.try_with(Cell::get).unwrap_or(true) {
        return;
    }
    if SITE_SEEN.fetch_add(1, Ordering::Relaxed) % SITE_EVERY.load(Ordering::Relaxed) != 0 {
        return;
    }
    let _ = IN_SITE.try_with(|c| c.set(true));
    let bt = format!("{}", std::backtrace::Backtrace::force_capture());
    let frames: Vec<&str> = bt
        .lines()
        .map(str::trim)
        .filter(|l| l.contains(": "))
        .filter_map(|l| l.split_once(": ").map(|(_, f)| f))
        .filter(|f| (f.contains("rs910") || f.contains("client910")) && !f.contains("perf_probe"))
        .take(3)
        .collect();
    let key = frames.join(" <- ");
    if let Ok(mut g) = SITES.lock() {
        let e = g
            .get_or_insert_with(HashMap::new)
            .entry(key)
            .or_insert((0, 0));
        e.0 += 1;
        e.1 += bytes as u64;
    }
    let _ = IN_SITE.try_with(|c| c.set(false));
}
fn dump_sites(frame: u64) {
    let _ = IN_SITE.try_with(|c| c.set(true));
    if let Ok(g) = SITES.lock() {
        if let Some(m) = g.as_ref() {
            let mut v: Vec<_> = m.iter().collect();
            v.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
            for (k, (n, b)) in v.into_iter().take(30) {
                eprintln!("PERF_ALLOC_SITE frame {frame} bytes {b} count {n} {k}");
            }
        }
    }
    let _ = IN_SITE.try_with(|c| c.set(false));
}
// SAFETY: forwards every call unchanged to `System`; counting touches only
// atomics and const-initialised thread locals without destructors.
unsafe impl std::alloc::GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        note_alloc(layout.size());
        std::alloc::System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        note_alloc(layout.size());
        std::alloc::System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        note_alloc(new_size);
        std::alloc::System.realloc(ptr, layout, new_size)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        std::alloc::System.dealloc(ptr, layout)
    }
}

// ---------------------------------------------------------------- segments

fn thread_cpu_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: plain libc call with a valid out pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

#[derive(Default)]
struct FrameAcc {
    /// cpu ns, wall ns per segment kind (0 logic, 1 redraw).
    cpu: [u64; 2],
    wall: [u64; 2],
    gpu_ns: u64,
    counts0: Vec<u64>,
    allocs0: (u64, u64, u64, u64),
    frame: u64,
    out: Option<std::io::BufWriter<std::fs::File>>,
}
static FRAME: Mutex<Option<FrameAcc>> = Mutex::new(None);

fn snapshot_counts() -> Vec<u64> {
    COUNTS.iter().map(|c| c.load(Ordering::Relaxed)).collect()
}
fn snapshot_allocs() -> (u64, u64, u64, u64) {
    (
        MY_ALLOCS.try_with(Cell::get).unwrap_or(0),
        MY_BYTES.try_with(Cell::get).unwrap_or(0),
        ALL_ALLOCS.load(Ordering::Relaxed),
        ALL_BYTES.load(Ordering::Relaxed),
    )
}

/// A timed stretch of the main thread: 0 = logic (`about_to_wait`), 1 =
/// redraw (`render_frame`). Ends on drop.
pub struct Segment {
    kind: usize,
    cpu: u64,
    wall: Instant,
}
impl Segment {
    pub fn begin(kind: usize) -> Option<Segment> {
        let _ = MAIN.try_with(|c| c.set(true));
        stats().then(|| Segment {
            kind,
            cpu: thread_cpu_ns(),
            wall: Instant::now(),
        })
    }
}
impl Drop for Segment {
    fn drop(&mut self) {
        let cpu = thread_cpu_ns().saturating_sub(self.cpu);
        let wall = self.wall.elapsed().as_nanos() as u64;
        let mut g = FRAME.lock().unwrap();
        let f = g.get_or_insert_with(new_frame_acc);
        f.cpu[self.kind] += cpu;
        f.wall[self.kind] += wall;
    }
}
fn new_frame_acc() -> FrameAcc {
    let path = modes().stats.as_ref().unwrap();
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let _ = writeln!(
        out,
        "frame\tlogic_cpu_ms\tlogic_wall_ms\tredraw_cpu_ms\tredraw_wall_ms\tgpu_ms\tmain_allocs\tmain_alloc_bytes\tall_allocs\tall_alloc_bytes\t{}",
        NAMES.join("\t")
    );
    FrameAcc {
        counts0: snapshot_counts(),
        allocs0: snapshot_allocs(),
        out: Some(out),
        ..Default::default()
    }
}

/// The device the GPU timing waits on (set by [`frame_end`]; the shell's
/// device outlives every frame).
static DEVICE: AtomicU64 = AtomicU64::new(0);

/// End of one redraw: writes the frame's stats row.
/// (`device`: the device or an `Arc` of it, whichever the tree's device
/// layer holds.)
pub fn frame_end<D: std::borrow::Borrow<wgpu::Device>>(device: Option<&D>) {
    let device = device.map(std::borrow::Borrow::borrow);
    if !stats() {
        return;
    }
    if let Some(d) = device {
        DEVICE.store(d as *const wgpu::Device as u64, Ordering::Relaxed);
    }
    let counts = snapshot_counts();
    let allocs = snapshot_allocs();
    let mut g = FRAME.lock().unwrap();
    let f = g.get_or_insert_with(new_frame_acc);
    let ms = |ns: u64| ns as f64 / 1e6;
    let mut row = format!(
        "{}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{}\t{}\t{}\t{}",
        f.frame,
        ms(f.cpu[0]),
        ms(f.wall[0]),
        ms(f.cpu[1]),
        ms(f.wall[1]),
        ms(f.gpu_ns),
        allocs.0 - f.allocs0.0,
        allocs.1 - f.allocs0.1,
        allocs.2 - f.allocs0.2,
        allocs.3 - f.allocs0.3,
    );
    for (k, (a, b)) in counts.iter().zip(&f.counts0).enumerate() {
        if k >= K::CreateBufferNs as usize {
            row.push_str(&format!("\t{:.1}", (a - b) as f64 / 1e3));
        } else {
            row.push_str(&format!("\t{}", a - b));
        }
    }
    let _ = writeln!(f.out.as_mut().unwrap(), "{row}");
    let _ = f.out.as_mut().unwrap().flush();
    f.frame += 1;
    if f.frame == 60 {
        let var = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok());
        if let Some(min) = var("CLIENT910_PERF_ALLOC_SITES") {
            SITE_EVERY.store(
                var("CLIENT910_PERF_ALLOC_EVERY").unwrap_or(1).max(1),
                Ordering::Relaxed,
            );
            SITE_MIN.store(min.max(1), Ordering::Relaxed);
        }
    }
    if f.frame % 100 == 0 && SITE_MIN.load(Ordering::Relaxed) != 0 {
        dump_sites(f.frame);
    }
    f.cpu = [0; 2];
    f.wall = [0; 2];
    f.gpu_ns = 0;
    f.counts0 = snapshot_counts();
    f.allocs0 = snapshot_allocs();
}

fn device_for_timing() -> Option<&'static wgpu::Device> {
    let p = DEVICE.load(Ordering::Relaxed);
    // SAFETY: the pointer is the shell's device, alive for the whole run
    // (set at the end of every redraw); probe builds only.
    (p != 0).then(|| unsafe { &*(p as *const wgpu::Device) })
}

// ---------------------------------------------------------------- trace state

fn fnv(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x100000001b3);
    }
}
pub fn hash(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325;
    fnv(&mut h, bytes);
    h
}
fn hs(s: &str) -> u64 {
    hash(s.as_bytes())
}

struct Buf {
    label: String,
    data: Vec<u8>,
}
struct Tex {
    desc: String,
    content: u64,
}
#[derive(Clone)]
struct VbLayout {
    stride: u64,
    instance: bool,
    /// (offset, size) of every attribute.
    attrs: Vec<(u64, u64)>,
}
struct Pipe {
    canon: u64,
    label: String,
    vbs: Vec<VbLayout>,
}
enum Res {
    Buf {
        id: u64,
        offset: u64,
        size: Option<u64>,
        dynamic: bool,
    },
    View(u64),
    Sampler(u64),
    Other(String),
}
struct Group {
    layout: u64,
    entries: Vec<(u32, Res)>,
}

#[derive(Clone)]
enum Cmd {
    Pass(String),
    SetPipeline(u64),
    SetBindGroup(u32, u64, Vec<u32>),
    SetVb(u32, u64, u64, Option<u64>),
    SetIb(u64, u64, Option<u64>, wgpu::IndexFormat),
    Draw(Range<u32>, Range<u32>),
    DrawIndexed(Range<u32>, i32, Range<u32>),
    Viewport([f32; 6]),
    Scissor([u32; 4]),
    Bundles(Vec<u64>),
    CopyBuf(u64, u64, u64, u64, u64),
    CopyTex(u64, u64, String),
    CopyTexBuf(u64, String),
}

#[derive(Default)]
struct State {
    out: Option<std::io::BufWriter<std::fs::File>>,
    shaders: HashMap<u64, u64>,
    bgls: HashMap<u64, u64>,
    /// Bind group layout id -> bindings with a dynamic offset.
    bgl_dynamic: HashMap<u64, Vec<u32>>,
    pls: HashMap<u64, u64>,
    pipes: HashMap<u64, Pipe>,
    groups: HashMap<u64, Group>,
    bufs: HashMap<u64, Buf>,
    texs: HashMap<u64, Tex>,
    /// view id -> texture id (or a synthetic id for unknown textures).
    views: HashMap<u64, u64>,
    samplers: HashMap<u64, u64>,
    bundles: HashMap<u64, Vec<Cmd>>,
    recording: Option<Vec<Cmd>>,
    pending: Vec<Cmd>,
    pending_writes: Vec<PendingWrite>,
    seq: u64,
}
enum PendingWrite {
    Buf(u64, u64, Vec<u8>),
    Tex(u64, u64),
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap();
    let s = g.get_or_insert_with(|| State {
        out: Some(std::io::BufWriter::new(
            std::fs::File::create(modes().trace.as_ref().unwrap()).unwrap(),
        )),
        ..Default::default()
    });
    f(s)
}
fn line(s: &mut State, text: String) {
    let _ = writeln!(s.out.as_mut().unwrap(), "{text}");
}

fn format_size(f: wgpu::VertexFormat) -> u64 {
    f.size()
}

// ---------------------------------------------------------------- device

pub trait DeviceTr {
    fn tr_create_shader_module(&self, desc: wgpu::ShaderModuleDescriptor<'_>)
        -> wgpu::ShaderModule;
    fn tr_create_bind_group_layout(
        &self,
        desc: &wgpu::BindGroupLayoutDescriptor<'_>,
    ) -> wgpu::BindGroupLayout;
    fn tr_create_pipeline_layout(
        &self,
        desc: &wgpu::PipelineLayoutDescriptor<'_>,
    ) -> wgpu::PipelineLayout;
    fn tr_create_render_pipeline(
        &self,
        desc: &wgpu::RenderPipelineDescriptor<'_>,
    ) -> wgpu::RenderPipeline;
    fn tr_create_render_bundle_encoder<'a>(
        &'a self,
        desc: &wgpu::RenderBundleEncoderDescriptor<'_>,
    ) -> wgpu::RenderBundleEncoder<'a>;
    fn tr_create_buffer(&self, desc: &wgpu::BufferDescriptor<'_>) -> wgpu::Buffer;
    fn tr_create_buffer_init(&self, desc: &wgpu::util::BufferInitDescriptor<'_>) -> wgpu::Buffer;
    fn tr_create_texture(&self, desc: &wgpu::TextureDescriptor<'_>) -> wgpu::Texture;
    fn tr_create_texture_with_data(
        &self,
        queue: &wgpu::Queue,
        desc: &wgpu::TextureDescriptor<'_>,
        order: wgpu::util::TextureDataOrder,
        data: &[u8],
    ) -> wgpu::Texture;
    fn tr_create_sampler(&self, desc: &wgpu::SamplerDescriptor<'_>) -> wgpu::Sampler;
    fn tr_create_bind_group(&self, desc: &wgpu::BindGroupDescriptor<'_>) -> wgpu::BindGroup;
    fn tr_create_command_encoder(
        &self,
        desc: &wgpu::CommandEncoderDescriptor<'_>,
    ) -> wgpu::CommandEncoder;
}

fn tex_desc(desc: &wgpu::TextureDescriptor<'_>) -> String {
    format!(
        "{:?} {:?} mips{} s{} {:?} {:?} {:?}",
        desc.label,
        desc.size,
        desc.mip_level_count,
        desc.sample_count,
        desc.dimension,
        desc.format,
        desc.usage
    )
}
fn tex_bytes(desc: &wgpu::TextureDescriptor<'_>) -> u64 {
    let (w, h) = desc.format.block_dimensions();
    let block = desc.format.block_copy_size(None).unwrap_or(4) as u64;
    let s = desc.size;
    (u64::from(s.width).div_ceil(u64::from(w)))
        * (u64::from(s.height).div_ceil(u64::from(h)))
        * u64::from(s.depth_or_array_layers)
        * block
        * u64::from(desc.sample_count)
}

impl DeviceTr for wgpu::Device {
    fn tr_create_shader_module(
        &self,
        desc: wgpu::ShaderModuleDescriptor<'_>,
    ) -> wgpu::ShaderModule {
        count(K::Shader, 1);
        let canon = tracing().then(|| {
            let src = match &desc.source {
                wgpu::ShaderSource::Wgsl(s) => hash(s.as_bytes()),
                _ => 0,
            };
            (format!("{:?}", desc.label), src)
        });
        let m = self.create_shader_module(desc);
        if let Some((label, src)) = canon {
            with(|s| {
                line(s, format!("shader {label} {src:016x}"));
                s.shaders.insert(hid(&m), src);
            });
        }
        m
    }
    fn tr_create_bind_group_layout(
        &self,
        desc: &wgpu::BindGroupLayoutDescriptor<'_>,
    ) -> wgpu::BindGroupLayout {
        count(K::BindGroupLayout, 1);
        let l = self.create_bind_group_layout(desc);
        if tracing() {
            let c = hs(&format!("{:?}", desc.entries));
            let dynamic: Vec<u32> = desc
                .entries
                .iter()
                .filter(|e| {
                    matches!(
                        e.ty,
                        wgpu::BindingType::Buffer {
                            has_dynamic_offset: true,
                            ..
                        }
                    )
                })
                .map(|e| e.binding)
                .collect();
            with(|s| {
                line(s, format!("bgl {:?} {c:016x}", desc.label));
                s.bgls.insert(hid(&l), c);
                s.bgl_dynamic.insert(hid(&l), dynamic);
            });
        }
        l
    }
    fn tr_create_pipeline_layout(
        &self,
        desc: &wgpu::PipelineLayoutDescriptor<'_>,
    ) -> wgpu::PipelineLayout {
        count(K::PipelineLayout, 1);
        let l = self.create_pipeline_layout(desc);
        if tracing() {
            with(|s| {
                let groups: Vec<u64> = desc
                    .bind_group_layouts
                    .iter()
                    .map(|b| b.map_or(0, |b| s.bgls.get(&hid(&b)).copied().unwrap_or(0)))
                    .collect();
                let c = hs(&format!("{groups:?}{:?}", desc.immediate_size));
                line(s, format!("playout {:?} {c:016x}", desc.label));
                s.pls.insert(hid(&l), c);
            });
        }
        l
    }
    fn tr_create_render_pipeline(
        &self,
        desc: &wgpu::RenderPipelineDescriptor<'_>,
    ) -> wgpu::RenderPipeline {
        count(K::Pipeline, 1);
        let p = self.create_render_pipeline(desc);
        if tracing() {
            with(|s| {
                let shader = |m: &wgpu::ShaderModule| {
                    s.shaders.get(&hid(&m)).copied().unwrap_or(0)
                };
                let layout = desc
                    .layout
                    .map(|l| s.pls.get(&hid(&l)).copied().unwrap_or(0));
                let frag = desc.fragment.as_ref().map(|f| {
                    format!(
                        "{:016x} {:?} {:?} {:?}",
                        shader(f.module),
                        f.entry_point,
                        f.targets,
                        f.compilation_options
                    )
                });
                let canon = format!(
                    "L{layout:?} V[{:016x} {:?} {:?} {:?}] F[{frag:?}] {:?} {:?} {:?} {:?}",
                    shader(desc.vertex.module),
                    desc.vertex.entry_point,
                    desc.vertex.buffers,
                    desc.vertex.compilation_options,
                    desc.primitive,
                    desc.depth_stencil,
                    desc.multisample,
                    desc.multiview_mask
                );
                let h = hs(&canon);
                line(s, format!("pipeline {h:016x} {:?}", desc.label));
                let vbs = desc
                    .vertex
                    .buffers
                    .iter()
                    .flatten()
                    .map(|b| VbLayout {
                        stride: b.array_stride,
                        instance: b.step_mode == wgpu::VertexStepMode::Instance,
                        attrs: b
                            .attributes
                            .iter()
                            .map(|a| (a.offset, format_size(a.format)))
                            .collect(),
                    })
                    .collect();
                s.pipes.insert(
                    hid(&p),
                    Pipe {
                        canon: h,
                        label: format!("{:?}", desc.label),
                        vbs,
                    },
                );
            });
        }
        p
    }
    fn tr_create_render_bundle_encoder<'a>(
        &'a self,
        desc: &wgpu::RenderBundleEncoderDescriptor<'_>,
    ) -> wgpu::RenderBundleEncoder<'a> {
        count(K::BundleEncoder, 1);
        if tracing() {
            with(|s| {
                line(s, format!("bundle-encoder {:?}", desc.label));
                assert!(s.recording.is_none(), "perf_probe: nested bundle encoders");
                s.recording = Some(Vec::new());
            });
        }
        self.create_render_bundle_encoder(desc)
    }
    fn tr_create_buffer(&self, desc: &wgpu::BufferDescriptor<'_>) -> wgpu::Buffer {
        count(K::Buffer, 1);
        count(K::BufferBytes, desc.size);
        let b = timed(K::CreateBufferNs, || self.create_buffer(desc));
        if tracing() {
            with(|s| {
                line(
                    s,
                    format!("buffer {:?} {:?} len{}", desc.label, desc.usage, desc.size),
                );
                s.bufs.insert(
                    hid(&b),
                    Buf {
                        label: format!("{:?}", desc.label),
                        data: vec![0; desc.size as usize],
                    },
                );
            });
        }
        b
    }
    fn tr_create_buffer_init(&self, desc: &wgpu::util::BufferInitDescriptor<'_>) -> wgpu::Buffer {
        use wgpu::util::DeviceExt as _;
        count(K::Buffer, 1);
        count(K::BufferInit, 1);
        count(K::BufferBytes, desc.contents.len() as u64);
        let b = timed(K::CreateBufferNs, || self.create_buffer_init(desc));
        if tracing() {
            with(|s| {
                line(
                    s,
                    format!(
                        "buffer-init {:?} {:?} len{} h{:016x}",
                        desc.label,
                        desc.usage,
                        desc.contents.len(),
                        hash(desc.contents)
                    ),
                );
                // create_buffer_init pads to COPY_BUFFER_ALIGNMENT.
                let mut data = desc.contents.to_vec();
                data.resize(b.size() as usize, 0);
                s.bufs.insert(
                    hid(&b),
                    Buf {
                        label: format!("{:?}", desc.label),
                        data,
                    },
                );
            });
        }
        b
    }
    fn tr_create_texture(&self, desc: &wgpu::TextureDescriptor<'_>) -> wgpu::Texture {
        count(K::Texture, 1);
        count(K::TextureBytes, tex_bytes(desc));
        let t = self.create_texture(desc);
        if tracing() {
            let d = tex_desc(desc);
            with(|s| {
                line(s, format!("texture {d}"));
                s.texs.insert(
                    hid(&t),
                    Tex {
                        desc: d,
                        content: 0,
                    },
                );
            });
        }
        t
    }
    fn tr_create_texture_with_data(
        &self,
        queue: &wgpu::Queue,
        desc: &wgpu::TextureDescriptor<'_>,
        order: wgpu::util::TextureDataOrder,
        data: &[u8],
    ) -> wgpu::Texture {
        use wgpu::util::DeviceExt as _;
        count(K::Texture, 1);
        count(K::TextureBytes, tex_bytes(desc));
        let t = self.create_texture_with_data(queue, desc, order, data);
        if tracing() {
            let d = tex_desc(desc);
            let h = hash(data);
            with(|s| {
                line(s, format!("texture-data {d} {order:?} h{h:016x}"));
                s.texs.insert(
                    hid(&t),
                    Tex {
                        desc: d,
                        content: h,
                    },
                );
            });
        }
        t
    }
    fn tr_create_sampler(&self, desc: &wgpu::SamplerDescriptor<'_>) -> wgpu::Sampler {
        count(K::Sampler, 1);
        let x = self.create_sampler(desc);
        if tracing() {
            // The label is a name, not state.
            let c = hs(&format!(
                "{:?}",
                wgpu::SamplerDescriptor {
                    label: None,
                    ..desc.clone()
                }
            ));
            with(|s| {
                line(s, format!("sampler {:?} {c:016x}", desc.label));
                s.samplers.insert(hid(&x), c);
            });
        }
        x
    }
    fn tr_create_bind_group(&self, desc: &wgpu::BindGroupDescriptor<'_>) -> wgpu::BindGroup {
        count(K::BindGroup, 1);
        let g = timed(K::CreateBindGroupNs, || self.create_bind_group(desc));
        if tracing() {
            with(|s| {
                let layout = s
                    .bgls
                    .get(&hid(&desc.layout))
                    .copied()
                    .unwrap_or(0);
                let dynamic = s
                    .bgl_dynamic
                    .get(&hid(&desc.layout))
                    .cloned()
                    .unwrap_or_default();
                let mut entries: Vec<(u32, Res)> = desc
                    .entries
                    .iter()
                    .map(|e| {
                        let r = match &e.resource {
                            wgpu::BindingResource::Buffer(b) => Res::Buf {
                                id: hid(&b.buffer),
                                offset: b.offset,
                                size: b.size.map(|n| n.get()),
                                dynamic: dynamic.contains(&e.binding),
                            },
                            wgpu::BindingResource::TextureView(v) => {
                                Res::View(hid(&v))
                            }
                            wgpu::BindingResource::Sampler(x) => {
                                Res::Sampler(hid(&x))
                            }
                            other => Res::Other(format!("{other:?}")),
                        };
                        (e.binding, r)
                    })
                    .collect();
                // Dynamic offsets apply in binding order.
                entries.sort_by_key(|e| e.0);
                line(s, format!("bindgroup {:?}", desc.label));
                s.groups
                    .insert(hid(&g), Group { layout, entries });
            });
        }
        g
    }
    fn tr_create_command_encoder(
        &self,
        desc: &wgpu::CommandEncoderDescriptor<'_>,
    ) -> wgpu::CommandEncoder {
        count(K::CommandEncoder, 1);
        self.create_command_encoder(desc)
    }
}

pub trait TextureTr {
    fn tr_create_view(&self, desc: &wgpu::TextureViewDescriptor<'_>) -> wgpu::TextureView;
}
impl TextureTr for wgpu::Texture {
    fn tr_create_view(&self, desc: &wgpu::TextureViewDescriptor<'_>) -> wgpu::TextureView {
        count(K::View, 1);
        let v = self.create_view(desc);
        if tracing() {
            let tid = hid(&self);
            let size = self.size();
            let format = self.format();
            let vd = format!(
                "{:?} {:?} {:?} {} {:?} {} {:?}",
                desc.format,
                desc.dimension,
                desc.aspect,
                desc.base_mip_level,
                desc.mip_level_count,
                desc.base_array_layer,
                desc.array_layer_count
            );
            with(|s| {
                s.texs.entry(tid).or_insert_with(|| Tex {
                    desc: format!("external {size:?} {format:?}"),
                    content: 0,
                });
                // A view is its texture plus the view descriptor: keyed by a
                // synthetic texture-like id per (texture, view desc).
                let vid = hid(&v);
                s.views.insert(vid, tid);
                s.texs.entry(u64::MAX - vid).or_insert(Tex {
                    desc: vd,
                    content: 0,
                });
            });
        }
        v
    }
}

// ---------------------------------------------------------------- queue

/// Buffer writes: `wgpu::Queue::write_buffer`, or anything implementing
/// the renderer's buffer-write trait (instrument.py appends the impl that
/// fits the tree: `impl WriteTr for wgpu::Queue` or one over
/// `rs910_gpu_device::uploads::Uploader`). Either way a write lands before
/// the next submission's commands, which is how the trace applies it.
pub trait WriteTr {
    fn tr_write_buffer(&self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]);
}
/// A submission through a wrapper that submits the queue itself (the
/// device layer's `submit` when the tree has one; instrument.py appends
/// the impl): counted and traced where it calls `queue.submit`.
pub trait DeviceSubmitTr {
    fn tr_submit<I: IntoIterator<Item = wgpu::CommandBuffer>>(
        &self,
        buffers: I,
    ) -> wgpu::SubmissionIndex;
}
/// Record a buffer write (counts, and the pending write of the trace).
pub fn note_write(buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
    count(K::WriteBuffer, 1);
    count(K::WriteBufferBytes, data.len() as u64);
    if tracing() {
        with(|s| {
            s.pending_writes.push(PendingWrite::Buf(
                hid(&buffer),
                offset,
                data.to_vec(),
            ))
        });
    }
}
/// Time a buffer write call.
pub fn time_write(f: impl FnOnce()) {
    timed(K::WriteBufferNs, f);
}

pub trait QueueTr {
    fn tr_write_texture(
        &self,
        texture: wgpu::TexelCopyTextureInfo<'_>,
        data: &[u8],
        layout: wgpu::TexelCopyBufferLayout,
        size: wgpu::Extent3d,
    );
    fn tr_submit<I: IntoIterator<Item = wgpu::CommandBuffer>>(
        &self,
        buffers: I,
    ) -> wgpu::SubmissionIndex;
}
impl QueueTr for wgpu::Queue {
    fn tr_write_texture(
        &self,
        texture: wgpu::TexelCopyTextureInfo<'_>,
        data: &[u8],
        layout: wgpu::TexelCopyBufferLayout,
        size: wgpu::Extent3d,
    ) {
        count(K::WriteTexture, 1);
        count(K::WriteTextureBytes, data.len() as u64);
        if tracing() {
            let h = hs(&format!(
                "{} {:?} {:?} {layout:?} {size:?} {:016x}",
                texture.mip_level,
                texture.origin,
                texture.aspect,
                hash(data)
            ));
            with(|s| {
                s.pending_writes
                    .push(PendingWrite::Tex(hid(&texture.texture), h))
            });
        }
        timed(K::WriteTextureNs, || {
            self.write_texture(texture, data, layout, size)
        });
    }
    fn tr_submit<I: IntoIterator<Item = wgpu::CommandBuffer>>(
        &self,
        buffers: I,
    ) -> wgpu::SubmissionIndex {
        count(K::Submit, 1);
        if tracing() {
            with(resolve);
        }
        let timing = stats() && modes().gpu_time;
        let device = if timing { device_for_timing() } else { None };
        if let Some(d) = device {
            let _ = d.poll(wgpu::PollType::wait_indefinitely());
        }
        let t0 = Instant::now();
        let index = timed(K::SubmitNs, || self.submit(buffers));
        if let Some(d) = device {
            let _ = d.poll(wgpu::PollType::Wait { submission_index: Some(index.clone()), timeout: None });
            let ns = t0.elapsed().as_nanos() as u64;
            let mut g = FRAME.lock().unwrap();
            g.get_or_insert_with(new_frame_acc).gpu_ns += ns;
        }
        index
    }
}

// ---------------------------------------------------------------- encoder

pub trait EncoderTr {
    fn tr_begin_render_pass<'e>(
        &'e mut self,
        desc: &wgpu::RenderPassDescriptor<'_>,
    ) -> wgpu::RenderPass<'e>;
    fn tr_copy_texture_to_texture(
        &mut self,
        source: wgpu::TexelCopyTextureInfo<'_>,
        destination: wgpu::TexelCopyTextureInfo<'_>,
        size: wgpu::Extent3d,
    );
    fn tr_copy_buffer_to_buffer(
        &mut self,
        src: &wgpu::Buffer,
        so: u64,
        dst: &wgpu::Buffer,
        doff: u64,
        size: u64,
    );
    fn tr_copy_texture_to_buffer(
        &mut self,
        source: wgpu::TexelCopyTextureInfo<'_>,
        destination: wgpu::TexelCopyBufferInfo<'_>,
        size: wgpu::Extent3d,
    );
}
fn push(cmd: Cmd) {
    with(|s| s.pending.push(cmd));
}
impl EncoderTr for wgpu::CommandEncoder {
    fn tr_begin_render_pass<'e>(
        &'e mut self,
        desc: &wgpu::RenderPassDescriptor<'_>,
    ) -> wgpu::RenderPass<'e> {
        count(K::Pass, 1);
        if tracing() {
            let colours: Vec<String> = desc
                .color_attachments
                .iter()
                .map(|c| {
                    c.as_ref().map_or("none".into(), |c| {
                        format!(
                            "V{} {:?} R{:?}",
                            hid(&c.view),
                            c.ops,
                            c.resolve_target.map(|r| hid(&r))
                        )
                    })
                })
                .collect();
            let depth = desc.depth_stencil_attachment.as_ref().map(|d| {
                format!(
                    "V{} {:?} {:?}",
                    hid(&d.view),
                    d.depth_ops,
                    d.stencil_ops
                )
            });
            push(Cmd::Pass(format!(
                "{:?} {colours:?} depth={depth:?}",
                desc.label
            )));
        }
        self.begin_render_pass(desc)
    }
    fn tr_copy_texture_to_texture(
        &mut self,
        source: wgpu::TexelCopyTextureInfo<'_>,
        destination: wgpu::TexelCopyTextureInfo<'_>,
        size: wgpu::Extent3d,
    ) {
        count(K::Copy, 1);
        if tracing() {
            push(Cmd::CopyTex(
                hid(&source.texture),
                hid(&destination.texture),
                format!("{:?} {:?} {size:?}", source.origin, destination.origin),
            ));
        }
        self.copy_texture_to_texture(source, destination, size);
    }
    fn tr_copy_buffer_to_buffer(
        &mut self,
        src: &wgpu::Buffer,
        so: u64,
        dst: &wgpu::Buffer,
        doff: u64,
        size: u64,
    ) {
        count(K::Copy, 1);
        if tracing() {
            push(Cmd::CopyBuf(
                hid(&src),
                so,
                hid(&dst),
                doff,
                size,
            ));
        }
        self.copy_buffer_to_buffer(src, so, dst, doff, size);
    }
    fn tr_copy_texture_to_buffer(
        &mut self,
        source: wgpu::TexelCopyTextureInfo<'_>,
        destination: wgpu::TexelCopyBufferInfo<'_>,
        size: wgpu::Extent3d,
    ) {
        count(K::Copy, 1);
        if tracing() {
            push(Cmd::CopyTexBuf(
                hid(&source.texture),
                format!("{:?} {size:?} {:?}", source.origin, destination.layout),
            ));
        }
        self.copy_texture_to_buffer(source, destination, size);
    }
}

// ---------------------------------------------------------------- passes

fn bounds<S: RangeBounds<u64>>(r: &S) -> (u64, Option<u64>) {
    let start = match r.start_bound() {
        Bound::Included(&x) => x,
        Bound::Excluded(&x) => x + 1,
        Bound::Unbounded => 0,
    };
    let end = match r.end_bound() {
        Bound::Included(&x) => Some(x + 1),
        Bound::Excluded(&x) => Some(x),
        Bound::Unbounded => None,
    };
    (start, end)
}

pub trait PassTr<'a> {
    fn tr_set_pipeline(&mut self, p: &'a wgpu::RenderPipeline);
    fn tr_set_bind_group(&mut self, i: u32, g: &'a wgpu::BindGroup, o: &[u32]);
    fn tr_set_vertex_buffer<S: RangeBounds<u64>>(&mut self, slot: u32, b: &'a wgpu::Buffer, r: S);
    fn tr_set_index_buffer<S: RangeBounds<u64>>(
        &mut self,
        b: &'a wgpu::Buffer,
        r: S,
        f: wgpu::IndexFormat,
    );
    fn tr_draw_indexed(&mut self, r: Range<u32>, base: i32, i: Range<u32>);
    fn tr_draw(&mut self, v: Range<u32>, i: Range<u32>);
}
macro_rules! pass_tr {
    ($rec:expr) => {
        fn tr_set_pipeline(&mut self, p: &'a wgpu::RenderPipeline) {
            count(K::SetPipeline, 1);
            if tracing() {
                $rec(Cmd::SetPipeline(hid(&p)));
            }
            self.set_pipeline(p);
        }
        fn tr_set_bind_group(&mut self, i: u32, g: &'a wgpu::BindGroup, o: &[u32]) {
            count(K::SetBindGroup, 1);
            if tracing() {
                $rec(Cmd::SetBindGroup(i, hid(&g), o.to_vec()));
            }
            self.set_bind_group(i, g, o);
        }
        fn tr_set_vertex_buffer<S: RangeBounds<u64>>(
            &mut self,
            slot: u32,
            b: &'a wgpu::Buffer,
            r: S,
        ) {
            if tracing() {
                let (start, end) = bounds(&r);
                $rec(Cmd::SetVb(slot, hid(&b), start, end));
            }
            self.set_vertex_buffer(slot, b.slice(r));
        }
        fn tr_set_index_buffer<S: RangeBounds<u64>>(
            &mut self,
            b: &'a wgpu::Buffer,
            r: S,
            f: wgpu::IndexFormat,
        ) {
            if tracing() {
                let (start, end) = bounds(&r);
                $rec(Cmd::SetIb(hid(&b), start, end, f));
            }
            self.set_index_buffer(b.slice(r), f);
        }
        fn tr_draw_indexed(&mut self, r: Range<u32>, base: i32, i: Range<u32>) {
            count(K::Draw, 1);
            if tracing() {
                $rec(Cmd::DrawIndexed(r.clone(), base, i.clone()));
            }
            self.draw_indexed(r, base, i);
        }
        fn tr_draw(&mut self, v: Range<u32>, i: Range<u32>) {
            count(K::Draw, 1);
            if tracing() {
                $rec(Cmd::Draw(v.clone(), i.clone()));
            }
            self.draw(v, i);
        }
    };
}
fn rec_pass(c: Cmd) {
    push(c);
}
fn rec_bundle(c: Cmd) {
    with(|s| {
        s.recording
            .as_mut()
            .expect("perf_probe: bundle command outside a bundle encoder")
            .push(c)
    });
}
impl<'a, 'e> PassTr<'a> for wgpu::RenderPass<'e> {
    pass_tr!(rec_pass);
}
impl<'a> PassTr<'a> for wgpu::RenderBundleEncoder<'a> {
    pass_tr!(rec_bundle);
}
pub trait RpassTr {
    fn tr_set_viewport(&mut self, x: f32, y: f32, w: f32, h: f32, a: f32, b: f32);
    fn tr_set_scissor_rect(&mut self, x: u32, y: u32, w: u32, h: u32);
    fn tr_execute_bundles<'b, I: IntoIterator<Item = &'b wgpu::RenderBundle>>(&mut self, it: I);
}
impl<'e> RpassTr for wgpu::RenderPass<'e> {
    fn tr_set_viewport(&mut self, x: f32, y: f32, w: f32, h: f32, a: f32, b: f32) {
        if tracing() {
            push(Cmd::Viewport([x, y, w, h, a, b]));
        }
        self.set_viewport(x, y, w, h, a, b);
    }
    fn tr_set_scissor_rect(&mut self, x: u32, y: u32, w: u32, h: u32) {
        if tracing() {
            push(Cmd::Scissor([x, y, w, h]));
        }
        self.set_scissor_rect(x, y, w, h);
    }
    fn tr_execute_bundles<'b, I: IntoIterator<Item = &'b wgpu::RenderBundle>>(&mut self, it: I) {
        count(K::ExecuteBundles, 1);
        let v: Vec<&wgpu::RenderBundle> = it.into_iter().collect();
        if tracing() {
            push(Cmd::Bundles(
                v.iter().map(|b| hid(&b)).collect(),
            ));
        }
        self.execute_bundles(v);
    }
}
pub trait BundleTr {
    fn tr_finish(self, desc: &wgpu::RenderBundleDescriptor<'_>) -> wgpu::RenderBundle;
}
impl<'a> BundleTr for wgpu::RenderBundleEncoder<'a> {
    fn tr_finish(self, desc: &wgpu::RenderBundleDescriptor<'_>) -> wgpu::RenderBundle {
        count(K::Bundle, 1);
        let b = self.finish(desc);
        if tracing() {
            with(|s| {
                let cmds = s.recording.take().unwrap_or_default();
                line(s, format!("bundle {:?} cmds{}", desc.label, cmds.len()));
                s.bundles.insert(hid(&b), cmds);
            });
        }
        b
    }
}

// ---------------------------------------------------------------- resolution

#[derive(Default, Clone)]
struct PassState {
    pipeline: Option<u64>,
    groups: HashMap<u32, (u64, Vec<u32>)>,
    vbs: HashMap<u32, (u64, u64, Option<u64>)>,
    ib: Option<(u64, u64, Option<u64>, wgpu::IndexFormat)>,
    viewport: Option<[f32; 6]>,
    scissor: Option<[u32; 4]>,
}

fn tex_canon(s: &State, tex: u64) -> String {
    s.texs.get(&tex).map_or(format!("?{tex}"), |t| {
        format!("{}#{:016x}", t.desc, t.content)
    })
}
fn view_canon(s: &State, view: u64) -> String {
    let tex = s.views.get(&view).copied().unwrap_or(0);
    let vd = s
        .texs
        .get(&(u64::MAX - view))
        .map_or("?", |t| t.desc.as_str());
    format!("[{} | {vd}]", tex_canon(s, tex))
}

fn slice_bytes<'s>(s: &'s State, id: u64, start: u64, end: Option<u64>) -> &'s [u8] {
    let Some(b) = s.bufs.get(&id) else {
        return &[];
    };
    let len = b.data.len() as u64;
    let e = end.unwrap_or(len).min(len);
    &b.data[start.min(e) as usize..e as usize]
}

fn group_hash(s: &State, id: u64, dynamic: &[u32]) -> String {
    let Some(g) = s.groups.get(&id) else {
        return "?group".into();
    };
    let mut h = 0xcbf29ce484222325u64;
    fnv(&mut h, &g.layout.to_le_bytes());
    let mut dyn_i = 0;
    for (binding, r) in &g.entries {
        fnv(&mut h, &binding.to_le_bytes());
        match r {
            Res::Buf {
                id,
                offset,
                size,
                dynamic: is_dynamic,
            } => {
                // Dynamic offsets apply in binding order to the layout's
                // dynamic buffer bindings.
                let extra = if *is_dynamic && dyn_i < dynamic.len() {
                    let o = dynamic[dyn_i];
                    dyn_i += 1;
                    u64::from(o)
                } else {
                    0
                };
                let start = offset + extra;
                let bytes = slice_bytes(s, *id, start, size.map(|n| start + n));
                fnv(&mut h, &(bytes.len() as u64).to_le_bytes());
                fnv(&mut h, bytes);
            }
            Res::View(v) => fnv(&mut h, view_canon(s, *v).as_bytes()),
            Res::Sampler(x) => fnv(
                &mut h,
                &s.samplers.get(x).copied().unwrap_or(0).to_le_bytes(),
            ),
            Res::Other(o) => fnv(&mut h, o.as_bytes()),
        }
    }
    format!("{h:016x}")
}

fn vertex_stream(
    s: &State,
    st: &PassState,
    verts: &mut dyn Iterator<Item = u32>,
    instances: Range<u32>,
) -> (u64, u64) {
    let Some(pipe) = st.pipeline.and_then(|p| s.pipes.get(&p)) else {
        return (0, 0);
    };
    let mut h = 0xcbf29ce484222325u64;
    let mut n = 0u64;
    let fetch = |h: &mut u64, slot: usize, lay: &VbLayout, element: u64| {
        let Some(&(id, start, end)) = st.vbs.get(&(slot as u32)) else {
            fnv(h, b"novb");
            return;
        };
        let bytes = slice_bytes(s, id, start, end);
        let base = element * lay.stride;
        for &(off, size) in &lay.attrs {
            let a = (base + off) as usize;
            let b = a + size as usize;
            if b <= bytes.len() {
                fnv(h, &bytes[a..b]);
            } else {
                fnv(h, b"oob");
            }
        }
    };
    for (slot, lay) in pipe.vbs.iter().enumerate() {
        if lay.instance {
            for i in instances.clone() {
                fetch(&mut h, slot, lay, u64::from(i));
            }
        }
    }
    for v in verts {
        n += 1;
        for (slot, lay) in pipe.vbs.iter().enumerate() {
            if !lay.instance {
                fetch(&mut h, slot, lay, u64::from(v));
            }
        }
    }
    (h, n)
}

fn emit_draw(s: &mut State, st: &PassState, cmd: &Cmd) {
    let (pipe, label) = st
        .pipeline
        .and_then(|p| s.pipes.get(&p))
        .map_or((0, "?".to_string()), |p| (p.canon, p.label.clone()));
    let mut groups: Vec<(u32, String)> = st
        .groups
        .iter()
        .map(|(i, (g, o))| (*i, group_hash(s, *g, o)))
        .collect();
    groups.sort();
    let (stream, n, inst) = match cmd {
        Cmd::Draw(v, i) => {
            let (h, n) = vertex_stream(s, st, &mut v.clone(), i.clone());
            (h, n, i.clone())
        }
        Cmd::DrawIndexed(r, base, i) => {
            let indices: Vec<u32> = match st.ib {
                Some((id, start, end, f)) => {
                    let bytes = slice_bytes(s, id, start, end);
                    let size = if f == wgpu::IndexFormat::Uint16 { 2 } else { 4 };
                    r.clone()
                        .map(|k| {
                            let a = k as usize * size;
                            let raw = if a + size <= bytes.len() {
                                if size == 2 {
                                    u32::from(u16::from_le_bytes([bytes[a], bytes[a + 1]]))
                                } else {
                                    u32::from_le_bytes(bytes[a..a + 4].try_into().unwrap())
                                }
                            } else {
                                u32::MAX
                            };
                            (raw as i64 + i64::from(*base)) as u32
                        })
                        .collect()
                }
                None => Vec::new(),
            };
            let (h, n) = vertex_stream(s, st, &mut indices.into_iter(), i.clone());
            (h, n, i.clone())
        }
        _ => unreachable!(),
    };
    let text = format!(
        "S draw {pipe:016x} {label} vp{:?} sc{:?} g{groups:?} v{stream:016x} n{n} i{inst:?}",
        st.viewport, st.scissor
    );
    line(s, text);
}

fn run_cmds(s: &mut State, cmds: &[Cmd], st: &mut PassState) {
    for cmd in cmds {
        match cmd {
            Cmd::Pass(desc) => {
                *st = PassState::default();
                // Attachments by content description, not id.
                let text = resolve_pass_desc(s, desc);
                line(s, format!("S pass {text}"));
            }
            Cmd::SetPipeline(p) => st.pipeline = Some(*p),
            Cmd::SetBindGroup(i, g, o) => {
                st.groups.insert(*i, (*g, o.clone()));
            }
            Cmd::SetVb(slot, id, a, b) => {
                st.vbs.insert(*slot, (*id, *a, *b));
            }
            Cmd::SetIb(id, a, b, f) => st.ib = Some((*id, *a, *b, *f)),
            Cmd::Viewport(v) => st.viewport = Some(*v),
            Cmd::Scissor(v) => st.scissor = Some(*v),
            Cmd::Draw(..) | Cmd::DrawIndexed(..) => emit_draw(s, st, cmd),
            Cmd::Bundles(ids) => {
                for id in ids {
                    let cmds = s.bundles.get(id).cloned().unwrap_or_default();
                    // A bundle starts from empty state and leaves the pass
                    // state cleared; viewport/scissor are the pass's.
                    let mut bs = PassState {
                        viewport: st.viewport,
                        scissor: st.scissor,
                        ..Default::default()
                    };
                    run_cmds(s, &cmds, &mut bs);
                }
                let (viewport, scissor) = (st.viewport, st.scissor);
                *st = PassState {
                    viewport,
                    scissor,
                    ..Default::default()
                };
            }
            Cmd::CopyBuf(src, so, dst, doff, size) => {
                let data = slice_bytes(s, *src, *so, Some(so + size)).to_vec();
                if let Some(b) = s.bufs.get_mut(dst) {
                    let a = *doff as usize;
                    let e = (a + data.len()).min(b.data.len());
                    b.data[a..e].copy_from_slice(&data[..e - a]);
                }
                line(s, format!("S copy-buffer len{size} h{:016x}", hash(&data)));
            }
            Cmd::CopyTex(src, dst, d) => {
                let text = format!(
                    "S copy-texture {} -> {} {d}",
                    tex_canon(s, *src),
                    tex_canon(s, *dst)
                );
                line(s, text);
            }
            Cmd::CopyTexBuf(src, d) => {
                let text = format!("S copy-texture-buffer {} {d}", tex_canon(s, *src));
                line(s, text);
            }
        }
    }
}

fn resolve_pass_desc(s: &State, desc: &str) -> String {
    // Replace `V<id>` / `R<Some(id)>` references with their view canon.
    let mut out = String::new();
    let mut rest = desc;
    while let Some(k) = rest.find('V') {
        out.push_str(&rest[..k]);
        let tail = &rest[k + 1..];
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            out.push('V');
            rest = tail;
        } else {
            out.push_str(&view_canon(s, digits.parse().unwrap()));
            rest = &tail[digits.len()..];
        }
    }
    out.push_str(rest);
    let mut res = String::new();
    let mut rest = out.as_str();
    while let Some(k) = rest.find("RSome(") {
        res.push_str(&rest[..k]);
        let tail = &rest[k + 6..];
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        res.push_str(&format!("R{}", view_canon(s, digits.parse().unwrap_or(0))));
        rest = &tail[digits.len() + 1..];
    }
    res.push_str(rest);
    res
}

/// The bytes `buffer` holds for the next submission's commands (its
/// shadow with the pending writes applied), in trace mode; `None` otherwise
/// or for an untracked buffer. For in-process equivalence checks
/// (`CLIENT910_PERF_CHECK_REUSE`, instrument.py).
pub fn buffer_contents(buffer: &wgpu::Buffer) -> Option<Vec<u8>> {
    if !tracing() {
        return None;
    }
    let id = hid(&buffer);
    with(|s| {
        let mut data = s.bufs.get(&id)?.data.clone();
        for w in &s.pending_writes {
            if let PendingWrite::Buf(b, offset, bytes) = w {
                if *b == id {
                    let a = *offset as usize;
                    let e = (a + bytes.len()).min(data.len());
                    data[a..e].copy_from_slice(&bytes[..e - a]);
                }
            }
        }
        Some(data)
    })
}

/// Reuse checks run and failed (`CLIENT910_PERF_CHECK_REUSE`).
pub static REUSE_CHECKS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// Whether the in-process reuse check is on (trace mode and
/// `CLIENT910_PERF_CHECK_REUSE`).
pub fn check_reuse() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    tracing() && *ON.get_or_init(|| std::env::var_os("CLIENT910_PERF_CHECK_REUSE").is_some())
}

/// Record one reuse check's outcome (a mismatch is printed).
pub fn reuse_checked(result: Result<(), String>) {
    REUSE_CHECKS[0].fetch_add(1, Ordering::Relaxed);
    if let Err(e) = result {
        REUSE_CHECKS[1].fetch_add(1, Ordering::Relaxed);
        eprintln!("PERF_REUSE_MISMATCH {e}");
    }
    let n = REUSE_CHECKS[0].load(Ordering::Relaxed);
    if n.is_power_of_two() {
        eprintln!(
            "PERF_REUSE_CHECKS {n} mismatches {}",
            REUSE_CHECKS[1].load(Ordering::Relaxed)
        );
    }
}

fn resolve(s: &mut State) {
    for w in std::mem::take(&mut s.pending_writes) {
        match w {
            PendingWrite::Buf(id, offset, data) => {
                let h = hash(&data);
                let label = if let Some(b) = s.bufs.get_mut(&id) {
                    let a = offset as usize;
                    let e = (a + data.len()).min(b.data.len());
                    b.data[a..e].copy_from_slice(&data[..e - a]);
                    b.label.clone()
                } else {
                    "?".into()
                };
                line(
                    s,
                    format!("write-buffer {label} @{offset} len{} h{h:016x}", data.len()),
                );
            }
            PendingWrite::Tex(id, h) => {
                if let Some(t) = s.texs.get_mut(&id) {
                    let mut c = t.content;
                    fnv(&mut c, &h.to_le_bytes());
                    t.content = c;
                }
            }
        }
    }
    let cmds = std::mem::take(&mut s.pending);
    let mut st = PassState::default();
    run_cmds(s, &cmds, &mut st);
    s.seq += 1;
    line(s, "S submit".into());
    let _ = s.out.as_mut().unwrap().flush();
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
