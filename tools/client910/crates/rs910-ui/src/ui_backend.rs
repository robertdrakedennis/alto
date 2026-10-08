//! Retained traversal on the scene target. Unsupported consumers are recorded
//! explicitly by the staging renderer; strict acceptance rejects the frame.
use crate::{
    ui_component_fields::Fields,
    ui_components::Store,
    ui_draw::{Backend, Call, Frame, Kind, Reply},
    ui_leaf::{Context, ObjectText, Services},
    ui_paint::Painter,
    ui_properties::State,
    ui_sprites::Sprite,
};
use anyhow::{Context as _, Result};
pub use rs910_toolkit::frame_plan::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    process::Command,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};
/// A projected `TextCoord` ready for the post-scene overlay pass. Projection
/// belongs to the app camera; the UI backend owns font lookup and painting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneText {
    pub position: [i32; 2],
    pub colour: i32,
    pub text: String,
}
/// A cache-backed NPC head icon projected by the live scene owner.
#[derive(Clone, Debug)]
pub struct SceneIcon {
    /// The draw position of the sprite.
    pub position: [i32; 2],
    pub sprite: Rc<Sprite>,
    /// The owner's own marker: an 18x18 yellow rectangle outline at
    /// `(x - 1, y - 1)`.
    pub outline: bool,
}
pub use crate::ui_output::PostRegion;
/// This frame's UI for the renderers: [`crate::ui_output::Output`] over the
/// interface-model draw.
pub type Output = crate::ui_output::Output<crate::ui_models::Draw>;
#[derive(Default)]
pub struct Resources {
    icons: Option<Rc<std::cell::RefCell<crate::ui_icons::Icons>>>,
    http: Option<HttpSprites>,
    skyboxes: Option<SkyboxResources>,
}

/// Where a world-map label is anchored: the component origin, the label's
/// position relative to the minimap centre and the icon height above it.
#[derive(Clone, Copy)]
struct LabelAnchor {
    origin: [i32; 2],
    rel: [i32; 2],
    sprite_height: i32,
}

struct SkyboxResources {
    pack: crate::cache::Pack,
    materials: Option<crate::texture::MaterialStore>,
    sprites: BTreeMap<i32, Option<Rc<Sprite>>>,
}

impl SkyboxResources {
    fn new(pack: crate::cache::Pack) -> Self {
        Self {
            pack,
            materials: None,
            sprites: BTreeMap::new(),
        }
    }

    fn sprite(&mut self, skybox_id: i32) -> Option<Rc<Sprite>> {
        if let Some(sprite) = self.sprites.get(&skybox_id) {
            return sprite.clone();
        }
        let material_id = skybox_material_id(&self.pack, skybox_id);
        let sprite = material_id
            .and_then(|id| {
                if self.materials.is_none() {
                    self.materials = crate::texture::MaterialStore::load(&self.pack).ok();
                }
                self.materials.as_ref()?.get(id)?.diffuse_texture
            })
            .and_then(|texture| crate::texture::load_texture(&self.pack, texture).ok())
            .and_then(|texture| match texture {
                crate::texture::Texture::Single(image) => Some(image),
                crate::texture::Texture::Cube(faces) => faces.into_iter().next(),
            })
            .map(|image| {
                Rc::new(Sprite {
                    paletted: None,
                    size: [image.w as i32, image.h as i32],
                    padding: [0; 4],
                    argb: image
                        .px
                        .chunks_exact(4)
                        .map(|rgba| {
                            (i32::from(rgba[3]) << 24)
                                | (i32::from(rgba[0]) << 16)
                                | (i32::from(rgba[1]) << 8)
                                | i32::from(rgba[2])
                        })
                        .collect(),
                })
            });
        self.sprites.insert(skybox_id, sprite.clone());
        sprite
    }
}

/// Skybox types are config group 29. The UI component only
/// needs opcode 1 (the material id); the remaining
/// fields describe decor and fill mode consumed by the 3D world owner.
fn skybox_material_id(pack: &crate::cache::Pack, id: i32) -> Option<u32> {
    let files = pack.read_group("config", 29).ok()?;
    let bytes = files.get(&u32::try_from(id).ok()?)?;
    let mut pos = 0;
    let mut material = None;
    while pos < bytes.len() {
        let opcode = bytes[pos];
        pos += 1;
        match opcode {
            0 => break,
            1 => material = Some(u32::from(read_u16(bytes, &mut pos)?)),
            2 => {
                let count = usize::from(*bytes.get(pos)?);
                pos += 1 + count * 2;
            }
            3 | 4 => pos += 1,
            5 | 6 => {
                let first = *bytes.get(pos)?;
                pos += if first < 128 { 2 } else { 4 };
            }
            _ => return None,
        }
        if pos > bytes.len() {
            return None;
        }
    }
    material
}

/// `Packet.g2`, all or nothing (`rs910_core::reader` since Phase 2.1).
fn read_u16(bytes: &[u8], pos: &mut usize) -> Option<u16> {
    let mut r = rs910_core::reader::Reader::at(bytes, *pos);
    let value = r.atomic(rs910_core::reader::Reader::g2).ok()?;
    *pos = r.pos();
    Some(value)
}

struct HttpSprites {
    host: String,
    requests: Sender<(i32, String)>,
    results: Receiver<(i32, Result<Vec<u8>, String>)>,
    pending: BTreeSet<i32>,
    cache: BTreeMap<i32, Option<Rc<Sprite>>>,
}
impl HttpSprites {
    fn new(host: String) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<(i32, String)>();
        let (result_tx, result_rx) = mpsc::channel::<(i32, Result<Vec<u8>, String>)>();
        std::thread::Builder::new()
            .name("client910-http-images".into())
            .spawn(move || {
                while let Ok((id, host)) = request_rx.recv() {
                    let result = fetch_http_png(&host, id);
                    let _ = result_tx.send((id, result));
                }
            })
            .expect("HTTP image worker thread");
        Self {
            host,
            requests: request_tx,
            results: result_rx,
            pending: BTreeSet::new(),
            cache: BTreeMap::new(),
        }
    }
    fn poll(&mut self) {
        while let Ok((id, result)) = self.results.try_recv() {
            self.pending.remove(&id);
            let sprite = result.ok().and_then(|bytes| {
                crate::texture::decode_png(&bytes).ok().and_then(|image| {
                    let pixels = image
                        .px
                        .chunks_exact(4)
                        .map(|rgba| {
                            (i32::from(rgba[3]) << 24)
                                | (i32::from(rgba[0]) << 16)
                                | (i32::from(rgba[1]) << 8)
                                | i32::from(rgba[2])
                        })
                        .collect::<Vec<_>>();
                    (pixels.len()
                        == usize::try_from(image.w)
                            .ok()?
                            .saturating_mul(image.h as usize))
                    .then(|| {
                        Rc::new(Sprite {
                            paletted: None,
                            size: [image.w as i32, image.h as i32],
                            padding: [0; 4],
                            argb: pixels,
                        })
                    })
                })
            });
            self.cache.insert(id, sprite);
        }
    }
    fn sprite(&mut self, id: i32) -> Option<Rc<Sprite>> {
        self.poll();
        if let Some(sprite) = self.cache.get(&id) {
            return sprite.clone();
        }
        if id >= 0 && !self.host.is_empty() && self.pending.insert(id) {
            let _ = self.requests.send((id, self.host.clone()));
        }
        None
    }
}

