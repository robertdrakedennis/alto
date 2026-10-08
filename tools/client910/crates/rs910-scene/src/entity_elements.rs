//! The per-entity 2D pass after the world draw: overhead chat collection, headbars, head
//! icons, NPC/player hint arrows and hitmarks, plus the chat stacking and effect pass.
//! Tile hint arrows are drawn by a separate function.
//!
//! The pass is a pure function of the retained entity state: projection,
//! sprites and fonts come from the caller, and the result is an ordered list of
//! paint commands (in draw order, including the clip-bounds changes).
//! Draw-time mutations are kept: expired hitmarks are cleared (their expiry is set to -1)
//! and headbar updates are consumed as bars are drawn, an emptied bar being removed.
use crate::entities910::Player;
use crate::protocol910::combat::visible_update;
use crate::protocol910::combat_types::{Bar, Hit};
use crate::sprite::Sprite;
use std::collections::BTreeMap;
use std::rc::Rc;

/// The default p11 or b12 font, or a hitmark config's `damagefont` (loaded with its
/// `damagecolour_set` flag).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FontRef {
    P11,
    B12,
    Config { id: i32, mono: bool },
}

/// Sprite and font lookups the pass reads (string width, ascent and descent).
pub trait Resources {
    /// Frame 0 of a config sprite group, as hitmark and headbar configs cache it.
    fn config_sprite(&mut self, group: i32) -> Option<Rc<Sprite>>;
    /// Head icon sprite `frame` of sprite group `group`.
    fn head_icon(&mut self, group: i32, frame: i32) -> Option<Rc<Sprite>>;
    /// Hint arrow sprite `index`.
    fn hint_arrow(&mut self, index: i32) -> Option<Rc<Sprite>>;
    /// Whether a configured font loads (the pass falls back to p11 when it does not).
    fn has_font(&mut self, font: FontRef) -> bool;
    fn string_width(&mut self, font: FontRef, text: &str) -> i32;
    fn ascent(&mut self, font: FontRef) -> i32;
    fn descent(&mut self, font: FontRef) -> i32;
}

#[derive(Clone, Debug)]
pub enum Draw {
    /// A sprite at `pos`: drawn plainly when `colour == -1`, otherwise tinted with
    /// `colour = alpha << 24 | 0xFFFFFF`.
    Sprite {
        sprite: Rc<Sprite>,
        pos: [i32; 2],
        colour: i32,
    },
    /// Intersect the current clip with `[x0, y0, x1, y1]`.
    SetBounds([i32; 4]),
    /// Reset the clip to the rectangle `[x0, y0, x1, y1]` (the scene viewport).
    ResetBounds([i32; 4]),
    /// `text` drawn at `pos` with `colour` and `shadow`; with `offsets`, each glyph is
    /// displaced by the per-glyph x/y offsets (the wave/shake effects).
    Text {
        font: FontRef,
        text: String,
        pos: [i32; 2],
        colour: i32,
        shadow: i32,
        offsets: Option<GlyphOffsets>,
    },
}

/// The pass output, split where the cover markers are drawn between the entity loop and
/// chat.
#[derive(Debug, Default)]
pub struct Elements {
    pub entities: Vec<Draw>,
    pub chats: Vec<Draw>,
}

/// Constant inputs for one pass over the scene viewport.
pub struct Frame<'a> {
    /// The current logic tick (loop cycle).
    pub loop_cycle: i32,
    /// The scene cycle (chat colour/effect animation).
    pub scene_cycle: i32,
    /// The scene viewport in canvas pixels: `[x, y, width, height]`.
    pub viewport: [i32; 4],
    /// `graphicsDefaults.hitmarkpos_x/y`.
    pub hitmark_positions: &'a [[i32; 2]],
    /// The bar gap used when no headbar drew.
    pub headbar_gap: i32,
    pub player_chat_visible: bool,
    pub npc_chat_visible: bool,
    /// The chat-effects option (0 = coloured/effect chat).
    pub chat_effects: i32,
    /// The public chat filter option, which gates whether a player's chat line shows.
    pub public_chat_filter: i32,
    /// Whether the player with this unfiltered name is a friend.
    pub friend_test: &'a dyn Fn(&str) -> bool,
    /// Emoji substitution, applied while the emoji list is in auto-chat mode; `None` leaves
    /// the text as sent.
    pub emoji: Option<&'a dyn Fn(&str) -> String>,
    /// The logic rate, used for NPC hint-arrow blinking.
    pub logic_rate: i32,
    pub hitmarks: &'a BTreeMap<i32, Hit>,
    pub headbars: &'a BTreeMap<i32, Bar>,
    /// Var reads for multi-hitmark visibility: `(is_varbit, id) -> value`.
    pub read_var: &'a dyn Fn(bool, i32) -> Option<i32>,
}

/// One live hint-arrow marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HintArrow {
    pub hint_type: i32,
    /// NPC slot or player index.
    pub target: i32,
    /// Sprite index.
    pub sprite: i32,
    /// Blink rate (0 = steady).
    pub blink: i32,
}

