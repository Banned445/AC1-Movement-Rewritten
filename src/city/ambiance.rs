//! The World's mood (RE/17 §11): `WorldAmbiance` (the fog and the post effects) and `LayeredSky` (the sun, the two ambient
//! cubes and the sky layers), read from the World file and applied to the scene.
//!
//! Both are reflected objects written field by field in their reflected order: a base class's fields first
//! (`GraphicObject`: one u32; `PostEffect`: the `Enabled` byte), an embedded object as `[u32 id][u32 class]` and its
//! fields, a list as `u32 count` and per element `[u8][u32 id][u32 class]` and its fields; `IColor` is four bytes in
//! B, G, R, A order. Field names recovered by CRC32 (`LayeredSky`: `SunLightAzimuth` …; `WorldFog`: `FogNear` …).
//!
//! Live (2026-10-16, a read-only scan of the running game): the direction toward the sun is
//! (sin az · cos el, cos az · cos el, sin el) in game space, the azimuth clockwise from north (+Y); the vector sits in
//! every object's shader constants. The terrain shader (embedded in the exe) fogs as
//! `lerp(lit, g_FogColor, saturate((d − c17.x) · c17.y) · c17.z)` and lights with `g_SunlightColor · N·L` plus
//! `g_AmbientCube`.

use bevy::asset::RenderAssetUsages;
use bevy::light::EnvironmentMapLight;
use bevy::prelude::*;
use bevy::core_pipeline::fullscreen_material::FullscreenMaterial;
use bevy::render::{extract_component::ExtractComponent, render_resource::{Extent3d, ShaderType, TextureDimension, TextureFormat, TextureViewDescriptor,
    TextureViewDimension}};
use bevy::shader::ShaderRef;

use crate::assets::forge::{crc32, Resource};

/// One ambient cube: a colour per direction (Bevy order +X, −X, +Y up, −Y, +Z, −Z) and its controls.
#[derive(Clone, Copy, Debug)]
pub struct AmbientCube {
    pub faces: [Vec3; 6],
    pub tint: Vec4,
    pub saturation: f32,
    pub scale: f32,
    pub bias: f32,
}

