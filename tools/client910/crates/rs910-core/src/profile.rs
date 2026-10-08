//! Engine profiler (code-quality programme §5, lane E-A4; engine audit §8):
//! named, nested CPU scopes recorded per frame into a fixed-size ring, the
//! GPU pass times of a frame attached when their readback completes, a
//! text summary (the developer console's `prof`, a dev overlay later) and a
//! CSV dump for scripts (`CLIENT910_PROFILE_OUT`, `tools/perf/profsum.py`).
//!
//! **Zero cost when off.**
//! - Compiled out (the default): this crate's `profile` feature is off
//!   (client910's `profile` feature forwards to it). [`profile_scope!`]
//!   expands to its body alone, the hooks ([`frame_mark`], [`enable`],
//!   [`gpu_pass_times`], ...) are empty `#[inline]` functions and
//!   [`enabled`] is the constant `false`, so the optimised code is the code
//!   without the profiler.
//! - Compiled in, switched off at run time: one relaxed atomic load per
//!   scope.
//!
//! **Observationally inert.** It reads a wall clock (`Instant`: the
//! diagnostic class D of `logic_clock`'s table) and writes only its own
//! ring and dump; nothing reads them back into game, UI, audio, scene or
//! render state. `client910`'s
//! `app::session_replay::profiler_is_observationally_inert` pins that on the
//! recorded sessions.
//!
//! **Model.**
//! - A *frame* runs from one [`frame_mark`] to the next. The windowed client
//!   marks the start of each `about_to_wait` turn, so a frame holds that
//!   turn's logic cycles (P0-P11), the frame tail, the redraw (R0-R14) and
//!   the `cpuUsage` sleep until the next turn.
//! - A *scope* is `profile::scope!("name")` (until the end of the enclosing
//!   block) or `profile::scope!("name", expr)` (around one expression; the
//!   value of `expr` is the macro's). Scopes nest; names are `&'static str`.
//! - Main records frame boundaries; a dedicated render worker receives an
//!   immutable [`ThreadContext`] and returns tagged scopes at completion.
//! - [`Profiler`] is the recorder itself, plain data over caller-supplied
//!   nanosecond times (the global hooks supply `Instant` times), so it is
//!   compiled and unit-tested in every configuration.
//!
//! **Dump format** (CSV, one header line): `kind,frame,depth,name,start_ns,dur_ns`
//! where `kind` is `frame` (start since the profiler started), `scope`
//! (start since its frame's start, `depth` its nesting level) or `gpu` (a
//! pass of that frame, start since the frame's first GPU mark; written when
//! the readback arrives, so after the frame's own rows).

use std::fmt::Write as _;
use std::io::Write;

/// Whether the scopes are compiled in (this crate's `profile` feature).
pub const COMPILED: bool = cfg!(feature = "profile");

/// Finished frames kept in the ring (about 4 s at 60 frames per second).
pub const RING_FRAMES: usize = 256;
/// Scopes recorded per frame; later ones are counted in [`Frame::dropped`].
pub const MAX_EVENTS: usize = 512;
/// GPU passes recorded per frame.
pub const MAX_GPU_PASSES: usize = 16;
/// Scope nesting deeper than this is recorded at this depth.
pub const MAX_DEPTH: u8 = 32;

const OPEN: u64 = u64::MAX;

/// One GPU pass of a frame: `(name, start_ns, dur_ns)`, the start relative
/// to the frame's first GPU mark ([`gpu_pass_times`]).
pub type GpuPass = (&'static str, u64, u64);

/// One recorded scope (or GPU pass).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub name: &'static str,
    /// Nesting level (0 = a top-level scope of the frame).
    pub depth: u8,
    /// Start, nanoseconds after the frame's start (GPU: after its first
    /// mark).
    pub start_ns: u64,
    /// Duration in nanoseconds.
    pub dur_ns: u64,
}

/// One frame of the ring: its scopes in the order they were entered (a
/// pre-order walk of the scope tree) and, once the readback arrived, its
/// GPU passes.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    /// Frame number since the profiler started (0 = the first mark).
    pub index: u64,
    /// Start, nanoseconds since the profiler started.
    pub start_ns: u64,
    /// Duration: to the next [`frame_mark`].
    pub dur_ns: u64,
    pub events: Vec<Event>,
    /// Scopes past [`MAX_EVENTS`] (not recorded).
    pub dropped: u32,
    /// GPU passes ([`gpu_pass_times`]), in submission order.
    pub gpu: Vec<Event>,
}

impl Frame {
    fn with_capacity() -> Self {
        Self {
            events: Vec::with_capacity(MAX_EVENTS),
            gpu: Vec::with_capacity(MAX_GPU_PASSES),
            ..Self::default()
        }
    }

    fn clear(&mut self) {
        self.index = 0;
        self.start_ns = 0;
        self.dur_ns = 0;
        self.events.clear();
        self.dropped = 0;
        self.gpu.clear();
    }

