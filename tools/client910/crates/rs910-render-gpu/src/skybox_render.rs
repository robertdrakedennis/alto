//! GPU consumers for the environment passes that surround the scene draw:
//!
//! - the skybox pre-pass, issued before the scene draw: the 3D sky model
//!   under a rotation-only view with its own depth (fog-colour clear, model,
//!   then a depth clear), or the 2D material path (tinted scaled tiles and
//!   rectangle fills);
//! - the underwater floor, drawn before the normal scene lists with water
//!   fog.
//!
//! Plans come from `crate::skybox`; the frame's sky models and material
//! textures from the renderer-neutral `crate::sky_frame::SkyFrame` (the
//! shell's `SkyCache` loads and fades them; this pass uploads them on first
//! use). The model and floor meshes reuse the ordinary `FloorPipeline`
//! (`Model` shader), so their materials, fog and MSAA/bloom variants match
//! the scene pass.

use std::collections::{BTreeMap, BTreeSet};

use wgpu::util::DeviceExt;

use crate::floor_render::{FloorMesh, FloorPipeline, FloorTextureCache, FloorUniforms, MeshUpload};
use crate::skybox::{FlatQuad, SkyLayer, SkyboxKey};

// The sky's CPU inputs moved to rs910-scene with its resolution (lane
// Q-PREM1); `skybox_render::SkyAssets` keeps its path.
pub use crate::sky_frame::SkyAssets;

/// A sky's uploaded model: the mesh and the fade last uploaded to it.
struct SkyModel {
    mesh: FloorMesh,
    applied_fade: i32,
}

/// Which sprite a flat quad samples: a fill (none), a box's material or one
/// of its decors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum QuadSprite {
    Material(SkyboxKey),
    Decor(SkyboxKey, usize),
}

/// The 2D path's material sprite and its first/last pixels.
struct SkySprite {
    bind_group: wgpu::BindGroup,
    first: u32,
    last: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FlatVertex {
    pos: [f32; 2],
    uv: [f32; 2],
    colour: [f32; 4],
}

const FLAT_SHADER: &str = r#"
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) colour: vec4<f32>,
};
@group(0) @binding(0) var sprite_tex: texture_2d<f32>;
@group(0) @binding(1) var sprite_sampler: sampler;
@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) colour: vec4<f32>) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(pos, 0.0, 1.0);
    out.uv = uv;
    out.colour = colour;
    return out;
}
// Sprite shader: texture * tint (the tile's ARGB colour).
@fragment
fn fs_opaque(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(sprite_tex, sprite_sampler, in.uv) * in.colour;
}
// Alpha test GREATER 0 + SRC_ALPHA blending.
@fragment
fn fs_blend(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(sprite_tex, sprite_sampler, in.uv) * in.colour;
    if (c.w <= 0.0) { discard; }
    return c;
}
"#;

/// What [`EnvPasses::prepare`] uploads through and reads.
pub struct SkyUploads<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a dyn crate::uploads::Uploader,
    pub floor: &'a FloorPipeline,
    pub textures: &'a mut FloorTextureCache,
    pub assets: &'a SkyAssets<'a>,
    pub camera: &'a crate::camera::SceneCamera,
    pub env: &'a crate::env::EnvFrame,
    pub viewport: (u32, u32),
}

/// The attachments [`EnvPasses::encode_sky`] draws into: the colour target
/// is loaded/cleared as the scene pass would have, `rect` is the scene
/// viewport and scissor when the UI bounds the scene.
#[derive(Clone, Copy)]
pub struct SkyTargets<'a> {
    pub colour: &'a wgpu::TextureView,
    pub resolve: Option<&'a wgpu::TextureView>,
    pub depth: &'a wgpu::TextureView,
    pub colour_load: wgpu::LoadOp<wgpu::Color>,
    pub rect: Option<([i32; 4], [i32; 4])>,
}

