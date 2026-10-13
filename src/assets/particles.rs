//! Particle effect data: the contact FX of the ContactTable, the FX that spawn particle entities, and the
//! ParticleSystem / ParticleEmitter they draw with (all in DataPC.forge "Rank 9"). RE/22.
//!
//! - ContactTable_AlTair: one group per surface material (CollisionMaterial +8; 4001 = Default). Each group lists, per
//!   ContactEventTypeHuman, the FX to spawn and the sound (ContactManager 0x55B1E0; group lookup 0x5AFB90).
//! - FX (class `FX`): FXSpawnedEntity records, each an entity spawned at the FX target with a local matrix for a
//!   lifetime (FXSpawnedEntity__Serialize 0x69C850 over FXElement 0x6A3DD0: start +4, duration +8).
//! - Entity → ParticleSystem (serializer 0xA541A0) → ParticleEmitter (serializer 0xA83550, 87 reflected properties).

use std::collections::HashMap;
use std::path::Path;

use super::forge::{crc32, Forge};

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn f32_at(b: &[u8], o: usize) -> Option<f32> {
    u32_at(b, o).map(f32::from_bits)
}

/// A byte reader over a serialized object.
struct R<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn u8(&mut self) -> Result<u8, String> {
        let v = *self.d.get(self.p).ok_or("truncated particle data")?;
        self.p += 1;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32, String> {
        let v = u32_at(self.d, self.p).ok_or("truncated particle data")?;
        self.p += 4;
        Ok(v)
    }
    fn f(&mut self) -> Result<f32, String> {
        self.u32().map(f32::from_bits)
    }
    fn fs<const N: usize>(&mut self) -> Result<[f32; N], String> {
        let mut out = [0.0; N];
        for v in &mut out {
            *v = self.f()?;
        }
        Ok(out)
    }
    /// An embedded IColor (id, class, then B, G, R, A bytes): returned as the game's ARGB u32.
    fn color(&mut self) -> Result<u32, String> {
        self.u32()?;
        self.u32()?;
        self.u32()
    }
}

/// ParticleEmitter::EmitterShape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EmitterShape {
    Point,
    Rectangle,
    Circle,
    Box,
    Sphere,
    Cylinder,
}

/// ParticleEmitter::FlowMode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlowMode {
    Once,
    Loop,
    SpawnOnRequest,
}

/// ParticleEmitter::DirectionMode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DirectionMode {
    EmitterZ,
    FromCenter,
    ToCenter,
}

/// BlendMode (Blend_Copy … Blend_Dodge).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlendMode {
    Copy,
    Additive,
    Subtractive,
    Modulate,
    Alpha,
    Dodge,
}