pub enum Kind<'a> {
    Player {
        index: usize,
        /// Head icon ids and their sprite groups.
        head_ids: [i32; 8],
        head_groups: [i32; 8],
        /// Partner-type serial of the player (0 = none); selects the headbar sprite variant.
        partner: i32,
    },
    Npc {
        index: usize,
        /// The NPC's head icon customisation, or its config's `headicon_groupid/headicon_id`.
        head_icons: Option<&'a ([i32; 8], [i16; 8])>,
        /// The headbar sprite choice reads the high-resolution player slot at the entity's
        /// row for every row, NPC rows included: the partner-type serial of the player in
        /// the stale slot at this NPC's row (`Players::high_resolution_player`), or 0 when
        /// that slot holds no player.
        bar_partner: i32,
    },
}

/// One entity in iteration order (high-resolution players, then NPC slots).
pub struct Entity<'a> {
    pub path: &'a mut Player,
    pub kind: Kind<'a>,
    /// Overhead height: the animation-set height when set, else `-height` from the last
    /// sequenced model (200 before one was built).
    pub height: i32,
    /// `drawPriority < 0 || sceneCycle != drawCycle && local level differs`.
    pub skip: bool,
    /// `sceneAddDeferred`.
    pub deferred: bool,
}

/// Overhead height of an entity: the animation-set height when set (not -1), else
/// `-height` of the last sequenced model (200 before one was built), raised by the height
/// of the ground decoration on the entity's tile when present (`ground_decoration` is that
/// tile lookup).
pub fn entity_height(bas_height: i32, model_height: i32, ground_decoration: Option<i32>) -> i32 {
    let height = if bas_height != -1 {
        bas_height
    } else if model_height == -32768 {
        200
    } else {
        model_height.wrapping_neg()
    };
    ground_decoration.map_or(height, |decor| decor.wrapping_add(height))
}

/// Overhead chat colours 0-5.
const CHAT_COLOURS: [i32; 6] = [16776960, 16711680, 65280, 65535, 16711935, 16777215];
/// The overhead chat capacity.
const MAX_CHATS: usize = 54;

struct ChatLine {
    x: i32,
    y: i32,
    width: i32,
    text: String,
    colour: i32,
    effect: i32,
    time: i32,
    total: i32,
}

/// Resolve a hitmark type to the one currently visible (following its multi-mark var).
fn visible_hitmark<'a>(frame: &'a Frame, id: i32) -> Option<&'a Hit> {
    let t = frame.hitmarks.get(&id)?;
    let Some(multi) = t.multimark.as_ref() else {
        return Some(t);
    };
    let mut value = -1;
    if t.multivarbit != -1 {
        value = (frame.read_var)(true, t.multivarbit).unwrap_or(-1);
    } else if t.multivarp != -1 {
        value = (frame.read_var)(false, t.multivarp).unwrap_or(-1);
    }
    let pick = if value >= 0 && (value as usize) < multi.len() - 1 {
        multi[value as usize]
    } else {
        *multi.last()?
    };
    if pick == -1 {
        None
    } else {
        frame.hitmarks.get(&pick)
    }
}

/// The damage text: `%1` replaced by the scaled damage.
fn format_damage(t: &Hit, damage: i32) -> String {
    let value = if t.damagescalefrom == 0 {
        0
    } else {
        t.damagescaleto.wrapping_mul(damage) / t.damagescalefrom
    };
    t.damageformat.replace("%1", &value.to_string())
}

struct HitGraphics {
    class: Option<Rc<Sprite>>,
    middle: Option<Rc<Sprite>>,
    left: Option<Rc<Sprite>>,
    right: Option<Rc<Sprite>>,
}

fn hit_graphics(r: &mut dyn Resources, t: &Hit) -> HitGraphics {
    let mut load = |g: i32| if g < 0 { None } else { r.config_sprite(g) };
    HitGraphics {
        class: load(t.classgraphic),
        middle: load(t.middlegraphic),
        left: load(t.leftgraphic),
        right: load(t.rightgraphic),
    }
}

fn width_of(s: &Option<Rc<Sprite>>) -> i32 {
    s.as_ref().map_or(0, |s| s.size[0])
}
fn inset_of(s: &Option<Rc<Sprite>>) -> i32 {
    s.as_ref().map_or(0, |s| s.padding[0])
}

