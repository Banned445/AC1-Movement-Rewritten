//! City materials and textures (RE/14 §2 material chain, RE/09 §4 TextureMap). Decoded on loader threads, cached by
//! resource id and shared by every placement that uses them.
//!
//! Textures stay BC1/BC3 compressed on the GPU when the adapter supports BC (as the game's D3D9 renderer uploads
//! them); the CPU decode is the fallback. PORT: Bevy PBR replaces the native pixel shaders, as in the Masyaf import.

use std::{collections::HashMap, sync::{Arc, Mutex, Weak}};

use bevy::render::render_resource::TextureFormat;

use super::archive::Archives;
use crate::assets::{ac_formats::{decode_bc, parse_texture_blocks}, forge::crc32, static_mesh::word};

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
        let data = Arc::new(MaterialData { name: material.name.clone(), template, diffuse, normal, water });
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
