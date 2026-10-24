//! Buffer uploads through one staging belt (code-quality programme Phase 6,
//! §6 "`StagingBelt` for uploads").
//!
//! `wgpu::Queue::write_buffer` allocates a staging buffer per call (wgpu-core
//! `StagingBuffer::new`), which dominated the per-frame cost of the faithful
//! renderer's ~180 buffer writes. [`Device`](crate::gpu_device::Device)'s
//! [`Uploader`] implementation copies the bytes into a reused
//! `wgpu::util::StagingBelt` chunk instead and records the copy in an upload
//! encoder that [`Device::submit`](crate::gpu_device::Device::submit) submits
//! first, ahead of the submission's own command buffers.
//!
//! Same semantics as `write_buffer`: a write issued before a submission is
//! in the buffer for every command of that submission (however early they
//! were encoded), and writes land in call order. This holds because every
//! submission using the belt goes through `Device::submit` (including
//! modern sky and probe bakes), and a buffer is written through one
//! mechanism only (the queue's own pending writes run before any command
//! buffer of a submission, so mixing the two on one buffer could reorder).
//! `wgpu::Queue` implements [`Uploader`] as its plain `write_buffer`, which
//! test harnesses with a bare device and queue keep using.

/// Where a renderer writes buffer contents: `wgpu::Queue::write_buffer`
/// semantics (module docs).
pub trait Uploader: Sync {
    /// Write `data` at `offset` of `buffer` for the next submission (an
    /// empty write does nothing, as `Queue::write_buffer`).
    fn write_buffer(&self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]);
    /// The queue, for what does not go through the belt (texture writes).
    fn queue(&self) -> &wgpu::Queue;
    /// Texture writes keep the queue's pending-write path.
    fn write_texture(
        &self,
        texture: wgpu::TexelCopyTextureInfo<'_>,
        data: &[u8],
        layout: wgpu::TexelCopyBufferLayout,
        size: wgpu::Extent3d,
    ) {
        self.queue().write_texture(texture, data, layout, size);
    }
    /// Submit the commands after this uploader's pending writes. A device
    /// drains its belt; a bare queue retains its ordinary write semantics.
    fn submit_uploads(&self, commands: Vec<wgpu::CommandBuffer>) -> wgpu::SubmissionIndex {
        self.queue().submit(commands)
    }
}

impl Uploader for wgpu::Queue {
    fn write_buffer(&self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        wgpu::Queue::write_buffer(self, buffer, offset, data);
    }
    fn queue(&self) -> &wgpu::Queue {
        self
    }
}

impl<T: Uploader + Send + ?Sized> Uploader for std::sync::Arc<T> {
    fn write_buffer(&self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        self.as_ref().write_buffer(buffer, offset, data);
    }
    fn queue(&self) -> &wgpu::Queue {
        self.as_ref().queue()
    }
    fn submit_uploads(&self, commands: Vec<wgpu::CommandBuffer>) -> wgpu::SubmissionIndex {
        self.as_ref().submit_uploads(commands)
    }
}

/// The belt and the upload encoder of the writes since the last submission.
pub struct Uploads {
    belt: wgpu::util::StagingBelt,
    encoder: Option<wgpu::CommandEncoder>,
}

/// Belt chunk size: a frame writes ~0.3 MB in ~180 writes online.
const CHUNK: u64 = 1 << 20;

impl Uploads {
    /// An empty set of uploads whose belt allocates on `device`.
    pub fn new(device: wgpu::Device) -> Self {
        Self {
            belt: wgpu::util::StagingBelt::new(device, CHUNK),
            encoder: None,
        }
    }

    /// Stage `data` for `buffer` at `offset` (the copy is recorded in the
    /// upload encoder).
    pub fn write(
        &mut self,
        device: &wgpu::Device,
        buffer: &wgpu::Buffer,
        offset: u64,
        data: &[u8],
    ) {
        let Some(size) = wgpu::BufferSize::new(data.len() as u64) else {
            return;
        };
        let encoder = self.encoder.get_or_insert_with(|| {
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("buffer uploads"),
            })
        });
        self.belt
            .write_buffer(encoder, buffer, offset, size)
            .copy_from_slice(data);
    }

    /// The staged copies as a command buffer to submit before the
    /// submission's own (none when nothing was written); the belt's chunks
    /// are closed for it.
    pub fn take(&mut self) -> Option<wgpu::CommandBuffer> {
        let encoder = self.encoder.take()?;
        self.belt.finish();
        Some(encoder.finish())
    }

    #[cfg(test)]
    pub(crate) fn has_pending(&self) -> bool {
        self.encoder.is_some()
    }

    /// After the submission: the used chunks return to the belt once the
    /// GPU is done with them.
    pub fn recall(&mut self) {
        self.belt.recall();
    }
}

#[cfg(test)]
mod tests {
    use super::Uploads;

    fn read(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let bytes = slice.get_mapped_range().expect("mapped range").to_vec();
        buffer.unmap();
        bytes
    }

