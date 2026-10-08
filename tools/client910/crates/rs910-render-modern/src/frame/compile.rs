//! Pipeline compiles that run at once (performance plan P5).
//!
//! `ModernRenderer::new` creates about 70 pipelines. With a warm shader
//! cache each takes about a millisecond; with a cold one (the first run
//! after a shader or driver change) each takes 50-630 ms of Metal compiler
//! time, 6.8 s in all. Metal compiles on the threads that ask (wgpu 30's
//! Metal backend holds no device lock while compiling, wgpu 22's did), so
//! independent pipelines are created on several threads: four threads
//! creating four renderers at once take 7 s cold against 27 s with wgpu 22.
//!
//! [`join`] and [`all`] run closures on scoped threads and return their
//! results in argument order; a panic in a closure reaches the caller. With
//! `CLIENT910_MODERN_THREADS=1` (the synchronous mode) they run the
//! closures one after another on the caller, in order. Which thread created
//! a pipeline does not show in a frame: the pipelines are the same
//! descriptors in the same places.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// A boxed job for [`all`].
pub(crate) type Job<'a, T> = Box<dyn FnOnce() -> T + Send + 'a>;

/// Whether compiles may run on several threads (not in the synchronous
/// mode).
fn concurrent() -> bool {
    crate::frame::jobs::default_threads() > 1
}

/// The most threads [`all`] runs at once: the cores, and at least four (the
/// Metal compiler waits on its own service, so a few more threads than
/// cores keep it busy).
fn width() -> usize {
    std::thread::available_parallelism()
        .map_or(4, std::num::NonZeroUsize::get)
        .max(4)
}

/// Run `a` and `b` at once.
pub(crate) fn join<'a, A: Send, B: Send>(
    a: impl FnOnce() -> A + Send + 'a,
    b: impl FnOnce() -> B + Send + 'a,
) -> (A, B) {
    if !concurrent() {
        return (a(), b());
    }
    std::thread::scope(|scope| {
        let handle = std::thread::Builder::new()
            .name("modern-compile".into())
            .spawn_scoped(scope, b)
            .expect("spawn a compile thread");
        let a = a();
        match handle.join() {
            Ok(b) => (a, b),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}

/// Run `jobs` on up to [`width`] threads, each thread taking the next job
/// that is left; the results are in job order.
pub(crate) fn all<'a, T: Send>(jobs: Vec<Job<'a, T>>) -> Vec<T> {
    let n = jobs.len();
    if n <= 1 || !concurrent() {
        return jobs.into_iter().map(|job| job()).collect();
    }
    let jobs: Vec<Mutex<Option<Job<'a, T>>>> =
        jobs.into_iter().map(|job| Mutex::new(Some(job))).collect();
    let results: Vec<Mutex<Option<T>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let work = || loop {
        let i = next.fetch_add(1, Ordering::Relaxed);
        if i >= n {
            return;
        }
        let job = jobs[i]
            .lock()
            .expect("a compile job")
            .take()
            .expect("each job runs once");
        *results[i].lock().expect("a compile result") = Some(job());
    };
    std::thread::scope(|scope| {
        let handles: Vec<_> = (1..width().min(n))
            .map(|_| {
                std::thread::Builder::new()
                    .name("modern-compile".into())
                    .spawn_scoped(scope, work)
                    .expect("spawn a compile thread")
            })
            .collect();
        work();
        for handle in handles {
            if let Err(panic) = handle.join() {
                std::panic::resume_unwind(panic);
            }
        }
    });
    results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .expect("a compile result")
                .expect("every job ran")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Results come back in job order whichever thread ran what, and every
    /// job runs once.
    #[test]
    fn results_keep_job_order() {
        let jobs: Vec<Job<'_, usize>> = (0..40)
            .map(|i| -> Job<'_, usize> {
                Box::new(move || {
                    std::thread::sleep(std::time::Duration::from_micros(((40 - i) * 30) as u64));
                    i * 2
                })
            })
            .collect();
        assert_eq!(all(jobs), (0..40).map(|i| i * 2).collect::<Vec<_>>());
        assert_eq!(join(|| 1, || "two"), (1, "two"));
    }
}
