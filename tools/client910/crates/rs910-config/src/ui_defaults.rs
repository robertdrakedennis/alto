//! The right-click menu defaults (defaults archive group 7, the menu group):
//! input bindings, member/free colours and the modifier keys. The bindings
//! themselves belong to the input owner and are only consumed here.
use crate::{cache::Pack, ui_bytes::Cursor};
use anyhow::{Context, Result};
use rs910_core::fault::Fault;

/// Input bindings, by type byte 0/1/2: a mouse binding (button action, click
/// count, held keys), a key binding (key code + modifier mask) and a
/// held-keys binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    Mouse {
        action: i32,
        count: i32,
        keys: Vec<i32>,
    },
    /// Key code and modifier mask; a press matches on the key code and the
    /// modifier mask of the keyboard event, not the typed character.
    Key {
        code: i32,
        modifiers: i32,
    },
    KeyHeld {
        keys: Vec<i32>,
    },
}
impl Binding {
    /// Decode one binding; an unknown type consumes only its type byte.
    fn decode(c: &mut Cursor<'_>) -> Result<Option<Self>> {
        Ok(match c.g1()? {
            0 => {
                let action = i32::from(c.g1()?);
                let count = i32::from(c.g1()?);
                Some(Self::Mouse {
                    action,
                    count,
                    keys: key_list(c)?,
                })
            }
            1 => Some(Self::Key {
                code: i32::from(c.g1()?),
                modifiers: i32::from(c.g1()?),
            }),
            2 => Some(Self::KeyHeld { keys: key_list(c)? }),
            _ => None,
        })
    }
    /// Decode one binding, for the other `defaults` groups.
    pub fn decode_from(c: &mut Cursor<'_>) -> Result<Option<Self>> {
        Self::decode(c)
    }
    /// Whether the binding is satisfied. `held` tells whether a key is
    /// currently held; `presses` are this cycle's keyboard events as (key
    /// code, modifier mask).
    pub fn test(
        &self,
        event: Option<&MouseEvent>,
        presses: &[(i32, i32)],
        held: &dyn Fn(i32) -> bool,
    ) -> bool {
        match self {
            Self::Mouse {
                action,
                count,
                keys,
            } => match event {
                Some(e) => {
                    *action == e.action && *count <= e.count && keys.iter().all(|&k| held(k))
                }
                None => *action == -1,
            },
            // Key code and modifier mask must both match.
            Self::Key { code, modifiers } => {
                presses.iter().any(|(c, m)| *c == *code && *m == *modifiers)
            }
            Self::KeyHeld { keys } => keys.iter().all(|&k| held(k)),
        }
    }
}
/// One queued mouse press: button action (0 left / 1 middle / 2 right), click
/// position and click count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEvent {
    pub pos: [i32; 2],
    pub action: i32,
    pub count: i32,
}

