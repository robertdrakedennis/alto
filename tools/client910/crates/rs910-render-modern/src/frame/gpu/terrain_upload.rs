//! Owned terrain build and incremental GPU handoff. No incomplete texture or
//! vertex buffer is exposed to drawing; the classic floor remains until ready.
use super::terrain::LevelGpu;
use crate::scene_snapshot::SceneSnapshot;
use crate::streaming::UploadBudget;
use crate::terrain::{SceneKey, TerrainInputs, TerrainScene};
use rs910_jobs::assets::{AssetHandle, AssetPriority, AssetQueue};

const MILLIS_PER_SECOND: f64 = 1_000.0;
const FIRST_PART: usize = 0;
const NEXT_PART: usize = 1;
const EMPTY_BYTES: usize = 0;
const COPY_CHUNK_BYTES: usize = 256 * 1024;
const MIN_INDEX_BYTES: usize = 16;
const SINGLE_LAYER: u32 = 1;
const SINGLE_SAMPLE: u32 = 1;
const RGBA_BYTES: u32 = 4;
const WHITE_CHANNEL: u8 = u8::MAX;
const ZERO_ORIGIN: u32 = 0;

pub(crate) struct Uploaded {
    pub(crate) cpu: TerrainScene,
    pub(crate) layers: wgpu::TextureView,
    pub(crate) levels: Vec<Option<LevelGpu>>,
}

#[derive(Default)]
pub(crate) struct Stream {
    key: Option<SceneKey>,
    pending: Option<AssetHandle<TerrainScene>>,
    upload: Option<Upload>,
    ready: Option<TerrainScene>,
    incomplete: bool,
}

impl Stream {
    pub(crate) fn pending(&self) -> bool {
        self.incomplete
    }
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn prepare(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        jobs: &AssetQueue,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        budget: &UploadBudget,
    ) -> Option<Uploaded> {
        let key = SceneKey::of(snapshot);
        if self.key.as_ref() != Some(&key) {
            self.reset();
        }
        self.incomplete = true;
        if self.pending.is_none() && self.upload.is_none() && self.ready.is_none() {
            let (pack, materials) = (snapshot.pack?.clone(), snapshot.materials?.clone());
            let inputs = TerrainInputs::capture(snapshot);
            self.pending = jobs.spawn(AssetPriority::Scene, move || {
                let started = std::time::Instant::now();
                let cpu = TerrainScene::build_inputs(&pack, &materials, inputs, |ids| {
                    ids.iter()
                        .map(|&id| crate::terrain::layer_of(&pack, &materials, id))
                        .collect()
                });
                log::info!(
                    "[modern] terrain worker prepared in {:.1} ms",
                    started.elapsed().as_secs_f64() * MILLIS_PER_SECOND
                );
                cpu
            });
            self.pending.as_ref()?;
            self.key = Some(key);
        }
        if let Some(pending) = self.pending.as_ref() {
            match pending.try_recv() {
                Ok(Ok(cpu)) => {
                    self.pending = None;
                    self.ready = Some(cpu);
                }
                Ok(Err(panic)) => std::panic::resume_unwind(panic),
                Err(std::sync::mpsc::TryRecvError::Empty) => return None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.reset();
                    return None;
                }
            }
        }
        if self.upload.is_none() {
            let _upload = budget.begin(EMPTY_BYTES)?;
            self.upload = Some(Upload::new(device, self.ready.take()?));
        }
        if !self.upload.as_mut()?.advance(device, queue, budget) {
            return None;
        }
        let upload = self.upload.take()?;
        self.incomplete = false;
        Some(Uploaded {
            layers: upload.texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            }),
            cpu: upload.cpu,
            levels: upload.levels,
        })
    }
}

struct Upload {
    cpu: TerrainScene,
    texture: wgpu::Texture,
    layer: usize,
    mip: usize,
    levels: Vec<Option<LevelGpu>>,
    level: usize,
    vertices: bool,
    offset: usize,
}

