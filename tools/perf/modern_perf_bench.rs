//! The modern renderer's headless matrix bench (never in the repo crate:
//! `tools/perf/modern_stage.py` adds it to a staged copy as
//! `frame::perf_bench`; `tools/perf/modern_bench.sh` builds and runs it).
//!
//! Offline scenes with the client's defaults (`ModernSettings::DEFAULT`:
//! the calibrated look, the shadow presets, the atmosphere layers, the
//! water effects, the skybox, far level 2, probes settled), one renderer per
//! configuration on a device with `TIMESTAMP_QUERY` and the client's texture
//! features. Per measured frame: `draw` CPU (and its phases from the hook),
//! `finish`, `submit`, submit-to-idle wall time, per-pass GPU timestamps,
//! the hook's counters and the draw's heap allocations. Per configuration:
//! renderer creation, the first frame, the settle (probe capture) frames,
//! Metal's allocated size and the process RSS, and the load average.
//!
//! Environment: `MODERN_BENCH_OUT` (directory, required), `MODERN_BENCH_PLAN`
//! (`base`, `shadows`, `far`, `ablate`, `points`, comma list; default
//! `base`; `points`: every shadow quality with point shadows off and on),
//! `MODERN_BENCH_SCENES`, `MODERN_BENCH_SIZES` (`WxH` list), `MODERN_BENCH_AA`
//! (`4,1`), `MODERN_BENCH_FRAMES` (default 60), `MODERN_BENCH_HOLD_SECS` (after
//! the plan, draw the first configuration for that long: `sample` target),
//! `MODERN_BENCH_NO_TS=1` (no timestamps: timing without their overhead),
//! `MODERN_BENCH_THREADS` (a list of the renderer's thread counts, each
//! configuration run with each, labelled `tN`; unset: the renderer's
//! default, unlabelled; the settings views take the first).
use super::tests::{river_scene, OfflineScene};
use super::*;
use crate::modern_perf_hook as hook;
use crate::settings::ModernSettings;
use crate::shadows::Quality;
use std::io::Write as _;
use std::time::Instant;

struct Counting;
// SAFETY: forwards to the system allocator; only counts.
unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        hook::note_alloc(layout.size());
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        hook::note_alloc(layout.size());
        unsafe { std::alloc::System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new: usize) -> *mut u8 {
        hook::note_alloc(new);
        unsafe { std::alloc::System.realloc(ptr, layout, new) }
    }
}
#[global_allocator]
static ALLOC: Counting = Counting;

#[derive(Clone, Debug)]
struct Config {
    plan: &'static str,
    label: String,
    samples: u32,
    shadow: Option<Quality>,
    far: Option<u8>,
    settings: ModernSettings,
    /// The renderer's threads (`None`: its default).
    threads: Option<usize>,
    /// Point-light shadows off (the tests' switch; shadows on otherwise).
    point_off: bool,
}

fn quality_name(q: Option<Quality>) -> &'static str {
    match q {
        None => "off",
        Some(Quality::Low) => "low",
        Some(Quality::Medium) => "med",
        Some(Quality::High) => "high",
        Some(Quality::Ultra) => "ultra",
        Some(Quality::UltraPlus) => "ultra+",
    }
}

fn configs(plans: &[String], aa: &[u32]) -> Vec<Config> {
    let base = |plan, samples, shadow, far: Option<u8>, label: String| Config {
        plan,
        label,
        samples,
        shadow,
        far,
        settings: ModernSettings::DEFAULT,
        threads: None,
        point_off: false,
    };
    let mut out = Vec::new();
    for &s in aa {
        if plans.iter().any(|p| p == "base") {
            out.push(base("base", s, Some(Quality::Medium), Some(2), "default".into()));
        }
        if plans.iter().any(|p| p == "shadows") {
            for q in [None, Some(Quality::Low), Some(Quality::High), Some(Quality::Ultra), Some(Quality::UltraPlus)] {
                out.push(base("shadows", s, q, Some(2), format!("shadows {}", quality_name(q))));
            }
        }
        if plans.iter().any(|p| p == "motion") {
            // A moving camera (`MODERN_BENCH_MOTION` units a frame along x):
            // the shadow cache's worst case.
            for q in [Some(Quality::Medium), Some(Quality::Ultra)] {
                out.push(base("motion", s, q, Some(2), format!("moving {}", quality_name(q))));
            }
        }
        if plans.iter().any(|p| p == "far") {
            for f in [None, Some(4)] {
                out.push(base("far", s, Some(Quality::Medium), f, format!("far {}", f.map_or("off".into(), |v| v.to_string()))));
            }
        }
        // Lane P6-FAR: every draw-distance level (the aerial review view).
        if plans.iter().any(|p| p == "levels") {
            for f in [None, Some(0), Some(2), Some(4)] {
                out.push(base("levels", s, Some(Quality::Medium), f, format!("level {}", f.map_or("off".into(), |v| v.to_string()))));
            }
        }
        // Point-light shadows: every quality with them off and on (the lit
        // scenes: Lumbridge and Draynor).
        if plans.iter().any(|p| p == "points") {
            for q in [Quality::Low, Quality::Medium, Quality::High, Quality::Ultra, Quality::UltraPlus] {
                for off in [true, false] {
                    let mut c = base("points", s, Some(q), Some(2), format!("points {} {}", quality_name(Some(q)), if off { "off" } else { "on" }));
                    c.point_off = off;
                    out.push(c);
                }
            }
        }
        // Lane P4-GPU: HIGH shadows alone (the GPU passes at MED and HIGH).
        if plans.iter().any(|p| p == "high") {
            out.push(base("high", s, Some(Quality::High), Some(2), "shadows high".into()));
        }
        // Lane P4-GPU: the render scale (`CLIENT910_MODERN_RENDER_SCALE`
        // through the settings' variable source, so a build without the
        // setting draws its default frame), MED.
        if plans.iter().any(|p| p == "scale") {
            let scale = std::env::var("MODERN_BENCH_SCALE").unwrap_or_else(|_| "0.5".into());
            let mut c = base("scale", s, Some(Quality::Medium), Some(2), format!("render scale {scale}"));
            c.settings = ModernSettings::from_vars(|n| (n == "CLIENT910_MODERN_RENDER_SCALE").then(|| scale.clone()));
            out.push(c);
        }
    }
    if plans.iter().any(|p| p == "ablate") {
        // One quality setting changed at a time (the settings' costs).
        let d = ModernSettings::DEFAULT;
        use crate::settings::{AoMode, EnvReflections, LookMode};
        let list: [(&str, ModernSettings); 6] = [
            ("ao off", ModernSettings { ao: AoMode::Off, ..d }),
            ("ao hbao ultra", ModernSettings { ao: AoMode::HbaoUltra, ..d }),
            ("volumetrics off", ModernSettings { volumetrics: false, ..d }),
            ("env reflections all", ModernSettings { env_reflections: EnvReflections::All, ..d }),
            ("dof on", ModernSettings { dof: true, ..d }),
            ("classic-calibrated look", ModernSettings { look: LookMode::ClassicCalibrated, ..d }),
        ];
        for (label, settings) in list {
            let mut c = base("ablate", aa[0], Some(Quality::Medium), Some(2), label.into());
            c.settings = settings;
            out.push(c);
        }
    }
    // Each configuration with each thread count.
    match bench_threads() {
        None => out,
        Some(list) => out
            .into_iter()
            .flat_map(|c| {
                list.iter().map(move |&n| Config {
                    label: format!("{} t{n}", c.label),
                    threads: Some(n),
                    ..c.clone()
                })
            })
            .collect(),
    }
}

/// `MODERN_BENCH_THREADS`.
fn bench_threads() -> Option<Vec<usize>> {
    let v = std::env::var("MODERN_BENCH_THREADS").ok()?;
    let list: Vec<usize> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    (!list.is_empty()).then_some(list)
}

struct Scene {
    name: &'static str,
    offline: OfflineScene,
    sky: Option<crate::sky_frame::SkyCache>,
}