struct FlatPipelines {
    key: (wgpu::TextureFormat, u32),
    opaque: wgpu::RenderPipeline,
    blend: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: wgpu::BindGroup,
}

/// Everything the scene pass needs from the environment passes this frame.
#[derive(Default)]
pub struct EnvPasses {
    models: BTreeMap<SkyboxKey, SkyModel>,
    sprites: BTreeMap<SkyboxKey, SkySprite>,
    /// The decors' baked sprites, with the baked picture each uploaded (by
    /// identity, so a rebake replaces it).
    decor_sprites:
        BTreeMap<(SkyboxKey, usize), (std::sync::Arc<crate::sky_frame::SkyTexture>, SkySprite)>,
    failed_models: BTreeSet<SkyboxKey>,
    failed_sprites: BTreeSet<SkyboxKey>,
    flat: Option<FlatPipelines>,
    /// This frame's sky layers (`None`: no environment skybox, the scene
    /// pass clears to the fog colour).
    layers: Option<Vec<SkyLayer>>,
    /// Flat quads resolved for the frame: `(sprite key or None for a fill,
    /// quad, blended)`.
    flat_quads: Vec<(Option<QuadSprite>, FlatQuad, bool)>,
    /// `flat_quads` range of each entry of `layers`.
    ranges: Vec<std::ops::Range<usize>>,
    /// The flat quads' vertices ([`EnvPasses::upload_flat`]; grown by
    /// doubling, rewritten each frame that draws the sky).
    flat_vertices: Option<wgpu::Buffer>,
    /// The uploaded underwater floor.
    pub underwater: Option<FloorMesh>,
}

impl EnvPasses {
    /// Whether this frame draws a skybox (the scene pass then loads colour).
    #[must_use]
    pub fn has_sky(&self) -> bool {
        self.layers.is_some()
    }