impl Upload {
    fn new(device: &wgpu::Device, mut cpu: TerrainScene) -> Self {
        let size = crate::terrain::atlas::LAYER_SIZE;
        let mips = size.ilog2() + SINGLE_LAYER;
        if cpu.layer_texels.is_empty() {
            cpu.layer_texels.push(
                (ZERO_ORIGIN..mips)
                    .map(|mip| {
                        let side = (size >> mip).max(SINGLE_LAYER);
                        (
                            side,
                            side,
                            vec![WHITE_CHANNEL; (side * side * RGBA_BYTES) as usize],
                        )
                    })
                    .collect(),
            );
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("modern streamed terrain layers"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: cpu.layer_texels.len() as u32,
            },
            mip_level_count: mips,
            sample_count: SINGLE_SAMPLE,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let levels = cpu.levels.iter().map(|_| None).collect();
        Self {
            cpu,
            texture,
            levels,
            layer: FIRST_PART,
            mip: FIRST_PART,
            level: FIRST_PART,
            vertices: true,
            offset: EMPTY_BYTES,
        }
    }

    fn advance(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        budget: &UploadBudget,
    ) -> bool {
        while let Some(levels) = self.cpu.layer_texels.get(self.layer) {
            if let Some((width, height, pixels)) = levels.get(self.mip) {
                let Some(_upload) = budget.begin(pixels.len()) else {
                    return false;
                };
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.texture,
                        mip_level: self.mip as u32,
                        origin: wgpu::Origin3d {
                            x: ZERO_ORIGIN,
                            y: ZERO_ORIGIN,
                            z: self.layer as u32,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    pixels,
                    wgpu::TexelCopyBufferLayout {
                        offset: EMPTY_BYTES as u64,
                        bytes_per_row: Some(width * RGBA_BYTES),
                        rows_per_image: Some(*height),
                    },
                    wgpu::Extent3d {
                        width: *width,
                        height: *height,
                        depth_or_array_layers: SINGLE_LAYER,
                    },
                );
                self.mip += NEXT_PART;
            } else {
                self.layer += NEXT_PART;
                self.mip = FIRST_PART;
            }
        }
        while self.level < self.cpu.levels.len() {
            let Some(mesh) = self.cpu.levels[self.level]
                .as_ref()
                .filter(|mesh| !mesh.indices.is_empty())
            else {
                self.level += NEXT_PART;
                continue;
            };
            if self.levels[self.level].is_none() {
                let Some(_upload) = budget.begin(EMPTY_BYTES) else {
                    return false;
                };
                let buffer = |label, size, usage| {
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(label),
                        size: size as u64,
                        usage: usage | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    })
                };
                self.levels[self.level] = Some(LevelGpu {
                    vertices: buffer(
                        "modern terrain vertices",
                        std::mem::size_of_val(mesh.vertices.as_slice()),
                        wgpu::BufferUsages::VERTEX,
                    ),
                    indices: buffer(
                        "modern terrain indices",
                        std::mem::size_of_val(mesh.indices.as_slice()).max(MIN_INDEX_BYTES),
                        wgpu::BufferUsages::INDEX,
                    ),
                    selection: None,
                    count: EMPTY_BYTES as u32,
                });
            }
            let gpu = self.levels[self.level].as_ref().expect("terrain buffers");
            let (buffer, data): (&wgpu::Buffer, &[u8]) = if self.vertices {
                (&gpu.vertices, bytemuck::cast_slice(&mesh.vertices))
            } else {
                (&gpu.indices, bytemuck::cast_slice(&mesh.indices))
            };
            let end = (self.offset + COPY_CHUNK_BYTES).min(data.len());
            if end > self.offset {
                let Some(_upload) = budget.begin(end - self.offset) else {
                    return false;
                };
                queue.write_buffer(buffer, self.offset as u64, &data[self.offset..end]);
                self.offset = end;
            }
            if self.offset == data.len() {
                self.offset = EMPTY_BYTES;
                if self.vertices {
                    self.vertices = false;
                } else {
                    self.level += NEXT_PART;
                    self.vertices = true;
                }
            }
        }
        true
    }
}
