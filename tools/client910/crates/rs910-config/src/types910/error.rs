//! The entity/packet decode error, shared by the `*_types` config decoders
//! here and by `rs910_game`'s entity state and appliers (`entities910::Error`
//! and `protocol910::Error` re-export it).
/// Entity/packet decode error shared by the entity state, the
/// `protocol910` appliers and the config decoders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    UnsupportedConfig {
        kind: &'static str,
        id: i32,
        opcode: u8,
    },
    Truncated {
        bit: usize,
    },
    Invalid(&'static str),
    UnsupportedMask {
        entity_index: usize,
        mask: u32,
    },
    UnsupportedContext(&'static str),
    UnsupportedZone(u8),
}