fn env_number(name: &str, default: i32) -> i32 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn scene(pack: &crate::cache::Pack, name: &'static str, size: [u32; 2]) -> Scene {
    let vp = (size[0] as i32, size[1] as i32);
    let offline = match name {
        "lumbridge" => OfflineScene::new(pack, (3222, 3222), vp),
        "river" => river_scene(pack, size, 2000.0, 1.6),
        "castle" => OfflineScene::with_camera(pack, (3208, 3218), vp, |c| {
            c.pitch = 2400.0;
            c.distance_scale = 0.7;
        }),
        "draynor" => OfflineScene::with_camera(pack, (3093, 3250), vp, |c| {
            c.yaw = 12000.0;
            c.pitch = 1400.0;
            c.distance_scale = 1.2;
        }),
        // The far scene's review view (renderer plan §4(t)): high over
        // Lumbridge towards the river (`--cam-target 3222 3218 --cam-pitch
        // 1700 --cam-yaw 12000 --cam-zoom 6`).
        "aerial" => OfflineScene::with_camera(pack, (3222, 3218), vp, |c| {
            c.yaw = 12000.0;
            c.pitch = 1700.0;
            c.distance_scale = 6.0;
        }),
        // The sky's review views: along the street at eye height, and looking
        // up (`MODERN_BENCH_PITCH` overrides the look-up pitch).
        "street" => OfflineScene::with_camera(pack, (3222, 3222), vp, |c| {
            c.legacy = Some(crate::camera::LegacyFrame {
                eye: [0, -300, 0],
                pitch: env_number("MODERN_BENCH_STREET_PITCH", 15500),
                yaw: 6000,
                roll: 0,
            });
        }),
        "lookup" => OfflineScene::with_camera(pack, (3222, 3222), vp, |c| {
            c.legacy = Some(crate::camera::LegacyFrame {
                eye: [0, -300, 0],
                pitch: env_number("MODERN_BENCH_PITCH", 13500),
                yaw: 6000,
                roll: 0,
            });
        }),
        // The four framings of the look reference (`tools/perf/look_stats.py`
        // compares them with the reference shots): an aerial view over the
        // castle and the river, the castle's front, its cannon deck from
        // above, and a street under the sky.
        "ref-aerial" => OfflineScene::with_camera(pack, (3222, 3218), vp, |c| {
            c.yaw = 2000.0;
            c.pitch = 1900.0;
            c.distance_scale = 3.0;
        }),
        "ref-castle" => OfflineScene::with_camera(pack, (3218, 3226), vp, |c| {
            c.yaw = 4096.0;
            c.pitch = 1300.0;
            c.distance_scale = 1.8;
        }),
        "ref-roof" => OfflineScene::with_camera(pack, (3218, 3222), vp, |c| {
            c.yaw = 0.0;
            c.pitch = 2300.0;
            c.distance_scale = 1.3;
        }),
        "ref-street" => OfflineScene::with_camera(pack, (3218, 3226), vp, |c| {
            c.legacy = Some(crate::camera::LegacyFrame {
                eye: [0, -250, 0],
                pitch: 15700,
                yaw: 8000,
                roll: 0,
            });
        }),
        other => panic!("unknown scene {other}"),
    };
    let sky = offline.sky(pack);
    Scene { name, offline, sky }
}

/// The process's retired instructions and cycles so far, all threads (macOS
/// `proc_pid_rusage`, `rusage_info_v4`): the CPU work of a stretch of frames
/// that other processes' load does not stretch the way it does wall time.
fn cpu_work() -> (u64, u64) {
    extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut std::ffi::c_void) -> i32;
    }
    let mut info = [0u64; 64];
    // SAFETY: the buffer is larger than `rusage_info_v4`.
    let ok = unsafe { proc_pid_rusage(std::process::id() as i32, 4, info.as_mut_ptr().cast()) };
    if ok == 0 {
        (info[31], info[32])
    } else {
        (0, 0)
    }
}

/// This thread's CPU time in milliseconds (macOS `CLOCK_THREAD_CPUTIME_ID`):
/// what a frame's `draw` cost the render thread whatever the load of the
/// machine preempted it for.
fn thread_cpu_ms() -> f64 {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    extern "C" {
        fn clock_gettime(clock: u32, ts: *mut Timespec) -> i32;
    }
    let mut ts = Timespec { sec: 0, nsec: 0 };
    // SAFETY: `ts` is a valid `timespec`; 16 is `CLOCK_THREAD_CPUTIME_ID`.
    if unsafe { clock_gettime(16, &mut ts) } == 0 {
        ts.sec as f64 * 1e3 + ts.nsec as f64 / 1e6
    } else {
        0.0
    }
}

fn load_average() -> String {
    std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_matches(|c| c == '{' || c == '}').trim().to_string())
        .unwrap_or_default()
}

fn rss_mb() -> f64 {
    let pid = std::process::id().to_string();
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

fn metal_mb(device: &wgpu::Device) -> f64 {
    rs910_gpu_device::gpu_device::allocated_bytes(device)
        .map_or(0.0, |bytes| bytes as f64 / 1_048_576.0)
}

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("no adapter");
    let features = adapter.features()
        & (wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES
            | wgpu::Features::TIMESTAMP_QUERY
            | crate::models::materials::optional_device_features()
            | wgpu::Features::INDIRECT_FIRST_INSTANCE
            | wgpu::Features::MULTI_DRAW_INDIRECT_COUNT);
    let features = if adapter.get_downlevel_capabilities().flags
        .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION) {
        features
    } else {
        features - (wgpu::Features::INDIRECT_FIRST_INSTANCE | wgpu::Features::MULTI_DRAW_INDIRECT_COUNT)
    };
    let features = if std::env::var_os("MODERN_BENCH_NO_TS").is_some() {
        features - wgpu::Features::TIMESTAMP_QUERY
    } else {
        features
    };
    pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("modern perf bench"),
            required_features: features,
            required_limits: rs910_gpu_device::gpu_device::renderer_limits(&adapter),
            memory_hints: Default::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            trace: wgpu::Trace::Off,

        }))
    .expect("device")
}

fn pct(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s[((s.len() - 1) as f64 * q).round() as usize]
}

struct Frame {
    draw: f64,
    /// The render thread's CPU time in `draw` ([`thread_cpu_ms`]).
    draw_cpu: f64,
    finish: f64,
    submit: f64,
    idle: f64,
    wall: f64,
}

fn frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    r: &mut ModernRenderer,
    snapshot: &SceneSnapshot<'_>,
    view: &wgpu::TextureView,
    size: [u32; 2],
) -> Frame {
    let mut encoder = device.create_command_encoder(&Default::default());
    let [w, h] = size.map(|v| v as i32);
    let t0 = Instant::now();
    let cpu0 = thread_cpu_ms();
    r.draw(
        Target {
            device,
            queue,
            encoder: &mut encoder,
            view,
            format: wgpu::TextureFormat::Rgba8Unorm,
            size,
            rect: [0, 0, w, h],
            clip: [0, 0, w, h],
        },
        snapshot,
    );
    let t1 = Instant::now();
    let draw_cpu = thread_cpu_ms() - cpu0;
    let cb = encoder.finish();
    let t2 = Instant::now();
    queue.submit(Some(cb));
    let t3 = Instant::now();
    hook::after_submit(device, true);
    let t4 = Instant::now();
    let ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1000.0;
    Frame {
        draw: ms(t0, t1),
        draw_cpu,
        finish: ms(t1, t2),
        submit: ms(t2, t3),
        idle: ms(t3, t4),
        wall: ms(t0, t4),
    }
}

struct Out {
    summary: std::fs::File,
    frames: std::fs::File,
    phases: std::fs::File,
    passes: std::fs::File,
}

/// The configuration's settings: the renderer (the far scene built in
/// full before each frame) and the shell's shadow options.
fn renderer(device: &wgpu::Device, queue: &wgpu::Queue, cfg: &Config) -> ModernRenderer {
    let settings = ModernSettings {
        far: cfg.far.and_then(rs910_far_scene::far_level::FarLevel::new),
        ..cfg.settings
    };
    let mut r = ModernRenderer::new(device, queue, wgpu::TextureFormat::Rgba8Unorm, cfg.samples, settings);
    // `MODERN_BENCH_FAR_ASYNC=1`: the client's streaming (the first frames'
    // cost); otherwise the whole ring before the first frame.
    r.far.sync = std::env::var_os("MODERN_BENCH_FAR_ASYNC").is_none();
    if let Some(n) = cfg.threads {
        super::perf_samples::set_threads(&mut r, n);
    }
    r
}

fn configure(r: &mut ModernRenderer, cfg: &Config) {
    r.shadow.point.test_off = cfg.point_off;
    r.set_shadow_settings(match cfg.shadow {
        None => crate::shadows::Settings::from_options(0, 0, 0),
        Some(q) => crate::shadows::Settings::from_options(2, q as i32, 1),
    });
}

