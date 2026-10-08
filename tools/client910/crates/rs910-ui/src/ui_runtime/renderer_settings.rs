//! Local modern graphics controls within the retained Graphics flow. Typed
//! changes leave through the shell preference phase, never through CS2 vars.

use crate::{ui_leaf::Context, ui_loop::Input, ui_properties::State};
use anyhow::Result;
use rs910_config::renderer_preferences::{RendererControl, RendererPreferences};

const ORIGIN: i32 = 0;
const OUTSIDE: i32 = -1;
const RECT_X: usize = 0;
const RECT_Y: usize = 1;
const RECT_WIDTH: usize = 2;
const RECT_HEIGHT: usize = 3;
const HALF: i32 = 2;
const AXES: usize = 2;
const RECT_COMPONENTS: usize = 4;
#[cfg(test)]
const PRIMARY_CLICK_COUNT: i32 = 1;
const MARGIN: i32 = 12;
const PANEL_WIDTH: i32 = 560;
const PANEL_HEIGHT: i32 = 450;
const HEADER_HEIGHT: i32 = 66;
const ROW_HEIGHT: i32 = 36;
const FOOTER_HEIGHT: i32 = 48;
const BUTTON_WIDTH: i32 = 148;
const BUTTON_HEIGHT: i32 = 28;
const ARROW_WIDTH: i32 = 28;
const VALUE_WIDTH: i32 = 240;
const FIRST_ROW: usize = 0;
const ROW_STEP: usize = 1;
const LEFT_PRESS: i32 = 0;
const KEY_ESCAPE: i32 = 13;
const KEY_TAB: i32 = 80;
const KEY_ENTER: i32 = 84;
const KEY_BACKSPACE: i32 = 85;
const KEY_LEFT: i32 = 96;
const KEY_RIGHT: i32 = 97;
const KEY_UP: i32 = 98;
const KEY_DOWN: i32 = 99;
const BACKGROUND: i32 = 0xff17232c_u32 as i32;
const BUTTON: i32 = 0xff263e4b_u32 as i32;
const FOCUS: i32 = 0xff45687a_u32 as i32;
const BORDER: i32 = 0xff71909e_u32 as i32;
const TEXT: i32 = 0xffeee5ce_u32 as i32;
const MUTED: i32 = 0xffabc2cb_u32 as i32;
const NO_SHADOW: i32 = -1;
const ALIGN_LEFT: i32 = 0;
const ALIGN_CENTRE: i32 = 1;
const LINE_HEIGHT: i32 = 0;
const DEFAULT_TEXT_FONT: usize = 1;

#[derive(Default)]
pub struct Panel {
    available: bool,
    open: bool,
    selected: usize,
    requested: RendererPreferences,
    effective: RendererPreferences,
    pending: Option<RendererPreferences>,
    status: Option<String>,
}

