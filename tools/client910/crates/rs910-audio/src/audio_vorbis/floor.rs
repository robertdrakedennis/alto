//! Vorbis floor type 1: a piecewise-linear spectral envelope. Each block
//! reads a handful of Y values per channel, expands them to amplitude
//! points, and renders the curve into the residue vector as a multiplier.
//! Floor type 0 is not supported by the client's streams.

use super::bits::{bit_length, BitReader};
use super::codebook::Codebook;
use anyhow::{bail, ensure, Result};

/// The amplitude range of each of the four floor multipliers.
const RANGES: [i32; 4] = [256, 128, 86, 64];

/// The Vorbis `floor1_inverse_dB_table`: 256 amplitude steps of 0.5 dB.
#[rustfmt::skip]
const INVERSE_DB_TABLE: [f32; 256] = [1.0649863E-7, 1.1341951E-7, 1.2079015E-7, 1.2863978E-7, 1.369995E-7, 1.459025E-7, 1.5538409E-7, 1.6548181E-7, 1.7623574E-7, 1.8768856E-7, 1.998856E-7, 2.128753E-7, 2.2670913E-7, 2.4144197E-7, 2.5713223E-7, 2.7384212E-7, 2.9163792E-7, 3.1059022E-7, 3.307741E-7, 3.5226967E-7, 3.7516213E-7, 3.995423E-7, 4.255068E-7, 4.5315863E-7, 4.8260745E-7, 5.1397E-7, 5.4737063E-7, 5.829419E-7, 6.208247E-7, 6.611694E-7, 7.041359E-7, 7.4989464E-7, 7.98627E-7, 8.505263E-7, 9.057983E-7, 9.646621E-7, 1.0273513E-6, 1.0941144E-6, 1.1652161E-6, 1.2409384E-6, 1.3215816E-6, 1.4074654E-6, 1.4989305E-6, 1.5963394E-6, 1.7000785E-6, 1.8105592E-6, 1.9282195E-6, 2.053526E-6, 2.1869757E-6, 2.3290977E-6, 2.4804558E-6, 2.6416496E-6, 2.813319E-6, 2.9961443E-6, 3.1908505E-6, 3.39821E-6, 3.619045E-6, 3.8542307E-6, 4.1047006E-6, 4.371447E-6, 4.6555283E-6, 4.958071E-6, 5.280274E-6, 5.623416E-6, 5.988857E-6, 6.3780467E-6, 6.7925284E-6, 7.2339453E-6, 7.704048E-6, 8.2047E-6, 8.737888E-6, 9.305725E-6, 9.910464E-6, 1.0554501E-5, 1.1240392E-5, 1.1970856E-5, 1.2748789E-5, 1.3577278E-5, 1.4459606E-5, 1.5399271E-5, 1.6400005E-5, 1.7465769E-5, 1.8600793E-5, 1.9809577E-5, 2.1096914E-5, 2.2467912E-5, 2.3928002E-5, 2.5482977E-5, 2.7139005E-5, 2.890265E-5, 3.078091E-5, 3.2781227E-5, 3.4911533E-5, 3.718028E-5, 3.9596467E-5, 4.2169668E-5, 4.491009E-5, 4.7828602E-5, 5.0936775E-5, 5.424693E-5, 5.7772202E-5, 6.152657E-5, 6.552491E-5, 6.9783084E-5, 7.4317984E-5, 7.914758E-5, 8.429104E-5, 8.976875E-5, 9.560242E-5, 1.0181521E-4, 1.0843174E-4, 1.1547824E-4, 1.2298267E-4, 1.3097477E-4, 1.3948625E-4, 1.4855085E-4, 1.5820454E-4, 1.6848555E-4, 1.7943469E-4, 1.9109536E-4, 2.0351382E-4, 2.167393E-4, 2.3082423E-4, 2.4582449E-4, 2.6179955E-4, 2.7881275E-4, 2.9693157E-4, 3.1622787E-4, 3.3677815E-4, 3.5866388E-4, 3.8197188E-4, 4.0679457E-4, 4.3323037E-4, 4.613841E-4, 4.913675E-4, 5.2329927E-4, 5.573062E-4, 5.935231E-4, 6.320936E-4, 6.731706E-4, 7.16917E-4, 7.635063E-4, 8.1312325E-4, 8.6596457E-4, 9.2223985E-4, 9.821722E-4, 0.0010459992, 0.0011139743, 0.0011863665, 0.0012634633, 0.0013455702, 0.0014330129, 0.0015261382, 0.0016253153, 0.0017309374, 0.0018434235, 0.0019632196, 0.0020908006, 0.0022266726, 0.0023713743, 0.0025254795, 0.0026895993, 0.0028643848, 0.0030505287, 0.003248769, 0.0034598925, 0.0036847359, 0.0039241905, 0.0041792067, 0.004450795, 0.004740033, 0.005048067, 0.0053761187, 0.005725489, 0.0060975635, 0.0064938175, 0.0069158226, 0.0073652514, 0.007843887, 0.008353627, 0.008896492, 0.009474637, 0.010090352, 0.01074608, 0.011444421, 0.012188144, 0.012980198, 0.013823725, 0.014722068, 0.015678791, 0.016697686, 0.017782796, 0.018938422, 0.020169148, 0.021479854, 0.022875736, 0.02436233, 0.025945531, 0.027631618, 0.029427277, 0.031339627, 0.03337625, 0.035545226, 0.037855156, 0.0403152, 0.042935107, 0.045725275, 0.048696756, 0.05186135, 0.05523159, 0.05882085, 0.062643364, 0.06671428, 0.07104975, 0.075666964, 0.08058423, 0.08582105, 0.09139818, 0.097337745, 0.1036633, 0.11039993, 0.11757434, 0.12521498, 0.13335215, 0.14201812, 0.15124726, 0.16107617, 0.1715438, 0.18269168, 0.19456401, 0.20720787, 0.22067343, 0.23501402, 0.25028655, 0.26655158, 0.28387362, 0.3023213, 0.32196787, 0.34289113, 0.36517414, 0.3889052, 0.41417846, 0.44109413, 0.4697589, 0.50028646, 0.53279793, 0.5674221, 0.6042964, 0.64356697, 0.6853896, 0.72993004, 0.777365, 0.8278826, 0.88168305, 0.9389798, 1.0];

