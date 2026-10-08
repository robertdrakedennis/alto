//! Vorbis residue decoding: the spectral detail added under the floor
//! envelope, coded in partitions with up to eight cascaded codebook passes.

use super::bits::BitReader;
use super::codebook::Codebook;
use super::floor::book;
use super::windows::ChannelWindows;
use anyhow::{bail, ensure, Result};

/// How the codebook vectors of a partition are laid into the spectrum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    /// Residue type 0: each vector is spread across the partition with a
    /// stride.
    Strided,
    /// Residue types 1 and 2: vectors follow one another. (Type 2 decodes
    /// every channel interleaved as one vector; the caller arranges that.)
    Sequential,
    /// An unknown residue type: the partition's codewords are read but
    /// nothing is added.
    Ignored,
}

/// One residue configuration from the setup header.
#[derive(Clone, Debug)]
pub(super) struct Residue {
    /// The residue type number (0, 1 or 2) as coded in the header.
    pub(super) kind: i32,
    layout: Layout,
    begin: i32,
    end: i32,
    /// Samples per partition.
    partition_size: i32,
    /// Number of partition classes.
    classes: i32,
    /// The book that codes the classification of a group of partitions.
    class_book: i32,
    /// Per class and pass: the book used (-1: none), `classes * 8` entries.
    pass_books: Vec<i32>,
}

impl Residue {
    /// Parse one residue from the setup header.
    pub(super) fn parse(bits: &mut BitReader<'_>) -> Self {
        let kind = bits.read(16);
        let begin = bits.read(24);
        let end = bits.read(24);
        let partition_size = bits.read(24) + 1;
        let classes = bits.read(6) + 1;
        let class_book = bits.read(8);
        let class_count = classes as usize;
        let mut cascade = vec![0_i32; class_count];
        for pass_mask in &mut cascade {
            let mut high = 0;
            let low = bits.read(3);
            if bits.read_bit() != 0 {
                high = bits.read(5);
            }
            *pass_mask = (high << 3) | low;
        }
        let pass_books = (0..class_count * 8)
            .map(|i| {
                if cascade[i >> 3] & (1 << (i & 7)) == 0 {
                    -1
                } else {
                    bits.read(8)
                }
            })
            .collect();
        Self {
            kind,
            layout: match kind {
                0 => Layout::Strided,
                1 | 2 => Layout::Sequential,
                _ => Layout::Ignored,
            },
            begin,
            end,
            partition_size,
            classes,
            class_book,
            pass_books,
        }
    }

    /// Decode this residue for the vectors in `vectors` (`silent[v]` marks a
    /// vector with no floor, which is skipped), adding into the first `n`
    /// samples of each.
    pub(super) fn decode(
        &self,
        bits: &mut BitReader<'_>,
        books: &[Codebook],
        vectors: &mut ChannelWindows,
        n: usize,
        silent: &[bool],
    ) -> Result<()> {
        let count = vectors.len();
        for (v, &is_silent) in silent[..count].iter().enumerate() {
            if !is_silent {
                vectors.get_mut(v)[..n].iter_mut().for_each(|x| *x = 0.0);
            }
        }
        let class_book = book(books, self.class_book)?;
        let classes_per_codeword = class_book.dimensions;
        let span = self.end - self.begin;
        ensure!(self.partition_size > 0, "residue partition size is zero");
        let partitions = (span / self.partition_size).max(0) as usize;
        // Per vector: the class of each partition.
        let mut classification = vec![vec![0_i32; partitions]; count];
        for pass in 0..8 {
            let mut partition = 0_usize;
            while partition < partitions {
                if pass == 0 {
                    for (v, &is_silent) in silent[..count].iter().enumerate() {
                        if !is_silent {
                            let mut value = class_book.decode_scalar(bits)?;
                            for d in (0..classes_per_codeword).rev() {
                                if partition + (d as usize) < partitions {
                                    classification[v][partition + d as usize] =
                                        value % self.classes;
                                }
                                value /= self.classes;
                            }
                        }
                    }
                }
                for _ in 0..classes_per_codeword {
                    for (v, &is_silent) in silent[..count].iter().enumerate() {
                        if is_silent {
                            continue;
                        }
                        let class = classification[v][partition];
                        let book_id = self.pass_books[(class * 8 + pass) as usize];
                        if book_id < 0 {
                            continue;
                        }
                        let start = self.partition_size * partition as i32 + self.begin;
                        let codebook = book(books, book_id)?;
                        ensure!(codebook.dimensions > 0, "residue book has no dimensions");
                        match self.layout {
                            Layout::Strided => {
                                let steps = self.partition_size / codebook.dimensions;
                                for step in 0..steps {
                                    let values = codebook.decode_vector(bits)?;
                                    for d in 0..codebook.dimensions {
                                        let index = (steps * d + start + step) as usize;
                                        let Some(slot) = vectors.get_mut(v).get_mut(index) else {
                                            bail!("residue sample {index} out of range");
                                        };
                                        *slot += values[d as usize];
                                    }
                                }
                            }
                            Layout::Sequential => {
                                let mut offset = 0;
                                while offset < self.partition_size {
                                    let values = codebook.decode_vector(bits)?;
                                    for &value in values {
                                        let index = (start + offset) as usize;
                                        let Some(slot) = vectors.get_mut(v).get_mut(index) else {
                                            bail!("residue sample {index} out of range");
                                        };
                                        *slot += value;
                                        offset += 1;
                                    }
                                }
                            }
                            Layout::Ignored => {}
                        }
                    }
                    partition += 1;
                    if partition >= partitions {
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}
