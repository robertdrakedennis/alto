//! The per-channel working buffers of a block.

/// One working buffer per channel, holding the spectrum while it is decoded
/// and the windowed samples after the inverse MDCT.
///
/// For a mapping with several submaps the decoder does not always keep one
/// buffer per channel: after each submap's residue pass, that submap's
/// channels are re-pointed at the buffer of the earlier channel with the same
/// position in the submap (see [`ChannelWindows::alias_submap_channels`]).
/// The audio the client produces depends on this sharing, so it is kept, and
/// it survives the swap between the current and the previous block. Channels
/// therefore index `buffers` through `slot`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ChannelWindows {
    buffers: Vec<Vec<f32>>,
    slot: Vec<usize>,
}

impl ChannelWindows {
    /// `count` distinct zeroed buffers of `size` samples.
    pub(super) fn new(count: usize, size: usize) -> Self {
        Self {
            buffers: vec![vec![0.0; size]; count],
            slot: (0..count).collect(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.slot.len()
    }

    pub(super) fn get(&self, channel: usize) -> &Vec<f32> {
        &self.buffers[self.slot[channel]]
    }

    pub(super) fn get_mut(&mut self, channel: usize) -> &mut Vec<f32> {
        &mut self.buffers[self.slot[channel]]
    }

    /// The buffer of a single channel that holds the interleaved spectrum of
    /// every channel (residue type 2 decodes all channels as one vector).
    pub(super) fn single_buffer(&mut self) -> &mut Vec<f32> {
        &mut self.buffers[0]
    }

    /// Re-point the channels of `submap` (those whose `mux` entry equals
    /// it) at buffers in channel order: the k-th channel of the submap takes
    /// the buffer currently at position k. Reading and writing the same slot
    /// table in one pass is intended: later channels see earlier ones already
    /// replaced.
    pub(super) fn alias_submap_channels(&mut self, mux: &[i32], submap: i32) {
        let mut k = 0;
        for (channel, &owner) in mux[..self.slot.len()].iter().enumerate() {
            if owner == submap {
                self.slot[channel] = self.slot[k];
                k += 1;
            }
        }
    }
}
