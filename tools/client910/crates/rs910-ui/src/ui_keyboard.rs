//! The keyboard owner: AWT key events → client key codes, held keys and
//! per-logic-update event processing, split into `allKeyboardEvents` (repeat/typed, cap 131, feeds `onkey` and
//! keybind chars) and `keyboardEvents` (presses, cap 75, feeds bindings).
//! AWT `keyPressed`/`keyReleased`/`keyTyped`/`focusLost` are replayed from the
//! window owner with the same actions (0/1/3/-1).

/// The key code map (with the platform patches applied), indexed by the AWT
/// virtual key code.
fn keycode_map() -> [i32; 528] {
    let mut m = [0i32; 528];
    // keycodeMap verbatim (521 entries), then patchKeymap.
    let head: [i32; 521] = [
        0, 0, 0, 0, 0, 0, 0, 0, 85, 80, 84, 0, 91, 0, 0, 0, 81, 82, 86, 0, 0, 0, 0, 0, 0, 0, 0, 13,
        0, 0, 0, 0, 83, 104, 105, 103, 102, 96, 98, 97, 99, 0, 0, 0, 0, 0, 0, 0, 25, 16, 17, 18,
        19, 20, 21, 22, 23, 24, 0, 0, 0, 0, 0, 0, 0, 48, 68, 66, 50, 34, 51, 52, 53, 39, 54, 55,
        56, 70, 69, 40, 41, 32, 35, 49, 36, 38, 67, 33, 65, 37, 64, 0, 0, 0, 0, 0, 25, 16, 17, 18,
        19, 20, 21, 22, 23, 24, 89, 87, 0, 88, 229, 90, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 0,
        0, 0, 101, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143,
        0, 0, 0, 0, 0, 0, 150, 151, 152, 153, 0, 100, 0, 0, 0, 0, 160, 161, 162, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    m[..head.len()].copy_from_slice(&head);
    // patchKeymap
    m[44] = 71;
    m[45] = 26;
    m[46] = 72;
    m[47] = 73;
    m[59] = 57;
    m[61] = 27;
    m[91] = 42;
    m[92] = 74;
    m[93] = 43;
    m[192] = 28;
    m[222] = 58;
    m[520] = 59;
    m
}

/// A queued keyboard event: action, char, client key code, time and modifier
/// mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub action: i32,
    pub ch: u16,
    pub code: i32,
    pub time: i64,
    pub modifiers: i32,
}

pub struct Keyboard {
    map: [i32; 528],
    raw: Vec<Event>,
    events: Vec<Event>,
    pub held: [bool; 112],
}
impl Default for Keyboard {
    fn default() -> Self {
        Self {
            map: keycode_map(),
            raw: Vec::new(),
            events: Vec::new(),
            held: [false; 112],
        }
    }
}
impl Keyboard {
    /// Whether an AWT virtual key is represented in this platform mapping.
    pub fn supports_awt_code(&self, awt_code: i32) -> bool {
        usize::try_from(awt_code)
            .ok()
            .and_then(|index| self.map.get(index))
            .is_some_and(|mapped| *mapped != 0)
    }

