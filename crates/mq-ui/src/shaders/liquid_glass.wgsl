// Apple-style Liquid Glass + Dual-Pass Separable GPU Backdrop Blur for egui 0.31 + wgpu 24.

struct BlurUniforms {
    texel_step: vec2<f32>,
    radius: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> blur_u: BlurUniforms;
@group(0) @binding(1) var blur_src: texture_2d<f32>;
@group(0) @binding(2) var blur_sampler: sampler;

struct BlurVertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_blur(@builtin(vertex_index) vi: u32) -> BlurVertexOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let uv = corners[vi];
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    var out: BlurVertexOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_blur(in: BlurVertexOut) -> @location(0) vec4<f32> {
    let blur_step = blur_u.texel_step * blur_u.radius;
    var acc = textureSampleLevel(blur_src, blur_sampler, in.uv, 0.0).rgb * 0.22702703;
    let offsets = array<f32, 4>(1.0, 2.0, 3.0, 4.0);
    let weights = array<f32, 4>(0.19459459, 0.12162162, 0.05405405, 0.01621622);
    for (var i = 0; i < 4; i = i + 1) {
        let d = blur_step * offsets[i];
        let uv_p = clamp(in.uv + d, vec2<f32>(0.001), vec2<f32>(0.999));
        let uv_n = clamp(in.uv - d, vec2<f32>(0.001), vec2<f32>(0.999));
        acc += (textureSampleLevel(blur_src, blur_sampler, uv_p, 0.0).rgb
              + textureSampleLevel(blur_src, blur_sampler, uv_n, 0.0).rgb) * weights[i];
    }
    return vec4<f32>(acc, 1.0);
}

// -----------------------------------------------------------------------------
// Liquid Glass Panel Pass (SDF mask, Snell bevel refraction, RGB dispersion,
// and directional + cursor-reactive specular caustics)
// -----------------------------------------------------------------------------

struct GlassUniforms {
    rect_min: vec2<f32>,
    rect_size: vec2<f32>,
    screen_size: vec2<f32>,
    light_dir: vec2<f32>,
    uv_min: vec2<f32>,
    uv_size: vec2<f32>,
    backdrop_min: vec2<f32>,
    backdrop_size: vec2<f32>,
    tint_color: vec4<f32>,
    corner_radius: f32,
    bevel_width: f32,
    ior_strength: f32,
    dispersion: f32,
    cursor_pos: vec2<f32>,
    spec_radius: f32,
    hover_t: f32,
};

@group(0) @binding(0) var<uniform> u: GlassUniforms;
@group(0) @binding(1) var tex_sharp: texture_2d<f32>;
@group(0) @binding(2) var tex_blurred: texture_2d<f32>;
@group(0) @binding(3) var tex_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let pad = 3.0;
    let px = (u.rect_min - vec2<f32>(pad)) + corners[vi] * (u.rect_size + vec2<f32>(pad * 2.0));
    let ndc = vec2<f32>(
        px.x / max(u.screen_size.x, 1.0) * 2.0 - 1.0,
        1.0 - px.y / max(u.screen_size.y, 1.0) * 2.0,
    );
    return vec4<f32>(ndc, 0.0, 1.0);
}

fn sd_rounded_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

