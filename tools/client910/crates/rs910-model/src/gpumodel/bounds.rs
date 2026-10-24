//! Extents of a lit model: the bounding box, the horizontal and full radii
//! and the frozen height. The box is cached until a transform invalidates it.

use super::GpuModel;

impl GpuModel {
    /// `(min_y, max_y, horizontal_radius)` for a frustum early-out: the cached
    /// bounds, or a fresh measurement (not cached) when they are stale.
    #[must_use]
    pub fn draw_bounds(&self) -> (i32, i32, i32) {
        if self.bounds_valid {
            return (self.min_y, self.max_y, self.horizontal_radius);
        }
        let b = self.measure_bounds();
        (b[2], b[3], b[6])
    }

    /// Measures `[min_x, max_x, min_y, max_y, min_z, max_z,
    /// horizontal_radius, radius]` over the vertices.
    fn measure_bounds(&self) -> [i32; 8] {
        let mut min_x = 32767;
        let mut min_y = 32767;
        let mut min_z = 32767;
        let mut max_x = -32768;
        let mut max_y = -32768;
        let mut max_z = -32768;
        let mut horizontal_squared = 0_i32;
        let mut radius_squared = 0_i32;
        for i in 0..self.vertex_count as usize {
            let x = self.vx[i];
            let y = self.vy[i];
            let z = self.vz[i];
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            min_z = min_z.min(z);
            max_z = max_z.max(z);
            let horizontal = x.wrapping_mul(x).wrapping_add(z.wrapping_mul(z));
            if horizontal > horizontal_squared {
                horizontal_squared = horizontal;
            }
            let full = y
                .wrapping_mul(y)
                .wrapping_add(x.wrapping_mul(x))
                .wrapping_add(z.wrapping_mul(z));
            if full > radius_squared {
                radius_squared = full;
            }
        }
        [
            min_x,
            max_x,
            min_y,
            max_y,
            min_z,
            max_z,
            (f64::from(horizontal_squared).sqrt() + 0.99) as i32,
            (f64::from(radius_squared).sqrt() + 0.99) as i32,
        ]
    }

    fn compute_bounds(&mut self) {
        [
            self.min_x,
            self.max_x,
            self.min_y,
            self.max_y,
            self.min_z,
            self.max_z,
            self.horizontal_radius,
            self.radius,
        ] = self.measure_bounds();
        self.bounds_valid = true;
    }

    /// `[min_x, max_x, min_y, max_y, min_z, max_z, horizontal_radius]` when
    /// the cached bounds are current, without measuring.
    #[must_use]
    pub fn cached_bounds(&self) -> Option<[i32; 7]> {
        self.bounds_valid.then_some([
            self.min_x,
            self.max_x,
            self.min_y,
            self.max_y,
            self.min_z,
            self.max_z,
            self.horizontal_radius,
        ])
    }

    pub(super) fn ensure_bounds(&mut self) {
        if !self.bounds_valid {
            self.compute_bounds();
        }
    }

    pub fn horizontal_radius(&mut self) -> i32 {
        self.ensure_bounds();
        self.horizontal_radius
    }

    pub fn radius(&mut self) -> i32 {
        self.ensure_bounds();
        self.radius
    }

    pub fn min_x(&mut self) -> i32 {
        self.ensure_bounds();
        self.min_x
    }

    pub fn max_x(&mut self) -> i32 {
        self.ensure_bounds();
        self.max_x
    }

    /// The model's top (Y grows downwards).
    pub fn min_y(&mut self) -> i32 {
        self.ensure_bounds();
        self.min_y
    }

    pub fn max_y(&mut self) -> i32 {
        self.ensure_bounds();
        self.max_y
    }

    pub fn min_z(&mut self) -> i32 {
        self.ensure_bounds();
        self.min_z
    }

    pub fn max_z(&mut self) -> i32 {
        self.ensure_bounds();
        self.max_z
    }

    /// The model's height: its top, frozen the first time it is asked.
    pub fn height(&mut self) -> i32 {
        if !self.height_valid {
            self.ensure_bounds();
            self.height = self.min_y;
            self.height_valid = true;
        }
        self.height
    }
}
