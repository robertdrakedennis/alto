//! The snapshot's skybox layers for the modern renderer: the layers (fills, clears, tiled
//! material sprites, the sky models under the rotation-only view) as uniform slots and draws,
//! their display colours through the inverse tonemap. The frame draws only the decor sprites
//! from them; the rest of the box is baked into the sky's cube ([`super::sky_box`]), and the
//! probe capture draws the layers and models itself.
use rs910_gpu_device::desc;
use wgpu::util::DeviceExt;

use crate::frame::*;

/// The `Layer` uniform block of [`crate::shaders::SKY_WGSL`] (256-byte
/// slots, bound with dynamic offsets).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct SkyLayerUniforms {
    pub(crate) colour: [f32; 4],
    pub(crate) rect: [f32; 4],
    pub(crate) tile: [f32; 4],
    pub(crate) top: [f32; 4],
    pub(crate) bottom: [f32; 4],
    pub(crate) params: [f32; 4],
    pub(crate) pad: [[f32; 4]; 10],
}

/// What a sky layer's texture belongs to: a box's material or one of its
/// decors' baked sprites.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SkyTextureKey {
    Material(crate::skybox::SkyboxKey),
    Decor(crate::skybox::SkyboxKey, usize),
}

/// One sky command.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SkyDraw {
    /// A fullscreen layer: its uniform slot and texture (`None`: white).
    Layer {
        slot: u32,
        texture: Option<SkyTextureKey>,
    },
}

/// A sky material layer's texture.
pub(crate) struct SkyTextureGpu {
    /// Its first and last pixels.
    pub(crate) first: i32,
    pub(crate) last: i32,
    pub(crate) bind_group: wgpu::BindGroup,
}

impl ModernRenderer {
    /// Upload `texture` for `key` unless the texture this key holds is it
    /// (by identity).
    pub(crate) fn ensure_sky_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        key: SkyTextureKey,
        texture: &std::sync::Arc<crate::sky_frame::SkyTexture>,
    ) {
        let fresh = self
            .preparation
            .sky_sources
            .get(&key)
            .is_none_or(|source| !std::sync::Arc::ptr_eq(source, texture));
        if fresh {
            let [tw, th] = texture.size.map(|v| v.max(1) as u32);
            let mut rgba = Vec::with_capacity((tw * th * 4) as usize);
            for &p in texture.argb.iter().take((tw * th) as usize) {
                let p = p as u32;
                rgba.extend_from_slice(&[
                    (p >> 16) as u8,
                    (p >> 8) as u8,
                    p as u8,
                    (p >> 24) as u8,
                ]);
            }
            rgba.resize((tw * th * 4) as usize, 0);
            let tex = device.create_texture_with_data(
                queue.queue(),
                &desc::texture_2d(
                    "modern sky material",
                    [tw, th],
                    wgpu::TextureFormat::Rgba8UnormSrgb,
                    wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                ),
                wgpu::util::TextureDataOrder::LayerMajor,
                &rgba,
            );
            let bind_group = desc::bind_group(
                device,
                "modern sky material",
                &self.device_resources.sky_texture_layout,
                &[
                    (
                        0,
                        wgpu::BindingResource::TextureView(
                            &tex.create_view(&wgpu::TextureViewDescriptor::default()),
                        ),
                    ),
                    (
                        1,
                        wgpu::BindingResource::Sampler(&self.device_resources.sky_sampler),
                    ),
                ],
            );
            self.preparation.sky_sources.insert(key, texture.clone());
            self.scene_resources.sky_textures.insert(
                key,
                SkyTextureGpu {
                    first: texture.first,
                    last: texture.last,
                    bind_group,
                },
            );
        }
    }
}

impl ModernRenderer {
    /// Record the sky's decor sprites (the frame's own sky work: the cube carries the rest of the
    /// box) and keep its textures resident.
    pub(crate) fn prepare_sky(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        rect: [i32; 4],
    ) -> Vec<SkyLayerUniforms> {
        use crate::skybox::SkyLayer;
        let mut layers = Vec::new();
        let Some(sky) = snapshot.sky else {
            return layers;
        };
        let [x, y, w, h] = rect;
        // At a render scale (`frame::scale`) the layers are placed in the
        // viewport's own pixels: `params.yz` viewport pixels per target
        // pixel (0: the same), and the sky models' camera keeps its size.
        let (ratio, (w, h)) = self.frame_resources.scaled.map_or(([0.0; 2], (w, h)), |s| {
            (s.ratio(), (s.native_rect[2], s.native_rect[3]))
        });
        // The sky through the modern composite.
        let exposure = self.look.sky_exposure();
        let base = SkyLayerUniforms {
            rect: [x as f32, y as f32, w as f32, h as f32],
            params: [exposure, ratio[0], ratio[1], 0.0],
            ..SkyLayerUniforms::default()
        };
        let push_layer = |layers: &mut Vec<SkyLayerUniforms>,
                          sky_draws: &mut Vec<SkyDraw>,
                          u: SkyLayerUniforms,
                          texture| {
            sky_draws.push(SkyDraw::Layer {
                slot: layers.len() as u32,
                texture,
            });
            layers.push(u);
        };
        let mut sky_draws = std::mem::take(&mut self.frame_resources.sky);
        for layer in sky.layers {
            match layer {
                // The box's fills, clears and dome model are drawn by the sky cube's bake and
                // by the probe captures (`capture_sky_models`), not by the frame.
                SkyLayer::Fill { .. } | SkyLayer::Clear { .. } | SkyLayer::Model { .. } => {}
                // The box's tiled material is drawn by the bake and the captures too; the frame
                // keeps its texture resident for them.
                SkyLayer::Material { key, .. } => {
                    if let Some(texture) = sky.sprite(*key) {
                        self.ensure_sky_texture(
                            device,
                            queue,
                            SkyTextureKey::Material(*key),
                            texture,
                        );
                    }
                }
                SkyLayer::Decor {
                    key,
                    decor,
                    size,
                    direction,
                    pitch,
                    yaw,
                    roll,
                    ..
                } => {
                    // A baked sprite (`sky_decor`) centred on the decor's
                    // direction, blended at the layer's alpha.
                    let Some(sprite) = sky.decor_sprite(*key, *decor) else {
                        continue;
                    };
                    let mut local = snapshot.camera.clone();
                    local.viewport = (w, h);
                    let Some([cx, cy]) = crate::skybox::decor_centre(
                        *direction,
                        (*pitch, *yaw, *roll),
                        local.projection(),
                        (w, h),
                    ) else {
                        continue;
                    };
                    let (x0, y0) = (
                        (cx - (size / 2) as f32) as i32,
                        (cy - (size / 2) as f32) as i32,
                    );
                    if y0 >= h || y0 + size <= 0 || x0 >= w || x0 + size <= 0 {
                        continue;
                    }
                    let texture = SkyTextureKey::Decor(*key, *decor);
                    // The decor fades with the sky's cross-fade (its cube's share), not
                    // with the classic fade the layer list carries.
                    let weight = self.scene_resources.sky_cubes.decor_weight(*key);
                    if weight <= 0.0 {
                        continue;
                    }
                    self.ensure_sky_texture(device, queue, texture, sprite);
                    push_layer(
                        &mut layers,
                        &mut sky_draws,
                        SkyLayerUniforms {
                            colour: [1.0, 1.0, 1.0, weight],
                            tile: [x0 as f32, y0 as f32, *size as f32, 3.0],
                            ..base
                        },
                        Some(texture),
                    );
                }
            }
        }
        self.frame_resources.sky = sky_draws;
        layers
    }
}
