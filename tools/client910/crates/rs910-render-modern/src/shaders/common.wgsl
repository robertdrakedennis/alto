
const GAMMA: f32 = 2.2;
const KNEE: f32 = 0.8;

// post::tonemap::display_to_linear / linear_to_display.
fn display_to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(GAMMA));
}

fn linear_to_display(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / GAMMA));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

// A sample of an sRGB-format texture (the hardware's piecewise decode) as
// linear light under the display transfer.
fn texture_linear(c: vec3<f32>) -> vec3<f32> {
    return display_to_linear(linear_to_srgb(c));
}

fn shoulder(m: f32) -> f32 {
    if (m <= KNEE) {
        return m;
    }
    return KNEE + (1.0 - KNEE) * (1.0 - exp(-(m - KNEE) / (1.0 - KNEE)));
}

fn shoulder_inverse(y: f32) -> f32 {
    if (y <= KNEE) {
        return y;
    }
    return KNEE - (1.0 - KNEE) * log(1.0 - (y - KNEE) / (1.0 - KNEE));
}

// post::tonemap::tonemap: identity below the knee, one shoulder of the
// largest channel above it (hue-preserving).
fn tonemap(hdr: vec3<f32>, exposure: f32) -> vec3<f32> {
    let x = max(hdr, vec3<f32>(0.0)) * exposure;
    let m = max(max(x.r, x.g), x.b);
    if (m <= KNEE) {
        return x;
    }
    return min(x * (shoulder(m) / m), vec3<f32>(1.0));
}

// The HDR value whose tonemap is the linear display value `y`
// (post::tonemap::inverse_tonemap).
fn inverse_tonemap(y_in: vec3<f32>, exposure: f32) -> vec3<f32> {
    let y = clamp(y_in, vec3<f32>(0.0), vec3<f32>(0.9999));
    let m = max(max(y.r, y.g), y.b);
    if (m <= KNEE) {
        return y / exposure;
    }
    return y * (shoulder_inverse(m) / m) / exposure;
}