    /// Upload this frame's sky (`sky`, resolved by the shell's
    /// `sky_frame::SkyCache`): the sky model and 2D sprite on first use, the
    /// faded face alphas when the fade changed, and the rotation-only model
    /// frame. Returns the boxes whose model this toolkit could not create
    /// (the failure is swallowed, the box falls back to the 2D path), for
    /// `SkyCache::model_upload_failed`.
    pub fn prepare(
        &mut self,
        inputs: SkyUploads<'_>,
        sky: Option<crate::sky_frame::SkyFrame<'_>>,
    ) -> Vec<SkyboxKey> {
        let SkyUploads {
            device,
            queue,
            floor,
            textures,
            assets,
            camera,
            env,
            viewport,
        } = inputs;
        self.flat_quads.clear();
        self.ranges.clear();
        let Some(sky) = sky else {
            self.layers = None;
            return Vec::new();
        };
        let mut failed = Vec::new();
        let mut state = crate::material::MaterialState::default();
        let millis = floor.frame_millis();
        for layer in sky.layers {
            let start = self.flat_quads.len();
            match layer {
                SkyLayer::Model {
                    key,
                    pitch,
                    yaw,
                    roll,
                    fade,
                } => {
                    if !self.models.contains_key(key) && !self.failed_models.contains(key) {
                        // The model as first created, before any fade.
                        if let Some(model) = sky.unfaded_model(*key) {
                            match upload_model(
                                device,
                                queue.queue(),
                                floor,
                                textures,
                                assets,
                                &model,
                            ) {
                                Ok(model) => {
                                    self.models.insert(*key, model);
                                }
                                Err(error) => {
                                    // The failure is swallowed; the box keeps
                                    // no model (2D path).
                                    crate::logging::warn_repeated!(
                                        "[client910] skybox {key:?} model upload: {error:#}"
                                    );
                                    self.failed_models.insert(*key);
                                    failed.push(*key);
                                }
                            }
                        }
                    }
                    let (Some(sky_model), Some(model)) =
                        (self.models.get_mut(key), sky.model(*key))
                    else {
                        self.ranges.push(start..start);
                        continue;
                    };
                    if sky_model.applied_fade != *fade {
                        if let Err(error) =
                            sky_model.mesh.update_model(queue, assets.materials, model)
                        {
                            crate::logging::warn_repeated!(
                                "[client910] skybox fade upload: {error:#}"
                            );
                        }
                        sky_model.applied_fade = *fade;
                    }
                    let uniforms = model_uniforms(camera, env, *pitch, *yaw, *roll, millis);
                    sky_model
                        .mesh
                        .set_model_uniforms(device, queue, floor, &uniforms);
                    sky_model
                        .mesh
                        .prepare_materials(queue, &mut state, millis, &[[0.; 4]; 8], 0);
                }
                SkyLayer::Material {
                    key,
                    material,
                    alpha,
                    first,
                    fog,
                    pitch,
                    yaw,
                    fill,
                    ..
                } => {
                    if !self.sprites.contains_key(key) && !self.failed_sprites.contains(key) {
                        let flat = self.flat_pipelines(device, queue.queue(), floor);
                        // `None`: the texture did not load (the cache logged
                        // it).
                        match sky.sprite(*key) {
                            Some(texture) => {
                                let sprite = upload_sprite(device, queue.queue(), flat, texture);
                                self.sprites.insert(*key, sprite);
                            }
                            None => {
                                self.failed_sprites.insert(*key);
                            }
                        }
                    }
                    let Some(sprite) = self.sprites.get(key) else {
                        // No sprite: nothing but the clear/fills.
                        self.ranges.push(start..start);
                        continue;
                    };
                    let multiply = assets
                        .materials
                        .get(*material as u32)
                        .is_some_and(|m| m.alpha == crate::texture::AlphaMode::Multiply);
                    // The sprite's blend mode.
                    let blended = *alpha != 255 || multiply;
                    if blended && *first {
                        self.flat_quads.push((
                            None,
                            FlatQuad::Fill {
                                rect: [0, 0, viewport.0 as i32, viewport.1 as i32],
                                argb: (*fog as u32) & 0xFF_FFFF,
                            },
                            false,
                        ));
                    }
                    for quad in crate::skybox::flat_quads(
                        [viewport.0 as i32, viewport.1 as i32],
                        crate::skybox::FlatSky {
                            pitch: *pitch,
                            yaw: *yaw,
                            alpha: *alpha,
                            fill: *fill,
                            edges: [sprite.first, sprite.last],
                        },
                    ) {
                        match quad {
                            FlatQuad::Sprite { .. } => {
                                self.flat_quads.push((
                                    Some(QuadSprite::Material(*key)),
                                    quad,
                                    blended,
                                ));
                            }
                            // Rectangle fills use blend mode 1.
                            FlatQuad::Fill { .. } => self.flat_quads.push((None, quad, true)),
                        }
                    }
                }
                SkyLayer::Decor {
                    key,
                    decor,
                    alpha,
                    size,
                    direction,
                    pitch,
                    yaw,
                    roll,
                } => {
                    // A baked sprite (`sky_decor`) centred on the decor's
                    // direction, blended at the layer's alpha.
                    let Some(texture) = sky.decor_sprite(*key, *decor) else {
                        self.ranges.push(start..start);
                        continue;
                    };
                    let (w, h) = (viewport.0 as i32, viewport.1 as i32);
                    let Some([cx, cy]) = crate::skybox::decor_centre(
                        *direction,
                        (*pitch, *yaw, *roll),
                        camera.projection(),
                        (w, h),
                    ) else {
                        self.ranges.push(start..start);
                        continue;
                    };
                    let x0 = (cx - (size / 2) as f32) as i32;
                    let y0 = (cy - (size / 2) as f32) as i32;
                    if y0 >= h || y0 + size <= 0 || x0 >= w || x0 + size <= 0 {
                        self.ranges.push(start..start);
                        continue;
                    }
                    let stale = self
                        .decor_sprites
                        .get(&(*key, *decor))
                        .is_none_or(|(source, _)| !std::sync::Arc::ptr_eq(source, texture));
                    if stale {
                        let flat = self.flat_pipelines(device, queue.queue(), floor);
                        let sprite = upload_sprite(device, queue.queue(), flat, texture);
                        self.decor_sprites
                            .insert((*key, *decor), (texture.clone(), sprite));
                    }
                    self.flat_quads.push((
                        Some(QuadSprite::Decor(*key, *decor)),
                        FlatQuad::Sprite {
                            rect: [x0, y0, *size, *size],
                            alpha: *alpha,
                        },
                        true,
                    ));
                }
                SkyLayer::Fill { argb, blend } => {
                    self.flat_quads.push((
                        None,
                        FlatQuad::Fill {
                            rect: [0, 0, viewport.0 as i32, viewport.1 as i32],
                            argb: *argb,
                        },
                        *blend,
                    ));
                }
                // `clear(3, fog)` restricted to the viewport: an opaque fill
                // (a pass-level clear would also wipe the UI drawn before).
                SkyLayer::Clear { rgb } => self.flat_quads.push((
                    None,
                    FlatQuad::Fill {
                        rect: [0, 0, viewport.0 as i32, viewport.1 as i32],
                        argb: 0xFF00_0000 | (*rgb as u32 & 0xFF_FFFF),
                    },
                    false,
                )),
            }
            self.ranges.push(start..self.flat_quads.len());
        }
        if !self.flat_quads.is_empty() {
            self.flat_pipelines(device, queue.queue(), floor);
        }
        self.layers = Some(sky.layers.to_vec());
        failed
    }