fn inverse_db(index: i32) -> Result<f32> {
    match usize::try_from(index)
        .ok()
        .and_then(|i| INVERSE_DB_TABLE.get(i))
    {
        Some(value) => Ok(*value),
        None => bail!("floor amplitude index {index} outside the dB table"),
    }
}

/// The per-channel working points of one block: the sorted X positions, the
/// decoded Y amplitudes and which points are used.
#[derive(Clone, Debug)]
struct Curve {
    x: Vec<i32>,
    y: Vec<i32>,
    used: Vec<bool>,
}

impl Curve {
    fn new(points: usize) -> Self {
        Self {
            x: vec![0; points],
            y: vec![0; points],
            used: vec![false; points],
        }
    }

    /// Sort the points by X (a quicksort over the three parallel arrays).
    fn sort(&mut self, low: i32, high: i32) {
        if low >= high {
            return;
        }
        let (low_at, high_at) = (low as usize, high as usize);
        let mut pivot_at = low_at;
        let pivot_x = self.x[low_at];
        let pivot_y = self.y[low_at];
        let pivot_used = self.used[low_at];
        for i in low_at + 1..=high_at {
            let x = self.x[i];
            if x < pivot_x {
                self.x[pivot_at] = x;
                self.y[pivot_at] = self.y[i];
                self.used[pivot_at] = self.used[i];
                pivot_at += 1;
                self.x[i] = self.x[pivot_at];
                self.y[i] = self.y[pivot_at];
                self.used[i] = self.used[pivot_at];
            }
        }
        self.x[pivot_at] = pivot_x;
        self.y[pivot_at] = pivot_y;
        self.used[pivot_at] = pivot_used;
        self.sort(low, pivot_at as i32 - 1);
        self.sort(pivot_at as i32 + 1, high);
    }
}

/// A floor type 1 configuration plus its per-channel working state.
#[derive(Clone, Debug)]
pub(super) struct Floor {
    /// The X position of every point (the first two are 0 and the range).
    x_list: Vec<i32>,
    multiplier: i32,
    /// The class of each partition.
    partition_classes: Vec<i32>,
    /// Per class: dimensions, subclass bits and master book.
    class_dimensions: Vec<i32>,
    class_subclass_bits: Vec<i32>,
    class_master_books: Vec<i32>,
    /// Per class and subclass: the book that codes the Y value (-1: none).
    subclass_books: Vec<Vec<i32>>,
    /// Per channel: the working points of the current block.
    curves: Vec<Curve>,
    /// Per channel: whether the current block carries a floor.
    nonzero: Vec<bool>,
}

