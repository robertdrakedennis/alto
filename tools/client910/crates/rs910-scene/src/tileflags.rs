//! Per-level tile flag bytes read from the LAND stream (bit `0x2` of the
//! opcode byte).
//!
//! Bit meanings:
//! - `0x2` on plane 1: "link below" / bridge column; the map build turns it
//!   into a scene bridge. On plane 3 it is the roof-of-world marker.
//! - `0x4`: under-roof (roof hiding).
//! - `0x8`: force render level 0.
//! - `0x10`: suppress the level remap when the render level is chosen.

/// The tile flags, `flags[level][x][z]`, stored as the signed bytes the
/// client keeps.
#[derive(Clone, Debug)]
pub struct SceneLevelTileFlags {
    levels: usize,
    size_x: usize,
    size_z: usize,
    flags: Vec<i8>,
}

impl SceneLevelTileFlags {
    /// All flags zeroed.
    #[must_use]
    pub fn new(levels: usize, size_x: usize, size_z: usize) -> Self {
        Self {
            levels,
            size_x,
            size_z,
            flags: vec![0; levels * size_x * size_z],
        }
    }

    /// Tiles along X.
    #[must_use]
    pub fn size_x(&self) -> usize {
        self.size_x
    }

    /// Tiles along Z.
    #[must_use]
    pub fn size_z(&self) -> usize {
        self.size_z
    }

    fn index(&self, level: usize, x: usize, z: usize) -> usize {
        (level * self.size_x + x) * self.size_z + z
    }

    /// `flags[level][x][z]` (out-of-range panics, so callers keep coordinates
    /// in range).
    #[must_use]
    pub fn get(&self, level: usize, x: usize, z: usize) -> i8 {
        self.flags[self.index(level, x, z)]
    }

    /// `flags[level][x][z] = value`.
    pub fn set(&mut self, level: usize, x: usize, z: usize, value: i8) {
        let i = self.index(level, x, z);
        self.flags[i] = value;
    }

    /// Plane-3 bit `0x2`, bounds-checked.
    #[must_use]
    pub fn is_roof_of_world(&self, x: i32, z: i32) -> bool {
        self.bit_on_level(3, x, z, 0x2)
    }

    /// `isLinkBelow`: plane-1 bit `0x2`, bounds-checked.
    #[must_use]
    pub fn is_link_below(&self, x: i32, z: i32) -> bool {
        self.bit_on_level(1, x, z, 0x2)
    }

    fn bit_on_level(&self, level: usize, x: i32, z: i32, bit: i8) -> bool {
        if level >= self.levels || x < 0 || z < 0 {
            return false;
        }
        let (x, z) = (x as usize, z as usize);
        if x >= self.size_x || z >= self.size_z {
            return false;
        }
        (self.get(level, x, z) & bit) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_rules() {
        let mut f = SceneLevelTileFlags::new(4, 4, 4);
        assert!(!f.is_link_below(1, 1));
        f.set(1, 1, 1, 0x2);
        assert!(f.is_link_below(1, 1));
        // Only plane 1 marks a bridge column.
        f.set(2, 2, 2, 0x2);
        f.set(1, 2, 2, 0x8);
        assert!(!f.is_link_below(2, 2));
        assert!(!f.is_link_below(-1, 0));
        assert!(!f.is_link_below(4, 0));
    }
}