/// Run the pass. `project(level, fine_x, fine_z, height)` gives viewport-relative screen
/// coordinates, `[-1, -1]` outside the map border, or NaN outside the clip volume.
pub fn draw(
    frame: &Frame,
    entities: &mut [Entity],
    npc_arrows: &[HintArrow],
    project: &dyn Fn(i32, f32, f32, i32) -> [f32; 2],
    r: &mut dyn Resources,
) -> Elements {
    let [view_x, view_y, view_width, view_height] = frame.viewport;
    let reset = [view_x, view_y, view_x + view_width, view_y + view_height];
    let mut out = Vec::new();
    let mut chats: Vec<ChatLine> = Vec::new();
    let b12_ascent = r.ascent(FontRef::B12);
    for entity in entities.iter_mut() {
        if entity.skip {
            continue;
        }
        let is_npc = matches!(entity.kind, Kind::Npc { .. });
        let e = &mut *entity.path;
        let projection = project(e.level, e.fine_x, e.fine_z, entity.height);
        // Only a negative x is skipped: NaN passes.
        if projection[0] < 0.0 {
            continue;
        }
        let show_chat = if is_npc {
            frame.npc_chat_visible
        } else {
            frame.player_chat_visible
        };
        // The chat line: a textless line is absent; a player's line also needs the public
        // chat filter at 0 or 3, or at 1 with the friend test passing on the unfiltered name.
        let chat_line = e.chat.as_ref().filter(|c| c.text.is_some()).filter(|_| {
            is_npc
                || matches!(frame.public_chat_filter, 0 | 3)
                || frame.public_chat_filter == 1
                    && (frame.friend_test)(e.appearance.name.as_deref().unwrap_or(""))
        });
        if show_chat {
            if let Some(chat) = chat_line {
                if chats.len() < MAX_CHATS {
                    let text = chat.text.clone().unwrap_or_default();
                    let text = match frame.emoji {
                        Some(substitute) => substitute(&text),
                        None => text,
                    };
                    chats.push(ChatLine {
                        width: r.string_width(FontRef::B12, &text) / 2,
                        x: projection[0] as i32,
                        y: projection[1] as i32,
                        text,
                        colour: chat.colour,
                        effect: chat.effect,
                        time: chat.time,
                        total: chat.total,
                    });
                }
            }
        }
        let base_y = (projection[1] + view_y as f32) as i32;
        let mut y = base_y - b12_ascent;
        let mut drew_bar = false;
        if !entity.deferred {
            if let Some(combat) = e.combat.as_mut() {
                let mut i = 0;
                while i < combat.bars.len() {
                    let id = combat.bars[i].id;
                    let Some(t) = frame.headbars.get(&id) else {
                        i += 1;
                        continue;
                    };
                    let update = visible_update(&mut combat.bars[i], frame.loop_cycle, &t.combat());
                    let Some([start_cycle, start_fill, end_fill, duration]) = update else {
                        // An emptied bar is removed.
                        if combat.bars[i].updates.is_empty() {
                            combat.bars.remove(i);
                        } else {
                            i += 1;
                        }
                        continue;
                    };
                    i += 1;
                    // The variant comes from the high-resolution slot at this row, NPC rows included.
                    let partner = match entity.kind {
                        Kind::Player { partner, .. } => partner,
                        Kind::Npc { bar_partner, .. } => bar_partner,
                    };
                    let (empty_id, full_id) = match partner {
                        0 => (t.empty, t.full),
                        1 => (t.emptylocalpartner, t.fulllocalpartner),
                        _ => (t.emptyglobalpartner, t.fullglobalpartner),
                    };
                    let empty = if empty_id < 0 {
                        None
                    } else {
                        r.config_sprite(empty_id)
                    };
                    let full = if full_id < 0 {
                        None
                    } else {
                        r.config_sprite(full_id)
                    };
                    let (Some(empty), Some(full)) = (empty, full) else {
                        continue;
                    };
                    let mut alpha = 255;
                    let elapsed = frame.loop_cycle - start_cycle;
                    let end_width = full.size[0] * end_fill / 255;
                    let width;
                    if duration > elapsed {
                        let stepped = if t.fill_step == 0 {
                            0
                        } else {
                            elapsed / t.fill_step * t.fill_step
                        };
                        let start_width = full.size[0] * start_fill / 255;
                        width = (end_width - start_width) * stepped / duration + start_width;
                    } else {
                        width = end_width;
                        let remaining = duration + t.sticktime - elapsed;
                        if t.fadeout >= 0 {
                            alpha = (remaining << 8) / (t.sticktime - t.fadeout);
                        }
                    }
                    let width = if end_fill > 0 && width < 2 { 2 } else { width };
                    let height = empty.size[1];
                    let x = (projection[0] + view_x as f32 - (empty.size[0] >> 1) as f32) as i32;
                    y -= height;
                    let colour = if (0..255).contains(&alpha) {
                        (alpha << 24) | 0xFFFFFF
                    } else {
                        -1
                    };
                    out.push(Draw::Sprite {
                        sprite: empty.clone(),
                        pos: [x, y],
                        colour,
                    });
                    out.push(Draw::SetBounds([x, y, width + x, y + height]));
                    out.push(Draw::Sprite {
                        sprite: full,
                        pos: [x, y],
                        colour,
                    });
                    out.push(Draw::ResetBounds(reset));
                    y -= 2;
                    drew_bar = true;
                }
            }
        }
        if !drew_bar {
            y -= frame.headbar_gap + 2;
        }
        if !entity.deferred {
            match &entity.kind {
                Kind::Player {
                    head_ids,
                    head_groups,
                    ..
                } => {
                    for slot in 0..head_ids.len() {
                        if head_ids[slot] < 0 {
                            continue;
                        }
                        let Some(sprite) = r.head_icon(head_groups[slot], head_ids[slot]) else {
                            continue;
                        };
                        y -= sprite.size[1];
                        out.push(Draw::Sprite {
                            sprite: sprite.clone(),
                            pos: [(projection[0] + view_x as f32 - 12.0) as i32, y],
                            colour: -1,
                        });
                        y -= 2;
                    }
                }
                Kind::Npc { head_icons, .. } => {
                    if let Some((groups, ids)) = head_icons {
                        for slot in 0..ids.len() {
                            if ids[slot] < 0 || groups[slot] < 0 {
                                continue;
                            }
                            let Some(sprite) = r.head_icon(groups[slot], i32::from(ids[slot]))
                            else {
                                continue;
                            };
                            y -= sprite.size[1];
                            out.push(Draw::Sprite {
                                sprite: sprite.clone(),
                                pos: [
                                    (projection[0] + view_x as f32 - (sprite.size[0] >> 1) as f32)
                                        as i32,
                                    y,
                                ],
                                colour: -1,
                            });
                            y -= 2;
                        }
                    }
                }
            }
        }
        // Hint arrows over this entity; their stacking offset is not carried forward.
        let (arrow_type, target) = match entity.kind {
            Kind::Npc { index, .. } => (1, index as i32),
            Kind::Player { index, .. } => (10, index as i32),
        };
        for arrow in npc_arrows
            .iter()
            .filter(|a| a.hint_type == arrow_type && a.target == target)
        {
            let Some(sprite) = r.hint_arrow(arrow.sprite) else {
                continue;
            };
            let visible = if arrow_type == 10 || arrow.blink == 0 {
                true
            } else {
                let half = frame.logic_rate * 1000 / arrow.blink / 2;
                half != 0 && frame.loop_cycle % (half * 2) < half
            };
            if visible {
                out.push(Draw::Sprite {
                    sprite: sprite.clone(),
                    pos: [
                        (projection[0] + view_x as f32 - 12.0) as i32,
                        y - sprite.size[1],
                    ],
                    colour: -1,
                });
            }
        }
        draw_hitmarks(frame, entity.height, e, project, r, &mut out);
    }
    let mut chat_draws = Vec::new();
    draw_chats(frame, chats, r, &mut chat_draws);
    Elements {
        entities: out,
        chats: chat_draws,
    }
}