/// ParticleEmitter (offsets are the in-memory fields the serializer fills; names from the reflection data).
#[derive(Clone, Debug)]
pub struct EmitterDef {
    /// +80: the flag word (ParticleType bit 0, EmitterShape 1-3, FlowMode 4-5, LocalParticles 6, kill below ZMin 7,
    /// DirectionMode 12-13, CameraRelativeSpawn 14, TextureAnimMode 15-16, random start frame 17, flip U / V 19 / 20,
    /// BlendMode 21-23, LightingMode 24-25, motion stretch 27, ParticleEffect 28).
    pub flags: u32,
    pub shape: EmitterShape,
    pub flow: FlowMode,
    pub direction: DirectionMode,
    pub blend: BlendMode,
    /// ParticleType 3D (scale keys) rather than 2D (size keys).
    pub three_d: bool,
    pub local: bool,
    pub kill_below_zmin: bool,
    pub stretch: bool,
    /// +16: the emitter's local matrix (game space, row-major as stored).
    pub local_matrix: [f32; 16],
    /// +120: shape sizes (rectangle w / h, circle / sphere / cylinder radius, box x / y / z, cylinder height).
    pub shape_size: [f32; 3],
    /// +108 ZMin, +136 Delay, +140 MaxParticleCount, +144 ParticleRate (per second).
    pub zmin: f32,
    pub delay: f32,
    pub max_count: u32,
    pub rate: f32,
    /// +148 Acceleration (game space, m/s²).
    pub acceleration: [f32; 3],
    /// +503: each key draws its own random channel (else one channel for all keys).
    pub per_key_random: bool,
    /// +200 / +204 Min / MaxVelocity, +208 DirectionVariation (rad), +216 / +220 Min / MaxRotation.
    pub velocity: [f32; 2],
    pub direction_variation: [f32; 2],
    pub rotation: [f32; 2],
    /// +224 / +236 Min / MaxPhaseDuration (birth, life, death).
    pub phase_min: [f32; 3],
    pub phase_max: [f32; 3],
    /// +248 / +280 Min / MaxSize per step (birth, begin life, end life, death), w × h.
    pub size_min: [[f32; 2]; 4],
    pub size_max: [[f32; 2]; 4],
    /// +312 / +360 Min / MaxScale per step (3D particles).
    pub scale_min: [[f32; 3]; 4],
    pub scale_max: [[f32; 3]; 4],
    /// +460 Color per step (ARGB).
    pub color: [u32; 4],
    /// +408 / +424 Min / MaxRotationSpeed per step (rad/s).
    pub rotation_speed_min: [f32; 4],
    pub rotation_speed_max: [f32; 4],
    /// +444 TextureMap.
    pub texture: u32,
    /// +104 / +132 / +448: motion-stretch length and the stretch fades (0xA84130). PORT: motion stretch is not drawn; no
    /// contact FX sets +80 bit 27.
    #[allow(dead_code)]
    pub stretch_length: f32,
    #[allow(dead_code)]
    pub stretch_fade: f32,
    #[allow(dead_code)]
    pub stretch_fade2: f32,
    /// +476 WindForce: Active, Scale.
    pub wind: (bool, f32),
}

impl EmitterDef {
    /// The longest life (accessor at 0xA7FCC0: the three MaxPhaseDurations).
    #[allow(dead_code)]
    pub fn max_life(&self) -> f32 {
        self.phase_max.iter().sum()
    }
}

/// ParticleSystem.
#[derive(Clone, Debug)]
pub struct ParticleSystemDef {
    pub emitters: Vec<EmitterDef>,
    /// +20 ParticleRateFactor, +24 DurationFactor, +28 VelocityFactor, +32 AlphaFactor, +36 ColorFactor (ARGB).
    pub rate_factor: f32,
    pub duration_factor: f32,
    pub velocity_factor: f32,
    pub alpha_factor: f32,
    /// Not applied by the quad builder (0xA228D0 / 0xA84130 write the particle colour only); neutral (white) in every
    /// contact FX.
    #[allow(dead_code)]
    pub color_factor: u32,
}