fn fetch_http_png(host: &str, id: i32) -> Result<Vec<u8>, String> {
    let host = host.trim_end_matches('/');
    if let Some(authority) = host.strip_prefix("https://") {
        let mut parts = authority.splitn(2, '/');
        let authority = parts.next().unwrap_or_default();
        let base_path = parts.next().unwrap_or_default();
        let path = if base_path.is_empty() {
            format!("/img/image_{id}.png")
        } else {
            format!("/{base_path}/img/image_{id}.png")
        };
        let url = format!(
            "https://{authority}{path}?a={}",
            crate::logic_clock::monotonic_millis()
        );
        let output = Command::new("curl")
            .args(["-fsSL", "--connect-timeout", "10", "--max-time", "30", &url])
            .output()
            .map_err(|error| format!("PNG HTTPS fetch could not start curl: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "PNG HTTPS fetch failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        return Ok(output.stdout);
    }
    let authority = host
        .strip_prefix("http://")
        .ok_or_else(|| "IF_SET_HTTP_IMAGE requires an http:// or https:// PNG host".to_string())?;
    let mut parts = authority.splitn(2, '/');
    let authority = parts.next().unwrap_or_default();
    let base_path = parts.next().unwrap_or_default();
    let path = if base_path.is_empty() {
        format!("/img/image_{id}.png")
    } else {
        format!("/{base_path}/img/image_{id}.png")
    };
    let addr = authority
        .to_socket_addrs()
        .map_err(|error| error.to_string())?
        .next()
        .ok_or_else(|| "PNG host resolved to no addresses".to_string())?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| error.to_string())?;
    let request = format!(
        "GET {path}?a={} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nUser-Agent: client910\r\n\r\n",
        crate::logic_clock::monotonic_millis()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "PNG HTTP response has no header terminator".to_string())?;
    let header = std::str::from_utf8(&bytes[..split]).map_err(|error| error.to_string())?;
    if !header.starts_with("HTTP/1.1 200 ") && !header.starts_with("HTTP/1.0 200 ") {
        return Err(format!("PNG HTTP response was not successful: {header}"));
    }
    Ok(bytes[split + 4..].to_vec())
}
impl Services for Resources {
    fn object_text(&mut self, id: i32) -> Result<ObjectText> {
        let icons = self
            .icons
            .as_ref()
            .context("inventory icon provider")?
            .borrow();
        let obj = icons
            .objs
            .get(id as u32)
            .context("object text definition")?;
        Ok(ObjectText {
            name: Some(obj.name.encode_utf16().collect()),
            stackable: obj.inventory.stackable,
        })
    }
    fn object_sprite(&mut self, f: &Fields) -> Result<Option<Rc<Sprite>>> {
        Ok(Some(
            self.icons
                .as_ref()
                .context("inventory icon provider")?
                .borrow_mut()
                .sprite(f)?,
        ))
    }
    fn http_sprite(&mut self, id: i32) -> Result<Option<Rc<Sprite>>> {
        Ok(self.http.as_mut().and_then(|http| http.sprite(id)))
    }
    fn skybox(&mut self, painter: &mut Painter, fields: &Fields, pos: [i32; 2]) -> Result<()> {
        let rect = [pos[0], pos[1], fields.width, fields.height];
        if let Some(sprite) = self
            .skyboxes
            .as_mut()
            .and_then(|skyboxes| skyboxes.sprite(fields.skyboxId))
        {
            painter.scaled(&sprite, rect, -1)?;
        } else {
            painter.fill(rect, fields.colour | i32::MIN)?;
        }
        Ok(())
    }
}
pub struct Target {
    pub models: Option<crate::ui_models::Models>,
    model_draws: Vec<crate::ui_models::Draw>,
    pub leaf: Context,
    resources: Resources,
    pub missing: BTreeMap<String, usize>,
    pub calls: BTreeMap<String, usize>,
    pub strict: bool,
    pub quiet: bool,
    scene: Option<Scene>,
    scene_quad: usize,
    clear: i32,
    /// The minimap inputs for this frame (the app resolves
    /// the base sprite, player position and camera yaw each redraw).
    pub minimap: Option<crate::minimap::Frame>,
    /// The world map state the world map and its overview
    /// `drawWorldMapOverview` read and update (shared with the engine).
    pub world_map: Option<Rc<std::cell::RefCell<crate::world_map_client::ClientWorldMap>>>,
    /// The compass inputs.
    pub compass: Option<crate::minimap::CompassFrame>,
    /// Minimap overlay families without an owner, counted once.
    #[allow(dead_code, reason = "diagnostic gap census; no reader yet")]
    pub minimap_gaps: BTreeMap<&'static str, usize>,
    /// The click-cross sprites and the cross position, mode and cycle
    /// (drawn over the scene viewport).
    pub cross_sprites: Vec<Rc<Sprite>>,
    pub cross: crate::ui_runtime::Cross,
    /// Scene text labels, projected by the live scene camera
    /// and painted after the world pass.
    pub scene_text: Vec<SceneText>,
    /// NPC cover markers, projected by the live
    /// scene camera and painted after the world pass.
    pub scene_icons: Vec<SceneIcon>,
    scene_icon_cache: BTreeMap<(i32, i16), Option<Rc<Sprite>>>,
    /// The gate for opening a post-process capture: the
    /// post-process manager exists and some enabled effect is not a no-op
    /// (`crate::postprocess::Chain::capture`). Set by the frame owner.
    pub postprocess_enabled: bool,
    /// The capture opened at `EnvironmentViewport` and its first painter quad.
    framebuffer: Option<([i32; 2], usize)>,
    postprocess: Option<PostRegion>,
    /// The 2D entity elements (headbars, head icons, hint arrows, hitmarks;
    /// chat drawn after the cover markers) and the tile hint arrows, painted
    /// after the world pass.
    pub entity_elements: crate::entity_elements::Elements,
    pub tile_hint_arrows: Vec<crate::entity_elements::Draw>,
    /// The toolkit clip the scene pass leaves for the 3D entity elements: the
    /// component clip, or the whole canvas after the viewport's letterbox
    /// clip reset.
    scene_element_clip: Option<[i32; 4]>,
    /// Hitmark and headbar sprites, cached (frame 0 per group).
    config_sprite_cache: BTreeMap<i32, Option<Rc<Sprite>>>,
}
impl Target {
    pub fn new(size: [u32; 2]) -> Self {
        Self {
            models: None,
            model_draws: Vec::new(),
            leaf: Context::new(size, Some(crate::ui_text_compare::Language::En), 0),
            resources: Resources::default(),
            missing: BTreeMap::new(),
            calls: BTreeMap::new(),
            strict: false,
            quiet: false,
            scene: None,
            scene_quad: 0,
            clear: 0xff000000u32 as i32,
            minimap: None,
            world_map: None,
            compass: None,
            minimap_gaps: BTreeMap::new(),
            cross_sprites: Vec::new(),
            cross: Default::default(),
            scene_text: Vec::new(),
            scene_icons: Vec::new(),
            scene_icon_cache: BTreeMap::new(),
            postprocess_enabled: false,
            framebuffer: None,
            postprocess: None,
            entity_elements: Default::default(),
            tile_hint_arrows: Vec::new(),
            scene_element_clip: None,
            config_sprite_cache: BTreeMap::new(),
        }
    }
    pub fn with_pack(pack: crate::cache::Pack) -> Result<Self> {
        let mut target = Self::new([1, 1]);
        target.models = Some(crate::ui_models::Models::new(pack.clone()));
        target.resources.skyboxes = Some(SkyboxResources::new(pack));
        Ok(target)
    }
    /// The `pngHost` applet parameter. The worker is deliberately opt-in;
    /// application mode installs it through `CLIENT910_PNG_HOST`, while cache
    /// and replay owners remain offline and deterministic.
    pub fn set_http_host(&mut self, host: Option<String>) {
        self.resources.http = host
            .filter(|host| !host.trim().is_empty())
            .map(HttpSprites::new);
    }
    /// Loads one sprites-archive group/file pair and retains the result for
    /// subsequent scene frames. Missing icons are cached as misses so a bad
    /// server-provided id cannot trigger per-frame disk reads.
    pub fn scene_icon(
        &mut self,
        pack: &crate::cache::Pack,
        group: i32,
        icon: i16,
    ) -> Option<Rc<Sprite>> {
        if group < 0 || icon < 0 {
            return None;
        }
        let key = (group, icon);
        if let Some(sprite) = self.scene_icon_cache.get(&key) {
            return sprite.clone();
        }
        let sprite = crate::minimap::sprite_frames(pack, group)
            .ok()
            .and_then(|frames| frames.get(icon as usize).cloned());
        self.scene_icon_cache.insert(key, sprite.clone());
        sprite
    }
    /// A hitmark or headbar config sprite: the group's first frame. Misses
    /// are cached so a bad config id cannot trigger per-frame reads.
    pub fn config_sprite(&mut self, pack: &crate::cache::Pack, group: i32) -> Option<Rc<Sprite>> {
        if group < 0 {
            return None;
        }
        if let Some(sprite) = self.config_sprite_cache.get(&group) {
            return sprite.clone();
        }
        let sprite = crate::minimap::sprite_frames(pack, group)
            .ok()
            .and_then(|frames| frames.first().cloned());
        self.config_sprite_cache.insert(group, sprite.clone());
        sprite
    }
    /// The 3D entity elements in draw order: text labels, the 2D entity
    /// elements (cover markers before chat), the tile hint arrows and last
    /// the click cross. Called from the scene pass or, while a post-process
    /// capture is open, after the processed layer. The pass never restores
    /// the clip inside itself: each headbar's bounds reset leaves the
    /// viewport clip for everything after it, the cross included. `clip` is
    /// the toolkit clip on entry; the caller's clip is restored afterwards
    /// (the component draw loop sets its own bounds next).
    fn draw_entity_elements(&mut self, state: &mut State, clip: Option<[i32; 4]>) -> Result<()> {
        let old = self.leaf.painter.sprite.clip;
        if let Some(clip) = clip {
            self.leaf.painter.reset_bounds(clip);
        }
        let drawn = self.draw_entity_elements_in_clip(state);
        if drawn.is_ok() {
            self.draw_cross();
        }
        self.leaf.painter.reset_bounds(old);
        drawn
    }
    fn draw_entity_elements_in_clip(&mut self, state: &mut State) -> Result<()> {
        self.draw_scene_text(state)?;
        let elements = std::mem::take(&mut self.entity_elements);
        let drawn = self
            .draw_entity_draws(state, &elements.entities)
            .map(|()| self.draw_scene_icons())
            .and_then(|()| self.draw_entity_draws(state, &elements.chats));
        self.entity_elements = elements;
        drawn?;
        let arrows = std::mem::take(&mut self.tile_hint_arrows);
        let drawn = self.draw_entity_draws(state, &arrows);
        self.tile_hint_arrows = arrows;
        drawn
    }
    /// Paint `entity_elements` commands: sprites through the native sprite
    /// path (`drawSprite(x, y[, 0, colour, 1])`), clips through
    /// bounds set and reset, text at its baseline.
    fn draw_entity_draws(
        &mut self,
        state: &mut State,
        draws: &[crate::entity_elements::Draw],
    ) -> Result<()> {
        use crate::entity_elements::{Draw, FontRef};
        for draw in draws {
            match draw {
                Draw::Sprite {
                    sprite,
                    pos,
                    colour,
                } => self.leaf.painter.native(sprite, *pos, *colour),
                Draw::SetBounds(b) => self.leaf.painter.set_bounds(*b),
                Draw::ResetBounds(b) => self.leaf.painter.reset_bounds(*b),
                Draw::Text {
                    font,
                    text,
                    pos,
                    colour,
                    shadow,
                    offsets,
                } => {
                    let Some(fonts) = state.fonts.as_ref() else {
                        continue;
                    };
                    let loaded = match font {
                        FontRef::P11 => fonts
                            .ids
                            .as_ref()
                            .and_then(|ids| ids.first().copied())
                            .map(|id| fonts.get_font(id, true, true))
                            .transpose()?
                            .flatten(),
                        FontRef::B12 => fonts
                            .ids
                            .as_ref()
                            .and_then(|ids| ids.get(2).copied())
                            .map(|id| fonts.get_font(id, true, true))
                            .transpose()?
                            .flatten(),
                        FontRef::Config { id, mono } => fonts.get_font(*id, true, *mono)?,
                    };
                    let Some(f) = loaded else { continue };
                    // `pos` is the string origin `(x, y)`; the font layout
                    // applies the baseline offset itself.
                    match offsets {
                        None => self
                            .leaf
                            .menu_text(fonts, f, text, *pos, *colour, *shadow)?,
                        // Per-character alpha text.
                        Some((xs, ys)) => self.leaf.alpha_text(
                            fonts,
                            f,
                            text,
                            *pos,
                            crate::font_layout::Style::new(*colour, *shadow),
                            [xs.as_deref(), ys.as_deref()],
                        )?,
                    }
                }
            }
        }
        Ok(())
    }
    /// The component graphic as a mask sprite with the component's size, or
    /// `None` when it is not loaded.
    fn graphic_mask(
        &mut self,
        state: &mut State,
        call: &Call,
    ) -> Result<Option<(Rc<Sprite>, [i32; 2])>> {
        let c = call.component.as_ref().context("minimap component")?;
        let component = c.borrow();
        let f = &component.f;
        let sprites = state
            .sprites
            .as_mut()
            .context("sprite provider not installed")?;
        let mut dirty = false;
        let Some(sprite) = sprites.sprite(f, &mut dirty, &mut crate::ui_sprites::Cpu)? else {
            return Ok(None);
        };
        Ok(Some((sprite, [f.width, f.height])))
    }
    /// Draws the minimap component at `(x, y)`.
    fn draw_minimap(&mut self, state: &mut State, call: &Call) -> Result<()> {
        let [x, y] = [call.args[0], call.args[1]];
        let c = call.component.as_ref().context("minimap component")?;
        // Drawing only reads the component; one shared borrow spans the call.
        let component = c.borrow();
        let f = &component.f;
        // A graphic component supplies the mask and must match its size.
        let mut mask = if f.r#type == 5 {
            let Some((sprite, _)) = self.graphic_mask(state, call)? else {
                return Ok(());
            };
            let [w, h] = sprite.full_size();
            anyhow::ensure!(
                f.width == w && f.height == h,
                "{}",
                rs910_core::fault::Fault::InvalidState.message(format!(
                    "minimap graphic {w}x{h} vs component {}x{}",
                    f.width, f.height
                ))
            );
            Some(crate::ui_paint::MaskRef {
                sprite,
                origin: [x, y],
            })
        } else {
            None
        };
        if crate::toolkit_debug_flags::flags().minimap_dump.is_some() {
            if let Some(m) = &mask {
                let opaque = m
                    .sprite
                    .argb
                    .iter()
                    .filter(|p| (**p as u32) >> 24 != 0)
                    .count();
                log::info!(
                    "[minimap] draw at ({x},{y}) {}x{} mask {:?} opaque {opaque}/{} frame {:?}",
                    f.width,
                    f.height,
                    m.sprite.size,
                    m.sprite.argb.len(),
                    self.minimap.as_ref().map(|m| (
                        m.base.clone(),
                        m.player,
                        m.angle,
                        m.scale,
                        m.toggle
                    ))
                );
            }
            if crate::ui_debug_flags::flags().minimap_nomask {
                mask = None;
            }
        }
        let old = self.leaf.painter.sprite.clip;
        self.leaf
            .painter
            .reset_bounds([x, y, f.width + x, f.height + y]);
        let frame = self.minimap.clone();
        // Toggled off or no base sprite: black through the mask.
        let base = frame.as_ref().and_then(|m| {
            if m.toggle == 2 || m.toggle == 5 {
                None
            } else {
                m.base.clone()
            }
        });
        let Some(base) = base else {
            if let Some(mask) = mask {
                self.leaf.painter.masked_fill(
                    [x, y, f.width, f.height],
                    0xff000000u32 as i32,
                    mask,
                );
            }
            self.leaf.painter.reset_bounds(old);
            return Ok(());
        };
        let m = frame.unwrap();
        // The base pixel under the player and the rotated, scaled draw.
        let px = m.player[0] / 128 + 48;
        let py = m.size_z * 4 + 48 - m.player[1] / 128;
        let origin = [
            f.width as f32 / 2.0 + x as f32,
            f.height as f32 / 2.0 + y as f32,
        ];
        let size = base.size as i32;
        let before = self.leaf.painter.quad_count();
        self.leaf
            .painter
            .rotated_image(crate::ui_paint::RotatedImage {
                image: crate::ui_paint::Image::External(base.texture),
                size: [size, size],
                origin,
                pivot: [px as f32, py as f32],
                scale: m.scale,
                angle: m.angle << 2,
                colour: -1,
                mask: mask.clone(),
            });
        if crate::toolkit_debug_flags::flags().minimap_dump.is_some() {
            let after = self.leaf.painter.quad_count();
            let last = self
                .leaf
                .painter
                .quads
                .last()
                .map(|q| (q.vertices.map(|v| v.position), q.clip, q.mask.is_some()));
            log::info!("[minimap] composite at ({x},{y}) {}x{} type {} origin {origin:?} pivot ({px},{py}) scale {} angle {} base {} quads {before}->{after} last {last:?} overlays {} (angle {} zoom {})", f.width, f.height, f.r#type, m.scale, m.angle, base.texture, m.overlays.len(), m.overlay_angle, m.zoom);
            for o in m.overlays.iter().filter(|o| o.scale == 100.0).take(12) {
                log::info!(
                    "[minimap]   overlay d {:?} sprite {:?} scale {} align {:?} offset {:?}",
                    o.d,
                    o.sprite.full_size(),
                    o.scale,
                    o.align,
                    o.offset
                );
            }
        }
        // Map elements, loc icons, obj stacks, NPC/player dots,
        // hint arrows and the map flag are resolved by the app
        // (`minimap::Minimap::overlays`); edge arrows are rotated below.
        for o in &m.overlays {
            self.draw_on_minimap(state, f, [x, y], &m, o, mask.clone())?;
        }
        // The local player's 3x3 white square.
        if !m.player_hidden {
            self.leaf
                .painter
                .fill([f.width / 2 + x - 1, f.height / 2 + y - 1, 3, 3], -1)?;
        }
        self.leaf.painter.reset_bounds(old);
        Ok(())
    }
    /// Draws one overlay sprite on the minimap: rotate the tile offset by the
    /// overlay angle (zoom-scaled unless camera state 4), align, then draw.
    /// The unmasked scale-1 path draws the sprite twice (once plain, once
    /// tinted and scaled, with no `else` between them); kept as is.
    fn draw_minimap_polygon(
        &mut self,
        f: &Fields,
        origin: [i32; 2],
        polygon: &crate::minimap::WorldMapPolygon,
        sin: i32,
        cos: i32,
        mask: Option<crate::ui_paint::MaskRef>,
    ) -> Result<()> {
        let points: Vec<[i32; 2]> = polygon
            .points
            .iter()
            .map(|point| {
                let rx = (point[0] * cos + point[1] * sin) >> 14;
                let ry = (point[1] * cos - point[0] * sin) >> 14;
                [f.width / 2 + origin[0] + rx, f.height / 2 + origin[1] - ry]
            })
            .collect();
        let Some(mask) = mask else {
            // No component graphic, no fill. The unmasked edges are drawn as
            // lines.
            for line in &polygon.lines {
                let project = |point: [i32; 2]| {
                    let rx = (point[0] * cos + point[1] * sin) >> 14;
                    let ry = (point[1] * cos - point[0] * sin) >> 14;
                    [f.width / 2 + origin[0] + rx, f.height / 2 + origin[1] - ry]
                };
                self.leaf.painter.dashed_line(
                    project(line.from),
                    project(line.to),
                    Self::world_map_colour(line.colour),
                    line.dash,
                    line.gap,
                    line.phase,
                );
            }
            return Ok(());
        };
        // The polygon fill runs through the graphic's row spans. The masked
        // edge calls are empty on this toolkit.
        self.fill_polygon_argb_masked(&points, Self::world_map_colour(polygon.fill), Some(mask))
    }
    fn draw_minimap_label(
        &mut self,
        state: &mut State,
        f: &Fields,
        anchor: LabelAnchor,
        label: &crate::minimap::WorldMapLabel,
        mask: Option<crate::ui_paint::MaskRef>,
    ) -> Result<()> {
        let LabelAnchor {
            origin,
            rel: [rx, ry],
            sprite_height,
        } = anchor;
        let Some(fonts) = state.fonts.as_ref() else {
            return Ok(());
        };
        let Some(&font_id) = fonts
            .ids
            .as_ref()
            .and_then(|ids| ids.get(label.size.clamp(0, 2) as usize))
        else {
            return Ok(());
        };
        let Some(font) = fonts.get_font(font_id, true, true)? else {
            return Ok(());
        };
        let units: Vec<u16> = label.text.encode_utf16().collect();
        let width = font.metrics.width_utf16(&units, None)?;
        let (_, lines) = fonts.lines(&font.metrics, Some(&units), 100, true)?;
        let line_count = lines.max(1) as i32;
        let height = font.metrics.ascent
            + font.metrics.descent
            + line_count.saturating_sub(1) * font.metrics.space_width;
        // The label is culled against its transformed centre,
        // then draws the baseline above the icon by its sprite height and
        // paragraph height.
        let text_offset = rx - width / 2;
        if text_offset < -f.width || text_offset > f.width || ry < -f.height || ry > f.height {
            return Ok(());
        }
        let pos = [
            f.width / 2 + origin[0] + text_offset,
            f.height / 2 + origin[1] - ry - sprite_height - height,
        ];
        let colour = 0xff00_0000u32 as i32 | (label.colour & 0x00ff_ffff);
        if let Some(mask) = mask {
            self.leaf.menu_text_masked(
                fonts,
                font,
                &label.text,
                pos,
                crate::font_layout::Style::new(colour, 0),
                mask,
            )?;
        } else {
            self.leaf
                .menu_text(fonts, font, &label.text, pos, colour, 0)?;
        }
        Ok(())
    }
    fn draw_scene_text(&mut self, state: &mut State) -> Result<()> {
        let Some(fonts) = state.fonts.as_ref() else {
            return Ok(());
        };
        let Some(&font_id) = fonts.ids.as_ref().and_then(|ids| ids.get(2)) else {
            return Ok(());
        };
        let Some(font) = fonts.get_font(font_id, true, true)? else {
            return Ok(());
        };
        for label in &self.scene_text {
            let units: Vec<u16> = label.text.encode_utf16().collect();
            let width = font.metrics.width_utf16(&units, None)?;
            // Centred text: `y` is the baseline, as `menu_text` takes it.
            let pos = [label.position[0] - width / 2, label.position[1]];
            self.leaf.menu_text(
                fonts,
                font.clone(),
                &label.text,
                pos,
                0xff00_0000u32 as i32 | (label.colour & 0x00ff_ffff),
                0,
            )?;
        }
        Ok(())
    }
    /// The debug overlay: right-aligned at the component's right edge,
    /// baselines from `y + 15`; nothing unless the debug display is on.
    fn draw_debug(&mut self, state: &mut State, call: &Call) -> Result<()> {
        if !state.debug_visible[0] {
            return Ok(());
        }
        let c = call.component.as_ref().context("debug component")?;
        let right = call.args[0].wrapping_add(c.borrow().f.width);
        let top = call.args[1];
        let fonts = state.fonts.as_ref().context("default fonts")?;
        let ids = fonts.ids.as_ref().context("default font ids")?;
        for line in crate::debug_overlay::lines(&state.debug_stats) {
            // The default 11 and 12 point fonts (font ids 0 and 1).
            let id = *ids.get(line.font).context("debug font id")?;
            let font = fonts.get_font(id, true, true)?.context("debug font")?;
            let units: Vec<u16> = line.text.encode_utf16().collect();
            let width = font.metrics.width_utf16(&units, None)?;
            let baseline = top + line.baseline;
            self.leaf.menu_text(
                fonts,
                font.clone(),
                &line.text,
                // Right-aligned text: `y` is the baseline.
                [right - width, baseline],
                line.colour,
                -1,
            )?;
        }
        Ok(())
    }
    /// Draws the click cross.
    fn draw_cross(&mut self) {
        let cross = self.cross;
        if cross.mode != 0 {
            let frame = (cross.cycle / 100) as usize + if cross.mode == 2 { 4 } else { 0 };
            if let Some(sprite) = self.cross_sprites.get(frame).cloned() {
                self.leaf
                    .painter
                    .native(&sprite, [cross.x - 8, cross.y - 8], -1);
            }
        }
    }
    /// Draws each cover marker.
    fn draw_scene_icons(&mut self) {
        for icon in &self.scene_icons {
            let [x, y] = icon.position;
            self.leaf.painter.native(&icon.sprite, [x, y], -1);
            if icon.outline {
                self.leaf.painter.outline([x - 1, y - 1, 18, 18], -256);
            }
        }
    }
    fn draw_on_minimap(
        &mut self,
        state: &mut State,
        f: &Fields,
        at: [i32; 2],
        m: &crate::minimap::Frame,
        o: &crate::minimap::Overlay,
        mask: Option<crate::ui_paint::MaskRef>,
    ) -> Result<()> {
        let [x, y] = [at[0] + o.offset[0], at[1] + o.offset[1]];
        let limit = (f.width / 2).max(f.height / 2) + 10;
        let [dx, dz] = o.d;
        if dx * dx + dz * dz > limit * limit && !o.edge_arrow && o.polygon.is_none() {
            return Ok(());
        }
        let (mut sin, mut cos) = crate::minimap::trig1(m.overlay_angle);
        if !m.camera_state_4 {
            sin = sin * 256 / (m.zoom + 256);
            cos = cos * 256 / (m.zoom + 256);
        }
        let rx = (dx * cos + dz * sin) >> 14;
        let ry = (dz * cos - dx * sin) >> 14;
        if let Some(polygon) = o.polygon.as_ref() {
            self.draw_minimap_polygon(f, [x, y], polygon, sin, cos, mask.clone())?;
        }
        if !o.draw_sprite {
            if let Some(label) = o.label.as_ref() {
                self.draw_minimap_label(
                    state,
                    f,
                    LabelAnchor {
                        origin: [x, y],
                        rel: [rx, ry],
                        sprite_height: 0,
                    },
                    label,
                    mask,
                )?;
            }
            return Ok(());
        }
        if o.edge_arrow && (rx * rx + ry * ry > limit * limit) {
            let sprite = o.edge_sprite.as_ref().unwrap_or(&o.sprite);
            let [sw, sh] = sprite.full_size();
            let angle = f64::from(rx).atan2(f64::from(ry));
            let slope = f64::from(rx.abs()).atan2(f64::from(ry.abs()));
            let aspect = f64::from(f.width / 2).atan2(f64::from(f.height / 2));
            let (boundary, angle) = if slope < aspect {
                (f.height / 2, std::f64::consts::FRAC_PI_2 - angle)
            } else {
                (f.width / 2, angle)
            };
            let radius = (f64::from(boundary) / angle.sin().abs().max(f64::EPSILON)) as i32;
            let centre = [
                f.width as f32 / 2.0 + x as f32,
                f.height as f32 / 2.0 + y as f32,
            ];
            let rotation = (-angle / (std::f64::consts::PI * 2.0) * 65535.0) as i32;
            self.leaf
                .painter
                .rotated_image(crate::ui_paint::RotatedImage {
                    image: crate::ui_paint::Image::Sprite(sprite.clone()),
                    size: [sw, sh],
                    origin: centre,
                    pivot: [sw as f32 / 2.0, radius as f32],
                    scale: 4096,
                    angle: rotation,
                    colour: -1,
                    mask: mask.clone(),
                });
            if let Some(label) = o.label.as_ref() {
                self.draw_minimap_label(
                    state,
                    f,
                    LabelAnchor {
                        origin: [x, y],
                        rel: [rx, ry],
                        sprite_height: sprite.size[1],
                    },
                    label,
                    mask,
                )?;
            }
            return Ok(());
        }
        let scale = o.scale / 100.0;
        let [sw, sh] = o.sprite.full_size();
        let px = match o.align[0] {
            0 => f.width / 2 + x + rx - (f64::from(sw) * scale) as i32,
            1 => f.width / 2 + x + rx,
            _ => f.width / 2 + x + rx - (f64::from(sw / 2) * scale) as i32,
        };
        let py = match o.align[1] {
            0 => f.height / 2 + y - ry - (f64::from(sh) * scale) as i32,
            1 => f.height / 2 + y - ry,
            _ => f.height / 2 + y - ry - (f64::from(sh / 2) * scale) as i32,
        };
        let scaled = [
            px,
            py,
            (f64::from(sw) * scale) as i32,
            (f64::from(sh) * scale) as i32,
        ];
        match mask.clone() {
            None => {
                if scale == 1.0 {
                    self.leaf.painter.native(&o.sprite, [px, py], -1);
                }
                self.leaf.painter.scaled(&o.sprite, scaled, -1)?;
            }
            Some(mask) if scale == 1.0 => {
                self.leaf.painter.masked_native(&o.sprite, [px, py], mask)
            }
            Some(_) => self.leaf.painter.scaled(&o.sprite, scaled, -1)?,
        }
        if let Some(label) = o.label.as_ref() {
            self.draw_minimap_label(
                state,
                f,
                LabelAnchor {
                    origin: [x, y],
                    rel: [rx, ry],
                    sprite_height: o.sprite.size[1],
                },
                label,
                mask,
            )?;
        }
        Ok(())
    }
    /// Draws the compass at `(x, y)`.
    fn draw_compass(&mut self, state: &mut State, call: &Call) -> Result<()> {
        let [x, y] = [call.args[0], call.args[1]];
        let Some((sprite, [width, height])) = self.graphic_mask(state, call)? else {
            return Ok(());
        };
        let mask = crate::ui_paint::MaskRef {
            sprite,
            origin: [x, y],
        };
        let old = self.leaf.painter.sprite.clip;
        self.leaf
            .painter
            .reset_bounds([x, y, width + x, height + y]);
        let Some(compass) = self.compass.clone() else {
            self.leaf.painter.reset_bounds(old);
            anyhow::bail!("compass sprite (graphics defaults) not installed");
        };
        if compass.toggle >= 3 {
            self.leaf
                .painter
                .masked_fill([x, y, width, height], 0xff000000u32 as i32, mask);
        } else {
            // Rotated about the sprite centre at scale 4226.
            let [w, h] = compass.sprite.full_size();
            self.leaf
                .painter
                .rotated_image(crate::ui_paint::RotatedImage {
                    image: crate::ui_paint::Image::Sprite(compass.sprite.clone()),
                    size: [w, h],
                    origin: [
                        width as f32 / 2.0 + x as f32,
                        height as f32 / 2.0 + y as f32,
                    ],
                    pivot: [w as f32 / 2.0, h as f32 / 2.0],
                    scale: 4226,
                    angle: compass.angle << 2,
                    colour: -1,
                    mask: Some(mask),
                });
        }
        self.leaf.painter.reset_bounds(old);
        Ok(())
    }
    fn world_map_colour(raw: i32) -> i32 {
        let value = raw as u32;
        if value == 0 {
            0
        } else if value >> 24 == 0 {
            (value | 0xff00_0000) as i32
        } else {
            raw
        }
    }
    /// The scanline fill (`crate::world_map_polygon::spans`) over the
    /// current clip rows.
    fn fill_polygon_argb_masked(
        &mut self,
        points: &[[i32; 2]],
        colour: i32,
        mask: Option<crate::ui_paint::MaskRef>,
    ) -> Result<()> {
        if colour == 0 {
            return Ok(());
        }
        let flat: Vec<i32> = points.iter().flat_map(|p| [p[0], p[1]]).collect();
        let clip = self.leaf.painter.sprite.clip;
        for span in crate::world_map_polygon::spans(&flat, clip[1], clip[3], None) {
            match mask.as_ref() {
                // The Graphic row clip (:69-77) keeps positive spans; the
                // component mask sprite bounds the pixels.
                Some(mask) => {
                    if span.len > 0 {
                        self.leaf.painter.masked_fill(
                            [span.x, span.y, span.len + 1, 1],
                            colour,
                            mask.clone(),
                        );
                    }
                }
                // A horizontal line is drawn as a line to `x + len`.
                None => {
                    if span.len != 0 {
                        self.leaf.painter.line(
                            [span.x, span.y],
                            [span.x + span.len, span.y],
                            colour,
                            1,
                        );
                    }
                }
            }
        }
        Ok(())
    }
    /// Draws the world map or its overview at the component (the clip is
    /// reset to the component first).
    fn draw_world_map(&mut self, state: &mut State, call: &Call) -> Result<()> {
        let [x, y] = [call.args[0], call.args[1]];
        let c = call.component.as_ref().context("world-map component")?;
        let [width, height] = {
            let f = &c.borrow().f;
            [f.width, f.height]
        };
        let Some(map) = self.world_map.clone() else {
            return Ok(());
        };
        let old = self.leaf.painter.sprite.clip;
        self.leaf
            .painter
            .reset_bounds([x, y, width + x, height + y]);
        let canvas_width = state.layout.canvas[0];
        {
            let mut map = map
                .try_borrow_mut()
                .map_err(|_| anyhow::anyhow!("world map busy"))?;
            let mut canvas = WorldMapCanvas {
                leaf: &mut self.leaf,
                fonts: state.fonts.as_ref(),
                canvas_width,
            };
            if call.kind == Kind::WorldMapOverview {
                map.draw_overview(&mut canvas, [x, y, width, height]);
            } else {
                map.draw(&mut canvas, [x, y, width, height]);
            }
        }
        // With the debug display on, the map shows the FPS and used-memory
        // lines in its bottom-right corner.
        if call.kind == Kind::WorldMap && map.borrow().loading >= 100 && state.debug_visible[0] {
            self.draw_world_map_debug(state, [x + width - 5, y + height - 8])?;
        }
        self.leaf.painter.reset_bounds(old);
        Ok(())
    }
    fn draw_world_map_debug(&mut self, state: &State, at: [i32; 2]) -> Result<()> {
        let fonts = state.fonts.as_ref().context("default fonts")?;
        let id = *fonts
            .ids
            .as_ref()
            .and_then(|ids| ids.get(1))
            .context("p12 font id")?;
        let font = fonts.get_font(id, true, true)?.context("p12 font")?;
        let stats = &state.debug_stats;
        let used = stats.mem_used_k;
        let lines = [
            (
                format!("Fps: {} ({} ms)", stats.fps, stats.fps_average),
                16776960,
            ),
            (
                format!("Mem:{used}k"),
                if used > 65536 { 16711680 } else { 16776960 },
            ),
        ];
        let mut baseline = at[1];
        for (text, colour) in lines {
            let units: Vec<u16> = text.encode_utf16().collect();
            let width = font.metrics.width_utf16(&units, None)?;
            self.leaf.menu_text(
                fonts,
                font.clone(),
                &text,
                // Right-aligned text: `y` is the baseline.
                [at[0] - width, baseline],
                0xff00_0000u32 as i32 | colour,
                -1,
            )?;
            baseline -= 15;
        }
        Ok(())
    }
    pub fn begin(&mut self, size: [u32; 2], clear: [f32; 3]) {
        if let Some(http) = self.resources.http.as_mut() {
            http.poll();
        }
        self.model_draws.clear();
        self.leaf.painter = Painter::new(size);
        self.scene = None;
        self.scene_quad = 0;
        self.framebuffer = None;
        self.postprocess = None;
        self.calls.clear();
        self.clear = 0xff000000u32 as i32
            | ((clear[0] * 255.) as i32) << 16
            | ((clear[1] * 255.) as i32) << 8
            | (clear[2] * 255.) as i32;
    }
    pub fn finish(&mut self) -> Output {
        let size = self.leaf.painter.sprite.size;
        let mut painter = std::mem::replace(&mut self.leaf.painter, Painter::new(size));
        let recording = std::mem::take(&mut painter.recording);
        let layers = std::mem::take(&mut painter.layers);
        let paint = painter.finish();
        Output {
            recording,
            layers,
            models: std::mem::take(&mut self.model_draws),
            scene_quad: if self.scene.is_some() {
                self.scene_quad
            } else {
                paint.quads.len()
            },
            paint,
            scene: self.scene.take(),
            postprocess: self.postprocess.take(),
        }
    }
    fn unsupported(&mut self, call: &Call, error: anyhow::Error) -> Result<Reply> {
        let component = call.component.as_ref().map(|c| {
            let c = c.borrow();
            (c.f.parentlayer, c.f.id, c.f.clientcode)
        });
        let key = format!("{:?} {component:?}: {error:#}", call.kind);
        if self.strict {
            anyhow::bail!(key);
        }
        if self.missing.contains_key(&key) || self.missing.len() < 128 {
            let count = self.missing.entry(key.clone()).or_default();
            if *count == 0 && !self.quiet {
                log::warn!("[client910] staged UI missing {key}");
            }
            *count += 1;
        }
        // Only paint/side-effect calls can be skipped in a diagnostic frame.
        // An unknown query must stop traversal instead of choosing a branch.
        anyhow::ensure!(
            !matches!(
                call.kind,
                Kind::Preview
                    | Kind::Streaming
                    | Kind::StreamReady
                    | Kind::FramebufferEnabled
                    | Kind::FramebufferSize
                    | Kind::GraphicReady
            ),
            "unbound UI query {key}"
        );
        Ok(Reply::Unit)
    }
    fn dispatch(&mut self, frame: &mut Frame, state: &mut State, call: &Call) -> Result<Reply> {
        self.resources.icons = state.icons.clone();
        if let Some(r) = self.leaf.call(frame, state, &mut self.resources, call)? {
            return Ok(r);
        }
        match call.kind {
            Kind::Model => {
                let c = call.component.as_ref().context("UI model component")?;
                let clip = call.args[3..7].try_into().unwrap();
                if let Some(draw) = self.models.as_mut().context("UI model provider")?.draw(
                    c,
                    state,
                    crate::ui_models::ModelPlacement {
                        at: [call.args[0], call.args[1]],
                        canvas: state.layout.canvas,
                        clip,
                        quad: self.leaf.painter.quad_count(),
                        before_scene: self.scene.is_none(),
                    },
                )? {
                    self.model_draws.push(draw);
                }
                Ok(Reply::Unit)
            }
            // The frame owner applies the current environment's bloom/levels/colour
            // remapping to the renderer before traversal (`postprocess.rs`).
            Kind::Environment => Ok(Reply::Unit),
            // Opens the capture at the layer origin when an effect is live.
            Kind::EnvironmentViewport => {
                if self.postprocess_enabled {
                    let origin = [call.args[0], call.args[1]];
                    self.framebuffer = Some((origin, self.leaf.painter.quad_count()));
                }
                Ok(Reply::Unit)
            }
            // Whether a capture is open.
            Kind::FramebufferEnabled => Ok(Reply::Bool(self.framebuffer.is_some())),
            // Resolves the chain into the layer bounds.
            Kind::FramebufferSize => {
                let ([x, y], start_quad) = self
                    .framebuffer
                    .take()
                    .context("framebuffer size without an open capture")?;
                let region = PostRegion {
                    rect: [x, y, call.args[0], call.args[1]],
                    start_quad,
                    end_quad: self.leaf.painter.quad_count(),
                };
                if crate::toolkit_debug_flags::flags().postfx_trace {
                    log::info!(
                        "[postfx] capture {:?} quads {}..{} scene_quad {}",
                        region.rect,
                        region.start_quad,
                        region.end_quad,
                        self.scene_quad
                    );
                }
                self.postprocess = Some(region);
                Ok(Reply::Unit)
            }
            // With the capture resolved, the 3D entity elements run after
            // the post-processed layer instead of inside the scene pass.
            Kind::EntityOverlays => {
                self.draw_entity_elements(state, None)?;
                self.leaf.painter.quad_count();
                Ok(Reply::Unit)
            }
            // Entities draw in the world pass between the UI before/after
            // splits (player_renderer), not as UI quads. This preserves the
            // Flush/Scene/.../Reset2d order around the scene viewport.
            Kind::EntityLayers => Ok(Reply::Unit),
            Kind::Flush => {
                self.leaf.painter.quad_count();
                Ok(Reply::Unit)
            }
            Kind::Reset2d => {
                // Inside the scene pass only while no capture is open; an
                // open capture defers these to `Kind::EntityOverlays`.
                if self.framebuffer.is_none() {
                    let clip = self.scene_element_clip.take();
                    self.draw_entity_elements(state, clip)?;
                }
                self.leaf.painter.quad_count();
                Ok(Reply::Unit)
            }
            Kind::ClientComponent => {
                // Only the player-model client component mutates state.
                let c = call.component.as_ref().context("client component")?;
                anyhow::ensure!(
                    c.borrow().f.clientcode != 328,
                    "player model component owner"
                );
                Ok(Reply::Unit)
            }
            // The streaming platform is unsupported: no preview image, nothing
            // streaming, so the Twitch components draw nothing.
            Kind::Preview => Ok(Reply::Size(None)),
            Kind::Streaming | Kind::StreamReady => Ok(Reply::Bool(false)),
            Kind::GraphicReady => Ok(Reply::Bool(
                state
                    .component_graphic(
                        call.component.as_ref().context("graphic component")?,
                        &mut crate::ui_sprites::Cpu,
                    )?
                    .is_some(),
            )),
            Kind::Minimap => {
                self.draw_minimap(state, call)?;
                Ok(Reply::Unit)
            }
            Kind::Compass => {
                self.draw_compass(state, call)?;
                Ok(Reply::Unit)
            }
            Kind::WorldMap | Kind::WorldMapOverview => {
                self.draw_world_map(state, call)?;
                Ok(Reply::Unit)
            }
            Kind::Debug => {
                self.draw_debug(state, call)?;
                Ok(Reply::Unit)
            }
            Kind::Scene => {
                anyhow::ensure!(
                    self.scene.is_none(),
                    "multiple scene viewports in one frame"
                );
                let requested = call.args[..4].try_into()?;
                let rect = state.set_viewport(requested)?;
                anyhow::ensure!(rect[2] > 0 && rect[3] > 0, "empty scene viewport");
                state.scene_without_projectiles = call.args[4] != 0;
                // Setting the viewport paints the letterbox bars
                // before restoring the effective scene clip. The surface is
                // otherwise left untouched by the scene pass.
                if rect != requested {
                    self.paint_letterbox(requested, rect)?;
                }
                let old = self.leaf.painter.sprite.clip;
                self.leaf.painter.set_bounds([
                    rect[0],
                    rect[1],
                    rect[0] + rect[2],
                    rect[1] + rect[3],
                ]);
                let clip = self.leaf.painter.sprite.clip;
                self.leaf.painter.fill(rect, self.clear)?;
                self.scene_quad = self.leaf.painter.quad_count();
                // The 3D entity elements (and the click cross) run at
                // `Kind::Reset2d`, after the world pass, under the clip the
                // scene pass leaves: letterboxing reset it to the canvas.
                self.scene_element_clip = Some(if rect != requested {
                    let [w, h] = self.leaf.painter.sprite.size.map(|v| v as i32);
                    [0, 0, w, h]
                } else {
                    old
                });
                self.leaf.painter.reset_bounds(old);
                self.scene = Some(Scene { rect, clip });
                Ok(Reply::Unit)
            }
            _ => anyhow::bail!("consumer not connected"),
        }
    }
}
impl Target {
    /// The two black bars a scene viewport of another shape than `requested`
    /// leaves outside its effective `rect`: at the sides when the view is
    /// too wide, above and below when too tall. The clip is reset to the
    /// canvas first.
    fn paint_letterbox(&mut self, requested: [i32; 4], rect: [i32; 4]) -> Result<()> {
        let [w, h] = self.leaf.painter.sprite.size.map(|v| v as i32);
        self.leaf.painter.reset_bounds([0, 0, w, h]);
        let [x, y, width, height] = requested;
        let black = 0xff000000u32 as i32;
        if rect[0] != x {
            let bar = rect[0] - x;
            self.leaf.painter.fill([x, y, bar, height], black)?;
            self.leaf
                .painter
                .fill([x + width - bar, y, bar, height], black)?;
        } else {
            let bar = rect[1] - y;
            self.leaf.painter.fill([x, y, width, bar], black)?;
            self.leaf
                .painter
                .fill([x, y + height - bar, width, bar], black)?;
        }
        Ok(())
    }
}

impl Backend for Target {
    fn call(
        &mut self,
        frame: &mut Frame,
        _: &mut Store,
        state: &mut State,
        call: Call,
    ) -> Result<Reply> {
        *self.calls.entry(format!("{:?}", call.kind)).or_default() += 1;
        match self.dispatch(frame, state, &call) {
            Ok(r) => Ok(r),
            Err(error) => self.unsupported(&call, error),
        }
    }
}

/// The toolkit calls of `ClientWorldMap`/`WorldMap` over the shared painter
/// (with the toolkit's semantics) and the default font provider.
struct WorldMapCanvas<'a> {
    leaf: &'a mut Context,
    fonts: Option<&'a crate::ui_fonts::Fonts>,
    canvas_width: i32,
}
impl WorldMapCanvas<'_> {
    fn font(&self, id: i32) -> Option<Rc<crate::ui_fonts::Font>> {
        self.fonts?.get_font(id, true, true).ok().flatten()
    }
}
impl crate::world_map::Canvas for WorldMapCanvas<'_> {
    fn sprite(&mut self, sprite: &Rc<Sprite>, pos: [i32; 2]) {
        self.leaf.painter.native(sprite, pos, -1);
    }
    fn scaled(&mut self, sprite: &Rc<Sprite>, rect: [i32; 4]) {
        let _ = self.leaf.painter.scaled(sprite, rect, -1);
    }
    fn sprite_tinted(&mut self, sprite: &Rc<Sprite>, pos: [i32; 2], colour: i32) {
        self.leaf.painter.native(sprite, pos, colour);
    }
    fn rotated(&mut self, sprite: &Rc<Sprite>, centre: [f32; 2], angle: i32) {
        // Rotate about the sprite centre at scale 4096.
        let [w, h] = sprite.full_size();
        self.leaf
            .painter
            .rotated_image(crate::ui_paint::RotatedImage {
                image: crate::ui_paint::Image::Sprite(sprite.clone()),
                size: [w, h],
                origin: centre,
                pivot: [w as f32 / 2.0, h as f32 / 2.0],
                scale: 4096,
                angle,
                colour: -1,
                mask: None,
            });
    }
    fn fill(&mut self, rect: [i32; 4], colour: i32) {
        let _ = self.leaf.painter.fill(rect, colour);
    }
    fn outline(&mut self, rect: [i32; 4], colour: i32) {
        self.leaf.painter.outline(rect, colour);
    }
    fn line(&mut self, from: [i32; 2], to: [i32; 2], colour: i32, style: [i32; 3]) {
        self.leaf.painter.dashed_line(
            from,
            to,
            Target::world_map_colour(colour),
            style[0],
            style[1],
            style[2],
        );
    }
    fn polygon(&mut self, points: &[i32], colour: i32) {
        // Row spans, each drawn as a horizontal line.
        let colour = Target::world_map_colour(colour);
        if colour == 0 {
            return;
        }
        let clip = self.leaf.painter.sprite.clip;
        for span in crate::world_map_polygon::spans(points, clip[1], clip[3], None) {
            if span.len != 0 {
                self.leaf
                    .painter
                    .line([span.x, span.y], [span.x + span.len, span.y], colour, 1);
            }
        }
    }
    fn measure(&mut self, font: i32, text: &str) -> Option<[i32; 2]> {
        self.fonts?.paragraph_size(font, text, self.canvas_width)
    }
    fn text(&mut self, font: i32, text: &str, rect: [i32; 4], colour: i32, shadow: i32) {
        let (Some(fonts), Some(font)) = (self.fonts, self.font(font)) else {
            return;
        };
        let _ = self.leaf.paragraph_text(
            fonts,
            font,
            text,
            crate::font_layout::Paragraph {
                x: rect[0],
                y: rect[1],
                width: rect[2],
                height: rect[3],
                colour,
                shadow,
                halign: 1,
                valign: 0,
                line_height: 0,
                max_lines: 0,
                masked: false,
            },
        );
    }
    fn text_centre(&mut self, text: &str, pos: [i32; 2], colour: i32) {
        let Some(fonts) = self.fonts else { return };
        let Some(&id) = fonts.ids.as_ref().and_then(|ids| ids.get(2)) else {
            return;
        };
        let Some(font) = self.font(id) else { return };
        let units: Vec<u16> = text.encode_utf16().collect();
        let width = fonts.width(&font.metrics, Some(&units)).unwrap_or(0);
        let _ = self
            .leaf
            .menu_text(fonts, font, text, [pos[0] - width / 2, pos[1]], colour, -1);
    }
}

