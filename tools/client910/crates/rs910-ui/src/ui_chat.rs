//! The chat history: lines bounded per chat type (100 entries), addressed by
//! a global uid or by (chat type, line index, newest first).
//!
//! The uid is global, wraps as a 32-bit int and resets on clear. The caller
//! may supply a clock for deterministic replay; production code can use
//! `with_clock` with its monotonic clock adapter.
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatLine {
    pub uid: i32,
    pub time: i64,
    pub chat_type: i32,
    pub flags: i32,
    pub name: String,
    pub name_unfiltered: String,
    pub name_simple: String,
    pub clan: Option<String>,
    pub phrase: i32,
    pub message: String,
    pub crown: Option<i32>,
}

/// A message to record in the [`ChatHistory`]; the history assigns the uid and
/// the time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewChatLine {
    pub chat_type: i32,
    pub flags: i32,
    pub name: String,
    pub name_unfiltered: String,
    pub name_simple: String,
    pub clan: Option<String>,
    pub phrase: i32,
    pub message: String,
    pub crown: Option<i32>,
}

impl NewChatLine {
    /// A line with no sender: no names, no clan, phrase -1, no crown.
    pub fn system(chat_type: i32, message: impl Into<String>) -> Self {
        Self {
            chat_type,
            flags: 0,
            name: String::new(),
            name_unfiltered: String::new(),
            name_simple: String::new(),
            clan: None,
            phrase: -1,
            message: message.into(),
            crown: None,
        }
    }

    /// A line from `sender`, who fills all three name fields.
    pub fn from_sender(
        chat_type: i32,
        sender: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let sender = sender.into();
        Self {
            name: sender.clone(),
            name_unfiltered: sender.clone(),
            name_simple: sender,
            ..Self::system(chat_type, message)
        }
    }
}

type Clock = Box<dyn Fn() -> i64 + Send + Sync>;

pub struct ChatHistory {
    next_uid: i32,
    by_uid: HashMap<i32, ChatLine>,
    by_type: HashMap<i32, VecDeque<i32>>, // newest first, original index 0 first
    ordered: VecDeque<i32>,               // oldest first, global secondary list
    clock: Clock,
}

impl Default for ChatHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatHistory {
    pub fn new() -> Self {
        Self::with_clock(crate::logic_clock::monotonic_millis)
    }

    pub fn with_clock(f: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        Self {
            next_uid: 0,
            by_uid: HashMap::new(),
            by_type: HashMap::new(),
            ordered: VecDeque::new(),
            clock: Box::new(f),
        }
    }
    pub fn next_uid(&mut self) -> i32 {
        let uid = self.next_uid;
        self.next_uid = self.next_uid.wrapping_add(1);
        uid
    }
    pub fn last_uid(&self) -> i32 {
        self.next_uid.wrapping_sub(1)
    }
    pub fn clear(&mut self) {
        self.next_uid = 0;
        self.by_uid.clear();
        self.by_type.clear();
        self.ordered.clear();
    }

    pub fn add_system_message(&mut self, chat_type: i32, message: impl Into<String>) -> i32 {
        self.add_message(NewChatLine::system(chat_type, message))
    }
    pub fn mes(&mut self, message: impl Into<String>) -> i32 {
        self.add_system_message(0, message)
    }
    /// Records a line and returns its uid.
    pub fn add_message(&mut self, new: NewChatLine) -> i32 {
        let NewChatLine {
            chat_type,
            flags,
            name,
            name_unfiltered,
            name_simple,
            clan,
            phrase,
            message,
            crown,
        } = new;
        let uid = self.next_uid();
        let line = ChatLine {
            uid,
            time: (self.clock)(),
            chat_type,
            flags,
            name,
            name_unfiltered,
            name_simple,
            clan,
            phrase,
            message,
            crown,
        };
        let typed = self.by_type.entry(chat_type).or_default();
        if typed.len() == 100 {
            if let Some(old) = typed.pop_back() {
                self.by_uid.remove(&old);
                self.ordered.retain(|x| *x != old);
            }
        }
        typed.push_front(uid);
        self.ordered.push_back(uid);
        self.by_uid.insert(uid, line);
        uid
    }
    pub fn get_by_uid(&self, uid: i32) -> Option<&ChatLine> {
        self.by_uid.get(&uid)
    }
    pub fn get_by_type_and_line(&self, chat_type: i32, line: i32) -> Option<&ChatLine> {
        if line < 0 {
            return None;
        }
        self.by_type
            .get(&chat_type)?
            .get(line as usize)
            .and_then(|uid| self.by_uid.get(uid))
    }
    pub fn has_type(&self, chat_type: i32) -> bool {
        self.by_type.contains_key(&chat_type)
    }
    pub fn length(&self, chat_type: i32) -> usize {
        self.by_type.get(&chat_type).map_or(0, VecDeque::len)
    }
    /// `previousUid`: older global message.
    pub fn previous_uid(&self, uid: i32) -> i32 {
        self.neighbor(uid, -1)
    }
    /// `nextUid`: newer global message.
    pub fn next_uid_of(&self, uid: i32) -> i32 {
        self.neighbor(uid, 1)
    }
    fn neighbor(&self, uid: i32, direction: i32) -> i32 {
        let Some(pos) = self.ordered.iter().position(|x| *x == uid) else {
            return -1;
        };
        let p = pos as i32 + direction;
        if p < 0 {
            -1
        } else {
            self.ordered.get(p as usize).copied().unwrap_or(-1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uids_global_and_per_type_is_newest_first() {
        let mut h = ChatHistory::with_clock(|| 42);
        assert_eq!(h.mes("one"), 0);
        assert_eq!(h.add_system_message(1, "two"), 1);
        assert_eq!(h.mes("three"), 2);
        assert_eq!(h.get_by_type_and_line(0, 0).unwrap().message, "three");
        assert_eq!(h.get_by_type_and_line(0, 1).unwrap().message, "one");
        assert_eq!(h.previous_uid(2), 1);
        assert_eq!(h.next_uid_of(0), 1);
        assert_eq!(h.previous_uid(0), -1);
        assert_eq!(h.next_uid_of(2), -1);
        assert_eq!(h.get_by_uid(1).unwrap().time, 42);
    }
    #[test]
    fn type_capacity_recycles_oldest_but_uid_never_reuses() {
        let mut h = ChatHistory::with_clock(|| 0);
        for i in 0..101 {
            h.add_system_message(7, i.to_string());
        }
        assert_eq!(h.length(7), 100);
        assert!(h.get_by_uid(0).is_none());
        assert_eq!(h.last_uid(), 100);
        assert_eq!(h.get_by_type_and_line(7, 99).unwrap().uid, 1);
        assert_eq!(h.previous_uid(1), -1);
    }
}

#[cfg(test)]
#[path = "ui_chat_oracle.rs"]
mod ui_chat_oracle;