/// ParticleEmitter__Serialize 0xA83550 (fields in stream order).
fn parse_emitter(r: &mut R) -> Result<EmitterDef, String> {
    // the flag word, field by field (+80)
    let mut flags = 0u32;
    let mut put = |v: u32, shift: u32, bits: u32| flags = flags & !(((1 << bits) - 1) << shift) | (v & ((1 << bits) - 1)) << shift;
    put(r.u32()?, 0, 1);
    put(r.u32()?, 1, 3);
    put(r.u32()?, 4, 2);
    for b in 6..=11 {
        put(r.u8()? as u32, b, 1);
    }
    put(r.u32()?, 12, 2);
    put(r.u8()? as u32, 14, 1);
    put(r.u32()?, 15, 2);
    for b in 17..=20 {
        put(r.u8()? as u32, b, 1);
    }
    put(r.u32()?, 21, 3);
    put(r.u32()?, 24, 2);
    put(r.u8()? as u32, 26, 1);
    put(r.u8()? as u32, 27, 1);
    put(r.u32()?, 28, 1);
    put(r.u8()? as u32, 29, 1);
    r.u8()?; // +500 bit 0
    r.u8()?; // +500 bits 1..
    r.u8()?; // +502
    let _v92: [f32; 3] = r.fs()?;
    r.color()?; // +452 LightColor
    r.color()?; // +456 LightAmbient
    let _fades: [f32; 2] = r.fs()?; // +112 DepthFadeScale, +116 NearFadeScale
    let local_matrix: [f32; 16] = r.fs()?;
    let shape_size: [f32; 3] = r.fs()?;
    let zmin = r.f()?;
    let delay = r.f()?;
    let max_count = r.u32()?;
    let rate = r.f()?;
    let acceleration: [f32; 3] = r.fs()?;
    let _weight = (r.u32()?, r.u32()?); // +160 / +164
    let per_key_random = r.u8()? != 0;
    let velocity: [f32; 2] = r.fs()?;
    let direction_variation: [f32; 2] = r.fs()?;
    let rotation: [f32; 2] = r.fs()?;
    let phase_min: [f32; 3] = r.fs()?;
    let phase_max: [f32; 3] = r.fs()?;
    let mut size_min = [[0.0; 2]; 4];
    let mut size_max = [[0.0; 2]; 4];
    for k in &mut size_min {
        *k = r.fs()?;
    }
    for k in &mut size_max {
        *k = r.fs()?;
    }
    let mut scale_min = [[0.0; 3]; 4];
    let mut scale_max = [[0.0; 3]; 4];
    for k in &mut scale_min {
        *k = r.fs()?;
    }
    for k in &mut scale_max {
        *k = r.fs()?;
    }
    let mut color = [0u32; 4];
    for c in &mut color {
        *c = r.color()?;
    }
    let rotation_speed_min: [f32; 4] = r.fs()?;
    let rotation_speed_max: [f32; 4] = r.fs()?;
    let _mesh = r.u32()?; // +440
    let texture = r.u32()?; // +444
    r.u8()?; // +505 m_ReceiveTrailBorn
    r.u8()?; // +504
    let _f84 = r.f()?; // +84 (texture frame rate)
    let _glow = r.f()?; // +88 GlowIntensity
    let stretch_length = r.f()?; // +104
    let stretch_fade = r.f()?; // +132
    r.f()?; // +448 (read twice by the serializer)
    let stretch_fade2 = r.f()?; // +448
    r.u32()?; // WindForce: id
    r.u32()?; //   class
    let active = r.u8()? != 0;
    let scale = r.f()?;
    r.u8()?;
    r.u8()?;
    let _cyclone: [f32; 8] = r.fs()?; // +168
    let spawns = r.u32()?; // +492 ParticleEmitterSpawnData
    for _ in 0..spawns {
        // embedded (id, class), ObjectPtr to the emitter it spawns, u32 SpawnEvent, u8 ParticleCount (0xA804B0)
        r.u32()?;
        r.u32()?;
        match r.u8()? {
            2 => {
                r.u32()?;
            }
            3 => {}
            _ => return Err("inline spawn emitter".into()),
        }
        r.u32()?;
        r.u8()?;
    }
    let shape = match (flags >> 1) & 7 {
        1 => EmitterShape::Rectangle,
        2 => EmitterShape::Circle,
        3 => EmitterShape::Box,
        4 => EmitterShape::Sphere,
        5 => EmitterShape::Cylinder,
        _ => EmitterShape::Point,
    };
    let flow = match (flags >> 4) & 3 {
        1 => FlowMode::Loop,
        2 => FlowMode::SpawnOnRequest,
        _ => FlowMode::Once,
    };
    let direction = match (flags >> 12) & 3 {
        1 => DirectionMode::FromCenter,
        2 => DirectionMode::ToCenter,
        _ => DirectionMode::EmitterZ,
    };
    let blend = match (flags >> 21) & 7 {
        0 => BlendMode::Copy,
        1 => BlendMode::Additive,
        2 => BlendMode::Subtractive,
        3 => BlendMode::Modulate,
        5 => BlendMode::Dodge,
        _ => BlendMode::Alpha,
    };
    Ok(EmitterDef {
        flags,
        shape,
        flow,
        direction,
        blend,
        three_d: flags & 1 != 0,
        local: flags & 0x40 != 0,
        kill_below_zmin: flags & 0x80 != 0,
        stretch: flags & 0x800_0000 != 0,
        local_matrix,
        shape_size,
        zmin,
        delay,
        max_count,
        rate,
        acceleration,
        per_key_random,
        velocity,
        direction_variation,
        rotation,
        phase_min,
        phase_max,
        size_min,
        size_max,
        scale_min,
        scale_max,
        color,
        rotation_speed_min,
        rotation_speed_max,
        texture,
        stretch_length,
        stretch_fade,
        stretch_fade2,
        wind: (active, scale),
    })
}