impl AmbientCube {
    /// The cube's colours with its controls applied (PORT, hypothesis: saturation toward the face's luma, then tint,
    /// scale and bias; the CPU side that fills `g_AmbientCube` is not traced).
    pub fn processed(&self) -> [Vec3; 6] {
        self.faces.map(|c| {
            let luma = c.dot(Vec3::new(0.299, 0.587, 0.114));
            let s = Vec3::splat(luma).lerp(c, self.saturation);
            (s * self.tint.truncate() * self.scale + Vec3::splat(self.bias)).max(Vec3::ZERO)
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Fog {
    pub enabled: bool,
    pub color: Vec3,
    /// `FogIntensity` (WorldFog +12): not in the shader constants (live 2026-10-16).
    pub intensity: f32,
    /// `FogDensity` (+16): the fog's most, `g_FogParams.z`.
    pub density: f32,
    pub near: f32,
    pub far: f32,
}

/// `ColourControlSettings` and `ColorBalanceSettings` of the PostEffects.
#[derive(Clone, Copy, Debug, Default)]
pub struct Grading {
    pub control: bool,
    pub hue: f32,
    pub saturation: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub balance: bool,
    /// The nine colour-balance shifts (three ranges × R, G, B), then black, white, gamma, opacity.
    pub shifts: [f32; 9],
    pub black: f32,
    pub white: f32,
    pub gamma: f32,
    pub opacity: f32,
}

#[derive(Clone, Debug)]
pub struct Ambiance {
    /// Toward the sun, Bevy space.
    pub sun_dir: Vec3,
    /// sRGB, 0..1.
    pub sun_color: Vec3,
    pub sun_intensity: f32,
    pub environment: AmbientCube,
    pub character: AmbientCube,
    pub use_character: bool,
    pub fog: Fog,
    pub grading: Grading,
    /// The sky layers: (Mesh id, Material override id).
    pub sky: Vec<(u32, u32)>,
    pub cloud_direction: f32,
    pub cloud_speed: f32,
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.i).ok_or("ambiance: truncated")?;
        self.i += 1;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32, String> {
        let v = u32::from_le_bytes(self.b.get(self.i..self.i + 4).ok_or("ambiance: truncated")?.try_into().unwrap());
        self.i += 4;
        Ok(v)
    }
    fn f32(&mut self) -> Result<f32, String> {
        self.u32().map(f32::from_bits)
    }
    fn vec4(&mut self) -> Result<Vec4, String> {
        Ok(Vec4::new(self.f32()?, self.f32()?, self.f32()?, self.f32()?))
    }
    /// An embedded object's header: `[u32 id][u32 class]`, the class checked.
    fn object(&mut self, class: &str) -> Result<(), String> {
        self.u32()?;
        let c = self.u32()?;
        if c != crc32(class) {
            return Err(format!("ambiance: expected {class} at {:#x}, found {c:#010x}", self.i - 4));
        }
        Ok(())
    }
    /// An embedded `IColor` as 0..1 RGB (stored B, G, R, A).
    fn icolor(&mut self) -> Result<Vec3, String> {
        self.object("IColor")?;
        let (b, g, r, _a) = (self.u8()?, self.u8()?, self.u8()?, self.u8()?);
        Ok(Vec3::new(r as f32, g as f32, b as f32) / 255.0)
    }
    fn cube(&mut self) -> Result<AmbientCube, String> {
        self.object("AmbientCube")?;
        // reflected order: Right, Left, Front, Back, Top, Down, Tint, Saturation, Scale, Bias, (one more float)
        let (right, left, front, back, top, down) = (self.vec4()?, self.vec4()?, self.vec4()?, self.vec4()?, self.vec4()?, self.vec4()?);
        let tint = self.vec4()?;
        let (saturation, scale, bias) = (self.f32()?, self.f32()?, self.f32()?);
        self.f32()?;
        // game +X right, +Y front (north), +Z top → Bevy +X, −Z, +Y
        let faces = [right, left, top, down, back, front].map(|v| v.truncate());
        Ok(AmbientCube { faces, tint, saturation, scale, bias })
    }
}

/// `LayeredSky` (reflected order, see the module notes).
fn layered_sky(b: &[u8]) -> Result<(Vec3, Vec3, f32, AmbientCube, AmbientCube, bool, Vec<(u32, u32)>, f32, f32), String> {
    let mut r = Reader { b, i: 0 };
    r.object("LayeredSky")?;
    r.u32()?; // GraphicObject
    let (az, el) = (r.f32()?.to_radians(), r.f32()?.to_radians());
    let sun_color = r.icolor()?;
    let sun_intensity = r.f32()?;
    r.u8()?;
    r.icolor()?;
    for _ in 0..6 { r.f32()?; }
    r.icolor()?;
    r.u32()?; r.u32()?; r.u32()?; r.f32()?; // the flare's type, texture, …, size
    r.icolor()?;
    r.u32()?; r.u32()?; r.f32()?; r.u32()?;
    let (cloud_direction, cloud_speed) = (r.f32()?, r.f32()?);
    let use_character = r.u8()? != 0;
    let environment = r.cube()?;
    let character = r.cube()?;
    let n = r.u32()?;
    let mut sky = Vec::new();
    for _ in 0..n.min(16) {
        r.u8()?;
        r.object("SkyLayer")?;
        sky.push((r.u32()?, r.u32()?));
    }
    // toward the sun in game space (azimuth clockwise from +Y), then to Bevy (x, z, −y)
    let g = Vec3::new(az.sin() * el.cos(), az.cos() * el.cos(), el.sin());
    let sun_dir = Vec3::new(g.x, g.z, -g.y).normalize();
    Ok((sun_dir, sun_color, sun_intensity, environment, character, use_character, sky, cloud_direction, cloud_speed))
}

fn world_ambiance(b: &[u8]) -> Result<(Fog, Grading), String> {
    let mut r = Reader { b, i: 0 };
    r.object("WorldAmbiance")?;
    r.u32()?; // the LayeredSky reference
    r.object("WorldFog")?;
    r.u32()?; // GraphicObject
    let enabled = r.u8()? != 0;
    r.object("Color")?;
    let c = r.vec4()?;
    let (intensity, density, near, far) = (r.f32()?, r.f32()?, r.f32()?, r.f32()?);
    let fog = Fog { enabled, color: c.truncate(), intensity, density, near, far };
    r.object("PostEffects")?;
    r.object("ColourControlSettings")?;
    let mut g = Grading { control: r.u8()? != 0, hue: r.f32()?, saturation: r.f32()?, brightness: r.f32()?, contrast: r.f32()?, ..default() };
    // the colour balance sits after settings this reader does not walk: found by its class hash
    let h = crc32("ColorBalanceSettings").to_le_bytes();
    if let Some(p) = b.windows(4).position(|w| w == h) {
        let mut r = Reader { b, i: p + 4 };
        g.balance = r.u8()? != 0;
        for s in &mut g.shifts { *s = r.f32()?; }
        (g.black, g.white, g.gamma, g.opacity) = (r.f32()?, r.f32()?, r.f32()?, r.f32()?);
    }
    Ok((fog, g))
}

/// The World file's ambiance.
pub fn parse(globals: &[std::sync::Arc<Resource>]) -> Result<Ambiance, String> {
    let sky = globals.iter().find(|r| r.class_hash == crc32("LayeredSky")).ok_or("no LayeredSky")?;
    let amb = globals.iter().find(|r| r.class_hash == crc32("WorldAmbiance")).ok_or("no WorldAmbiance")?;
    // the payloads start with the resource's own `[id][class]`
    let (sun_dir, sun_color, sun_intensity, environment, character, use_character, sky, cloud_direction, cloud_speed) = layered_sky(&full(sky))?;
    let (fog, grading) = world_ambiance(&full(amb))?;
    Ok(Ambiance { sun_dir, sun_color, sun_intensity, environment, character, use_character, fog, grading, sky, cloud_direction, cloud_speed })
}

/// The payload as stored, with the `[id][class]` header the forge reader keeps or strips.
fn full(r: &Resource) -> Vec<u8> {
    if r.payload.get(4..8) == Some(&r.class_hash.to_le_bytes()) {
        r.payload.clone()
    } else {
        let mut v = r.id.to_le_bytes().to_vec();
        v.extend(r.class_hash.to_le_bytes());
        v.extend(&r.payload);
        v
    }
}

/// A 1×1 cubemap of the processed cube (sRGB, faces in Bevy's order).
pub fn cube_image(cube: &AmbientCube) -> Image {
    let data: Vec<u8> = cube.processed().iter().flat_map(|c| {
        let c = c.min(Vec3::ONE);
        [(c.x * 255.0).round() as u8, (c.y * 255.0).round() as u8, (c.z * 255.0).round() as u8, 255]
    }).collect();
    Image {
        texture_view_descriptor: Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::Cube), ..default() }),
        ..Image::new(Extent3d { width: 1, height: 1, depth_or_array_layers: 6 }, TextureDimension::D2, data, TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD)
    }
}

