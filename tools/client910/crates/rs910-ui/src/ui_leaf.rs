//! Synchronous text and sprite component consumers.
//! Engine-owned object/HTTP/skybox resources are required service boundaries.
use crate::{
    font_layout::{self, Draw, IconProvider, Paragraph, Sink, Style},
    ui_component_fields::Fields,
    ui_components::{field_snapshot, Ref},
    ui_draw::{Call, Frame, Kind, Reply},
    ui_fonts::{Font, Fonts},
    ui_paint,
    ui_properties::State,
    ui_sprites::Sprite,
    ui_text_compare::Language,
};
use anyhow::{Context as _, Result};
use rs910_core::fault::Fault;
use std::rc::Rc;
field_snapshot!(
    /// The type-4 paragraph scalars `Context::text` reads.
    TextFields {
        colour: i32,
        width: i32,
        height: i32,
        textshadow: bool,
        textHAlign: i32,
        textVAlign: i32,
        textLineHeight: i32,
        maxlines: i32,
        textantimacro: bool,
    }
);
pub struct ObjectText {
    pub name: Option<Vec<u16>>,
    pub stackable: i32,
}
/// Where a text or sprite component is painted: its screen position, its
/// transparency and the parent clip.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub pos: [i32; 2],
    pub trans: i32,
    pub parent: [i32; 4],
}
pub trait Services {
    fn object_text(&mut self, id: i32) -> Result<ObjectText>;
    fn object_sprite(&mut self, fields: &Fields) -> Result<Option<Rc<Sprite>>>;
    /// Client's cached HTTP lookup and pending-request scheduling branch.
    fn http_sprite(&mut self, id: i32) -> Result<Option<Rc<Sprite>>>;
    fn skybox(
        &mut self,
        painter: &mut ui_paint::Painter,
        fields: &Fields,
        pos: [i32; 2],
    ) -> Result<()>;
}
struct Provider<'a> {
    fonts: &'a Fonts,
    toolkit: u64,
}
impl IconProvider for Provider<'_> {
    fn width(&self, id: i32) -> Result<i32> {
        self.fonts.icon_width(id)
    }
    fn frames(&self, id: i32) -> Result<Option<Rc<Vec<font_layout::Image>>>> {
        self.fonts.icon_dimensions(Some(self.toolkit), id)
    }
}
struct TextSink<'a> {
    painter: &'a mut ui_paint::Painter,
    font: Rc<Font>,
    fonts: &'a Fonts,
    toolkit: u64,
    mask: Option<ui_paint::MaskRef>,
}
impl Sink for TextSink<'_> {
    fn emit(&mut self, draw: Draw) -> Result<()> {
        match draw {
            Draw::Glyph { .. } => self
                .painter
                .glyph_masked(&self.font, &draw, self.mask.clone()),
            Draw::Line {
                x,
                y,
                width,
                colour,
            } => self.painter.horizontal_line(x, y, width, colour),
            Draw::Image {
                icon,
                id,
                frame,
                x,
                y,
                colour,
                ..
            } => {
                let sprite = if icon {
                    // The immediately preceding layout lookup retained this
                    // array in the same cache. Keep the actual sprite in the
                    // painter before another lookup can evict the array.
                    self.fonts
                        .icon_sprites(Some(self.toolkit), id)?
                        .and_then(|v| v.sprites.get(frame as usize).cloned())
                } else {
                    self.fonts
                        .inline_sprites
                        .as_ref()
                        .and_then(|v| v.get(id as usize))
                        .cloned()
                }
                .context(Fault::MissingValue.message("font image"))?;
                if let Some(mask) = self.mask.clone() {
                    self.painter
                        .masked_native_tinted(&sprite, [x, y], colour, mask);
                } else {
                    self.painter.native(&sprite, [x, y], colour);
                }
            }
        }
        Ok(())
    }
}
pub struct Context {
    pub painter: ui_paint::Painter,
    pub style: Style,
    pub random: font_layout::Random,
    pub random_seed: i32,
    pub hover_bounds: [i32; 4],
    pub clip_text: bool,
    pub language: Option<Language>,
    pub toolkit: u64,
}
impl Context {
    /// drawEntry/drawTitleBar use the same font and sprite painter.
    pub fn menu_text(
        &mut self,
        fonts: &Fonts,
        font: Rc<Font>,
        text: &str,
        pos: [i32; 2],
        colour: i32,
        shadow: i32,
    ) -> Result<()> {
        let mut style = Style::new(colour, shadow);
        let mut sink = TextSink {
            painter: &mut self.painter,
            font: font.clone(),
            fonts,
            toolkit: self.toolkit,
            mask: None,
        };
        font_layout::line_to(
            &font.metrics,
            font_layout::LineAt {
                text: &text.encode_utf16().collect::<Vec<_>>(),
                x: pos[0],
                baseline: pos[1],
            },
            &mut style,
            font_layout::TextResources {
                images: &fonts.images,
                provider: Some(&Provider {
                    fonts,
                    toolkit: self.toolkit,
                }),
            },
            false,
            &mut sink,
        )
    }
    pub fn menu_text_masked(
        &mut self,
        fonts: &Fonts,
        font: Rc<Font>,
        text: &str,
        pos: [i32; 2],
        mut style: Style,
        mask: ui_paint::MaskRef,
    ) -> Result<()> {
        let mut sink = TextSink {
            painter: &mut self.painter,
            font: font.clone(),
            fonts,
            toolkit: self.toolkit,
            mask: Some(mask),
        };
        font_layout::line_to(
            &font.metrics,
            font_layout::LineAt {
                text: &text.encode_utf16().collect::<Vec<_>>(),
                x: pos[0],
                baseline: pos[1],
            },
            &mut style,
            font_layout::TextResources {
                images: &fonts.images,
                provider: Some(&Provider {
                    fonts,
                    toolkit: self.toolkit,
                }),
            },
            false,
            &mut sink,
        )
    }
    /// `Font.drawStringTaggable(text, x, y, w, h, colour, shadow, halign,
    /// valign, lineHeight, ...)` outside a component, as
    /// the world map draws its element labels.
    pub fn paragraph_text(
        &mut self,
        fonts: &Fonts,
        font: Rc<Font>,
        text: &str,
        paragraph: font_layout::Paragraph,
    ) -> Result<i32> {
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut style = Style::new(paragraph.colour, paragraph.shadow);
        let mut sink = TextSink {
            painter: &mut self.painter,
            font: font.clone(),
            fonts,
            toolkit: self.toolkit,
            mask: None,
        };
        font_layout::paragraph_to(
            &font.metrics,
            Some(&units),
            paragraph,
            &fonts.images,
            Some(&Provider {
                fonts,
                toolkit: self.toolkit,
            }),
            &mut style,
            &mut sink,
        )
    }
    /// `Font.drawCharsAlpha` with explicit per-glyph
    /// offsets, as used by `drawCenteredWave`, `drawCenteredWave2` and
    /// `drawCenteredShake`. `pos` is the baseline start.
    pub fn alpha_text(
        &mut self,
        fonts: &Fonts,
        font: Rc<Font>,
        text: &str,
        pos: [i32; 2],
        mut style: Style,
        offsets: [Option<&[i32]>; 2],
    ) -> Result<()> {
        let [offset_x, offset_y] = offsets;
        let mut sink = TextSink {
            painter: &mut self.painter,
            font: font.clone(),
            fonts,
            toolkit: self.toolkit,
            mask: None,
        };
        font_layout::alpha_line_to(
            &font.metrics,
            font_layout::LineAt {
                text: &text.encode_utf16().collect::<Vec<_>>(),
                x: pos[0],
                baseline: pos[1],
            },
            &mut style,
            font_layout::TextResources {
                images: &fonts.images,
                provider: Some(&Provider {
                    fonts,
                    toolkit: self.toolkit,
                }),
            },
            font_layout::Offsets {
                x: offset_x,
                y: offset_y,
            },
            &mut sink,
        )
    }
    /// Entry text shares the original client's per-character alpha jitter.
    pub fn menu_entry(
        &mut self,
        fonts: &Fonts,
        font: Rc<Font>,
        text: &str,
        pos: [i32; 2],
        colour: i32,
    ) -> Result<()> {
        let text: Vec<u16> = text.encode_utf16().collect();
        self.random.set_seed(i64::from(self.random_seed));
        let alpha = (self.random.next_int() & 31) + 192;
        let mut style = Style::new(alpha << 24 | (colour & 0xffffff), alpha << 24);
        let mut shift = 0;
        let offsets: Vec<_> = text
            .iter()
            .map(|_| {
                let old = shift;
                if self.random.next_int() & 3 == 0 {
                    shift += 1;
                }
                old
            })
            .collect();
        let mut sink = TextSink {
            painter: &mut self.painter,
            font: font.clone(),
            fonts,
            toolkit: self.toolkit,
            mask: None,
        };
        font_layout::alpha_line_to(
            &font.metrics,
            font_layout::LineAt {
                text: &text,
                x: pos[0],
                baseline: pos[1],
            },
            &mut style,
            font_layout::TextResources {
                images: &fonts.images,
                provider: Some(&Provider {
                    fonts,
                    toolkit: self.toolkit,
                }),
            },
            font_layout::Offsets {
                x: Some(&offsets),
                y: None,
            },
            &mut sink,
        )
    }
    pub fn new(size: [u32; 2], language: Option<Language>, toolkit: u64) -> Self {
        Self {
            painter: ui_paint::Painter::new(size),
            style: Style::default(),
            random: Default::default(),
            random_seed: 0,
            hover_bounds: [0; 4],
            clip_text: false,
            language,
            toolkit,
        }
    }
    /// Returns None only for a call owned by another engine/scene consumer.
    /// The caller must service it, flushing this paint plan at the original client's boundary.
    pub fn call(
        &mut self,
        frame: &mut Frame,
        state: &mut State,
        services: &mut impl Services,
        call: &Call,
    ) -> Result<Option<Reply>> {
        let a = &call.args;
        match call.kind {
            Kind::Bounds => self.painter.reset_bounds(a[..4].try_into()?),
            Kind::Fill => self.painter.fill(a[..4].try_into()?, a[4])?,
            Kind::Outline => self.painter.outline(a[..4].try_into()?, a[4]),
            Kind::Line => self.painter.line([a[0], a[1]], [a[2], a[3]], a[4], a[5]),
            Kind::Text | Kind::Sprite => {
                let c = call.component.as_ref().context("missing leaf component")?;
                let placement = Placement {
                    pos: [a[0], a[1]],
                    trans: a[2],
                    parent: a[3..7].try_into()?,
                };
                if call.kind == Kind::Text {
                    self.text(frame, state, services, c, placement)?;
                } else {
                    self.sprite(frame, state, services, c, placement)?;
                }
            }
            _ => return Ok(None),
        }
        Ok(Some(Reply::Unit))
    }
    pub fn text(
        &mut self,
        frame: &mut Frame,
        state: &mut State,
        services: &mut impl Services,
        c: &Ref,
        placement: Placement,
    ) -> Result<()> {
        let Placement { pos, trans, parent } = placement;
        let alpha = 255 - (trans & 255);
        if alpha == 0 {
            return Ok(());
        }
        let Some(font) = state.component_font(c)? else {
            if state.resource_missing {
                frame.component_updated(state, c)?;
            }
            return Ok(());
        };
        let pressed = state
            .life
            .pressed_continue
            .as_ref()
            .is_some_and(|v| Rc::ptr_eq(v, c));
        // One borrow for the text selection and the paragraph scalars: the
        // object-name lookup does not reach the component.
        let (f, text) = {
            let component = c.borrow();
            let f = TextFields::of(&component.f);
            (
                f,
                text_content(&component.f, pressed, self.language, services)?,
            )
        };
        let colour = f.colour;
        if self.clip_text {
            self.painter.set_bounds([
                pos[0],
                pos[1],
                pos[0].wrapping_add(f.width),
                pos[1].wrapping_add(f.height),
            ]);
        }
        let p = Paragraph {
            x: pos[0],
            y: pos[1],
            width: f.width,
            height: f.height,
            colour: alpha << 24 | colour,
            shadow: if f.textshadow { alpha << 24 } else { -1 },
            halign: f.textHAlign,
            valign: f.textVAlign,
            line_height: f.textLineHeight,
            max_lines: f.maxlines,
            masked: false,
        };
        let fonts = state
            .fonts
            .as_ref()
            .context("font provider not installed")?;
        let provider = Provider {
            fonts,
            toolkit: self.toolkit,
        };
        let mut sink = TextSink {
            painter: &mut self.painter,
            font: font.clone(),
            fonts,
            toolkit: self.toolkit,
            mask: None,
        };
        if f.textantimacro {
            font_layout::antimacro_to(
                &font.metrics,
                text.as_deref(),
                p,
                font_layout::TextResources {
                    images: &fonts.images,
                    provider: Some(&provider),
                },
                &mut self.style,
                font_layout::Antimacro {
                    random: &mut self.random,
                    seed: self.random_seed,
                    bounds: Some(&mut self.hover_bounds),
                },
                &mut sink,
            )?;
        } else {
            font_layout::paragraph_to(
                &font.metrics,
                text.as_deref(),
                p,
                &fonts.images,
                Some(&provider),
                &mut self.style,
                &mut sink,
            )?;
        }
        if self.clip_text {
            self.painter.reset_bounds(parent);
        }
        Ok(())
    }
    pub fn sprite(
        &mut self,
        frame: &mut Frame,
        state: &mut State,
        services: &mut impl Services,
        c: &Ref,
        placement: Placement,
    ) -> Result<()> {
        let Placement { pos, trans, parent } = placement;
        // The sprite lookups and the painter only read the component, so one
        // shared borrow spans them; it ends before `component_updated`.
        let component = c.borrow();
        let f = &component.f;
        if f.skyboxId >= 0 {
            return services.skybox(&mut self.painter, f, pos);
        }
        let sprite = if f.invobject != -1 {
            services.object_sprite(f)?
        } else if f.httpImageId == -1 {
            state.component_sprite(c, &mut crate::ui_sprites::Cpu)?
        } else {
            services.http_sprite(f.httpImageId)?
        };
        if let Some(sprite) = sprite {
            self.painter
                .component_sprite(&sprite, f, pos, trans, parent)?;
        } else if state.resource_missing {
            drop(component);
            frame.component_updated(state, c)?;
        }
        Ok(())
    }
}
/// The text selection prefix of Client's type-4 leaf, after font lookup.
/// Object lookup still occurs when the continue prompt will replace the text.
pub fn text_content(
    f: &Fields,
    pressed: bool,
    language: Option<Language>,
    services: &mut impl Services,
) -> Result<Option<Vec<u16>>> {
    let mut text = f.text.clone();
    if f.invobject != -1 {
        let obj = services.object_text(f.invobject)?;
        let name = obj.name.unwrap_or_else(|| "null".encode_utf16().collect());
        text = Some(name.clone());
        if (obj.stackable == 1 || f.invcount != 1) && f.invcount != -1 {
            let mut v = "<col=ff9040>".encode_utf16().collect::<Vec<_>>();
            v.extend(name);
            v.extend("</col> x".encode_utf16());
            v.extend(format_count(f.invcount, language).encode_utf16());
            text = Some(v);
        }
    }
    if pressed {
        text = please_wait(language).map(|s| s.encode_utf16().collect());
    }
    Ok(text)
}
/// The pressed-button text; none for a language without texts.
fn please_wait(language: Option<Language>) -> Option<&'static str> {
    rs910_core::texts::Msg::PleaseWait.for_lang(language?)
}
/// Formats an object count with grouping. Grouping includes the sign; the
/// original client's string concatenation renders an unavailable language
/// suffix as the literal null.
pub fn format_count(count: i32, language: Option<Language>) -> String {
    let mut s = count.to_string();
    let mut at = s.len() as i32 - 3;
    while at > 0 {
        s.insert(at as usize, ',');
        at -= 3;
    }
    use rs910_core::texts::Msg;
    // An unavailable language renders its suffix as the literal null.
    let suffix = |msg: Msg| language.map_or("null", |l| msg.display(l));
    if s.len() > 9 {
        format!(
            " <col=ff80>{}{} ({s})</col>",
            &s[..s.len() - 8],
            suffix(Msg::Million)
        )
    } else if s.len() > 6 {
        format!(
            " <col=ffffff>{}{} ({s})</col>",
            &s[..s.len() - 4],
            suffix(Msg::Thousand)
        )
    } else {
        format!(" <col=ffff00>{s}</col>")
    }
}