fn draw_hitmarks(
    frame: &Frame,
    entity_height: i32,
    e: &mut Player,
    project: &dyn Fn(i32, f32, f32, i32) -> [f32; 2],
    r: &mut dyn Resources,
    out: &mut Vec<Draw>,
) {
    let [view_x, view_y, ..] = frame.viewport;
    let Some(combat) = e.combat.as_mut() else {
        return;
    };
    let (level, fine_x, fine_z) = (e.level, e.fine_x, e.fine_z);
    for slot in 0..combat.hits.len() {
        let [kind, damage, secondary_kind, secondary_damage, expiry] = combat.hits[slot];
        let mut stick = 0;
        let primary = if kind >= 0 {
            if expiry <= frame.loop_cycle {
                continue;
            }
            stick = frame.hitmarks.get(&kind).map_or(0, |t| t.sticktime);
            match visible_hitmark(frame, kind) {
                Some(t) => Some(t),
                None => {
                    combat.hits[slot][4] = -1;
                    continue;
                }
            }
        } else if expiry < 0 {
            continue;
        } else {
            None
        };
        let secondary = if secondary_kind >= 0 {
            visible_hitmark(frame, secondary_kind)
        } else {
            None
        };
        if expiry - stick > frame.loop_cycle {
            continue;
        }
        let Some(t) = primary else {
            combat.hits[slot][4] = -1;
            continue;
        };
        let mut projection = project(level, fine_x, fine_z, entity_height / 2);
        // Drawn only when `x > -1.0`; NaN is skipped.
        let on_screen = projection[0] > -1.0;
        if !on_screen {
            continue;
        }
        if let Some(offset) = frame.hitmark_positions.get(slot) {
            projection[0] += offset[0] as f32;
            projection[1] += offset[1] as f32;
        }
        let g = hit_graphics(r, t);
        let (w_class, w_mid, w_left, w_right) = (
            width_of(&g.class),
            width_of(&g.middle),
            width_of(&g.left),
            width_of(&g.right),
        );
        let (i_class, i_mid, i_left, i_right) = (
            inset_of(&g.class),
            inset_of(&g.middle),
            inset_of(&g.left),
            inset_of(&g.right),
        );
        let g2 = secondary.map(|s| hit_graphics(r, s));
        let (w2_class, w2_mid, w2_left, w2_right, i2_class, i2_mid, i2_left, i2_right) =
            g2.as_ref().map_or((0, 0, 0, 0, 0, 0, 0, 0), |g| {
                (
                    width_of(&g.class),
                    width_of(&g.middle),
                    width_of(&g.left),
                    width_of(&g.right),
                    inset_of(&g.class),
                    inset_of(&g.middle),
                    inset_of(&g.left),
                    inset_of(&g.right),
                )
            });
        let font_for = |r: &mut dyn Resources, h: &Hit| {
            if h.damagefont >= 0 {
                let f = FontRef::Config {
                    id: h.damagefont,
                    mono: h.damagecolour_set,
                };
                if r.has_font(f) {
                    return f;
                }
            }
            FontRef::P11
        };
        let font = font_for(r, t);
        let font2 = secondary.map_or(FontRef::P11, |s| font_for(r, s));
        let text = format_damage(t, damage);
        let text_width = r.string_width(font, &text);
        let (text2, text2_width) = match secondary {
            Some(s) => {
                let text = format_damage(s, secondary_damage);
                let width = r.string_width(font2, &text);
                (Some(text), width)
            }
            None => (None, 0),
        };
        let mid_count = if w_mid > 0 { text_width / w_mid + 1 } else { 0 };
        let mid2_count = if secondary.is_some() && w2_mid > 0 {
            text2_width / w2_mid + 1
        } else {
            0
        };
        let mut cursor = 0;
        let class_x = cursor;
        if w_class > 0 {
            cursor += w_class;
        }
        cursor += 2;
        let left_x = cursor;
        if w_left > 0 {
            cursor += w_left;
        }
        let mid_x = cursor;
        let mut text_x = cursor;
        let mut end;
        if w_mid > 0 {
            let span = w_mid * mid_count;
            end = cursor + span;
            text_x = (span - text_width) / 2 + cursor;
        } else {
            end = text_width + cursor;
        }
        let right_x = end;
        if w_right > 0 {
            end += w_right;
        }
        let (mut class2_x, mut left2_x, mut mid2_x, mut right2_x, mut text2_x) = (0, 0, 0, 0, 0);
        if secondary.is_some() {
            end += 2;
            class2_x = end;
            if w2_class > 0 {
                end += w2_class;
            }
            end += 2;
            left2_x = end;
            if w2_left > 0 {
                end += w2_left;
            }
            mid2_x = end;
            text2_x = end;
            if w2_mid > 0 {
                let span = w2_mid * mid2_count;
                end += span;
                text2_x += (span - text2_width) / 2;
            } else {
                end += text2_width;
            }
            right2_x = end;
            if w2_right > 0 {
                end += w2_right;
            }
        }
        let remaining = combat.hits[slot][4] - frame.loop_cycle;
        let sticktime = if t.sticktime == 0 { 1 } else { t.sticktime };
        let scroll_x = t.scrolltooffsetx - t.scrolltooffsetx * remaining / sticktime;
        let scroll_y = t.scrolltooffsety * remaining / sticktime + -t.scrolltooffsety;
        let x = (projection[0] + view_x as f32 - (end >> 1) as f32 + scroll_x as f32) as i32;
        let y = (projection[1] + view_y as f32 - 12.0 + scroll_y as f32) as i32;
        let text_y = t.damageyof + y + 15;
        let text2_y = secondary.map_or(0, |s| s.damageyof + y + 15);
        let mut alpha = 255;
        if t.fadeat >= 0 {
            let span = t.sticktime - t.fadeat;
            alpha = if span == 0 {
                255
            } else {
                (remaining << 8) / span
            };
        }
        let translucent = (0..255).contains(&alpha);
        let colour = if translucent {
            (alpha << 24) | 0xFFFFFF
        } else {
            -1
        };
        let text_alpha = if translucent {
            alpha << 24
        } else {
            0xFF000000u32 as i32
        };
        let sprite = |s: &Option<Rc<Sprite>>, pos: [i32; 2], out: &mut Vec<Draw>| {
            if let Some(s) = s {
                out.push(Draw::Sprite {
                    sprite: s.clone(),
                    pos,
                    colour,
                });
            }
        };
        sprite(&g.class, [class_x + x - i_class, y], out);
        sprite(
            &g.left,
            [t.graphicxof + (left_x + x - i_left), t.graphicyof + y],
            out,
        );
        if g.middle.is_some() {
            for n in 0..mid_count {
                sprite(
                    &g.middle,
                    [
                        t.graphicxof + w_mid * n + (mid_x + x - i_mid),
                        t.graphicyof + y,
                    ],
                    out,
                );
            }
        }
        sprite(
            &g.right,
            [t.graphicxof + (right_x + x - i_right), t.graphicyof + y],
            out,
        );
        out.push(Draw::Text {
            font,
            text,
            pos: [text_x + x, text_y],
            colour: t.damagecolour | text_alpha,
            shadow: 0,
            offsets: None,
        });
        if let (Some(s), Some(g2), Some(text2)) = (secondary, g2.as_ref(), text2) {
            sprite(&g2.class, [class2_x + x - i2_class, y], out);
            sprite(
                &g2.left,
                [s.graphicxof + (left2_x + x - i2_left), s.graphicyof + y],
                out,
            );
            if g2.middle.is_some() {
                for n in 0..mid2_count {
                    sprite(
                        &g2.middle,
                        [
                            s.graphicxof + w2_mid * n + (mid2_x + x - i2_mid),
                            s.graphicyof + y,
                        ],
                        out,
                    );
                }
            }
            sprite(
                &g2.right,
                [s.graphicxof + (right2_x + x - i2_right), s.graphicyof + y],
                out,
            );
            out.push(Draw::Text {
                font: font2,
                text: text2,
                pos: [text2_x + x, text2_y],
                colour: s.damagecolour | text_alpha,
                shadow: 0,
                offsets: None,
            });
        }
    }
}

