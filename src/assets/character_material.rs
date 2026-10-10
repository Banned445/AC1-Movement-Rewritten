//! Authored character parameters (RE/09 §9). No game shader code is embedded.
use std::collections::HashMap;
use super::forge::crc32;
use super::ac_formats::{AcTexture, decode_bc};

#[derive(Clone, Debug)]
pub struct CubeTexture {
    pub size: u32,
    /// Mip-major, six faces per mip, as required by Bevy's Image loader.
    pub mips: Vec<Vec<u8>>,
}

/// Rank 9 TextureGradient's uncompressed pixel stream (archive layout, RE/09 §9).
pub fn parse_gradient(p: &[u8]) -> Option<AcTexture> {
    let word = |o| p.get(o..o + 4).map(|b: &[u8]| u32::from_le_bytes(b.try_into().unwrap()));
    if word(4)? != crc32("TextureGradient") { return None; }
    let (w, h) = (word(8)?, word(12)?);
    if !(1..=4096).contains(&w) || !(1..=4096).contains(&h) || word(24)? != 1 { return None; }
    let bytes = w.checked_mul(h)?.checked_mul(4)? as usize;
    if word(79)? as usize != bytes { return None; }
    let mut rgba = p.get(83..83 + bytes)?.to_vec();
    for pixel in rgba.chunks_exact_mut(4) { pixel.swap(0, 2); }
    // PixelFormat_RGBA8888 (+20 = 0) is uploaded as D3DFMT_A8R8G8B8, whose bytes are B, G, R, A
    let (address, filter) = super::ac_formats::texture_sampler_fields(p);
    Some(AcTexture { width: w, height: h, mips: vec![rgba], srgb: word(28)? == 1, address, filter })
}