/// ParticleSystem__Serialize 0xA541A0 on a ParticleSystem payload (id, class, then the fields).
pub fn parse_particle_system(d: &[u8]) -> Result<ParticleSystemDef, String> {
    if u32_at(d, 4) != Some(crc32("ParticleSystem")) {
        return Err("not a ParticleSystem".into());
    }
    let mut r = R { d, p: 8 };
    r.u32()?; // base +8
    let n = r.u32()?;
    let mut emitters = Vec::new();
    for _ in 0..n {
        if r.u8()? != 0 {
            return Err("ParticleEmitter is not inline".into());
        }
        r.u32()?;
        if r.u32()? != crc32("ParticleEmitter") {
            return Err("expected a ParticleEmitter".into());
        }
        emitters.push(parse_emitter(&mut r)?);
    }
    let rate_factor = r.f()?;
    let duration_factor = r.f()?;
    let velocity_factor = r.f()?;
    let color_factor = r.color()?;
    let alpha_factor = r.f()?;
    if r.p != d.len() {
        return Err(format!("ParticleSystem: {} of {} bytes read", r.p, d.len()));
    }
    Ok(ParticleSystemDef { emitters, rate_factor, duration_factor, velocity_factor, alpha_factor, color_factor })
}

/// One entity an FX spawns.
#[derive(Clone, Debug, PartialEq)]
pub struct FxSpawn {
    pub entity: u32,
    /// FXElement +4 start and +8 duration (s).
    pub start: f32,
    pub duration: f32,
    /// FXSpawnedEntity +32: the entity's matrix relative to the FX target (game space, row-major).
    pub local: [f32; 16],
}

/// The FXSpawnedEntity records of an FX payload. Each is found by its class and then by the entity reference that
/// precedes its matrix.
/// PORT: the records' FXProperty bindings (curves and linked values that drive the spawned entity's fields, e.g. the
/// grass FX setting an emitter count) are not read; the entity runs with its authored values.
pub fn parse_fx(d: &[u8], is_entity: &dyn Fn(u32) -> bool) -> Vec<FxSpawn> {
    let class = crc32("FXSpawnedEntity").to_le_bytes();
    let starts: Vec<usize> = (0..d.len().saturating_sub(4)).filter(|&p| d[p..p + 4] == class).collect();
    let mut out = Vec::new();
    for (i, &p) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(d.len());
        for q in p + 8..end.saturating_sub(68) {
            let Some(id) = u32_at(d, q) else { break };
            if !is_entity(id) {
                continue;
            }
            let m: Vec<f32> = (0..16).filter_map(|k| f32_at(d, q + 4 + 4 * k)).collect();
            if m.len() == 16 && m[3] == 0.0 && m[7] == 0.0 && m[11] == 0.0 && (m[15] - 1.0).abs() < 1e-4 {
                let (Some(start), Some(duration)) = (f32_at(d, q - 8), f32_at(d, q - 4)) else { break };
                out.push(FxSpawn { entity: id, start, duration, local: m.try_into().unwrap() });
                break;
            }
        }
    }
    out
}

/// The ParticleSystem an Entity draws (its Visual's drawable): the first reference that names one.
/// PORT: found by scanning the entity for a ParticleSystem id rather than walking its component list.
pub fn entity_particle_system(d: &[u8], is_system: &dyn Fn(u32) -> bool) -> Option<u32> {
    (8..d.len().saturating_sub(4)).filter_map(|p| u32_at(d, p)).find(|&id| is_system(id))
}