/// The overhead chat RGB for a chat colour id (0-5 fixed, 6-11 animated); `progress` is
/// `150 - time * 150 / total_duration`.
pub fn chat_colour(colour: i32, scene_cycle: i32, progress: i32) -> i32 {
    let mut rgb = 16776960;
    if (0..6).contains(&colour) {
        rgb = CHAT_COLOURS[colour as usize];
    }
    match colour {
        6 => {
            rgb = if scene_cycle % 20 < 10 {
                16711680
            } else {
                16776960
            }
        }
        7 => rgb = if scene_cycle % 20 < 10 { 255 } else { 65535 },
        8 => {
            rgb = if scene_cycle % 20 < 10 {
                45056
            } else {
                8454016
            }
        }
        9 => {
            if progress < 50 {
                rgb = progress * 1280 + 16711680;
            } else if progress < 100 {
                rgb = 16776960 - (progress - 50) * 327680;
            } else if progress < 150 {
                rgb = (progress - 100) * 5 + 65280;
            }
        }
        10 => {
            if progress < 50 {
                rgb = progress * 5 + 16711680;
            } else if progress < 100 {
                rgb = 16711935 - (progress - 50) * 327680;
            } else if progress < 150 {
                rgb = (progress - 100) * 327680 + 255 - (progress - 100) * 5;
            }
        }
        11 => {
            if progress < 50 {
                rgb = 16777215 - progress * 327685;
            } else if progress < 100 {
                rgb = (progress - 50) * 327685 + 65280;
            } else if progress < 150 {
                rgb = 16777215 - (progress - 100) * 327680;
            }
        }
        _ => {}
    }
    rgb
}

