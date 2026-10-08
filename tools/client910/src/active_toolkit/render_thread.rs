//! One in-flight owned frame. Completion returns every resource before its
//! main-thread slot can be reused; the channel cannot accumulate frame work.
use rs910_render_gpu::render::Composition;
use rs910_render_modern::frame::{ModernRenderer, PrepareTarget};
use rs910_scene::scene_snapshot::owned::OwnedSnapshot;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

const IN_FLIGHT_FRAMES: usize = 1;
const MILLIS_PER_SECOND: f64 = 1_000.0;

pub(super) struct Job {
    pub device: crate::gpu_device::Device,
    pub modern: Box<ModernRenderer>,
    pub composition: Composition,
    pub snapshot: Option<OwnedSnapshot>,
    pub frame: u64,
    pub cycle: i32,
    pub queued: Instant,
    pub profile: rs910_core::profile::ThreadContext,
}
pub(super) struct Completed {
    pub job: Job,
    pub result: anyhow::Result<()>,
    pub profile: rs910_core::profile::ThreadRecording,
}
pub(super) struct RenderThread {
    send: Option<SyncSender<Job>>,
    done: Receiver<Completed>,
    worker: Option<JoinHandle<()>>,
}
impl RenderThread {
    pub fn new() -> Self {
        let (send, jobs) = mpsc::sync_channel::<Job>(IN_FLIGHT_FRAMES);
        let (complete, done) = mpsc::sync_channel(IN_FLIGHT_FRAMES);
        let worker = thread::Builder::new().name("scene render".into()).spawn(move || {
            while let Ok(mut job) = jobs.recv() {
                let started = Instant::now();
                rs910_core::profile::begin_thread(job.profile);
                let result = rs910_core::profile::scope!("worker frame", draw(&mut job));
                let profile = rs910_core::profile::end_thread();
                log::trace!("[render] frame {} cycle {} queue {:.3} ms worker {:.3} ms completion {:.3} ms clock_age {} ms main_frame_to_completion {:.3} ms",
                    job.frame, job.cycle, started.duration_since(job.queued).as_secs_f64() * MILLIS_PER_SECOND,
                    started.elapsed().as_secs_f64() * MILLIS_PER_SECOND, job.queued.elapsed().as_secs_f64() * MILLIS_PER_SECOND, job.snapshot.as_ref().map_or(0, |snapshot| snapshot.captured_age().as_millis()), job.profile.elapsed_since_frame_start().map_or(-1.0, |elapsed| elapsed.as_secs_f64() * MILLIS_PER_SECOND));
                if complete.send(Completed { job, result, profile }).is_err() { break; }
            }
        }).expect("start scene render thread");
        Self {
            send: Some(send),
            done,
            worker: Some(worker),
        }
    }
    pub fn submit(&self, job: Job) {
        self.send
            .as_ref()
            .expect("active render thread")
            .send(job)
            .unwrap_or_else(|_| panic!("scene render thread stopped"));
    }
    pub fn receive(&self) -> Completed {
        self.done.recv().expect("scene render thread completion")
    }
    pub fn poll(&self) -> Option<Completed> {
        match self.done.try_recv() {
            Ok(done) => Some(done),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => panic!("scene render thread stopped"),
        }
    }
}
impl Drop for RenderThread {
    fn drop(&mut self) {
        drop(self.send.take());
        if let Some(worker) = self.worker.take() {
            if let Err(panic) = worker.join() {
                if !thread::panicking() {
                    std::panic::resume_unwind(panic);
                }
            }
        }
    }
}
fn draw(job: &mut Job) -> anyhow::Result<()> {
    let Some(snapshot) = job.snapshot.as_ref() else {
        return job.composition.present(&mut job.device);
    };
    let snapshot = snapshot.snapshot();
    let mut prepared = job.composition.viewport.as_ref().and_then(|viewport| {
        rs910_core::profile::scope!(
            "worker prepare",
            job.modern.prepare_frame(
                PrepareTarget {
                    device: &job.device.device,
                    queue: &job.device,
                    size: job.composition.size,
                    rect: viewport.rect,
                    clip: viewport.clip,
                },
                &snapshot
            )
        )
    });
    let mut recorded = false;
    let result = rs910_core::profile::scope!(
        "worker encode and submit",
        job.composition.draw(&mut job.device, |target| {
            prepared.take().map_or_else(Vec::new, |prepared| {
                recorded = true;
                job.modern
                    .record_frame(target.device, target.encoder, target.view, prepared)
            })
        })
    );
    if recorded {
        job.modern.frame_submitted();
    } else if let Some(prepared) = prepared {
        job.modern.frame_skipped(prepared);
        job.device.submit(std::iter::empty());
    }
    result.map(|_| ())
}