    /// Each event's exclusive time: its duration less its direct children's
    /// (the events after it one level deeper, up to the next event at its
    /// depth or above).
    pub fn self_times(&self) -> Vec<u64> {
        let mut own: Vec<u64> = self.events.iter().map(|e| e.dur_ns).collect();
        let mut stack: Vec<usize> = Vec::new();
        for (i, event) in self.events.iter().enumerate() {
            while stack
                .last()
                .is_some_and(|&p| self.events[p].depth >= event.depth)
            {
                stack.pop();
            }
            if let Some(&parent) = stack.last() {
                own[parent] = own[parent].saturating_sub(event.dur_ns);
            }
            stack.push(i);
        }
        own
    }
}

/// A scope's handle from [`Profiler::enter`] for [`Profiler::exit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    index: u32,
    generation: u32,
}

impl Token {
    /// A scope that was not recorded (not recording, or the frame is full).
    pub const NONE: Token = Token {
        index: u32::MAX,
        generation: u32::MAX,
    };
}

/// The recorder: the frame being recorded, the ring of finished frames,
/// the slowest frame since the last reset and the optional CSV dump. Times
/// are the caller's, in nanoseconds on one monotonic clock.
pub struct Profiler {
    current: Frame,
    recording: bool,
    /// Bumped per frame: a [`Token`] of an earlier frame is ignored.
    generation: u32,
    depth: u8,
    next_index: u64,
    ring: Vec<Frame>,
    /// Next ring slot to write.
    head: usize,
    /// Finished frames in the ring.
    len: usize,
    worst: Frame,
    dump: Option<Box<dyn Write>>,
    /// The dump's text buffer, reused per frame.
    line: String,
}

impl Default for Profiler {
    fn default() -> Self {
        Self::new()
    }
}

impl Profiler {
    /// An empty recorder; every buffer is allocated here, none later.
    pub fn new() -> Self {
        Self {
            current: Frame::with_capacity(),
            recording: false,
            generation: 0,
            depth: 0,
            next_index: 0,
            ring: (0..RING_FRAMES).map(|_| Frame::with_capacity()).collect(),
            head: 0,
            len: 0,
            worst: Frame::with_capacity(),
            dump: None,
            line: String::new(),
        }
    }

    /// Write every finished frame (and late GPU rows) to `out` as CSV.
    pub fn set_dump(&mut self, mut out: Box<dyn Write>) {
        let _ = writeln!(out, "kind,frame,depth,name,start_ns,dur_ns");
        self.dump = Some(out);
    }

    /// Flush the dump.
    pub fn flush(&mut self) {
        if let Some(out) = self.dump.as_mut() {
            let _ = out.flush();
        }
    }

    /// End the frame being recorded (if any) at `now` and start the next.
    pub fn frame_mark(&mut self, now: u64) {
        if self.recording {
            let start = self.current.start_ns;
            for event in &mut self.current.events {
                if event.dur_ns == OPEN {
                    event.dur_ns = now.saturating_sub(start + event.start_ns);
                }
            }
            self.current.dur_ns = now.saturating_sub(start);
            self.write_frame();
            if self.current.dur_ns > self.worst.dur_ns {
                self.worst.clone_from(&self.current);
            }
            std::mem::swap(&mut self.ring[self.head], &mut self.current);
            self.head = (self.head + 1) % RING_FRAMES;
            self.len = (self.len + 1).min(RING_FRAMES);
        }
        self.current.clear();
        self.current.index = self.next_index;
        self.current.start_ns = now;
        self.next_index += 1;
        self.generation = self.generation.wrapping_add(1);
        self.depth = 0;
        self.recording = true;
    }

    /// Open scope `name` at `now`.
    pub fn enter(&mut self, name: &'static str, now: u64) -> Token {
        if !self.recording {
            return Token::NONE;
        }
        if self.current.events.len() >= MAX_EVENTS {
            self.current.dropped += 1;
            return Token::NONE;
        }
        let index = self.current.events.len() as u32;
        self.current.events.push(Event {
            name,
            depth: self.depth,
            start_ns: now.saturating_sub(self.current.start_ns),
            dur_ns: OPEN,
        });
        self.depth = (self.depth + 1).min(MAX_DEPTH);
        Token {
            index,
            generation: self.generation,
        }
    }

    /// Close the scope of `token` at `now` (scopes close innermost first).
    pub fn exit(&mut self, token: Token, now: u64) {
        if token.generation != self.generation {
            return;
        }
        let start = self.current.start_ns;
        if let Some(event) = self.current.events.get_mut(token.index as usize) {
            event.dur_ns = now.saturating_sub(start + event.start_ns);
            self.depth = event.depth;
        }
    }

