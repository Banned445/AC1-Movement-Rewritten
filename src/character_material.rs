//! Character-only lighting path. Authored controls and template equations: RE/09 §9.
use bevy::{asset::embedded_asset, pbr::{ExtendedMaterial, MaterialExtension}, prelude::*, render::render_resource::{AsBindGroup, ShaderType}, shader::ShaderRef};
use crate::assets::character_material::{CharacterMaterial, Parameter};

pub const CHARACTER_MATERIAL_LAYERS: bool = true;
pub type CharacterSurface = ExtendedMaterial<StandardMaterial, CharacterLayers>;

#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect)]
pub struct LayerControls {
    pub multiply: Vec4,
    pub specular: Vec4,
    /// power, factor, rim power, Fresnel baseline
    pub highlight: Vec4,
    /// template style, skin specular tiling, eye normal blend, eye color blend
    pub options: Vec4,
    pub eye_color: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct CharacterLayers {
    #[uniform(100)]
    pub controls: LayerControls,
    #[texture(101)]
    #[sampler(102)]
    pub specular_map: Option<Handle<Image>>,
    #[texture(103)]
    #[sampler(104)]
    pub ramp: Option<Handle<Image>>,
    #[texture(105)]
    #[sampler(106)]
    pub multiply_map: Option<Handle<Image>>,
    #[texture(107, dimension = "cube")]
    #[sampler(108)]
    pub eye_cube: Option<Handle<Image>>,
}

impl MaterialExtension for CharacterLayers {
    fn fragment_shader() -> ShaderRef { "embedded://ac_port/character_material.wgsl".into() }
}

pub fn install(app: &mut App) {
    embedded_asset!(app, "character_material.wgsl");
    app.add_plugins(MaterialPlugin::<CharacterSurface>::default());
}

pub fn enabled() -> bool {
    CHARACTER_MATERIAL_LAYERS && std::env::var("AC_CHARACTER_MATERIAL_LAYERS").as_deref() != Ok("0")
}

pub fn controls(m: &CharacterMaterial) -> LayerControls {
    let eye_scalar = |id, default| match m.parameters.get(&id) { Some(Parameter::Scalar(x)) => *x, _ => default };
    let style = m.style().expect("validated character template");
    LayerControls {
        multiply: Vec4::from_array(m.color("MultiplyColor", [1.0; 4])) * m.scalar("MultiplyColorValue", 1.0),
        specular: Vec4::from_array(m.color("SpecularColor", [1.0; 4])),
        highlight: Vec4::new(
            if style == 4 { eye_scalar(799441376, 100.0) } else { m.scalar("SpecularPower", 1.0) },
            if style == 4 { eye_scalar(2606792976, 10.0) } else if style == 1 { 0.0 } else { m.scalar("SpecularFactor", 1.0) },
            m.scalar("Rim_Power", m.scalar("RimPower", 0.0)), m.scalar("Fresnel", 1.0)),
        options: Vec4::new(style as f32, m.scalar("SpecularTiling", 1.0), m.scalar("NormalMapBlend", 0.0), eye_scalar(1837413648, 0.0)),
        eye_color: Vec4::from_array(m.color("EyeColor", [1.0; 4])),
    }
}