/// The illuminance of a sun of intensity 1 (lux). PORT: the game's lighting is unitless (`g_SunlightColor · N·L` plus
/// the ambient cube); this scale and `ambient_intensity` keep that ratio under Bevy's camera exposure.
pub const SUN_LUX: f32 = 10_000.0;

/// The environment light's intensity for a cube read as radiance relative to the sun's `SUN_LUX`: a Lambertian surface
/// then gets `albedo · cube` against the sun's `albedo · colour · intensity · N·L`, the game's ratio.
pub fn ambient_intensity() -> f32 {
    SUN_LUX / std::f32::consts::PI
}

/// The camera exposure that maps `SUN_LUX / π` (a white Lambertian surface under a sun of intensity 1) to 1: the game's
/// unitless lighting, which it writes to an LDR target without tone mapping. `Exposure::exposure() = 2^-ev100 / 1.2`.
pub fn exposure() -> bevy::camera::Exposure {
    bevy::camera::Exposure { ev100: (SUN_LUX / std::f32::consts::PI / 1.2).log2() }
}

/// The game lights in gamma space (`texel · (sun · N·L + ambient)` on sRGB values); Bevy lights in linear space with
/// the colours linearised, so an intensity above 1 is linearised as well (PORT approximation of the gamma-space sum).
fn linear_intensity(i: f32) -> f32 {
    i.max(0.0).powf(2.2)
}

