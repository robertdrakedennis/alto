//! The per-square ambient capture on the GPU (the model and its stand-ins:
//! [`crate::lighting::ambient`], the schedule, cache and blend:
//! [`crate::lighting::ambient_schedule`]).
//!
//! With the verified look ([`crate::settings::LookMode::captured_ambient`])
//! each frame:
//!
//! 1. **Harvest.** The faces whose readback finished are handed to the
//!    schedule; the sixth face of a square goes to a worker thread that
//!    converts the texels, projects them and queues the block; the blocks
//!    the workers queued enter the schedule's cache and start their blend.
//! 2. **Request.** The map squares the scene window touches are requested
//!    under the environment's key once it has held for two frames (the sky
//!    textures that are ready are part of it: a sky that arrives later asks
//!    for every square again).
//! 3. **Capture.** At most one face: the schedule names it, the frame's
//!    scene draws record for it (the probe captures' machinery: floors and
//!    locs culled to the face) with the face's frame block, and
//!    `encode_ambient` renders it into the face target in the probes unit
//!    (the frame's sky first, shaded over the face at the capture's exposure
//!    offset: [`crate::frame::gpu::sky_cube`]) and copies it into a readback
//!    buffer. The buffer is
//!    mapped after the frame's submission ([`ModernRenderer::map_ambient_readback`])
//!    and read a frame or more later; nothing waits on the GPU.
//! 4. **Upload.** The table of cells around the camera (each cell the block
//!    its square shows now) is written when it changed.
//!
//! The shader picks the cell under the fragment's position
//! (`lighting/ambient.wgsl`), so a model spanning two squares shades with
//! the block under each fragment (stand-in: the reference binds one block per
//! node), and every level of a square shows the same block (stand-in: the
//! reference writes it to the levels up to the camera's plus one near the
//! camera square, the ground level elsewhere).
//!
//! Capture state (stand-ins where the reference's is not known): the
//! capture draws the frame's scene with the flat ambient, no sun shadows
//! and no SSAO, at the near scene's model detail (the reference forces the
//! low detail), with the point lights the frame has (the reference turns
//! their shadows off); the far scene and the water are not drawn.

use crate::frame::encoding::EncodeInputs;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use wgpu::util::DeviceExt;

use crate::frame::gpu::probes::{
    capture_frame, cull_faces, face_windings, CapturePipes, SceneCapture, SLOT,
};
use crate::frame::*;
use crate::lighting::ambient::{
    face_from_rgba16f, project, Faces, Irradiance, CAPTURE_EXPOSURE, CAPTURE_OFFSET, EYE_CLEARANCE,
    FACE_FAR, FACE_NEAR, FACE_RES,
};
use crate::lighting::ambient_schedule::{
    square_of, window_squares, Key, Projection, Schedule, Square, Table, Ticket, SQUARE_SIZE,
};

/// The table's binding in the lights' group.
pub(crate) const BINDING: u32 = 13;

/// The vec4s of a cell in the table buffer (the block's seven, padded).
const CELL_STRIDE: usize = 8;

/// The readback buffers a capture cycles through (one face is captured a
/// frame; a face's buffer is free again once it has been read).
const READBACKS: usize = 3;

/// The frames the environment's key must hold before the squares are
/// requested under it (a transition changes it every frame).
const SETTLE_FRAMES: u32 = 2;

/// A capture skips what subtends less than about one texel of the face.
const SMALL_ANGLE: f32 = std::f32::consts::FRAC_PI_2 / FACE_RES as f32;

/// The spacing of the terrain heights sampled for a square's bounds (tiles).
const BOUNDS_STEP: usize = 4;

/// A face's readback bytes: RGBA half floats, rows of `FACE_RES` texels.
type Raw = Vec<u8>;

/// The table buffer: every cell the default block until written.
pub(crate) fn cell_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    let words = cell_words(&Table {
        origin: (0, 0),
        cells: vec![Irradiance::DEFAULT; (Table::SIDE * Table::SIDE) as usize],
    });
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("modern ambient squares"),
        contents: bytemuck::cast_slice(&words),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}

/// The table's cells as the shader reads them.
fn cell_words(table: &Table) -> Vec<[f32; 4]> {
    let mut words = Vec::with_capacity(table.cells.len() * CELL_STRIDE);
    for cell in &table.cells {
        words.extend_from_slice(&cell.0);
        words.push([0.0; 4]);
    }
    words
}