/// Eye reflection map: six BC3 face chains (archive layout, RE/09 §9).
pub fn parse_eye_cube(p: &[u8]) -> Option<CubeTexture> {
    let word = |o| p.get(o..o + 4).map(|b: &[u8]| u32::from_le_bytes(b.try_into().unwrap()));
    let size = word(8)?;
    let count = word(32)? as usize;
    if word(4)? != crc32("TextureMap") || size != word(12)? || word(20)? != 5
        || word(24)? != 2 || !(1..=4096).contains(&size) || !(1..=13).contains(&count) { return None; }
    let lengths: Vec<_> = (0..count).map(|m| {
        let side = (size >> m).max(1);
        (side.div_ceil(4).max(1).pow(2) * 16) as usize
    }).collect();
    let chain: usize = lengths.iter().sum();
    if word(79)? as usize != chain * 6 { return None; }
    let data = p.get(83..83 + chain * 6)?;
    let mut offset = 0;
    let mut mips = Vec::new();
    for (m, bytes) in lengths.into_iter().enumerate() {
        let side = (size >> m).max(1);
        let mut mip = Vec::new();
        for face in 0..6 { mip.extend(decode_bc(&data[face * chain + offset..face * chain + offset + bytes], side, side, true)); }
        mips.push(mip);
        offset += bytes;
    }
    Some(CubeTexture { size, mips })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Parameter {
    Scalar(f32),
    Color([f32; 4]),
    /// TextureSelector__Deserialize 0xA14190: mode, channel, set, map.
    Map { mode: u32, channel: u32, set: u32, map: u32 },
}

#[derive(Clone, Debug)]
pub struct CharacterMaterial {
    pub template: u32,
    pub texture_set: u32,
    pub parameters: HashMap<u32, Parameter>,
    pub specular_map: Option<u32>,
    pub ramp: Option<u32>,
    pub multiply_map: Option<u32>,
    pub eye_cube: Option<u32>,
    pub diffuse_map: Option<u32>,
    pub normal_map: Option<u32>,
    pub inside_material: Option<u32>,
}

impl CharacterMaterial {
    /// Material__Deserialize 0xA4E7C0; parameter dictionary 0x931DC0/0x931820.
    /// This bounded reader accepts the Rank 9 layout and rejects unsupported encodings.
    pub fn parse(p: &[u8]) -> Result<Self, String> {
        let word = |o: usize| p.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        if word(4) != Some(crc32("Material")) { return Err("wrong material class".into()); }
        // PORT: bounded Rank 9 layout; variable native object lists are not supported.
        if word(49) != Some(0) || word(57) != Some(0) && word(57) != Some(1) {
            return Err("unsupported character material header".into());
        }
        let template = word(8).ok_or("truncated material template")?;
        let texture_set = word(12).ok_or("truncated texture set")?;
        let count = word(61).ok_or("truncated parameter count")?;
        if count > 64 { return Err("invalid material parameter count".into()); }
        let mut at = 65;
        let mut parameters = HashMap::new();
        for _ in 0..count {
            let id = word(at).ok_or("truncated parameter name")?;
            let class = word(at + 4).ok_or("truncated parameter class")?;
            let kind = (word(at + 8).ok_or("truncated parameter type")? >> 16) & 63;
            at += 12;
            let (value, size) = match kind {
                10 if class == 0 => (Parameter::Scalar(f32::from_bits(word(at).ok_or("truncated scalar")?)), 4),
                13 if class == 0 => {
                    let mut v = [0.0; 4];
                    for (i, x) in v.iter_mut().enumerate() { *x = f32::from_bits(word(at + i * 4).ok_or("truncated color")?); }
                    (Parameter::Color(v), 16)
                }
                19 if class == crc32("TextureSelector") && word(at + 4) == Some(class) => {
                    let mode = word(at + 8).ok_or("truncated map mode")?;
                    let channel = word(at + 12).ok_or("truncated map channel")?;
                    if mode > 2 || channel > 8 { return Err("unsupported material map binding".into()); }
                    (Parameter::Map { mode, channel, set: word(at + 16).ok_or("truncated map set")?, map: word(at + 20).ok_or("truncated map reference")? }, 24)
                }
                _ => return Err(format!("unsupported material parameter type {kind}")),
            };
            match value {
                Parameter::Scalar(v) if !v.is_finite() => return Err("nonfinite material scalar".into()),
                Parameter::Color(v) if v.iter().any(|x| !x.is_finite()) => return Err("nonfinite material color".into()),
                _ => {}
            }
            if parameters.insert(id, value).is_some() { return Err("duplicate material parameter".into()); }
            at += size;
        }
        if at != p.len() { return Err("unexpected material trailing data".into()); }
        let inside_material = word(53).filter(|&id| id != 0);
        Ok(Self { template, texture_set, parameters, specular_map: None, ramp: None, multiply_map: None, eye_cube: None, diffuse_map: None, normal_map: None, inside_material })
    }

    pub fn scalar(&self, name: &str, fallback: f32) -> f32 {
        match self.parameters.get(&crc32(name)) { Some(Parameter::Scalar(v)) => *v, _ => fallback }
    }

    pub fn color(&self, name: &str, fallback: [f32; 4]) -> [f32; 4] {
        match self.parameters.get(&crc32(name)) { Some(Parameter::Color(v)) => *v, _ => fallback }
    }

    /// Archive template identities, not a guess based on a mesh name (RE/09 §9).
    pub fn style(&self) -> Result<u32, String> {
        match self.template {
            433482562 | 2055988768 => Ok(0), // cloth / alpha cloth
            439116860 => Ok(1), // no specular inside cloth
            433482821 => Ok(2), // skin
            3292696482 | 661746657 | 439311561 => Ok(3), // weapon / throwing dagger / mouth
            326205072 => Ok(4), // eyes
            id => Err(format!("unsupported character template {id}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn material() -> Vec<u8> {
        let mut p = vec![0; 65];
        p[4..8].copy_from_slice(&crc32("Material").to_le_bytes());
        p[8..12].copy_from_slice(&433482562u32.to_le_bytes());
        p[57..61].copy_from_slice(&1u32.to_le_bytes());
        p[61..65].copy_from_slice(&1u32.to_le_bytes());
        for x in [crc32("SpecularPower"), 0, 10 << 16, 3.5f32.to_bits()] { p.extend(x.to_le_bytes()); }
        p
    }
    #[test]
    fn scalar_is_read_from_the_typed_table() {
        let m = CharacterMaterial::parse(&material()).unwrap();
        assert_eq!(m.scalar("SpecularPower", 0.0), 3.5);
        assert_eq!(m.style().unwrap(), 0);
    }
    #[test]
    fn malformed_parameters_fail_instead_of_silently_changing_appearance() {
        let p = material();
        for n in 0..p.len() { assert!(CharacterMaterial::parse(&p[..n]).is_err()); }
        let mut p = p;
        p[77..81].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
        assert!(CharacterMaterial::parse(&p).is_err());
    }
    #[test]
    fn gradient_channels_and_truncation_are_checked() {
        let mut p = vec![0; 87];
        for (at, value) in [(4, crc32("TextureGradient")), (8, 1), (12, 1), (24, 1), (79, 4)] {
            p[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        p[83..87].copy_from_slice(&[10, 20, 30, 40]);
        assert_eq!(parse_gradient(&p).unwrap().mips[0], [30, 20, 10, 40]);
        assert!(parse_gradient(&p[..86]).is_none());
    }
    #[test]
    fn cube_faces_keep_their_order_at_every_mip() {
        let mut p = vec![0; 83];
        for (at, value) in [(4, crc32("TextureMap")), (8, 4), (12, 4), (20, 5), (24, 2), (32, 2), (79, 192)] {
            p[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        for face in 0..6u8 {
            for mip in 0..2 {
                let mut block = [0; 16];
                block[0] = face * 10 + mip;
                block[1] = block[0];
                p.extend(block);
            }
        }
        let cube = parse_eye_cube(&p).unwrap();
        for (mip, pixels) in cube.mips.iter().enumerate() {
            let face_bytes = (4usize >> mip).pow(2) * 4;
            for face in 0..6 { assert_eq!(pixels[face * face_bytes + 3], (face * 10 + mip) as u8); }
        }
        assert!(parse_eye_cube(&p[..p.len() - 1]).is_none());
    }
}
