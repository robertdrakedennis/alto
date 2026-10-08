//! Vorbis channel mappings: which floor and residue each channel uses and
//! how coupled channel pairs (magnitude and angle) are stored.

use super::bits::{field_width, BitReader};

/// One coupled channel pair.
#[derive(Clone, Copy, Debug)]
pub(super) struct Coupling {
    pub(super) magnitude: usize,
    pub(super) angle: usize,
}

/// One mapping from the setup header.
#[derive(Clone, Debug)]
pub(super) struct Mapping {
    pub(super) submaps: i32,
    /// Coupled pairs in coding order (undone in reverse).
    pub(super) coupling: Vec<Coupling>,
    /// The submap of each channel; absent when the mapping has one submap.
    pub(super) submap_of_channel: Option<Vec<i32>>,
    /// Per submap: the floor and residue it uses.
    pub(super) submap_floor: Vec<i32>,
    pub(super) submap_residue: Vec<i32>,
}

impl Mapping {
    /// Parse one mapping from the setup header.
    pub(super) fn parse(bits: &mut BitReader<'_>, channels: i32) -> Self {
        bits.read(16); // mapping type (always 0)
        let submaps = if bits.read_bit() == 0 {
            1
        } else {
            bits.read(4) + 1
        };
        let mut coupling = Vec::new();
        if bits.read_bit() != 0 {
            let steps = bits.read(8) + 1;
            for _ in 0..steps {
                let magnitude = bits.read(field_width(channels - 1)) as usize;
                let angle = bits.read(field_width(channels - 1)) as usize;
                coupling.push(Coupling { magnitude, angle });
            }
        }
        bits.read(2); // reserved
        let submap_of_channel =
            (submaps > 1).then(|| (0..channels).map(|_| bits.read(4)).collect());
        let mut submap_floor = Vec::new();
        let mut submap_residue = Vec::new();
        for _ in 0..submaps {
            bits.read(8); // time configuration (unused)
            submap_floor.push(bits.read(8));
            submap_residue.push(bits.read(8));
        }
        Self {
            submaps,
            coupling,
            submap_of_channel,
            submap_floor,
            submap_residue,
        }
    }

    /// The submap a channel belongs to.
    pub(super) fn submap_of(&self, channel: usize) -> usize {
        self.submap_of_channel
            .as_ref()
            .map_or(0, |mux| mux[channel]) as usize
    }
}
