//! Exact type-6 UI model transform/projection helpers.
use crate::{
    actor_matrix::Matrix,
    camera::{perspective_pixels, PixelLens},
    trig,
    ui_component_fields::Fields,
};

/// The model matrix and projection of a type-6 component. `y_shift` is the
/// extra vertical offset item models take (half their height); every other
/// model passes 0.
pub fn matrices(
    f: &Fields,
    at: [i32; 2],
    canvas: [i32; 2],
    world: [f32; 2],
    cam: Option<[f32; 2]>,
    y_shift: i32,
) -> (Matrix, [f32; 16]) {
    let sx = if f.modelobjwidth > 0 {
        ((f.width.wrapping_shl(9)) / f.modelobjwidth) as f32
    } else {
        512.0
    };
    let sy = if f.modelobjheight > 0 {
        ((f.height.wrapping_shl(9)) / f.modelobjheight) as f32
    } else {
        512.0
    };
    let mut cx = (f.width / 2 + at[0]) as f32;
    let mut cy = (f.height / 2 + at[1]) as f32;
    if !f.useExtendedModelTransform {
        cx += ((f.modelorigin_x * sx as i32) >> 9) as f32;
        cy += ((f.modelorigin_y * sy as i32) >> 9) as f32;
    }
    let [near, far] = cam.unwrap_or([world[0], world[1] + f.modelzoom as f32]);
    let mut p = [0.0; 16];
    if f.modelorthog {
        let z = if f.useExtendedModelTransform {
            f.modelzoom as f32
        } else {
            (f.modelzoom << 2) as f32
        };
        let l = -(cx * z) / sx;
        let r = ((canvas[0] as f32 - cx) * z) / sx;
        let b = -(cy * z) / sy;
        let t = ((canvas[1] as f32 - cy) * z) / sy;
        p[0] = 2.0 / (r - l);
        p[5] = 2.0 / (t - b);
        p[10] = 2.0 / (far - near);
        p[12] = -(l + r) / (r - l);
        p[13] = -(b + t) / (t - b);
        p[14] = -(near + far) / (far - near);
        p[15] = 1.0;
    } else {
        p = perspective_pixels(PixelLens {
            centre: [cx, cy],
            focal: [sx, sy],
            near,
            far,
            size: [canvas[0] as f32, canvas[1] as f32],
        });
    }
    let mut m;
    if f.useExtendedModelTransform {
        m = Matrix::axis(1., 0., 0., trig::radians(f.modelangle_x));
        m.rotate(0., 1., 0., trig::radians(f.modelangle_y));
        m.rotate(0., 0., 1., trig::radians(f.modelangle_z));
        m.0[9] += f.modelorigin_x as f32;
        m.0[10] += f.modelorigin_y as f32;
        m.0[11] += f.modelorigin_z as f32;
    } else {
        let vx = ((f.modelzoom << 2) * trig::sin(f.modelangle_x << 3)) >> 14;
        let vz = ((f.modelzoom << 2) * trig::cos(f.modelangle_x << 3)) >> 14;
        m = Matrix::axis(0., 0., 1., trig::radians((-f.modelangle_z) << 3));
        m.rotate(0., 1., 0., trig::radians(f.modelangle_y << 3));
        m.0[9] += (f.modelxof << 2) as f32;
        m.0[10] += ((f.modelyof << 2) + vx + y_shift) as f32;
        m.0[11] += ((f.modelyof << 2) + vz) as f32;
        m.rotate(1., 0., 0., trig::radians(f.modelangle_x << 3));
    }
    (m, p)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ui_model_matrices_match_the_recording() -> anyhow::Result<()> {
        let read = |name: &str| -> Vec<i32> {
            rs910_core::test_support::frozen::bytes(name)
                .chunks_exact(4)
                .map(|b| i32::from_be_bytes(b.try_into().unwrap()))
                .collect()
        };
        let input = read("ui-player-model/inputs.bin");
        let output = read("ui-player-model/recorded-matrices.bin");
        assert_eq!(input[0], output[0]);
        assert_eq!(input.len(), 1 + input[0] as usize * 25);
        assert_eq!(output.len(), 1 + input[0] as usize * 28);
        for (id, row) in input[1..].chunks_exact(25).enumerate() {
            let f = Fields {
                width: row[0],
                height: row[1],
                modelorigin_x: row[2],
                modelorigin_y: row[3],
                modelorigin_z: row[4],
                modelxof: row[5],
                modelyof: row[6],
                modelangle_x: row[7],
                modelangle_y: row[8],
                modelangle_z: row[9],
                modelzoom: row[10],
                modelobjwidth: row[11],
                modelobjheight: row[12],
                useExtendedModelTransform: row[13] != 0,
                modelorthog: row[14] != 0,
                ..Default::default()
            };
            let (m, p) = matrices(
                &f,
                [row[15], row[16]],
                [row[17], row[18]],
                [row[20] as f32, row[21] as f32],
                (row[24] == 3).then_some([row[22] as f32, row[23] as f32]),
                row[19],
            );
            let actual: Vec<_> =
                m.0.into_iter()
                    .chain(p)
                    .map(|f| f.to_bits() as i32)
                    .collect();
            assert_eq!(
                actual,
                &output[1 + id * 28..1 + (id + 1) * 28],
                "recorded case {id}: {row:?}"
            );
        }
        eprintln!("{} recorded UI model matrices match exactly", input[0]);
        Ok(())
    }
}