    /// `Queue::write_buffer` semantics through the belt: writes land in call
    /// order before every command of the submission they precede, even a
    /// command encoded before the write, and each submission sees only the
    /// writes issued before it.
    #[test]
    #[ignore = "gpu: needs a desktop GPU adapter"]
    fn staged_writes_land_before_the_submission_in_call_order() {
        let (device, queue) = crate::test_support::require_gpu();
        let usage = wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        let target = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage,
            mapped_at_creation: false,
        });
        let readback = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        };
        let (first, second) = (readback("first"), readback("second"));
        let mut uploads = Uploads::new(device.clone());
        let submit = |uploads: &mut Uploads, commands: wgpu::CommandBuffer| {
            let staged = uploads.take();
            queue.submit(staged.into_iter().chain(Some(commands)));
            uploads.recall();
        };
        // The copy is encoded before the writes; the writes still precede it.
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&target, 0, &first, 0, 16);
        uploads.write(&device, &target, 0, &[1; 16]);
        uploads.write(&device, &target, 4, &[2; 8]);
        uploads.write(&device, &target, 12, &[]);
        submit(&mut uploads, encoder.finish());
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&target, 0, &second, 0, 16);
        uploads.write(&device, &target, 8, &[3; 4]);
        submit(&mut uploads, encoder.finish());
        assert_eq!(
            read(&device, &first),
            [[1; 4], [2; 4], [2; 4], [1; 4]].concat()
        );
        assert_eq!(
            read(&device, &second),
            [[1; 4], [2; 4], [3; 4], [1; 4]].concat()
        );
        // Nothing staged: no extra command buffer.
        assert!(uploads.take().is_none());
    }

    /// A prepared frame that gets no drawable leaves its writes queued. A
    /// later submission sees those cache writes and later overlapping writes
    /// before even commands encoded before preparation.
    #[test]
    #[ignore = "gpu: needs a desktop GPU adapter"]
    fn device_uploads_survive_a_skipped_frame_and_precede_internal_submissions() {
        use super::Uploader;
        use crate::gpu_device::{Device, DeviceOptions};
        const BUFFER_BYTES: u64 = 16;
        const OVERWRITE_OFFSET: u64 = 4;
        const OVERWRITE_BYTES: usize = 8;
        const INITIAL_BYTE: u8 = 17;
        const OVERWRITE_BYTE: u8 = 34;
        const CANVAS: (u32, u32) = (1, 1);
        const OCCLUDED_ATTEMPTS: u8 = 64;
        let gpu = pollster::block_on(Device::headless(
            CANVAS,
            DeviceOptions::default(),
            wgpu::Features::empty(),
        ))
        .expect("headless device");
        let target = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("skipped frame cached resource"),
            size: BUFFER_BYTES,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("later internal submission readback"),
            size: BUFFER_BYTES,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&target, 0, &readback, 0, BUFFER_BYTES);
        let uploads: &dyn Uploader = &gpu;
        uploads.write_buffer(&target, 0, &[INITIAL_BYTE; BUFFER_BYTES as usize]);
        assert!(matches!(
            gpu.acquire(),
            wgpu::CurrentSurfaceTexture::Occluded
        ));
        // No submit occurred, like a failed late drawable acquisition.
        uploads.write_buffer(
            &target,
            OVERWRITE_OFFSET,
            &[OVERWRITE_BYTE; OVERWRITE_BYTES],
        );
        uploads.submit_uploads(vec![encoder.finish()]);
        let mut expected = vec![INITIAL_BYTE; BUFFER_BYTES as usize];
        expected[OVERWRITE_OFFSET as usize..OVERWRITE_OFFSET as usize + OVERWRITE_BYTES]
            .fill(OVERWRITE_BYTE);
        assert_eq!(read(&gpu.device, &readback), expected);
        for attempt in 0..OCCLUDED_ATTEMPTS {
            let next_byte = INITIAL_BYTE.wrapping_add(attempt);
            uploads.write_buffer(&target, 0, &[next_byte; BUFFER_BYTES as usize]);
            uploads.write_buffer(
                &target,
                OVERWRITE_OFFSET,
                &[OVERWRITE_BYTE; OVERWRITE_BYTES],
            );
            assert!(gpu.has_pending_uploads());
            assert!(matches!(
                gpu.acquire(),
                wgpu::CurrentSurfaceTexture::Occluded
            ));
            // Only the uploads are submitted on acquisition failure. The pending
            // copy set is empty between attempts, however long occlusion lasts.
            gpu.submit(std::iter::empty());
            assert!(!gpu.has_pending_uploads());
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            encoder.copy_buffer_to_buffer(&target, 0, &readback, 0, BUFFER_BYTES);
            // A bare-queue copy cannot drain the belt: it observes that the
            // upload-only submission applied the newest bytes in call order.
            gpu.queue.submit([encoder.finish()]);
            expected.fill(next_byte);
            expected[OVERWRITE_OFFSET as usize..OVERWRITE_OFFSET as usize + OVERWRITE_BYTES]
                .fill(OVERWRITE_BYTE);
            assert_eq!(read(&gpu.device, &readback), expected);
        }
    }
}
