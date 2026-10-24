//! Overhead chat lines of players and NPCs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chat {
    pub text: Option<String>,
    pub colour: i32,
    pub effect: i32,
    pub total: i32,
    pub time: i32,
}
impl Chat {
    pub fn tick(&mut self) {
        if self.text.is_some() {
            self.time = self.time.wrapping_sub(1);
            if self.time == 0 {
                self.text = None
            }
        }
    }
}
/// A request to add a message to the chat history; caller drains these after installing state.
/// UI timestamps/UID allocation stay in the future chat-history adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRequest {
    pub flags: u8,
    pub text: String,
    pub name: Option<String>,
    pub title: Option<String>,
}