fn key_list(c: &mut Cursor<'_>) -> Result<Vec<i32>> {
    let n = c.g1()?;
    (0..n).map(|_| c.g1().map(i32::from)).collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MiniMenuDefaults {
    /// Opcodes 1-4: `primaryBinding`, `secondaryBinding`, `menuBinding`, `selectBinding`.
    pub primary: Option<Binding>,
    pub secondary: Option<Binding>,
    pub menu: Option<Binding>,
    pub select: Option<Binding>,
    pub members_colour: i32,
    pub free_colour: i32,
    pub hover_cursor_override: bool,
    /// `menuModifierKey` (opcode 5): every listed key must be held for the
    /// menu modifier to count as held. `None` is an absent opcode, which
    /// never tests true, while a present empty list does.
    pub menu_modifier_key: Option<Vec<i32>>,
    /// `ctrlrunning` (opcode 6, `isCtrlKeyHeld`) and `shiftteleport` (opcode 7, `isShiftKeyHeld`).
    pub ctrlrunning: Option<Vec<i32>>,
    pub shiftteleport: Option<Vec<i32>>,
}

/// Whether a held-keys binding is satisfied: absent never is, present needs
/// every key held.
pub fn key_binding_held(binding: &Option<Vec<i32>>, held: impl Fn(i32) -> bool) -> bool {
    binding
        .as_ref()
        .is_some_and(|keys| keys.iter().all(|&k| held(k)))
}

impl MiniMenuDefaults {
    pub fn load(pack: &Pack) -> Result<Self> {
        let bytes = crate::js5_fetch::fetch_file(pack, "defaults", 7)?
            .with_context(|| Fault::MissingValue.message("defaults group 7 (menu) absent"))?;
        Self::decode(&bytes)
    }
    /// Decode the menu defaults file.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut d = Self::default();
        let mut c = Cursor::new(bytes);
        loop {
            match c.g1()? {
                0 => return Ok(d),
                1 => d.primary = Binding::decode(&mut c)?,
                2 => d.secondary = Binding::decode(&mut c)?,
                3 => d.menu = Binding::decode(&mut c)?,
                4 => d.select = Binding::decode(&mut c)?,
                8..=10 => {
                    Binding::decode(&mut c)?;
                }
                5 => d.menu_modifier_key = Some(key_list(&mut c)?),
                6 => d.ctrlrunning = Some(key_list(&mut c)?),
                7 => d.shiftteleport = Some(key_list(&mut c)?),
                11 => d.hover_cursor_override = true,
                12 => d.members_colour = c.g4s()?,
                13 => d.free_colour = c.g4s()?,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No mouse binding of the cache's menu defaults asks for more than a
    /// single click, so a window system that reports no click count (every
    /// press counts as one) loses none of them.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn no_cache_mouse_binding_needs_a_double_click() {
        let pack = crate::test_support::require_pack("client.config.js5");
        let defaults = MiniMenuDefaults::load(&pack).expect("the menu defaults");
        let mut mouse = 0;
        for binding in [
            &defaults.primary,
            &defaults.secondary,
            &defaults.menu,
            &defaults.select,
        ]
        .into_iter()
        .flatten()
        {
            if let Binding::Mouse { count, .. } = binding {
                mouse += 1;
                assert!(*count <= 1, "{binding:?} needs {count} clicks");
            }
        }
        assert!(mouse > 0, "the defaults hold mouse bindings");
    }

    #[test]
    fn colours_survive_binding_skips() {
        let bytes = [
            1, 0, 3, 1, 1, 42, 2, 1, 5, 6, 5, 2, 7, 8, 11, 12, 0, 0xFF, 0, 0, 13, 0, 0, 0x12, 0x34,
            0,
        ];
        let d = MiniMenuDefaults::decode(&bytes).unwrap();
        assert_eq!(
            d,
            MiniMenuDefaults {
                primary: Some(Binding::Mouse {
                    action: 3,
                    count: 1,
                    keys: vec![42]
                }),
                secondary: Some(Binding::Key {
                    code: 5,
                    modifiers: 6
                }),
                menu: None,
                select: None,
                members_colour: 0x00FF_0000,
                free_colour: 0x1234,
                hover_cursor_override: true,
                menu_modifier_key: Some(vec![7, 8]),
                ctrlrunning: None,
                shiftteleport: None,
            }
        );
        let held = |k: i32| k == 42;
        let event = MouseEvent {
            pos: [0, 0],
            action: 3,
            count: 1,
        };
        assert!(d.primary.as_ref().unwrap().test(Some(&event), &[], &held));
        assert!(!d.primary.as_ref().unwrap().test(None, &[], &held));
        assert!(!d.primary.as_ref().unwrap().test(
            Some(&MouseEvent { action: 0, ..event }),
            &[],
            &held
        ));
        // A key binding compares the key code and the modifier mask, not the
        // typed character.
        let key = d.secondary.as_ref().unwrap();
        assert!(key.test(None, &[(5, 6)], &held));
        assert!(!key.test(None, &[(5, 0)], &held));
        assert!(!key.test(None, &[(6, 6)], &held));
    }
    /// An absent binding is never held, so an absent `menuModifierKey` never
    /// swaps the left-click option; a held-keys binding needs every key held.
    #[test]
    fn key_bindings_hold_only_when_present_and_every_key_is_held() {
        assert!(!key_binding_held(&None, |_| true));
        assert!(key_binding_held(&Some(vec![]), |_| false));
        assert!(key_binding_held(&Some(vec![82]), |k| k == 82));
        assert!(!key_binding_held(&Some(vec![82, 81]), |k| k == 82));
    }

    /// The real cache MENU defaults carry ctrl/shift bindings but no opcode
    /// 5, so the menu modifier is never held.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn absent_key_bindings_are_never_held() {
        let pack = crate::cache::Pack::open(rs910_core::test_support::pack_root());
        let d = MiniMenuDefaults::load(&pack).unwrap();
        assert_eq!(d.menu_modifier_key, None);
        assert!(!key_binding_held(&d.menu_modifier_key, |_| true));
        assert_eq!(d.ctrlrunning, Some(vec![82]));
        assert_eq!(d.shiftteleport, Some(vec![82, 81]));
    }
}