#[cfg(test)]
mod scene_overlay_order_tests {
    use super::*;
    use crate::entity_elements::Draw;
    use crate::ui_paint::Op;

    fn sprite(argb: i32) -> Rc<Sprite> {
        Rc::new(Sprite {
            paletted: None,
            size: [1, 1],
            padding: [0; 4],
            argb: vec![argb],
        })
    }

    /// The click cross is drawn last, after the entity elements, under the
    /// clip the headbar bounds reset left (the viewport), and the caller's
    /// clip comes back after.
    #[test]
    fn cross_draws_after_entity_elements_in_viewport_clip() {
        let mut target = Target::new([800, 600]);
        let mut state = State::default();
        let (bar, head, cross) = (sprite(1), sprite(2), sprite(3));
        target.cross_sprites = vec![cross.clone()];
        target.cross = crate::ui_runtime::Cross {
            x: 100,
            y: 100,
            mode: 1,
            cycle: 0,
        };
        let viewport = [10, 20, 510, 420];
        target.entity_elements.entities = vec![
            Draw::Sprite {
                sprite: bar.clone(),
                pos: [50, 50],
                colour: -1,
            },
            Draw::SetBounds([50, 50, 51, 51]),
            Draw::ResetBounds(viewport),
            Draw::Sprite {
                sprite: head.clone(),
                pos: [50, 40],
                colour: -1,
            },
        ];
        let component_clip = [0, 0, 700, 500];
        target.leaf.painter.reset_bounds([0, 0, 800, 600]);
        let start = target.leaf.painter.recording.ops.len();
        target
            .draw_entity_elements(&mut state, Some(component_clip))
            .unwrap();
        let ops = &target.leaf.painter.recording.ops[start..];
        let summary: Vec<String> = ops
            .iter()
            .map(|op| match op {
                Op::Sprite(s, _, _) => format!("sprite{}", s.argb[0]),
                Op::SetBounds(b) => format!("set{b:?}"),
                Op::ResetBounds(b) => format!("reset{b:?}"),
                _ => "other".into(),
            })
            .collect();
        assert_eq!(
            summary,
            [
                "reset[0, 0, 700, 500]",
                "sprite1",
                "set[50, 50, 51, 51]",
                "reset[10, 20, 510, 420]",
                "sprite2",
                "sprite3",
                "reset[0, 0, 800, 600]",
            ]
        );
        let Op::Sprite(_, pos, _) = &ops[5] else {
            unreachable!()
        };
        assert_eq!(*pos, [92, 92], "drawCrossSprites draws at crossX - 8");
    }
}