/// The environment light for the world's cube.
pub fn environment_light(images: &mut Assets<Image>, cube: &AmbientCube) -> EnvironmentMapLight {
    let h = images.add(cube_image(cube));
    EnvironmentMapLight { diffuse_map: h.clone(), specular_map: h, intensity: ambient_intensity(), ..default() }
}

/// The sky dome (follows the camera; `follow_sky`).
#[derive(Component)]
pub struct SkyDome;

/// The dome's radius in the scene (inside the camera's 1 km far plane).
const SKY_RADIUS: f32 = 900.0;

/// The World's post effects on the camera (`grade.wgsl`, a fullscreen pass in the game's gamma space).
#[derive(Component, ExtractComponent, Clone, Copy, ShaderType, Default, Debug)]
pub struct PostGrade {
    /// ColourControl rows (R, G, B): colour · xyz + w.
    pub r: Vec4,
    pub g: Vec4,
    pub b: Vec4,
    /// ColorBalance shifts per range (rgb) and its levels (black, white, gamma, opacity).
    pub shadows: Vec4,
    pub midtones: Vec4,
    pub highlights: Vec4,
    pub levels: Vec4,
    /// x: ColourControl on, y: ColorBalance on.
    pub enabled: Vec4,
}

impl FullscreenMaterial for PostGrade {
    fn fragment_shader() -> ShaderRef { "embedded://ac_port/city/grade.wgsl".into() }
}

/// ColourControl's matrix as its vertex shader (0x12C72B0, embedded in the exe) builds it from `g_Hue` (degrees),
/// `g_Saturation`, `g_Brightness`, `g_Contrast`: the saturation matrix on the luma weights (0.3086, 0.6094, 0.082)
/// scaled by brightness · contrast, turned by the hue about the grey axis, plus 0.5 · (1 − contrast). Rows R, G, B.
pub fn colour_control(hue: f32, saturation: f32, brightness: f32, contrast: f32) -> [Vec4; 3] {
    const L: [f32; 3] = [0.3086, 0.6094, 0.082];
    let m = contrast * brightness;
    let d = L.map(|l| m * ((1.0 - saturation) * l + saturation));
    let o = L.map(|l| m * (1.0 - saturation) * l);
    let angle = (hue / 360.0 + 0.5).fract() * std::f32::consts::TAU - std::f32::consts::PI;
    let (sin, cos) = angle.sin_cos();
    let a = (1.0 - cos) / 3.0 + sin * 0.577_350_26;
    let b = (1.0 - cos) / 3.0 - sin * 0.577_350_26;
    let c = 1.0 / 3.0 + 2.0 / 3.0 * cos;
    let t = 0.5 - 0.5 * contrast;
    [
        Vec4::new(o[0] * a + o[0] * b + d[0] * c, d[1] * a + o[1] * b + o[1] * c, o[2] * a + d[2] * b + o[2] * c, t),
        Vec4::new(o[0] * a + d[0] * b + o[0] * c, o[1] * a + o[1] * b + d[1] * c, d[2] * a + o[2] * b + o[2] * c, t),
        Vec4::new(d[0] * a + o[0] * b + o[0] * c, o[1] * a + d[1] * b + o[1] * c, o[2] * a + o[2] * b + d[2] * c, t),
    ]
}