    /// The GPU passes of frame `frame` (`(name, start, duration)`, start
    /// relative to the frame's first mark), attached to the frame when it is
    /// still in the ring and written to the dump.
    pub fn gpu_pass_times(&mut self, frame: u64, passes: &[GpuPass]) {
        let events = passes
            .iter()
            .take(MAX_GPU_PASSES)
            .map(|&(name, start, dur)| Event {
                name,
                depth: 0,
                start_ns: start,
                dur_ns: dur,
            });
        if self.current.index == frame && self.recording {
            self.current.gpu.clear();
            self.current.gpu.extend(events);
        } else if let Some(slot) = self.ring.iter_mut().find(|f| f.index == frame) {
            slot.gpu.clear();
            slot.gpu.extend(events);
            if let Some(out) = self.dump.as_mut() {
                let line = &mut self.line;
                line.clear();
                for event in &slot.gpu {
                    let _ = writeln!(
                        line,
                        "gpu,{frame},0,{},{},{}",
                        event.name, event.start_ns, event.dur_ns
                    );
                }
                let _ = out.write_all(line.as_bytes());
            }
            if self.worst.index == frame {
                self.worst.gpu.clone_from(&slot.gpu);
            }
        }
    }

    fn write_frame(&mut self) {
        let Some(out) = self.dump.as_mut() else {
            return;
        };
        let (frame, line) = (&self.current, &mut self.line);
        line.clear();
        let _ = writeln!(
            line,
            "frame,{},0,,{},{}",
            frame.index, frame.start_ns, frame.dur_ns
        );
        for event in &frame.events {
            let _ = writeln!(
                line,
                "scope,{},{},{},{},{}",
                frame.index, event.depth, event.name, event.start_ns, event.dur_ns
            );
        }
        for event in &frame.gpu {
            let _ = writeln!(
                line,
                "gpu,{},0,{},{},{}",
                frame.index, event.name, event.start_ns, event.dur_ns
            );
        }
        let _ = out.write_all(line.as_bytes());
    }

    /// The finished frames, oldest first.
    pub fn frames(&self) -> impl Iterator<Item = &Frame> {
        let first = (self.head + RING_FRAMES - self.len) % RING_FRAMES;
        (0..self.len).map(move |i| &self.ring[(first + i) % RING_FRAMES])
    }

    /// The frame being recorded.
    pub fn current(&self) -> &Frame {
        &self.current
    }

    /// The slowest finished frame since the start or [`Profiler::reset`].
    pub fn worst(&self) -> Option<&Frame> {
        (self.worst.dur_ns > 0).then_some(&self.worst)
    }

    /// Forget the finished frames and the slowest one (the dump continues).
    pub fn reset(&mut self) {
        self.len = 0;
        self.worst.clear();
    }

    /// Statistics over the last `last` finished frames.
    pub fn summary(&self, last: usize) -> Summary {
        let skip = self.len.saturating_sub(last);
        Summary::of(self.frames().skip(skip))
    }
}

/// One scope name's statistics over a [`Summary`]'s frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScopeStat {
    pub name: &'static str,
    /// The shallowest depth it was seen at.
    pub depth: u8,
    pub calls: u64,
    /// Inclusive time, all calls.
    pub total_ns: u64,
    /// Exclusive time (less the scopes nested in it), all calls.
    pub self_ns: u64,
    /// The largest per-frame inclusive time.
    pub max_frame_ns: u64,
}

/// Frame-time and per-scope statistics of some frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub frames: usize,
    pub frame_mean_ns: u64,
    pub frame_p50_ns: u64,
    pub frame_p95_ns: u64,
    pub frame_max_ns: u64,
    /// By inclusive total, largest first.
    pub scopes: Vec<ScopeStat>,
    /// GPU passes, by total, largest first; `frames` counts the frames
    /// with GPU times.
    pub gpu: Vec<ScopeStat>,
    pub gpu_frames: usize,
    pub dropped: u64,
}