/// Push line `n` above every earlier overlapping line
/// until it settles. `positions` are `chatX/chatY`, `widths` the half widths
/// (`chatWidth`), `line` is b12 `descent + ascent + 2`.
pub fn stack_chat_line(positions: &mut [[i32; 2]], widths: &[i32], n: usize, line: i32) {
    let ([x, mut y], w) = (positions[n], widths[n]);
    let mut moved = true;
    while moved {
        moved = false;
        for m in 0..n {
            let ([ox, oy], ow) = (positions[m], widths[m]);
            if y + 2 > oy - line
                && y - line < oy + 2
                && x - w < ow + ox
                && x + w > ox - ow
                && oy - line < y
            {
                y = oy - line;
                moved = true;
            }
        }
    }
    positions[n][1] = y;
}

/// Per-glyph `(x, y)` offset arrays for the glyph draw; `None` is an absent array.
pub type GlyphOffsets = (Option<Vec<i32>>, Option<Vec<i32>>);

/// Per-glyph `(x, y)` offsets for the wave, wave2 and shake chat effects, sized by the
/// UTF-16 length of the text; `None` is an absent array.
pub fn chat_effect_offsets(
    effect: i32,
    text_len: usize,
    scene_cycle: i32,
    progress: i32,
) -> GlyphOffsets {
    let c = f64::from(scene_cycle);
    let wave = |step: f64, amplitude: f64, speed: f64| -> Vec<i32> {
        (0..text_len)
            .map(|i| ((c / speed + i as f64 / step).sin() * amplitude) as i32)
            .collect()
    };
    match effect {
        1 => (None, Some(wave(2.0, 5.0, 5.0))),
        2 => (Some(wave(5.0, 5.0, 5.0)), Some(wave(3.0, 5.0, 5.0))),
        3 => {
            let amplitude = (7.0 - f64::from(progress) / 8.0).max(0.0);
            (None, Some(wave(1.5, amplitude, 1.0)))
        }
        _ => (None, None),
    }
}

