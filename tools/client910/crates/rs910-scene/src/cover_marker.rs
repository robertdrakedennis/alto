//! NPC cover markers: pairs each deferred NPC that has a cover marker with the actor owning its
//! tile, then places those markers in a grid over the owner during the 2D element pass and
//! records a clickbox for each.
//!
//! The owner search reads the scene tile's entity list as the actor push left it: per plane,
//! the tile-centred, non-deferred actors with draw priority >= 0 in row order (high-resolution
//! players, then NPC slots), appended to every tile of their clamped tile-bounds rectangle.
//! Moving actors join the scene only later, after the pairing.

/// One actor row (players, then NPC slots).
#[derive(Clone, Debug, PartialEq)]
pub struct Actor {
    /// The player index, plus 2048 for an NPC.
    pub key: i32,
    pub npc: bool,
    pub level: i32,
    /// Fine position x/z, truncated to integers.
    pub fine: [i32; 2],
    pub size: i32,
    /// Draw priority.
    pub priority: i32,
    /// Whether the scene add is deferred.
    pub deferred: bool,
    /// The cover marker id (-1 for players and NPCs without one).
    pub cover_marker: i32,
}

impl Actor {
    fn centred(&self) -> bool {
        let offset = if self.size & 1 == 0 { 0 } else { 256 };
        self.fine[0] & 0x1ff == offset && self.fine[1] & 0x1ff == offset
    }
    /// Whether the actor is added to the scene tile lists before the pairing.
    fn in_tile_lists(&self) -> bool {
        self.priority >= 0 && self.centred() && !self.deferred
    }
    /// `(size - 1) * 256 + margin` footprint in tiles: [min x, min z, max x, max z].
    fn footprint(&self, margin: i32) -> [i32; 4] {
        let r = (self.size - 1) * 256 + margin;
        [
            (self.fine[0] - r) >> 9,
            (self.fine[1] - r) >> 9,
            (self.fine[0] + r) >> 9,
            (self.fine[1] + r) >> 9,
        ]
    }
}

/// The actor in the tile's list whose 252-margin footprint covers the tile with the largest
/// remaining area; the first such actor wins ties. `size` is `[max tile x, max tile z]`: each
/// actor's tile-bounds rectangle (240 margin) is clamped into the scene. A tile outside the
/// scene has no list.
fn tile_owner(rows: &[Actor], size: [i32; 2], level: i32, x: i32, z: i32) -> Option<usize> {
    if x < 0 || z < 0 || x >= size[0] || z >= size[1] {
        return None;
    }
    let mut best = None;
    let mut best_area = -1;
    for (i, row) in rows.iter().enumerate() {
        if row.level != level || !row.in_tile_lists() {
            continue;
        }
        let [min_x, min_z, max_x, max_z] = row.footprint(240);
        let listed = min_x.clamp(0, size[0] - 1) <= x
            && x <= max_x.clamp(0, size[0] - 1)
            && min_z.clamp(0, size[1] - 1) <= z
            && z <= max_z.clamp(0, size[1] - 1);
        if !listed {
            continue;
        }
        let [x0, z0, x1, z1] = row.footprint(252);
        if x0 <= x && z0 <= z && x1 >= x && z1 >= z {
            let area = (x1 + 1 - x) * (z1 + 1 - z);
            if area > best_area {
                best = Some(i);
                best_area = area;
            }
        }
    }
    best
}

/// In-place quicksort of `keys` carrying `values`. The partitioning is deliberately exact
/// (pivot swapped to the end, `i & 1` bias) and not stable: equal keys may end up reordered.
fn quicksort(keys: &mut [i32], values: &mut [i32], lo: i32, hi: i32) {
    if lo >= hi {
        return;
    }
    let (l, h) = (lo as usize, hi as usize);
    let mid = ((lo + hi) / 2) as usize;
    let mut store = l;
    let pivot = keys[mid];
    keys.swap(mid, h);
    let carried = values[mid];
    values.swap(mid, h);
    let bias = if pivot == i32::MAX { 0 } else { 1 };
    for i in l..h {
        if keys[i] < (i as i32 & bias) + pivot {
            keys.swap(i, store);
            values.swap(i, store);
            store += 1;
        }
    }
    keys[h] = keys[store];
    keys[store] = pivot;
    values[h] = values[store];
    values[store] = carried;
    quicksort(keys, values, lo, store as i32 - 1);
    quicksort(keys, values, store as i32 + 1, hi);
}

