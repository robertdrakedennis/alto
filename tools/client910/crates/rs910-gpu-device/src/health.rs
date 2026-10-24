//! What a GPU device reports about itself: whether it was lost and the
//! errors it raised on the side.
//!
//! wgpu panics on a lost device and on any error nothing catches. The client
//! must not: a lost device (a driver reset, an unplugged adapter) and an
//! allocation that failed are reasons to fall back to a safer toolkit and
//! carry on, as the original client does when its renderer fails. The
//! callbacks installed by [`Health::watch`] only record; the shell asks
//! [`Health::take_fault`] once per frame and acts.
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Something that went wrong on the device that its owner has to answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The device is gone: nothing created on it works any more.
    Lost {
        /// Why the device went (`Destroyed` or `Unknown`).
        reason: String,
        /// The driver's message.
        message: String,
    },
    /// An allocation failed.
    NoMemory,
    /// The device failed in a way the API does not name (a system limit, a
    /// driver fault).
    Internal(String),
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fault::Lost { reason, message } => write!(f, "device lost ({reason}): {message}"),
            Fault::NoMemory => f.write_str("out of GPU memory"),
            Fault::Internal(description) => write!(f, "internal GPU error: {description}"),
        }
    }
}

/// The recorded state of one device (see the module docs).
#[derive(Default)]
pub struct Health {
    lost: Mutex<Lost>,
    faults: Mutex<Vec<Fault>>,
    /// Validation errors seen: bugs in what the client asked of the device,
    /// logged and counted, never fatal.
    validation: AtomicU32,
}

/// The loss of a device and whether it was handed out yet.
#[derive(Default)]
struct Lost {
    fault: Option<Fault>,
    taken: bool,
}

impl Health {
    /// Record `device`'s loss and uncaptured errors instead of letting wgpu
    /// panic.
    pub fn watch(device: &wgpu::Device) -> Arc<Self> {
        let health = Arc::new(Self::default());
        let lost = Arc::clone(&health);
        device.set_device_lost_callback(move |reason, message| {
            lost.record_loss(format!("{reason:?}"), message);
        });
        let errors = Arc::clone(&health);
        device.on_uncaptured_error(Arc::new(move |error| errors.record_error(&error)));
        health
    }

    fn record_loss(&self, reason: String, message: String) {
        log::error!("[gpu] device lost ({reason}): {message}");
        self.lost.lock().expect("lost lock").fault = Some(Fault::Lost { reason, message });
    }

    fn record_error(&self, error: &wgpu::Error) {
        match error {
            wgpu::Error::Internal { description, .. } => {
                log::error!("[gpu] internal error: {description}");
                self.faults
                    .lock()
                    .expect("faults lock")
                    .push(Fault::Internal(description.clone()));
            }
            wgpu::Error::Validation { description, .. } => {
                // Logged for the first few and every hundredth after.
                let seen = self.validation.fetch_add(1, Ordering::Relaxed);
                if seen < 8 || seen.is_multiple_of(100) {
                    log::error!("[gpu] validation error #{}: {description}", seen + 1);
                }
            }
            _ => {
                log::error!("[gpu] out of memory");
                self.faults
                    .lock()
                    .expect("faults lock")
                    .push(Fault::NoMemory);
            }
        }
    }

    /// Whether the device was lost.
    #[must_use]
    pub fn is_lost(&self) -> bool {
        self.lost.lock().expect("lost lock").fault.is_some()
    }

    /// How many validation errors the device raised.
    #[must_use]
    pub fn validation_errors(&self) -> u32 {
        self.validation.load(Ordering::Relaxed)
    }

    /// The next fault to answer, each handed out once: the loss first (the
    /// errors of a dead device say nothing more), then the errors in the
    /// order they came.
    pub fn take_fault(&self) -> Option<Fault> {
        {
            let mut lost = self.lost.lock().expect("lost lock");
            if let Some(fault) = lost.fault.clone() {
                if lost.taken {
                    return None;
                }
                lost.taken = true;
                self.faults.lock().expect("faults lock").clear();
                return Some(fault);
            }
        }
        let mut faults = self.faults.lock().expect("faults lock");
        (!faults.is_empty()).then(|| faults.remove(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A destroyed device is reported lost once, with its reason, and the
    /// errors of the dead device add nothing; a device that is fine reports
    /// nothing.
    #[test]
    #[ignore = "needs a GPU adapter"]
    fn a_destroyed_device_is_reported_lost_once() {
        let (device, _queue) = crate::test_support::require_gpu();
        let health = Health::watch(&device);
        assert!(!health.is_lost());
        assert_eq!(health.take_fault(), None);
        device.destroy();
        // The loss is reported when the device is next polled.
        let _ = device.poll(wgpu::PollType::Poll);
        assert!(health.is_lost());
        let fault = health.take_fault().expect("the loss");
        assert!(
            matches!(&fault, Fault::Lost { reason, .. } if reason == "Destroyed"),
            "{fault:?}"
        );
        assert_eq!(health.take_fault(), None, "handed out once");
        assert!(health.is_lost());
    }

    /// An error nothing catches is recorded, not a panic: a buffer that
    /// cannot exist raises a validation error that is counted.
    #[test]
    #[ignore = "needs a GPU adapter"]
    fn uncaptured_errors_are_recorded_instead_of_panicking() {
        let (device, _queue) = crate::test_support::require_gpu();
        let health = Health::watch(&device);
        let _ = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::MAP_WRITE,
            mapped_at_creation: false,
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        assert_eq!(health.validation_errors(), 1);
        assert!(!health.is_lost());
        assert_eq!(health.take_fault(), None);
    }
}
