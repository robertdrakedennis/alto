//! The ISAAC stream cipher the game stream masks its opcodes with.
//!
//! One generator per direction, seeded from the four session seeds of the
//! login block: the client's outgoing opcodes use the seeds as they are, the
//! server's use each seed plus [`INBOUND_SEED_OFFSET`]. The seed fills the
//! first four words of the generator's 256-word seed array, the rest is zero,
//! and values are handed out from the end of each 256-value batch down to its
//! start.

/// Added to each session seed for the server-to-client generator.
pub const INBOUND_SEED_OFFSET: i32 = 50;

const WORDS: usize = 256;
const GOLDEN_RATIO: u32 = 0x9E37_79B9;

/// An ISAAC generator.
#[derive(Clone, PartialEq, Eq)]
pub struct IsaacCipher {
    /// The current batch, handed out from the back.
    results: [u32; WORDS],
    memory: [u32; WORDS],
    accumulator: u32,
    last: u32,
    counter: u32,
    /// Values of the batch not yet handed out.
    remaining: usize,
}

impl std::fmt::Debug for IsaacCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IsaacCipher")
            .field("remaining", &self.remaining)
            .finish_non_exhaustive()
    }
}

/// One round of the ISAAC mixing function over its eight working words.
fn mix(w: &mut [u32; 8]) {
    w[0] ^= w[1] << 11;
    w[3] = w[3].wrapping_add(w[0]);
    w[1] = w[1].wrapping_add(w[2]);
    w[1] ^= w[2] >> 2;
    w[4] = w[4].wrapping_add(w[1]);
    w[2] = w[2].wrapping_add(w[3]);
    w[2] ^= w[3] << 8;
    w[5] = w[5].wrapping_add(w[2]);
    w[3] = w[3].wrapping_add(w[4]);
    w[3] ^= w[4] >> 16;
    w[6] = w[6].wrapping_add(w[3]);
    w[4] = w[4].wrapping_add(w[5]);
    w[4] ^= w[5] << 10;
    w[7] = w[7].wrapping_add(w[4]);
    w[5] = w[5].wrapping_add(w[6]);
    w[5] ^= w[6] >> 4;
    w[0] = w[0].wrapping_add(w[5]);
    w[6] = w[6].wrapping_add(w[7]);
    w[6] ^= w[7] << 8;
    w[1] = w[1].wrapping_add(w[6]);
    w[7] = w[7].wrapping_add(w[0]);
    w[7] ^= w[0] >> 9;
    w[2] = w[2].wrapping_add(w[7]);
    w[0] = w[0].wrapping_add(w[1]);
}

impl IsaacCipher {
    /// The generator of one direction from four session seeds.
    #[must_use]
    pub fn from_seeds(seeds: [i32; 4]) -> Self {
        let mut cipher = Self {
            results: [0; WORDS],
            memory: [0; WORDS],
            accumulator: 0,
            last: 0,
            counter: 0,
            remaining: 0,
        };
        for (slot, seed) in cipher.results.iter_mut().zip(seeds) {
            *slot = seed as u32;
        }
        cipher.initialise();
        cipher
    }

    /// The server-to-client generator for the seeds of a login block.
    #[must_use]
    pub fn inbound_from_seeds(seeds: [i32; 4]) -> Self {
        Self::from_seeds(seeds.map(|seed| seed.wrapping_add(INBOUND_SEED_OFFSET)))
    }

    fn initialise(&mut self) {
        let mut words = [GOLDEN_RATIO; 8];
        for _ in 0..4 {
            mix(&mut words);
        }
        // Two passes: the seed array first, then the memory it produced.
        for pass in 0..2 {
            for start in (0..WORDS).step_by(8) {
                for (offset, word) in words.iter_mut().enumerate() {
                    let source = if pass == 0 {
                        self.results[start + offset]
                    } else {
                        self.memory[start + offset]
                    };
                    *word = word.wrapping_add(source);
                }
                mix(&mut words);
                self.memory[start..start + 8].copy_from_slice(&words);
            }
        }
        self.refill();
    }

    /// Generate the next batch of 256 values.
    fn refill(&mut self) {
        self.counter = self.counter.wrapping_add(1);
        self.last = self.last.wrapping_add(self.counter);
        for index in 0..WORDS {
            let previous = self.memory[index];
            match index & 3 {
                0 => self.accumulator ^= self.accumulator << 13,
                1 => self.accumulator ^= self.accumulator >> 6,
                2 => self.accumulator ^= self.accumulator << 2,
                _ => self.accumulator ^= self.accumulator >> 16,
            }
            self.accumulator = self
                .accumulator
                .wrapping_add(self.memory[(index + 128) & 0xFF]);
            let updated = self
                .last
                .wrapping_add(self.accumulator)
                .wrapping_add(self.memory[(previous >> 2) as usize & 0xFF]);
            self.memory[index] = updated;
            self.last = self.memory[(updated >> 10) as usize & 0xFF].wrapping_add(previous);
            self.results[index] = self.last;
        }
        self.remaining = WORDS;
    }

    /// The next value, consuming it.
    pub fn next_value(&mut self) -> u32 {
        if self.remaining == 0 {
            self.refill();
        }
        self.remaining -= 1;
        self.results[self.remaining]
    }

    /// The value [`IsaacCipher::next_value`] returns next, without consuming it.
    pub fn peek_value(&mut self) -> u32 {
        if self.remaining == 0 {
            self.refill();
        }
        self.results[self.remaining - 1]
    }

    /// The next value as the byte the stream adds to (or subtracts from) a
    /// masked byte.
    pub fn next_byte(&mut self) -> u8 {
        self.next_value() as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both directions' generators reproduce the first 600 values (two
    /// refills) the original client's own generator produced.
    #[test]
    fn keystream_matches_the_original_client() {
        let recorded = include_str!("../../../fixtures/recorded/login-crypto/primitives.txt");
        let mut checked = 0;
        for line in recorded.lines().filter(|l| l.starts_with("isaac ")) {
            let mut fields = line.split(' ').skip(1);
            let seeds: Vec<i32> = fields
                .next()
                .unwrap()
                .split(',')
                .map(|h| u32::from_str_radix(h, 16).unwrap() as i32)
                .collect();
            let values: Vec<u32> = fields
                .next()
                .unwrap()
                .split(',')
                .map(|h| u32::from_str_radix(h, 16).unwrap())
                .collect();
            let mut cipher = IsaacCipher::from_seeds(seeds.clone().try_into().unwrap());
            for (index, want) in values.iter().enumerate() {
                if index == 300 {
                    assert_eq!(cipher.peek_value(), *want, "peek at value {index}");
                }
                assert_eq!(cipher.next_value(), *want, "value {index} of {seeds:08x?}");
            }
            checked += 1;
        }
        assert_eq!(checked, 3);
    }

    #[test]
    fn inbound_seeds_are_offset_by_fifty() {
        let seeds = [1, -2, i32::MAX, 0];
        let mut inbound = IsaacCipher::inbound_from_seeds(seeds);
        let mut manual = IsaacCipher::from_seeds([51, 48, i32::MIN + 49, 50]);
        for _ in 0..300 {
            assert_eq!(inbound.next_value(), manual.next_value());
        }
    }
}
