//! Installed terrain in the sun-shadow volumes. All levels cast independently
//! of the camera's colour selection and roof removal. Hidden-entity and tile
//! diagnostics still describe roof removal; entity casters join the gather in
//! `shadows::casters`.
use crate::frame::encoding::EncodeInputs;
use crate::frame::*;
use crate::terrain::SceneKey;
use wgpu::util::DeviceExt;

/// One installed level's full terrain index buffer.
pub(crate) struct CasterLevel {
    pub(crate) indices: wgpu::Buffer,
    pub(crate) count: u32,
}

/// See the module docs.
#[derive(Default)]
pub(crate) struct InteriorGpu {
    /// This frame's caster draws of roof-hidden entities (lane Q-FIN: among
    /// the frame's shadow-only draws, `shadows::casters`).
    pub(crate) hidden_draws: usize,
    /// The terrain scene the level buffers index.
    pub(crate) scene: Option<SceneKey>,
    pub(crate) levels: Vec<Option<CasterLevel>>,
    /// This frame's terrain draws: `(level, index count)`.
    pub(crate) terrain: Vec<(usize, u32)>,
    /// This frame's counts: roof-hidden entities and floor tiles.
    pub(crate) hidden_entities: usize,
    pub(crate) hidden_tiles: usize,
}

/// Frames between the interior log lines.
const INTERIOR_LOG_INTERVAL: u64 = 600;

impl ModernRenderer {
    /// Whether this frame logs the interior casters (the first frame and
    /// every [`INTERIOR_LOG_INTERVAL`]th).
    fn logs_interior(&self) -> bool {
        self.history.frame == 1 || self.history.frame.is_multiple_of(INTERIOR_LOG_INTERVAL)
    }

    /// What this frame's roof removal hid (`crate::shadows::interior::roof_hidden`):
    /// `None` without shadows.
    pub(crate) fn roof_hidden(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
    ) -> Option<crate::shadows::interior::RoofHidden> {
        self.frame_resources.shadow_frame.as_ref()?;
        // The hidden floor tiles only feed the interior log line.
        Some(crate::shadows::interior::roof_hidden(
            snapshot,
            self.logs_interior(),
        ))
    }

    /// Prepare full installed terrain for the sun, independently of colour
    /// visibility. Roof diagnostics are available only while shadows are on.
    pub(crate) fn prepare_interior(
        &mut self,
        device: &wgpu::Device,
        _queue: &dyn rs910_gpu_device::uploads::Uploader,
        hidden: Option<&crate::shadows::interior::RoofHidden>,
    ) {
        self.scene_resources.interior.terrain.clear();
        self.scene_resources.interior.hidden_entities = 0;
        self.scene_resources.interior.hidden_tiles = 0;
        let Some(hidden) = hidden else {
            return;
        };
        self.scene_resources.interior.hidden_entities = hidden.entities.len();
        self.scene_resources.interior.hidden_tiles = hidden.tile_count();
        if self.logs_interior() {
            log::info!(
                "[modern] interior: {} roof-hidden entities ({} caster draws) and {} floor tiles cast",
                self.scene_resources.interior.hidden_entities,
                self.scene_resources.interior.hidden_draws,
                self.scene_resources.interior.hidden_tiles
            );
        }
        // Every installed level casts under the floors' scenery setting.
        if !self.terrain_casts() {
            return;
        }
        let Some(scene) = self.scene_resources.terrain.scene.as_ref() else {
            return;
        };
        let i = &mut self.scene_resources.interior;
        if i.scene.as_ref() != Some(&scene.key) {
            i.scene = Some(scene.key.clone());
            i.levels.clear();
        }
        i.levels.resize_with(scene.cpu.levels.len(), || None);
        for (level, mesh) in scene.cpu.levels.iter().enumerate() {
            let Some(mesh) = mesh else {
                continue;
            };
            if scene.levels.get(level).is_none_or(Option::is_none) {
                continue;
            }
            let slot = &mut i.levels[level];
            if slot.is_none() && !mesh.indices.is_empty() {
                *slot = Some(CasterLevel {
                    indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("modern shadow terrain indices"),
                        contents: bytemuck::cast_slice(&mesh.indices),
                        usage: wgpu::BufferUsages::INDEX,
                    }),
                    count: mesh.indices.len() as u32,
                });
            }
            if let Some(h) = slot.as_ref().filter(|h| h.count > 0) {
                i.terrain.push((level, h.count));
            }
        }
    }

    /// The last frame's roof-hidden entity caster draws and terrain draws.
    #[must_use]
    pub fn interior_casters(&self) -> (usize, usize) {
        (
            self.scene_resources.interior.hidden_draws,
            self.scene_resources.interior.terrain.len(),
        )
    }
}

impl<'a> EncodeInputs<'a> {
    /// The roof-hidden terrain casters into one cascade of the caster pass
    /// (groups 0 and 2 set); leaves the caster pipeline set. (The
    /// roof-hidden entities are the frame's shadow-only draws.)
    pub(crate) fn encode_interior<'p>(&self, pass: &mut wgpu::RenderPass<'p>) {
        let i = self.interior;
        if !i.terrain.is_empty() {
            if let (Some(pipes), Some(scene)) =
                (self.terrain.pipes.as_ref(), self.terrain.scene.as_ref())
            {
                pass.set_pipeline(&pipes.shadow);
                pass.set_bind_group(1, &scene.bind, &[]);
                for &(level, count) in &i.terrain {
                    let (Some(Some(gpu)), Some(Some(hidden))) =
                        (scene.levels.get(level), i.levels.get(level))
                    else {
                        continue;
                    };
                    pass.set_vertex_buffer(0, gpu.vertices.slice(..));
                    pass.set_index_buffer(hidden.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..count, 0, 0..1);
                }
            }
            pass.set_pipeline(&self.shadow.pipeline);
        }
    }
}
