//! Portable indexed draw commands shared by every scene pass. Three reusable
//! argument buffers separate consecutive submissions; unsupported first-instance
//! hardware uses the same ordered direct packets.

use crate::frame::Draw;

const BUFFERED_SUBMISSIONS: usize = 3;
const NEXT_BUFFER: usize = 1;
const SINGLE_INSTANCE: u32 = 1;
const BUFFER_OFFSET: u64 = 0;

#[derive(Default)]
pub(crate) struct DrawCommands {
    buffers: [Option<(wgpu::Buffer, u64)>; BUFFERED_SUBMISSIONS],
    current: usize,
    arguments: Vec<wgpu::util::DrawIndexedIndirectArgs>,
}

impl DrawCommands {
    #[cfg(test)]
    pub(crate) fn arguments(&self) -> &[wgpu::util::DrawIndexedIndirectArgs] {
        &self.arguments
    }

    pub(crate) fn buffer(&self) -> Option<&wgpu::Buffer> {
        if self.arguments.is_empty() {
            return None;
        }
        self.buffers[self.current]
            .as_ref()
            .map(|(buffer, _)| buffer)
    }

    pub(crate) fn prepare<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        draws: impl Iterator<Item = &'a mut Draw>,
    ) {
        self.arguments.clear();
        if !device
            .features()
            .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE)
        {
            for draw in draws {
                draw.indirect = None;
            }
            return;
        }
        for draw in draws {
            draw.indirect = Some(self.arguments.len() as u32);
            self.arguments.push(wgpu::util::DrawIndexedIndirectArgs {
                index_count: draw.count,
                instance_count: SINGLE_INSTANCE,
                first_index: draw.first_index,
                base_vertex: draw.geometry.base_vertex(),
                first_instance: draw.instance,
            });
        }
        if self.arguments.is_empty() {
            return;
        }
        self.current = (self.current + NEXT_BUFFER) % BUFFERED_SUBMISSIONS;
        let need = std::mem::size_of_val(self.arguments.as_slice()) as u64;
        let slot = &mut self.buffers[self.current];
        if slot.as_ref().is_none_or(|(_, capacity)| *capacity < need) {
            let capacity = need.next_power_of_two();
            *slot = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("modern draw commands"),
                    size: capacity,
                    usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                capacity,
            ));
        }
        queue.write_buffer(
            &slot.as_ref().expect("draw command buffer").0,
            BUFFER_OFFSET,
            bytemuck::cast_slice(&self.arguments),
        );
    }
}