impl Panel {
    pub fn sync(
        &mut self,
        available: bool,
        requested: RendererPreferences,
        effective: RendererPreferences,
    ) {
        self.available = available;
        if !available {
            self.open = false;
        }
        if self.pending.is_none() {
            self.requested = requested;
        }
        self.effective = effective;
    }
    pub fn take_change(&mut self) -> Option<RendererPreferences> {
        self.pending.take()
    }
    pub fn saved(&mut self, error: Option<String>) {
        self.status =
            Some(error.unwrap_or_else(|| "Changes saved. Applied to the next frame.".into()));
    }
    fn mounted(&self, state: &State) -> bool {
        let id = rs910_symbols::interface::GRAPHICS_SETTINGS_PANEL.id();
        self.available
            && (state.life.top == id || state.life.subs.ordered().any(|s| s.borrow().id == id))
    }
    fn launch(canvas: [i32; AXES]) -> [i32; RECT_COMPONENTS] {
        [
            canvas[RECT_X] - BUTTON_WIDTH - MARGIN,
            canvas[RECT_Y] - BUTTON_HEIGHT - MARGIN,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
        ]
    }
    fn bounds(canvas: [i32; AXES]) -> [i32; RECT_COMPONENTS] {
        let width = PANEL_WIDTH.min(canvas[RECT_X]);
        let height = PANEL_HEIGHT.min(canvas[RECT_Y]);
        [
            (canvas[RECT_X] - width) / HALF,
            (canvas[RECT_Y] - height) / HALF,
            width,
            height,
        ]
    }
    fn close(bounds: [i32; RECT_COMPONENTS]) -> [i32; RECT_COMPONENTS] {
        [
            bounds[RECT_X] + bounds[RECT_WIDTH] - MARGIN - BUTTON_HEIGHT,
            bounds[RECT_Y] + MARGIN,
            BUTTON_HEIGHT,
            BUTTON_HEIGHT,
        ]
    }
    fn reset(bounds: [i32; RECT_COMPONENTS]) -> [i32; RECT_COMPONENTS] {
        [
            bounds[RECT_X] + MARGIN,
            bounds[RECT_Y] + bounds[RECT_HEIGHT] - FOOTER_HEIGHT,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
        ]
    }
    fn row(bounds: [i32; RECT_COMPONENTS], index: usize) -> [i32; RECT_COMPONENTS] {
        [
            bounds[RECT_X] + MARGIN,
            bounds[RECT_Y] + HEADER_HEIGHT + index as i32 * ROW_HEIGHT,
            bounds[RECT_WIDTH] - MARGIN * HALF,
            ROW_HEIGHT,
        ]
    }
    fn arrow(row: [i32; RECT_COMPONENTS], forward: bool) -> [i32; RECT_COMPONENTS] {
        [
            row[RECT_X] + row[RECT_WIDTH] - if forward { ARROW_WIDTH } else { VALUE_WIDTH },
            row[RECT_Y],
            ARROW_WIDTH,
            BUTTON_HEIGHT,
        ]
    }
    fn change(&mut self, control: RendererControl, forward: bool) {
        self.requested.step(control, forward);
        self.pending = Some(self.requested);
        self.status = None;
    }
    fn defaults(&mut self) {
        self.requested = RendererPreferences::DEFAULT;
        self.pending = Some(self.requested);
        self.status = None;
    }
    /// Consume overlay input before the native interface and menu walk. The
    /// recorded mouse/key stream still reaches this owner on session replay.
    pub fn input(&mut self, state: &State, input: &mut Input) -> bool {
        if !self.mounted(state) {
            self.open = false;
            return false;
        }
        if input.console_open {
            return false;
        }
        let canvas = state.layout.canvas;
        let click = input
            .event
            .filter(|e| e.action == LEFT_PRESS)
            .and(input.click);
        if !self.open {
            if click.is_some_and(|p| contains(Self::launch(canvas), p)) {
                self.open = true;
                self.selected = FIRST_ROW;
            } else {
                return false;
            }
        } else {
            let bounds = Self::bounds(canvas);
            if let Some(p) = click {
                if contains(Self::close(bounds), p) {
                    self.open = false;
                } else if contains(Self::reset(bounds), p) {
                    self.defaults();
                } else {
                    for (index, &control) in RendererControl::ALL.iter().enumerate() {
                        let row = Self::row(bounds, index);
                        if contains(row, p) {
                            self.selected = index;
                            if contains(Self::arrow(row, false), p) {
                                self.change(control, false);
                            } else if contains(Self::arrow(row, true), p) {
                                self.change(control, true);
                            }
                            break;
                        }
                    }
                }
            }
            for &(code, _) in &input.keys {
                match code {
                    KEY_ESCAPE => self.open = false,
                    KEY_BACKSPACE => self.defaults(),
                    KEY_UP => {
                        self.selected = (self.selected + RendererControl::ALL.len() - ROW_STEP)
                            % RendererControl::ALL.len()
                    }
                    KEY_DOWN | KEY_TAB => {
                        self.selected = (self.selected + ROW_STEP) % RendererControl::ALL.len()
                    }
                    KEY_LEFT => self.change(RendererControl::ALL[self.selected], false),
                    KEY_RIGHT | KEY_ENTER => self.change(RendererControl::ALL[self.selected], true),
                    _ => {}
                }
            }
        }
        input.click = None;
        input.event = None;
        input.keys.clear();
        input.held_keys.fill(false);
        input.wheel = ORIGIN;
        input.left_held = false;
        input.middle_held = false;
        input.right_held = false;
        input.mouse = [OUTSIDE; AXES];
        true
    }