impl Floor {
    /// Parse one floor from the setup header.
    pub(super) fn parse(bits: &mut BitReader<'_>, channels: i32) -> Result<Self> {
        let kind = bits.read(16);
        ensure!(kind == 1, "floor type {kind} is not supported");
        let channels = channels.max(0) as usize;
        let partitions = bits.read(5) as usize;
        let mut class_count = 0_i32;
        let mut partition_classes = vec![0; partitions];
        for class in &mut partition_classes {
            *class = bits.read(4);
            if *class >= class_count {
                class_count = *class + 1;
            }
        }
        let class_count = class_count as usize;
        let mut class_dimensions = vec![0; class_count];
        let mut class_subclass_bits = vec![0; class_count];
        let mut class_master_books = vec![0; class_count];
        let mut subclass_books = vec![Vec::new(); class_count];
        for class in 0..class_count {
            class_dimensions[class] = bits.read(3) + 1;
            let subclass = bits.read(2);
            class_subclass_bits[class] = subclass;
            if subclass != 0 {
                class_master_books[class] = bits.read(8);
            }
            subclass_books[class] = (0..1_usize << subclass).map(|_| bits.read(8) - 1).collect();
        }
        let multiplier = bits.read(2) + 1;
        let range_bits = bits.read(4);
        let mut points = 2_usize;
        for &class in &partition_classes {
            points += class_dimensions[class as usize] as usize;
        }
        let mut x_list = vec![0; points];
        x_list[1] = 1 << range_bits;
        let mut next = 2;
        for &class in &partition_classes {
            for _ in 0..class_dimensions[class as usize] {
                x_list[next] = bits.read(range_bits);
                next += 1;
            }
        }
        Ok(Self {
            curves: vec![Curve::new(points); channels],
            nonzero: vec![false; channels],
            x_list,
            multiplier,
            partition_classes,
            class_dimensions,
            class_subclass_bits,
            class_master_books,
            subclass_books,
        })
    }

    /// The nearest point before `index` with a smaller X.
    fn low_neighbor(list: &[i32], index: usize) -> usize {
        let value = list[index];
        let mut best = usize::MAX;
        let mut best_value = i32::MIN;
        for (i, &candidate) in list.iter().enumerate().take(index) {
            if candidate < value && candidate > best_value {
                best = i;
                best_value = candidate;
            }
        }
        best
    }

    /// The nearest point before `index` with a larger X.
    fn high_neighbor(list: &[i32], index: usize) -> usize {
        let value = list[index];
        let mut best = usize::MAX;
        let mut best_value = i32::MAX;
        for (i, &candidate) in list.iter().enumerate().take(index) {
            if candidate > value && candidate < best_value {
                best = i;
                best_value = candidate;
            }
        }
        best
    }

    /// The Y of the straight line through two points at `x`.
    fn render_point(x0: i32, y0: i32, x1: i32, y1: i32, x: i32) -> Result<i32> {
        let dy = y1 - y0;
        let dx = x1 - x0;
        ensure!(dx != 0, "floor segment has zero width");
        let ady = dy.abs();
        let offset = (x - x0).wrapping_mul(ady) / dx;
        Ok(if dy < 0 { y0 - offset } else { y0 + offset })
    }

    /// Multiply `window[x0..x1]` by the dB-table amplitude of the line from
    /// `(x0, y0)` to `(x1, y1)`, drawn with Bresenham steps.
    fn render_line(
        x0: i32,
        y0: i32,
        mut x1: i32,
        y1: i32,
        window: &mut [f32],
        limit: i32,
    ) -> Result<()> {
        let dy = y1 - y0;
        let adx = x1 - x0;
        ensure!(adx != 0, "floor line has zero width");
        let ady = dy.abs();
        let base = dy / adx;
        let mut y = y0;
        let mut err = 0;
        let sign_step = if dy < 0 { base - 1 } else { base + 1 };
        let step = ady - base.abs() * adx;
        let Some(first) = window.get_mut(x0 as usize) else {
            bail!("floor x {x0} lies beyond the window");
        };
        *first *= inverse_db(y0)?;
        if x1 > limit {
            x1 = limit;
        }
        for x in x0 + 1..x1 {
            err += step;
            if err >= adx {
                err -= adx;
                y += sign_step;
            } else {
                y += base;
            }
            window[x as usize] *= inverse_db(y)?;
        }
        Ok(())
    }