/// One marker placement: the owner and marker as row indices, and the owner's grid slot
/// (its marker count after decrementing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub owner: usize,
    pub marker: usize,
    pub slot: i32,
}

/// Pair the markers with their owners, then number each owner's grid slots downwards in draw
/// order. `rows` are in actor row order; `size` is the scene size.
pub fn pairs(rows: &[Actor], size: [i32; 2]) -> Vec<Pair> {
    // Each owner's marker count starts at zero.
    let mut counts = vec![0i32; rows.len()];
    let mut owners: Vec<i32> = Vec::new(); // owner actor keys
    let mut markers: Vec<i32> = Vec::new(); // marker actor keys
    for row in rows.iter().filter(|r| r.npc) {
        if !row.deferred || row.cover_marker == -1 {
            continue;
        }
        let [x, z, ..] = row.footprint(252);
        let Some(owner) = tile_owner(rows, size, row.level, x, z) else {
            continue;
        };
        let key = rows[owner].key;
        if counts[owner] == 0 && rows[owner].cover_marker != -1 {
            owners.push(key);
            markers.push(key);
            counts[owner] += 1;
        }
        owners.push(key);
        markers.push(row.key);
        counts[owner] += 1;
    }
    let n = owners.len() as i32;
    quicksort(&mut markers, &mut owners, 0, n - 1);
    let index = |key: i32| rows.iter().position(|r| r.key == key);
    owners
        .iter()
        .zip(&markers)
        .filter_map(|(&o, &m)| {
            let owner = index(o)?;
            let marker = index(m)?;
            counts[owner] -= 1;
            Some(Pair {
                owner,
                marker,
                slot: counts[owner],
            })
        })
        .collect()
}

/// The pairing the last ordinary scene frame drew, kept for the cutscene frames that follow.
///
/// Entity placement skips the pairing while a cutscene draws, but the 2D element pass still
/// walks the last pairing: the markers stay on screen, and each owner's slot counter keeps
/// counting down below zero, so the grid position walks up and left with every frame.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    /// `(owner key, marker key)` in draw order.
    entries: Vec<(i32, i32)>,
    /// Each owner's slot counter; owners not listed sit at zero.
    counters: std::collections::HashMap<i32, i32>,
}

impl Memory {
    /// Keep the pairing of an ordinary frame. Every owner's counter has run down to zero
    /// by the end of that frame's draw.
    pub fn record(&mut self, rows: &[Actor], pairs: &[Pair]) {
        self.entries = pairs
            .iter()
            .map(|p| (rows[p.owner].key, rows[p.marker].key))
            .collect();
        self.counters.clear();
    }

    /// Forget the pairing (a new session).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.counters.clear();
    }

    /// One cutscene frame's draws: `(owner key, marker key, slot)` in draw order, each owner's
    /// counter stepping one lower per draw.
    pub fn replay(&mut self) -> Vec<(i32, i32, i32)> {
        self.entries
            .iter()
            .map(|&(owner, marker)| {
                let counter = self.counters.entry(owner).or_insert(0);
                *counter -= 1;
                (owner, marker, *counter)
            })
            .collect()
    }
}

/// Marker placement: the owner's projection (at height `size * 256`, relative to the viewport)
/// shifted by the viewport origin, then an 18-pixel grid cell per slot, four to a column.
/// Returns the sprite position; the clickbox is `[x, y, x + 16, y + 16]` and the owner's own
/// marker gets a rectangle outline `(x - 1, y - 1, 18, 18)` in colour -256.
pub fn position(projection: [f32; 2], viewport: [i32; 2], slot: i32) -> [i32; 2] {
    let x = (projection[0] + viewport[0] as f32 - 18.0) as i32;
    let y = (projection[1] + viewport[1] as f32 - 16.0 - 54.0) as i32;
    [slot / 4 * 18 + x, slot % 4 * 18 + y]
}
