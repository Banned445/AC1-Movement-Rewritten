// The city's layered materials (Env_MultiBlender* templates, RE/17 §12): the native pixel shader's blend
//   albedo = lerp(lerp(base * BaseColor, layer1 * Layer1Color, w.x), layer2 * Layer2Color, w.y) * vertex colour
// with w the vertex's layer weights (UV_1 here) and each layer sampled at (mesh UV or world X, Y) * its tile factor;
// the normal maps are blended the same way before decoding.
// PORT: Bevy PBR lights the result (the native template's ambient cube, fog and specular are not ported).
#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, apply_normal_mapping, calculate_tbn_mikktspace, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT,
}

struct BlendControls {
    colors: array<vec4<f32>, 3>,
    tiles: array<vec4<f32>, 3>,
    options: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> blend: BlendControls;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var base_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var base_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var layer1_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var layer1_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var layer2_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var layer2_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var base_normal: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var base_normal_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var layer1_normal: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(110) var layer1_normal_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(111) var layer2_normal: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(112) var layer2_normal_sampler: sampler;

// A layer's UV: the mesh UV, or the game's world X, Y (Bevy x, -z) for a world-UV layer, times its tile factor.
fn layer_uv(k: u32, uv: vec2<f32>, world: vec2<f32>) -> vec2<f32> {
    let t = blend.tiles[k];
    return select(uv, world, t.z > 0.5) * t.xy;
}

// A missing normal map is the flat normal.
fn normal_texel(t: vec4<f32>, k: u32) -> vec3<f32> {
    return select(vec3<f32>(0.5, 0.5, 1.0), t.rgb, blend.tiles[k].w > 0.5);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var p = pbr_input_from_standard_material(in, is_front);
    let world = vec2<f32>(in.world_position.x, -in.world_position.z);
#ifdef VERTEX_UVS_B
    var w = in.uv_b;
#else
    var w = vec2<f32>(0.0);
#endif
    if blend.options.x < 2.5 { w.y = 0.0; }
    let uv0 = layer_uv(0u, in.uv, world);
    let uv1 = layer_uv(1u, in.uv, world);
    let uv2 = layer_uv(2u, in.uv, world);
    var albedo = textureSample(base_map, base_sampler, uv0).rgb * blend.colors[0].rgb;
    albedo = mix(albedo, textureSample(layer1_map, layer1_sampler, uv1).rgb * blend.colors[1].rgb, w.x);
    albedo = mix(albedo, textureSample(layer2_map, layer2_sampler, uv2).rgb * blend.colors[2].rgb, w.y);
    // base_color is the material colour times the vertex colour here
    p.material.base_color = vec4<f32>(p.material.base_color.rgb * albedo, p.material.base_color.a);
    p.material.base_color = alpha_discard(p.material, p.material.base_color);
#ifdef VERTEX_TANGENTS
    var nt = normal_texel(textureSample(base_normal, base_normal_sampler, uv0), 0u);
    nt = mix(nt, normal_texel(textureSample(layer1_normal, layer1_normal_sampler, uv1), 1u), w.x);
    nt = mix(nt, normal_texel(textureSample(layer2_normal, layer2_normal_sampler, uv2), 2u), w.y);
    let tbn = calculate_tbn_mikktspace(p.world_normal, in.world_tangent);
    let double_sided = (p.material.flags & STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT) != 0u;
    p.N = apply_normal_mapping(p.material.flags, tbn, double_sided, is_front, nt);
#endif
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(p);
    out.color = main_pass_post_lighting_processing(p, out.color);
    return out;
}
