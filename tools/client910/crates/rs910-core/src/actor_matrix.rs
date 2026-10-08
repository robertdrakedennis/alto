//! Column-major 4x3 actor matrices; the float expression order matches the
//! reference client's.
#[derive(Clone, Copy, Debug)]
pub struct Matrix(pub [f32; 12]);
impl Default for Matrix {
    fn default() -> Self {
        Self([1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.])
    }
}
impl Matrix {
    /// Actor transform: quaternion rotation, then translation plus a vertical offset.
    pub fn actor(rotation: [f32; 4], position: [f32; 3], y_offset: f32) -> Self {
        let mut m = Self::default();
        m.rotate_quaternion(rotation);
        for (dst, p) in m.0[9..12].iter_mut().zip(position) {
            *dst += p;
        }
        m.0[10] += y_offset;
        m
    }
    /// Scale-rotate-translate transform:
    /// scale, quaternion rotation, translation.
    pub fn srt(rotation: [f32; 4], scale: [f32; 3], position: [f32; 3]) -> Self {
        let mut m = Self([
            scale[0], 0., 0., 0., scale[1], 0., 0., 0., scale[2], 0., 0., 0.,
        ]);
        m.rotate_quaternion(rotation);
        for (dst, p) in m.0[9..12].iter_mut().zip(position) {
            *dst += p;
        }
        m
    }
    /// Rotate by a quaternion given
    /// as `(x, y, z, w)`.
    fn rotate_quaternion(&mut self, rotation: [f32; 4]) {
        let [x, y, z, w] = rotation;
        let (xx, xy, xz, xw, yy, yz, yw, zz, zw) = (
            x * x,
            x * y,
            x * z,
            x * w,
            y * y,
            y * z,
            y * w,
            z * z,
            z * w,
        );
        let r = [
            1. - (yy + zz) * 2.,
            (xy + zw) * 2.,
            (xz - yw) * 2.,
            (xy - zw) * 2.,
            1. - (xx + zz) * 2.,
            (xw + yz) * 2.,
            (xz + yw) * 2.,
            (yz - xw) * 2.,
            1. - (xx + yy) * 2.,
        ];
        for c in 0..4 {
            let i = c * 3;
            let (a, b, d) = (self.0[i], self.0[i + 1], self.0[i + 2]);
            self.0[i] = d * r[6] + r[0] * a + r[3] * b;
            self.0[i + 1] = d * r[7] + r[1] * a + r[4] * b;
            self.0[i + 2] = d * r[8] + r[2] * a + r[5] * b;
        }
    }
    pub fn entries(&self) -> [f32; 16] {
        let mut out = [0.; 16];
        for c in 0..4 {
            for r in 0..3 {
                out[c * 4 + r] = self.0[c * 3 + r];
            }
        }
        out[15] = 1.;
        out
    }
    pub fn vector(&self, v: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| v[2] * self.0[6 + i] + self.0[3 + i] * v[1] + self.0[i] * v[0])
    }
    pub fn axis(x: f32, y: f32, z: f32, angle: f32) -> Self {
        let c = (angle as f64).cos() as f32;
        let s = (angle as f64).sin() as f32;
        Self([
            x * x * (1. - c) + c,
            x * y * (1. - c) + z * s,
            x * z * (1. - c) + -y * s,
            x * y * (1. - c) + -z * s,
            y * y * (1. - c) + c,
            y * z * (1. - c) + x * s,
            x * z * (1. - c) + y * s,
            y * z * (1. - c) + -x * s,
            z * z * (1. - c) + c,
            0.,
            0.,
            0.,
        ])
    }
    pub fn rotate(&mut self, x: f32, y: f32, z: f32, angle: f32) {
        let r = Self::axis(x, y, z, angle).0;
        for c in 0..4 {
            let i = c * 3;
            let a = self.0[i];
            let b = self.0[i + 1];
            let d = self.0[i + 2];
            self.0[i] = d * r[6] + r[0] * a + r[3] * b;
            self.0[i + 1] = d * r[7] + r[1] * a + r[4] * b;
            self.0[i + 2] = d * r[8] + r[2] * a + r[5] * b;
        }
    }
    pub fn inverse(&self) -> Self {
        let a = self.0;
        let mut out = Self([
            a[0], a[3], a[6], a[1], a[4], a[7], a[2], a[5], a[8], 0., 0., 0.,
        ]);
        for row in 0..3 {
            out.0[9 + row] = -(out.0[6 + row] * a[11] + out.0[3 + row] * a[10] + out.0[row] * a[9]);
        }
        out
    }
    pub fn point(&self, x: f32, y: f32, z: f32) -> [f32; 3] {
        std::array::from_fn(|i| {
            self.0[6 + i] * z + self.0[3 + i] * y + self.0[i] * x + self.0[9 + i]
        })
    }
    pub fn bas(t: &[i32]) -> Self {
        let mut m = Self::default();
        for (i, axis) in [(5, [0., 0., 1.]), (3, [1., 0., 0.]), (4, [0., 1., 0.])] {
            if t[i] != 0 {
                m.rotate(axis[0], axis[1], axis[2], crate::trig::radians(t[i] << 3));
            }
        }
        for (dst, &v) in m.0[9..12].iter_mut().zip(t) {
            *dst += v as f32;
        }
        m
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_scales_then_rotates_then_translates() {
        // A quarter turn
        // about y (q = (0, sin 45, 0, cos 45)) of the scaled +x axis lands
        // on -z, then the translation applies.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let m = Matrix::srt([0.0, h, 0.0, h], [2.0, 1.0, 1.0], [10.0, 20.0, 30.0]);
        let p = m.point(1.0, 0.0, 0.0);
        assert!((p[0] - 10.0).abs() < 1e-5, "{p:?}");
        assert!((p[1] - 20.0).abs() < 1e-5, "{p:?}");
        assert!((p[2] - 28.0).abs() < 1e-5, "{p:?}");
        // Identity rotation/scale is the translation-only actor matrix.
        let a = Matrix::srt([0.0, 0.0, 0.0, 1.0], [1.0; 3], [1.0, 2.0, 3.0]).entries();
        let b = Matrix::actor([0.0, 0.0, 0.0, 1.0], [1.0, 2.0, 3.0], 0.0).entries();
        assert_eq!(a, b);
    }
}