#[allow(clippy::too_many_lines)]
fn run(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pack: &crate::cache::Pack,
    sc: &Scene,
    size: [u32; 2],
    cfg: &Config,
    frames: usize,
    out: &mut Out,
    id: usize,
) -> ModernRenderer {
    let _ = pack;
    let load = load_average();
    let mut snapshot = sc.offline.snapshot(pack);
    snapshot.sky = sc.sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
    let mut clock = 1_700_000_000_000_i64;
    crate::logic_clock::set_test_now(Some(clock));
    // The motion plan's camera: the target moves `step` units along x each
    // warm and measured frame.
    let step: i32 = if cfg.plan == "motion" {
        std::env::var("MODERN_BENCH_MOTION").ok().and_then(|v| v.parse().ok()).unwrap_or(12)
    } else {
        0
    };
    let target0 = snapshot.camera.target;
    let mut moved = 0;
    hook::reset(device);
    let c0 = hook::counts();
    let t = Instant::now();
    let mut r = renderer(device, queue, cfg);
    let new_ms = t.elapsed().as_secs_f64() * 1000.0;
    let c1 = hook::counts();
    let new_pipes = c1[hook::K::CreatePipeline as usize] - c0[hook::K::CreatePipeline as usize];
    let new_compile_ms = (c1[hook::K::ShaderPipelineNs as usize] - c0[hook::K::ShaderPipelineNs as usize]) as f64 / 1e6;
    configure(&mut r, cfg);
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench frame"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    // The first frame (lazy pipelines, far ring build, uploads), then the
    // probe capture's frames until it settles.
    let first = frame(device, queue, &mut r, &snapshot, &view, size);
    let recs = hook::take_records();
    let first_pipes = recs.iter().map(|x| x.counts[hook::K::CreatePipeline as usize]).sum::<u64>();
    let first_compile = recs.iter().map(|x| x.counts[hook::K::ShaderPipelineNs as usize]).sum::<u64>() as f64 / 1e6;
    let (mut settle, mut settle_max, mut settle_total) = (0, 0.0_f64, 0.0);
    let mut settle_draw_max = 0.0_f64;
    // `MODERN_BENCH_FAR_TRACE=1`: each early frame's `draw` and the far
    // scene's own render-thread time (the streaming's first frames).
    let trace = std::env::var_os("MODERN_BENCH_FAR_TRACE").is_some();
    let mut early = 0;
    for _ in 0..80 {
        if !r.probe_capture_pending() {
            break;
        }
        clock += 16;
        crate::logic_clock::set_test_now(Some(clock));
        let f = frame(device, queue, &mut r, &snapshot, &view, size);
        early += 1;
        if trace {
            let s = r.far_stats();
            eprintln!("[{id}] early frame {early}: draw {:.2} far {:.2} ms {s:?}", f.draw, s.build_ms);
        }
        settle += 1;
        settle_max = settle_max.max(f.wall);
        settle_draw_max = settle_draw_max.max(f.draw);
        settle_total += f.wall;
    }
    for _ in 0..10 {
        clock += 16;
        crate::logic_clock::set_test_now(Some(clock));
        moved += step;
        snapshot.camera.target[0] = target0[0] + moved;
        let f = frame(device, queue, &mut r, &snapshot, &view, size);
        early += 1;
        if trace {
            let s = r.far_stats();
            eprintln!("[{id}] early frame {early}: draw {:.2} far {:.2} ms {s:?}", f.draw, s.build_ms);
        }
    }
    let _ = hook::take_records();
    hook::reset_pass_counts();
    let work0 = cpu_work();
    let mut fs = Vec::new();
    let mut shadow_draws = Vec::new();
    for _ in 0..frames {
        clock += 16;
        crate::logic_clock::set_test_now(Some(clock));
        moved += step;
        snapshot.camera.target[0] = target0[0] + moved;
        fs.push(frame(device, queue, &mut r, &snapshot, &view, size));
        shadow_draws.push(r.stats.shadow_draws as f64);
    }
    let work1 = cpu_work();
    let recs = hook::take_records();
    hook::print_pass_counts(frames as u64);
    eprintln!("[{id}] far stats {:?}", r.far_stats());
    eprintln!("[{id}] materials {} {:?} (with a layer, own textures) {:?}", r.textures.len(), r.textures.stats, r.textures.layer_counts());
    let metal = metal_mb(device);
    let rss = rss_mb();
    let col = |f: fn(&Frame) -> f64| fs.iter().map(f).collect::<Vec<f64>>();
    let (draw, finish, submit, idle, wall) = (
        col(|f| f.draw),
        col(|f| f.finish),
        col(|f| f.submit),
        col(|f| f.idle),
        col(|f| f.wall),
    );
    let span: Vec<f64> = recs.iter().map(|r| r.gpu_span_ms).collect();
    let gsum: Vec<f64> = recs.iter().map(|r| r.gpu.iter().map(|p| p.1).sum()).collect();
    let mean_count = |k: hook::K| recs.iter().map(|r| r.counts[k as usize] as f64).sum::<f64>() / recs.len().max(1) as f64;
    let key = format!(
        "{id}\t{}\t{}x{}\t{}\t{}\t{}\t{}\t{}",
        sc.name,
        size[0],
        size[1],
        cfg.samples,
        quality_name(cfg.shadow),
        cfg.far.map_or("off".into(), |v| v.to_string()),
        cfg.plan,
        cfg.label
    );
    let threads = super::perf_samples::set_threads(&mut r, 0);
    let s = &r.stats;
    let f3 = |v: f64| format!("{v:.3}");
    let f1 = |v: f64| format!("{v:.1}");
    let vals: Vec<String> = vec![
        f3(pct(&draw, 0.5)),
        f3(pct(&draw, 0.99)),
        f3(pct(&finish, 0.5)),
        f3(pct(&submit, 0.5)),
        f3(pct(&idle, 0.5)),
        f3(pct(&wall, 0.5)),
        f3(pct(&wall, 0.99)),
        f3(wall.iter().copied().fold(0.0, f64::max)),
        f3(pct(&span, 0.5)),
        f3(pct(&span, 0.99)),
        f3(pct(&gsum, 0.5)),
        f3(pct(&draw, 0.5) + pct(&finish, 0.5) + pct(&submit, 0.5)),
        f3(pct(&draw, 0.0)),
        f3(pct(&wall, 0.0)),
        f1(mean_count(hook::K::Draws)),
        f1(mean_count(hook::K::Passes)),
        f1(mean_count(hook::K::ComputePasses)),
        f1(mean_count(hook::K::SetPipeline)),
        f1(mean_count(hook::K::SetPipelineSame)),
        f1(mean_count(hook::K::SetBindGroup)),
        f1(mean_count(hook::K::SetBindGroupSame)),
        f1(mean_count(hook::K::SetVertexBuffer)),
        f1(mean_count(hook::K::SetVertexBufferSame)),
        f1(mean_count(hook::K::WriteBuffer)),
        f1(mean_count(hook::K::WriteBufferBytes)),
        f1(mean_count(hook::K::CreateBuffer) + mean_count(hook::K::CreateTexture)),
        f1(mean_count(hook::K::CreateBindGroup)),
        f1(mean_count(hook::K::Allocs)),
        f1(mean_count(hook::K::AllocBytes)),
        f1(mean_count(hook::K::Indices) / 1000.0),
        f1(mean_count(hook::K::Instances)),
        f1(new_ms),
        new_pipes.to_string(),
        f1(new_compile_ms),
        first_pipes.to_string(),
        f1(first_compile),
        f1(first.draw),
        f1(first.wall),
        settle.to_string(),
        f1(settle_max),
        f1(settle_draw_max),
        f1(settle_total),
        f1(metal),
        f1(rss),
        s.draws.to_string(),
        s.shadow_cascades.to_string(),
        format!("{:.0}", shadow_draws.iter().sum::<f64>() / shadow_draws.len().max(1) as f64),
        (s.opaque + s.transparent).to_string(),
        s.statics.to_string(),
        s.materials.to_string(),
        threads.to_string(),
        f1((work1.1 - work0.1) as f64 / frames.max(1) as f64 / 1e6),
        f1((work1.0 - work0.0) as f64 / frames.max(1) as f64 / 1e6),
    ];
    let line = format!("{key}\t{load}\t{}", vals.join("\t"));
    let _ = writeln!(out.summary, "{line}");
    eprintln!(
        "[{id}] {} {}x{} {}x {} far {} {} ({} threads): cpu {:.1} Mcycles {:.1} Minstr per frame | draw {:.2} (p99 {:.2}) finish {:.2} submit {:.2} | idle {:.2} wall {:.2} (p99 {:.2}) | gpu span {:.2} sum {:.2} | draws {:.0} passes {:.0} allocs {:.0} | load {load}",
        sc.name,
        size[0],
        size[1],
        cfg.samples,
        quality_name(cfg.shadow),
        cfg.far.map_or("off".into(), |v| v.to_string()),
        cfg.label,
        threads,
        (work1.1 - work0.1) as f64 / frames.max(1) as f64 / 1e6,
        (work1.0 - work0.0) as f64 / frames.max(1) as f64 / 1e6,
        pct(&draw, 0.5),
        pct(&draw, 0.99),
        pct(&finish, 0.5),
        pct(&submit, 0.5),
        pct(&idle, 0.5),
        pct(&wall, 0.5),
        pct(&wall, 0.99),
        pct(&span, 0.5),
        pct(&gsum, 0.5),
        mean_count(hook::K::Draws),
        mean_count(hook::K::Passes),
        mean_count(hook::K::Allocs),
    );
    for (i, f) in fs.iter().enumerate() {
        let rec = recs.get(i);
        let _ = writeln!(
            out.frames,
            "{id}\t{i}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{}",
            f.draw,
            f.finish,
            f.submit,
            f.idle,
            f.wall,
            rec.map_or(0.0, |r| r.gpu_span_ms),
            rec.map_or(String::new(), |r| r.counts.iter().map(u64::to_string).collect::<Vec<_>>().join("\t"))
        );
    }
    // Per phase and per pass: the median over frames (a pass name repeated in
    // a frame is summed within it).
    let mut names: Vec<&'static str> = Vec::new();
    for r in &recs {
        for (n, _) in &r.phases {
            if !names.contains(n) {
                names.push(n);
            }
        }
    }
    for n in &names {
        let v: Vec<f64> = recs.iter().map(|r| r.phases.iter().filter(|p| p.0 == *n).map(|p| p.1).sum()).collect();
        let _ = writeln!(out.phases, "{id}\t{n}\t{:.4}\t{:.4}", pct(&v, 0.5), pct(&v, 0.99));
    }
    let mut pnames: Vec<&'static str> = Vec::new();
    for r in &recs {
        for (n, _) in &r.gpu {
            if !pnames.contains(n) {
                pnames.push(n);
            }
        }
    }
    // Incremental end: a pass's end past every earlier pass's end (passes
    // overlap on this GPU, so begin-to-end durations over-count; the
    // increments sum to the frame's span).
    let inc = |r: &hook::Record| -> Vec<f64> {
        let mut last = 0.0_f64;
        r.gpu_at
            .iter()
            .map(|&(_, e)| {
                let d = (e - last).max(0.0);
                last = last.max(e);
                d
            })
            .collect()
    };
    for n in &pnames {
        let v: Vec<f64> = recs.iter().map(|r| r.gpu.iter().filter(|p| p.0 == *n).map(|p| p.1).sum()).collect();
        let w: Vec<f64> = recs
            .iter()
            .map(|r| r.gpu.iter().zip(inc(r)).filter(|(p, _)| p.0 == *n).map(|(_, d)| d).sum())
            .collect();
        let b: Vec<f64> = recs
            .iter()
            .filter_map(|r| r.gpu.iter().zip(&r.gpu_at).find(|(p, _)| p.0 == *n).map(|(_, a)| a.0))
            .collect();
        let k = recs.first().map_or(0, |r| r.gpu.iter().filter(|p| p.0 == *n).count());
        let _ = writeln!(
            out.passes,
            "{id}\t{n}\t{k}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}",
            pct(&v, 0.5),
            pct(&v, 0.99),
            pct(&w, 0.5),
            pct(&w, 0.99),
            pct(&b, 0.5)
        );
    }
    for f in [&mut out.summary, &mut out.frames, &mut out.phases, &mut out.passes] {
        let _ = f.flush();
    }
    r
}

