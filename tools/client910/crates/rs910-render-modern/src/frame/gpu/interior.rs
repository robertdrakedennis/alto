//! The interior sun shadows' GPU half (renderer plan §4(m);
//! the CPU half is [`crate::shadows::interior`]): the roof-hidden terrain tiles'
//! indices, drawn into the sun's cascades only (never the colour, depth,
//! normal or reflection passes). The roof-hidden entities join the
//! off-screen caster gather (`shadows::casters`).
use crate::frame::encoding::EncodeInputs;
use crate::frame::*;
use crate::terrain::SceneKey;

/// One level's roof-hidden terrain tiles: an index buffer over the level's
/// terrain vertices, re-selected when the hidden set changes.
pub(crate) struct HiddenLevel {
    pub(crate) indices: wgpu::Buffer,
    pub(crate) capacity: u64,
    pub(crate) count: u32,
    pub(crate) selection: crate::draw::FloorSelection,
}

/// See the module docs.
#[derive(Default)]
pub(crate) struct InteriorGpu {
    /// This frame's caster draws of roof-hidden entities (lane Q-FIN: among
    /// the frame's shadow-only draws, `shadows::casters`).
    pub(crate) hidden_draws: usize,
    /// The terrain scene the level buffers index.
    pub(crate) scene: Option<SceneKey>,
    pub(crate) levels: Vec<Option<HiddenLevel>>,
    /// This frame's terrain draws: `(level, index count)`.
    pub(crate) terrain: Vec<(usize, u32)>,
    /// Frames with the check on, and mismatching ones.
    pub(crate) frames: u64,
    pub(crate) mismatches: u64,
    /// This frame's counts: roof-hidden entities and floor tiles.
    pub(crate) hidden_entities: usize,
    pub(crate) hidden_tiles: usize,
}

impl ModernRenderer {
    /// What this frame's roof removal hid (`crate::shadows::interior::roof_hidden`):
    /// `None` without shadows.
    pub(crate) fn roof_hidden(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
    ) -> Option<crate::shadows::interior::RoofHidden> {
        self.frame_resources.shadow_frame.as_ref()?;
        Some(crate::shadows::interior::roof_hidden(snapshot))
    }

    /// The frame's roof-hidden terrain casters (after the draw list's
    /// entities, the terrain's selection and the caster gather, which takes
    /// the roof-hidden entities, `shadows::casters`): nothing without
    /// `hidden` or when nothing is hidden.
    pub(crate) fn prepare_interior(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
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
        if self.history.frame == 1 || self.history.frame.is_multiple_of(600) {
            log::info!(
                "[modern] interior: {} roof-hidden entities ({} caster draws) and {} floor tiles cast",
                self.scene_resources.interior.hidden_entities,
                self.scene_resources.interior.hidden_draws,
                self.scene_resources.interior.hidden_tiles
            );
        }
        if crate::modern_debug_flags::flags().check {
            let (ok, report) = crate::shadows::interior::check(snapshot, hidden);
            self.scene_resources.interior.frames += 1;
            if !ok {
                self.scene_resources.interior.mismatches += 1;
                log::warn!(
                    "[modern] interior check: {report} ({} mismatching frames)",
                    self.scene_resources.interior.mismatches
                );
            } else if self.scene_resources.interior.frames == 1
                || self.scene_resources.interior.frames.is_multiple_of(600)
            {
                log::info!("[modern] interior check: {report}");
            }
        }
        // The terrain's hidden tiles (the floors' scenery setting).
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
        i.levels
            .resize_with(hidden.tiles.len().max(i.levels.len()), || None);
        for (level, selection) in hidden.tiles.iter().enumerate() {
            let (Some(selection), Some(Some(mesh))) = (selection, scene.cpu.levels.get(level))
            else {
                continue;
            };
            if scene.levels.get(level).is_none_or(Option::is_none) {
                continue;
            }
            let slot = &mut i.levels[level];
            if slot.as_ref().is_none_or(|h| &h.selection != selection) {
                let indices = mesh.select(selection);
                let need = (indices.len().max(4) * 4) as u64;
                let reuse = slot
                    .take()
                    .filter(|h| h.capacity >= need)
                    .map(|h| (h.indices, h.capacity));
                let (buffer, capacity) = reuse.unwrap_or_else(|| {
                    let capacity = need.next_power_of_two();
                    (
                        device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("modern interior terrain indices"),
                            size: capacity,
                            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        }),
                        capacity,
                    )
                });
                if !indices.is_empty() {
                    queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&indices));
                }
                *slot = Some(HiddenLevel {
                    indices: buffer,
                    capacity,
                    count: indices.len() as u32,
                    selection: selection.clone(),
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
