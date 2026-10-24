//! One installation allowance for streamed resources on the GPU owner.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const UPLOAD_TIME: Duration = Duration::from_millis(2);
const UPLOAD_BYTES: usize = 2 * 1024 * 1024;
const NO_UPLOAD_BYTES: usize = 0;

#[derive(Default)]
struct Usage {
    time: Duration,
    bytes: usize,
}

#[derive(Clone, Default)]
pub(crate) struct UploadBudget(Arc<Mutex<Usage>>);

impl UploadBudget {
    pub(crate) fn reset(&self) {
        *self.0.lock().expect("upload budget") = Usage::default();
    }
    pub(crate) fn available(&self) -> bool {
        let usage = self.0.lock().expect("upload budget");
        usage.time < UPLOAD_TIME && usage.bytes < UPLOAD_BYTES
    }
    pub(crate) fn begin_installation(&self) -> Option<UploadScope> {
        self.begin(NO_UPLOAD_BYTES)
    }
    pub(crate) fn begin(&self, bytes: usize) -> Option<UploadScope> {
        let mut usage = self.0.lock().expect("upload budget");
        if usage.time >= UPLOAD_TIME
            || usage.bytes >= UPLOAD_BYTES
            || (usage.bytes != NO_UPLOAD_BYTES && usage.bytes.saturating_add(bytes) > UPLOAD_BYTES)
        {
            return None;
        }
        usage.bytes = usage.bytes.saturating_add(bytes);
        Some(UploadScope {
            budget: self.clone(),
            started: Instant::now(),
        })
    }
}

pub(crate) struct UploadScope {
    budget: UploadBudget,
    started: Instant,
}
impl Drop for UploadScope {
    fn drop(&mut self) {
        self.budget.0.lock().expect("upload budget").time += self.started.elapsed();
    }
}

/// A resource's first residency, independent of camera visibility and frame rate.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ResidencyFade(Option<i64>);
const RESIDENCY_FADE_MILLIS: i64 = 250;
pub(crate) const FULLY_RESIDENT: f32 = 1.0;
const SMOOTHSTEP_LINEAR: f32 = 3.0;
const SMOOTHSTEP_QUADRATIC: f32 = 2.0;
impl ResidencyFade {
    pub(crate) fn new(now: i64, streamed: bool) -> Self {
        Self(streamed.then_some(now))
    }
    pub(crate) fn weight(self, now: i64) -> f32 {
        let Some(start) = self.0 else {
            return FULLY_RESIDENT;
        };
        let t = (now.saturating_sub(start) as f32 / RESIDENCY_FADE_MILLIS as f32)
            .clamp(0.0, FULLY_RESIDENT);
        t * t * (SMOOTHSTEP_LINEAR - SMOOTHSTEP_QUADRATIC * t)
    }
}
