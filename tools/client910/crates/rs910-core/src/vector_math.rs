//! `Vec3` / `Quat`: the `float` math the cam2 camera runs on, with the
//! reference evaluation order. Split out of client910's `ui_cam2` (Phase 2.8)
//! so the scene's camera trackables can use it below the UI.

// ---------------------------------------------------------------------------
// `f32` arithmetic with the reference evaluation order kept.
// ---------------------------------------------------------------------------

/// A 3-vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    /// `new Vector3(Float.NaN, Float.NaN, Float.NaN)`: the "not initialised" marker.
    pub const NAN: Self = Self {
        x: f32::NAN,
        y: f32::NAN,
        z: f32::NAN,
    };
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn from_array(v: [f32; 3]) -> Self {
        Self::new(v[0], v[1], v[2])
    }
    /// `reset`.
    pub fn reset(&mut self) {
        *self = Self::ZERO;
    }
    /// `isEqualTo`.
    pub fn is_equal_to(&self, o: &Self) -> bool {
        self.x == o.x && self.y == o.y && self.z == o.z
    }
    /// `negate`.
    pub fn negate(&mut self) {
        self.x = -self.x;
        self.y = -self.y;
        self.z = -self.z;
    }
    /// `normalise`.
    pub fn normalise(&mut self) {
        let inv_length = 1.0 / self.length();
        self.x *= inv_length;
        self.y *= inv_length;
        self.z *= inv_length;
    }
    /// `add(Vector3)`.
    pub fn add(&mut self, o: &Self) {
        self.x += o.x;
        self.y += o.y;
        self.z += o.z;
    }
    /// `addScaled`.
    pub fn add_scaled(&mut self, o: &Self, s: f32) {
        self.x += o.x * s;
        self.y += o.y * s;
        self.z += o.z * s;
    }
    /// static `add(a, b)`.
    pub fn sum(a: &Self, b: &Self) -> Self {
        let mut v = *a;
        v.add(b);
        v
    }
    /// `sub(Vector3)`.
    pub fn sub(&mut self, o: &Self) {
        self.x -= o.x;
        self.y -= o.y;
        self.z -= o.z;
    }
    /// static `sub(a, b)`.
    pub fn diff(a: &Self, b: &Self) -> Self {
        let mut v = *a;
        v.sub(b);
        v
    }
    /// `dot` (z, y, x order).
    pub fn dot(&self, o: &Self) -> f32 {
        self.z * o.z + self.y * o.y + self.x * o.x
    }
    /// `length`: float sum, double sqrt, float result.
    pub fn length(&self) -> f32 {
        f64::from(self.z * self.z + self.y * self.y + self.x * self.x).sqrt() as f32
    }
    /// `abs`.
    pub fn abs(&mut self) {
        if self.x < 0.0 {
            self.x *= -1.0;
        }
        if self.y < 0.0 {
            self.y *= -1.0;
        }
        if self.z < 0.0 {
            self.z *= -1.0;
        }
    }
    /// `multiply(Vector3)`.
    pub fn multiply(&mut self, o: &Self) {
        self.x *= o.x;
        self.y *= o.y;
        self.z *= o.z;
    }
    /// static `multiply(a, b)`.
    pub fn product(a: &Self, b: &Self) -> Self {
        let mut v = *a;
        v.multiply(b);
        v
    }
    /// `multiply(float)`.
    pub fn scale(&mut self, f: f32) {
        self.x *= f;
        self.y *= f;
        self.z *= f;
    }
    /// static `multiply(a, float)`.
    pub fn scaled(a: &Self, f: f32) -> Self {
        let mut v = *a;
        v.scale(f);
        v
    }
    /// `divide(Vector3)`.
    pub fn divide(&mut self, o: &Self) {
        self.x /= o.x;
        self.y /= o.y;
        self.z /= o.z;
    }
    /// static `divide(a, b)`.
    pub fn quotient(a: &Self, b: &Self) -> Self {
        let mut v = *a;
        v.divide(b);
        v
    }
    /// `divide(float)`.
    pub fn divide_by(&mut self, f: f32) {
        self.x /= f;
        self.y /= f;
        self.z /= f;
    }
    /// `rotate(Quaternion)`: `opposite(q) * (x, y, z, 0) * q`, taking
    /// the first three components of the product.
    pub fn rotate(&mut self, q: &Quat) {
        let vector = Quat {
            w: self.x,
            x: self.y,
            y: self.z,
            z: 0.0,
        };
        let mut product = Quat::opposite_of(q);
        product.multiply(&vector);
        product.multiply(q);
        *self = Self::new(product.w, product.x, product.y);
    }
    /// `this = this * (1 - t) + v * t`.
    pub fn blend(&mut self, o: &Self, t: f32) {
        self.scale(1.0 - t);
        self.add(&Self::scaled(o, t));
    }
}