fn accumulate(stats: &mut Vec<ScopeStat>, events: &[Event], own: Option<&[u64]>) {
    let mut per_frame: Vec<(usize, u64)> = Vec::new();
    // The open ancestors of each event (depth, name): a recursive scope's
    // inclusive time counts once, at its outermost call.
    let mut ancestors: Vec<(u8, &'static str)> = Vec::new();
    for (i, event) in events.iter().enumerate() {
        while ancestors.last().is_some_and(|&(d, _)| d >= event.depth) {
            ancestors.pop();
        }
        let nested_in_same = ancestors.iter().any(|&(_, name)| name == event.name);
        ancestors.push((event.depth, event.name));
        let slot = match stats.iter().position(|s| s.name == event.name) {
            Some(slot) => slot,
            None => {
                stats.push(ScopeStat {
                    name: event.name,
                    depth: event.depth,
                    ..ScopeStat::default()
                });
                stats.len() - 1
            }
        };
        let stat = &mut stats[slot];
        stat.depth = stat.depth.min(event.depth);
        stat.calls += 1;
        stat.self_ns += own.map_or(event.dur_ns, |own| own[i]);
        if !nested_in_same {
            stat.total_ns += event.dur_ns;
            match per_frame.iter_mut().find(|(s, _)| *s == slot) {
                Some((_, sum)) => *sum += event.dur_ns,
                None => per_frame.push((slot, event.dur_ns)),
            }
        }
    }
    for (slot, sum) in per_frame {
        stats[slot].max_frame_ns = stats[slot].max_frame_ns.max(sum);
    }
}

impl Summary {
    /// Statistics over `frames`.
    pub fn of<'a>(frames: impl Iterator<Item = &'a Frame>) -> Self {
        let mut summary = Summary::default();
        let mut durations = Vec::new();
        for frame in frames {
            durations.push(frame.dur_ns);
            summary.dropped += u64::from(frame.dropped);
            let own = frame.self_times();
            accumulate(&mut summary.scopes, &frame.events, Some(&own));
            if !frame.gpu.is_empty() {
                summary.gpu_frames += 1;
                accumulate(&mut summary.gpu, &frame.gpu, None);
            }
        }
        summary.frames = durations.len();
        if !durations.is_empty() {
            durations.sort_unstable();
            let pick = |q: usize| durations[((durations.len() - 1) * q) / 100];
            summary.frame_mean_ns = durations.iter().sum::<u64>() / durations.len() as u64;
            summary.frame_p50_ns = pick(50);
            summary.frame_p95_ns = pick(95);
            summary.frame_max_ns = *durations.last().unwrap();
        }
        summary
            .scopes
            .sort_by_key(|s| std::cmp::Reverse(s.total_ns));
        summary.gpu.sort_by_key(|s| std::cmp::Reverse(s.total_ns));
        summary
    }

    /// Readout lines: the frame times, then the `top` scopes by inclusive
    /// time per frame (`ms/f` mean over all frames, `self` exclusive,
    /// `n/f` calls per frame, `max` the worst frame's), then the GPU passes.
    pub fn lines(&self, top: usize) -> Vec<String> {
        let ms = |ns: u64| ns as f64 / 1e6;
        let per = |ns: u64, frames: usize| ns as f64 / 1e6 / frames.max(1) as f64;
        let mut lines = vec![format!(
            "{} frames: mean {:.2} ms, p50 {:.2}, p95 {:.2}, max {:.2}{}",
            self.frames,
            ms(self.frame_mean_ns),
            ms(self.frame_p50_ns),
            ms(self.frame_p95_ns),
            ms(self.frame_max_ns),
            if self.dropped > 0 {
                format!(" ({} scopes dropped)", self.dropped)
            } else {
                String::new()
            }
        )];
        for s in self.scopes.iter().take(top) {
            lines.push(format!(
                "{:<24} {:>7.3} ms/f self {:>7.3} n/f {:>5.2} max {:>7.2}",
                s.name,
                per(s.total_ns, self.frames),
                per(s.self_ns, self.frames),
                s.calls as f64 / self.frames.max(1) as f64,
                ms(s.max_frame_ns)
            ));
        }
        if self.gpu_frames > 0 {
            lines.push(format!("gpu ({} frames timed):", self.gpu_frames));
            for s in self.gpu.iter().take(top) {
                lines.push(format!(
                    "{:<24} {:>7.3} ms/f max {:>7.2}",
                    s.name,
                    per(s.total_ns, self.gpu_frames),
                    ms(s.max_frame_ns)
                ));
            }
        }
        lines
    }
}

/// Readout lines of one frame's scope tree: every scope of at least
/// `min_ns`, indented by depth, then its GPU passes.
pub fn frame_lines(frame: &Frame, min_ns: u64) -> Vec<String> {
    let mut lines = vec![format!(
        "frame {}: {:.2} ms, {} scopes{}",
        frame.index,
        frame.dur_ns as f64 / 1e6,
        frame.events.len(),
        if frame.dropped > 0 {
            format!(" (+{} dropped)", frame.dropped)
        } else {
            String::new()
        }
    )];
    for event in frame.events.iter().filter(|e| e.dur_ns >= min_ns) {
        lines.push(format!(
            "{:indent$}{} {:.3} ms",
            "",
            event.name,
            event.dur_ns as f64 / 1e6,
            indent = 2 * usize::from(event.depth) + 1
        ));
    }
    for event in &frame.gpu {
        lines.push(format!(
            " {} {:.3} ms",
            event.name,
            event.dur_ns as f64 / 1e6
        ));
    }
    lines
}

#[cfg(feature = "profile")]
mod global {
    use super::*;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    pub(super) static ENABLED: AtomicBool = AtomicBool::new(false);
    pub(super) static GPU: AtomicBool = AtomicBool::new(true);

    thread_local! {
        pub(super) static STATE: RefCell<Option<(Instant, Box<Profiler>)>> =
            const { RefCell::new(None) };
    }

    pub(super) fn now(epoch: Instant) -> u64 {
        epoch.elapsed().as_nanos() as u64
    }

