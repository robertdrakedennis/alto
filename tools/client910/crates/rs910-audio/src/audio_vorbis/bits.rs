//! The least-significant-bit-first bit reader of the Vorbis bitstream.

/// Reads bit fields out of a byte slice, low bit first, as the Vorbis
/// specification lays them out.
///
/// Reading past the end of the slice yields zero bits instead of failing:
/// the decoder relies on truncated packets padding out with zeros.
pub(super) struct BitReader<'a> {
    data: &'a [u8],
    byte: usize,
    bit: u32,
}

impl<'a> BitReader<'a> {
    /// A reader positioned at the first bit of `data[offset]`.
    pub(super) fn new(data: &'a [u8], offset: usize) -> Self {
        Self {
            data,
            byte: offset,
            bit: 0,
        }
    }

    fn byte_at(&self, index: usize) -> u32 {
        u32::from(self.data.get(index).copied().unwrap_or(0))
    }

    /// Bit offset within the current byte (0-7).
    pub(super) fn bit_offset(&self) -> i32 {
        self.bit as i32
    }

    /// Index of the byte the next bit comes from.
    pub(super) fn byte_offset(&self) -> i32 {
        self.byte as i32
    }

    pub(super) fn read_bit(&mut self) -> i32 {
        let value = (self.byte_at(self.byte) >> self.bit) & 1;
        self.bit += 1;
        self.byte += (self.bit >> 3) as usize;
        self.bit &= 7;
        value as i32
    }

    /// Read `count` bits (0-32) as an unsigned field, wrapping into `i32`.
    pub(super) fn read(&mut self, mut count: i32) -> i32 {
        let mut value = 0_i32;
        let mut shift = 0_u32;
        while count >= 8 - self.bit as i32 {
            let take = 8 - self.bit;
            let mask = (1_u32 << take) - 1;
            let part = ((self.byte_at(self.byte) >> self.bit) & mask) as i32;
            value = value.wrapping_add(part.wrapping_shl(shift));
            self.bit = 0;
            self.byte += 1;
            shift = shift.wrapping_add(take);
            count -= take as i32;
        }
        if count > 0 {
            let mask = (1_u32 << count) - 1;
            let part = ((self.byte_at(self.byte) >> self.bit) & mask) as i32;
            value = value.wrapping_add(part.wrapping_shl(shift));
            self.bit += count as u32;
        }
        value
    }
}

/// The number of bits needed to represent `value` (`ilog` of the Vorbis
/// specification: 0 for 0, 1 for 1, 2 for 2-3, and so on).
pub(super) fn bit_length(value: i32) -> i32 {
    let mut value = value as u32;
    let mut bits = 0;
    if value >= 65536 {
        value >>= 16;
        bits += 16;
    }
    if value >= 256 {
        value >>= 8;
        bits += 8;
    }
    if value >= 16 {
        value >>= 4;
        bits += 4;
    }
    if value >= 4 {
        value >>= 2;
        bits += 2;
    }
    if value >= 1 {
        value >>= 1;
        bits += 1;
    }
    value as i32 + bits
}

/// The lowest `bits` bits of `value`, in reverse order.
pub(super) fn reverse_bits(mut value: i32, mut bits: i32) -> i32 {
    let mut out = 0;
    while bits > 0 {
        out = (out << 1) | (value & 1);
        value = ((value as u32) >> 1) as i32;
        bits -= 1;
    }
    out
}

/// The number of bits needed to represent a non-negative `value`, counting
/// negative values as 0. Used for the channel-number and mode-number fields.
pub(super) fn field_width(mut value: i32) -> i32 {
    let mut bits = 0;
    while value > 0 {
        bits += 1;
        value >>= 1;
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_read_low_bit_first_across_byte_boundaries() {
        let data = [0b1010_0101, 0b0000_1111, 0xFF];
        let mut bits = BitReader::new(&data, 0);
        assert_eq!(bits.read(4), 0b0101);
        assert_eq!(bits.read_bit(), 0);
        // Bits 5-7 of the first byte (1, 0, 1: value 5), then the low four
        // bits of the second byte (1111) shifted above them.
        assert_eq!(bits.read(7), 5 | (0b1111 << 3));
        assert_eq!((bits.byte_offset(), bits.bit_offset()), (1, 4));
    }

    #[test]
    fn reads_past_the_end_are_zero() {
        let mut bits = BitReader::new(&[0xFF], 0);
        assert_eq!(bits.read(8), 0xFF);
        assert_eq!(bits.read(16), 0);
    }

    #[test]
    fn bit_helpers() {
        assert_eq!(
            [0, 1, 2, 3, 4, 255, 256].map(bit_length),
            [0, 1, 2, 2, 3, 8, 9]
        );
        assert_eq!(reverse_bits(0b0011, 4), 0b1100);
        assert_eq!([0, 1, 2, 3].map(field_width), [0, 1, 2, 2]);
    }
}
