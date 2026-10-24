
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

// The night sky over the cube (`pass_block.c[1]`: the moon's colour and the night amount,
// `atmosphere::distance_fog::night_amount`): the zenith deepens towards a dark blue while the horizon
// keeps the area's fog colour, a moon with its halo sits in the light's direction,
// and stars come out above the horizon haze. Stars are fixed to the sky (no twinkle) and drawn
// as soft dots one or two pixels wide, so they stay still as the camera moves.
const NIGHT_ZENITH: vec3<f32> = vec3<f32>(0.32, 0.42, 0.72);
const MOON_DISK_COS: f32 = 0.99975;
const MOON_EDGE_COS: f32 = 0.9996;
const MOON_DISK: f32 = 3.0;
const MOON_HALO: f32 = 0.35;
const MOON_HALO_POWER: f32 = 24.0;
const STAR_CELLS_PER_RADIAN: f32 = 90.0;
const STAR_DENSITY: f32 = 0.02;
// Stars show only against a dark sky (display-referred luminance of the sky below).
const STAR_SKY_DARK: f32 = 0.02;
const STAR_SKY_LIT: f32 = 0.06;
const STAR_BRIGHTNESS: f32 = 2.0;

fn star_hash(p: vec2<f32>) -> vec3<f32> {
    var q = vec3<f32>(p.xyx) * vec3<f32>(0.1031, 0.1030, 0.0973);
    q = fract(q);
    q = q + dot(q, q.yxz + 33.33);
    return fract((q.xxy + q.yzz) * q.zyx);
}

fn night_sky(c: vec3<f32>, dir: vec3<f32>, up: f32, haze: f32, pixel: f32) -> vec3<f32> {
    let n = pass_block.c[1].w;
    if (n <= 0.0) {
        return c;
    }
    let height = smoothstep(0.0, 0.7, max(up, 0.0));
    var s = mix(c, c * NIGHT_ZENITH, n * height);
    // The moon (the light's direction; classic space, as `frame.sun_dir`).
    let moon = pass_block.c[1].rgb;
    let m = dot(normalize(frame.sun_dir.xyz), dir);
    let disk = smoothstep(MOON_EDGE_COS, MOON_DISK_COS, m) * MOON_DISK;
    let halo = pow(max(m, 0.0), MOON_HALO_POWER) * MOON_HALO;
    s += moon * (disk + halo) * n * (1.0 - haze);
    // Stars: one candidate per cell of an elevation/azimuth grid, jittered in its cell.
    let az = atan2(dir.z, dir.x);
    let el = asin(clamp(up, -1.0, 1.0));
    let ring = max(cos(el), 0.05);
    let cell_w = 1.0 / STAR_CELLS_PER_RADIAN;
    let g = vec2<f32>(az * ring, el) * STAR_CELLS_PER_RADIAN;
    let cell = floor(g);
    let h = star_hash(cell);
    if (h.z < STAR_DENSITY) {
        let centre = (cell + 0.2 + 0.6 * h.xy) * cell_w;
        let d = distance(vec2<f32>(az * ring, el), centre);
        let size = pixel * (0.5 + 0.5 * h.x);
        let star = exp(-(d * d) / (size * size)) * (0.15 + h.y * h.y * h.y) * STAR_BRIGHTNESS;
        let warm = mix(vec3<f32>(0.85, 0.9, 1.0), vec3<f32>(1.0, 0.92, 0.8), h.x);
        let dark = 1.0 - smoothstep(STAR_SKY_DARK, STAR_SKY_LIT, dot(s, vec3<f32>(0.2125, 0.7154, 0.0721)));
        s += warm * star * n * dark * smoothstep(0.03, 0.25, up) * (1.0 - haze);
    }
    return s;
}

@fragment
fn fs_sky(in: FullOut) -> @location(0) vec4<f32> {
    let px = in.clip.xy;
    let dir = normalize(pass_unproject(px, 0.5) - frame.eye.xyz);
    // One pixel's angle, for the stars' size.
    let pixel = length(fwidth(dir));
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
        c = night_sky(c, dir, up, f, pixel);
        if (frame.ground_ambient.w > 0.0) {
            c += vec3<f32>(sky_sun_glow(dot(frame.sun_dir.xyz, dir), up, pass_block.p0.x));
        }
    }
    return vec4<f32>(max(c + vec3<f32>(exposure), vec3<f32>(0.0)), 1.0);
}
