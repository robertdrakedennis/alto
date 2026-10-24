//! Decoding one Vorbis audio packet (a block): floors, coupled residue,
//! inverse MDCT, windowing and the overlap-add with the previous block.

use super::bits::{field_width, BitReader};
use super::imdct::{imdct, window_fall, window_rise};
use super::setup::{Identification, Setup};
use super::windows::ChannelWindows;
use anyhow::{anyhow, ensure, Result};

/// The samples of the overlap-added region of a block: one vector per
/// channel.
pub(super) type PcmBlock = Vec<Vec<f32>>;

/// The block-to-block state of the decoder: the previous block's transform
/// output that the next block overlaps with, and the per-channel flags.
#[derive(Default)]
pub(super) struct BlockDecoder {
    /// The buffers the next block decodes into.
    current: Option<ChannelWindows>,
    /// The previous block's transformed samples.
    previous: Option<ChannelWindows>,
    /// Size of the previous block (0 after a reset: nothing to overlap).
    previous_block_size: i32,
    /// End of the previous block's right half, relative to its midpoint.
    previous_right_end: i32,
    /// Channels without a floor in this block (they decode to silence).
    silent: Vec<bool>,
    /// The same flags as decoded from the floors, before coupling.
    silent_by_floor: Vec<bool>,
    /// The silence flags of the channels of the submap being decoded.
    /// Entries past the current submap's channels keep earlier values; a
    /// mapping with several submaps depends on that.
    residue_silent: Vec<bool>,
}

impl BlockDecoder {
    /// Start a stream: allocate the working buffers for `ident`'s long block.
    pub(super) fn install(&mut self, ident: &Identification) {
        self.current = Some(ChannelWindows::new(
            ident.channels as usize,
            ident.long_block as usize,
        ));
    }

    /// Forget all block history (a new stream, or a loop restart at a stream
    /// boundary).
    pub(super) fn clear(&mut self) {
        self.previous = None;
        self.current = None;
        self.previous_block_size = 0;
        self.previous_right_end = 0;
    }

    /// After a packet that produced no audio (a header, or a block skipped
    /// while seeking to a loop point) the next audio block must not overlap
    /// with the previous one: drop the overlap and make sure a zeroed
    /// previous block of the right shape exists.
    pub(super) fn break_overlap(&mut self, ident: &Identification) {
        self.previous_block_size = 0;
        let stale = self.previous.as_ref().is_none_or(|w| {
            w.len() != ident.channels as usize || w.get(0).len() != ident.long_block as usize
        });
        if stale {
            self.previous = Some(ChannelWindows::new(
                ident.channels as usize,
                ident.long_block as usize,
            ));
        }
    }