/// Stack overlapping chat lines upward, then draw each with its colour and effect in the
/// b12 font.
fn draw_chats(frame: &Frame, chats: Vec<ChatLine>, r: &mut dyn Resources, out: &mut Vec<Draw>) {
    let [view_x, view_y, view_width, view_height] = frame.viewport;
    let line = r.descent(FontRef::B12) + r.ascent(FontRef::B12) + 2;
    let mut positions: Vec<[i32; 2]> = chats.iter().map(|c| [c.x, c.y]).collect();
    let widths: Vec<i32> = chats.iter().map(|c| c.width).collect();
    let shadow = 0xFF000000u32 as i32;
    for (n, c) in chats.iter().enumerate() {
        stack_chat_line(&mut positions, &widths, n, line);
        // The text was already substituted at collection; substituting it again gives the
        // same result.
        let text = &c.text;
        let [x, y] = positions[n];
        let centre_x = view_x + x;
        let base_y = view_y + y;
        let half = r.string_width(FontRef::B12, text) / 2;
        let draw = |pos: [i32; 2], colour: i32, offsets: Option<GlyphOffsets>| Draw::Text {
            font: FontRef::B12,
            text: text.clone(),
            pos,
            colour,
            shadow,
            offsets,
        };
        if frame.chat_effects != 0 {
            out.push(draw([centre_x - half, base_y], -256, None));
            continue;
        }
        // The line's total duration; a zero total would divide by zero, so it counts as 1.
        let total = if c.total == 0 { 1 } else { c.total };
        let progress = 150 - c.time * 150 / total;
        let colour = chat_colour(c.colour, frame.scene_cycle, progress) | shadow;
        match c.effect {
            // Plain centred text.
            0 => out.push(draw([centre_x - half, base_y], colour, None)),
            // Wave, wave2 and shake effects.
            1..=3 => {
                let offsets = chat_effect_offsets(
                    c.effect,
                    text.encode_utf16().count(),
                    frame.scene_cycle,
                    progress,
                );
                out.push(draw([centre_x - half, base_y], colour, Some(offsets)));
            }
            4 => {
                let scroll = progress * (r.string_width(FontRef::B12, text) + 100) / 150;
                out.push(Draw::SetBounds([
                    centre_x - 50,
                    view_y,
                    centre_x + 50,
                    view_y + view_height,
                ]));
                out.push(draw([centre_x + 50 - scroll, base_y], colour, None));
                out.push(Draw::ResetBounds([
                    view_x,
                    view_y,
                    view_x + view_width,
                    view_y + view_height,
                ]));
            }
            5 => {
                let offset = if progress < 25 {
                    progress - 25
                } else if progress > 125 {
                    progress - 125
                } else {
                    0
                };
                let h = r.descent(FontRef::B12) + r.ascent(FontRef::B12);
                out.push(Draw::SetBounds([
                    view_x,
                    base_y - h - 1,
                    view_x + view_width,
                    base_y + 5,
                ]));
                out.push(draw([centre_x - half, base_y + offset], colour, None));
                out.push(Draw::ResetBounds([
                    view_x,
                    view_y,
                    view_x + view_width,
                    view_y + view_height,
                ]));
            }
            _ => {}
        }
    }
}

/// Tile hint arrows (type 2)
/// blink every 10 of 20 cycles, 12/28 pixels up-left of the projected tile.
pub fn draw_tile_hint_arrows(
    frame: &Frame,
    arrows: &[(i32, [i32; 2], i32, i32)],
    project: &dyn Fn(i32, i32, i32, i32) -> [f32; 2],
    r: &mut dyn Resources,
) -> Vec<Draw> {
    let [view_x, view_y, ..] = frame.viewport;
    let mut out = Vec::new();
    for &(level, fine, height, sprite) in arrows {
        let p = project(level, fine[0], fine[1], height * 2);
        if p[0] > -1.0 && frame.loop_cycle % 20 < 10 {
            if let Some(s) = r.hint_arrow(sprite) {
                out.push(Draw::Sprite {
                    sprite: s,
                    pos: [
                        (p[0] + view_x as f32 - 12.0) as i32,
                        (p[1] + view_y as f32 - 28.0) as i32,
                    ],
                    colour: -1,
                });
            }
        }
    }
    out
}

/// The NPC config fields the scene-flag pass reads for one NPC (existence and visibility
/// checks fold into `Some`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NpcSceneType {
    pub follower: bool,
    pub drawabove: bool,
    pub drawbelow: bool,
}

fn centred(e: &Player) -> bool {
    let offset = if e.size & 1 == 0 { 0 } else { 256 };
    (e.fine_x as i32 & 511) == offset && (e.fine_z as i32 & 511) == offset
}

/// Whether this NPC is added to the scene: a non-negative draw priority, and a
/// tile-centred actor must not be deferred.
pub fn npc_added_to_scene(e: &Player) -> bool {
    let s = &e.actor.scene;
    s.priority >= 0 && !(centred(e) && s.deferred)
}

