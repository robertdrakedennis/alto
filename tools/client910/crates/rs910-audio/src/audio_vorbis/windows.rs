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

#[cfg(test)]
mod tests {
    use super::*;

    /// With mux [1, 0, 0], submap 0 stores channel 0's buffer into slots 1
    /// and 2 (slot 2 reads the already replaced slot 1), submap 1 keeps slot
    /// 0: one buffer then backs all three channels.
    #[test]
    fn multi_submap_channels_share_one_buffer() {
        let mut w = ChannelWindows::new(3, 4);
        w.get_mut(1)[0] = 5.0;
        w.alias_submap_channels(&[1, 0, 0], 0);
        w.alias_submap_channels(&[1, 0, 0], 1);
        assert_eq!(w.slot, [0, 0, 0]);
        assert_eq!(w.get(1)[0], 0.0);
        w.get_mut(2)[3] = 7.0;
        assert_eq!(w.get(0)[3], 7.0);
        assert_eq!(w.get(1)[3], 7.0);
        // Two channels, mux [1, 0]: channel 1 takes position 0's buffer.
        let mut w = ChannelWindows::new(2, 2);
        w.alias_submap_channels(&[1, 0], 0);
        assert_eq!(w.slot, [0, 0]);
        w.alias_submap_channels(&[1, 0], 1);
        assert_eq!(w.slot, [0, 0]);
        // Each submap restarts at position 0, so mux [0, 1] also shares.
        let mut w = ChannelWindows::new(2, 2);
        w.alias_submap_channels(&[0, 1], 0);
        w.alias_submap_channels(&[0, 1], 1);
        assert_eq!(w.slot, [0, 0]);
    }
}