/// The face target: what one capture face is drawn into.
struct FaceTarget {
    colour: wgpu::Texture,
    view: wgpu::TextureView,
    depth: wgpu::TextureView,
}

/// A readback buffer and where its face is.
enum Stage {
    Free,
    /// The copy was encoded; the buffer is mapped after the submission.
    Copied(Ticket),
    /// Mapping: 0 pending, 1 ready, 2 failed.
    Mapping(Ticket, Arc<AtomicU8>),
}

struct Readback {
    buffer: wgpu::Buffer,
    stage: Stage,
}

/// This frame's capture (recorded in `draw`, encoded in `encode`).
pub(crate) struct AmbientCapture {
    scene: SceneCapture,
    /// The face's draws and the frame slot's bind group; the buffer it
    /// slices stays alive with it.
    list: Vec<u32>,
    _frames: wgpu::Buffer,
    bind: wgpu::BindGroup,
    /// The face's sky.
    sky: crate::frame::gpu::sky_cube::SkyFace,
    ticket: Ticket,
    readback: usize,
}

/// What the capture did so far (diagnostics and the tests).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AmbientStats {
    /// Faces captured so far.
    pub faces: u64,
    /// Blocks cached.
    pub cached: usize,
    /// Squares still waiting, captured or projecting.
    pub pending: usize,
    /// The CPU time recording the latest face (milliseconds).
    pub record_ms: f32,
    /// The latest face's draw calls.
    pub draws: usize,
}

/// See the module docs.
pub(crate) struct AmbientGpu {
    pub(crate) schedule: Schedule<Raw>,
    pub(crate) enabled: bool,
    target: Option<FaceTarget>,
    readbacks: Vec<Readback>,
    /// The blocks the workers finished.
    results: Arc<Mutex<Vec<(Key, Irradiance)>>>,
    /// The active square's eye in world units, fixed for its six faces.
    eye: Option<(Square, [f32; 3])>,
    pub(crate) capture: Option<AmbientCapture>,
    /// The environment key of the last frame and the frames it has held.
    env: Option<(u64, u32)>,
    /// The squares the scene window touches (requested once the key holds).
    window: usize,
    table: Option<Table>,
    /// The shader's parameters (`ProbeUniforms::ambient`, `ambient_dims`).
    params: ([f32; 4], [u32; 4]),
    faces: u64,
    record_ms: f32,
    draws: usize,
    /// Tests: blocks shown in place of some squares' own.
    #[cfg(test)]
    pub(crate) test_blocks: std::collections::HashMap<Square, Irradiance>,
}

impl Default for AmbientGpu {
    fn default() -> Self {
        Self {
            schedule: Schedule::default(),
            enabled: false,
            target: None,
            readbacks: Vec::new(),
            results: Arc::default(),
            eye: None,
            capture: None,
            env: None,
            window: 0,
            table: None,
            params: ([0.0; 4], [0; 4]),
            faces: 0,
            record_ms: 0.0,
            draws: 0,
            #[cfg(test)]
            test_blocks: Default::default(),
        }
    }
}

pub(crate) struct AmbientProgress {
    env: Option<(u64, u32)>,
    faces: u64,
    record_ms: f32,
    draws: usize,
}

impl AmbientGpu {
    pub(crate) fn checkpoint(&self) -> AmbientProgress {
        AmbientProgress {
            env: self.env,
            faces: self.faces,
            record_ms: self.record_ms,
            draws: self.draws,
        }
    }
    /// `table` with the blocks tests put in place of some squares'.
    #[cfg(test)]
    fn with_test_blocks(&self, mut table: Table) -> Table {
        for (&square, &block) in &self.test_blocks {
            table.set(square, block);
        }
        table
    }

    #[cfg(not(test))]
    fn with_test_blocks(&self, table: Table) -> Table {
        table
    }

    /// The shader's `ambient` and `ambient_dims` (zero while off).
    pub(crate) fn shader_params(&self) -> ([f32; 4], [u32; 4]) {
        if self.enabled {
            self.params
        } else {
            ([0.0; 4], [0; 4])
        }
    }

