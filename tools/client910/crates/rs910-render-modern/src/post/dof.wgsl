
fn dof_view_depth(px: vec2<f32>, z: f32) -> f32 {
    let p = pass_unproject(px, min(z, 1.0)) - frame.eye.xyz;
    return dot(p, pass_block.p3.xyz);
}

// The focus weights (near, far) at a view depth.
fn dof_focus(depth: f32) -> vec2<f32> {
    let v = depth - pass_block.p1.x;
    let p = pass_block.p0;
    let near = step(0.0, p.y) * smoothstep(p.x, p.x + p.y, -v);
    let far = step(0.0, p.w) * smoothstep(p.z, p.z + p.w, v);
    return vec2<f32>(near, far);
}

fn dof_texel(t: texture_2d<f32>, p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(t));
    return textureLoad(t, clamp(p, vec2<i32>(0), size - vec2<i32>(1)), 0);
}

fn dof_depth(p: vec2<i32>) -> f32 {
    let size = vec2<i32>(textureDimensions(depth_tex));
    let q = clamp(p, vec2<i32>(0), size - vec2<i32>(1));
    return dof_view_depth(vec2<f32>(q) + 0.5, textureLoad(depth_tex, q, 0));
}

// One of the Kawase filter's four taps at (iteration + 1/2) texels
// diagonally, as whole-texel pairs averaged (the bilinear tap of the
// half-texel offset): a bilinear tap at a texel corner (a 2x2 mean); invalid
// (the reference: NaN) when any of the four texels is.
fn dof_tap(t: texture_2d<f32>, p: vec2<i32>, o: vec2<i32>, d: vec2<i32>) -> vec4<f32> {
    let a = dof_texel(t, p + o);
    let b = dof_texel(t, p + o + vec2<i32>(d.x, 0));
    let c = dof_texel(t, p + o + vec2<i32>(0, d.y));
    let e = dof_texel(t, p + o + d);
    let valid = min(min(a.a, b.a), min(c.a, e.a));
    return vec4<f32>((a.rgb + b.rgb + c.rgb + e.rgb) * 0.25, select(0.0, (a.a + b.a + c.a + e.a) * 0.25, valid > 0.0));
}

// The view depth of the tap's 2x2 depth mean.
fn dof_tap_depth(p: vec2<i32>) -> f32 {
    let size = vec2<i32>(textureDimensions(depth_tex));
    var z = 0.0;
    for (var k = 0; k < 4; k++) {
        let q = clamp(p + vec2<i32>(k % 2, k / 2), vec2<i32>(0), size - vec2<i32>(1));
        z += textureLoad(depth_tex, q, 0);
    }
    return dof_view_depth(vec2<f32>(p) + 1.0, z * 0.25);
}

// The focus pass: both weights per pixel.
@fragment
fn fs_dof_focus(in: FullOut) -> @location(0) vec4<f32> {
    let ip = vec2<i32>(in.clip.xy);
    return vec4<f32>(dof_focus(dof_depth(ip)), 0.0, 1.0);
}

// The blur pass: p1.z 0 the far blur, 1 the near.
@fragment
fn fs_dof_blur(in: FullOut) -> @location(0) vec4<f32> {
    let ip = vec2<i32>(in.clip.xy);
    let focus = dof_texel(tex1, ip);
    let near_pass = pass_block.p1.z > 0.5;
    let weight = select(focus.y, focus.x, near_pass);
    let k = i32(pass_block.p1.y);
    var offs = array<vec2<i32>, 4>(vec2<i32>(-k - 1, k), vec2<i32>(k, k), vec2<i32>(k, -k - 1), vec2<i32>(-k - 1, -k - 1));
    if (weight == 0.0) {
        if (near_pass) {
            var any = 0.0;
            for (var i = 0; i < 4; i++) {
                any += dof_tap(tex1, ip, offs[i], vec2<i32>(1, 1)).x;
            }
            if (any > 0.0) {
                return vec4<f32>(dof_texel(tex0, ip).rgb, 1.0);
            }
        }
        return vec4<f32>(0.0);
    }
    var sum = vec3<f32>(0.0);
    var mx = vec3<f32>(0.0);
    var count = 0.0;
    for (var i = 0; i < 4; i++) {
        let c = dof_tap(tex0, ip, offs[i], vec2<i32>(1, 1));
        var valid = c.a > 0.0;
        if (!near_pass) {
            // A far-blur tap counts when its depth is past either band's
            // start.
            let v = dof_tap_depth(ip + offs[i]) - pass_block.p1.x;
            valid = valid && (-v >= pass_block.p0.x + pass_block.p0.y * 0.1 || v >= pass_block.p0.z + pass_block.p0.w * 0.1);
        }
        if (valid) {
            sum += c.rgb;
            mx = max(mx, c.rgb);
            count += 1.0;
        }
    }
    var mean = sum / max(1.0, count);
    var peak = mx;
    if (!near_pass) {
        if (count < 0.5) {
            mean = dof_texel(tex0, ip).rgb;
            peak = vec3<f32>(0.0);
        }
        peak = peak * pass_block.p2.w;
    } else {
        peak = peak * pass_block.p2.z;
    }
    // The boost per channel; a near pixel without a valid tap is black (l = 0 / 1), valid.
    let f = select(mx, vec3<f32>(0.0), !near_pass && count < 0.5);
    let rgb = mix(mean, peak, smoothstep(vec3<f32>(pass_block.p2.x), vec3<f32>(pass_block.p2.y), f));
    return vec4<f32>(rgb, 1.0);
}

// The spread pass: the focus weights spread.
@fragment
fn fs_dof_spread(in: FullOut) -> @location(0) vec4<f32> {
    let ip = vec2<i32>(in.clip.xy);
    let k = i32(pass_block.p1.y);
    var offs = array<vec2<i32>, 4>(vec2<i32>(-k - 1, k), vec2<i32>(k, k), vec2<i32>(k, -k - 1), vec2<i32>(-k - 1, -k - 1));
    var u = vec2<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        u += dof_tap(tex1, ip, offs[i], vec2<i32>(1, 1)).xy;
    }
    // The near weight spreads (it is what the near blur and the composite
    // read); the far weight stays the pixel's own.
    // Both weights are 0.4 times the four taps' sum.
    return vec4<f32>(u * 0.4, 0.0, 1.0);
}

// The composite pass.
@fragment
fn fs_dof_composite(in: FullOut) -> @location(0) vec4<f32> {
    let ip = vec2<i32>(in.clip.xy);
    let s = dof_texel(tex1, ip);
    let x = dof_texel(tex0, ip);
    var d = x.rgb;
    let v = s.x;
    if (v > 0.0) {
        let u = dof_texel(tex3, ip);
        if (u.a > 0.0) {
            d = mix(d, u.rgb, clamp(v, 0.0, 1.0));
        }
    }
    let p = dof_depth(ip) - pass_block.p1.x;
    let e = smoothstep(pass_block.p0.z, pass_block.p0.z + pass_block.p0.w, p);
    if (s.y > 0.0) {
        let u = dof_texel(tex2, ip);
        var t = x.rgb;
        if (u.a > 0.0) {
            t = mix(x.rgb, u.rgb, e);
        }
        d = mix(d, t, step(v, e));
    }
    return vec4<f32>(d, x.a);
}
