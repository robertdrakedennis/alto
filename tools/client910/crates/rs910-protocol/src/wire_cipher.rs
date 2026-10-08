//! The game stream's opcode masking, applied below the framing: the two
//! directions of one connection's ISAAC state.
//!
//! The consumers of the stream see the plain framing ([`crate::framing`]); the
//! masking is added to client frames on their way out ([`OutboundCipher`]) and
//! removed from server frames as they arrive ([`InboundCipher`]), by walking
//! the frame boundaries with the opcode size tables.
//!
//! - Client frame: the opcode byte plus one generator byte, then the size
//!   prefix and payload unchanged.
//! - Server frame: the opcode is one byte below 128 and two bytes from 128 up
//!   (the high byte carries 128 plus the opcode's high part), each plus a
//!   generator byte. The size prefix and payload are unchanged, except that
//!   the payload bytes of the page-opening frames (`URL_OPEN` after its flag
//!   byte, `SOCIAL_NETWORK_LOGOUT` whole) are also masked, one generator byte
//!   each.

use anyhow::{bail, Result};

use crate::isaac_cipher::IsaacCipher;
use crate::proto;
use crate::tiny_cipher;

/// The four seeds a login block carries.
pub type SessionSeeds = [i32; 4];

/// Client request frames whose payload an account-creation connection also
/// protects with the tiny cipher (keyed with the connection's seeds).
const ACCOUNT_CREATION_REQUESTS: [u8; 3] = [
    proto::client::CREATE_CHECK_EMAIL,
    proto::client::CREATE_CHECK_NAME,
    proto::client::CREATE_ACCOUNT,
];

/// Adds the masking to client frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboundCipher {
    isaac: IsaacCipher,
    /// The seeds, on an account-creation connection.
    request_key: Option<SessionSeeds>,
    /// The bytes of a frame that is not complete yet.
    held: Vec<u8>,
}

impl OutboundCipher {
    #[must_use]
    pub fn new(seeds: SessionSeeds) -> Self {
        Self {
            isaac: IsaacCipher::from_seeds(seeds),
            request_key: None,
            held: Vec::new(),
        }
    }

    /// Also protect the account-creation request payloads with the seeds.
    #[must_use]
    pub fn with_request_key(mut self, seeds: SessionSeeds) -> Self {
        self.request_key = Some(seeds);
        self
    }

    /// Bytes of an unfinished frame held back.
    #[must_use]
    pub fn held_len(&self) -> usize {
        self.held.len()
    }

    /// Append the wire form of the plain client bytes `plain` to `wire`. A
    /// trailing incomplete frame is held until its remaining bytes arrive.
    /// A frame of an opcode the table does not know is an error (its length
    /// cannot be known).
    pub fn seal(&mut self, plain: &[u8], wire: &mut Vec<u8>) -> Result<()> {
        self.held.extend_from_slice(plain);
        let mut at = 0;
        loop {
            let rest = &self.held[at..];
            let Some(&opcode) = rest.first() else { break };
            let Some(size) = proto::client::size(opcode) else {
                bail!("unknown client opcode {opcode}");
            };
            let (prefix, payload) = match size {
                -1 => match rest.get(1) {
                    Some(&len) => (1, usize::from(len)),
                    None => break,
                },
                -2 => match rest.get(1..3) {
                    Some(len) => (2, usize::from(u16::from_be_bytes([len[0], len[1]]))),
                    None => break,
                },
                fixed => (0, usize::try_from(fixed).unwrap_or(0)),
            };
            let total = 1 + prefix + payload;
            if rest.len() < total {
                break;
            }
            let start = wire.len();
            wire.extend_from_slice(&rest[..total]);
            wire[start] = opcode.wrapping_add(self.isaac.next_byte());
            if let (Some(key), true) = (
                self.request_key.as_ref(),
                ACCOUNT_CREATION_REQUESTS.contains(&opcode),
            ) {
                let from = start + 1 + prefix;
                let to = wire.len();
                tiny_cipher::encrypt_range(wire, from, to, key);
            }
            at += total;
        }
        self.held.drain(..at);
        Ok(())
    }
}

/// Where the inbound decoder is within a server frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Position {
    /// The first opcode byte.
    Opcode,
    /// The low opcode byte; `high` is the unmasked first byte (128 or more).
    OpcodeLow { high: u8 },
    /// The size prefix byte(s) of `opcode`.
    Size {
        opcode: u8,
        bytes_left: u8,
        size: u16,
    },
    /// `left` payload bytes of `opcode` follow; `seen` have passed.
    Payload { opcode: u8, left: u32, seen: u32 },
}

