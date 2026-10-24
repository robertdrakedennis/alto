
// The lit surface: the RT5 and RT7 BRDFs, the lighting sum and the fog.

// The GGX (Trowbridge-Reitz) normal distribution of perceptual roughness
// `rough` (the roughness is its square).
fn ggx(rough: f32, n_dot_h: f32) -> f32 {
    let a = rough * rough;
    let p = a * a;
    let t = n_dot_h * n_dot_h * (p - 1.0) + 1.0;
    return p / (3.14159 * (t * t + 0.0001));
}

// The Schlick-Smith visibility term.
fn schlick_smith(power: f32, n_dot_l: f32, n_dot_v: f32) -> f32 {
    let p = 1.0 / sqrt(3.14159 / 4.0 * power + 3.14159 / 2.0);
    let d = 1.0 - p;
    let v = (n_dot_l * d + p) * (n_dot_v * d + p);
    return 1.0 / (v + 0.0001);
}

// The RT5 and RT7 surface main: the lighting sum, then the fog.
fn shade_ambient(in: VsOut, s: Surface, fog: f32) -> vec3<f32> {
    let flags = u32(in.p0.w);
    if ((flags & 8u) != 0u) {
        // A sky model: its (display) colour as the tonemap will show it.
        return inverse_tonemap(s.colour, frame.params.x);
    }
    var lit = s.colour;
    if ((flags & 4u) == 0u) {
        let n = lit_normal(in, s.detail);
        let l = frame.sun_dir.xyz;
        let n_dot_l = max(dot(n, l), 0.0);
        var e = n_dot_l;
        if (n_dot_l > 0.0) {
            e = n_dot_l * sun_visibility(in.world, n, in.view_depth);
        }
        // The ambient: the hemisphere until light probes (M6: the modern client
        // takes the ambient colour times the SH irradiance), times the SSAO.
        var ambient = mix(frame.ground_ambient.rgb, frame.sky_ambient.rgb, 0.5 - 0.5 * n.y);
        if (frame.params.y > 0.5) {
            ambient = ambient * ambient_occlusion(in.clip.xy);
        }
        let v = normalize(frame.eye.xyz - in.world);
        let h = normalize(v + l);
        let sun = frame.sun_colour.rgb * e;
        let point = point_light_sum(in, n);
        if (s.rt7) {
            // The RT7 lighting: metal-mixed albedo, Cook-Torrance sun.
            let albedo = mix(s.colour, s.tex, s.metal);
            let rough = clamp(s.rough * RT7_ROUGHNESS_MULTIPLIER, 0.0, 1.0);
            let gloss = 1.0 - rough;
            let power = pow(8192.0, gloss);
            let f0 = mix(vec3<f32>(RT7_NON_METAL_SPECULAR), albedo, s.metal);
            let n_dot_v = max(dot(n, v), 0.0);
            // The RT7 sun term: Schlick fresnel times the Cook-Torrance
            // BRDF.
            let fresnel = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - clamp(dot(l, h), 0.0, 1.0), RT7_FRESNEL_EXPONENT);
            let brdf = ggx(rough, max(dot(n, h), 0.0)) * schlick_smith(power, n_dot_l, n_dot_v);
            let spec = fresnel * brdf * sun;
            // The energy conservation factor: one minus the roughness-aware
            // Schlick fresnel (F0, N.V, gloss), zero for metal.
            let fr = f0 + (max(vec3<f32>(gloss), f0) - f0) * pow(1.0 - n_dot_v, 5.0);
            let conserve = (vec3<f32>(1.0) - clamp(fr, vec3<f32>(0.0), vec3<f32>(1.0))) * (1.0 - s.metal);
            lit = (ambient + sun + point) * conserve * albedo + spec;
            // The emissive mask: the unlit colour, black where metallic
            // (emissive colour, source and scale: the settings atlas's 0).
            lit = mix(lit, s.colour * (1.0 - s.metal), s.emissive);
        } else {
            var spec = vec3<f32>(0.0);
            if (in.p1.z > 0.0) {
                // The RT5 sun term: Schlick fresnel (0.65, L.H, 5) times the
                // normalised Blinn-Phong, times the shadowed sun.
                let fresnel = RT5_F0 + (1.0 - RT5_F0) * pow(1.0 - clamp(dot(l, h), 0.0, 1.0), RT5_FRESNEL_POWER);
                let ndf = (in.p1.z + 2.0) * 0.125 * pow(max(dot(h, n), 0.0), in.p1.z);
                spec = sun * (fresnel * ndf * in.p1.w);
            }
            // Ambient +
            // diffuse + specular * D, times the albedo.
            lit = (ambient + sun + point + spec * s.spec_mul) * s.colour;
        }
    }
    return scatter(apply_fog(lit, fog), fog, in.world);
}

@fragment
fn fs_forward_ambient(inf: VsOutFog) -> @location(0) vec4<f32> {
    let in = unfog(inf);
    let s = surface(in);
    if (!s.keep) {
        discard;
    }
    return vec4<f32>(shade_ambient(in, s, inf.fog), s.alpha);
}
