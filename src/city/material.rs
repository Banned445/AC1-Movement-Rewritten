//! City materials and textures (RE/14 §2 material chain, RE/09 §4 TextureMap). Decoded on loader threads, cached by
//! resource id and shared by every placement that uses them.
//!
//! Textures stay BC1/BC3 compressed on the GPU when the adapter supports BC (as the game's D3D9 renderer uploads
//! them); the CPU decode is the fallback. PORT: Bevy PBR replaces the native pixel shaders, as in the Masyaf import.

use std::{collections::HashMap, sync::{Arc, Mutex, Weak}};

use bevy::render::render_resource::TextureFormat;

use super::archive::Archives;
use crate::assets::{ac_formats::{decode_bc, parse_texture_blocks}, character_material::{CharacterMaterial, Parameter}, forge::crc32, static_mesh::word};

/// A texture ready to wrap in a Bevy `Image`: all mips concatenated, largest first.
pub struct TextureData {
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    pub format: TextureFormat,
    pub data: Vec<u8>,
}

pub struct MaterialData {
    pub name: String,
    pub template: String,
    /// None for water (`GFX_Water_*` templates carry only a normal map).
    pub diffuse: Option<u32>,
    pub normal: Option<u32>,
    pub water: bool,
    /// The MultiBlender layers (base, layer 1, layer 2), empty for the single-texture templates.
    pub layers: Vec<BlendLayer>,
}

impl MaterialData {
    /// Every texture the material draws with: (TextureMap id, linear) — linear for normal maps.
    pub fn maps(&self) -> Vec<(u32, bool)> {
        let mut out: Vec<(u32, bool)> = self.diffuse.map(|d| (d, false)).into_iter().chain(self.normal.map(|n| (n, true))).collect();
        for l in &self.layers {
            for m in l.diffuse.map(|d| (d, false)).into_iter().chain(l.normal.map(|n| (n, true))) {
                if !out.iter().any(|o| o.0 == m.0) { out.push(m); }
            }
        }
        out
    }
}

/// One `Env_MultiBlender*` layer (RE/17 §12): its maps, tint and UV scale. The pixel shader blends
/// `lerp(lerp(base · BaseColor, layer1 · Layer1Color, w.x), layer2 · Layer2Color, w.y) · vertex colour`, w the vertex's
/// layer weights (`StaticMesh::blend`); a layer samples at (mesh UV or, for the `WorldUV[..]` layers, the world X, Y) · tile.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendLayer {
    pub diffuse: Option<u32>,
    pub normal: Option<u32>,
    pub color: [f32; 4],
    pub tile: [f32; 2],
    pub world: bool,
}

/// Materials are small and kept; texture pixels are only held weakly: a loaded cell carries the textures it needs
/// to the main thread, which uploads them and drops the CPU copy.
#[derive(Default)]
pub struct MaterialCache {
    pub materials: Mutex<HashMap<u32, Arc<MaterialData>>>,
    textures: Mutex<HashMap<u32, Weak<TextureData>>>,
}

