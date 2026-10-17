// The World's post effects on the frame (city/ambiance.rs `PostGrade`), in the game's gamma space:
// ColourControl, the colour matrix its vertex shader (0x12C72B0) builds, then ColorBalance.
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct PostGrade {
    // ColourControl rows (R, G, B): colour · xyz + w
    r: vec4<f32>,
    g: vec4<f32>,
    b: vec4<f32>,
    // ColorBalance shifts per range (rgb) and the levels (black, white, gamma, opacity)
    shadows: vec4<f32>,
    midtones: vec4<f32>,
    highlights: vec4<f32>,
    levels: vec4<f32>,
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

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let src = textureSample(screen_texture, texture_sampler, in.uv);
    var c = to_gamma(clamp(src.rgb, vec3(0.0), vec3(1.0)));
    if grade.enabled.x > 0.5 {
        c = clamp(vec3(dot(grade.r.xyz, c) + grade.r.w, dot(grade.g.xyz, c) + grade.g.w, dot(grade.b.xyz, c) + grade.b.w), vec3(0.0), vec3(1.0));
    }
    if grade.enabled.y > 0.5 {
        // PORT (hypothesis): the game bakes ColorBalance into a 3D LUT on the CPU (shader 0x12C6F20,
        // g_PreLutScale / g_PreLutOffset), not traced; shifts by luma range, then levels, blended by the opacity
        let l = dot(c, vec3(0.299, 0.587, 0.114));
        let ws = clamp(1.0 - 2.0 * l, 0.0, 1.0);
        let wh = clamp(2.0 * l - 1.0, 0.0, 1.0);
        let wm = 1.0 - ws - wh;
        var d = clamp(c + grade.shadows.rgb * ws + grade.midtones.rgb * wm + grade.highlights.rgb * wh, vec3(0.0), vec3(1.0));
        let black = grade.levels.x;
        let white = max(grade.levels.y, black + 1e-3);
        d = clamp((d - vec3(black)) / (white - black), vec3(0.0), vec3(1.0));
        d = pow(d, vec3(1.0 / max(grade.levels.z, 1e-3)));
        c = mix(c, d, grade.levels.w);
    }
    return vec4(to_linear(c), src.a);
}
