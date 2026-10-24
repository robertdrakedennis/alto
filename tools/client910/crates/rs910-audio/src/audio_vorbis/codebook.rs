//! Vorbis codebooks: the Huffman decode tree and the vector quantisation
//! table, plus the cache that lets sounds from one encoder share their
//! (large, identical) codebook sets.

use super::bits::{bit_length, BitReader};
use anyhow::{bail, ensure, Result};
use std::sync::{Arc, Mutex};

/// One codebook: a binary decode tree over the entries and, for vector
/// books, the value vector of each entry.
#[derive(Clone, Debug, Default)]
pub(super) struct Codebook {
    /// Values per vector entry.
    pub(super) dimensions: i32,
    /// Decode tree stored as an array. A node's zero branch is the next
    /// slot; its one branch is the index stored in the node. A negative
    /// value `!entry` is a leaf.
    tree: Vec<i32>,
    /// One vector of `dimensions` values per entry (empty for scalar books).
    vectors: Vec<Vec<f32>>,
    /// The size of the serialised codebook, so a cached book can be skipped
    /// in the setup header without decoding it again.
    serialised_bytes: i32,
    serialised_bits: i32,
}

/// The largest whole `n` with `n.pow(dimensions) <= entries`, computed the
/// way the reference encoder does (float estimate, then correct downwards).
fn lookup1_values(entries: i32, dimensions: i32) -> i32 {
    let mut values = (f64::from(entries).powf(1.0 / f64::from(dimensions))) as i32 + 1;
    loop {
        let mut base = values;
        let mut exponent = dimensions;
        let mut accumulator = 1_i32;
        while exponent > 1 {
            if exponent & 1 != 0 {
                accumulator = base.wrapping_mul(accumulator);
            }
            base = base.wrapping_mul(base);
            exponent >>= 1;
        }
        let power = if exponent == 1 {
            base.wrapping_mul(accumulator)
        } else {
            accumulator
        };
        if power <= entries {
            return values;
        }
        values -= 1;
    }
}

/// A Vorbis 32-bit packed float: 21-bit mantissa, 10-bit exponent, sign.
fn unpack_float(value: i32) -> f32 {
    let mut mantissa = value & 0x1F_FFFF;
    let sign = value & i32::MIN;
    let exponent = (value >> 21) & 0x3FF;
    if sign != 0 {
        mantissa = -mantissa;
    }
    (f64::from(mantissa) * 2.0_f64.powf(f64::from(exponent - 788))) as f32
}

impl Codebook {
    /// Skip a codebook already known from the cache.
    fn skip(&self, bits: &mut BitReader<'_>) {
        bits.read(self.serialised_bytes * 8 + self.serialised_bits);
    }

    /// Parse one codebook from the setup header.
    fn parse(bits: &mut BitReader<'_>) -> Result<Self> {
        let start_bit = bits.bit_offset();
        let start_byte = bits.byte_offset();
        bits.read(24); // sync pattern
        let dimensions = bits.read(16);
        let entries = bits.read(24);
        let mut lengths = vec![0_i32; entries as usize];
        if bits.read_bit() != 0 {
            // Ordered: runs of equal length, lengths ascending.
            let mut entry = 0_i32;
            let mut length = bits.read(5) + 1;
            while entry < entries {
                let count = bits.read(bit_length(entries - entry));
                for _ in 0..count {
                    let Some(slot) = lengths.get_mut(entry as usize) else {
                        bail!("ordered codeword lengths overrun the entry count");
                    };
                    *slot = length;
                    entry += 1;
                }
                length += 1;
            }
        } else {
            let sparse = bits.read_bit() != 0;
            for length in &mut lengths {
                *length = if sparse && bits.read_bit() == 0 {
                    0
                } else {
                    bits.read(5) + 1
                };
            }
        }
        let tree = build_tree(&lengths)?;
        let lookup_type = bits.read(4);
        let mut vectors = Vec::new();
        if lookup_type > 0 {
            let minimum = unpack_float(bits.read(32));
            let delta = unpack_float(bits.read(32));
            let value_bits = bits.read(4) + 1;
            let sequence = bits.read_bit() != 0;
            let quant_count = if lookup_type == 1 {
                lookup1_values(entries, dimensions)
            } else {
                dimensions.wrapping_mul(entries)
            };
            ensure!(quant_count >= 0, "negative quantised value count");
            let quantised: Vec<i32> = (0..quant_count).map(|_| bits.read(value_bits)).collect();
            let dims = dimensions.max(0) as usize;
            vectors = vec![vec![0.0; dims]; entries as usize];
            if lookup_type == 1 {
                for (entry, vector) in vectors.iter_mut().enumerate() {
                    let mut last = 0.0_f32;
                    let mut divisor = 1_i32;
                    for slot in vector.iter_mut() {
                        ensure!(
                            divisor != 0 && quant_count != 0,
                            "codebook lookup divisor is zero"
                        );
                        let index = (entry as i32 / divisor) % quant_count;
                        let Some(&quant) =
                            usize::try_from(index).ok().and_then(|i| quantised.get(i))
                        else {
                            bail!("codebook quantised index {index} out of range");
                        };
                        let value = quant as f32 * delta + minimum + last;
                        *slot = value;
                        if sequence {
                            last = value;
                        }
                        divisor = quant_count.wrapping_mul(divisor);
                    }
                }
            } else {
                for (entry, vector) in vectors.iter_mut().enumerate() {
                    let mut last = 0.0_f32;
                    for (d, slot) in vector.iter_mut().enumerate() {
                        let value = quantised[dims * entry + d] as f32 * delta + minimum + last;
                        *slot = value;
                        if sequence {
                            last = value;
                        }
                    }
                }
            }
        }
        Ok(Self {
            dimensions,
            tree,
            vectors,
            serialised_bytes: bits.byte_offset() - start_byte,
            serialised_bits: bits.bit_offset() - start_bit,
        })
    }