#[cfg(test)]
mod platform_component_tests {
    use super::*;
    use crate::ui_draw::Frame;

    fn call(target: &mut Target, state: &mut State, kind: Kind, args: &[i32]) -> Reply {
        let mut frame = Frame::default();
        let mut store = Store::default();
        target
            .call(
                &mut frame,
                &mut store,
                state,
                Call {
                    kind,
                    component: None,
                    args: args.to_vec(),
                },
            )
            .expect("backend call")
    }

    /// The Twitch preview and stream components ask the platform, which has no
    /// streaming: both answer so the walk goes on and nothing is drawn.
    #[test]
    fn twitch_queries_answer_without_stopping_the_walk() {
        let mut target = Target::new([800, 600]);
        let mut state = State::default();
        assert!(matches!(
            call(&mut target, &mut state, Kind::Preview, &[]),
            Reply::Size(None)
        ));
        for kind in [Kind::Streaming, Kind::StreamReady] {
            assert!(matches!(
                call(&mut target, &mut state, kind, &[]),
                Reply::Bool(false)
            ));
        }
    }

    /// The scene viewport variant without projectiles draws like the plain
    /// one, and records that the world pass leaves projectiles out.
    #[test]
    fn scene_viewport_without_projectiles_is_drawn_and_remembered() {
        let mut target = Target::new([800, 600]);
        let mut state = State::default();
        let plain = [0, 0, 765, 503];
        call(&mut target, &mut state, Kind::Scene, &[0, 0, 765, 503, 0]);
        assert!(!state.scene_without_projectiles);
        assert_eq!(target.scene.as_ref().map(|s| s.rect), Some(plain));
        target.scene = None;
        call(&mut target, &mut state, Kind::Scene, &[0, 0, 765, 503, 1]);
        assert!(state.scene_without_projectiles);
        assert_eq!(target.scene.as_ref().map(|s| s.rect), Some(plain));
    }
}