/// The ContactTable's FX per surface material: material id → FX ids per ContactEventTypeHuman.
pub fn contact_fx(table: &[u8]) -> HashMap<u32, [Vec<u32>; 15]> {
    const GROUP: u32 = 0xE1CB_F60F;
    const ENTRY: u32 = 0x1133_7755;
    const SOUND: u32 = 0x54A4_C59C;
    let at = |p: usize| u32_at(table, p).unwrap_or(0);
    let groups: Vec<usize> = (0..table.len().saturating_sub(4)).filter(|&p| at(p) == GROUP).collect();
    let mut out = HashMap::new();
    for (gi, &g) in groups.iter().enumerate() {
        let material = at(g + 4);
        let end = groups.get(gi + 1).copied().unwrap_or(table.len());
        let mut types: [Vec<u32>; 15] = Default::default();
        let entries: Vec<usize> = (g..end.saturating_sub(4)).filter(|&p| at(p) == ENTRY).collect();
        for &e in &entries {
            // ContactTypeTable entry: u32 FX count, the FX references, then its ContactSound (type at +37)
            let n = at(e + 4) as usize;
            let fx: Vec<u32> = (0..n.min(8)).map(|k| at(e + 8 + 4 * k)).collect();
            let Some(cs) = (e..(e + 256).min(end)).find(|&p| at(p) == SOUND) else { continue };
            if let Some(slot) = types.get_mut(at(cs + 37) as usize) {
                *slot = fx;
            }
        }
        out.insert(material, types);
    }
    out
}

/// The 256-entry random table the particle code indexes by seed and channel (`flt_192E0D8`, read by 0x946740),
/// read from the user's own AssassinsCreed_Dx9.exe (static data; mapped through the PE section table).
pub fn particle_random_table(game_dir: &Path) -> Option<[f32; 256]> {
    const VA: u32 = 0x0192_E0D8;
    let exe = std::fs::read(game_dir.join("AssassinsCreed_Dx9.exe")).ok()?;
    let pe = u32_at(&exe, 0x3C)? as usize;
    if exe.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let sections = u16::from_le_bytes(exe.get(pe + 6..pe + 8)?.try_into().ok()?) as usize;
    let opt = u16::from_le_bytes(exe.get(pe + 20..pe + 22)?.try_into().ok()?) as usize;
    let image_base = u32_at(&exe, pe + 24 + 28)?;
    let rva = VA - image_base;
    let table = pe + 24 + opt;
    for s in 0..sections {
        let h = table + 40 * s;
        let (vsize, vaddr, raw_size, raw) = (u32_at(&exe, h + 8)?, u32_at(&exe, h + 12)?, u32_at(&exe, h + 16)?, u32_at(&exe, h + 20)?);
        if rva >= vaddr && rva + 1024 <= vaddr + vsize.max(raw_size) && rva + 1024 - vaddr <= raw_size {
            let off = (raw + rva - vaddr) as usize;
            let mut out = [0f32; 256];
            for (k, v) in out.iter_mut().enumerate() {
                *v = f32_at(&exe, off + 4 * k)?;
            }
            // the table holds randoms in [0, 1)
            return out.iter().all(|v| (0.0..1.0).contains(v)).then_some(out);
        }
    }
    None
}

/// Everything the contact FX need, from the install.
#[derive(Default)]
pub struct ContactFxData {
    pub groups: HashMap<u32, [Vec<u32>; 15]>,
    pub fx: HashMap<u32, Vec<FxSpawn>>,
    /// Entity id → its particle system.
    pub entities: HashMap<u32, u32>,
    pub systems: HashMap<u32, ParticleSystemDef>,
    pub textures: HashMap<u32, super::ac_formats::AcTexture>,
    pub random: Option<[f32; 256]>,
}