fn linear_from_gamma_rgb(srgb: vec3<f32>) -> vec3<f32> {
    let cutoff = srgb < vec3<f32>(0.04045);
    let lower = srgb / vec3<f32>(12.92);
    let higher = pow((srgb + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(higher, lower, cutoff);
}

fn evaluate_liquid_glass(frag_xy: vec2<f32>) -> vec4<f32> {
    let half_size = u.rect_size * 0.5;
    let center = u.rect_min + half_size;
    let p = frag_xy - center;
    let r = min(u.corner_radius, min(half_size.x, half_size.y));

    let dist = sd_rounded_box(p, half_size, r);

    // Subtle soft drop shadow outside rounded panel (matching Serpantinum shadow = 0.13)
    if dist > 0.5 {
        if u.corner_radius >= 0.5 {
            let shadow_dist = sd_rounded_box(p - vec2<f32>(0.0, 1.5), half_size, r);
            if shadow_dist > 0.0 && shadow_dist < 3.0 {
                let sa = exp(-shadow_dist * 1.15) * (0.14 + 0.04 * u.hover_t);
                return vec4<f32>(0.0, 0.0, 0.0, sa);
            }
        }
        return vec4<f32>(0.0, 0.0, 0.0, -1.0);
    }

    let alpha_mask = clamp(0.5 - dist, 0.0, 1.0);

    // Analytical 2D normal pointing outward near the rounded bevel
    let eps = vec2<f32>(0.75, 0.0);
    let raw_grad = vec2<f32>(
        sd_rounded_box(p + eps.xy, half_size, r) - sd_rounded_box(p - eps.xy, half_size, r),
        sd_rounded_box(p + eps.yx, half_size, r) - sd_rounded_box(p - eps.yx, half_size, r),
    );
    let grad_len = length(raw_grad);
    let grad = select(vec2<f32>(0.0, -1.0), raw_grad / max(grad_len, 1e-5), grad_len > 1e-4);

    // Smooth bezel depth falloff inward from border
    let bevel_w = max(u.bevel_width, 1.0);
    let rim = select(0.0, pow(clamp(1.0 + dist / bevel_w, 0.0, 1.0), 2.2), u.bevel_width > 0.5);

    // Exact screen -> backdrop aspect-fill UV mapping + gentle center lens magnification
    let bsize = max(u.backdrop_size, vec2<f32>(1.0));
    let local_uv = clamp((frag_xy - u.backdrop_min) / bsize, vec2<f32>(0.0), vec2<f32>(1.0));
    let base_uv = u.uv_min + local_uv * u.uv_size;
    let center_pull = -(p / bsize) * u.uv_size * 0.032;
    let refract_vec = -grad * rim * u.ior_strength * u.uv_size;

    let uv_lo = u.uv_min + vec2<f32>(0.0015);
    let uv_hi = u.uv_min + u.uv_size - vec2<f32>(0.0015);

    // RGB chromatic aberration across the refracting curved bevel
    let uv_r = clamp(base_uv + center_pull + refract_vec * (1.0 + u.dispersion), uv_lo, uv_hi);
    let uv_g = clamp(base_uv + center_pull + refract_vec, uv_lo, uv_hi);
    let uv_b = clamp(base_uv + center_pull + refract_vec * (1.0 - u.dispersion), uv_lo, uv_hi);

    // Sharp rim meets frosted Gaussian interior
    let blur_mix = clamp(1.0 - rim * 0.72, 0.24, 0.94);
    let s_r = textureSampleLevel(tex_sharp, tex_sampler, uv_r, 0.0).r;
    let s_g = textureSampleLevel(tex_sharp, tex_sampler, uv_g, 0.0).g;
    let s_b = textureSampleLevel(tex_sharp, tex_sampler, uv_b, 0.0).b;
    let b_r = textureSampleLevel(tex_blurred, tex_sampler, uv_r, 0.0).r;
    let b_g = textureSampleLevel(tex_blurred, tex_sampler, uv_g, 0.0).g;
    let b_b = textureSampleLevel(tex_blurred, tex_sampler, uv_b, 0.0).b;

    var glass_rgb = vec3<f32>(
        mix(s_r, b_r, blur_mix),
        mix(s_g, b_g, blur_mix),
        mix(s_b, b_b, blur_mix),
    );

    // Adaptive frosted glaze: preserves high text contrast over bright snow/sky while keeping shadows crystal clear
    let bg_luma = dot(glass_rgb, vec3<f32>(0.299, 0.587, 0.114));
    let hover_tint_a = clamp(u.tint_color.a + 0.04 * u.hover_t + max(bg_luma - 0.42, 0.0) * 0.40, 0.0, 0.92);
    glass_rgb = mix(glass_rgb, u.tint_color.rgb, hover_tint_a);

    // Cursor-reactive light vector + rim caustics
    var eff_light = normalize(u.light_dir);
    var cursor_caustic = 0.0;
    if u.spec_radius > 1.0 && u.cursor_pos.x > -5000.0 {
        let to_cur = u.cursor_pos - frag_xy;
        let cd = length(to_cur);
        if cd > 1.0 && cd < u.spec_radius {
            let falloff = 1.0 - cd / u.spec_radius;
            let c_dir = to_cur / cd;
            eff_light = normalize(mix(eff_light, c_dir, 0.42 * falloff * (0.35 + 0.65 * u.hover_t)));
            cursor_caustic = pow(falloff, 2.2) * (0.14 + 0.12 * u.hover_t);
        }
    }

    // Crisp 1.1px directional silver-white bezel rim + subtle inner caustic band
    let light_align = max(dot(grad, eff_light), 0.0);
    let opp_align = max(dot(grad, -eff_light), 0.0);
    let crisp_edge = smoothstep(-1.35, 0.0, dist);
    let rim_spec = crisp_edge * (0.09 + 0.48 * pow(light_align, 1.8) + 0.16 * pow(opp_align, 2.2));
    let caustic = rim * (pow(light_align, 2.2) * 0.10 + cursor_caustic * 0.45) + crisp_edge * cursor_caustic * 0.35;

    let spec_color = vec3<f32>(0.93, 0.96, 1.0);
    glass_rgb = clamp(glass_rgb + spec_color * (rim_spec + caustic), vec3<f32>(0.0), vec3<f32>(1.0));

    return vec4<f32>(glass_rgb, alpha_mask);
}

@fragment
fn fs_main_gamma(@builtin(position) frag_coord: vec4<f32>) -> @location(0) vec4<f32> {
    let out_gamma = evaluate_liquid_glass(frag_coord.xy);
    if out_gamma.a < 0.0 {
        discard;
    }
    return vec4<f32>(out_gamma.rgb * out_gamma.a, out_gamma.a);
}

@fragment
fn fs_main_linear(@builtin(position) frag_coord: vec4<f32>) -> @location(0) vec4<f32> {
    let out_gamma = evaluate_liquid_glass(frag_coord.xy);
    if out_gamma.a < 0.0 {
        discard;
    }
    let lin = linear_from_gamma_rgb(out_gamma.rgb);
    return vec4<f32>(lin * out_gamma.a, out_gamma.a);
}