    /// Runs `projection` on a worker thread (on the render thread when the
    /// renderer is synchronous or no thread can be made).
    fn project(&self, projection: Projection<Raw>, inline: bool) {
        let results = Arc::clone(&self.results);
        let job = move || {
            let size = projection.size;
            let faces = Faces {
                size,
                texels: projection
                    .faces
                    .map(|f| face_from_rgba16f(&f, size * 8, size)),
            };
            let block = project(&faces);
            results
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((projection.key, block));
        };
        if inline {
            job();
            return;
        }
        let job = Arc::new(Mutex::new(Some(job)));
        let worker = Arc::clone(&job);
        let spawned = std::thread::Builder::new()
            .name("ambient projection".into())
            .spawn(move || {
                if let Some(job) = worker.lock().ok().and_then(|mut j| j.take()) {
                    job();
                }
            });
        if spawned.is_err() {
            if let Some(job) = job.lock().ok().and_then(|mut j| j.take()) {
                job();
            }
        }
    }

    fn free_readback(&self) -> Option<usize> {
        self.readbacks
            .iter()
            .position(|r| matches!(r.stage, Stage::Free))
    }

    /// The statistics.
    pub(crate) fn stats(&self) -> AmbientStats {
        AmbientStats {
            faces: self.faces,
            cached: self.schedule.cached(),
            pending: self.schedule.pending()
                + self
                    .readbacks
                    .iter()
                    .filter(|r| !matches!(r.stage, Stage::Free))
                    .count(),
            record_ms: self.record_ms,
            draws: self.draws,
        }
    }
}

impl ModernRenderer {
    /// The ambient capture's statistics.
    #[must_use]
    pub fn ambient_stats(&self) -> AmbientStats {
        self.history.ambient.stats()
    }

    /// The block map square `square` shows at `now_ms` (tests and
    /// diagnostics).
    #[must_use]
    pub fn ambient_block(&self, square: Square, now_ms: i64) -> Irradiance {
        self.history.ambient.schedule.shown(square, now_ms)
    }

    /// Whether a capture, a projection or a blend is still under way at
    /// `now_ms` (false while the ambient is off).
    #[must_use]
    pub fn ambient_pending(&self, now_ms: i64) -> bool {
        let a = &self.history.ambient;
        a.enabled
            && (a.schedule.wanted().len() < a.window
                || !a.schedule.idle()
                || a.schedule.blending(now_ms)
                || a.readbacks.iter().any(|r| !matches!(r.stage, Stage::Free)))
    }

    /// Whether every requested square has its block and no blend is moving
    /// at `now_ms` (the ambient has settled).
    #[must_use]
    pub fn ambient_settled(&self, now_ms: i64) -> bool {
        self.history.ambient.enabled
            && !self.history.ambient.schedule.wanted().is_empty()
            && !self.ambient_pending(now_ms)
    }

    /// This frame's ambient work (the module docs): the harvest, the request,
    /// at most one face's capture recording, the table.
    pub(crate) fn prepare_ambient(&mut self, prep: &PrepareFrame<'_, '_>, frame: &FrameUniforms) {
        let PrepareFrame {
            device,
            queue,
            snapshot,
            origin,
        } = *prep;
        self.history.ambient.capture = None;
        self.history.ambient.enabled = self.preparation.settings.look.captured_ambient();
        if !self.history.ambient.enabled {
            return;
        }
        let now = self.frame_millis();
        self.harvest_ambient(device, now);

        // Request the window's squares under the settled environment.
        let key = self.ambient_key(snapshot);
        let held = match self.history.ambient.env {
            Some((k, n)) if k == key => n.saturating_add(1),
            _ => 1,
        };
        self.history.ambient.env = Some((key, held));
        let window = snapshot
            .floors
            .first()
            .and_then(Option::as_ref)
            .map(|g| window_squares(snapshot.floor_base, [g.tiles_x, g.tiles_z]))
            .unwrap_or_default();
        self.history.ambient.window = window.len();
        if held >= SETTLE_FRAMES && !window.is_empty() {
            self.history.ambient.schedule.request(key, &window, now);
        }

        // At most one face a frame.
        // (The buffers are made with the first face.)
        let room = self.history.ambient.readbacks.is_empty()
            || self.history.ambient.free_readback().is_some();
        if !window.is_empty() && room {
            if let Some(ticket) = self.history.ambient.schedule.next_face() {
                self.record_ambient_face(prep, frame, ticket);
            }
        }

        // The table around the camera.
        let camera = square_of(snapshot.floor_base, origin[0] as i32, origin[2] as i32);
        let table_origin = Table::origin_for(camera);
        let table = self.history.ambient.with_test_blocks(Table::fill(
            &self.history.ambient.schedule,
            table_origin,
            now,
        ));
        if self.history.ambient.table.as_ref() != Some(&table) {
            queue.write_buffer(
                &self.scene_resources.lights.probes.squares,
                0,
                bytemuck::cast_slice(&cell_words(&table)),
            );
            self.history.ambient.table = Some(table);
        }
        let base = snapshot.floor_base;
        self.history.ambient.params = (
            [
                1.0,
                (base[0] * 512 - table_origin.0 * SQUARE_SIZE) as f32,
                (base[1] * 512 - table_origin.1 * SQUARE_SIZE) as f32,
                0.0,
            ],
            [Table::SIDE as u32, 0, 0, 0],
        );
    }