/// The ContactTable, its FX, their entities and particle systems ("Rank 9"), their textures, and the random table.
pub fn load_contact_fx(game_dir: &Path) -> Result<ContactFxData, String> {
    let path = game_dir.join("DataPC.forge");
    let forge = Forge::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let rank9 = forge.find("Rank 9").cloned().ok_or("file 'Rank 9' not found")?;
    let res = forge.resources_shared(&rank9).map_err(|e| e.to_string())?;
    let (c_fx, c_entity, c_system, c_texture) = (crc32("FX"), crc32("Entity"), crc32("ParticleSystem"), crc32("TextureMap"));
    let by_class = |c: u32| res.iter().filter(move |r| r.class_hash == c);
    let entity_ids: std::collections::HashSet<u32> = by_class(c_entity).map(|r| r.id).collect();
    let system_ids: std::collections::HashSet<u32> = by_class(c_system).map(|r| r.id).collect();
    let mut data = ContactFxData::default();
    let table = res.iter().find(|r| r.name == "ContactTable_AlTair").ok_or("ContactTable_AlTair not found")?;
    data.groups = contact_fx(&table.payload);
    let wanted: std::collections::HashSet<u32> = data.groups.values().flat_map(|t| t.iter().flatten().copied()).collect();
    for r in by_class(c_fx).filter(|r| wanted.contains(&r.id)) {
        data.fx.insert(r.id, parse_fx(&r.payload, &|id| entity_ids.contains(&id)));
    }
    let spawned: std::collections::HashSet<u32> = data.fx.values().flatten().map(|s| s.entity).collect();
    for r in by_class(c_entity).filter(|r| spawned.contains(&r.id)) {
        if let Some(ps) = entity_particle_system(&r.payload, &|id| system_ids.contains(&id)) {
            data.entities.insert(r.id, ps);
        }
    }
    let systems: std::collections::HashSet<u32> = data.entities.values().copied().collect();
    for r in by_class(c_system).filter(|r| systems.contains(&r.id)) {
        data.systems.insert(r.id, parse_particle_system(&r.payload)?);
    }
    let textures: std::collections::HashSet<u32> = data.systems.values().flat_map(|s| s.emitters.iter().map(|e| e.texture)).collect();
    // the particle textures live in "Game Bootstrap Settings" (some in "Rank 9")
    let shared = forge.find("Game Bootstrap Settings").cloned().ok_or("file 'Game Bootstrap Settings' not found")?;
    let shared = forge.resources_shared(&shared).map_err(|e| e.to_string())?;
    for r in res.iter().chain(shared.iter()).filter(|r| r.class_hash == c_texture && textures.contains(&r.id)) {
        if let Some(t) = super::ac_formats::parse_texture(&r.payload) {
            data.textures.insert(r.id, t);
        }
    }
    data.random = particle_random_table(game_dir);
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The install's contact FX resolve: the default dust, the material groups and their particle systems.
    #[test]
    #[ignore = "reads the game install (DataPC.forge, the exe)"]
    fn contact_fx_from_the_install() {
        let data = load_contact_fx(&crate::assets::game_dir()).unwrap();
        assert_eq!(data.groups.len(), 24, "23 materials + Default");
        let default = &data.groups[&4001];
        assert!(!default[10].is_empty() && default[6].is_empty(), "step_foot spawns, pivot does not");
        assert!(data.fx.len() >= 15, "{}", data.fx.len());
        assert_eq!(data.systems.len(), 5, "Default, Default_Smoke, Straw, Grass, Water_Puddle");
        let dust = data.systems.values().find(|s| s.emitters.len() == 1 && s.emitters[0].max_count == 30 && s.emitters[0].rate == 300.0).expect("the default dust");
        let e = &dust.emitters[0];
        assert_eq!(e.shape, EmitterShape::Circle);
        assert_eq!(e.flow, FlowMode::Once);
        assert_eq!(e.blend, BlendMode::Alpha);
        assert_eq!(e.acceleration, [0.0, 0.0, -2.0]);
        assert!((e.max_life() - 1.83965).abs() < 1e-3);
        assert_eq!(e.color[0] >> 24, 36, "alpha 36 at birth");
        // GFX_Dust_Sand_01 (256² BC3, one mip): a soft puff, transparent at its border, sandy at its centre (its size field
        // is at +0x4F; a header value at +0x2E equals the same size)
        let t = &data.textures[&e.texture];
        let px = |x: usize, y: usize| &t.mips[0][(y * 256 + x) * 4..][..4];
        assert_eq!((t.width, t.height, px(0, 0)[3], px(128, 2)[3]), (256, 256, 0, 0));
        assert!(px(128, 128)[3] > 64 && px(128, 128)[0] > 100 && px(128, 128)[2] < px(128, 128)[0], "{:?}", px(128, 128));
        // the system factors are neutral in every contact FX (so where the renderer applies them does not matter)
        for s in data.systems.values() {
            assert_eq!((s.color_factor, s.alpha_factor, s.rate_factor, s.duration_factor, s.velocity_factor), (0xFFFF_FFFF, 1.0, 1.0, 1.0, 1.0));
        }
        let random = data.random.expect("the random table from the exe");
        assert!((random[0] - 0.977_89).abs() < 1e-4, "{}", random[0]);
    }
}
