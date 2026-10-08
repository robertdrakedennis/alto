//! Minimal PNG writer (RGB8, stored/uncompressed deflate) for screenshots.
//! No crates: zlib framing with stored blocks; adler32 and crc32 from
//! [`crate::checksum`] (this module's copies were merged there in Phase 2.1).

use std::io::Write;

use crate::checksum::{adler32, crc32};

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + body.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(body);
    out.extend_from_slice(&crc_input);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// Encode `rgb` (`w*h*3` bytes, top row first) as a PNG file.
pub fn write_rgb_png(path: &std::path::Path, w: u32, h: u32, rgb: &[u8]) -> crate::Result<()> {
    crate::ensure!(
        rgb.len() == (w * h * 3) as usize,
        "png: rgb buffer size mismatch"
    );
    // Filter byte 0 per scanline.
    let mut raw = Vec::with_capacity((w * 3 + 1) as usize * h as usize);
    for row in rgb.chunks_exact((w * 3) as usize) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    // zlib: CMF/FLG, stored blocks of <= 65535 bytes, adler32.
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535).peekable();
    while let Some(block) = blocks.next() {
        let last = blocks.peek().is_none();
        z.push(u8::from(last));
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    if raw.is_empty() {
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    let mut file = std::fs::File::create(path)?;
    file.write_all(&out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_known_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }
}
