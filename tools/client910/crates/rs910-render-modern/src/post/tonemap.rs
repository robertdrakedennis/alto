//! The modern renderer's display transfer and tonemap (renderer plan M1,
//! recalibrated by lane Q-LOOK): how display-referred classic colours enter
//! the linear HDR forward target, and how the target comes back out.
//!
//! # Display transfer: a 2.2 power, as the modern client decodes environment colours
//!
//! Every classic colour the modern pass consumes (HSL vertex colours, the
//! classic 0.7 texture gamma's output, the sun, fog and sky colours) is
//! display-referred: the faithful toolkit multiplies them by its lighting as
//! display values. The modern client decodes such colours with a pure power,
//! `powf(c / 255, 2.2)`, when its gamma-correct path is on (for the sun
//! colour and the second environment colour), and so does this crate
//! ([`display_to_linear`], [`linear_to_display`]; the WGSL mirrors them). A
//! pure power commutes with
//! a grey light: `(a^2.2 * k^2.2)^(1/2.2) = a * k`, so a lit albedo keeps
//! its hue and saturation exactly. The piecewise sRGB curve does not: its
//! linear toe crushes the darkest channel of a dark colour, which raised the
//! saturation of Lumbridge's dark grass and dirt by 0.07-0.12 (lane Q-LOOK,
//! measured against the faithful frames). The sRGB curve remains only where
//! the hardware applies it (sRGB texture formats and targets); the shader
//! converts those samples to the display value and decodes that.
//!
//! # Tonemap: identity below a knee, one hue-preserving shoulder above
//!
//! The modern client picks one of several operators (Reinhard
//! RGB/luminance, Hejl-Dawson, Uncharted 2, Hable). The M1 choice was the
//! published Hable curve applied per channel at exposure 3.2: an S-curve
//! that lifts darks by about 15% and compresses mid-tones, per channel, so it
//! moved saturation too. This backend now uses its own operator, in the
//! spirit of a luminance-scaled Reinhard: the HDR colour times [`EXPOSURE`]
//! is shown unchanged
//! while its largest channel stays at or below [`KNEE`], and above it the
//! whole colour is scaled by one smooth shoulder of the largest channel
//! towards white (slope 1 at the knee, 1.0 approached asymptotically). The
//! scene's albedo range therefore passes through untouched (the faithful
//! colours, [`display_colour`]), only lit highlights (sun-facing light
//! stone, specular) roll off, and a colour's hue survives the shoulder.
//!
//! [`inverse_tonemap`] places display-referred inputs (the classic fog and sky
//! colours) into the HDR target so the tonemap shows them unchanged.

/// The display transfer's exponent (a pure `powf(c, 2.2)`, see the module
/// docs).
pub const GAMMA: f32 = 2.2;

/// The largest channel (after exposure) the tonemap passes through
/// unchanged (linear; about 0.90 on the display).
pub const KNEE: f32 = 0.8;

/// The exposure the forward target is scaled by before the tonemap. The
/// modern lighting (`lighting::environment`) already carries the environment's
/// intensities, so 1.0: an HDR value of a display colour's decode shows that
/// colour.
pub const EXPOSURE: f32 = 1.0;

/// A display value (0..1) to linear light: the 2.2 power.
#[must_use]
pub fn display_to_linear(c: f32) -> f32 {
    c.max(0.0).powf(GAMMA)
}

/// Linear light to the display value (the inverse of
/// [`display_to_linear`]).
#[must_use]
pub fn linear_to_display(c: f32) -> f32 {
    c.max(0.0).powf(1.0 / GAMMA)
}

/// The shoulder above [`KNEE`]: continuous with slope 1 at the knee,
/// approaching 1.0.
fn shoulder(m: f32) -> f32 {
    if m <= KNEE {
        m
    } else {
        KNEE + (1.0 - KNEE) * (1.0 - (-(m - KNEE) / (1.0 - KNEE)).exp())
    }
}

/// The inverse of [`shoulder`] for `y < 1`.
fn shoulder_inverse(y: f32) -> f32 {
    if y <= KNEE {
        y
    } else {
        KNEE - (1.0 - KNEE) * (1.0 - (y - KNEE) / (1.0 - KNEE)).ln()
    }
}

/// The linear display colour (0..1) of an HDR colour: exposure, then the
/// largest channel through the shoulder with the colour scaled along.
#[must_use]
pub fn tonemap(rgb: [f32; 3]) -> [f32; 3] {
    let x = rgb.map(|c| c.max(0.0) * EXPOSURE);
    let m = x[0].max(x[1]).max(x[2]);
    if m <= KNEE {
        return x;
    }
    let scale = shoulder(m) / m;
    x.map(|c| (c * scale).min(1.0))
}

/// The HDR colour whose [`tonemap`] is the linear display colour `rgb`
/// (each channel below 1; values at or above are held just under white).
#[must_use]
pub fn inverse_tonemap(rgb: [f32; 3]) -> [f32; 3] {
    let y = rgb.map(|c| c.clamp(0.0, 0.9999));
    let m = y[0].max(y[1]).max(y[2]);
    if m <= KNEE {
        return y.map(|c| c / EXPOSURE);
    }
    let scale = shoulder_inverse(m) / m;
    y.map(|c| c * scale / EXPOSURE)
}

/// The sRGB transfer function (display value to linear), for the texture
/// paths the hardware decodes with it.
#[must_use]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The inverse sRGB transfer function (linear to display value).
#[must_use]
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// The HDR colour that the tonemap and the display encode turn back into
/// the display colour `rgb` (0..1 per channel): how the classic fog and clear
/// colours enter the forward target.
#[must_use]
pub fn display_to_hdr(rgb: [f32; 3]) -> [f32; 3] {
    inverse_tonemap(rgb.map(|c| display_to_linear(c.clamp(0.0, 1.0))))
}

/// The display colour the composite shows for an HDR colour, before the
/// colour grading (`post::grading`): the tonemap and the display encode.
#[must_use]
pub fn display_colour(hdr: [f32; 3]) -> [f32; 3] {
    tonemap(hdr).map(linear_to_display)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_undoes_the_curve() {
        for i in 1..255 {
            let y = i as f32 / 255.0;
            for rgb in [[y, y, y], [y, y * 0.5, y * 0.25], [0.1, y, 0.3]] {
                let x = inverse_tonemap(rgb);
                assert!(
                    x.iter().all(|c| c.is_finite() && *c >= 0.0),
                    "{rgb:?}: {x:?}"
                );
                let back = tonemap(x);
                for (b, c) in back.iter().zip(rgb) {
                    assert!((b - c).abs() < 1e-4, "{rgb:?} -> {x:?} -> {back:?}");
                }
            }
        }
        assert_eq!(inverse_tonemap([0.0; 3]), [0.0; 3]);
        assert_eq!(tonemap([0.0; 3]), [0.0; 3]);
    }

    #[test]
    fn display_colours_round_trip_through_the_encode() {
        for rgb in [[0.0, 0.5, 1.0], [0.47, 0.69, 0.78], [0.02, 0.2, 0.9]] {
            let hdr = display_to_hdr(rgb);
            let shown = display_colour(hdr);
            for (s, c) in shown.iter().zip(rgb) {
                assert!((s - c).abs() < 1e-3, "{rgb:?}: {shown:?}");
            }
        }
    }
}