/// The matrix (timing; not a pass/fail test). See the module docs.
#[test]
#[ignore = "timing bench; needs a GPU and server/data/pack"]
fn modern_matrix() {
    let Some(dir) = std::env::var_os("MODERN_BENCH_OUT").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let list = |var: &str, default: &str| -> Vec<String> {
        std::env::var(var)
            .unwrap_or_else(|_| default.into())
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let plans = list("MODERN_BENCH_PLAN", "base");
    let scenes = list("MODERN_BENCH_SCENES", "lumbridge,river,castle,draynor");
    let sizes: Vec<[u32; 2]> = list("MODERN_BENCH_SIZES", "1600x1000,2560x1440")
        .iter()
        .map(|s| {
            let (w, h) = s.split_once('x').unwrap();
            [w.parse().unwrap(), h.parse().unwrap()]
        })
        .collect();
    let aa: Vec<u32> = list("MODERN_BENCH_AA", "4,1").iter().map(|s| s.parse().unwrap()).collect();
    let frames: usize = std::env::var("MODERN_BENCH_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let hold: f64 = std::env::var("MODERN_BENCH_HOLD_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let tag = std::env::var("MODERN_BENCH_TAG").unwrap_or_else(|_| "run".into());
    let open = |name: &str, header: &str| {
        let p = dir.join(format!("{tag}-{name}.tsv"));
        let new = !p.exists();
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p).unwrap();
        if new {
            writeln!(f, "{header}").unwrap();
        }
        f
    };
    let key = "id\tscene\tsize\taa\tshadows\tfar\tplan\tlabel";
    let mut out = Out {
        summary: open(
            "summary",
            &format!("{key}\tload\tdraw_p50\tdraw_p99\tfinish_p50\tsubmit_p50\tidle_p50\twall_p50\twall_p99\twall_max\tgpu_span_p50\tgpu_span_p99\tgpu_sum_p50\tcpu_total_p50\tdraw_min\twall_min\tdraws\tpasses\tcompute_passes\tset_pipeline\tset_pipeline_same\tset_bind_group\tset_bind_group_same\tset_vb\tset_vb_same\twrite_buffer\twrite_buffer_bytes\tcreates\tcreate_bind_group\tallocs\talloc_bytes\tkindices\tinstances\tnew_ms\tnew_pipelines\tnew_compile_ms\tfirst_pipelines\tfirst_compile_ms\tfirst_draw_ms\tfirst_wall_ms\tsettle_frames\tsettle_max_ms\tsettle_draw_max_ms\tsettle_total_ms\tmetal_mb\trss_mb\tstat_draws\tcascades\tshadow_draws\tentities\tstatics\tmaterials\tthreads\tmcycles\tminstr"),
        ),
        frames: open("frames", &format!("id\tframe\tdraw\tfinish\tsubmit\tidle\twall\tgpu_span\t{}", hook::NAMES.join("\t"))),
        phases: open("phases", "id\tphase\tp50\tp99"),
        passes: open("passes", "id\tpass\tcount\tdur_p50\tdur_p99\tinc_p50\tinc_p99\tbegin_p50"),
    };
    let _reset = super::tests::ClockReset;
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = device();
    eprintln!(
        "modern bench pid {} features {:?} plans {plans:?}",
        std::process::id(),
        device.features()
    );
    let cfgs = configs(&plans, &aa);
    let mut id = std::env::var("MODERN_BENCH_ID0").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut held = false;
    for size in &sizes {
        for name in &scenes {
            let name: &'static str = Box::leak(name.clone().into_boxed_str());
            let t = Instant::now();
            let sc = scene(&pack, name, *size);
            eprintln!("scene {name} {size:?} built in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
            for cfg in &cfgs {
                let mut r = run(&device, &queue, &pack, &sc, *size, cfg, frames, &mut out, id);
                id += 1;
                if hold > 0.0 && !held {
                    held = true;
                    eprintln!("HOLD {hold} s (pid {})", std::process::id());
                    let output = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("bench hold"),
                        size: wgpu::Extent3d {
                            width: size[0],
                            height: size[1],
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    });
                    let view = output.create_view(&Default::default());
                    let mut snapshot = sc.offline.snapshot(&pack);
                    snapshot.sky = sc.sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
                    let t = Instant::now();
                    let mut n = 0;
                    while t.elapsed().as_secs_f64() < hold {
                        frame(&device, &queue, &mut r, &snapshot, &view, *size);
                        n += 1;
                    }
                    let _ = hook::take_records();
                    eprintln!("HOLD done: {n} frames");
                }
            }
        }
    }
}

/// The settings combinations of [`modern_settings_views`]: each changes one
/// setting from the default (MED shadows, far 2, 4x) and names its frames.
fn settings_combinations() -> Vec<(&'static str, Config)> {
    use crate::settings::{AoMode, EnvReflections, LookMode};
    let d = ModernSettings::DEFAULT;
    let threads = bench_threads().map(|l| l[0]);
    let c = |samples, shadow, far, settings| Config {
        plan: "views",
        label: "views".into(),
        samples,
        shadow,
        far,
        settings,
        threads,
        point_off: false,
    };
    let med = Some(Quality::Medium);
    vec![
        ("default", c(4, med, Some(2), d)),
        ("1x", c(1, med, Some(2), d)),
        ("shadows-off", c(4, None, Some(2), d)),
        ("shadows-low", c(4, Some(Quality::Low), Some(2), d)),
        ("shadows-high", c(4, Some(Quality::High), Some(2), d)),
        ("shadows-ultra", c(4, Some(Quality::Ultra), Some(2), d)),
        ("shadows-ultraplus", c(4, Some(Quality::UltraPlus), Some(2), d)),
        ("ao-off", c(4, med, Some(2), ModernSettings { ao: AoMode::Off, ..d })),
        ("ao-ssao", c(4, med, Some(2), ModernSettings { ao: AoMode::Ssao, ..d })),
        ("ao-hbao-ultra", c(4, med, Some(2), ModernSettings { ao: AoMode::HbaoUltra, ..d })),
        ("point-shadows-off", Config { point_off: true, ..c(4, med, Some(2), d) }),
        ("env-all", c(4, med, Some(2), ModernSettings { env_reflections: EnvReflections::All, ..d })),
        ("far-off", c(4, med, None, d)),
        ("far-4", c(4, med, Some(4), d)),
        ("volumetrics-off", c(4, med, Some(2), ModernSettings { volumetrics: false, ..d })),
        ("dof", c(4, med, Some(2), ModernSettings { dof: true, ..d })),
        ("bloom", c(4, med, Some(2), ModernSettings { bloom: Some(true), ..d })),
        ("classic-look", c(4, med, Some(2), ModernSettings { look: LookMode::ClassicCalibrated, ..d })),
        // Lane P4-GPU: the water reflection's setting and the render scale
        // through the variable source (a build without them draws the default).
        ("1x-hbao-ultra", c(1, med, Some(2), ModernSettings { ao: AoMode::HbaoUltra, ..d })),
        ("1x-volumetrics-off", c(1, med, Some(2), ModernSettings { volumetrics: false, ..d })),
        ("render-scale-50", c(4, med, Some(2), ModernSettings::from_vars(|n| (n == "CLIENT910_MODERN_RENDER_SCALE").then(|| "0.5".into())))),
        ("1x-render-scale-50", c(1, med, Some(2), ModernSettings::from_vars(|n| (n == "CLIENT910_MODERN_RENDER_SCALE").then(|| "0.5".into())))),
        ("render-scale-150", c(4, med, Some(2), ModernSettings::from_vars(|n| (n == "CLIENT910_MODERN_RENDER_SCALE").then(|| "1.5".into())))),
        ("reflections-half", c(4, med, Some(2), ModernSettings::from_vars(|n| (n == "CLIENT910_MODERN_REFLECTIONS").then(|| "half".into())))),
        ("reflections-off", c(4, med, Some(2), ModernSettings::from_vars(|n| (n == "CLIENT910_MODERN_REFLECTIONS").then(|| "off".into())))),
    ]
}

/// One settled frame per scene and settings combination
/// ([`settings_combinations`]; `MODERN_VIEWS_COMBOS` filters by name,
/// `MODERN_VIEWS_NO_VOLUMETRICS=1` turns the volumetrics off in each) at
/// 800x500, each from a fresh renderer at a fixed clock: raw RGBA frames
/// `MODERN_BENCH_OUT/TAG-SCENE-COMBO.rgba` for byte comparisons between two
/// builds (`tools/perf/modern_views_diff.py`).
#[test]
#[ignore = "views; needs a GPU and server/data/pack"]
fn modern_settings_views() {
    let Some(dir) = std::env::var_os("MODERN_BENCH_OUT").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let tag = std::env::var("MODERN_BENCH_TAG").unwrap_or_else(|_| "views".into());
    let scenes: Vec<String> = std::env::var("MODERN_BENCH_SCENES")
        .unwrap_or_else(|_| "lumbridge,river,castle,draynor".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let only: Option<Vec<String>> = std::env::var("MODERN_VIEWS_COMBOS")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect());
    let size = [800_u32, 500];
    let _reset = super::tests::ClockReset;
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = device();
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("views frame"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    for name in &scenes {
        let name: &'static str = Box::leak(name.clone().into_boxed_str());
        let sc = scene(&pack, name, size);
        for (combo, mut cfg) in settings_combinations() {
            if only.as_ref().is_some_and(|o| !o.iter().any(|n| n == combo)) {
                continue;
            }
            // `MODERN_VIEWS_NO_VOLUMETRICS=1`: every combination without the
            // volumetrics (lane P4-GPU: compares the rest of two builds whose
            // volumetrics differ on purpose).
            if std::env::var_os("MODERN_VIEWS_NO_VOLUMETRICS").is_some() {
                cfg.settings.volumetrics = false;
            }
            let mut clock = 1_700_000_000_000_i64;
            crate::logic_clock::set_test_now(Some(clock));
            let mut r = renderer(&device, &queue, &cfg);
            configure(&mut r, &cfg);
            let mut snapshot = sc.offline.snapshot(&pack);
            snapshot.sky = sc.sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
            // The captured ambient (the verified look) needs a face a frame
            // for every square and its blend: more frames, a longer step.
            let ambient = cfg.settings.look.captured_ambient();
            let (cap, step) = if ambient { (400, 40) } else { (80, 16) };
            let mut n = 0;
            loop {
                frame(&device, &queue, &mut r, &snapshot, &view, size);
                clock += step;
                crate::logic_clock::set_test_now(Some(clock));
                n += 1;
                if !r.probe_capture_pending() || n >= cap {
                    break;
                }
            }
            for _ in 0..3 {
                frame(&device, &queue, &mut r, &snapshot, &view, size);
                clock += 16;
                crate::logic_clock::set_test_now(Some(clock));
            }
            let px = super::tests::read_back(&device, &queue, &output, 4);
            std::fs::write(dir.join(format!("{tag}-{name}-{combo}.rgba")), &px).unwrap();
            let _ = hook::take_records();
            eprintln!("{name} {combo}: {} frames, draws {}", n + 3, r.stats.draws);
        }
    }
}

/// The offline views for byte comparisons between two builds and
/// the anti-aliasing change's cost. Per scene at 800x500 (MED, far 2): the
/// settled frame of a fresh 4x and a fresh 1x renderer, the 1x renderer's
/// frame after 12 frames of animated locs (`OfflineScene::animate`), and a
/// 4x renderer changed to 1x (`perf_samples::change_samples`: this build's
/// client path) and back, timed. Raw RGBA frames: `MODERN_BENCH_OUT/TAG-SCENE-*.rgba`;
/// timings: `TAG-aa.tsv`, capture frame IDs and clocks: `TAG-manifest.tsv`.
#[test]
#[ignore = "views and timing; needs a GPU and server/data/pack"]
fn modern_views() {
    const VIEW_CLOCK_START_MS: i64 = 1_700_000_000_000;
    const VIEW_STEP_MS: i64 = 16;
    const VIEW_WARM_FRAMES: usize = 400;
    const VIEW_HOLD_FRAMES: usize = 3;
    const VIEW_FRESH_FRAMES: usize = VIEW_WARM_FRAMES + VIEW_HOLD_FRAMES;
    const VIEW_ANIMATION_FRAMES: i32 = 12;
    const VIEW_ANIMATION_CYCLE_STEP: i32 = 5;
    const VIEW_CHANGE_FIRST_FRAME: usize = 1;
    const RGBA_CHANNELS: u32 = 4;
    const STANDARD_CAPTURE_COUNT: usize = 5;
    let Some(dir) = std::env::var_os("MODERN_BENCH_OUT").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let tag = std::env::var("MODERN_BENCH_TAG").unwrap_or_else(|_| "views".into());
    let scenes: Vec<String> = std::env::var("MODERN_BENCH_SCENES")
        .unwrap_or_else(|_| "lumbridge,river,castle,draynor".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    // `MODERN_VIEWS_SIZE=WxH` overrides the size (default 800x500).
    let size = std::env::var("MODERN_VIEWS_SIZE")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some([w.parse().ok()?, h.parse().ok()?])
        })
        .unwrap_or([800_u32, 500]);
    let _reset = super::tests::ClockReset;
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = device();
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("views frame"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    let mut aa = std::fs::File::create(dir.join(format!("{tag}-aa.tsv"))).unwrap();
    writeln!(aa, "scene\tchange\tkept\tchange_ms\tpipelines\tcompile_ms\tfirst_ms\tsettle_frames\tsettle_ms\tlog_diff_vs_fresh").unwrap();
    let capture_all = std::env::var_os("MODERN_VIEWS_ALL").is_some();
    let mut captures_per_scene = STANDARD_CAPTURE_COUNT;
    if capture_all {
        captures_per_scene += VIEW_ANIMATION_FRAMES as usize;
    }
    let expected_captures = scenes.len() * captures_per_scene;
    let mut manifest = std::fs::File::create(dir.join(format!("{tag}-manifest.tsv"))).unwrap();
    writeln!(
        manifest,
        "frame\twidth\theight\tframe_id\tclock_ms\tpending"
    )
    .unwrap();
    for name in &scenes {
        let name: &'static str = Box::leak(name.clone().into_boxed_str());
        // Dynamic loc construction reads the clock, including on later scenes.
        let _scene_clock = super::tests::fixed_clock();
        let mut sc = scene(&pack, name, size);
        let cfg = |samples| Config {
            plan: "views",
            label: "views".into(),
            samples,
            shadow: Some(Quality::Medium),
            far: Some(2),
            settings: ModernSettings::DEFAULT,
            threads: bench_threads().map(|l| l[0]),
            point_off: false,
        };
        let mut clock = VIEW_CLOCK_START_MS;
        let mut step = |clock: &mut i64| {
            *clock += VIEW_STEP_MS;
            crate::logic_clock::set_test_now(Some(*clock));
        };
        let mut write = |label: &str, frame_id: usize, clock: i64, pending: bool| {
            assert!(
                !pending,
                "capture {name}-{label} has unfinished ambient work"
            );
            let px = super::tests::read_back(&device, &queue, &output, RGBA_CHANNELS);
            let frame_name = format!("{name}-{label}.rgba");
            std::fs::write(dir.join(format!("{tag}-{frame_name}")), &px).unwrap();
            // `step` has installed the next clock; the completed draw used the previous one.
            let capture_clock = clock - VIEW_STEP_MS;
            writeln!(
                manifest,
                "{frame_name}\t{}\t{}\t{frame_id}\t{capture_clock}\t{pending}",
                size[0], size[1]
            )
            .unwrap();
            eprintln!(
                "CAPTURE {name}-{label} frame={frame_id} clock={capture_clock} pending={pending}"
            );
            px
        };
        let pending = |r: &ModernRenderer, clock: i64| {
            r.probe_capture_pending() || r.ambient_pending(clock - VIEW_STEP_MS)
        };
        // Fresh renderers, settled at a fixed clock.
        let mut fresh = |samples: u32, clock: &mut i64| {
            *clock = VIEW_CLOCK_START_MS;
            crate::logic_clock::set_test_now(Some(*clock));
            let mut r = renderer(&device, &queue, &cfg(samples));
            configure(&mut r, &cfg(samples));
            let mut snapshot = sc.offline.snapshot(&pack);
            snapshot.sky = sc.sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
            for _ in 0..VIEW_WARM_FRAMES {
                frame(&device, &queue, &mut r, &snapshot, &view, size);
                step(clock);
            }
            for _ in 0..VIEW_HOLD_FRAMES {
                frame(&device, &queue, &mut r, &snapshot, &view, size);
                step(clock);
            }
            r
        };
        let mut r4 = fresh(4, &mut clock);
        let px4 = write("4x", VIEW_FRESH_FRAMES, clock, pending(&r4, clock));
        let mut r4_frames = VIEW_FRESH_FRAMES;
        let mut r1 = fresh(1, &mut clock);
        let px1 = write("1x", VIEW_FRESH_FRAMES, clock, pending(&r1, clock));
        // Animated locs: 12 posed frames of the 1x renderer.
        for k in 1..=VIEW_ANIMATION_FRAMES {
            sc.offline.animate(VIEW_ANIMATION_CYCLE_STEP * k);
            let mut snapshot = sc.offline.snapshot(&pack);
            snapshot.sky = sc.sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
            frame(&device, &queue, &mut r1, &snapshot, &view, size);
            step(&mut clock);
            if capture_all {
                write(
                    &format!("anim{k}"),
                    VIEW_FRESH_FRAMES + k as usize,
                    clock,
                    pending(&r1, clock),
                );
            }
        }
        write(
            "anim",
            VIEW_FRESH_FRAMES + VIEW_ANIMATION_FRAMES as usize,
            clock,
            pending(&r1, clock),
        );
        drop(r1);
        // The anti-aliasing change of this build (4x -> 1x -> 4x).
        for (to, reference) in [(1_u32, &px1), (4, &px4)] {
            let mut snapshot = sc.offline.snapshot(&pack);
            snapshot.sky = sc.sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
            let c0 = hook::counts();
            let t = Instant::now();
            let kept = super::perf_samples::change_samples(&mut r4, &device, &queue, to);
            if !kept {
                configure(&mut r4, &cfg(to));
            }
            let change_ms = t.elapsed().as_secs_f64() * 1000.0;
            let first = frame(&device, &queue, &mut r4, &snapshot, &view, size);
            step(&mut clock);
            let c1 = hook::counts();
            let pipes = c1[hook::K::CreatePipeline as usize] - c0[hook::K::CreatePipeline as usize];
            let compile = (c1[hook::K::ShaderPipelineNs as usize]
                - c0[hook::K::ShaderPipelineNs as usize]) as f64
                / 1e6;
            let (mut n, mut settle_ms) = (0, 0.0);
            while n < VIEW_WARM_FRAMES {
                let f = frame(&device, &queue, &mut r4, &snapshot, &view, size);
                step(&mut clock);
                n += 1;
                settle_ms += f.wall;
            }
            for _ in 0..VIEW_HOLD_FRAMES {
                frame(&device, &queue, &mut r4, &snapshot, &view, size);
                step(&mut clock);
            }
            r4_frames += VIEW_CHANGE_FIRST_FRAME + n + VIEW_HOLD_FRAMES;
            let px = write(
                &format!("to{to}x"),
                r4_frames,
                clock,
                pending(&r4, clock),
            );
            let diff = px
                .iter()
                .zip(reference.iter())
                .filter(|(a, b)| a != b)
                .count();
            let _ = hook::take_records();
            writeln!(
                aa,
                "{name}\t->{to}x\t{kept}\t{change_ms:.1}\t{pipes}\t{compile:.1}\t{:.1}\t{n}\t{settle_ms:.1}\t{diff}",
                first.wall
            )
            .unwrap();
            eprintln!("{name} ->{to}x kept {kept}: change {change_ms:.1} ms, {pipes} pipelines ({compile:.1} ms), first frame {:.1} ms, settle {n} frames {settle_ms:.1} ms; {diff} bytes differ from a fresh renderer's frame", first.wall);
        }
    }
    writeln!(manifest, "complete\t{expected_captures}").unwrap();
}

/// The per-square ambient capture's cost (the verified look, per scene at
/// 800x500, 4x, far 2): frames with a fixed clock step until the ambient has
/// settled, then 30 more. `TAG-ambient-SCENE.tsv` holds per frame the faces
/// captured, the CPU `draw`, the wall time, the GPU span and the ambient
/// capture pass's GPU time; the log the medians of the capture frames and of
/// the settled ones.
#[test]
#[ignore = "timing; needs a GPU and server/data/pack"]
fn modern_ambient_cost() {
    let Some(dir) = std::env::var_os("MODERN_BENCH_OUT").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let tag = std::env::var("MODERN_BENCH_TAG").unwrap_or_else(|_| "ambient".into());
    let scenes: Vec<String> = std::env::var("MODERN_BENCH_SCENES")
        .unwrap_or_else(|_| "lumbridge,river,castle,draynor".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let size = [800_u32, 500];
    let _reset = super::tests::ClockReset;
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = device();
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ambient frame"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    for name in &scenes {
        let name: &'static str = Box::leak(name.clone().into_boxed_str());
        let sc = scene(&pack, name, size);
        let cfg = Config {
            plan: "ambient",
            label: "verified".into(),
            samples: 4,
            shadow: Some(Quality::Medium),
            far: Some(2),
            settings: ModernSettings {
                look: crate::settings::LookMode::Verified,
                ..ModernSettings::DEFAULT
            },
            threads: bench_threads().map(|l| l[0]),
            point_off: false,
        };
        let mut r = renderer(&device, &queue, &cfg);
        configure(&mut r, &cfg);
        let mut snapshot = sc.offline.snapshot(&pack);
        snapshot.sky = sc.sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
        let mut tsv = std::fs::File::create(dir.join(format!("{tag}-ambient-{name}.tsv"))).unwrap();
        writeln!(tsv, "frame\tfaces\tdraw_ms\twall_ms\tgpu_span_ms\tambient_gpu_ms\tsettled").unwrap();
        let _ = hook::take_records();
        let (mut capturing, mut steady): (Vec<[f64; 4]>, Vec<[f64; 4]>) = (Vec::new(), Vec::new());
        let (mut clock, mut faces, mut after) = (1_700_000_000_000_i64, 0, None);
        // The renderer builds the far ring synchronously here (frames repeat;
        // the client streams it on its workers): that takes seconds in the
        // first frame the far scene is usable, the second, and is no part
        // of the capture's cost. Two frames of warm-up, unrecorded.
        for _ in 0..2 {
            clock += 40;
            crate::logic_clock::set_test_now(Some(clock));
            frame(&device, &queue, &mut r, &snapshot, &view, size);
        }
        let _ = hook::take_records();
        faces = r.ambient_stats().faces.max(faces);
        for k in 0..400 {
            clock += 40;
            crate::logic_clock::set_test_now(Some(clock));
            let f = frame(&device, &queue, &mut r, &snapshot, &view, size);
            let rec = hook::take_records().pop();
            if k > 0 && f.draw > 500.0 {
                // A hitch after the first frame: where it was spent.
                if let Some(x) = &rec {
                    eprintln!("{name} frame {k}: draw {:.0} ms, phases {:?}", f.draw, x.phases);
                }
            }
            let drawn = r.ambient_stats().faces;
            let gpu_ambient: f64 = rec
                .as_ref()
                .map_or(0.0, |x| x.gpu.iter().filter(|p| p.0.contains("ambient capture")).map(|p| p.1).sum());
            let span = rec.as_ref().map_or(0.0, |x| x.gpu_span_ms);
            let settled = r.ambient_settled(clock);
            writeln!(tsv, "{k}\t{}\t{:.3}\t{:.3}\t{span:.3}\t{gpu_ambient:.3}\t{settled}", drawn - faces, f.draw, f.wall).unwrap();
            let row = [f.draw, f.wall, span, gpu_ambient];
            if drawn > faces {
                capturing.push(row);
            } else if after.is_some() {
                steady.push(row);
            }
            faces = drawn;
            if settled && after.is_none() && k > 10 {
                after = Some(k);
            }
            if after.is_some_and(|a| k >= a + 30) {
                break;
            }
        }
        let med = |rows: &[[f64; 4]], c: usize| pct(&rows.iter().map(|r| r[c]).collect::<Vec<_>>(), 0.5);
        let p99 = |rows: &[[f64; 4]], c: usize| pct(&rows.iter().map(|r| r[c]).collect::<Vec<_>>(), 0.99);
        eprintln!(
            "{name}: settled at frame {after:?}; {:?}; capture frames ({}): draw {:.2}/{:.2} ms, wall {:.2}/{:.2}, gpu span {:.2}, ambient pass {:.3}/{:.3}; steady frames ({}): draw {:.2}/{:.2}, wall {:.2}/{:.2}, gpu span {:.2}",
            r.ambient_stats(),
            capturing.len(),
            med(&capturing, 0),
            p99(&capturing, 0),
            med(&capturing, 1),
            p99(&capturing, 1),
            med(&capturing, 2),
            med(&capturing, 3),
            p99(&capturing, 3),
            steady.len(),
            med(&steady, 0),
            p99(&steady, 0),
            med(&steady, 1),
            p99(&steady, 1),
            med(&steady, 2),
        );
    }
}

/// The shadow views for byte comparisons between two builds (lane
/// P3-SHADOWS): per scene at 800x500, 4x, far 2, every shadow quality
/// (options 0-4) with point shadows off and on, a fresh renderer's settled
/// frame (`s{q}p{0|1}`) and the same renderer 10 frames later (`-late`: the
/// shadow cache's steady state); then a moving camera (`m{q}-f{i}`: the
/// target 12 units further along x each frame) and a turning one
/// (`t{q}-f{i}`: yaw +64 a frame) at MED and ULTRA, 24 frames each. Raw
/// RGBA frames `MODERN_BENCH_OUT/TAG-SCENE-*.rgba`; per frame the caster
/// draws in `TAG-shadow-views.tsv`.
#[test]
#[ignore = "views; needs a GPU and server/data/pack"]
fn modern_shadow_views() {
    let Some(dir) = std::env::var_os("MODERN_BENCH_OUT").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let tag = std::env::var("MODERN_BENCH_TAG").unwrap_or_else(|_| "views".into());
    let scenes: Vec<String> = std::env::var("MODERN_BENCH_SCENES")
        .unwrap_or_else(|_| "lumbridge,river,castle,draynor".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let size = [800_u32, 500];
    let _reset = super::tests::ClockReset;
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = device();
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("views frame"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    let mut tsv = std::fs::File::create(dir.join(format!("{tag}-shadow-views.tsv"))).unwrap();
    writeln!(tsv, "scene\tview\tframe\tshadow_draws\tpoint_shadow_draws").unwrap();
    for name in &scenes {
        let name: &'static str = Box::leak(name.clone().into_boxed_str());
        let sc = scene(&pack, name, size);
        let write = |label: &str| {
            let px = super::tests::read_back(&device, &queue, &output, 4);
            std::fs::write(dir.join(format!("{tag}-{name}-{label}.rgba")), &px).unwrap();
        };
        let fresh = |quality: i32, point: bool, clock: &mut i64| {
            *clock = 1_700_000_000_000;
            crate::logic_clock::set_test_now(Some(*clock));
            let cfg = Config {
                plan: "views",
                label: "views".into(),
                samples: 4,
                shadow: None,
                far: Some(2),
                settings: ModernSettings::DEFAULT,
                threads: bench_threads().map(|l| l[0]),
                point_off: !point,
            };
            let mut r = renderer(&device, &queue, &cfg);
            r.shadow.point.test_off = cfg.point_off;
            r.set_shadow_settings(crate::shadows::Settings::from_options(2, quality, 1));
            r
        };
        let snapshot0 = {
            let mut s = sc.offline.snapshot(&pack);
            s.sky = sc.sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
            s
        };
        let mut step = |r: &mut ModernRenderer, clock: &mut i64, snapshot: &SceneSnapshot<'_>| {
            frame(&device, &queue, r, snapshot, &view, size);
            *clock += 16;
            crate::logic_clock::set_test_now(Some(*clock));
        };
        let mut clock = 0;
        for quality in 0..=4 {
            for point in [false, true] {
                let mut r = fresh(quality, point, &mut clock);
                for _ in 0..80 {
                    step(&mut r, &mut clock, &snapshot0);
                    if !r.probe_capture_pending() {
                        break;
                    }
                }
                for _ in 0..3 {
                    step(&mut r, &mut clock, &snapshot0);
                }
                let label = format!("s{quality}p{}", u8::from(point));
                write(&label);
                for i in 0..10 {
                    step(&mut r, &mut clock, &snapshot0);
                    writeln!(tsv, "{name}\t{label}\t{i}\t{}\t{}", r.stats.shadow_draws, r.stats.point_shadow_draws).unwrap();
                }
                write(&format!("{label}-late"));
            }
        }
        for quality in [1, 3] {
            for (kind, label) in [(0, "m"), (1, "t")] {
                let mut r = fresh(quality, false, &mut clock);
                for _ in 0..80 {
                    step(&mut r, &mut clock, &snapshot0);
                    if !r.probe_capture_pending() {
                        break;
                    }
                }
                for _ in 0..3 {
                    step(&mut r, &mut clock, &snapshot0);
                }
                let mut snapshot = sc.offline.snapshot(&pack);
                snapshot.sky = sc.sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
                for i in 1..=24 {
                    if kind == 0 {
                        snapshot.camera.target[0] = snapshot0.camera.target[0] + 12 * i;
                    } else {
                        snapshot.camera.yaw = snapshot0.camera.yaw + 64.0 * i as f32;
                    }
                    step(&mut r, &mut clock, &snapshot);
                    let v = format!("{label}{quality}");
                    writeln!(tsv, "{name}\t{v}\t{i}\t{}\t{}", r.stats.shadow_draws, r.stats.point_shadow_draws).unwrap();
                    write(&format!("{v}-f{i}"));
                }
            }
        }
    }
}

/// Lane P5: the hitches as the client meets them. One renderer as the
/// client makes it (the far ring streamed on its workers,
/// `MODERN_BENCH_FAR_SYNC=1` builds it before each frame) travels a route
/// of offline scenes (`MODERN_HITCH_ROUTE`, default
/// `lumbridge,draynor,river`: the first stop is the cold start, each next
/// one a region change), `MODERN_HITCH_FRAMES` frames (default 150) at each;
/// then, at the last stop, the settings changes the client makes at run
/// time (anti-aliasing 4x -> 1x -> 4x, shadows MED -> ULTRA -> off -> MED),
/// 40 frames each. Size: the first of `MODERN_BENCH_SIZES` (default
/// 1600x1000), 4x MSAA. Every frame's serial wall time (draw + finish +
/// submit + GPU idle) and `draw` CPU, the shader modules and pipelines
/// created during it on the render thread and off it, and their CPU time:
/// `TAG-hitch-frames.tsv`; per event (start, region change, setting) the
/// first frame, the worst, p99, frames over 50 ms and the time over a
/// 60 Hz budget: `TAG-hitches.tsv`. The start's row also has the
/// renderer's creation (`new_ms`, its pipelines).
#[test]
#[ignore = "hitch timing; needs a GPU and server/data/pack"]
fn modern_hitches() {
    let Some(dir) = std::env::var_os("MODERN_BENCH_OUT").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let tag = std::env::var("MODERN_BENCH_TAG").unwrap_or_else(|_| "hitch".into());
    let route: Vec<String> = std::env::var("MODERN_HITCH_ROUTE")
        .unwrap_or_else(|_| "lumbridge,draynor,river".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let per_stop: usize = std::env::var("MODERN_HITCH_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    let size: [u32; 2] = std::env::var("MODERN_BENCH_SIZES")
        .ok()
        .and_then(|v| {
            let first = v.split(',').next()?.to_string();
            let (w, h) = first.split_once('x')?;
            Some([w.trim().parse().ok()?, h.trim().parse().ok()?])
        })
        .unwrap_or([1600, 1000]);
    let _reset = super::tests::ClockReset;
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = device();
    hook::mark_render_thread();
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hitch frame"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    // Every scene of the route built before the first renderer (the scene's
    // own CPU build is the client's map load, not the renderer's).
    let scenes: Vec<Scene> = route
        .iter()
        .map(|name| {
            let name: &'static str = Box::leak(name.clone().into_boxed_str());
            scene(&pack, name, size)
        })
        .collect();
    let mut frames_tsv = std::fs::File::create(dir.join(format!("{tag}-hitch-frames.tsv"))).unwrap();
    writeln!(frames_tsv, "event\tframe\twall_ms\tdraw_ms\tdraw_cpu_ms\trender_pipelines\trender_compile_ms\tbg_pipelines\tbg_compile_ms\tcreates\tprobe_pending\tphases").unwrap();
    let mut sum = std::fs::File::create(dir.join(format!("{tag}-hitches.tsv"))).unwrap();
    writeln!(sum, "event\tframes\tnew_ms\tnew_pipelines\tnew_compile_ms\tfirst_ms\tto_frame1_ms\tmax_ms\tp99_ms\tp50_ms\tover50\tover_budget_ms\trender_pipelines\trender_compile_ms\tbg_pipelines\tbg_compile_ms\tsettle_frame\tload").unwrap();
    let cfg = Config {
        plan: "hitches",
        label: "hitches".into(),
        samples: 4,
        shadow: Some(Quality::Medium),
        far: Some(2),
        settings: ModernSettings::DEFAULT,
        threads: bench_threads().map(|l| l[0]),
        point_off: false,
    };
    let mut clock = 1_700_000_000_000_i64;
    crate::logic_clock::set_test_now(Some(clock));
    hook::reset(&device);
    let c0 = hook::counts();
    let t = Instant::now();
    let mut r = renderer(&device, &queue, &cfg);
    r.far.sync = std::env::var_os("MODERN_BENCH_FAR_SYNC").is_some();
    configure(&mut r, &cfg);
    let new_ms = t.elapsed().as_secs_f64() * 1000.0;
    let c1 = hook::counts();
    // The client builds the other anti-aliasing counts' sets on the
    // renderer's startup thread, off the render thread (not timed here).
    super::perf_samples::prepare_counts(&mut r, &device, &queue, &[1, 2, 4]);
    let _ = hook::take_records();
    // The first frame follows the creation at once.
    let t = Instant::now() - std::time::Duration::from_secs_f64(new_ms / 1000.0);
    let delta = |a: &[u64], b: &[u64], k: hook::K| b[k as usize] - a[k as usize];
    let new_pipes = delta(&c0, &c1, hook::K::CreatePipeline) + delta(&c0, &c1, hook::K::CreateShader);
    let new_compile = delta(&c0, &c1, hook::K::ShaderPipelineNs) as f64 / 1e6;
    let mut run_event = |r: &mut ModernRenderer,
                         clock: &mut i64,
                         event: &str,
                         sc: &Scene,
                         n: usize,
                         created: Option<(f64, u64, f64, Instant)>| {
        let mut snapshot = sc.offline.snapshot(&pack);
        snapshot.sky = sc.sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
        let (mut walls, mut settle) = (Vec::new(), None);
        let mut to_first = 0.0;
        let totals0 = hook::counts();
        for i in 0..n {
            let c0 = hook::counts();
            let f = frame(&device, &queue, r, &snapshot, &view, size);
            if i == 0 {
                if let Some((.., t)) = created {
                    to_first = t.elapsed().as_secs_f64() * 1000.0;
                }
            }
            let c1 = hook::counts();
            // The frame's CPU phases of at least 2 ms (where a hitch goes).
            let phases: Vec<String> = hook::take_records()
                .iter()
                .flat_map(|rec| rec.phases.iter())
                .filter(|p| p.1 >= 2.0)
                .map(|(n, v)| format!("{n}={v:.1}"))
                .collect();
            *clock += 16;
            crate::logic_clock::set_test_now(Some(*clock));
            let all = delta(&c0, &c1, hook::K::CreatePipeline) + delta(&c0, &c1, hook::K::CreateShader);
            let bg = delta(&c0, &c1, hook::K::BgPipelines);
            let all_ns = delta(&c0, &c1, hook::K::ShaderPipelineNs);
            let bg_ns = delta(&c0, &c1, hook::K::BgCompileNs);
            let pending = r.probe_capture_pending();
            if settle.is_none() && !pending && i > 0 {
                settle = Some(i);
            }
            writeln!(
                frames_tsv,
                "{event}\t{i}\t{:.3}\t{:.3}\t{:.3}\t{}\t{:.2}\t{bg}\t{:.2}\t{}\t{}\t{}",
                f.wall,
                f.draw,
                f.draw_cpu,
                all - bg,
                (all_ns - bg_ns) as f64 / 1e6,
                bg_ns as f64 / 1e6,
                delta(&c0, &c1, hook::K::CreateBuffer) + delta(&c0, &c1, hook::K::CreateTexture),
                u8::from(pending),
                phases.join(";")
            )
            .unwrap();
            walls.push(f.wall);
        }
        let totals1 = hook::counts();
        let all = delta(&totals0, &totals1, hook::K::CreatePipeline) + delta(&totals0, &totals1, hook::K::CreateShader);
        let bg = delta(&totals0, &totals1, hook::K::BgPipelines);
        let all_ns = delta(&totals0, &totals1, hook::K::ShaderPipelineNs);
        let bg_ns = delta(&totals0, &totals1, hook::K::BgCompileNs);
        let over50 = walls.iter().filter(|&&w| w > 50.0).count();
        let over_budget: f64 = walls.iter().map(|w| (w - 1000.0 / 60.0).max(0.0)).sum();
        let (nm, np, nc) = created.map_or((String::new(), String::new(), String::new()), |(m, p, c, _)| {
            (format!("{m:.1}"), p.to_string(), format!("{c:.1}"))
        });
        writeln!(
            sum,
            "{event}\t{n}\t{nm}\t{np}\t{nc}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{over50}\t{over_budget:.0}\t{}\t{:.1}\t{bg}\t{:.1}\t{}\t{}",
            walls[0],
            to_first,
            walls.iter().copied().fold(0.0, f64::max),
            pct(&walls, 0.99),
            pct(&walls, 0.5),
            all - bg,
            (all_ns - bg_ns) as f64 / 1e6,
            bg_ns as f64 / 1e6,
            settle.map_or("-".into(), |s| s.to_string()),
            load_average()
        )
        .unwrap();
        let _ = sum.flush();
        let _ = frames_tsv.flush();
        eprintln!(
            "{event}: first {:.1} ms, max {:.1}, p99 {:.1}, p50 {:.1}, {over50} frames > 50 ms, {over_budget:.0} ms over 60 Hz; {} render-thread compiles ({:.1} ms), {bg} off-thread ({:.1} ms); probes settled at {settle:?}",
            walls[0],
            walls.iter().copied().fold(0.0, f64::max),
            pct(&walls, 0.99),
            pct(&walls, 0.5),
            all - bg,
            (all_ns - bg_ns) as f64 / 1e6,
            bg_ns as f64 / 1e6,
        );
    };
    for (k, sc) in scenes.iter().enumerate() {
        if k == 0 {
            run_event(&mut r, &mut clock, &format!("start {}", sc.name), sc, per_stop, Some((new_ms, new_pipes, new_compile, t)));
        } else {
            run_event(&mut r, &mut clock, &format!("region {}", sc.name), sc, per_stop, None);
        }
    }
    let last = scenes.last().expect("a route");
    let changes: [(&str, Box<dyn Fn(&mut ModernRenderer)>); 5] = [
        ("aa 4x->1x", Box::new(|r: &mut ModernRenderer| super::perf_samples::set_samples_only(r, &device, 1))),
        ("aa 1x->4x", Box::new(|r: &mut ModernRenderer| super::perf_samples::set_samples_only(r, &device, 4))),
        ("shadows ultra", Box::new(|r: &mut ModernRenderer| r.set_shadow_settings(crate::shadows::Settings::from_options(2, Quality::Ultra as i32, 1)))),
        ("shadows off", Box::new(|r: &mut ModernRenderer| r.set_shadow_settings(crate::shadows::Settings::from_options(0, 0, 0)))),
        ("shadows med", Box::new(|r: &mut ModernRenderer| r.set_shadow_settings(crate::shadows::Settings::from_options(2, Quality::Medium as i32, 1)))),
    ];
    for (name, change) in &changes {
        change(&mut r);
        run_event(&mut r, &mut clock, name, last, 40, None);
    }
}

/// Lane P5: whether pipeline creation runs in parallel on this backend.
/// For each count in `MODERN_COMPILE_THREADS` (default `1,4`), that many
/// threads each create a renderer at once (scoped threads over one device;
/// with `MODERN_BENCH_COLD=1` every module is new to Metal's shader cache):
/// the wall time against one thread's shows how much of the compile the
/// threads overlap. Prints one line per count.
#[test]
#[ignore = "compile timing; needs a GPU"]
fn modern_parallel_compile() {
    let (device, queue) = device();
    let counts: Vec<usize> = std::env::var("MODERN_COMPILE_THREADS")
        .unwrap_or_else(|_| "1,4".into())
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    for n in counts {
        let t = Instant::now();
        let each: Vec<f64> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..n)
                .map(|_| {
                    s.spawn(|| {
                        let t = Instant::now();
                        let r = ModernRenderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm, 4, ModernSettings::DEFAULT);
                        drop(r);
                        t.elapsed().as_secs_f64() * 1000.0
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let wall = t.elapsed().as_secs_f64() * 1000.0;
        eprintln!("parallel compile: {n} renderers on {n} threads: wall {wall:.0} ms, each {each:.0?} ms; wall per renderer {:.0} ms", wall / n as f64);
    }
}