/// Removes the masking from server frames, in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboundCipher {
    isaac: IsaacCipher,
    position: Position,
}

impl InboundCipher {
    #[must_use]
    pub fn new(seeds: SessionSeeds) -> Self {
        Self {
            isaac: IsaacCipher::inbound_from_seeds(seeds),
            position: Position::Opcode,
        }
    }

    /// Unmask the next `bytes` of the stream in place; the plain framing
    /// results ([`crate::framing::decode_frame_at`] reads it). An opcode the
    /// table does not know is an error: the frame length is lost with it.
    pub fn open(&mut self, bytes: &mut [u8]) -> Result<()> {
        for byte in bytes {
            self.position = match self.position {
                Position::Opcode => {
                    let first = byte.wrapping_sub(self.isaac.next_byte());
                    *byte = first;
                    if first < 128 {
                        Self::after_opcode(first)?
                    } else {
                        Position::OpcodeLow { high: first }
                    }
                }
                Position::OpcodeLow { high } => {
                    let low = byte.wrapping_sub(self.isaac.next_byte());
                    *byte = low;
                    let opcode = (u16::from(high) - 128) * 256 + u16::from(low);
                    match u8::try_from(opcode) {
                        Ok(opcode) => Self::after_opcode(opcode)?,
                        Err(_) => bail!("unknown server opcode {opcode}"),
                    }
                }
                Position::Size {
                    opcode,
                    bytes_left,
                    size,
                } => {
                    let size = (size << 8) | u16::from(*byte);
                    if bytes_left > 1 {
                        Position::Size {
                            opcode,
                            bytes_left: bytes_left - 1,
                            size,
                        }
                    } else {
                        Self::payload_or_next(opcode, u32::from(size))
                    }
                }
                Position::Payload { opcode, left, seen } => {
                    if Self::masked_payload_byte(opcode, seen) {
                        *byte = byte.wrapping_sub(self.isaac.next_byte());
                    }
                    if left > 1 {
                        Position::Payload {
                            opcode,
                            left: left - 1,
                            seen: seen + 1,
                        }
                    } else {
                        Position::Opcode
                    }
                }
            };
        }
        Ok(())
    }

    fn after_opcode(opcode: u8) -> Result<Position> {
        match proto::server::size(opcode) {
            None => bail!("unknown server opcode {opcode}"),
            Some(-1) => Ok(Position::Size {
                opcode,
                bytes_left: 1,
                size: 0,
            }),
            Some(-2) => Ok(Position::Size {
                opcode,
                bytes_left: 2,
                size: 0,
            }),
            Some(fixed) => Ok(Self::payload_or_next(
                opcode,
                u32::try_from(fixed).unwrap_or(0),
            )),
        }
    }

    fn payload_or_next(opcode: u8, length: u32) -> Position {
        if length == 0 {
            Position::Opcode
        } else {
            Position::Payload {
                opcode,
                left: length,
                seen: 0,
            }
        }
    }

    /// Whether payload byte `index` of `opcode` carries a generator byte.
    fn masked_payload_byte(opcode: u8, index: u32) -> bool {
        match opcode {
            proto::server::URL_OPEN => index > 0,
            proto::server::SOCIAL_NETWORK_LOGOUT => true,
            _ => false,
        }
    }

    /// Unmask `bytes` that are masked outside any frame (the payload of a
    /// login reply that carries a page address, one generator byte each).
    pub fn unmask_block(&mut self, bytes: &mut [u8]) {
        for byte in bytes {
            *byte = byte.wrapping_sub(self.isaac.next_byte());
        }
    }

    /// One byte masked outside any frame, unmasked.
    pub fn unmask_byte(&mut self, byte: u8) -> u8 {
        byte.wrapping_sub(self.isaac.next_byte())
    }

    /// Whether the decoder sits on a frame boundary.
    #[must_use]
    pub fn at_frame_boundary(&self) -> bool {
        self.position == Position::Opcode
    }
}

/// Both directions of one connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireCipher {
    pub inbound: InboundCipher,
    pub outbound: OutboundCipher,
}

impl WireCipher {
    /// The ciphers of a connection whose login block carried `seeds`.
    #[must_use]
    pub fn new(seeds: SessionSeeds) -> Self {
        Self {
            inbound: InboundCipher::new(seeds),
            outbound: OutboundCipher::new(seeds),
        }
    }