    pub(super) fn with<R>(f: impl FnOnce(&mut Profiler, u64) -> R) -> Option<R> {
        STATE
            .try_with(|state| {
                let mut state = state.try_borrow_mut().ok()?;
                let (epoch, profiler) = state.as_mut()?;
                let now = now(*epoch);
                Some(f(profiler, now))
            })
            .ok()
            .flatten()
    }

    pub(super) fn enabled() -> bool {
        ENABLED.load(Ordering::Relaxed)
    }

    pub(super) fn set(on: bool) {
        if on && !enabled() {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                match state.as_mut() {
                    None => *state = Some((Instant::now(), Box::new(Profiler::new()))),
                    // Switched back on: the frame open since the switch-off
                    // is dropped, the next mark starts a new one.
                    Some((_, profiler)) => profiler.recording = false,
                }
            });
        }
        ENABLED.store(on, Ordering::Relaxed);
    }
}

/// The immutable main frame tag and epoch lent to a render worker. It carries
/// diagnostic state only and is empty when profiling is compiled out.
#[derive(Clone, Copy, Default)]
pub struct ThreadContext {
    #[cfg(feature = "profile")]
    value: Option<(std::time::Instant, u64, u64)>,
}
impl ThreadContext {
    /// Wall time from the captured main frame mark, through the caller's
    /// completion. Separate from enqueue latency; unavailable without profiling.
    pub fn elapsed_since_frame_start(&self) -> Option<std::time::Duration> {
        #[cfg(feature = "profile")]
        if let Some((epoch, _, start)) = self.value {
            return epoch
                .elapsed()
                .checked_sub(std::time::Duration::from_nanos(start));
        }
        None
    }
}
/// Completed worker scopes and late GPU rows, merged on the recorder's owner.
#[derive(Default)]
pub struct ThreadRecording {
    #[cfg(feature = "profile")]
    frame: u64,
    #[cfg(feature = "profile")]
    events: Vec<Event>,
    #[cfg(feature = "profile")]
    dropped: u32,
    #[cfg(feature = "profile")]
    gpu: Vec<(u64, Vec<GpuPass>)>,
}
#[cfg(feature = "profile")]
type GpuReadbacks = Vec<(u64, Vec<GpuPass>)>;
#[cfg(feature = "profile")]
std::thread_local! {
    static THREAD_GPU: std::cell::RefCell<Option<GpuReadbacks>> = const { std::cell::RefCell::new(None) };
}
/// Capture the frame being recorded; worker recording does not advance it.
pub fn thread_context() -> ThreadContext {
    #[cfg(feature = "profile")]
    if enabled() {
        return ThreadContext {
            value: global::STATE.with(|state| {
                let state = state.borrow();
                let (epoch, profiler) = state.as_ref()?;
                profiler.recording.then_some((
                    *epoch,
                    profiler.current.index,
                    profiler.current.start_ns,
                ))
            }),
        };
    }
    ThreadContext::default()
}
/// Install a captured main-frame identity on a dedicated worker. Storage is
/// allocated only on its first profiled frame, then reused.
pub fn begin_thread(context: ThreadContext) {
    #[cfg(feature = "profile")]
    if let Some((epoch, index, start)) = context.value {
        global::STATE.with(|state| {
            let mut state = state.borrow_mut();
            let (_, profiler) = state.get_or_insert_with(|| (epoch, Box::new(Profiler::new())));
            profiler.current.clear();
            profiler.current.index = index;
            profiler.current.start_ns = start;
            profiler.depth = 0;
            profiler.generation = profiler.generation.wrapping_add(1);
            profiler.recording = true;
        });
        THREAD_GPU.with(|gpu| *gpu.borrow_mut() = Some(Vec::new()));
    }
    #[cfg(not(feature = "profile"))]
    let _ = context;
}
/// Finish worker scopes without manufacturing another main-loop frame.
pub fn end_thread() -> ThreadRecording {
    #[cfg(feature = "profile")]
    {
        let gpu = THREAD_GPU.with(|gpu| gpu.borrow_mut().take());
        if let Some(gpu) = gpu {
            return global::with(|p, _| {
                p.recording = false;
                ThreadRecording {
                    frame: p.current.index,
                    events: p.current.events.clone(),
                    dropped: p.current.dropped,
                    gpu,
                }
            })
            .unwrap_or_default();
        }
    }
    ThreadRecording::default()
}
/// Attach worker CPU rows to their original frame, even after its main turn
/// ended. Their root scopes remain separate from main scopes, so overlapping
/// time is never subtracted from main work or summed into frame latency.
pub fn merge_thread(recording: ThreadRecording) {
    #[cfg(feature = "profile")]
    global::with(|p, _| {
        let ThreadRecording {
            frame,
            events,
            dropped,
            gpu,
        } = recording;
        let current = p.recording && p.current.index == frame;
        if !current {
            if let Some(out) = p.dump.as_mut() {
                p.line.clear();
                for event in &events {
                    let _ = writeln!(
                        p.line,
                        "scope,{frame},{},{},{},{}",
                        event.depth, event.name, event.start_ns, event.dur_ns
                    );
                }
                let _ = out.write_all(p.line.as_bytes());
            }
        }
        let target = if current {
            Some(&mut p.current)
        } else {
            p.ring.iter_mut().find(|slot| slot.index == frame)
        };
        if let Some(target) = target {
            let available = MAX_EVENTS.saturating_sub(target.events.len());
            target.dropped += dropped + events.len().saturating_sub(available) as u32;
            target.events.extend(events.into_iter().take(available));
        }
        for (frame, passes) in gpu {
            p.gpu_pass_times(frame, &passes);
        }
    });
    #[cfg(not(feature = "profile"))]
    let _ = recording;
}

