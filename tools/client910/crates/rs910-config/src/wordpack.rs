//! Word-pack (Huffman) message encoding and decoding.
use anyhow::{Context, Result};

#[derive(Clone, Debug)]
struct Node {
    child: [i32; 2],
    symbol: i16,
}

#[derive(Clone, Debug)]
pub struct Huffman {
    nodes: Vec<Node>,
    /// Canonical MSB-first code and bit length for each CP1252 symbol.
    codes: Vec<(u32, u8)>,
}

impl Huffman {
    /// Build the same canonical tree as `Huffman.<init>` from the binary JS5
    /// code-length table. Codes are consumed most-significant bit first.
    pub fn from_lengths(lengths: &[u8]) -> Result<Self> {
        anyhow::ensure!(lengths.len() <= 256, "Huffman symbol table too large");
        let mut next = [0u32; 33];
        let mut encoded = vec![0u32; lengths.len()];
        for (symbol, &length) in lengths.iter().enumerate() {
            let length = usize::from(length);
            if length == 0 {
                continue;
            }
            anyhow::ensure!(length <= 32, "Huffman code length {length}");
            let bit = 1u32 << (32 - length);
            let code = next[length];
            encoded[symbol] = code;
            let replacement = if code & bit == 0 {
                let replacement = code | bit;
                // The canonical next-code update is carried down through
                // shorter lengths before publishing it.  The shorter table
                // entries are needed when a later symbol uses the same
                // prefix.
                for shorter in (1..length).rev() {
                    if next[shorter] != code {
                        break;
                    }
                    let shorter_bit = 1u32 << (32 - shorter);
                    if next[shorter] & shorter_bit != 0 {
                        next[shorter] = next[shorter - 1];
                        break;
                    }
                    next[shorter] |= shorter_bit;
                }
                replacement
            } else {
                next[length - 1]
            };
            next[length] = replacement;
            for longer in &mut next[(length + 1)..=32] {
                if *longer == code {
                    *longer = replacement;
                }
            }
        }
        let mut nodes = vec![Node {
            child: [-1; 2],
            symbol: -1,
        }];
        for (symbol, &length) in lengths.iter().enumerate() {
            let length = usize::from(length);
            if length == 0 {
                continue;
            }
            let mut node = 0usize;
            for bit_index in 0..length {
                anyhow::ensure!(nodes[node].symbol < 0, "Huffman prefix conflict");
                let bit = ((encoded[symbol] >> (31 - bit_index)) & 1) as usize;
                let next_node = nodes[node].child[bit];
                if next_node < 0 {
                    let fresh = i32::try_from(nodes.len()).context("Huffman tree too large")?;
                    nodes[node].child[bit] = fresh;
                    nodes.push(Node {
                        child: [-1; 2],
                        symbol: -1,
                    });
                    node = usize::try_from(fresh).unwrap();
                } else {
                    node = usize::try_from(next_node).unwrap();
                }
            }
            anyhow::ensure!(nodes[node].symbol < 0, "Huffman duplicate code");
            anyhow::ensure!(nodes[node].child == [-1; 2], "Huffman code prefix");
            nodes[node].symbol = i16::try_from(symbol).context("Huffman symbol")?;
        }
        let codes = lengths
            .iter()
            .zip(encoded)
            .map(|(&length, code)| (code, length))
            .collect();
        Ok(Self { nodes, codes })
    }

    /// Decode exactly `length` bytes, returning the compressed byte count.
    pub fn decode(&self, input: &[u8], length: usize) -> Result<(Vec<u8>, usize)> {
        let mut output = Vec::with_capacity(length);
        let mut node = 0usize;
        let mut consumed = 0usize;
        while output.len() < length {
            let byte = *input.get(consumed).context("truncated Huffman message")?;
            consumed += 1;
            for shift in (0..8).rev() {
                let bit = usize::from((byte >> shift) & 1);
                let next = self.nodes[node].child[bit];
                anyhow::ensure!(next >= 0, "invalid Huffman code");
                node = usize::try_from(next).unwrap();
                if self.nodes[node].symbol >= 0 {
                    output.push(self.nodes[node].symbol as u8);
                    node = 0;
                    if output.len() == length {
                        break;
                    }
                }
            }
        }
        Ok((output, consumed))
    }

