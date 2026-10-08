//! Animation curve evaluation (cubic keyframe segments and the pre/post
//! extrapolation modes), preserving the f32 expression order of the original
//! client. The tangent clamp has no caller-visible effect; the infinity
//! routine's double casts are likewise kept, including their quirks.
use crate::anim::Curve;

#[derive(Clone, Debug)]
pub struct EvaluatedCurve {
    start: i32,
    values: Vec<f32>,
    before: f32,
    after: f32,
}
impl EvaluatedCurve {
    pub fn new(c: &Curve) -> Self {
        let start = c.keyframes.first().map_or(0, |k| k.time);
        let end = c.keyframes.last().map_or(start, |k| k.time);
        Self {
            start,
            values: (start..=end).map(|t| evaluate(c, t as f32)).collect(),
            before: evaluate(c, (start - 1) as f32),
            after: evaluate(c, (end + 1) as f32),
        }
    }
    pub fn value(&self, t: i32) -> f32 {
        if t < self.start {
            self.before
        } else {
            self.values
                .get(t.wrapping_sub(self.start) as usize)
                .copied()
                .unwrap_or(self.after)
        }
    }
}

fn coefficients(a: f32, b: f32, c: f32, d: f32) -> [f32; 4] {
    let x = b - a;
    let y = c - b;
    let z = d - c;
    let q = y - x;
    [a, x + x + x, q + q + q, z - y - q]
}

pub fn evaluate(c: &Curve, time: f32) -> f32 {
    let Some(first) = c.keyframes.first() else {
        return 0.;
    };
    let last = c.keyframes.last().unwrap();
    if time < first.time as f32 {
        return if c.pre == 0 {
            first.value
        } else {
            infinity(c, time, true)
        };
    }
    if time > last.time as f32 {
        return if c.post == 0 {
            last.value
        } else {
            infinity(c, time, false)
        };
    }
    let index = c
        .keyframes
        .partition_point(|k| k.time as f32 <= time)
        .saturating_sub(1);
    let a = &c.keyframes[index];
    let next = c.keyframes.get(index + 1);
    if a.tan_out == [0., 0.] {
        return a.value;
    }
    if a.tan_out == [f32::MAX, f32::MAX] {
        return match next {
            Some(b) if a.time as f32 != time => b.value,
            _ => a.value,
        };
    }
    let Some(b) = next else {
        return a.value;
    };
    let mut x = [
        a.time as f32,
        a.tan_out[0] * 0.33333334 + a.time as f32,
        b.time as f32 - b.tan_in[0] * 0.33333334,
        b.time as f32,
    ];
    let mut y = [
        a.value,
        a.tan_out[1] * 0.33333334 + a.value,
        b.value - b.tan_in[1] * 0.33333334,
        b.value,
    ];
    let range = x[3] - x[0];
    if c.bezier {
        let mut u = (x[1] - x[0]) / range;
        let mut v = (x[2] - x[0]) / range;
        let linear = u == 0.33333334 && v == 0.6666667;
        let old_u = u;
        let old_v = v;
        if u < 0. {
            u = 0.;
        }
        if v > 1. {
            v = 1.;
        }
        // Move the handles when the clamp changed the tangent.
        if u != old_u {
            x[1] = x[0] + u * range;
            if old_u != 0. {
                y[1] = y[0] + (y[1] - y[0]) * u / old_u;
            }
        }
        if v != old_v {
            x[2] = x[0] + v * range;
            if old_v != 1. {
                y[2] = (y[3] as f64 - (y[3] - y[2]) as f64 * (1. - v as f64) / (1. - old_v as f64))
                    as f32;
            }
        }
        let s = if x[0] == time {
            0.
        } else if x[3] == time {
            1.
        } else {
            (time - x[0]) / (x[3] - x[0])
        };
        let t = if linear {
            s
        } else {
            let mut p = coefficients(0., u, v, 1.);
            p[0] -= s;
            let roots = zeroes(&p, 3, 0., true, 1., true);
            match roots {
                Some(r) if r.len() == 1 => r[0],
                _ => 0.,
            }
        };
        let p = coefficients(y[0], y[1], y[2], y[3]);
        ((p[3] * t + p[2]) * t + p[1]) * t + p[0]
    } else {
        let delta = y[3] - y[0];
        let left = x[1] - x[0];
        let right = x[3] - x[2];
        let l = if left != 0. { (y[1] - y[0]) / left } else { 0. };
        let r = if right != 0. {
            (y[3] - y[2]) / right
        } else {
            0.
        };
        let inverse = 1. / (range * range);
        let dl = range * l;
        let dr = range * r;
        let p0 = (dl + dr - delta - delta) * inverse / range;
        let p1 = (delta + delta + delta - dl - dl - dr) * inverse;
        let t = time - x[0];
        ((p0 * t + p1) * t + l) * t + y[0]
    }
}