/// Whether scopes are being recorded (always `false` when compiled out).
#[inline(always)]
pub fn enabled() -> bool {
    #[cfg(feature = "profile")]
    {
        global::enabled()
    }
    #[cfg(not(feature = "profile"))]
    {
        false
    }
}

/// Start recording on this thread (a no-op when compiled out). The
/// recorder is created on first use and kept across [`disable`].
#[inline]
pub fn enable() {
    #[cfg(feature = "profile")]
    global::set(true);
}

/// Stop recording (the ring and dump are kept).
#[inline]
pub fn disable() {
    #[cfg(feature = "profile")]
    global::set(false);
}

/// Start recording on this thread and dump every frame to `path` (CSV,
/// module docs). A no-op when compiled out.
pub fn start_dump(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(feature = "profile")]
    {
        let file = std::fs::File::create(path)?;
        enable();
        global::with(|p, _| p.set_dump(Box::new(std::io::BufWriter::with_capacity(1 << 16, file))));
    }
    #[cfg(not(feature = "profile"))]
    let _ = path;
    Ok(())
}

/// Flush the dump (the shell calls it when the client exits).
#[inline]
pub fn finish() {
    #[cfg(feature = "profile")]
    global::with(|p, _| p.flush());
}

/// The start of a frame (module docs): the previous frame is finished.
#[inline(always)]
pub fn frame_mark() {
    #[cfg(feature = "profile")]
    if global::enabled() {
        global::with(|p, now| p.frame_mark(now));
    }
}

/// The number of the frame being recorded, when recording (to tag GPU work
/// whose times arrive later, [`gpu_pass_times`]).
#[inline(always)]
pub fn current_frame() -> Option<u64> {
    #[cfg(feature = "profile")]
    if global::enabled() {
        return global::with(|p, _| p.recording.then_some(p.current.index)).flatten();
    }
    None
}

/// Whether the renderer times its GPU passes while recording (default on;
/// the timestamps cost GPU time, measured in programme §5 "Engine
/// profiler", so a CPU-only A/B switches them off).
#[inline(always)]
pub fn gpu_timing() -> bool {
    #[cfg(feature = "profile")]
    {
        global::GPU.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(feature = "profile"))]
    {
        false
    }
}

/// Switch the GPU pass timing on or off ([`gpu_timing`]).
#[inline]
pub fn set_gpu_timing(on: bool) {
    #[cfg(feature = "profile")]
    global::GPU.store(on, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(feature = "profile"))]
    let _ = on;
}

/// Attach frame `frame`'s GPU passes (`(name, start_ns, dur_ns)`).
#[inline]
pub fn gpu_pass_times(frame: u64, passes: &[GpuPass]) {
    #[cfg(feature = "profile")]
    {
        let remote = THREAD_GPU.with(|gpu| {
            let mut gpu = gpu.borrow_mut();
            if let Some(gpu) = gpu.as_mut() {
                gpu.push((frame, passes.to_vec()));
                true
            } else {
                false
            }
        });
        if !remote {
            global::with(|p, _| p.gpu_pass_times(frame, passes));
        }
    }
    #[cfg(not(feature = "profile"))]
    let _ = (frame, passes);
}

/// Run `f` on this thread's recorder (the console readout); `None` when
/// compiled out or never enabled on this thread.
pub fn with_profiler<R>(f: impl FnOnce(&mut Profiler) -> R) -> Option<R> {
    #[cfg(feature = "profile")]
    {
        global::with(|p, _| f(p))
    }
    #[cfg(not(feature = "profile"))]
    {
        let _ = f;
        None
    }
}

/// The guard of [`profile_scope!`]: records its scope from creation to
/// drop.
#[must_use = "a scope ends when its guard drops"]
pub struct Scope {
    #[cfg(feature = "profile")]
    token: Token,
}

impl Scope {
    /// Open scope `name` when recording.
    #[inline(always)]
    pub fn enter(name: &'static str) -> Self {
        #[cfg(feature = "profile")]
        {
            let token = if global::enabled() {
                global::with(|p, now| p.enter(name, now)).unwrap_or(Token::NONE)
            } else {
                Token::NONE
            };
            Scope { token }
        }
        #[cfg(not(feature = "profile"))]
        {
            let _ = name;
            Scope {}
        }
    }
}

