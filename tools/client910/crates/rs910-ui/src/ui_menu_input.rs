//! Popup placement, row hits and grouped submenus.
use super::*;
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Popup {
    pub bounds: [i32; 4],
    pub sub_bounds: [i32; 4],
    pub expanded: Option<usize>,
    pub ascent: i32,
    pub descent: i32,
    pub custom: bool,
    pub alpha_noise: i32,
}
/// Font metrics and style of a popup being opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PopupLook {
    pub ascent: i32,
    pub descent: i32,
    pub custom: bool,
}
#[derive(Clone)]
pub struct Row {
    pub text: String,
    pub entry: usize,
    pub group: Option<usize>,
    pub baseline: i32,
    pub enabled: bool,
    /// `hasArrow` of an entry row (never a submenu title).
    pub arrow: bool,
}
impl Row {
    /// `getEntryWidth` (`stringWidth(text + quest icon
    /// tags, fontSprites)`, plus `submenuArrowSprite.getWidth() + 4` for an
    /// arrow entry) or `getSubmenuWidth` for a submenu title.
    pub fn width(&self, arrow_width: i32, width: &mut impl FnMut(&str) -> i32) -> i32 {
        let text = width(&self.text);
        if self.arrow {
            text + arrow_width + 4
        } else {
            text
        }
    }
}
impl Entry {
    /// getEntryText (quest icons are appended per row).
    pub fn label(&self) -> String {
        let mut text = self.op.clone();
        for value in [&self.target, &self.detail]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
        {
            text.push_str(rs910_core::texts::Msg::MenuSeparator.get());
            text.push_str(value);
        }
        text
    }
}
fn inside([x, y]: [i32; 2], [l, t, w, h]: [i32; 4], margin: i32) -> bool {
    x >= l - margin && x <= l + w + margin && y >= t - margin && y <= t + h + margin
}
impl MiniMenu {
    /// getHoverCursor. Hover uses font metrics even with
    /// classic formatting; click hit-testing has different hardcoded extents.
    pub fn hover_cursor(&self, mouse: [i32; 2], dragged: bool, override_secondary: bool) -> i32 {
        if dragged {
            return -1;
        }
        if !self.open {
            return (if override_secondary {
                self.secondary
            } else {
                self.active
            })
            .map_or(-1, |id| self.entry(id).cursor);
        }
        let in_column = |b: [i32; 4]| mouse[0] > b[0] && mouse[0] < b[0].wrapping_add(b[2]);
        let sub = if in_column(self.popup.bounds) {
            false
        } else if self.grouped && self.popup.expanded.is_some() && in_column(self.popup.sub_bounds)
        {
            true
        } else {
            return -1;
        };
        // Grouped parent columns take precedence even if no Y row matches.
        self.popup_rows(sub)
            .iter()
            .rfind(|r| {
                mouse[1] > r.baseline - self.popup.ascent - 1
                    && mouse[1] < r.baseline + self.popup.descent
            })
            .map_or(-1, |r| self.entry(r.entry).cursor)
    }
    pub fn popup_rows(&self, sub: bool) -> Vec<Row> {
        let bounds = if sub {
            self.popup.sub_bounds
        } else {
            self.popup.bounds
        };
        let offset = if self.popup.custom {
            self.popup.ascent + 21
        } else {
            31
        };
        let groups: Vec<(usize, Option<usize>)> = if sub {
            self.popup
                .expanded
                .and_then(|id| self.subs[id].as_ref())
                .map(|s| s.entries.iter().rev().map(|&e| (e, None)).collect())
                .unwrap_or_default()
        } else if self.grouped {
            self.submenus
                .iter()
                .rev()
                .filter_map(|&id| {
                    self.subs[id]
                        .as_ref()
                        .map(|s| (*s.entries.last().unwrap(), (s.size > 1).then_some(id)))
                })
                .collect()
        } else {
            self.entries.iter().rev().map(|&e| (e, None)).collect()
        };
        groups
            .into_iter()
            .enumerate()
            .map(|(i, (entry, group))| {
                let e = self.entry(entry);
                let text = group
                    .map(|id| {
                        format!(
                            "{}<col=ffffff> >",
                            self.subs[id]
                                .as_ref()
                                .unwrap()
                                .title
                                .as_deref()
                                .unwrap_or("null")
                        )
                    })
                    .unwrap_or_else(|| {
                        // drawEntry / getEntryWidth:
                        // getEntryText + getQuestIconTags(getEntryQuests).
                        let mut text = e.label();
                        if let Some(tags) = self.quest_texts.get(&entry) {
                            text.push_str(tags);
                        }
                        text
                    });
                Row {
                    text,
                    entry,
                    group,
                    baseline: bounds[1] + offset + i as i32 * self.row_height,
                    enabled: e.enabled,
                    arrow: group.is_none() && e.has_arrow,
                }
            })
            .collect()
    }
    /// Opens the popup at `at` on a canvas of the given size, sized to its
    /// widest row (`width` measures a string).
    pub fn open_at(
        &mut self,
        at: [i32; 2],
        canvas: [i32; 2],
        look: PopupLook,
        show_single: bool,
        arrow_width: i32,
        mut width: impl FnMut(&str) -> i32,
    ) {
        if !show_single && self.option_count == 1 {
            return;
        }
        let PopupLook {
            ascent,
            descent,
            custom,
        } = look;
        self.popup = Popup {
            ascent,
            descent,
            custom,
            ..Default::default()
        };
        self.row_height = ascent + descent;
        let rows = self.popup_rows(false);
        let title_width = width(rs910_core::texts::Msg::ChooseOption.get());
        // openMenu: widest row, +8, +10.
        let w = rows
            .iter()
            .map(|r| r.width(arrow_width, &mut width))
            .fold(title_width, i32::max)
            + 18;
        let h = rows.len() as i32 * self.row_height;
        self.popup.bounds = [
            (at[0] - w / 2).min(canvas[0] - w).max(0),
            at[1].min(canvas[1] - h - 21).max(0),
            w,
            h + if custom { 26 } else { 22 },
        ];
        self.open = true;
    }
    /// Whether any queued entry draws `submenuArrowSprite` (`hasArrow`).
    pub fn has_arrow_entry(&self) -> bool {
        self.entries.iter().any(|&e| self.entry(e).has_arrow)
    }
    pub fn close_popup(&mut self) {
        self.open = false;
        self.popup.expanded = None;
        self.active = None;
        self.secondary = None;
    }
    /// Hit geometry uses the original client's strict row boundaries, independently of the
    /// inclusive popup bounds and the ten-pixel mouse-leave margin.
    pub fn popup_hit(&self, at: [i32; 2], sub: bool) -> Option<Row> {
        let b = if sub {
            self.popup.sub_bounds
        } else {
            self.popup.bounds
        };
        if !inside(at, b, 0) {
            return None;
        }
        let (above, below) = if self.popup.custom {
            (self.popup.ascent + 1, self.popup.descent)
        } else {
            (13, 3)
        };
        self.popup_rows(sub)
            .into_iter()
            .find(|r| at[1] > r.baseline - above && at[1] < r.baseline + below)
    }
    pub fn popup_input(
        &mut self,
        at: [i32; 2],
        select: bool,
        canvas: [i32; 2],
        arrow_width: i32,
        mut width: impl FnMut(&str) -> i32,
    ) -> Option<Entry> {
        if !self.open {
            return None;
        }
        if select {
            if self.popup.expanded.is_some() && inside(at, self.popup.sub_bounds, 0) {
                let hit = self
                    .popup_hit(at, true)
                    .map(|r| self.entry(r.entry).clone());
                self.close_popup();
                return hit;
            }
            if inside(at, self.popup.bounds, 0) {
                let hit = self
                    .popup_hit(at, false)
                    .map(|r| self.entry(r.entry).clone());
                if !self.grouped || hit.is_some() {
                    self.close_popup();
                }
                return hit;
            }
            return None;
        }
        if self.popup.expanded.is_some() {
            if inside(at, self.popup.sub_bounds, 10) {
                return None;
            }
            self.popup.expanded = None;
        }
        if !inside(at, self.popup.bounds, 10) {
            self.close_popup();
            return None;
        }
        // The original client tests row Y throughout the ten-pixel margin, without requiring
        // the cursor to be horizontally inside the frame.
        let bounds = self.popup.bounds;
        let probe = [at[0].clamp(bounds[0], bounds[0] + bounds[2]), at[1]];
        if let Some(row) = self.popup_hit(probe, false).filter(|r| r.group.is_some()) {
            self.popup.expanded = row.group;
            let rows = self.popup_rows(true);
            // expandSubmenu.
            let w = rows
                .iter()
                .map(|r| r.width(arrow_width, &mut width))
                .max()
                .unwrap_or(0)
                + 8;
            let h = rows.len() as i32 * self.row_height;
            let [x, y, mw, _] = self.popup.bounds;
            let left = if x + mw + w > canvas[0] {
                x - w
            } else {
                x + mw
            };
            let row_top = row.baseline
                - if self.popup.custom {
                    self.popup.ascent + 1
                } else {
                    13
                };
            let offset = if self.popup.custom {
                self.popup.ascent + 21
            } else {
                31
            };
            self.popup.sub_bounds = [
                left.max(0),
                (self.popup.ascent + row_top - offset + 1)
                    .min(canvas[1] - h - 21)
                    .max(0),
                w,
                h + if self.popup.custom { 26 } else { 22 },
            ];
            let _ = y;
        }
        None
    }
}
