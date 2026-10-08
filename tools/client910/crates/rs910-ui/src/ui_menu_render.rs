//! Shared-target popup consumer.
use crate::{ui_fonts::Font, ui_leaf::Context, ui_properties::State, ui_sprites::Sprite};
use anyhow::{Context as _, Result};
use rs910_core::fault::Fault;
use std::rc::Rc;
pub fn font(state: &State) -> Result<Rc<Font>> {
    let fonts = state.fonts.as_ref().context("menu fonts not installed")?;
    if state.menu.custom {
        if let Some(font) = fonts.get_font(state.menu.format[11], true, true)? {
            return Ok(font);
        }
    }
    let id = *fonts
        .ids
        .as_ref()
        .and_then(|ids| ids.get(2))
        .context("b12_full font id")?;
    fonts
        .get_font(id, true, true)?
        .context("b12_full font unavailable")
}
fn sprite(state: &State, index: usize, flip: bool) -> Result<Option<Rc<Sprite>>> {
    let id = state.menu.format[index];
    if let Some(sprite) = state.menu.decoded.borrow().get(&(id, flip)) {
        return Ok(Some(sprite.clone()));
    }
    let Some(bytes) = state
        .menu
        .resources
        .iter()
        .find(|(a, i, _)| *a == "sprites" && *i == id)
        .and_then(|(_, _, b)| b.as_ref())
    else {
        return Ok(None);
    };
    let mut data = crate::sprite_data::Data::decode(bytes)?;
    let Some(data) = data.first_mut() else {
        return Ok(None);
    };
    if flip {
        data.flip(false);
    }
    let sprite = Rc::new(Sprite::new(data)?);
    state
        .menu
        .decoded
        .borrow_mut()
        .insert((id, flip), sprite.clone());
    Ok(Some(sprite))
}
pub fn paint(ctx: &mut Context, state: &mut State, mouse: [i32; 2]) -> Result<()> {
    let menu = &state.minimenu;
    if !menu.open {
        return paint_hover_text(ctx, state);
    }
    let fonts = state.fonts.as_ref().context("menu fonts")?;
    let font = font(state)?;
    let old = ctx.painter.sprite.clip;
    ctx.painter
        .reset_bounds([0, 0, state.layout.canvas[0], state.layout.canvas[1]]);
    for sub in [false, true] {
        if sub && menu.popup.expanded.is_none() {
            continue;
        }
        let [x, y, w, h] = if sub {
            menu.popup.sub_bounds
        } else {
            menu.popup.bounds
        };
        let title = if sub {
            menu.popup_rows(false)
                .into_iter()
                .find(|r| r.group == menu.popup.expanded)
                .map(|r| r.text.trim_end_matches("<col=ffffff> >").to_owned())
                .unwrap_or_default()
        } else {
            rs910_core::texts::Msg::ChooseOption.get().into()
        };
        let f = state.menu.format;
        if state.menu.custom {
            let alpha = (255 - f[1] - menu.popup.alpha_noise).max(0);
            if let (Some(mid), Some(left), Some(right)) = (
                sprite(state, 4, false)?,
                sprite(state, 5, false)?,
                sprite(state, 5, true)?,
            ) {
                let mw = mid.full_size()[0];
                let lw = left.full_size()[0];
                if mw > 0 {
                    for i in 0..(w - lw * 2) / mw {
                        ctx.painter.native(&mid, [x + lw + i * mw, y], -1);
                    }
                }
                ctx.painter.native(&left, [x, y], -1);
                ctx.painter
                    .native(&right, [x + w - right.full_size()[0], y], -1);
            } else {
                ctx.painter.fill([x, y, w, 20], alpha << 24 | f[0])?;
            }
            ctx.painter
                .fill([x, y + 20, w, h - 20], alpha << 24 | f[0])?;
            ctx.menu_text(
                fonts,
                font.clone(),
                &title,
                [
                    x + 3,
                    y + (20 - font.metrics.ascent) / 2 + font.metrics.ascent,
                ],
                f[9] | 0xff000000u32 as i32,
                -1,
            )?;
            if let Some(row) = menu
                .popup_hit(mouse, sub)
                .filter(|r| r.enabled || r.group.is_some())
            {
                ctx.painter.fill(
                    [x, row.baseline - font.metrics.ascent, w, menu.row_height],
                    (255 - f[3] - menu.popup.alpha_noise).max(0) << 24 | f[2],
                )?;
            }
            if let (Some(bottom), Some(left), Some(right), Some(corner), Some(other)) = (
                sprite(state, 6, false)?,
                sprite(state, 7, false)?,
                sprite(state, 7, true)?,
                sprite(state, 8, false)?,
                sprite(state, 8, true)?,
            ) {
                let [bw, bh] = bottom.full_size();
                let [lw, lh] = left.full_size();
                let [cw, ch] = corner.full_size();
                if bw > 0 {
                    for i in 0..(w - 2 * cw) / bw {
                        ctx.painter
                            .native(&bottom, [x + cw + i * bw, y + h - bh], -1);
                    }
                }
                if lh > 0 {
                    for i in 0..(h - ch - 20) / lh {
                        ctx.painter.native(&left, [x, y + 20 + i * lh], -1);
                        ctx.painter
                            .native(&right, [x + w - lw, y + 20 + i * lh], -1);
                    }
                }
                ctx.painter.native(&corner, [x, y + h - ch], -1);
                ctx.painter.native(&other, [x + w - cw, y + h - ch], -1);
            }
        } else {
            ctx.painter.fill([x, y, w, h], -10660793)?;
            ctx.painter
                .fill([x + 1, y + 1, w - 2, 16], 0xff000000u32 as i32)?;
            ctx.painter
                .outline([x + 1, y + 18, w - 2, h - 19], 0xff000000u32 as i32);
            ctx.menu_text(fonts, font.clone(), &title, [x + 3, y + 14], -10660793, -1)?;
        }
        for row in menu.popup_rows(sub) {
            let hover = mouse[0] > x
                && mouse[0] < x + w
                && mouse[1] > row.baseline - font.metrics.ascent - 1
                && mouse[1] < row.baseline + font.metrics.descent
                && (row.enabled || row.group.is_some());
            let colour = if state.menu.custom {
                f[if hover { 10 } else { 9 }] | 0xff000000u32 as i32
            } else if hover {
                -256
            } else {
                -1
            };
            ctx.menu_entry(
                fonts,
                font.clone(),
                &row.text,
                [x + 3, row.baseline],
                colour,
            )?;
            // drawEntry: submenuArrowSprite after the entry text.
            if row.arrow {
                let arrow = state
                    .menu
                    .submenu_arrow
                    .as_ref()
                    .context(Fault::MissingValue.message("submenu arrow sprite"))?;
                let units: Vec<u16> = row.text.encode_utf16().collect();
                let text_width = fonts.width(&font.metrics, Some(&units))?;
                ctx.painter.native(
                    arrow,
                    [x + 5 + text_width, row.baseline - font.metrics.ascent],
                    -1,
                );
            }
        }
    }
    paint_hover_text(ctx, state)?;
    ctx.painter.reset_bounds(old);
    Ok(())
}

