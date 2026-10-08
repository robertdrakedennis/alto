//! Accepted retained input at a platform event boundary, shared by live and replay.
use super::session_core::{retained_key, retained_mouse_button, retained_mouse_move, Session};
use serde::{Deserialize, Serialize};

pub const RECORD_TAG: &[u8; 4] = b"UIEV";
const MAX_RECORD_BYTES: usize = 64 * 1024;
const MAX_TEXT_BYTES: usize = 4096;
pub const LEFT_BUTTON: i32 = 0;
pub const MIDDLE_BUTTON: i32 = 1;
pub const RIGHT_BUTTON: i32 = 2;

/// An already resolved native minimenu choice, with its full observable identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuChoice {
    pub op: String,
    pub target: Option<String>,
    pub cursor: i32,
    pub action: i32,
    pub obj_id: i32,
    pub entity_id: i64,
    pub tile_x: i32,
    pub tile_z: i32,
    pub enabled: bool,
    pub has_arrow: bool,
    pub sub_id: i64,
    pub force_submenu: bool,
    pub detail: Option<String>,
}

impl From<&rs910_ui::ui_minimenu::Entry> for MenuChoice {
    fn from(entry: &rs910_ui::ui_minimenu::Entry) -> Self {
        Self {
            op: entry.op.clone(),
            target: entry.target.clone(),
            cursor: entry.cursor,
            action: entry.action,
            obj_id: entry.obj_id,
            entity_id: entry.entity_id,
            tile_x: entry.tile_x,
            tile_z: entry.tile_z,
            enabled: entry.enabled,
            has_arrow: entry.has_arrow,
            sub_id: entry.sub_id,
            force_submenu: entry.force_submenu,
            detail: entry.detail.clone(),
        }
    }
}

impl MenuChoice {
    fn entry(&self) -> rs910_ui::ui_minimenu::Entry {
        rs910_ui::ui_minimenu::Entry {
            op: self.op.clone(),
            target: self.target.clone(),
            cursor: self.cursor,
            action: self.action,
            obj_id: self.obj_id,
            entity_id: self.entity_id,
            tile_x: self.tile_x,
            tile_z: self.tile_z,
            enabled: self.enabled,
            has_arrow: self.has_arrow,
            sub_id: self.sub_id,
            force_submenu: self.force_submenu,
            detail: self.detail.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputEvent {
    Key {
        code: i32,
        pressed: bool,
        text: Option<String>,
        time: i64,
    },
    Button {
        action: i32,
        pressed: bool,
        time: i64,
    },
    Move {
        position: [i32; 2],
        time: i64,
    },
    Wheel {
        delta: i32,
    },
    Component {
        parent: i32,
        child: i32,
        operation: i32,
    },
    Menu {
        choice: MenuChoice,
        position: [i32; 2],
        from_menu: bool,
    },
}

impl InputEvent {
    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        anyhow::ensure!(bytes.len() <= MAX_RECORD_BYTES, "input record too large");
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        anyhow::ensure!(bytes.len() <= MAX_RECORD_BYTES, "input record too large");
        let event: Self = serde_json::from_slice(bytes)?;
        event.validate()?;
        Ok(event)
    }

    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Button { action, .. } => anyhow::ensure!(
                (LEFT_BUTTON..=RIGHT_BUTTON).contains(action),
                "unknown pointer button"
            ),
            Self::Key {
                text: Some(text), ..
            } => anyhow::ensure!(text.len() <= MAX_TEXT_BYTES, "typed input too large"),
            Self::Component { operation, .. } => {
                anyhow::ensure!(*operation >= 0, "negative component operation")
            }
            _ => {}
        }
        Ok(())
    }

    /// Apply only through the same retained owners that the native event loop uses.
    pub fn apply(&self, session: &mut Session) -> anyhow::Result<()> {
        self.validate()?;
        let ui = &mut session.ui;
        match self {
            Self::Key {
                code,
                pressed,
                text,
                time,
            } => retained_key(ui, *code, *pressed, text.as_deref(), *time),
            Self::Button {
                action,
                pressed,
                time,
            } => retained_mouse_button(ui, *action, *pressed, *time),
            Self::Move { position, time } => retained_mouse_move(ui, *position, *time),
            Self::Wheel { delta } => ui.input.wheel = ui.input.wheel.wrapping_add(*delta),
            Self::Component {
                parent,
                child,
                operation,
            } => {
                let action = if *operation == 0 {
                    rs910_ui::ui_interaction::Action::Pause {
                        parent: *parent,
                        child: *child,
                    }
                } else {
                    rs910_ui::ui_interaction::Action::Op {
                        parent: *parent,
                        child: *child,
                        op: *operation,
                        base: None,
                    }
                };
                ui.state.interaction.actions.push_back(action);
            }
            Self::Menu {
                choice,
                position,
                from_menu,
            } => ui.use_menu_option(&choice.entry(), position[0], position[1], *from_menu),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_input_cannot_reach_pointer_or_text_owners() {
        assert!(
            InputEvent::decode(br#"{"kind":"button","action":3,"pressed":true,"time":1}"#).is_err()
        );
        assert!(InputEvent::decode(br#"{"kind":"move","position":[1],"time":1}"#).is_err());
        assert!(InputEvent::decode(br#"{"kind":"wheel","delta":1,"extra":true}"#).is_err());
        assert!(InputEvent::decode(&vec![b' '; MAX_RECORD_BYTES + 1]).is_err());
        let oversized = InputEvent::Key {
            code: 0,
            pressed: true,
            text: Some("x".repeat(MAX_TEXT_BYTES + 1)),
            time: 1,
        };
        assert!(oversized.encode().is_err());
    }

    #[test]
    fn input_record_preserves_unicode_timestamp_and_release_order() {
        let events = [
            InputEvent::Move {
                position: [-1, 2],
                time: 1234,
            },
            InputEvent::Button {
                action: RIGHT_BUTTON,
                pressed: true,
                time: 1235,
            },
            InputEvent::Key {
                code: 0,
                pressed: true,
                text: Some("é🦀".into()),
                time: 1236,
            },
            InputEvent::Key {
                code: 0,
                pressed: false,
                text: None,
                time: 1237,
            },
            InputEvent::Button {
                action: RIGHT_BUTTON,
                pressed: false,
                time: 1238,
            },
        ];
        let decoded: Vec<_> = events
            .iter()
            .map(|e| InputEvent::decode(&e.encode().unwrap()).unwrap())
            .collect();
        assert_eq!(decoded, events);
    }
}
