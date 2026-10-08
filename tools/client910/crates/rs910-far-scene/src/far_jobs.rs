//! The far scene's worker threads (`docs/renderer/modern-renderer.md`).
//!
//! The modern client builds map squares on asset worker threads while the
//! main thread only submits jobs, nearest first, and collects the results.
//! [`FarJobs`] is that pool, renderer-neutral: jobs are
//! closures over a per-thread context `C` (each worker owns one, made by
//! the pool's factory on the thread's first job, so a context may hold
//! state that is `Send` but not `Sync`, such as a config store), queued in
//! priority order (lower first; ties in submission order) and cancellable
//! while queued. Results are collected by the owner with
//! [`FarJobs::take_results`].
//!
//! Synchronous mode ([`FarJobs::run_inline`]) runs a job on the calling
//! thread with the pool's inline context: the tests and screenshots build
//! the whole ring in ring order before a frame, so their frames repeat.
//!
//! The results are render-only (no game state depends on them), so the
//! order in which worker results arrive may vary between runs; the owner
//! draws them in its own fixed order.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

/// One queued job: its key, priority, submission number and closure.
type Job<C, R> = Box<dyn FnOnce(&mut C) -> R + Send>;

struct Queued<K, C, R> {
    key: K,
    priority: i64,
    seq: u64,
    job: Job<C, R>,
}

struct Shared<K, C, R> {
    queue: Mutex<VecDeque<Queued<K, C, R>>>,
    wake: Condvar,
    results: Mutex<Vec<(K, R)>>,
    stop: AtomicBool,
    running: AtomicUsize,
    make: Box<dyn Fn() -> C + Send + Sync>,
}

/// See the module docs.
pub struct FarJobs<K: Send + 'static, C: 'static, R: Send + 'static> {
    shared: Arc<Shared<K, C, R>>,
    threads: Vec<JoinHandle<()>>,
    want_threads: usize,
    name: &'static str,
    seq: u64,
    inline: Option<C>,
    inline_runs: usize,
}

/// Worker threads for a pool on this machine: the cores left after the
/// render and logic threads, 1 to `max`.
#[must_use]
pub fn default_threads(max: usize) -> usize {
    std::thread::available_parallelism()
        .map_or(2, std::num::NonZeroUsize::get)
        .saturating_sub(2)
        .clamp(1, max.max(1))
}

impl<K: Send + 'static, C: 'static, R: Send + 'static> FarJobs<K, C, R> {
    /// A pool of `threads` workers (started on the first queued job), each
    /// with its own context from `make`.
    pub fn new(
        name: &'static str,
        threads: usize,
        make: impl Fn() -> C + Send + Sync + 'static,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                queue: Mutex::new(VecDeque::new()),
                wake: Condvar::new(),
                results: Mutex::new(Vec::new()),
                stop: AtomicBool::new(false),
                running: AtomicUsize::new(0),
                make: Box::new(make),
            }),
            threads: Vec::new(),
            want_threads: threads.max(1),
            name,
            seq: 0,
            inline: None,
            inline_runs: 0,
        }
    }

    /// Queue `job` under `key` at `priority` (lower runs first; equal
    /// priorities in submission order).
    pub fn submit(
        &mut self,
        key: K,
        priority: i64,
        job: impl FnOnce(&mut C) -> R + Send + 'static,
    ) {
        self.start_threads();
        self.seq += 1;
        let item = Queued {
            key,
            priority,
            seq: self.seq,
            job: Box::new(job),
        };
        let mut queue = self.shared.queue.lock().expect("far job queue");
        let at = queue.partition_point(|q| (q.priority, q.seq) <= (item.priority, item.seq));
        queue.insert(at, item);
        drop(queue);
        self.shared.wake.notify_one();
    }

    /// Run `job` now on this thread with the pool's inline context (the
    /// synchronous mode).
    pub fn run_inline(&mut self, job: impl FnOnce(&mut C) -> R) -> R {
        self.inline_runs += 1;
        let ctx = self.inline.get_or_insert_with(|| (self.shared.make)());
        job(ctx)
    }

    /// Jobs run on the calling thread so far ([`Self::run_inline`]).
    #[must_use]
    pub fn inline_runs(&self) -> usize {
        self.inline_runs
    }

    /// Drop the queued jobs whose key `keep` rejects (a job already running
    /// finishes; its result still arrives). Returns how many were dropped.
    pub fn retain_queued(&mut self, keep: impl Fn(&K) -> bool) -> usize {
        let mut queue = self.shared.queue.lock().expect("far job queue");
        let before = queue.len();
        queue.retain(|q| keep(&q.key));
        before - queue.len()
    }

    /// The finished jobs' results since the last call, in finishing order.
    pub fn take_results(&mut self) -> Vec<(K, R)> {
        std::mem::take(&mut *self.shared.results.lock().expect("far job results"))
    }

    /// Jobs queued or running.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.shared.queue.lock().expect("far job queue").len()
            + self.shared.running.load(Ordering::Acquire)
    }

    fn start_threads(&mut self) {
        while self.threads.len() < self.want_threads {
            let shared = Arc::clone(&self.shared);
            let name = format!("{}-{}", self.name, self.threads.len());
            let handle = std::thread::Builder::new()
                .name(name)
                .spawn(move || worker(&shared))
                .expect("spawn a far scene worker");
            self.threads.push(handle);
        }
    }
}

