//! The loc meshes' shared geometry (the submission core's storage layout;
//! the cache policy, which mesh a slot holds and when the cache drops, stays
//! in `frame::resources`).
//!
//! Every loc mesh lives in a few large pages, each one vertex, one colour
//! and one index buffer, so a loc mesh is an offset (`base_vertex`, first
//! index) rather than its own buffers: a pass binds a page once and draws
//! every loc in it, where it used to bind three buffers per draw. Indices
//! stay 16-bit per mesh (drawn with the mesh's base vertex). A mesh larger
//! than a page gets a page of its own size.
//!
//! A mesh's ranges are rewritten in place while the new streams fit (a
//! dynamic loc's next pose keeps its size); otherwise they are freed and the
//! mesh moves. Frees take effect at the next frame's start, so no range a
//! draw recorded this frame is reused within it. A scene change frees
//! everything and keeps the pages.
//!
//! Writes are staged (performance plan P5): consecutive meshes land in
//! consecutive ranges of a page, so a mesh's bytes extend the open run of its
//! buffer and a run goes to the queue in one `write_buffer` when the next
//! write is elsewhere or at [`LocArena::flush`] (before the frame's
//! submission). wgpu makes a staging buffer per `write_buffer`, which cost
//! the first frame of a scene 3 of them per mesh (2,000-4,700 meshes). The
//! runs keep the writes' order per buffer, so the buffers end up as writing
//! each mesh at once leaves them.

use crate::frame::{ModelStreams, Vertex};

/// Vertices per page (24 MiB of vertices, 2 MiB of colours).
pub(crate) const PAGE_VERTICES: u32 = 1 << 19;
/// Index pairs per page (4 MiB of 16-bit indices). Indices are allocated in
/// pairs so every write starts and ends on 4 bytes.
pub(crate) const PAGE_INDEX_PAIRS: u32 = 1 << 20;

/// Free ranges of a page's units, sorted by start and coalesced.
#[derive(Clone, Debug)]
pub(crate) struct Ranges {
    free: Vec<(u32, u32)>,
    capacity: u32,
}

impl Ranges {
    pub(crate) fn new(capacity: u32) -> Self {
        Self {
            free: vec![(0, capacity)],
            capacity,
        }
    }

    /// The first free range of `len` units (first fit), taken.
    pub(crate) fn alloc(&mut self, len: u32) -> Option<u32> {
        if len == 0 {
            return Some(0);
        }
        let i = self.free.iter().position(|&(_, n)| n >= len)?;
        let (start, n) = self.free[i];
        if n == len {
            self.free.remove(i);
        } else {
            self.free[i] = (start + len, n - len);
        }
        Some(start)
    }

    /// Return `len` units at `start`.
    pub(crate) fn free(&mut self, start: u32, len: u32) {
        if len == 0 {
            return;
        }
        let i = self.free.partition_point(|&(s, _)| s < start);
        self.free.insert(i, (start, len));
        // Merge with the next, then the previous.
        if i + 1 < self.free.len() && self.free[i].0 + self.free[i].1 == self.free[i + 1].0 {
            self.free[i].1 += self.free[i + 1].1;
            self.free.remove(i + 1);
        }
        if i > 0 && self.free[i - 1].0 + self.free[i - 1].1 == self.free[i].0 {
            self.free[i - 1].1 += self.free[i].1;
            self.free.remove(i);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.free.clear();
        self.free.push((0, self.capacity));
    }

    /// Units in use.
    pub(crate) fn used(&self) -> u32 {
        self.capacity - self.free.iter().map(|&(_, n)| n).sum::<u32>()
    }
}

/// One page: the buffers and their free ranges.
pub(crate) struct Page {
    pub(crate) vertices: wgpu::Buffer,
    pub(crate) colours: wgpu::Buffer,
    pub(crate) indices: wgpu::Buffer,
    vertex_ranges: Ranges,
    index_ranges: Ranges,
}

/// Where one loc mesh lives: its page, its vertex range (the draws' base
/// vertex) and its index range (in pairs), each with the room it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Alloc {
    pub(crate) page: u16,
    pub(crate) vertex: u32,
    pub(crate) vertex_room: u32,
    pub(crate) index_pair: u32,
    pub(crate) index_pairs: u32,
}