#[cfg(test)]
mod cutscene_letterbox_tests {
    use super::*;
    use crate::ui_paint::Op;

    /// The effective viewport of `requested` and the ops the scene pass
    /// paints for the bars it leaves.
    fn scene_viewport(
        state: &mut State,
        canvas: [u32; 2],
        requested: [i32; 4],
    ) -> ([i32; 4], Vec<Op>) {
        let mut target = Target::new(canvas);
        let rect = state.set_viewport(requested).unwrap();
        let start = target.leaf.painter.recording.ops.len();
        if rect != requested {
            target.paint_letterbox(requested, rect).unwrap();
        }
        (rect, target.leaf.painter.recording.ops[start..].to_vec())
    }

    fn fills(ops: &[Op]) -> Vec<[i32; 4]> {
        ops.iter()
            .filter_map(|op| match op {
                Op::Fill(rect, _) => Some(*rect),
                _ => None,
            })
            .collect()
    }

    /// A cutscene's picture ratio pins the scene view's field of view and
    /// height, so a viewport of another shape is cut to it and the two black
    /// bars are painted outside (the sides of a wide window, the top and
    /// bottom of a tall one) with the clip reset to the canvas; a viewport
    /// of the picture's own shape gets none, and the limits the cutscene
    /// replaced come back at its end. The stream of the wide frame is pinned.
    #[test]
    fn cutscene_ratio_cuts_the_scene_viewport_and_paints_the_bars() {
        let mut state = State::default();
        let wide = [0, 0, 1400, 600];
        let tall = [0, 0, 600, 1000];
        // Before a cutscene the viewport is the component.
        let (rect, ops) = scene_viewport(&mut state, [1400, 600], wide);
        assert_eq!((rect, ops.len()), (wide, 0));

        let saved = state.set_cutscene_ratio(16, 9);
        let (rect, ops) = scene_viewport(&mut state, [1400, 600], wide);
        assert_eq!(rect, [167, 0, 1066, 600]);
        assert_eq!(fills(&ops), [[0, 0, 167, 600], [1233, 0, 167, 600]]);
        assert!(matches!(ops[0], Op::ResetBounds([0, 0, 1400, 600])));
        assert_eq!(
            rs910_toolkit::toolkit::digest(&ops),
            0xdd3d_9456_f532_f455,
            "the wide cutscene frame's bar ops"
        );

        let (rect, ops) = scene_viewport(&mut state, [600, 1000], tall);
        assert_eq!(rect, [0, 331, 600, 338]);
        assert_eq!(fills(&ops), [[0, 0, 600, 331], [0, 669, 600, 331]]);

        // A 16:9 component keeps its shape: no bars.
        let (rect, ops) = scene_viewport(&mut state, [1600, 900], [0, 0, 1600, 900]);
        assert_eq!(rect, [0, 0, 1600, 900]);
        assert!(ops.is_empty());

        // The end of the cutscene restores what it replaced.
        let p = &mut state.viewport_profile;
        [p.min_height, p.max_height, p.min_fov, p.max_fov] = saved;
        let (rect, ops) = scene_viewport(&mut state, [1400, 600], wide);
        assert_eq!((rect, ops.len()), (wide, 0));
    }