impl MaterialCache {
    /// Decode material `id` (and its textures) once; later calls return the cached copy.
    pub fn material(&self, archives: &Archives, id: u32) -> Result<Arc<MaterialData>, String> {
        if let Some(m) = self.materials.lock().unwrap().get(&id) { return Ok(m.clone()); }
        let material = archives.get(id)?;
        if material.class_hash != crc32("Material") { return Err(format!("{id:#x} is not a Material")); }
        // Material__Read 0xA4E7C0: template reference at payload 8, primary TextureSet at 12.
        let template = archives.get(word(&material.payload, 8)?).map(|t| t.name.clone()).unwrap_or_default();
        let set = archives.get(word(&material.payload, 12)?)?;
        if set.class_hash != crc32("TextureSet") { return Err(format!("{} has no TextureSet", material.name)); }
        let channel = |name: &str| -> Result<Option<u32>, String> {
            for p in (8..set.payload.len().saturating_sub(3)).step_by(4) {
                let spec_id = word(&set.payload, p)?;
                if spec_id == 0 || !archives.contains(spec_id) { continue; }
                let spec = archives.get(spec_id)?;
                if spec.class_hash != crc32("TextureMapSpec") || !spec.name.contains(name) { continue; }
                // TextureMapSpec__Read 0xA15F70 ends with a handle and the typed TextureMap reference.
                let tex = word(&spec.payload, spec.payload.len().checked_sub(4).ok_or("truncated texture spec")?)?;
                return Ok(Some(tex));
            }
            Ok(None)
        };
        let water = template.contains("Water");
        let diffuse = channel("Diffuse")?;
        if diffuse.is_none() && !water { return Err(format!("{} has no decoded diffuse map", material.name)); }
        let normal = channel("Normal").ok().flatten();
        let own_set = set.id;
        // TextureSelector: mode 0 the material's own set, 1 an explicit set (both by channel), 2 an explicit map (RE/09 §8.12)
        let layers = match CharacterMaterial::parse(&material.payload) {
            Ok(m) => blend_layers(&template, &m.parameters, |p| match *p {
                Parameter::Map { mode: 0, channel, .. } => set_channel(archives, own_set, channel),
                Parameter::Map { mode: 1, channel, set, .. } => set_channel(archives, set, channel),
                Parameter::Map { mode: 2, map, .. } => Some(map).filter(|&m| m != 0),
                _ => None,
            }),
            Err(_) => Vec::new(),
        };
        let data = Arc::new(MaterialData { name: material.name.clone(), template, diffuse, normal, water, layers });
        self.materials.lock().unwrap().entry(id).or_insert(data.clone());
        Ok(data)
    }

    /// Texture pixels for upload (`linear` for normal maps).
    pub fn texture(&self, archives: &Archives, id: u32, linear: bool, bc: bool) -> Result<Arc<TextureData>, String> {
        if let Some(t) = self.textures.lock().unwrap().get(&id).and_then(Weak::upgrade) { return Ok(t); }
        let r = archives.get(id)?;
        let Some((width, height, mips, bc3, blocks)) = parse_texture_blocks(&r.payload) else {
            return uncompressed(&r.payload, linear).map(Arc::new).ok_or_else(|| format!("unsupported texture {}", r.name));
        };
        // wgpu wants block-aligned base dimensions for compressed formats.
        let data = if bc && width % 4 == 0 && height % 4 == 0 {
            let format = match (bc3, linear) {
                (false, false) => TextureFormat::Bc1RgbaUnormSrgb, (false, true) => TextureFormat::Bc1RgbaUnorm,
                (true, false) => TextureFormat::Bc3RgbaUnormSrgb, (true, true) => TextureFormat::Bc3RgbaUnorm,
            };
            TextureData { width, height, mips, format, data: blocks }
        } else {
            let block = if bc3 { 16 } else { 8 };
            let (mut w, mut h, mut p, mut out) = (width, height, 0usize, Vec::new());
            for _ in 0..mips {
                let bytes = (w.div_ceil(4).max(1) * h.div_ceil(4).max(1) * block) as usize;
                out.extend(decode_bc(blocks.get(p..p + bytes).ok_or("truncated texture mips")?, w, h, bc3));
                p += bytes;
                w = (w / 2).max(1); h = (h / 2).max(1);
            }
            TextureData { width, height, mips, format: if linear { TextureFormat::Rgba8Unorm } else { TextureFormat::Rgba8UnormSrgb }, data: out }
        };
        let data = Arc::new(data);
        self.textures.lock().unwrap().insert(id, Arc::downgrade(&data));
        Ok(data)
    }
}