/// The post grade for the World's `ColourControlSettings` and `ColorBalanceSettings`.
pub fn post_grade(g: &Grading) -> PostGrade {
    let [r, gr, b] = colour_control(g.hue, g.saturation, g.brightness, g.contrast);
    let s = &g.shifts;
    PostGrade { r, g: gr, b, shadows: Vec4::new(s[0], s[1], s[2], 0.0), midtones: Vec4::new(s[3], s[4], s[5], 0.0),
        highlights: Vec4::new(s[6], s[7], s[8], 0.0), levels: Vec4::new(g.black, g.white, g.gamma, g.opacity),
        enabled: Vec4::new(g.control as u8 as f32, g.balance as u8 as f32, 0.0, 0.0) }
}

/// Lights, fog, grading and the sky from the World's ambiance. Returns the entities it spawns (the map owns them).
#[allow(clippy::too_many_arguments)]
pub fn spawn_scene(a: &Ambiance, data: &super::decode::CityData, commands: &mut Commands, meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>, cams: &[Entity]) {
    let sun = Color::srgb(a.sun_color.x, a.sun_color.y, a.sun_color.z);
    commands.spawn((crate::map_menu::MapEntity,
        DirectionalLight { color: sun, illuminance: SUN_LUX * linear_intensity(a.sun_intensity), shadow_maps_enabled: true, ..default() },
        bevy::light::CascadeShadowConfigBuilder { num_cascades: 3, first_cascade_far_bound: 12.0, maximum_distance: 120.0, ..default() }.build(),
        Transform::default().looking_to(-a.sun_dir, Vec3::Y)));
    let env = environment_light(images, &a.environment);
    let grade = post_grade(&a.grading);
    // the shaders' fog (terrain pixel shader at 0x12D0C68): `lerp(lit, g_FogColor, saturate((d - c17.x) · c17.y) · c17.z)`;
    // live 2026-10-16 the device's constants hold c16 = (FogColor, 0) and c17 = (FogNear 15, 1 / (FogFar - FogNear),
    // FogDensity 0.5, 0): metres, the density as the most
    let fog = a.fog.enabled.then(|| DistanceFog {
        color: Color::srgba(a.fog.color.x, a.fog.color.y, a.fog.color.z, a.fog.density),
        falloff: FogFalloff::Linear { start: a.fog.near, end: a.fog.far },
        ..default()
    });
    for &cam in cams {
        let mut e = commands.entity(cam);
        e.insert((env.clone(), grade.clone(), exposure(), bevy::core_pipeline::tonemapping::Tonemapping::None));
        if let Some(f) = fog.clone() { e.insert(f); }
    }
    commands.insert_resource(ClearColor(Color::srgb(a.fog.color.x, a.fog.color.y, a.fog.color.z)));
    // the sky layers: their meshes unlit, centred on the camera
    for &(mesh, material) in &a.sky {
        let Ok(m) = data.meshes.get(&data.archives, mesh) else { continue };
        let texture = data.materials.material(&data.archives, material).ok()
            .and_then(|md| md.diffuse)
            .and_then(|t| data.materials.texture(&data.archives, t, false, data.bc).ok())
            .map(|t| images.add(super::stream::image(&t, false)));
        let mat = materials.add(StandardMaterial { base_color_texture: texture, unlit: true, fog_enabled: false, cull_mode: None,
            double_sided: true, depth_bias: -1.0e6, ..default() });
        let radius = m.positions.iter().map(|p| p.length()).fold(0.0f32, f32::max).max(1e-3);
        for sec in &m.sections {
            let mesh = meshes.add(super::stream::section_mesh(&m, None, &sec.indices));
            commands.spawn((crate::map_menu::MapEntity, SkyDome, Mesh3d(mesh), MeshMaterial3d(mat.clone()),
                bevy::light::NotShadowCaster, bevy::camera::visibility::NoFrustumCulling,
                Transform::from_matrix(Mat4::from_scale(Vec3::splat(SKY_RADIUS / radius)) * super::decode::TO_BEVY)));
        }
    }
}

