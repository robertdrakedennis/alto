//! Normal merging across two placed models.

use super::{GpuModel, MergedNormals};

impl GpuModel {
    /// Merges the normals of coincident vertices of two models, so the seam
    /// between neighbouring models shades smoothly. `other` sits at
    /// `(dx, dy, dz)` relative to `self`. Both models keep separate merged
    /// copies; the base normals stay shared.
    ///
    /// The slot walk over `other` reads each slot as a signed 16-bit value,
    /// while every other read of a slot masks it to 16 bits. A model with
    /// more than 32767 unique vertices therefore has slots that read as
    /// negative there; the original client throws on the first one, so the
    /// merge stops at that point and keeps what it merged so far.
    pub fn merge_normals(&mut self, other: &mut GpuModel, dx: i32, dy: i32, dz: i32) {
        if self.face_count == 0 || other.face_count == 0 {
            return;
        }
        other.ensure_bounds();
        let other_min_y = other.min_y;
        let other_max_y = other.max_y;
        let other_min_x = other.min_x;
        let other_max_x = other.max_x;
        let other_min_z = other.min_z;
        let other_max_z = other.max_z;
        for vertex in 0..self.vertex_count as usize {
            let y = self.vy[vertex] - dy;
            if y < other_min_y || y > other_max_y {
                continue;
            }
            let x = self.vx[vertex] - dx;
            if x < other_min_x || x > other_max_x {
                continue;
            }
            let z = self.vz[vertex] - dz;
            if z < other_min_z || z > other_max_z {
                continue;
            }
            let mut own_unique: i32 = -1;
            let own_first_slot = self.vertex_offsets[vertex] as usize;
            let own_end_slot = self.vertex_offsets[vertex + 1] as usize;
            let mut own_slot = own_first_slot;
            while own_slot < own_end_slot && self.vertex_slots[own_slot] != 0 {
                own_unique = (i32::from(self.vertex_slots[own_slot]) & 0xFFFF) - 1;
                if self.ncount[own_unique as usize] != 0 {
                    break;
                }
                own_slot += 1;
            }
            if own_unique == -1 {
                continue;
            }
            for other_vertex in 0..other.vertex_count as usize {
                if other.vx[other_vertex] == x
                    && other.vz[other_vertex] == z
                    && other.vy[other_vertex] == y
                {
                    let mut other_unique: i32 = -1;
                    let other_first_slot = other.vertex_offsets[other_vertex] as usize;
                    let other_end_slot = other.vertex_offsets[other_vertex + 1] as usize;
                    let mut other_slot = other_first_slot;
                    while other_slot < other_end_slot && other.vertex_slots[other_slot] != 0 {
                        other_unique = (i32::from(other.vertex_slots[other_slot]) - 1) & 0xFFFF;
                        if other.ncount[other_unique as usize] != 0 {
                            break;
                        }
                        other_slot += 1;
                    }
                    if other_unique == -1 {
                        continue;
                    }
                    if self.merged.is_none() {
                        self.merged = Some(MergedNormals {
                            nx: self.nx.clone(),
                            ny: self.ny.clone(),
                            nz: self.nz.clone(),
                            count: self.ncount.clone(),
                        });
                    }
                    if other.merged.is_none() {
                        other.merged = Some(MergedNormals {
                            nx: other.nx.clone(),
                            ny: other.ny.clone(),
                            nz: other.nz.clone(),
                            count: other.ncount.clone(),
                        });
                    }
                    let own_nx = self.nx[own_unique as usize];
                    let own_ny = self.ny[own_unique as usize];
                    let own_nz = self.nz[own_unique as usize];
                    let own_count = self.ncount[own_unique as usize];
                    {
                        let other_merged = other.merged.as_mut().expect("merged");
                        for slot in other_first_slot..other_end_slot {
                            let index = i32::from(other.vertex_slots[slot]) - 1;
                            if index == -1 {
                                break;
                            }
                            let Ok(s) = usize::try_from(index) else {
                                return;
                            };
                            if other_merged.count[s] != 0 {
                                other_merged.nx[s] = other_merged.nx[s].wrapping_add(own_nx);
                                other_merged.ny[s] = other_merged.ny[s].wrapping_add(own_ny);
                                other_merged.nz[s] = other_merged.nz[s].wrapping_add(own_nz);
                                other_merged.count[s] =
                                    other_merged.count[s].wrapping_add(own_count);
                            }
                        }
                    }
                    let other_nx = other.nx[other_unique as usize];
                    let other_ny = other.ny[other_unique as usize];
                    let other_nz = other.nz[other_unique as usize];
                    let other_count = other.ncount[other_unique as usize];
                    {
                        let own_merged = self.merged.as_mut().expect("merged");
                        let mut merge_slot = own_first_slot;
                        while merge_slot < own_end_slot && self.vertex_slots[merge_slot] != 0 {
                            let merge_index =
                                ((i32::from(self.vertex_slots[merge_slot]) & 0xFFFF) - 1) as usize;
                            if own_merged.count[merge_index] != 0 {
                                own_merged.nx[merge_index] =
                                    own_merged.nx[merge_index].wrapping_add(other_nx);
                                own_merged.ny[merge_index] =
                                    own_merged.ny[merge_index].wrapping_add(other_ny);
                                own_merged.nz[merge_index] =
                                    own_merged.nz[merge_index].wrapping_add(other_nz);
                                own_merged.count[merge_index] =
                                    own_merged.count[merge_index].wrapping_add(other_count);
                            }
                            merge_slot += 1;
                        }
                    }
                }
            }
        }
    }
}
