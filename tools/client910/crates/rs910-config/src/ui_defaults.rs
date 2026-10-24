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