/// The sky dome stays centred on the camera.
pub fn follow_sky(cams: Query<&GlobalTransform, With<crate::camera::MainCamera>>, mut domes: Query<&mut Transform, With<SkyDome>>) {
    let Some(cam) = cams.iter().next() else { return };
    for mut t in &mut domes {
        t.translation = cam.translation();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ColourControl's matrix (vertex shader 0x12C72B0): neutral settings are the identity; saturation 0 gives every
    /// row the luma weights; contrast scales about 0.5; a hue turn keeps grey grey.
    #[test]
    fn colour_control_matrix() {
        let id = colour_control(0.0, 1.0, 1.0, 1.0);
        for (k, row) in id.iter().enumerate() {
            for j in 0..4 { assert!((row[j] - if j == k { 1.0 } else { 0.0 }).abs() < 1e-5, "{id:?}"); }
        }
        let grey = colour_control(0.0, 0.0, 1.0, 1.0);
        for row in grey { assert!((row.truncate() - Vec3::new(0.3086, 0.6094, 0.082)).length() < 1e-5); }
        let c = colour_control(0.0, 1.0, 1.0, 2.0);
        assert!((c[0].x - 2.0).abs() < 1e-5 && (c[0].w + 0.5).abs() < 1e-5, "{c:?}");
        let h = colour_control(120.0, 1.0, 1.0, 1.0);
        for row in h { assert!((row.truncate().element_sum() - 1.0).abs() < 1e-4); }
        assert!((h[0].y - 1.0).abs() < 1e-4 && (h[1].z - 1.0).abs() < 1e-4 && (h[2].x - 1.0).abs() < 1e-4, "120 degrees: a channel turn {h:?}");
    }

    /// Damascus's World: the values decoded by hand from the install (RE/17 §11).
    #[test]
    fn damascus_ambiance_reads_the_sun_fog_and_cubes() {
        let Some(game) = crate::assets::find_game_dir() else { return };
        if !game.join(super::super::DAMASCUS.archive).is_file() { return; }
        let w = super::super::open_with(&game, &super::super::DAMASCUS, false, "").unwrap();
        let a = parse(&w.globals).unwrap();
        // 115° / 45°: toward the sun (sin 115 · cos 45, cos 115 · cos 45, sin 45) in game space
        let g = Vec3::new(a.sun_dir.x, -a.sun_dir.z, a.sun_dir.y);
        assert!((g - Vec3::new(0.641, -0.299, 0.707)).length() < 0.01, "{g}");
        assert!((a.sun_color - Vec3::new(229.0, 200.0, 160.0) / 255.0).length() < 1e-3 && (a.sun_intensity - 1.2).abs() < 1e-4);
        assert!(a.fog.enabled && (a.fog.near - 15.0).abs() < 1e-3 && (a.fog.far - 70.0).abs() < 1e-3 && (a.fog.intensity - 0.8).abs() < 1e-4
            && (a.fog.density - 0.5).abs() < 1e-4);
        // live 2026-10-16: the shader's g_AmbientCube (c24..c29: right, left, front, back, top, down) is the cube as read
        let live = [(0.4471, 0.3843, 0.3686), (0.4, 0.3412, 0.3137), (0.5020, 0.5294, 0.5294), (0.5373, 0.3922, 0.3451), (0.4784, 0.4157, 0.3765), (0.4549, 0.4627, 0.4667)];
        for (f, l) in a.environment.processed().iter().zip(live) {
            assert!((*f - Vec3::new(l.0, l.1, l.2)).length() < 0.002, "{f} {l:?}");
        }
        assert!((a.character.saturation - 0.5).abs() < 1e-4 && a.use_character);
        assert!(a.grading.control && (a.grading.saturation - 0.85).abs() < 1e-4);
        assert!(a.grading.balance && (a.grading.white - 0.93).abs() < 1e-4 && (a.grading.gamma - 0.8).abs() < 1e-4);
        assert_eq!(a.sky, vec![(0x2395_6265, 0x2395_78ED)]);
    }
}