    /// The same, for an account-creation connection: its request payloads
    /// are also protected with the seeds.
    #[must_use]
    pub fn for_account_creation(seeds: SessionSeeds) -> Self {
        Self {
            inbound: InboundCipher::new(seeds),
            outbound: OutboundCipher::new(seeds).with_request_key(seeds),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::decode_frame;

    const SEEDS: SessionSeeds = [
        0x1234_5678,
        0x9abc_def0u32 as i32,
        0xf1e2_d3c4u32 as i32,
        0x7fff_ffff,
    ];

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn recorded_line(tag: &str) -> Vec<String> {
        include_str!("../../../fixtures/recorded/login-crypto/primitives.txt")
            .lines()
            .find(|l| l.starts_with(tag))
            .unwrap()
            .split(' ')
            .map(str::to_owned)
            .collect()
    }

    /// Client frames written with the seeds carry the opcode bytes the
    /// original client writes (the recorded opcodes are fixed-size frames
    /// with zero payloads here); everything else is unchanged.
    #[test]
    fn outbound_opcodes_match_the_original_client() {
        let line = recorded_line("client-opcodes ");
        let opcodes: Vec<u8> = line[2]
            .trim_matches(|c| c == '[' || c == ']')
            .split(',')
            .map(|n| n.parse().unwrap())
            .collect();
        let mut plain = Vec::new();
        let mut at = Vec::new();
        for &opcode in &opcodes {
            at.push(plain.len());
            plain.push(opcode);
            let size = proto::client::size(opcode).unwrap();
            assert!(size >= 0, "the recording uses fixed-size opcodes");
            plain.extend(std::iter::repeat_n(0, size as usize));
        }
        let mut wire = Vec::new();
        OutboundCipher::new(SEEDS).seal(&plain, &mut wire).unwrap();
        let masked: Vec<u8> = at.iter().map(|&i| wire[i]).collect();
        assert_eq!(masked, unhex(&line[3]));
        let unchanged = |from: &[u8]| {
            from.iter()
                .enumerate()
                .filter(|(i, _)| !at.contains(i))
                .map(|(_, b)| *b)
                .collect::<Vec<u8>>()
        };
        assert_eq!(unchanged(&wire), unchanged(&plain));
    }

    /// The server's opcodes, one and two bytes wide, unmask as the original
    /// client reads them, whether the stream arrives whole or byte by byte,
    /// and the plain framing then decodes them.
    #[test]
    fn inbound_opcodes_match_the_original_client() {
        let line = recorded_line("server-opcodes ");
        let wire_opcodes = unhex(&line[2]);
        let expected: Vec<(u16, bool)> = line[3]
            .split(',')
            .map(|item| {
                let (op, width) = item.split_once('/').unwrap();
                (op.parse().unwrap(), width == "2")
            })
            .collect();
        let mut stream = Vec::new();
        let mut cursor = 0;
        for &(opcode, two_bytes) in &expected {
            let width = if two_bytes { 2 } else { 1 };
            stream.extend_from_slice(&wire_opcodes[cursor..cursor + width]);
            cursor += width;
            let size = proto::server::size(opcode as u8).unwrap();
            // A zero length prefix and a fixed payload of zero bytes.
            match size {
                -1 => stream.push(0),
                -2 => stream.extend([0, 0]),
                fixed => stream.extend(std::iter::repeat_n(0, fixed as usize)),
            }
        }
        let decode = |opened: &[u8]| {
            let mut frames = Vec::new();
            let mut rest = opened;
            while let Some((frame, used)) = decode_frame(rest).unwrap() {
                frames.push(u16::from(frame.opcode));
                rest = &rest[used..];
            }
            assert!(rest.is_empty());
            frames
        };
        let want: Vec<u16> = expected.iter().map(|&(op, _)| op).collect();
        let mut whole = stream.clone();
        InboundCipher::new(SEEDS).open(&mut whole).unwrap();
        assert_eq!(decode(&whole), want);
        let mut trickled = stream;
        let mut cipher = InboundCipher::new(SEEDS);
        for byte in trickled.chunks_mut(1) {
            cipher.open(byte).unwrap();
        }
        assert_eq!(trickled, whole);
        assert!(cipher.at_frame_boundary());
    }

    /// The payload of the page-opening frames is masked byte by byte, after
    /// `URL_OPEN`'s flag byte and from the first byte of
    /// `SOCIAL_NETWORK_LOGOUT`; other frames keep their payload.
    #[test]
    fn page_frames_unmask_their_payload() {
        let line = recorded_line("payload ");
        let masked = unhex(&line[2]);
        let plain = unhex(&line[3]);
        let seeds_plus_50 = [
            SEEDS[0] + 50,
            SEEDS[1] + 50,
            SEEDS[2] + 50,
            SEEDS[3].wrapping_add(50),
        ];
        assert_eq!(
            line[1],
            seeds_plus_50.map(|s| format!("{:08x}", s as u32)).join(","),
            "the recording masks with the inbound generator"
        );
        let mut stream = Vec::new();
        // SOCIAL_NETWORK_LOGOUT: u8-or-u16 size, then the masked text. Its
        // opcode byte is masked with the generator's first value, so mask the
        // whole frame with one generator the way the server would.
        let mut generator = IsaacCipher::inbound_from_seeds(SEEDS);
        let opcode = proto::server::SOCIAL_NETWORK_LOGOUT;
        assert!(opcode < 128);
        stream.push(opcode.wrapping_add(generator.next_byte()));
        match proto::server::size(opcode).unwrap() {
            -1 => stream.push(plain.len() as u8),
            -2 => stream.extend((plain.len() as u16).to_be_bytes()),
            _ => unreachable!("a page address has a variable size"),
        }
        for byte in &plain {
            stream.push(byte.wrapping_add(generator.next_byte()));
        }
        InboundCipher::new(SEEDS).open(&mut stream).unwrap();
        let (frame, _) = decode_frame(&stream).unwrap().unwrap();
        assert_eq!(frame.payload, plain);
        // The recorded masking (the generator's values from the first on)
        // unmasks the same way outside a frame (a login reply's page address).
        let mut block = masked;
        InboundCipher::new(SEEDS).unmask_block(&mut block);
        assert_eq!(block, plain);
    }

    /// The three account-creation requests the original client wrote after
    /// its connect block (opcode masked, size prefix, tiny-encrypted body)
    /// are what this cipher produces from their plain frames.
    #[test]
    fn account_creation_requests_match_the_original_client() {
        let recorded = include_str!("../../../fixtures/recorded/login-crypto/logins.txt");
        let seeds: SessionSeeds = recorded
            .lines()
            .find_map(|l| l.strip_prefix("create-connect "))
            .map(|l| {
                let parts: Vec<i32> = l
                    .split(' ')
                    .next()
                    .unwrap()
                    .split(',')
                    .map(|h| u32::from_str_radix(h, 16).unwrap() as i32)
                    .collect();
                parts.try_into().unwrap()
            })
            .unwrap();
        let mut cipher = OutboundCipher::new(seeds).with_request_key(seeds);
        let opcodes = [
            proto::client::CREATE_CHECK_EMAIL,
            proto::client::CREATE_CHECK_NAME,
            proto::client::CREATE_ACCOUNT,
        ];
        let requests: Vec<&str> = recorded
            .lines()
            .filter(|l| l.starts_with("create-request-"))
            .collect();
        assert_eq!(requests.len(), 3);
        for (line, opcode) in requests.iter().zip(opcodes) {
            let fields: Vec<&str> = line.split(' ').collect();
            let masked_opcode =
                u8::from_str_radix(fields[1].trim_start_matches("op="), 16).unwrap();
            let body = unhex(fields[3]);
            // The plain frame: opcode, size prefix, the body decrypted with the seeds.
            let mut plain_body = body.clone();
            tiny_cipher::decrypt_range(&mut plain_body, 0, body.len(), &seeds);
            let mut plain = vec![opcode];
            match proto::client::size(opcode).unwrap() {
                -1 => plain.push(body.len() as u8),
                -2 => plain.extend((body.len() as u16).to_be_bytes()),
                _ => unreachable!("account creation requests are variable-sized"),
            }
            let prefix = plain.len() - 1;
            plain.extend(&plain_body);
            let mut wire = Vec::new();
            cipher.seal(&plain, &mut wire).unwrap();
            assert_eq!(wire[0], masked_opcode, "the masked opcode of {line}");
            assert_eq!(
                &wire[1 + prefix..],
                &body[..],
                "the tiny-encrypted body of {line}"
            );
        }
    }

    /// A frame written in pieces is held until it is complete, and an
    /// unknown opcode is refused.
    #[test]
    fn partial_frames_wait_and_unknown_opcodes_fail() {
        let mut cipher = OutboundCipher::new(SEEDS);
        let mut wire = Vec::new();
        let frame = [proto::client::MAP_BUILD_COMPLETE, 0, 0, 1, 2];
        cipher.seal(&frame[..3], &mut wire).unwrap();
        assert!(wire.is_empty());
        assert_eq!(cipher.held_len(), 3);
        cipher.seal(&frame[3..], &mut wire).unwrap();
        assert_eq!(wire.len(), 5);
        assert_eq!(cipher.held_len(), 0);
        assert!(cipher.seal(&[255], &mut wire).is_err());
    }
}