impl Alloc {
    /// The mesh's first index in its page's index buffer.
    pub(crate) fn first_index(&self) -> u32 {
        self.index_pair * 2
    }
}

/// A run of staged bytes for one buffer of one page (`LocArena::flush`).
#[derive(Default)]
struct Run {
    page: u16,
    offset: u64,
    bytes: Vec<u8>,
}

/// The pages, the frees waiting for the next frame and the staged writes
/// (see the module docs).
#[derive(Default)]
pub(crate) struct LocArena {
    pub(crate) pages: Vec<Page>,
    pending: Vec<Alloc>,
    /// The open run of each buffer kind: vertices, colours, indices.
    runs: [Run; 3],
    /// Buffers created so far (three per page).
    pub(crate) buffers_created: u64,
}

/// A page's buffer of kind `kind` (0 vertices, 1 colours, 2 indices).
fn buffer(page: &Page, kind: usize) -> &wgpu::Buffer {
    match kind {
        0 => &page.vertices,
        1 => &page.colours,
        _ => &page.indices,
    }
}

impl LocArena {
    /// Stage `data` for `kind`'s buffer of `page` at `offset` (see the
    /// module docs).
    fn write(
        &mut self,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        page: u16,
        kind: usize,
        offset: u64,
        data: &[u8],
    ) {
        let run = &self.runs[kind];
        let extends = !run.bytes.is_empty()
            && run.page == page
            && run.offset + run.bytes.len() as u64 == offset;
        if !extends {
            self.flush_run(queue, kind);
            let run = &mut self.runs[kind];
            run.page = page;
            run.offset = offset;
        }
        self.runs[kind].bytes.extend_from_slice(data);
    }

    fn flush_run(&mut self, queue: &dyn rs910_gpu_device::uploads::Uploader, kind: usize) {
        let run = &mut self.runs[kind];
        if !run.bytes.is_empty() {
            queue.write_buffer(
                buffer(&self.pages[run.page as usize], kind),
                run.offset,
                &run.bytes,
            );
            run.bytes.clear();
        }
    }

    /// Write the staged runs (before the frame's submission).
    pub(crate) fn flush(&mut self, queue: &dyn rs910_gpu_device::uploads::Uploader) {
        for kind in 0..3 {
            self.flush_run(queue, kind);
        }
    }

    /// The start of a frame: the previous frames' frees take effect.
    pub(crate) fn begin_frame(&mut self) {
        for a in std::mem::take(&mut self.pending) {
            self.release(a);
        }
    }

    /// A new scene: every range free, the pages kept (the staged writes
    /// are dropped: nothing draws them).
    pub(crate) fn reset(&mut self) {
        self.pending.clear();
        for run in &mut self.runs {
            run.bytes.clear();
        }
        for p in &mut self.pages {
            p.vertex_ranges.clear();
            p.index_ranges.clear();
        }
    }

    fn release(&mut self, a: Alloc) {
        let p = &mut self.pages[a.page as usize];
        p.vertex_ranges.free(a.vertex, a.vertex_room);
        p.index_ranges.free(a.index_pair, a.index_pairs);
    }

    /// Free `a` from the next frame on.
    pub(crate) fn free(&mut self, a: Alloc) {
        self.pending.push(a);
    }

