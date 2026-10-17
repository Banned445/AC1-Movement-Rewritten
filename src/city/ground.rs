//! The city's layered materials (`Env_MultiBlender*` templates, RE/17 §12): up to three diffuse / normal layers blended by
//! the vertex's layer weights, each sampled at the mesh UV or the world X, Y times its tile factor.
//! PORT: Bevy PBR lights the blended albedo and normal (the native template's lighting, fog and specular are not ported).
use bevy::{pbr::{ExtendedMaterial, MaterialExtension}, prelude::*, render::render_resource::{AsBindGroup, ShaderType}, shader::ShaderRef};

use super::material::BlendLayer;

pub type GroundSurface = ExtendedMaterial<StandardMaterial, GroundBlend>;

#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect)]
pub struct BlendControls {
    /// Per layer: the tint (`BaseColor`, `Layer1Color`, `Layer2Color`).
    pub colors: [Vec4; 3],
    /// Per layer: tile x, y; z 1 for a world-UV layer; w 1 when the layer has a normal map.
    pub tiles: [Vec4; 3],
    /// x: the layer count (2 or 3).
    pub options: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct GroundBlend {
    #[uniform(100)]
    pub controls: BlendControls,
    #[texture(101)]
    #[sampler(102)]
    pub base: Option<Handle<Image>>,
    #[texture(103)]
    #[sampler(104)]
    pub layer1: Option<Handle<Image>>,
    #[texture(105)]
    #[sampler(106)]
    pub layer2: Option<Handle<Image>>,
    #[texture(107)]
    #[sampler(108)]
    pub base_normal: Option<Handle<Image>>,
    #[texture(109)]
    #[sampler(110)]
    pub layer1_normal: Option<Handle<Image>>,
    #[texture(111)]
    #[sampler(112)]
    pub layer2_normal: Option<Handle<Image>>,
}

impl MaterialExtension for GroundBlend {
    fn fragment_shader() -> ShaderRef { "embedded://ac_port/city/ground.wgsl".into() }
}

pub fn install(app: &mut App) {
    bevy::asset::embedded_asset!(app, "ground.wgsl");
    app.add_plugins(MaterialPlugin::<GroundSurface>::default());
}

/// The shader controls for `layers` (2 or 3 of them).
pub fn controls(layers: &[BlendLayer]) -> BlendControls {
    let mut c = BlendControls { options: Vec4::new(layers.len() as f32, 0.0, 0.0, 0.0), ..default() };
    for (k, l) in layers.iter().enumerate().take(3) {
        c.colors[k] = Vec4::from_array(l.color);
        c.tiles[k] = Vec4::new(l.tile[0], l.tile[1], if l.world { 1.0 } else { 0.0 }, if l.normal.is_some() { 1.0 } else { 0.0 });
    }
    c
}