    pub fn paint(&self, ctx: &mut Context, state: &State) -> Result<()> {
        if !self.mounted(state) {
            return Ok(());
        }
        let Some(fonts) = state.fonts.as_ref() else {
            return Ok(());
        };
        let Some(&id) = fonts
            .ids
            .as_ref()
            .and_then(|ids| ids.get(DEFAULT_TEXT_FONT))
        else {
            return Ok(());
        };
        let Some(font) = fonts.get_font(id, true, true)? else {
            return Ok(());
        };
        let old = ctx.painter.sprite.clip;
        ctx.painter.reset_bounds([
            ORIGIN,
            ORIGIN,
            state.layout.canvas[RECT_X],
            state.layout.canvas[RECT_Y],
        ]);
        let result = (|| -> Result<()> {
            let text = |ctx: &mut Context, label: &str, rect, colour, centre| -> Result<()> {
                crate::loading::draw_string_taggable(
                    &mut ctx.painter,
                    &font,
                    label,
                    rect,
                    [colour, NO_SHADOW],
                    [if centre { ALIGN_CENTRE } else { ALIGN_LEFT }, ALIGN_CENTRE],
                    LINE_HEIGHT,
                )?;
                Ok(())
            };
            if !self.open {
                let rect = Self::launch(state.layout.canvas);
                ctx.painter.fill(rect, BUTTON)?;
                ctx.painter.outline(rect, BORDER);
                return text(ctx, "Modern graphics", rect, TEXT, true);
            }
            let bounds = Self::bounds(state.layout.canvas);
            ctx.painter.fill(bounds, BACKGROUND)?;
            ctx.painter.outline(bounds, BORDER);
            let title = [
                bounds[RECT_X] + MARGIN,
                bounds[RECT_Y] + MARGIN,
                bounds[RECT_WIDTH] - MARGIN * HALF - BUTTON_HEIGHT,
                BUTTON_HEIGHT,
            ];
            text(ctx, "Modern graphics", title, TEXT, false)?;
            let description = [
                title[RECT_X],
                title[RECT_Y] + BUTTON_HEIGHT,
                title[RECT_WIDTH],
                BUTTON_HEIGHT,
            ];
            text(
                ctx,
                "Arrows change. Tab selects. Backspace resets. Esc closes.",
                description,
                MUTED,
                false,
            )?;
            let close = Self::close(bounds);
            ctx.painter.fill(close, BUTTON)?;
            text(ctx, "X", close, TEXT, true)?;
            for (index, &control) in RendererControl::ALL.iter().enumerate() {
                let row = Self::row(bounds, index);
                if self.selected == index {
                    ctx.painter.fill(row, FOCUS)?;
                }
                let label = [
                    row[RECT_X],
                    row[RECT_Y],
                    row[RECT_WIDTH] - VALUE_WIDTH,
                    BUTTON_HEIGHT,
                ];
                text(ctx, control.label(), label, TEXT, false)?;
                let value = [
                    row[RECT_X] + row[RECT_WIDTH] - VALUE_WIDTH + ARROW_WIDTH,
                    row[RECT_Y],
                    VALUE_WIDTH - ARROW_WIDTH * HALF,
                    BUTTON_HEIGHT,
                ];
                let requested = self.requested.value(control);
                let effective = self.effective.value(control);
                let label = if control != RendererControl::Preset && requested != effective {
                    format!("{requested} (using {effective})")
                } else {
                    requested
                };
                text(ctx, &label, value, TEXT, true)?;
                for (forward, arrow) in [(false, "<"), (true, ">")] {
                    let rect = Self::arrow(row, forward);
                    ctx.painter.fill(rect, BUTTON)?;
                    text(ctx, arrow, rect, TEXT, true)?;
                }
            }
            let reset = Self::reset(bounds);
            ctx.painter.fill(reset, BUTTON)?;
            text(ctx, "Reset to defaults", reset, TEXT, true)?;
            let status = [
                reset[RECT_X] + BUTTON_WIDTH + MARGIN,
                reset[RECT_Y],
                bounds[RECT_WIDTH] - BUTTON_WIDTH - MARGIN * HALF,
                BUTTON_HEIGHT,
            ];
            let message = self
                .status
                .as_deref()
                .unwrap_or(if self.requested != self.effective {
                    "Some choices are overridden for this session."
                } else {
                    "Changes apply immediately and save locally."
                });
            text(ctx, message, status, MUTED, false)?;
            Ok(())
        })();
        ctx.painter.reset_bounds(old);
        result
    }
}

fn contains(rect: [i32; RECT_COMPONENTS], p: [i32; AXES]) -> bool {
    p[RECT_X] >= rect[RECT_X]
        && p[RECT_Y] >= rect[RECT_Y]
        && p[RECT_X] < rect[RECT_X] + rect[RECT_WIDTH]
        && p[RECT_Y] < rect[RECT_Y] + rect[RECT_HEIGHT]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_only_capture_mounted_graphics_input_and_queue_typed_changes() {
        const CANVAS: [i32; AXES] = [800, 600];
        let mut state = State::default();
        state.layout.canvas = CANVAS;
        state.life.top = rs910_symbols::interface::GRAPHICS_SETTINGS_PANEL.id();
        let mut panel = Panel::default();
        panel.sync(
            true,
            RendererPreferences::DEFAULT,
            RendererPreferences::DEFAULT,
        );
        let launch = Panel::launch(CANVAS);
        let mut input = Input {
            click: Some([launch[RECT_X], launch[RECT_Y]]),
            event: Some(crate::ui_defaults::MouseEvent {
                action: LEFT_PRESS,
                pos: [ORIGIN; AXES],
                count: PRIMARY_CLICK_COUNT,
            }),
            ..Default::default()
        };
        assert!(panel.input(&state, &mut input));
        assert!(panel.open);
        assert!(input.click.is_none());
        input.keys = vec![(KEY_RIGHT, ORIGIN)];
        assert!(panel.input(&state, &mut input));
        assert_eq!(
            panel.take_change(),
            Some(rs910_config::renderer_preferences::QualityPreset::Ultra.preferences())
        );
        state.life.top = OUTSIDE;
        input.keys = vec![(KEY_RIGHT, ORIGIN)];
        assert!(!panel.input(&state, &mut input));
        assert!(!panel.open);
        assert!(!input.keys.is_empty());
    }
}