    /// Huffman compression, emitting the same MSB-first bit stream used by
    /// `decode`. The returned bytes contain only compressed symbols; callers
    /// add the `pSmart1or2s` decoded-byte count first.
    pub fn encode(&self, input: &[u8]) -> Result<Vec<u8>> {
        let mut output = Vec::with_capacity(input.len());
        let mut current = 0u8;
        let mut used = 0u8;
        for &symbol in input {
            let (code, length) = *self
                .codes
                .get(usize::from(symbol))
                .context("Huffman symbol outside table")?;
            anyhow::ensure!(length != 0, "Huffman symbol {symbol} has no code");
            let code = code >> (32 - u32::from(length));
            for bit in (0..length).rev() {
                current |= (((code >> bit) & 1) as u8) << (7 - used);
                used += 1;
                if used == 8 {
                    output.push(current);
                    current = 0;
                    used = 0;
                }
            }
        }
        if used != 0 {
            output.push(current);
        }
        Ok(output)
    }

    /// Pack a chat message: CP1252 conversion, smart decoded length, then
    /// Huffman compression.
    pub fn encode_string(&self, text: &str) -> Result<Vec<u8>> {
        let raw: Vec<u8> = text
            .encode_utf16()
            .map(rs910_core::cp1252::cp1252_encode_unit)
            .collect();
        anyhow::ensure!(
            raw.len() < 32768,
            "packed message exceeds the smart length limit"
        );
        let mut out = if raw.len() < 128 {
            vec![raw.len() as u8]
        } else {
            let value = (raw.len() as u16) | 0x8000;
            value.to_be_bytes().to_vec()
        };
        out.extend(self.encode(&raw)?);
        Ok(out)
    }

    pub fn from_pack(pack: &crate::cache::Pack) -> Result<Self> {
        let group = pack
            .group_id_by_name("binary", "huffman")?
            .context("binary/huffman group missing")?;
        let files = pack.read_group("binary", group)?;
        let bytes = files.get(&0).context("binary/huffman file missing")?;
        Self::from_lengths(bytes)
    }
}

pub fn read_gjstr(bytes: &[u8], pos: &mut usize) -> Result<String> {
    let start = *pos;
    while *pos < bytes.len() && bytes[*pos] != 0 {
        *pos += 1;
    }
    anyhow::ensure!(*pos < bytes.len(), "unterminated gjstr");
    let value = bytes[start..*pos]
        .iter()
        .map(|&b| rs910_core::cp1252::cp1252_decode_byte(b))
        .collect::<String>();
    *pos += 1;
    Ok(value)
}

pub fn read_smart(bytes: &[u8], pos: &mut usize) -> Result<usize> {
    let first = *bytes.get(*pos).context("truncated packed message length")?;
    if first < 128 {
        *pos += 1;
        Ok(usize::from(first))
    } else {
        anyhow::ensure!(*pos + 1 < bytes.len(), "truncated packed message length");
        let value = u16::from_be_bytes([first, bytes[*pos + 1]]) & 0x7fff;
        *pos += 2;
        Ok(usize::from(value))
    }
}

pub fn decode_string(huffman: &Huffman, bytes: &[u8], pos: &mut usize) -> Result<String> {
    let length = read_smart(bytes, pos)?.min(32767);
    let (raw, consumed) = huffman.decode(&bytes[*pos..], length)?;
    *pos += consumed;
    Ok(raw
        .into_iter()
        .map(rs910_core::cp1252::cp1252_decode_byte)
        .collect())
}

/// Protect chat markup before the text enters the chat history.
pub fn escape(input: impl AsRef<str>) -> String {
    let mut out = String::with_capacity(input.as_ref().len());
    for ch in input.as_ref().chars() {
        match ch {
            '<' => out.push_str("<lt>"),
            '>' => out.push_str("<gt>"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_msb_first_two_symbol_fixture() -> Result<()> {
        // A two-entry code-length table is the smallest valid Huffman
        // table: symbol 0 is `0`, symbol 1 is `1`.
        let huffman = Huffman::from_lengths(&[1, 1])?;
        let (decoded, consumed) = huffman.decode(&[0b0101_0000], 4)?;
        assert_eq!(decoded, vec![0, 1, 0, 1]);
        assert_eq!(consumed, 1);
        Ok(())
    }

    #[test]
    fn wordpack_reads_smart_length_and_cp1252_payload() -> Result<()> {
        let huffman = Huffman::from_lengths(&[1, 1])?;
        let mut pos = 0;
        let text = decode_string(&huffman, &[2, 0b0100_0000], &mut pos)?;
        assert_eq!(text, "\0\x01");
        assert_eq!(pos, 2);
        Ok(())
    }

    #[test]
    fn wordpack_encode_matches_msb_smart_framing() -> Result<()> {
        let huffman = Huffman::from_lengths(&[1, 1])?;
        assert_eq!(huffman.encode_string("\u{1}\u{1}")?, vec![2, 0b1100_0000]);
        Ok(())
    }
}
