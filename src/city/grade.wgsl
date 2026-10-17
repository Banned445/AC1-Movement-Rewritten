// The World's post effects on the frame (city/ambiance.rs `PostGrade`), in the game's gamma space, as the game's
// 16³ LUT (`ColorBalance__BuildLut` 0xAD7B80) holds them: ColorBalance's per-channel curves, then ColourControl's
// colour matrix, clamped.
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct PostGrade {
    // ColourControl rows (R, G, B): colour · xyz + w
    r: vec4<f32>,
    g: vec4<f32>,
    b: vec4<f32>,
    // ColorBalance's curves at x = i / 15: R in 0..4, G in 4..8, B in 8..12
    curve: array<vec4<f32>, 12>,
    // x: ColourControl on, y: ColorBalance on
    enabled: vec4<f32>,
}

@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var texture_sampler: sampler;
@group(0) @binding(2) var<uniform> grade: PostGrade;

fn to_gamma(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3(0.0031308));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3(2.4));
    return select(hi, lo, c <= vec3(0.04045));
}

// the channel's curve between its 16 nodes (the LUT's linear filtering)
fn curve(k: u32, x: f32) -> f32 {
    let t = clamp(x, 0.0, 1.0) * 15.0;
    let i = min(u32(t), 14u);
    let a = grade.curve[k * 4u + i / 4u][i % 4u];
    let j = i + 1u;
    let b = grade.curve[k * 4u + j / 4u][j % 4u];
    return mix(a, b, t - f32(i));
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let src = textureSample(screen_texture, texture_sampler, in.uv);
    var c = to_gamma(clamp(src.rgb, vec3(0.0), vec3(1.0)));
    if grade.enabled.y > 0.5 {
        c = vec3(curve(0u, c.r), curve(1u, c.g), curve(2u, c.b));
    }
    if grade.enabled.x > 0.5 {
        c = vec3(dot(grade.r.xyz, c) + grade.r.w, dot(grade.g.xyz, c) + grade.g.w, dot(grade.b.xyz, c) + grade.b.w);
    }
    return vec4(to_linear(clamp(c, vec3(0.0), vec3(1.0))), src.a);
}