    /// Store `s` (in `at`'s ranges when it fits there, else in new ones;
    /// `at` is then freed). Returns where it lives.
    pub(crate) fn store(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        at: Option<Alloc>,
        s: &ModelStreams,
    ) -> Alloc {
        let vertices = s.vertices.len() as u32;
        let pairs = (s.indices.len() as u32).div_ceil(2);
        let a = match at {
            Some(a) if a.vertex_room >= vertices && a.index_pairs >= pairs => a,
            _ => {
                if let Some(old) = at {
                    self.free(old);
                }
                self.alloc(device, vertices, pairs)
            }
        };
        if !s.vertices.is_empty() {
            self.write(
                queue,
                a.page,
                0,
                u64::from(a.vertex) * std::mem::size_of::<Vertex>() as u64,
                bytemuck::cast_slice(&s.vertices),
            );
            self.write(
                queue,
                a.page,
                1,
                u64::from(a.vertex) * 4,
                bytemuck::cast_slice(&s.colours),
            );
        }
        if !s.indices.is_empty() {
            let indices = crate::frame::resources::padded_indices(s);
            self.write(
                queue,
                a.page,
                2,
                u64::from(a.index_pair) * 4,
                bytemuck::cast_slice(&indices),
            );
        }
        a
    }

    fn alloc(&mut self, device: &wgpu::Device, vertices: u32, pairs: u32) -> Alloc {
        for (k, p) in self.pages.iter_mut().enumerate() {
            let Some(vertex) = p.vertex_ranges.alloc(vertices) else {
                continue;
            };
            let Some(index_pair) = p.index_ranges.alloc(pairs) else {
                p.vertex_ranges.free(vertex, vertices);
                continue;
            };
            return Alloc {
                page: k as u16,
                vertex,
                vertex_room: vertices,
                index_pair,
                index_pairs: pairs,
            };
        }
        let (vc, ic) = (vertices.max(PAGE_VERTICES), pairs.max(PAGE_INDEX_PAIRS));
        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let mut page = Page {
            vertices: buffer(
                "modern loc vertices",
                u64::from(vc) * std::mem::size_of::<Vertex>() as u64,
                wgpu::BufferUsages::VERTEX,
            ),
            colours: buffer(
                "modern loc colours",
                u64::from(vc) * 4,
                wgpu::BufferUsages::VERTEX,
            ),
            indices: buffer(
                "modern loc indices",
                u64::from(ic) * 4,
                wgpu::BufferUsages::INDEX,
            ),
            vertex_ranges: Ranges::new(vc),
            index_ranges: Ranges::new(ic),
        };
        self.buffers_created += 3;
        let vertex = page.vertex_ranges.alloc(vertices).expect("a new page fits");
        let index_pair = page.index_ranges.alloc(pairs).expect("a new page fits");
        self.pages.push(page);
        log::debug!(
            "[modern] loc page {} created ({vc} vertices, {} indices)",
            self.pages.len() - 1,
            ic * 2
        );
        Alloc {
            page: (self.pages.len() - 1) as u16,
            vertex,
            vertex_room: vertices,
            index_pair,
            index_pairs: pairs,
        }
    }

    /// Vertices and indices in use over the pages.
    pub(crate) fn used(&self) -> (u64, u64) {
        self.pages.iter().fold((0, 0), |(v, i), p| {
            (
                v + u64::from(p.vertex_ranges.used()),
                i + 2 * u64::from(p.index_ranges.used()),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Ranges;

    /// Freed ranges coalesce, so a page that emptied takes a mesh of its
    /// whole size again (no fragmentation creep from moved meshes).
    #[test]
    fn freed_ranges_coalesce_back_to_the_whole_page() {
        let mut r = Ranges::new(100);
        let a = r.alloc(30).unwrap();
        let b = r.alloc(30).unwrap();
        let c = r.alloc(40).unwrap();
        assert_eq!(r.alloc(1), None);
        r.free(b, 30);
        assert_eq!(r.alloc(31), None);
        r.free(a, 30);
        assert_eq!(r.alloc(60), Some(0), "a and b merged");
        r.free(0, 60);
        r.free(c, 40);
        assert_eq!(r.used(), 0);
        assert_eq!(r.alloc(100), Some(0));
    }
}