    /// Decode one entry number with the Huffman tree.
    pub(super) fn decode_scalar(&self, bits: &mut BitReader<'_>) -> Result<i32> {
        let mut node = 0_usize;
        loop {
            let Some(&value) = self.tree.get(node) else {
                bail!("codeword walked off the decode tree");
            };
            if value < 0 {
                return Ok(!value);
            }
            node = if bits.read_bit() == 0 {
                node + 1
            } else {
                value as usize
            };
        }
    }

    /// Decode one entry and return its value vector.
    pub(super) fn decode_vector(&self, bits: &mut BitReader<'_>) -> Result<&[f32]> {
        let entry = self.decode_scalar(bits)?;
        match self.vectors.get(entry as usize) {
            Some(vector) => Ok(vector),
            None => bail!("codebook has no value vector for entry {entry}"),
        }
    }
}

/// Assign canonical codewords to the entries (the specification's
/// "next free codeword per length" scheme) and build the decode tree.
fn build_tree(lengths: &[i32]) -> Result<Vec<i32>> {
    let mut codewords = vec![0_u32; lengths.len()];
    // The next unassigned codeword of each length, left-aligned in 32 bits.
    let mut next_free = [0_u32; 33];
    for (entry, &length) in lengths.iter().enumerate() {
        if length == 0 {
            continue;
        }
        ensure!(
            (1..=32).contains(&length),
            "codeword length {length} out of range"
        );
        let length = length as usize;
        let bit = 1_u32 << (32 - length);
        let word = next_free[length];
        codewords[entry] = word;
        let following;
        if word & bit == 0 {
            following = word | bit;
            for shorter in (1..length).rev() {
                let existing = next_free[shorter];
                if word != existing {
                    break;
                }
                let shorter_bit = 1_u32 << (32 - shorter);
                if existing & shorter_bit != 0 {
                    next_free[shorter] = next_free[shorter - 1];
                    break;
                }
                next_free[shorter] = existing | shorter_bit;
            }
        } else {
            following = next_free[length - 1];
        }
        next_free[length] = following;
        for longer in &mut next_free[length + 1..=32] {
            if word == *longer {
                *longer = following;
            }
        }
    }
    let mut tree = vec![0_i32; 8];
    let mut free_node = 0_i32;
    for (entry, &length) in lengths.iter().enumerate() {
        if length == 0 {
            continue;
        }
        let word = codewords[entry];
        let mut node = 0_i32;
        for depth in 0..length {
            let mask = 0x8000_0000_u32 >> depth;
            if word & mask == 0 {
                node += 1;
            } else {
                if tree[node as usize] == 0 {
                    tree[node as usize] = free_node;
                }
                node = tree[node as usize];
            }
            ensure!(node >= 0, "decode tree node is negative");
            if node as usize >= tree.len() {
                let mut grown = vec![0; tree.len() * 2];
                grown[..tree.len()].copy_from_slice(&tree);
                tree = grown;
            }
        }
        tree[node as usize] = !(entry as i32);
        if node >= free_node {
            free_node = node + 1;
        }
    }
    Ok(tree)
}

/// A cached codebook set and the stream properties it was built for.
struct CachedSet {
    channels: i32,
    sample_rate: i32,
    bitrate_upper: i32,
    bitrate_lower: i32,
    bitrate_nominal: i32,
    books: Arc<Vec<Codebook>>,
}

/// The stream properties that identify a codebook set.
#[derive(Clone, Copy)]
pub(super) struct SetKey {
    pub(super) channels: i32,
    pub(super) sample_rate: i32,
    pub(super) bitrate_upper: i32,
    pub(super) bitrate_lower: i32,
    pub(super) bitrate_nominal: i32,
}

/// Codebook sets already parsed, shared by every decoder holding a clone of
/// the handle.
///
/// The cache is an optimisation for sounds that come from one encoder
/// configuration: a stream with the same channel count, sample rate, nominal
/// bitrate and codebook count (and no upper or lower bitrate limit) reuses
/// the first stream's books instead of building them again, skipping the
/// serialised codebooks in its own header.
#[derive(Clone, Default)]
pub struct SetupCache {
    sets: Arc<Mutex<Vec<CachedSet>>>,
}

impl SetupCache {
    /// Read the `count` codebooks that follow in the setup header, or reuse
    /// a cached set for the same stream properties.
    pub(super) fn read_books(
        &self,
        bits: &mut BitReader<'_>,
        key: SetKey,
        count: usize,
    ) -> Result<Arc<Vec<Codebook>>> {
        let mut sets = self.sets.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = sets.iter().find(|c| {
            c.bitrate_upper == 0
                && c.bitrate_lower == 0
                && c.books.len() == count
                && key.channels == c.channels
                && key.sample_rate == c.sample_rate
                && key.bitrate_nominal == c.bitrate_nominal
        }) {
            for book in cached.books.iter() {
                book.skip(bits);
            }
            return Ok(cached.books.clone());
        }
        let books = (0..count)
            .map(|_| Codebook::parse(bits))
            .collect::<Result<Vec<_>>>()?;
        let books = Arc::new(books);
        sets.push(CachedSet {
            channels: key.channels,
            sample_rate: key.sample_rate,
            bitrate_upper: key.bitrate_upper,
            bitrate_lower: key.bitrate_lower,
            bitrate_nominal: key.bitrate_nominal,
            books: books.clone(),
        });
        Ok(books)
    }
}