/// The `Env_MultiBlender*` layers of a material (RE/17 §12): the templates' parameter names (`DiffuseBase` /
/// `DiffuseLayer1` / `DiffuseLayer2`, `Normal…`, `BaseColor` / `Layer1Color` / `Layer2Color`, `TileFactorBase` /
/// `TileFactorLayer1` / `TileFactorLayer2`, and `TileFactor` for the layers the `WorldUV[..]` suffix names), each map a
/// TextureSelector that `resolve` turns into a TextureMap id. Empty for other templates or when a layer has no diffuse.
pub fn blend_layers(template: &str, params: &HashMap<u32, Parameter>, resolve: impl Fn(&Parameter) -> Option<u32>) -> Vec<BlendLayer> {
    let Some(rest) = template.strip_prefix("Env_MultiBlender") else { return Vec::new() };
    // MultiBlender1: base and one layer (COLOR1.x); MultiBlender2: base and two layers
    let count = if rest.starts_with('1') { 2 } else { 3 };
    let world: Vec<usize> = rest.split_once("WorldUV[").and_then(|(_, l)| l.split(']').next())
        .map(|l| l.split(',').filter_map(|k| k.trim().parse().ok()).collect()).unwrap_or_default();
    let map = |name: &str| params.get(&crc32(name)).and_then(&resolve);
    let vec4 = |name: &str| match params.get(&crc32(name)) { Some(Parameter::Color(v)) => Some(*v), _ => None };
    let mut layers = Vec::new();
    for (k, (part, color)) in [("Base", "BaseColor"), ("Layer1", "Layer1Color"), ("Layer2", "Layer2Color")].into_iter().enumerate().take(count) {
        let Some(diffuse) = map(&format!("Diffuse{part}")) else { return Vec::new() };
        let world = world.contains(&k);
        // PORT: 1 when the material leaves a tile factor out (the template's default is not read, hypothesis)
        let tile = vec4(&if world { "TileFactor".to_string() } else { format!("TileFactor{part}") }).map_or([1.0; 2], |v| [v[0], v[1]]);
        layers.push(BlendLayer { diffuse: Some(diffuse), normal: map(&format!("Normal{part}")), color: vec4(color).unwrap_or([1.0; 4]), tile, world });
    }
    layers
}

/// A TextureSet's channel `k` (the k-th TextureMapSpec reference after the header, RE/09 §8.12) → its TextureMap.
fn set_channel(archives: &Archives, set: u32, k: u32) -> Option<u32> {
    let set = archives.get(set).ok().filter(|s| s.class_hash == crc32("TextureSet"))?;
    let spec = archives.get(word(&set.payload, 8 + 4 * k as usize).ok()?).ok().filter(|s| s.class_hash == crc32("TextureMapSpec"))?;
    // TextureMapSpec__Read 0xA15F70 ends with a handle and the typed TextureMap reference.
    word(&spec.payload, spec.payload.len().checked_sub(4)?).ok().filter(|&t| t != 0)
}