/// NPC draw priority and deferral flags: NPC priorities, hint arrows, then per plane the
/// entity markers are cleared, rebuilt and used to decide deferral. The high-resolution
/// players, already prioritised by `player_scene::insert`, occupy their places ahead of
/// the NPCs in each plane's marker arrays; only NPC flags are written here.
pub fn npc_scene_flags(
    players: &crate::protocol910::Players,
    npcs: &mut crate::protocol910::npc::Npcs,
    types: &dyn Fn(&crate::entities910::Npc) -> Option<NpcSceneType>,
    size: [usize; 2],
    draw_order: i32,
    active_target: i32,
    hint_npcs: &[usize],
) {
    let [sx, sz] = [size[0] as i32, size[1] as i32];
    for &index in &npcs.slots {
        let Some(n) = npcs.entities.get_mut(&index) else {
            continue;
        };
        let t = types(n);
        let bounds = crate::player_body::tile_bounds(&n.path);
        let bars = n.path.combat.as_ref().is_some_and(|c| !c.bars.is_empty());
        let npc_size = n.path.size;
        let s = &mut n.path.actor.scene;
        let Some(t) = t else {
            s.priority = -1;
            continue;
        };
        s.bounds = bounds;
        if bounds[0] < 0 || bounds[2] < 0 || bounds[1] >= sx || bounds[3] >= sz {
            s.priority = -1;
            continue;
        }
        let mut p = i32::from(!s.deferred);
        if bars {
            p += 2;
        }
        if npc_size < 5 {
            p += (5 - npc_size) << 2;
        }
        if draw_order == 0 {
            p += if t.follower { 64 } else { 128 };
        } else if draw_order == 1 {
            p += if t.follower { 32 } else { 64 };
        }
        if t.drawabove {
            p += 1024;
        } else if !t.drawbelow {
            p += 256;
        }
        // The active target is stored as the NPC index + 1.
        if index as i32 + 1 == active_target {
            p += 2047;
        }
        s.priority = p + 1;
    }
    for &index in hint_npcs {
        if let Some(n) = npcs.entities.get_mut(&index) {
            if n.path.actor.scene.priority >= 0 {
                n.path.actor.scene.priority += 2048;
            }
        }
    }
    if sx <= 0 || sz <= 0 {
        return;
    }
    let stride = size[1];
    let mut maximum = vec![0i32; size[0] * size[1]];
    let mut count = maximum.clone();
    let player_rows: Vec<&Player> = players
        .high_indices
        .iter()
        .filter_map(|&i| players.players.get(i).and_then(Option::as_ref))
        .collect();
    let cell = |x: i32, z: i32| -> Option<usize> {
        (x >= 0 && z >= 0 && x < sx && z < sz).then(|| x as usize * stride + z as usize)
    };
    let footprint = |e: &Player, margin: i32| {
        let r = (e.size - 1) * 256 + margin;
        let (x, z) = (e.fine_x as i32, e.fine_z as i32);
        [(x - r) >> 9, (x + r) >> 9, (z - r) >> 9, (z + r) >> 9]
    };
    for plane in 0..4 {
        maximum.fill(0);
        // Build the per-tile priority maxima and counts.
        let mut mark = |e: &Player, priority: i32, flag: bool| {
            if e.level != plane || priority < 0 || flag || !centred(e) {
                return;
            }
            let b = if e.size == 1 {
                let (x, z) = ((e.fine_x as i32) >> 9, (e.fine_z as i32) >> 9);
                [x, x, z, z]
            } else {
                footprint(e, 60)
            };
            for x in b[0]..=b[1] {
                for z in b[2]..=b[3] {
                    let Some(i) = cell(x, z) else { continue };
                    if priority > maximum[i] {
                        maximum[i] = priority;
                        count[i] = 1;
                    } else if priority == maximum[i] {
                        count[i] += 1;
                    }
                }
            }
        };
        for e in &player_rows {
            mark(e, e.actor.scene.priority, e.actor.scene.force_show);
        }
        for &index in &npcs.slots {
            if let Some(n) = npcs.entities.get(&index) {
                mark(&n.path, n.path.actor.scene.priority, n.flag);
            }
        }
        // The deferral decision only.
        let mut decide = |e: &Player, priority: i32, flag: bool| -> Option<bool> {
            if e.level != plane {
                return None;
            }
            if priority < 0 || !centred(e) {
                return Some(false);
            }
            if flag {
                return Some(false);
            }
            if e.size == 1 {
                let Some(i) = cell((e.fine_x as i32) >> 9, (e.fine_z as i32) >> 9) else {
                    return Some(false);
                };
                if priority != maximum[i] {
                    return Some(true);
                }
                if count[i] > 1 {
                    count[i] -= 1;
                    return Some(true);
                }
                return Some(false);
            }
            let b = footprint(e, 252);
            let mut available = false;
            for x in b[0]..=b[1] {
                for z in b[2]..=b[3] {
                    if let Some(i) = cell(x, z) {
                        available |= maximum[i] == priority && count[i] <= 1;
                    }
                }
            }
            if available {
                return Some(false);
            }
            for x in b[0]..=b[1] {
                for z in b[2]..=b[3] {
                    if let Some(i) = cell(x, z) {
                        if maximum[i] == priority {
                            count[i] -= 1;
                        }
                    }
                }
            }
            Some(true)
        };
        for e in &player_rows {
            decide(e, e.actor.scene.priority, e.actor.scene.force_show);
        }
        for &index in &npcs.slots {
            let Some(n) = npcs.entities.get_mut(&index) else {
                continue;
            };
            let (priority, flag) = (n.path.actor.scene.priority, n.flag);
            if let Some(deferred) = decide(&n.path, priority, flag) {
                n.path.actor.scene.deferred = deferred;
            }
        }
    }
}

#[cfg(test)]
mod tests;