fn worker<K: Send, C, R: Send>(shared: &Shared<K, C, R>) {
    let mut ctx: Option<C> = None;
    loop {
        let item = {
            let mut queue = shared.queue.lock().expect("far job queue");
            loop {
                if shared.stop.load(Ordering::Acquire) {
                    return;
                }
                if let Some(item) = queue.pop_front() {
                    // Counted as running before the queue lock is released,
                    // so `pending` never misses it.
                    shared.running.fetch_add(1, Ordering::AcqRel);
                    break item;
                }
                queue = shared.wake.wait(queue).expect("far job queue");
            }
        };
        let ctx = ctx.get_or_insert_with(|| (shared.make)());
        let result = (item.job)(ctx);
        shared
            .results
            .lock()
            .expect("far job results")
            .push((item.key, result));
        shared.running.fetch_sub(1, Ordering::AcqRel);
    }
}

// SAFETY: the one field that may hold a value that is not `Sync` is the
// inline context `C`, which only `&mut self` methods reach (`run_inline`);
// through `&FarJobs` nothing touches it, so sharing the pool shares no `C`
// (the renderer encodes its frame on several threads through a shared
// borrow of state that holds this pool, `rs910_render_modern::frame::jobs`).
unsafe impl<K: Send + 'static, C: Send + 'static, R: Send + 'static> Sync for FarJobs<K, C, R> {}

impl<K: Send + 'static, C: 'static, R: Send + 'static> Drop for FarJobs<K, C, R> {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.queue.lock().expect("far job queue").clear();
        self.shared.wake.notify_all();
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_all<K: Send + 'static, C: 'static, R: Send + 'static>(
        pool: &mut FarJobs<K, C, R>,
    ) -> Vec<(K, R)> {
        let mut out = Vec::new();
        let start = std::time::Instant::now();
        while pool.pending() > 0 {
            out.extend(pool.take_results());
            assert!(start.elapsed().as_secs() < 30, "far jobs never finished");
            std::thread::yield_now();
        }
        out.extend(pool.take_results());
        out
    }

    /// Workers run every job once with their own context; a cancelled
    /// queued job never runs; the inline context is separate and the
    /// result of an inline run is the worker's.
    #[test]
    fn jobs_run_once_on_their_threads_context_and_cancel_while_queued() {
        let made = Arc::new(AtomicUsize::new(0));
        let m = Arc::clone(&made);
        let mut pool: FarJobs<u32, Vec<u32>, (u32, usize)> =
            FarJobs::new("far-test", 2, move || {
                m.fetch_add(1, Ordering::SeqCst);
                Vec::new()
            });
        // A blocker holds one worker so the queue backs up behind it.
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        for key in 0..2 {
            let g = Arc::clone(&gate);
            pool.submit(key, 0, move |ctx| {
                let (lock, cv) = &*g;
                let mut open = lock.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
                ctx.push(key);
                (key, ctx.len())
            });
        }
        for key in 2..20 {
            pool.submit(key, i64::from(key), move |ctx| {
                ctx.push(key);
                (key, ctx.len())
            });
        }
        // Cancel the odd ones while both workers are blocked.
        while pool.shared.running.load(Ordering::Acquire) < 2 {
            std::thread::yield_now();
        }
        let dropped = pool.retain_queued(|k| k % 2 == 0);
        assert_eq!(dropped, 9);
        {
            let (lock, cv) = &*gate;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }
        let mut done = wait_all(&mut pool);
        done.sort_unstable();
        let keys: Vec<u32> = done.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            (0..20).filter(|k| k < &2 || k % 2 == 0).collect::<Vec<_>>()
        );
        assert!(done.iter().all(|(k, (r, _))| k == r));
        // One context per worker, kept across its jobs: only a context's
        // first job sees it empty.
        assert_eq!(made.load(Ordering::SeqCst), 2);
        let firsts = done.iter().filter(|(_, (_, n))| *n == 1).count();
        assert!((1..=2).contains(&firsts), "{firsts} fresh contexts");
        let inline = pool.run_inline(|ctx| {
            ctx.push(99);
            (99, ctx.len())
        });
        assert_eq!(inline, (99, 1));
        assert_eq!(made.load(Ordering::SeqCst), 3);
    }
}