    /// The environment's key for the capture: the values the capture
    /// depends on (`probes::env_key`) and how many sky cubes were baked (a sky
    /// that becomes ready asks for every square again).
    fn ambient_key(&self, snapshot: &SceneSnapshot<'_>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        crate::frame::gpu::probes::env_key(snapshot).hash(&mut h);
        self.scene_resources.sky_cubes.baked().hash(&mut h);
        h.finish()
    }

    /// Hand the finished readbacks to the schedule and the finished blocks
    /// to it too.
    fn harvest_ambient(&mut self, device: &wgpu::Device, now: i64) {
        let inline = self.threads() == 1;
        let _ = device.poll(wgpu::PollType::Poll);
        for i in 0..self.history.ambient.readbacks.len() {
            let (ticket, state) = match &self.history.ambient.readbacks[i].stage {
                Stage::Mapping(ticket, flag) => (*ticket, flag.load(Ordering::Acquire)),
                _ => continue,
            };
            if state == 0 {
                continue;
            }
            let bytes = (state == 1).then(|| {
                let buffer = &self.history.ambient.readbacks[i].buffer;
                let bytes = buffer
                    .slice(..)
                    .get_mapped_range()
                    .expect("the mapped ambient face")
                    .to_vec();
                buffer.unmap();
                bytes
            });
            self.history.ambient.readbacks[i].stage = Stage::Free;
            match bytes {
                Some(bytes) => {
                    if let Some(p) =
                        self.history
                            .ambient
                            .schedule
                            .face_done(ticket, FACE_RES as usize, bytes)
                    {
                        self.history.ambient.project(p, inline);
                    }
                }
                None => {
                    log::warn!(
                        "[modern] ambient: a face readback failed; its square is captured again"
                    );
                    self.history.ambient.schedule.abandon(ticket);
                }
            }
        }
        let done = std::mem::take(
            &mut *self
                .history
                .ambient
                .results
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for (key, block) in done {
            self.history.ambient.schedule.complete(key, block, now);
        }
    }

    /// The eye of `square`'s capture (scene-local fine units): the centre of
    /// the loaded tiles' bounds, [`EYE_CLEARANCE`] above their top (the
    /// terrain of every level and the locs standing in them), plus the
    /// capture offset.
    fn square_eye(
        snapshot: &SceneSnapshot<'_>,
        square: Square,
        spheres: &[(glam::Vec3, f32)],
        locs: usize,
        origin: [f32; 3],
    ) -> [f32; 3] {
        let base = snapshot.floor_base;
        let tiles = snapshot
            .floors
            .iter()
            .flatten()
            .map(|g| [g.tiles_x, g.tiles_z])
            .next()
            .unwrap_or([0, 0]);
        let lo = [base[0].max(square.0 * 64), base[1].max(square.1 * 64)];
        let hi = [
            (base[0] + tiles[0] as i32)
                .min((square.0 + 1) * 64)
                .max(lo[0]),
            (base[1] + tiles[1] as i32)
                .min((square.1 + 1) * 64)
                .max(lo[1]),
        ];
        let local = |tile: i32, axis: usize| (tile - base[axis]) * 512;
        let mut top = i32::MAX;
        for g in snapshot.floors.iter().flatten() {
            for tz in (lo[1]..=hi[1]).step_by(BOUNDS_STEP).chain([hi[1]]) {
                for tx in (lo[0]..=hi[0]).step_by(BOUNDS_STEP).chain([hi[0]]) {
                    top = top.min(
                        g.heights
                            .get_fine_height_clamped(local(tx, 0), local(tz, 1)),
                    );
                }
            }
        }
        let mut top = if top == i32::MAX { 0.0 } else { top as f32 };
        let (x0, x1) = (local(lo[0], 0) as f32, local(hi[0], 0) as f32);
        let (z0, z1) = (local(lo[1], 1) as f32, local(hi[1], 1) as f32);
        for &(c, r) in &spheres[..locs] {
            let (x, z) = (c.x + origin[0], c.z + origin[2]);
            if (x0..=x1).contains(&x) && (z0..=z1).contains(&z) {
                top = top.min(c.y + origin[1] - r);
            }
        }
        [
            (x0 + x1) * 0.5 + CAPTURE_OFFSET[0],
            top - EYE_CLEARANCE + CAPTURE_OFFSET[1],
            (z0 + z1) * 0.5 + CAPTURE_OFFSET[2],
        ]
    }

    /// Record `ticket`'s face: its draws, frame blocks and sky, and the
    /// readback buffer it will be copied to.
    fn record_ambient_face(
        &mut self,
        prep: &PrepareFrame<'_, '_>,
        frame: &FrameUniforms,
        ticket: Ticket,
    ) {
        let PrepareFrame {
            device,
            queue,
            snapshot,
            origin,
        } = *prep;
        let started = std::time::Instant::now();
        let face = usize::from(ticket.face);
        if self.history.probes.pipes.is_none() {
            self.history.probes.pipes =
                Some(CapturePipes::new(device, queue, &self.encoding_inputs()));
        }
        if self.history.ambient.target.is_none() {
            self.history.ambient.target = Some(face_target(device));
            self.history.ambient.readbacks = (0..READBACKS)
                .map(|_| Readback {
                    buffer: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("modern ambient readback"),
                        size: u64::from(FACE_RES) * u64::from(FACE_RES) * 8,
                        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    stage: Stage::Free,
                })
                .collect();
        }
        let readback = self
            .history
            .ambient
            .free_readback()
            .expect("a free readback buffer");
        let candidates = self.capture_candidates(prep);
        // The eye is kept in world units (the scene window may move between a
        // square's faces).
        let shift = [
            (snapshot.floor_base[0] * 512) as f32,
            0.0,
            (snapshot.floor_base[1] * 512) as f32,
        ];
        let eye = match self.history.ambient.eye {
            Some((square, world)) if square == ticket.square && ticket.face != 0 => {
                [world[0] - shift[0], world[1], world[2] - shift[2]]
            }
            _ => {
                let eye = Self::square_eye(
                    snapshot,
                    ticket.square,
                    &candidates.spheres,
                    candidates.locs.len(),
                    origin,
                );
                self.history.ambient.eye = Some((
                    ticket.square,
                    [eye[0] + shift[0], eye[1], eye[2] + shift[2]],
                ));
                eye
            }
        };
        let local = [eye[0] - origin[0], eye[1] - origin[1], eye[2] - origin[2]];
        let mut lists = cull_faces(&candidates.spheres, local, FACE_FAR, SMALL_ANGLE);
        let seen = std::mem::take(&mut lists[face]);
        let mut used = vec![false; candidates.spheres.len()];
        for &i in &seen {
            used[i as usize] = true;
        }
        let built = self.capture_draws(prep, &candidates.locs, &candidates.parts, &used);
        let list: Vec<u32> = seen
            .iter()
            .flat_map(|&i| {
                let (a, b) = built.ranges[i as usize];
                a..b
            })
            .collect();
        let winding = face_windings(frame, FACE_NEAR);
        let limits = (FACE_NEAR, FACE_FAR);
        // The frame's sky over the face, at the capture's exposure offset.
        let sky = self.prepare_sky_face(
            device,
            queue,
            frame,
            crate::frame::gpu::sky_cube::FaceView {
                face,
                limits,
                res: FACE_RES,
                exposure: CAPTURE_EXPOSURE,
            },
        );
        let slot = capture_frame(frame, local, face, limits, false);
        let frames = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modern ambient frames"),
            contents: bytemuck::bytes_of(&slot),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let pipes = self
            .history
            .probes
            .pipes
            .as_ref()
            .expect("capture pipelines");
        let frame_layout = self.pipes().forward.get_bind_group_layout(0);
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern ambient frame"),
            layout: &frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &frames,
                        offset: 0,
                        size: wgpu::BufferSize::new(SLOT),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&pipes.ao_white),
                },
            ],
        });
        self.history.ambient.draws = seen.len();
        self.history.ambient.record_ms = started.elapsed().as_secs_f32() * 1000.0;
        self.history.ambient.faces += 1;
        self.history.ambient.readbacks[readback].stage = Stage::Copied(ticket);
        self.history.ambient.capture = Some(AmbientCapture {
            scene: SceneCapture {
                draws: built.draws,
                sky_layers: None,
                sky_draws: Default::default(),
                sky_models: Vec::new(),
                winding,
            },
            list,
            _frames: frames,
            bind,
            sky,
            ticket,
            readback,
        });
    }

    /// Return a prepared face that never reached a submitted command buffer.
    pub(crate) fn discard_ambient_frame(&mut self, previous: AmbientProgress) {
        self.history.ambient.capture = None;
        for readback in &mut self.history.ambient.readbacks {
            if let Stage::Copied(ticket) = readback.stage {
                self.history.ambient.schedule.retry_unsubmitted(ticket);
                readback.stage = Stage::Free;
            }
        }
        self.history.ambient.env = previous.env;
        self.history.ambient.faces = previous.faces;
        self.history.ambient.record_ms = previous.record_ms;
        self.history.ambient.draws = previous.draws;
    }

    /// Map the readback buffer of the face this frame encoded (its copy was
    /// submitted with the frame); the harvest reads it once it is ready.
    pub(crate) fn map_ambient_readback(&mut self) {
        for r in &mut self.history.ambient.readbacks {
            let Stage::Copied(ticket) = r.stage else {
                continue;
            };
            let flag = Arc::new(AtomicU8::new(0));
            let done = Arc::clone(&flag);
            r.buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    done.store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
                });
            r.stage = Stage::Mapping(ticket, flag);
        }
    }
}