    fn flat_pipelines(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        floor: &FloorPipeline,
    ) -> &FlatPipelines {
        let key = (floor.scene_format(), floor.sample_count);
        if self.flat.as_ref().is_none_or(|f| f.key != key) {
            // Sprites keep their bind groups: the layout is recreated only
            // with a new format/sample count, so drop the cached sprites.
            self.sprites.clear();
            self.decor_sprites.clear();
            self.flat = Some(FlatPipelines::new(device, queue, key));
        }
        self.flat.as_ref().expect("flat pipelines")
    }

    /// This frame's 2D sky quads at `viewport` into the flat vertex buffer
    /// (programme Phase 6: one buffer kept across frames instead of one per
    /// frame); [`EnvPasses::encode_sky`] draws them. Called once per frame
    /// that draws the sky, before its submission.
    pub fn upload_flat(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        viewport: (u32, u32),
    ) {
        if self.layers.is_none() || self.flat_quads.is_empty() {
            return;
        }
        let mut verts = Vec::with_capacity(self.flat_quads.len() * 6);
        for (_, quad, _) in &self.flat_quads {
            push_quad(&mut verts, quad, viewport);
        }
        let bytes: &[u8] = bytemuck::cast_slice(&verts);
        if self
            .flat_vertices
            .as_ref()
            .is_none_or(|b| b.size() < bytes.len() as u64)
        {
            self.flat_vertices = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("skybox 2D quads"),
                size: (bytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        queue.write_buffer(self.flat_vertices.as_ref().unwrap(), 0, bytes);
    }

    /// Encode the skybox pre-pass into its own render pass: the colour
    /// target is loaded/cleared exactly as the scene pass would have, and
    /// the depth attachment is cleared for the sky model; the scene pass
    /// then clears depth again, which ends the sky's own depth use.
    pub fn encode_sky(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        floor: &FloorPipeline,
        targets: &SkyTargets<'_>,
    ) {
        let Some(layers) = &self.layers else {
            return;
        };
        let SkyTargets {
            colour,
            resolve,
            depth,
            colour_load: load,
            rect,
        } = *targets;
        // Uploaded for this frame by `upload_flat`.
        let flat_buffer = (!self.flat_quads.is_empty())
            .then_some(self.flat_vertices.as_ref())
            .flatten();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("skybox pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: colour,
                resolve_target: resolve,
                depth_slice: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            occlusion_query_set: None,
            multiview_mask: None,
            timestamp_writes: None,
        });
        if let Some(([x, y, w, h], [l, t, r, b])) = rect {
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0., 1.);
            pass.set_scissor_rect(l as u32, t as u32, (r - l) as u32, (b - t) as u32);
        }
        for (layer, range) in layers.iter().zip(&self.ranges) {
            if let SkyLayer::Model { key, .. } = layer {
                if let Some(sky) = self.models.get(key) {
                    sky.mesh.draw_model(&mut pass, floor);
                }
                continue;
            }
            let (Some(flat), Some(buffer)) = (&self.flat, flat_buffer) else {
                continue;
            };
            pass.set_vertex_buffer(0, buffer.slice(..));
            for quad in range.clone() {
                let (key, _, blended) = &self.flat_quads[quad];
                pass.set_pipeline(if *blended { &flat.blend } else { &flat.opaque });
                let group = key
                    .and_then(|k| match k {
                        QuadSprite::Material(k) => self.sprites.get(&k).map(|s| &s.bind_group),
                        QuadSprite::Decor(k, i) => {
                            self.decor_sprites.get(&(k, i)).map(|(_, s)| &s.bind_group)
                        }
                    })
                    .unwrap_or(&flat.white);
                pass.set_bind_group(0, group, &[]);
                let q = quad as u32;
                pass.draw(q * 6..q * 6 + 6, 0..1);
            }
        }
    }