impl Scope {
    /// End the scope here (before its guard would go out of scope).
    #[inline(always)]
    pub fn end(self) {}
}

#[cfg(feature = "profile")]
impl Drop for Scope {
    #[inline(always)]
    fn drop(&mut self) {
        if self.token != Token::NONE {
            global::with(|p, now| p.exit(self.token, now));
        }
    }
}

/// A profiler scope (module docs): `profile_scope!("name");` records until
/// the end of the enclosing block, `profile_scope!("name", expr)` around
/// `expr` (the macro's value is `expr`'s). Compiled out, the first form is
/// nothing and the second is `expr`.
#[cfg(feature = "profile")]
#[macro_export]
macro_rules! profile_scope {
    ($name:expr) => {
        let _profile_scope = $crate::profile::Scope::enter($name);
    };
    ($name:expr, $body:expr) => {{
        let _profile_scope = $crate::profile::Scope::enter($name);
        $body
    }};
}

/// A profiler scope (module docs); compiled out.
#[cfg(not(feature = "profile"))]
#[macro_export]
macro_rules! profile_scope {
    ($name:expr) => {};
    ($name:expr, $body:expr) => {
        $body
    };
}

/// `rs910_core::profile::scope!`: [`profile_scope!`](crate::profile_scope)
/// by its module path (the call sites name the module, so the DAG check
/// sees an edge to `profile`, not to the crate root).
pub use crate::profile_scope as scope;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(feature = "profile")]
    fn worker_scopes_keep_the_captured_main_frame() {
        enable();
        frame_mark();
        let frame = current_frame().unwrap();
        let context = thread_context();
        let recording = std::thread::spawn(move || {
            begin_thread(context);
            assert_eq!(current_frame(), Some(frame));
            {
                let _scope = Scope::enter("worker test");
            }
            end_thread()
        })
        .join()
        .unwrap();
        frame_mark();
        merge_thread(recording);
        with_profiler(|profiler| {
            let recorded = profiler
                .ring
                .iter()
                .find(|slot| slot.index == frame)
                .unwrap();
            assert!(recorded
                .events
                .iter()
                .any(|event| event.name == "worker test"));
            assert!(!profiler
                .current
                .events
                .iter()
                .any(|event| event.name == "worker test"));
        })
        .unwrap();
        disable();
    }

    fn names(frame: &Frame) -> Vec<(&str, u8, u64, u64)> {
        frame
            .events
            .iter()
            .map(|e| (e.name, e.depth, e.start_ns, e.dur_ns))
            .collect()
    }

    /// Scopes nest by depth with frame-relative starts; a frame ends at the
    /// next mark; the ring keeps the finished frames oldest first.
    #[test]
    fn nested_scopes_are_recorded_per_frame() {
        let mut p = Profiler::new();
        assert_eq!(p.enter("before", 1), Token::NONE, "no frame yet");
        p.frame_mark(100);
        let a = p.enter("a", 110);
        let b = p.enter("b", 120);
        p.exit(b, 150);
        let c = p.enter("c", 150);
        p.exit(c, 160);
        p.exit(a, 170);
        let d = p.enter("d", 180);
        p.exit(d, 190);
        p.frame_mark(200);
        let frames: Vec<&Frame> = p.frames().collect();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].index, 0);
        assert_eq!(frames[0].dur_ns, 100);
        assert_eq!(
            names(frames[0]),
            [
                ("a", 0, 10, 60),
                ("b", 1, 20, 30),
                ("c", 1, 50, 10),
                ("d", 0, 80, 10)
            ]
        );
        assert_eq!(frames[0].self_times(), [20, 30, 10, 10]);
        assert_eq!(p.current().index, 1);
    }

    /// A scope still open at a mark is closed there; its late exit is
    /// ignored and the next frame starts at depth 0.
    #[test]
    fn a_scope_open_across_a_mark_is_closed_at_the_mark() {
        let mut p = Profiler::new();
        p.frame_mark(0);
        let open = p.enter("open", 10);
        p.frame_mark(50);
        p.exit(open, 70);
        let next = p.enter("next", 80);
        p.exit(next, 90);
        p.frame_mark(100);
        let frames: Vec<&Frame> = p.frames().collect();
        assert_eq!(names(frames[0]), [("open", 0, 10, 40)]);
        assert_eq!(names(frames[1]), [("next", 0, 30, 10)]);
    }

    /// The ring holds the last `RING_FRAMES` frames, reuses its buffers and
    /// counts scopes past `MAX_EVENTS`; the slowest frame is kept.
    #[test]
    fn the_ring_wraps_and_full_frames_count_dropped_scopes() {
        let mut p = Profiler::new();
        let mut t = 0;
        for frame in 0..RING_FRAMES as u64 + 10 {
            p.frame_mark(t);
            let n = if frame == 3 { MAX_EVENTS + 5 } else { 2 };
            for _ in 0..n {
                let s = p.enter("s", t);
                t += 1;
                p.exit(s, t);
            }
            t += if frame == 7 { 1000 } else { 10 };
        }
        p.frame_mark(t);
        let frames: Vec<&Frame> = p.frames().collect();
        assert_eq!(frames.len(), RING_FRAMES);
        assert_eq!(frames[0].index, 10);
        assert_eq!(frames.last().unwrap().index, RING_FRAMES as u64 + 9);
        let worst = p.worst().unwrap();
        assert_eq!(worst.index, 7);
        assert_eq!(worst.dur_ns, 1002);
        let mut q = Profiler::new();
        q.frame_mark(0);
        for _ in 0..MAX_EVENTS + 5 {
            let s = q.enter("s", 1);
            q.exit(s, 2);
        }
        q.frame_mark(3);
        let frame = q.frames().next().unwrap();
        assert_eq!((frame.events.len(), frame.dropped), (MAX_EVENTS, 5));
        assert_eq!(q.summary(10).dropped, 5);
    }

    /// The summary sums inclusive and exclusive time per name, counts a
    /// recursive scope once, and attaches GPU passes that arrive late.
    #[test]
    fn summary_and_late_gpu_times() {
        let mut p = Profiler::new();
        for f in 0..4u64 {
            let t = f * 1000;
            p.frame_mark(t);
            let outer = p.enter("logic", t + 10);
            let inner = p.enter("logic", t + 20);
            let leaf = p.enter("leaf", t + 30);
            p.exit(leaf, t + 50);
            p.exit(inner, t + 60);
            p.exit(outer, t + 110);
        }
        p.frame_mark(4000);
        p.gpu_pass_times(1, &[("gpu scene", 0, 700), ("gpu ui", 700, 100)]);
        p.gpu_pass_times(9999, &[("gone", 0, 1)]);
        let s = p.summary(3);
        assert_eq!(s.frames, 3);
        assert_eq!(s.frame_mean_ns, 1000);
        let logic = s.scopes.iter().find(|x| x.name == "logic").unwrap();
        assert_eq!(
            (logic.calls, logic.total_ns, logic.max_frame_ns),
            (6, 300, 100)
        );
        // Exclusive: outer 100-40 = 60, inner 40-20 = 20, per frame.
        assert_eq!(logic.self_ns, 3 * 80);
        assert_eq!(s.scopes[0].name, "logic");
        assert_eq!(s.gpu_frames, 1);
        assert_eq!(s.gpu[0].name, "gpu scene");
        assert!(s.lines(5).iter().any(|l| l.starts_with("gpu scene")));
        let tree = frame_lines(p.frames().nth(1).unwrap(), 0);
        assert_eq!(tree[1].trim_start(), "logic 0.000 ms");
        assert!(tree.iter().any(|l| l.contains("gpu ui")));
    }

    /// The CSV dump: a header, one `frame` row and its `scope` rows per
    /// finished frame, and `gpu` rows when the readback arrives.
    #[test]
    fn dump_rows() {
        #[derive(Clone, Default)]
        struct Shared(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
        impl Write for Shared {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.borrow_mut().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let out = Shared::default();
        let mut p = Profiler::new();
        p.set_dump(Box::new(out.clone()));
        p.frame_mark(5);
        let a = p.enter("P1 js5", 6);
        p.exit(a, 9);
        p.frame_mark(15);
        p.gpu_pass_times(0, &[("gpu scene", 0, 4)]);
        p.flush();
        let text = String::from_utf8(out.0.borrow().clone()).unwrap();
        assert_eq!(
            text,
            "kind,frame,depth,name,start_ns,dur_ns\n\
             frame,0,0,,5,10\n\
             scope,0,0,P1 js5,1,3\n\
             gpu,0,0,gpu scene,0,4\n"
        );
    }

    /// The macro's value is its body's in both configurations; with the
    /// profiler compiled in and enabled the scopes reach this thread's
    /// recorder.
    #[test]
    fn macro_forms() {
        let value = crate::profile_scope!("value", 40 + 2);
        assert_eq!(value, 42);
        {
            crate::profile_scope!("block");
        }
        if COMPILED {
            enable();
            frame_mark();
            {
                crate::profile_scope!("outer");
                let _ = crate::profile_scope!("inner", 1);
            }
            frame_mark();
            disable();
            let recorded = with_profiler(|p| {
                p.frames().last().map(|f| {
                    f.events
                        .iter()
                        .map(|e| (e.name, e.depth))
                        .collect::<Vec<_>>()
                })
            })
            .flatten();
            assert_eq!(recorded, Some(vec![("outer", 0), ("inner", 1)]));
        } else {
            assert!(!enabled());
            assert_eq!(current_frame(), None);
        }
    }
}