fn infinity(c: &Curve, time: f32, pre: bool) -> f32 {
    let first = &c.keyframes[0];
    let last = c.keyframes.last().unwrap();
    let start = first.time as f32;
    let end = last.time as f32;
    let range = end - start;
    if range == 0. {
        return first.value;
    }
    let cycles = if time > end {
        (time - end) / range
    } else {
        (time - start) / range
    };
    let truncated = cycles as f64; // A double cast, NOT an integer cast.
    let fraction = ((cycles as f64 - truncated) as f32).abs();
    let mut position = range * fraction;
    let repetitions = (truncated + 1.).abs();
    let half = repetitions / 2.;
    // Subtracting the value from itself gives 0, or NaN when `half` is
    // infinite. Kept verbatim (see programme §8).
    #[allow(
        clippy::eq_op,
        reason = "subtracts a value from itself on purpose: NaN for infinite input"
    )]
    let parity = (half - half) as f32;
    let mode = if pre { c.pre } else { c.post };
    if mode == 1 {
        if pre {
            return first.value
                - if first.tan_in[0] != 0. {
                    (start - time) * first.tan_in[1] / first.tan_in[0]
                } else {
                    0.
                };
        }
        return last.value
            + if last.tan_out[0] != 0. {
                (time - end) * last.tan_out[1] / last.tan_out[0]
            } else {
                0.
            };
    }
    if pre {
        if mode == 4 {
            if parity == 0. {
                position = end - position;
            } else {
                position += start;
            }
        } else if mode == 2 || mode == 3 {
            position = end - position;
        }
    } else if mode == 4 {
        if parity == 0. {
            position += start;
        } else {
            position = end - position;
        }
    } else if mode == 2 || mode == 3 {
        position += start;
    }
    let value = evaluate(c, position);
    let delta = last.value - first.value;
    if mode == 3 {
        if pre {
            (value as f64 - delta as f64 * repetitions) as f32
        } else {
            (delta as f64 * repetitions + value as f64) as f32
        }
    } else {
        value
    }
}

fn horner(p: &[f32], n: usize, x: f32) -> f32 {
    let mut value = p[n];
    for i in (0..n).rev() {
        value = x * value + p[i];
    }
    value
}
fn zeroes(p: &[f32], mut n: usize, a: f32, ac: bool, b: f32, bc: bool) -> Option<Vec<f32>> {
    let sum = p[..=n].iter().fold(0f32, |s, v| s + v.abs());
    let tolerance = (a.abs() + b.abs()) * (n + 1) as f32 * f32::EPSILON;
    if sum <= tolerance {
        return None;
    }
    let p: Vec<f32> = p[..=n].iter().map(|v| 1. / sum * v).collect();
    while n > 0 && p[n].abs() < tolerance {
        n -= 1;
    }
    let mut roots = Vec::new();
    if n == 0 {
        return Some(roots);
    }
    if n == 1 {
        let mut root = -p[0] / p[1];
        let left = if ac {
            a < root + tolerance
        } else {
            a < root - tolerance
        };
        let right = if bc {
            b > root - tolerance
        } else {
            b > root + tolerance
        };
        if left && right {
            if ac && root < a {
                root = a;
            } else if bc && root > b {
                root = b;
            }
            roots.push(root);
        }
        return Some(roots);
    }
    let derivative: Vec<f32> = (1..=n).map(|i| p[i] * i as f32).collect();
    let Some(dr) = zeroes(&derivative, n - 1, a, false, b, false) else {
        return Some(roots);
    };
    let mut skip = false;
    let mut pe = 0.;
    let mut end = 0.;
    for i in 0..=dr.len() {
        if roots.len() > n {
            return Some(roots);
        }
        let (start, ps) = if i == 0 {
            let value = horner(&p, n, a);
            if value.abs() <= tolerance && ac {
                roots.push(a);
            }
            (a, value)
        } else {
            (end, pe)
        };
        if i == dr.len() {
            end = b;
            skip = false;
        } else {
            end = dr[i];
        }
        pe = horner(&p, n, end);
        if skip {
            skip = false;
        } else if pe.abs() < tolerance {
            if i != dr.len() || bc {
                roots.push(end);
                skip = true;
            }
        } else if (ps < 0. && pe > 0.) || (ps > 0. && pe < 0.) {
            roots.push(zeroin(&p, n, start, end));
            let len = roots.len();
            if len > 1 && roots[len - 2] >= roots[len - 1] - tolerance {
                roots[len - 2] = (roots[len - 2] + roots[len - 1]) * 0.5;
                roots.pop();
            }
        }
    }
    Some(roots)
}
fn zeroin(p: &[f32], n: usize, mut a: f32, mut b: f32) -> f32 {
    let mut fa = horner(p, n, a);
    if fa.abs() < f32::EPSILON {
        return a;
    }
    let mut fb = horner(p, n, b);
    if fb.abs() < f32::EPSILON {
        return b;
    }
    let (mut c, mut d, mut e, mut fc) = (0f32, 0f32, 0f32, 0f32);
    let mut reset = true;
    loop {
        if reset {
            c = a;
            fc = fa;
            d = b - a;
            e = d;
            reset = false;
        }
        if fc.abs() < fb.abs() {
            a = b;
            b = c;
            c = a;
            fa = fb;
            fb = fc;
            fc = fa;
        }
        let tolerance = f32::EPSILON * 2. * b.abs();
        let half = (c - b) * 0.5;
        if half.abs() <= tolerance || fb == 0. {
            return b;
        }
        if e.abs() < tolerance || fa.abs() <= fb.abs() {
            d = half;
            e = half;
        } else {
            let s = fb / fa;
            let (mut x, mut y) = if a == c {
                (half * 2. * s, 1. - s)
            } else {
                let q = fa / fc;
                let r = fb / fc;
                (
                    (half * 2. * q * (q - r) - (b - a) * (r - 1.)) * s,
                    (q - 1.) * (r - 1.) * (s - 1.),
                )
            };
            if x > 0. {
                y = -y;
            } else {
                x = -x;
            }
            let previous = e;
            e = d;
            if x * 2. < half * 3. * y - (tolerance * y).abs() && x < (previous * 0.5 * y).abs() {
                d = x / y;
            } else {
                d = half;
                e = half;
            }
        }
        a = b;
        fa = fb;
        if d.abs() > tolerance {
            b += d;
        } else if half > 0. {
            b += tolerance;
        } else {
            b -= tolerance;
        }
        fb = horner(p, n, b);
        if fb * (fc / fc.abs()) > 0. {
            reset = true;
        }
    }
}
