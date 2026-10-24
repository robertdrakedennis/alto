//! Overhead chat masks of the player and NPC info packets.
use super::{Error, Packet, Result};
use crate::entities910::{
    chat::{Chat, HistoryRequest},
    Player,
};
pub(super) fn read(
    p: &mut Packet,
    e: &mut Player,
    timeout: Option<i32>,
    flagged: bool,
    local: bool,
    npc: bool,
) -> Result<()> {
    let ttl = timeout.ok_or(Error::UnsupportedContext("chat timeout in logic ticks"))?;
    let text = p.string()?;
    let flags = if flagged { p.byte()? as u8 } else { 0 };
    if !npc && (if flagged { flags & 1 != 0 } else { local }) {
        e.chat_history.push(HistoryRequest {
            flags,
            text: text.clone(),
            name: e.appearance.name.clone(),
            title: e.appearance.title.clone(),
        });
    }
    e.chat = Some(Chat {
        text: Some(text),
        colour: 0,
        effect: 0,
        total: ttl,
        time: ttl,
    });
    Ok(())
}
