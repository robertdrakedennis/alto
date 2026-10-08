//! Classes of runtime fault a script or a data decoder can hit.
//!
//! The 910 game logic fails hard on a handful of conditions (an absent
//! object, an index outside an array, a value of the wrong type, a
//! division by zero, a negative array size, and a generic invalid state).
//! Decoders and script hosts in this workspace keep those failures instead
//! of papering over them, and name the class in the message as
//! `"<label>: <detail>"`. [`Fault::in_message`] recovers the class from such
//! a message, so a conformance harness can compare failure classes without
//! matching prose.

use std::fmt;

/// The class of a runtime fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fault {
    /// A value that must exist is absent.
    MissingValue,
    /// An array or list index outside its bounds.
    IndexOutOfRange,
    /// A value of another type than the operation requires.
    WrongValueType,
    /// An integer division or remainder by zero.
    DivisionByZero,
    /// An array created with a negative size.
    NegativeSize,
    /// The operation is not valid in the current state.
    InvalidState,
}

impl Fault {
    /// Every class, for scanning a message.
    pub const ALL: [Fault; 6] = [
        Fault::MissingValue,
        Fault::IndexOutOfRange,
        Fault::WrongValueType,
        Fault::DivisionByZero,
        Fault::NegativeSize,
        Fault::InvalidState,
    ];

    /// The message prefix naming this class.
    pub const fn label(self) -> &'static str {
        match self {
            Fault::MissingValue => "missing value",
            Fault::IndexOutOfRange => "index out of range",
            Fault::WrongValueType => "wrong value type",
            Fault::DivisionByZero => "division by zero",
            Fault::NegativeSize => "negative size",
            Fault::InvalidState => "invalid state",
        }
    }

    /// The message `"<label>: <detail>"`.
    pub fn message(self, detail: impl fmt::Display) -> String {
        format!("{}: {detail}", self.label())
    }

    /// The class a message names: the earliest label found in `text` as a
    /// prefix (`"<label>: ..."`) or as the whole message.
    pub fn in_message(text: &str) -> Option<Fault> {
        Fault::ALL
            .into_iter()
            .filter_map(|fault| {
                let label = fault.label();
                text.match_indices(label)
                    .find(|(at, _)| {
                        let rest = &text[at + label.len()..];
                        rest.is_empty() || rest.starts_with(':')
                    })
                    .map(|(at, _)| (at, fault))
            })
            .min_by_key(|(at, _)| *at)
            .map(|(_, fault)| fault)
    }
}

#[cfg(test)]
mod tests {
    use super::Fault;

    #[test]
    fn a_class_is_recovered_from_its_message() {
        for fault in Fault::ALL {
            let text = format!("trap: {}", fault.message("entities[3]"));
            assert_eq!(Fault::in_message(&text), Some(fault));
        }
        assert_eq!(Fault::in_message("connection reset"), None);
    }
}