/// `drawHoverText`. The HOVER_TEXT component is
/// retained by the UI walk; this consumer formats the active ordinary menu
/// entry and paints it with that component's bounds/style after the interface
/// tree and popup have been drawn.
fn paint_hover_text(ctx: &mut Context, state: &mut State) -> Result<()> {
    let Some(hover) = state.hover_text.clone() else {
        return Ok(());
    };
    let menu = &state.minimenu;
    if (menu.option_count < 2 && !state.interaction.target.active)
        || state.interaction.drag.component.is_some()
    {
        return Ok(());
    }
    let mut text = if state.interaction.target.active && menu.option_count < 2 {
        format!(
            "{}{}{} ->",
            state.interaction.target.verb,
            rs910_core::texts::Msg::MenuSeparator.get(),
            state.interaction.target.name
        )
    } else {
        let view = menu.view(menu.active);
        if view.op.is_empty() {
            return Ok(());
        }
        let mut value = view.op;
        if !view.op_base.is_empty() {
            value.push_str(rs910_core::texts::Msg::MenuSeparator.get());
            value.push_str(&view.op_base);
        }
        if let Some(quest) = view.quest_text.filter(|text| !text.is_empty()) {
            value.push_str(&quest);
        }
        value
    };
    if menu.option_count > 2 {
        text.push_str(&format!(
            " / {}{}",
            menu.option_count - 2,
            rs910_core::texts::Msg::MoreOptions.get()
        ));
    }
    let fonts = state.fonts.as_ref().context("hover text fonts")?;
    let font = fonts
        .get_font(hover.font, true, hover.font_mono)?
        .or_else(|| font(state).ok())
        .context("hover text font unavailable")?;
    let units: Vec<u16> = text.encode_utf16().collect();
    let width = font.metrics.width_utf16(&units, None)?;
    let line_height = font.metrics.ascent + font.metrics.descent;
    let x = match hover.halign {
        1 => hover.origin[0] + (hover.size[0] - width) / 2,
        2 => hover.origin[0] + hover.size[0] - width,
        _ => hover.origin[0],
    };
    let y = match hover.valign {
        1 => hover.origin[1] + (hover.size[1] - line_height) / 2 + font.metrics.ascent,
        2 => hover.origin[1] + hover.size[1] - font.metrics.descent,
        _ => hover.origin[1] + font.metrics.ascent,
    };
    let old = ctx.painter.sprite.clip;
    ctx.painter
        .reset_bounds([0, 0, state.layout.canvas[0], state.layout.canvas[1]]);
    ctx.menu_text(fonts, font, &text, [x, y], hover.colour, hover.shadow)?;
    ctx.painter.reset_bounds(old);
    Ok(())
}