    /// Decode the audio packet at `data[offset..]`, whose first bit (the
    /// packet type) has already been consumed into `bits`. Returns the
    /// overlap-added samples the block completes, or `None` for the first
    /// block after a reset.
    pub(super) fn decode(
        &mut self,
        setup: &mut Setup,
        ident: &Identification,
        bits: &mut BitReader<'_>,
    ) -> Result<Option<PcmBlock>> {
        let Setup {
            books,
            floors,
            residues,
            mappings,
            modes,
            tables,
        } = setup;
        let mode_index = bits.read(field_width(modes.len() as i32 - 1)) as usize;
        ensure!(
            mode_index < modes.len(),
            "block mode {mode_index} undefined"
        );
        let mode = modes[mode_index];
        let long = mode.long_block;
        let n = if long {
            ident.long_block
        } else {
            ident.short_block
        };
        let (mut previous_long, mut next_long) = (false, false);
        if long {
            previous_long = bits.read_bit() != 0;
            next_long = bits.read_bit() != 0;
        }
        let half = n >> 1;
        let short_quarter = ident.short_block >> 2;
        let (left_start, left_end, left_len) = if long && !previous_long {
            (
                (n >> 2) - short_quarter,
                short_quarter + (n >> 2),
                ident.short_block >> 1,
            )
        } else {
            (0, half, n >> 1)
        };
        let (right_start, right_end, right_len) = if long && !next_long {
            (
                n - (n >> 2) - short_quarter,
                short_quarter + (n - (n >> 2)),
                ident.short_block >> 1,
            )
        } else {
            (half, n, n >> 1)
        };
        ensure!(
            mode.mapping < mappings.len(),
            "mapping {} undefined",
            mode.mapping
        );
        let mapping = &mappings[mode.mapping];
        let channels = ident.channels as usize;
        if self.silent.len() != channels {
            self.silent = vec![false; channels];
            self.silent_by_floor = vec![false; channels];
        }
        for channel in 0..channels {
            let submap = mapping.submap_of(channel);
            let floor = *mapping
                .submap_floor
                .get(submap)
                .ok_or_else(|| anyhow!("submap {submap} undefined"))?
                as usize;
            ensure!(floor < floors.len(), "floor {floor} undefined");
            self.silent[channel] = !floors[floor].read_channel(channel, bits, books)?;
            self.silent_by_floor[channel] = self.silent[channel];
        }
        // A coupled pair is silent only when both channels are.
        for pair in &mapping.coupling {
            ensure!(
                pair.magnitude < channels && pair.angle < channels,
                "coupled channel out of range"
            );
            if !self.silent[pair.magnitude] || !self.silent[pair.angle] {
                self.silent[pair.magnitude] = false;
                self.silent[pair.angle] = false;
            }
        }
        if self.residue_silent.len() != channels {
            self.residue_silent = vec![false; channels];
        }
        let mut current = self
            .current
            .take()
            .ok_or_else(|| anyhow!("no working buffers for the block"))?;
        let n_us = n as usize;
        for submap in 0..mapping.submaps {
            // Gather the silence flags of this submap's channels; entries
            // beyond them keep their previous values.
            let mut packed = 0;
            for channel in 0..channels {
                let owner = mapping
                    .submap_of_channel
                    .as_ref()
                    .map_or(submap, |m| m[channel]);
                if owner == submap {
                    self.residue_silent[packed] = self.silent[channel];
                    packed += 1;
                }
            }
            let residue_id = mapping.submap_residue[submap as usize] as usize;
            ensure!(
                residue_id < residues.len(),
                "residue {residue_id} undefined"
            );
            let residue = &residues[residue_id];
            if residue.kind == 2 {
                // Type 2 codes all channels as one interleaved vector.
                let mut interleaved = ChannelWindows::new(1, channels * n_us);
                for i in 0..n_us {
                    for channel in 0..channels {
                        interleaved.single_buffer()[channels * i + channel] =
                            current.get(channel)[i];
                    }
                }
                residue.decode(
                    bits,
                    books,
                    &mut interleaved,
                    half as usize,
                    &self.residue_silent,
                )?;
                for i in 0..n_us {
                    for channel in 0..channels {
                        current.get_mut(channel)[i] =
                            interleaved.single_buffer()[channels * i + channel];
                    }
                }
            } else {
                residue.decode(
                    bits,
                    books,
                    &mut current,
                    half as usize,
                    &self.residue_silent,
                )?;
            }
            if let Some(mux) = mapping.submap_of_channel.as_ref() {
                current.alias_submap_channels(mux, submap);
            }
        }
        // Undo channel coupling, last pair first.
        for pair in mapping.coupling.iter().rev() {
            let (m, a) = (pair.magnitude, pair.angle);
            let len = current.get(m).len();
            for i in 0..len {
                let magnitude = current.get(m)[i];
                let angle = current.get(a)[i];
                let (new_magnitude, new_angle);
                if magnitude > 0.0 {
                    if angle > 0.0 {
                        new_magnitude = magnitude;
                        new_angle = magnitude - angle;
                    } else {
                        new_angle = magnitude;
                        new_magnitude = magnitude + angle;
                    }
                } else if angle > 0.0 {
                    new_magnitude = magnitude;
                    new_angle = magnitude + angle;
                } else {
                    new_angle = magnitude;
                    new_magnitude = magnitude - angle;
                }
                current.get_mut(m)[i] = new_magnitude;
                current.get_mut(a)[i] = new_angle;
            }
        }
        self.silent.copy_from_slice(&self.silent_by_floor);
        for channel in 0..channels {
            if !self.silent[channel] {
                let floor = mapping.submap_floor[mapping.submap_of(channel)] as usize;
                floors[floor].compute_amplitudes(channel)?;
                floors[floor].render(channel, current.get_mut(channel), half)?;
            }
        }
        let table = &tables[usize::from(long)];
        for channel in 0..channels {
            if self.silent[channel] {
                for x in &mut current.get_mut(channel)[half as usize..n_us] {
                    *x = 0.0;
                }
            } else {
                let window = current.get_mut(channel);
                imdct(window, n, table);
                window_rise(window, left_start, left_end, left_len);
                window_fall(window, right_start, right_end, right_len);
            }
        }
        let mut output = None;
        if self.previous_block_size > 0 {
            let len = ((self.previous_block_size + n) >> 2) as usize;
            let mut out = vec![vec![0.0_f32; len]; channels];
            let previous = self
                .previous
                .as_ref()
                .ok_or_else(|| anyhow!("no previous block to overlap"))?;
            // The silence test below reads this block's flags for both
            // halves of the overlap, including the previous block's tail.
            for (channel, out_channel) in out.iter_mut().enumerate() {
                if !self.silent[channel] {
                    let base = (self.previous_block_size >> 1) as usize;
                    let end = self.previous_right_end.max(0) as usize;
                    for (i, sample) in out_channel[..end].iter_mut().enumerate() {
                        *sample += previous.get(channel)[base + i];
                    }
                    for i in left_start..half {
                        let at = (len as i32 - half + i) as usize;
                        out_channel[at] += current.get(channel)[i as usize];
                    }
                }
            }
            output = Some(out);
        }
        let previous = self.previous.take();
        self.previous = Some(current);
        self.current = previous;
        self.previous_block_size = n;
        self.previous_right_end = right_end - half;
        Ok(output)
    }
}