    /// Read this block's floor for `channel`. Returns whether the channel
    /// carries a floor (a channel without one is silent).
    pub(super) fn read_channel(
        &mut self,
        channel: usize,
        bits: &mut BitReader<'_>,
        books: &[Codebook],
    ) -> Result<bool> {
        let nonzero = bits.read_bit() != 0;
        self.nonzero[channel] = nonzero;
        if !nonzero {
            return Ok(false);
        }
        let points = self.x_list.len();
        let curve = &mut self.curves[channel];
        curve.x[..points].copy_from_slice(&self.x_list);
        let range = RANGES[(self.multiplier - 1) as usize];
        let range_bits = bit_length(range - 1);
        curve.y[0] = bits.read(range_bits);
        curve.y[1] = bits.read(range_bits);
        let mut offset = 2;
        for &class in &self.partition_classes {
            let class = class as usize;
            let dimensions = self.class_dimensions[class];
            let subclass_bits = self.class_subclass_bits[class];
            let mask = (1 << subclass_bits) - 1;
            let mut selector = 0_i32;
            if subclass_bits > 0 {
                selector = book(books, self.class_master_books[class])?.decode_scalar(bits)?;
            }
            for _ in 0..dimensions {
                let book_id = self.subclass_books[class][(selector & mask) as usize];
                selector = ((selector as u32) >> subclass_bits) as i32;
                curve.y[offset] = if book_id >= 0 {
                    book(books, book_id)?.decode_scalar(bits)?
                } else {
                    0
                };
                offset += 1;
            }
        }
        Ok(true)
    }

    /// Turn the decoded Y offsets of `channel` into absolute amplitudes by
    /// predicting each point from its neighbours (the specification's
    /// "amplitude value synthesis", step 1 and 2).
    pub(super) fn compute_amplitudes(&mut self, channel: usize) -> Result<()> {
        if !self.nonzero[channel] {
            return Ok(());
        }
        let points = self.x_list.len();
        let range = RANGES[(self.multiplier - 1) as usize];
        let curve = &mut self.curves[channel];
        curve.used[1] = true;
        curve.used[0] = true;
        for i in 2..points {
            let low = Self::low_neighbor(&self.x_list, i);
            let high = Self::high_neighbor(&self.x_list, i);
            ensure!(
                low != usize::MAX && high != usize::MAX,
                "floor point has no neighbour"
            );
            let predicted = Self::render_point(
                self.x_list[low],
                curve.y[low],
                self.x_list[high],
                curve.y[high],
                self.x_list[i],
            )?;
            let value = curve.y[i];
            let high_room = range - predicted;
            let room = high_room.min(predicted) << 1;
            if value == 0 {
                curve.used[i] = false;
                curve.y[i] = predicted;
            } else {
                curve.used[high] = true;
                curve.used[low] = true;
                curve.used[i] = true;
                curve.y[i] = if value >= room {
                    if high_room > predicted {
                        value - predicted + predicted
                    } else {
                        predicted - value + high_room - 1
                    }
                } else if value & 1 == 0 {
                    (value >> 1) + predicted
                } else {
                    predicted - ((value + 1) >> 1)
                };
            }
        }
        for i in 0..points {
            if !curve.used[i] {
                curve.y[i] = -1;
            }
        }
        Ok(())
    }

    /// Render the floor curve of `channel` into `window[..limit]`, scaling
    /// the residue spectrum by the envelope.
    pub(super) fn render(&mut self, channel: usize, window: &mut [f32], limit: i32) -> Result<()> {
        if !self.nonzero[channel] {
            return Ok(());
        }
        let points = self.x_list.len();
        let multiplier = self.multiplier;
        let curve = &mut self.curves[channel];
        curve.sort(0, points as i32 - 1);
        let mut last_x = 0;
        let mut last_y = curve.y[0] * multiplier;
        for i in 1..points {
            if curve.y[i] >= 0 {
                let x = curve.x[i];
                let y = curve.y[i] * multiplier;
                Self::render_line(last_x, last_y, x, y, window, limit)?;
                last_x = x;
                last_y = y;
            }
        }
        let tail = inverse_db(last_y)?;
        for x in last_x..limit {
            window[x as usize] *= tail;
        }
        Ok(())
    }
}

/// Look up codebook `id`.
pub(super) fn book(books: &[Codebook], id: i32) -> Result<&Codebook> {
    match usize::try_from(id).ok().and_then(|i| books.get(i)) {
        Some(book) => Ok(book),
        None => bail!("codebook {id} out of range ({} books)", books.len()),
    }
}