/// The face target: the capture format at the face size and its depth.
fn face_target(device: &wgpu::Device) -> FaceTarget {
    let size = wgpu::Extent3d {
        width: FACE_RES,
        height: FACE_RES,
        depth_or_array_layers: 1,
    };
    let texture = |label, format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let colour = texture(
        "modern ambient face",
        crate::frame::gpu::probes::CAPTURE_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let depth = texture(
        "modern ambient face depth",
        DEPTH_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    );
    FaceTarget {
        view: colour.create_view(&Default::default()),
        depth: depth.create_view(&Default::default()),
        colour,
    }
}

impl<'a> EncodeInputs<'a> {
    /// Encode this frame's ambient face (the probes unit): the sky, then the
    /// scene's draws, into the face target, copied to the face's readback
    /// buffer.
    pub(crate) fn encode_ambient(&self, encoder: &mut wgpu::CommandEncoder) {
        let (Some(capture), Some(pipes), Some(target)) = (
            self.ambient.capture.as_ref(),
            self.probes.pipes.as_ref(),
            self.ambient.target.as_ref(),
        ) else {
            return;
        };
        let face = usize::from(capture.ticket.face);
        let clear = wgpu::Color {
            r: f64::from(self.clear[0] + CAPTURE_EXPOSURE),
            g: f64::from(self.clear[1] + CAPTURE_EXPOSURE),
            b: f64::from(self.clear[2] + CAPTURE_EXPOSURE),
            a: 1.0,
        };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(self.begin_pass(crate::frame::passes::Pass::AmbientCapture)),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            self.draw_sky_face(&mut pass, &capture.sky);
            pass.set_bind_group(0, &capture.bind, &[]);
            pass.set_bind_group(2, &pipes.no_shadow, &[]);
            // The capture draws without point-light shadows.
            pass.set_bind_group(3, &self.lights.bind_capture, &[]);
            self.draw_face(&mut pass, &capture.scene, &capture.list, face);
        }
        encoder.copy_texture_to_buffer(
            target.colour.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.ambient.readbacks[capture.readback].buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(FACE_RES * 8),
                    rows_per_image: Some(FACE_RES),
                },
            },
            wgpu::Extent3d {
                width: FACE_RES,
                height: FACE_RES,
                depth_or_array_layers: 1,
            },
        );
    }
}