    /// The underwater floor, drawn before the scene lists.
    /// The caller has bound the scene floor pipeline and group 0.
    pub fn draw_underwater<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if let Some(mesh) = self.underwater.as_ref().filter(|m| m.vertex_count > 0) {
            mesh.draw(pass);
        }
    }
}

fn push_quad(out: &mut Vec<FlatVertex>, quad: &FlatQuad, viewport: (u32, u32)) {
    let (rect, colour) = match *quad {
        FlatQuad::Sprite { rect, alpha } => (rect, [1.0, 1.0, 1.0, alpha as f32 / 255.0]),
        FlatQuad::Fill { rect, argb } => (
            rect,
            [
                ((argb >> 16) & 0xFF) as f32 / 255.0,
                ((argb >> 8) & 0xFF) as f32 / 255.0,
                (argb & 0xFF) as f32 / 255.0,
                ((argb >> 24) & 0xFF) as f32 / 255.0,
            ],
        ),
    };
    let (vw, vh) = (viewport.0.max(1) as f32, viewport.1.max(1) as f32);
    let x0 = rect[0] as f32 / vw * 2.0 - 1.0;
    let x1 = (rect[0] + rect[2]) as f32 / vw * 2.0 - 1.0;
    let y0 = 1.0 - rect[1] as f32 / vh * 2.0;
    let y1 = 1.0 - (rect[1] + rect[3]) as f32 / vh * 2.0;
    let v = |x: f32, y: f32, u: f32, t: f32| FlatVertex {
        pos: [x, y],
        uv: [u, t],
        colour,
    };
    out.extend([
        v(x0, y0, 0.0, 0.0),
        v(x1, y0, 1.0, 0.0),
        v(x1, y1, 1.0, 1.0),
        v(x0, y0, 0.0, 0.0),
        v(x1, y1, 1.0, 1.0),
        v(x0, y1, 0.0, 1.0),
    ]);
}

/// Upload the sky model `SkyCache` loaded (flags 1099776, ambient 255,
/// contrast 1, detail 0; see `sky_frame::load_model`).
fn upload_model(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    floor: &FloorPipeline,
    textures: &mut FloorTextureCache,
    assets: &SkyAssets<'_>,
    model: &crate::gpumodel::GpuModel,
) -> anyhow::Result<SkyModel> {
    let mesh = FloorMesh::from_model(
        MeshUpload {
            device,
            queue,
            pipeline: floor,
            textures,
            pack: assets.pack,
            materials: assets.materials,
        },
        model,
        [0.0; 3],
        "skybox model",
    )?;
    Ok(SkyModel {
        mesh,
        applied_fade: 0,
    })
}

