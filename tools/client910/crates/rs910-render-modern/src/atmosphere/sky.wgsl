
// The sky: the cube (or two cross-fading cubes) sampled along the view direction, fogged by
// elevation, plus the sun's glow and the exposure offset (`atmosphere::sky`). The cubes hold the
// classic sky's colour at the calibrated level the composite expects (`p1.z` divides it out).
@group(3) @binding(0) var sky_previous: texture_cube<f32>;
@group(3) @binding(1) var sky_current: texture_cube<f32>;
@group(3) @binding(2) var sky_sampler: sampler;

// `sky::angle_fog`: the fog colour covers the horizon and below, thinning upwards.
fn sky_angle_fog(up: f32, z: f32, w: f32) -> f32 {
    return clamp(pow(1.0 - clamp(up + w, 0.0, 1.0), z), 0.0, 1.0);
}

// `sky::sun_glow`: the lobe around the sun, scaled by the elevation.
fn sky_sun_glow(sun_dot: f32, up: f32, z: f32) -> f32 {
    return pow(max(sun_dot, 0.0), 128.0 * z) * min(z * up, 1.0);
}

@fragment
fn fs_sky(in: FullOut) -> @location(0) vec4<f32> {
    let px = in.clip.xy;
    let dir = normalize(pass_unproject(px, 0.5) - frame.eye.xyz);
    // Classic space is y down: the up component, and the cube's own direction.
    let up = -dir.y;
    let exposure = pass_block.p0.w;
    if (pass_block.p1.x < 0.5) {
        // No cube: the flat colour, plus the exposure.
        return vec4<f32>(max(pass_block.c[0].rgb + vec3<f32>(exposure), vec3<f32>(0.0)), 1.0);
    }
    let cube_dir = vec3<f32>(dir.x, -dir.y, dir.z);
    let previous = textureSampleLevel(sky_previous, sky_sampler, cube_dir, 0.0).rgb;
    let current = textureSampleLevel(sky_current, sky_sampler, cube_dir, 0.0).rgb;
    var c = current;
    if (pass_block.p2.z > 0.5) {
        // Two cubes mix; a side that does not exist takes the other.
        var a = previous;
        var b = current;
        if (pass_block.p2.x < 0.5) {
            a = b;
        }
        if (pass_block.p2.y < 0.5) {
            b = a;
        }
        c = mix(a, b, pass_block.p1.w);
    }
    c = c * pass_block.p1.z;
    // The decor sprites drawn over the cube (premultiplied, with their coverage).
    if (pass_block.p1.y > 0.5) {
        let decor = textureLoad(tex0, vec2<i32>(px), 0);
        c = c * (1.0 - decor.a) + decor.rgb;
    }
    // The angle fog, and with light scattering and the distance fog on, the sun's glow.
    if (frame.fog_colour.w > 0.0) {
        let f = sky_angle_fog(up, pass_block.p0.x, pass_block.p0.y);
        c = mix(c, frame.fog_colour.rgb, f);
        if (frame.ground_ambient.w > 0.0) {
            c += vec3<f32>(sky_sun_glow(dot(frame.sun_dir.xyz, dir), up, pass_block.p0.x));
        }
    }
    return vec4<f32>(max(c + vec3<f32>(exposure), vec3<f32>(0.0)), 1.0);
}
