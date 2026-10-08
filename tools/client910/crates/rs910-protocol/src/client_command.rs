//! Remote-command wire writer for the current no-ISAAC connection profile.
//! Local command dispatch and console/interface presentation are separate owners.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("command contains a NUL character")]
    Nul,
    #[error("command exceeds the command buffer")]
    Overflow,
}
/// Windows-1252 encoding per UTF-16 unit, including each surrogate.
/// rs910-core's copy since Phase 2.1 (this module's was identical on all
/// 65536 units).
pub use rs910_core::cp1252::cp1252_encode_unit as byte;
pub fn remote(command: &str, scripted: bool, suggest: bool) -> Result<Vec<u8>, Error> {
    let units: Vec<_> = command.encode_utf16().collect();
    remote_units(&units, scripted, suggest)
}
/// Console editing uses UTF-16 indices and can retain a lone surrogate after
/// deleting half of a pasted pair. Each remaining unit is still encoded.
pub fn remote_units(units: &[u16], scripted: bool, suggest: bool) -> Result<Vec<u8>, Error> {
    if units.contains(&0) {
        return Err(Error::Nul);
    }
    // The message buffer is 260 bytes: opcode, size, two flags, string, NUL.
    if units.len() > 255 {
        return Err(Error::Overflow);
    }
    let mut packet = Vec::with_capacity(units.len() + 5);
    packet.extend([
        crate::proto::client::CLIENT_CHEAT,
        (units.len() + 3) as u8,
        scripted as u8,
        suggest as u8,
    ]);
    packet.extend(units.iter().copied().map(byte));
    packet.push(0);
    Ok(packet)
}