/// A 32-bit uncompressed TextureMap (the water normal map: 128 × 128, 8 mips, a u32 chain size of exactly
/// Σ w·h·4 inside the header, as for BC maps, RE/09 §4). Byte order B, G, R, A (D3D A8R8G8B8, hypothesis).
fn uncompressed(d: &[u8], linear: bool) -> Option<TextureData> {
    let (w, h, mips) = (word(d, 8).ok()?, word(d, 12).ok()?, word(d, 0x20).ok()?);
    if !(1..=4096).contains(&w) || !(1..=4096).contains(&h) || !(1..=13).contains(&mips) { return None; }
    let size: u32 = (0..mips).map(|k| (w >> k).max(1) * (h >> k).max(1) * 4).sum();
    let off = (0x20..0x100.min(d.len().saturating_sub(4))).find(|&o| word(d, o).ok() == Some(size))?;
    let mut data = d.get(off + 4..off + 4 + size as usize)?.to_vec();
    for px in data.chunks_exact_mut(4) { px.swap(0, 2); }
    Some(TextureData { width: w, height: h, mips, format: if linear { TextureFormat::Rgba8Unorm } else { TextureFormat::Rgba8UnormSrgb }, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(set: u32, channel: u32) -> Parameter { Parameter::Map { mode: 1, channel, set, map: 0 } }

    /// `blend_layers` follows the templates' parameter names: the `WorldUV[0,2]` layers take `TileFactor`, the others
    /// their own tile factor; MultiBlender1 has two layers; a layer without a diffuse map drops the blend.
    #[test]
    fn blend_layers_follow_the_templates() {
        let mut p = HashMap::new();
        p.insert(crc32("DiffuseBase"), Parameter::Map { mode: 0, channel: 0, set: 0, map: 0 });
        p.insert(crc32("NormalBase"), Parameter::Map { mode: 0, channel: 1, set: 0, map: 0 });
        p.insert(crc32("DiffuseLayer1"), map(10, 0));
        p.insert(crc32("NormalLayer1"), map(10, 1));
        p.insert(crc32("DiffuseLayer2"), map(20, 0));
        p.insert(crc32("TileFactor"), Parameter::Color([0.35, 0.35, 0.0, 0.0]));
        p.insert(crc32("TileFactorLayer1"), Parameter::Color([2.0, 3.0, 1.0, 1.0]));
        p.insert(crc32("Layer2Color"), Parameter::Color([0.5, 0.25, 1.0, 0.0]));
        // own set = 1: texture id = set · 100 + channel
        let resolve = |q: &Parameter| match *q { Parameter::Map { mode: 0, channel, .. } => Some(100 + channel), Parameter::Map { set, channel, .. } => Some(set * 100 + channel), _ => None };
        let l = blend_layers("Env_MultiBlender2_WorldUV[0,2]", &p, resolve);
        assert_eq!(l, vec![
            BlendLayer { diffuse: Some(100), normal: Some(101), color: [1.0; 4], tile: [0.35, 0.35], world: true },
            BlendLayer { diffuse: Some(1000), normal: Some(1001), color: [1.0; 4], tile: [2.0, 3.0], world: false },
            BlendLayer { diffuse: Some(2000), normal: None, color: [0.5, 0.25, 1.0, 0.0], tile: [0.35, 0.35], world: true },
        ]);
        let l = blend_layers("Env_MultiBlender1_UVScale[0,1]", &p, resolve);
        assert_eq!(l.len(), 2);
        assert!(!l[0].world && l[0].tile == [1.0, 1.0] && l[1].tile == [2.0, 3.0]);
        assert!(blend_layers("GEN_Standard", &p, resolve).is_empty());
        p.remove(&crc32("DiffuseLayer2"));
        assert!(blend_layers("Env_MultiBlender2", &p, resolve).is_empty());
    }

    /// The Souk's paving (`DAM_Multi_Avenue_01A`, Env_MultiBlender2_WorldUV[0,2]): the mud base and layer 2 in world UV
    /// at 0.35 / m, the cobbles of layer 1 in mesh UV, layer 2 tinted; every map a decodable texture.
    #[test]
    fn souk_ground_material_decodes_its_layers() {
        let Some(game) = crate::assets::find_game_dir() else { return };
        if !game.join(super::super::DAMASCUS.archive).is_file() { return; }
        let archives = super::super::archive::shared(&game, &[super::super::DAMASCUS.archive, "DataPC_Common.forge", "DataPC.forge"]).unwrap();
        let cache = MaterialCache::default();
        let m = cache.material(&archives, 0x6da4_94bf).unwrap();
        assert_eq!((m.name.as_str(), m.template.as_str()), ("DAM_Multi_Avenue_01A", "Env_MultiBlender2_WorldUV[0,2]"));
        assert_eq!(m.layers.len(), 3);
        let name = |id: Option<u32>| archives.get(id.unwrap()).unwrap().name.clone();
        for (l, n) in m.layers.iter().zip(&m.layers) { println!("{} / {} {:?} {:?} {}", name(l.diffuse), name(n.normal), l.tile, l.color, l.world); }
        assert_eq!(m.layers[0].diffuse, m.diffuse, "the base is the material's own set");
        assert_eq!(m.layers.iter().map(|l| (l.world, l.tile)).collect::<Vec<_>>(), [(true, [0.35, 0.35]), (false, [1.0, 1.0]), (true, [0.35, 0.35])]);
        assert_eq!(m.layers[2].color, [0.866_666_7, 0.827_451, 0.737_254_9, 0.0]);
        for l in &m.layers {
            assert!(l.normal.is_some());
            let t = cache.texture(&archives, l.diffuse.unwrap(), false, true).unwrap();
            assert!(t.width >= 64 && t.height >= 64);
        }
        assert_eq!(m.maps().len(), 6);
    }
}