/// The rotation-only view over the scene's projection, model drawn at
/// identity. The distance fog plane is
/// `(0, 0, 1, -fogStart) * view^T / (end - start)` with that view.
fn model_uniforms(
    camera: &crate::camera::SceneCamera,
    env: &crate::env::EnvFrame,
    pitch: i32,
    yaw: i32,
    roll: i32,
    millis: i32,
) -> FloorUniforms {
    let view = crate::skybox::model_view(pitch, yaw, roll).to_entries();
    let vp = crate::camera::multiply(&view, &camera.projection());
    let mut u = FloorUniforms::new(glam::Mat4::IDENTITY, env);
    u.wvp = (crate::camera::gl_to_wgpu_depth() * crate::camera::to_glam(&vp)).to_cols_array_2d();
    u.shadow_wvp = u.wvp;
    u.scene_origin = [0.; 4];
    u.scene_base = [0.; 4];
    u.eye_time = [0., 0., 0., (millis % 128000) as f32 / 1000.];
    u.distance_fog_plane = match env.fog.range {
        Some((start, end)) => {
            let v = [0., 0., 1., -start];
            let scale = 1. / (end - start);
            std::array::from_fn(|c| {
                (view[c * 4 + 3] * v[3]
                    + view[c * 4 + 2] * v[2]
                    + view[c * 4] * v[0]
                    + view[c * 4 + 1] * v[1])
                    * scale
            })
        }
        None => [0.; 4],
    };
    u
}

/// The sprite of the material texture `SkyCache` loaded
/// (`sky_frame::load_texture`: gamma 0.7, black texels transparent unless
/// multiply).
fn upload_sprite(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    flat: &FlatPipelines,
    texture: &crate::sky_frame::SkyTexture,
) -> SkySprite {
    let px = texture.rgba();
    let texture_gpu = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("skybox material"),
            size: wgpu::Extent3d {
                width: texture.size[0] as u32,
                height: texture.size[1] as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &px,
    );
    let view = texture_gpu.create_view(&Default::default());
    SkySprite {
        bind_group: flat.bind(device, &view),
        first: texture.first as u32,
        last: texture.last as u32,
    }
}

impl FlatPipelines {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, key: (wgpu::TextureFormat, u32)) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("skybox 2D"),
            source: wgpu::ShaderSource::Wgsl(FLAT_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("skybox 2D"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skybox 2D"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
        let make = |entry: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("skybox 2D"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<FlatVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    })],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: key.0,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: crate::pipelines::DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: key.1,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let opaque = make("fs_opaque", None);
        let blend = make(
            "fs_blend",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            }),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let white_texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("skybox fill"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255, 255, 255, 255],
        );
        let white = bind_sprite(
            device,
            &layout,
            &sampler,
            &white_texture.create_view(&Default::default()),
        );
        Self {
            key,
            opaque,
            blend,
            layout,
            sampler,
            white,
        }
    }

    fn bind(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        bind_sprite(device, &self.layout, &self.sampler, view)
    }
}

fn bind_sprite(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("skybox 2D"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quads_map_viewport_pixels_to_ndc() {
        let mut v = Vec::new();
        push_quad(
            &mut v,
            &FlatQuad::Fill {
                rect: [0, 0, 800, 600],
                argb: 0x80FF_0000,
            },
            (800, 600),
        );
        assert_eq!(v.len(), 6);
        assert_eq!(v[0].pos, [-1.0, 1.0]);
        assert_eq!(v[2].pos, [1.0, -1.0]);
        assert_eq!(v[0].colour, [1.0, 0.0, 0.0, 128.0 / 255.0]);
    }
}