    /// Translates a key event: 0 = pressed, 1 = released. Codes with
    /// bit 0x80 are release-only; the bit is stripped from the queued code.
    pub fn key(&mut self, awt_code: i32, action: i32, now: i64) {
        let mapped = if awt_code == 0 {
            0
        } else if let Some(&m) = usize::try_from(awt_code).ok().and_then(|i| self.map.get(i)) {
            if action == 0 && m & 0x80 != 0 {
                0
            } else {
                m & !0x80
            }
        } else {
            0
        };
        if mapped != 0 {
            self.queue(action, 65535, mapped, now);
        }
    }
    /// A typed character: printable Windows-1252 characters only.
    pub fn typed(&mut self, ch: char, now: i64) {
        let c = ch as u32;
        let printable = (c > 0 && c < 128) || (160..=255).contains(&c) || {
            // The Cp1252 extension table: the 0x80-0x9F code points.
            (0x80..=0x9F).any(|b| crate::config::cp1252(b as u8) == Some(ch))
        };
        if c != 65535 && printable {
            self.queue(3, ch as u16, -1, now);
        }
    }
    /// Window focus lost: releases every held key.
    pub fn focus_lost(&mut self, now: i64) {
        self.queue(-1, 0, 0, now);
    }
    fn queue(&mut self, action: i32, ch: u16, code: i32, now: i64) {
        self.raw.push(Event {
            action,
            ch,
            code,
            time: now,
            modifiers: 0,
        });
    }
    /// Whether the key with this code is held.
    pub fn held(&self, code: i32) -> bool {
        usize::try_from(code)
            .ok()
            .and_then(|c| self.held.get(c))
            .copied()
            .unwrap_or(false)
    }
    /// The modifier mask: shift 0x1 (81), ctrl 0x4 (82), alt 0x2 (86).
    pub fn modifier_mask(&self) -> i32 {
        (self.held[81] as i32) | ((self.held[82] as i32) << 2) | ((self.held[86] as i32) << 1)
    }
    /// Moves the pending events into the per-update queues.
    pub fn process_events(&mut self) {
        self.events.clear();
        for mut e in std::mem::take(&mut self.raw) {
            e.modifiers = self.modifier_mask();
            match e.action {
                0 => {
                    let slot = e.code as usize;
                    if !self.held[slot] {
                        self.events.push(Event {
                            action: 0,
                            ch: 65535,
                            ..e
                        });
                        self.held[slot] = true;
                    }
                    e.action = 2;
                    self.events.push(e);
                }
                1 => {
                    let slot = e.code as usize;
                    if self.held[slot] {
                        self.events.push(e);
                        self.held[slot] = false;
                    }
                }
                -1 => {
                    for slot in 0..112 {
                        if self.held[slot] {
                            self.events.push(Event {
                                action: 1,
                                ch: 65535,
                                code: slot as i32,
                                time: e.time,
                                modifiers: e.modifiers,
                            });
                            self.held[slot] = false;
                        }
                    }
                }
                3 => self.events.push(e),
                _ => {}
            }
        }
    }
    /// `mainloop`: (allKeyboardEvents, keyboardEvents).
    pub fn poll(&mut self) -> (Vec<Event>, Vec<Event>) {
        let mut all = Vec::new();
        let mut presses = Vec::new();
        for e in std::mem::take(&mut self.events) {
            match e.action {
                2 | 3 => {
                    if all.len() < 131 {
                        all.push(e);
                    }
                }
                0 if presses.len() < 75 => {
                    presses.push(e);
                }
                _ => {}
            }
        }
        (all, presses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_repeat_release_follow_the_recorded_keyboard() {
        let mut k = Keyboard::default();
        k.key(65, 0, 1); // A pressed -> code 48
        k.typed('a', 1);
        k.key(65, 0, 2); // AWT repeat
        k.key(65, 1, 3);
        k.key(16, 0, 4); // shift -> 81
        k.process_events();
        assert!(k.held(81));
        assert!(!k.held(48));
        let (all, presses) = k.poll();
        assert_eq!(
            presses.iter().map(|e| e.code).collect::<Vec<_>>(),
            vec![48, 81]
        );
        // Every press also yields its action-2 repeat entry.
        assert_eq!(
            all.iter()
                .map(|e| (e.action, e.code, e.ch))
                .collect::<Vec<_>>(),
            vec![
                (2, 48, 65535),
                (3, -1, 'a' as u16),
                (2, 48, 65535),
                (2, 81, 65535)
            ]
        );
        k.focus_lost(5);
        k.process_events();
        assert!(!k.held(81));
        assert_eq!(k.modifier_mask(), 0);
    }

    #[test]
    fn release_only_codes_are_ignored_on_press() {
        let mut k = Keyboard::default();
        k.key(110, 0, 1); // keycodeMap[110] = 229 (0x80 set): no press event
        k.key(110, 1, 2); // released as 229 & !0x80 = 101, but never held: dropped
        k.process_events();
        let (all, presses) = k.poll();
        assert!(all.is_empty() && presses.is_empty());
    }

    #[test]
    fn modifier_mask_follows_the_recorded_keyboard() {
        // The modifier mask: shift (81) is 0x1, ctrl (82) is 0x4, alt (86) is
        // 0x2. The mask is the modifier operand the camera branch rejects
        // ("Modifier keys cannot be used for camera controls."), so these bits
        // are the guard's input.
        let mut k = Keyboard::default();
        assert_eq!(k.modifier_mask(), 0);
        k.key(16, 0, 1); // shift -> 81
        k.process_events();
        assert_eq!(k.modifier_mask(), 0x1);
        k.key(17, 0, 2); // ctrl -> 82
        k.process_events();
        assert_eq!(k.modifier_mask(), 0x1 | 0x4);
        k.key(18, 0, 3); // alt -> 86
        k.process_events();
        assert_eq!(k.modifier_mask(), 0x1 | 0x4 | 0x2);
        k.key(16, 1, 4);
        k.process_events();
        assert_eq!(k.modifier_mask(), 0x4 | 0x2);
        k.focus_lost(5);
        k.process_events();
        assert_eq!(k.modifier_mask(), 0);
    }
}