    /// The viewport rectangle and zoom of a scene component under a
    /// cutscene's picture ratio match the original's over twelve window shapes
    /// at two origins (no ratio, 16:9, 4:3, 1:1, 21:9).
    #[test]
    fn cutscene_viewport_matches_the_recording() {
        let mut lines = String::new();
        for ratio in [(0, 0), (16, 9), (640, 480), (1, 1), (21, 9)] {
            for origin in [(0, 0), (17, 23)] {
                for (w, h) in [
                    (765, 503),
                    (1024, 768),
                    (1400, 600),
                    (1800, 500),
                    (600, 1000),
                    (1600, 900),
                    (1920, 1080),
                    (300, 1200),
                    (2560, 1440),
                    (800, 600),
                    (500, 500),
                    (3440, 1440),
                ] {
                    let mut state = State::default();
                    state.set_cutscene_ratio(ratio.0, ratio.1);
                    let rect = state.set_viewport([origin.0, origin.1, w, h]).unwrap();
                    let (_, zoom) = state.viewport.unwrap();
                    lines.push_str(&format!(
                        "{} {} {} {} {w} {h} -> {} {} {} {} {zoom}\n",
                        ratio.0, ratio.1, origin.0, origin.1, rect[0], rect[1], rect[2], rect[3]
                    ));
                }
            }
        }
        rs910_core::test_support::frozen::assert_stream(
            "cutscene-viewport/recording",
            lines.as_bytes(),
        );
    }
}