/// A quaternion. The vector part is stored in
/// `w, x, y` and the scalar in `z` (the identity has `z = 1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub w: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Quat {
    pub const IDENTITY: Self = Self {
        w: 0.0,
        x: 0.0,
        y: 0.0,
        z: 1.0,
    };
    /// The uninitialised (all-NaN) quaternion of a look-at orientation.
    pub const NAN: Self = Self {
        w: f32::NAN,
        x: f32::NAN,
        y: f32::NAN,
        z: f32::NAN,
    };
    /// `setToRotation(ax, ay, az, angle)`.
    pub fn set_to_rotation(&mut self, ax: f32, ay: f32, az: f32, angle: f32) {
        let sin_half = f64::from(angle * 0.5).sin() as f32;
        let cos_half = f64::from(angle * 0.5).cos() as f32;
        self.w = ax * sin_half;
        self.x = ay * sin_half;
        self.y = az * sin_half;
        self.z = cos_half;
    }
    pub fn rotation(ax: f32, ay: f32, az: f32, angle: f32) -> Self {
        let mut q = Self::IDENTITY;
        q.set_to_rotation(ax, ay, az, angle);
        q
    }
    /// `setToRotation(Vector3, angle)`.
    pub fn rotation_axis(axis: &Vec3, angle: f32) -> Self {
        Self::rotation(axis.x, axis.y, axis.z, angle)
    }
    /// `setToRotation(yaw, pitch, roll)`: Y, then X, then Z axis.
    pub fn set_to_rotation_ypr(&mut self, yaw: f32, pitch: f32, roll: f32) {
        self.set_to_rotation(0.0, 1.0, 0.0, yaw);
        let mut axis_turn = Self::rotation(1.0, 0.0, 0.0, pitch);
        self.multiply(&axis_turn);
        axis_turn.set_to_rotation(0.0, 0.0, 1.0, roll);
        self.multiply(&axis_turn);
    }
    /// `multiply(Quaternion)`: every component reads the old values.
    pub fn multiply(&mut self, o: &Self) {
        let (w, x, y, z) = (self.w, self.x, self.y, self.z);
        self.w = y * o.x + w * o.z + z * o.w - x * o.y;
        self.x = w * o.y + z * o.x + (x * o.z - y * o.w);
        self.y = z * o.y + (y * o.z + x * o.w - w * o.x);
        self.z = z * o.z - w * o.w - x * o.x - y * o.y;
    }
    /// `opposite` (conjugate: vector part negated).
    pub fn opposite(&mut self) {
        self.w = -self.w;
        self.x = -self.x;
        self.y = -self.y;
    }
    /// static `opposite(q)`.
    pub fn opposite_of(q: &Self) -> Self {
        let mut v = *q;
        v.opposite();
        v
    }
    /// All four components negated.
    pub fn negate(&mut self) {
        self.w = -self.w;
        self.x = -self.x;
        self.y = -self.y;
        self.z = -self.z;
    }
    /// `dot` (z, y, x, w order).
    pub fn dot(&self, o: &Self) -> f32 {
        self.z * o.z + self.y * o.y + self.x * o.x + self.w * o.w
    }
    /// static `length`.
    pub fn length(&self) -> f32 {
        f64::from(self.dot(self)).sqrt() as f32
    }
    /// `inverse`: normalisation, not the algebraic inverse.
    pub fn inverse(&mut self) {
        let inv_length = 1.0 / self.length();
        self.w *= inv_length;
        self.x *= inv_length;
        self.y *= inv_length;
        self.z *= inv_length;
    }
    pub fn is_finite(&self) -> bool {
        self.w.is_finite() && self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
    /// `add`.
    pub fn add(&mut self, o: &Self) {
        self.w += o.w;
        self.x += o.x;
        self.y += o.y;
        self.z += o.z;
    }
    /// `scale`.
    pub fn scale(&mut self, f: f32) {
        self.w *= f;
        self.x *= f;
        self.y *= f;
        self.z *= f;
    }
    /// Hemisphere-corrected linear blend, renormalised.
    pub fn blend(&mut self, o: &Self, t: f32) {
        if self.dot(o) < 0.0 {
            self.negate();
        }
        self.scale(1.0 - t);
        let mut scaled = *o;
        scaled.scale(t);
        self.add(&scaled);
        self.inverse();
    }
}
