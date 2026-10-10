//! Character cloth targets, pin masks and edge constraints (RE/09 §8).

use bevy::prelude::*;

// PORT: final render contacts cover edge-pass penetration and frames without a solver step (RE/09 §8.3).
const CLOTH_RENDER_CONTACT_GUARD: bool = false;
// PORT: discrete surface contacts supplement native vertex contacts (RE/09 §8.4).
const CLOTH_SURFACE_CONTACT_GUARD: bool = false;
const CLOTH_NATIVE_TIMING: bool = true;
// Visible cloth component dispatches once per frame with this literal timestep (0x5B55B0 / 0x577220).
const CLOTH_NATIVE_DISPATCH: bool = true;
const NATIVE_CLOTH_STEP: f32 = 0.033333;

#[derive(Clone)]
pub struct ClothSettings {
    pub source_positions: Vec<Vec3>,
    pub source_weights: Vec<[f32; 4]>,
    pub source_palette: Vec<[u16; 4]>,
    pub pinned: Vec<bool>,
    pub pull: Vec<f32>,
    pub edges: Vec<[usize; 2]>,
    pub triangles: Vec<[usize; 3]>,
    pub damping: f32,
    pub upward_damping: f32,
    pub gravity: f32,
    pub iterations: usize,
    pub vertex_radius: Vec<f32>,
    pub colliders: Vec<ClothCollider>,
    pub pull_motion: Vec2,
    pub pull_decay: f32,
    pub upward_motion: Vec2,
    pub action_settings: Vec<(u32, f32)>,
    pub action_strength: f32,
    /// Embedded force config +456 (ForceConfig__Read 0x5C6B20): enabled, scale; global sources only.
    pub force_enabled: bool,
    pub force_scale: f32,
    /// +152 tangent share and +156 random amplitude of the sampled force (0x4D0EC0).
    pub force_tangent: f32,
    pub force_random: f32,
}

#[derive(Clone)]
pub struct ClothCollider {
    pub bone_id: u32,
    pub local_start: Vec3,
    pub local_end: Vec3,
    pub radius: f32,
    pub mode: u32,
    pub threshold: f32,
}

impl ClothCollider {
    fn closest(&self, position: Vec3) -> Vec3 {
        let axis = self.local_end - self.local_start;
        let length = axis.length_squared();
        let t = if length > 1e-12 { ((position - self.local_start).dot(axis) / length).clamp(0.0, 1.0) } else { 0.0 };
        self.local_start + axis * t
    }

    fn penetration(&self, position: Vec3, vertex_radius: f32) -> f32 {
        if self.mode == 2 { return 0.0; }
        (self.radius + vertex_radius - position.distance(self.closest(position))).max(0.0)
    }
    /// Native capsule response (Cloth__CapsuleContact 0x6C8260; RE/09 §8.2), branch for branch.
    fn correct(&self, position: Vec3, target: Vec3, vertex_radius: f32, pull: f32) -> Vec3 {
        if self.mode == 2 { return position; }
        let radial = self.mode == 1 || self.threshold > pull;
        let radius = self.radius + vertex_radius;
        let rr = radius * radius;
        let axis = self.local_end - self.local_start;
        let from_start = position - self.local_start;
        let t = from_start.dot(axis);
        // Cloth__Integrate 0x6C9C50 stores normalize(target - capsule start) as the mode-0 escape.
        let escape = native_normalize(target - self.local_start);
        if t <= 0.0 {
            let d2 = from_start.length_squared();
            if rr <= d2 { return position; }
            return position + if radial { native_normalize(from_start) } else { escape } * (radius - d2.sqrt());
        }
        let length2 = axis.length_squared();
        if t >= length2 {
            let delta = position - self.local_end;
            let d2 = delta.length_squared();
            if rr <= d2 { return position; }
            return position + if radial { native_normalize(delta) } else { escape } * (radius - d2.sqrt());
        }
        let delta = position - (self.local_start + axis * (t / length2));
        let d2 = delta.length_squared();
        if rr <= d2 || (d2 - rr).abs() <= 0.0005 { return position; } // 0x6C865D
        // 0x6C86C1 compares the raw (unnormalized) axis with the unit escape.
        if radial || (escape.dot(axis).abs() - 1.0).abs() <= 0.0005 {
            return position + native_normalize(delta) * (radius - d2.sqrt());
        }
        let side = native_normalize(axis.cross(escape));
        let out = native_normalize(side.cross(axis));
        let across = delta.dot(side);
        position + out * ((rr - across * across).max(0.0).sqrt() - delta.dot(out))
    }
}

/// 0x48DEA0: zero below FLT_MIN instead of a fallback direction.
fn native_normalize(v: Vec3) -> Vec3 {
    let length = v.length();
    if length >= f32::MIN_POSITIVE { v / length } else { Vec3::ZERO }
}

fn adaptive_length_squared(base: f32, rest: [Vec3; 2], targets: [Vec3; 2], current: [Vec3; 2]) -> f32 {
    // Cloth__sub_6C97B0 0x6C9D52–0x6CA172. PORT: retain base when both deviations vanish.
    let r = rest[0].distance(current[0]) + rest[1].distance(current[1]);
    let t = targets[0].distance(current[0]) + targets[1].distance(current[1]);
    if r + t <= 1e-12 { return base; }
    base + (targets[0].distance_squared(targets[1]) - base) * r / (r + t)
}

fn word(data: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(p..p.checked_add(4)?)?.try_into().ok()?))
}

fn cloth_data(entity: &[u8]) -> Option<usize> {
    let class = 0xCE38BB17; // embedded data class, ClothComponent__sub_5B5FA0 0x5B5FA0
    (4..entity.len().saturating_sub(4)).find(|&p| word(entity, p) == Some(class))
}

pub fn collision_resource(entity: &[u8]) -> Option<u32> {
    word(entity, cloth_data(entity)? + 4)
}

/// Cloth's typed action-settings reference follows seven floats and a flag (0x6C7CA0; RE/09 §8.5).
pub fn action_resource(entity: &[u8]) -> Option<u32> {
    let class = crate::assets::forge::crc32("Cloth");
    let c = (4..entity.len().saturating_sub(4)).find(|&p| word(entity, p) == Some(class))?;
    word(entity, c + 286)
}

pub fn decode_action_settings(data: &[u8]) -> Result<Vec<(u32, f32)>, String> {
    let decode = || -> Option<Vec<(u32, f32)>> {
        if word(data, 4)? != crate::assets::forge::crc32("ClothActionSettings") { return None; }
        let count = word(data, 8)? as usize;
        if count > 1024 || data.len() != 12 + count * 16 { return None; }
        (0..count).map(|i| {
            let p = 12 + i * 16;
            if word(data, p)? != 0 || word(data, p + 4)? != 1319343419 { return None; }
            let strength = f32::from_bits(word(data, p + 12)?);
            if !strength.is_finite() || !(0.0..=1.0).contains(&strength) { return None; }
            Some((word(data, p + 8)?, strength))
        }).collect()
    };
    decode().ok_or_else(|| "unsupported cloth action settings".into())
}

/// Read only direct cloth-frame mappings, not Havok physics (0x5B4F00, 0x10B96F0; RE/09 §8.1).
pub fn decode_colliders(entity: &[u8], ragdoll: &[u8]) -> Result<Vec<ClothCollider>, String> {
    let decode = || -> Option<Vec<ClothCollider>> {
        let c = cloth_data(entity)?;
        let n = word(entity, c + 8)? as usize;
        if n != 21 { return None; }
        let bone_ids: Vec<_> = (0..n).map(|i| word(entity, c + 12 + i * 4)).collect::<Option<_>>()?;
        let count_pos = c + 12 + n * 4;
        let count = word(entity, count_pos)? as usize;
        if count != 19 { return None; }
        // Havok 4.6.1, 32-bit little-endian packfile section headers and pointer fixups.
        let base = ragdoll.windows(8).position(|b| b == [0x57, 0xe0, 0xe0, 0x57, 0x10, 0xc0, 0xc0, 0x10])?;
        if ragdoll.get(base + 16..base + 20)? != [4, 1, 0, 1] || word(ragdoll, base + 20)? != 3 { return None; }
        let mut sections = Vec::new();
        for i in 0..3 {
            let p = base + 64 + i * 48;
            let start = base.checked_add(word(ragdoll, p + 20)? as usize)?;
            let offsets: Vec<_> = (0..6).map(|k| word(ragdoll, p + 24 + k * 4).map(|v| v as usize)).collect::<Option<_>>()?;
            if offsets.windows(2).any(|p| p[0] > p[1]) { return None; }
            ragdoll.get(start..start.checked_add(offsets[5])?)?;
            sections.push((start, offsets));
        }
        let mut pointers = std::collections::HashMap::new();
        for (start, offsets) in &sections {
            for p in (*start + offsets[0]..*start + offsets[1]).step_by(8) {
                let from = word(ragdoll, p)?;
                if from == u32::MAX { continue; }
                pointers.insert(*start + from as usize, *start + word(ragdoll, p + 4)? as usize);
            }
            for p in (*start + offsets[1]..*start + offsets[2]).step_by(12) {
                let from = word(ragdoll, p)?;
                if from == u32::MAX { continue; }
                let to_section = sections.get(word(ragdoll, p + 4)? as usize)?;
                pointers.insert(*start + from as usize, to_section.0 + word(ragdoll, p + 8)? as usize);
            }
        }
        let (start, offsets) = &sections[1];
        let mut mapped = std::collections::HashMap::new();
        for p in (*start + offsets[2]..*start + offsets[3]).step_by(12) {
            let from = word(ragdoll, p)?;
            if from == u32::MAX { continue; }
            let name_section = sections.get(word(ragdoll, p + 4)? as usize)?;
            let name = name_section.0 + word(ragdoll, p + 8)? as usize;
            if !ragdoll.get(name..)?.starts_with(b"hkSkeletonMapper\0") { continue; }
            let object = *start + from as usize;
            let source = *pointers.get(&(object + 8))?;
            let destination = *pointers.get(&(object + 12))?;
            if word(ragdoll, source + 16)? as usize != n || word(ragdoll, destination + 16)? as usize != count { continue; }
            let records = *pointers.get(&(object + 16))?;
            let records_count = word(ragdoll, object + 20)? as usize;
            if records_count > count { return None; }
            let float = |p| word(ragdoll, p).map(f32::from_bits);
            for i in 0..records_count {
                let r = records + i * 64;
                let pair = word(ragdoll, r)?;
                let a = (pair & 65535) as usize;
                let b = (pair >> 16) as usize;
                if a >= n || b >= count { return None; }
                let t = Vec3::new(float(r + 16)?, float(r + 20)?, float(r + 24)?);
                let q = Quat::from_xyzw(float(r + 32)?, float(r + 36)?, float(r + 40)?, float(r + 44)?);
                let scale = Vec3::new(float(r + 48)?, float(r + 52)?, float(r + 56)?);
                if !t.is_finite() || !q.is_finite() || (q.length_squared() - 1.0).abs() > 0.01 || !scale.is_finite() { return None; }
                if mapped.insert(b, (bone_ids[a], Mat4::from_scale_rotation_translation(scale, q.normalize(), t), scale.abs().max_element())).is_some() { return None; }
            }
        }
        let mut colliders = Vec::new();
        for i in 0..count {
            let p = count_pos + 4 + i * 52 + 8;
            let mode = word(entity, p)?;
            if mode == 2 { continue; }
            if mode > 2 { return None; }
            let (bone_id, frame, scale) = mapped.get(&i)?;
            let float = |p| word(entity, p).map(f32::from_bits);
            let a = Vec3::new(float(p + 8)?, float(p + 12)?, float(p + 16)?);
            let b = Vec3::new(float(p + 24)?, float(p + 28)?, float(p + 32)?);
            let radius = float(p + 40)? * scale;
            let threshold = float(p + 4)?;
            if !a.is_finite() || !b.is_finite() || !radius.is_finite() || radius <= 0.0 || !threshold.is_finite() { return None; }
            colliders.push(ClothCollider { bone_id: *bone_id, local_start: frame.transform_point3(a), local_end: frame.transform_point3(b), radius, mode, threshold });
        }
        Some(colliders)
    };
    decode().ok_or_else(|| "unsupported character cloth collision mapping".into())
}

/// Read the soft mesh's inline SubMesh arrays (SubMesh__sub_9FC6C0 0x9FC6C0).
pub fn decode_settings(mesh: &[u8], entity: &[u8], compiled: &[[f32; 3]]) -> Result<ClothSettings, String> {
    let decode = || -> Option<ClothSettings> {
        let word = |d: &[u8], p: usize| d.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        let float = |d: &[u8], p: usize| word(d, p).map(f32::from_bits);
        let mut p = 29; // Mesh kind 4, one inline SubMesh, then its format field.
        let mut array = |width: usize| -> Option<&[u8]> {
            let n = word(mesh, p)? as usize;
            p += 4;
            let end = p.checked_add(n.checked_mul(width)?)?;
            let data = mesh.get(p..end)?;
            p = end;
            Some(data)
        };
        if word(mesh, 8)? != 4 || word(mesh, 12)? != 1 { return None; }
        let indices = array(2)?;
        array(4)?; // vertex remapping
        // This outfit has no embedded colour records; their serialization is not a raw array.
        if !array(7)?.is_empty() { return None; }
        array(4)?; // triangle metadata
        let masks = array(1)?;
        let positions = array(12)?;
        array(12)?; // normals
        array(12)?; // tangents
        array(12)?; // binormals
        array(4)?; // UVs
        let flags = array(4)?;
        array(4)?; // SubMesh +132
        let weights = array(16)?; // SubMesh +144, 0x9FCE05
        let bones = array(1)?; // SubMesh +156, 0x9FCE8F
        let n = positions.len() / 12;
        if n != compiled.len() || flags.len() != 4 * n || weights.len() != 16 * n || bones.len() != 4 * n || indices.len() != masks.len() * 6 { return None; }
        let source: Vec<Vec3> = (0..n).map(|i| Some(Vec3::new(float(positions, i * 12)?, float(positions, i * 12 + 4)?, float(positions, i * 12 + 8)?))).collect::<Option<_>>()?;
        let mut to_render = vec![usize::MAX; n];
        // PORT: the compiler reorders vertices. Require a bijection within s16 quantization error.
        for (render, position) in compiled.iter().enumerate() {
            let position = Vec3::from_array(*position);
            let (source_index, distance) = source.iter().enumerate().map(|(i, p)| (i, p.distance_squared(position)))
                .min_by(|a, b| a.1.total_cmp(&b.1))?;
            if distance > 3.0 / (2048.0 * 2048.0) || to_render[source_index] != usize::MAX { return None; }
            to_render[source_index] = render;
        }
        // Cloth__SkinTarget 0x6C8060 uses source floats and mesh-bone indices directly.
        let mut source_positions = vec![Vec3::ZERO; n];
        let mut source_weights = vec![[0.0; 4]; n];
        let mut source_palette = vec![[0; 4]; n];
        for i in 0..n {
            let render = to_render[i];
            source_positions[render] = source[i];
            for k in 0..4 {
                let weight = float(weights, 16 * i + 4 * k)?;
                let bone = bones[4 * i + k];
                if !weight.is_finite() || !(0.0..=1.0).contains(&weight) || (bone == 255 && weight != 0.0) { return None; }
                source_weights[render][k] = weight;
                source_palette[render][k] = if bone == 255 { 0 } else { bone as u16 };
            }
            if (source_weights[render].iter().sum::<f32>() - 1.0).abs() > 0.0001 { return None; }
        }
        let class = crate::assets::forge::crc32("Cloth");
        let c = (4..entity.len().saturating_sub(4)).find(|&i| word(entity, i) == Some(class))?;
        // Rank 9's fixed-size base fields; layout traced through 0x4CFCF0 / 0x6C7CA0.
        let damping = float(entity, c + 28)?;
        let upward_damping = float(entity, c + 24)?;
        // BoundingBox ends at +257 (0x4CF120); Cloth's seven floats follow (0x6C7CA0).
        let pull_min = float(entity, c + 269)?;
        let pull_max = float(entity, c + 273)?;
        let mut pinned = vec![false; n];
        let mut pull = vec![0.0; n];
        let mut vertex_radius = vec![0.0; n];
        for i in 0..n {
            let blend = flags[4 * i + 2];
            pinned[to_render[i]] = blend == 255; // 0x4D5AEF
            pull[to_render[i]] = pull_min + (pull_max - pull_min) * blend as f32 / 255.0; // 0x6CA38A
            vertex_radius[to_render[i]] = float(entity, c + 36)?
                + (float(entity, c + 40)? - float(entity, c + 36)?) * flags[4 * i + 1] as f32 / 255.0; // 0x4D5370
        }
        let component = crate::assets::forge::crc32("ClothComponent");
        let component = (4..entity.len().saturating_sub(4)).find(|&i| word(entity, i) == Some(component))?;
        let gravity = float(entity, component + 13)?;
        // Iteration count is the serialized enum at base +168 (0x4CFF7C, 0x4D397D).
        let iterations = *entity.get(c + 79)? as usize;
        // Embedded force config (class -403070396) at +44, then +152 / +156 (SoftBody__Read 0x4CFCF0).
        if word(entity, c + 48)? != (-403070396i32) as u32 || entity.get(c + 57..c + 59)? != [0, 0] { return None; }
        let force_enabled = match *entity.get(c + 52)? { 0 => false, 1 => true, _ => return None };
        let (force_scale, force_tangent, force_random) = (float(entity, c + 53)?, float(entity, c + 59)?, float(entity, c + 63)?);
        if ![force_scale, force_tangent, force_random].iter().all(|v| v.is_finite()) { return None; }
        if !(1..=7).contains(&iterations) || ![damping, upward_damping, pull_min, pull_max, gravity].iter().all(|v| v.is_finite())
            || !(0.0..=1.0).contains(&damping) || !(0.0..=1.0).contains(&upward_damping)
            || !(0.0..=1.0).contains(&pull_min) || !(0.0..=1.0).contains(&pull_max) { return None; }
        let mut edges = Vec::new();
        let mut triangles = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (tri, &mask) in indices.chunks_exact(6).zip(masks) {
            let v: Vec<usize> = tri.chunks_exact(2).map(|b| u16::from_le_bytes(b.try_into().unwrap()) as usize).collect();
            if v.iter().any(|&i| i >= n) { return None; }
            triangles.push([to_render[v[0]], to_render[v[1]], to_render[v[2]]]);
            for k in 0..3 {
                if mask & (1 << k) == 0 { continue; }
                let (a, b) = (to_render[v[k]], to_render[v[(k + 1) % 3]]);
                let key = (a.min(b), a.max(b));
                if seen.insert(key) { edges.push([a, b]); }
            }
        }
        Some(ClothSettings { source_positions, source_weights, source_palette, pinned, pull, edges, triangles, damping, upward_damping, gravity, iterations, vertex_radius, colliders: Vec::new(),
            pull_motion: Vec2::new(float(entity, c + 257)?, float(entity, c + 261)?), pull_decay: float(entity, c + 265)?,
            upward_motion: Vec2::new(float(entity, c + 277)?, float(entity, c + 281)?), action_settings: Vec::new(), action_strength: 1.0,
            force_enabled, force_scale, force_tangent, force_random })
    };
    decode().ok_or_else(|| "unsupported or inconsistent character cloth layout".into())
}

#[derive(Component)]
pub struct CharacterCloth {
    pub settings: ClothSettings,
    pub rest: Vec<Vec3>,
    pub weights: Vec<[f32; 4]>,
    pub palette: Vec<[u16; 4]>,
    pub inverse_bindposes: Vec<Mat4>,
    pub joints: Vec<Entity>,
    pub collider_joints: Vec<Entity>,
    pub mesh: Handle<Mesh>,
    pub player: Entity,
    pub state: ClothState,
}

#[derive(Default)]
pub struct ClothState {
    native_dispatch: Option<bool>,
    current: Vec<Vec3>,
    previous: Vec<Vec3>,
    lengths_squared: Vec<f32>,
    accumulator: f32,
    anchor: Option<Vec3>,
    motion_anchor: Option<Vec3>,
    contact_before: (usize, f32),
    contact_after: (usize, f32),
    surface_guard: Option<bool>,
    render_guard: Option<bool>,
    motion: Vec3,
    orientation: Option<Quat>,
    action_pull: f32,
    /// Area-weighted vertex normals of the last export (SoftBody__ComputeNormals 0x4D49A0).
    normals: Vec<Vec3>,
    /// PORT: the game shares one LCG (dword_1A1FC3C) across systems; the robe keeps its own.
    random: u32,
}

/// SoftBody__ComputeNormals 0x4D49A0: sum of (b - a) × (c - b) per vertex, normalized.
fn vertex_normals(triangles: &[[usize; 3]], positions: &[Vec3]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for &[a, b, c] in triangles {
        let n = (positions[b] - positions[a]).cross(positions[c] - positions[b]);
        normals[a] += n; normals[b] += n; normals[c] += n;
    }
    for n in &mut normals { if n.length_squared() > 0.0 { *n = n.normalize(); } }
    normals
}

impl ClothState {
    /// SoftBody__ComputeAcceleration 0x4D0EC0: the sampled force scaled by 1 + amplitude·r, its normal part plus a
    /// tangent share, then gravity. Without an enabled config only gravity applies.
    fn acceleration(&mut self, settings: &ClothSettings, i: usize, force: Vec3) -> Vec3 {
        let gravity = Vec3::Y * settings.gravity;
        if !settings.force_enabled { return gravity; }
        self.random = self.random.wrapping_mul(1664525).wrapping_add(1013904223);
        let r = (self.random & 0x0FFF_FFFF) as f32 * 3.725290298461914e-9 * 2.0 - 1.0;
        let f = force * (r * settings.force_random) + force;
        let n = self.normals.get(i).copied().unwrap_or(Vec3::ZERO);
        let normal = n * f.dot(n);
        settings.force_tangent * (f - normal) + normal + gravity
    }
}

fn contact_report(settings: &ClothSettings, positions: &[Vec3], capsules: &[ClothCollider]) -> (usize, f32) {
    let mut count = 0;
    let mut worst = 0.0f32;
    for (i, &position) in positions.iter().enumerate() {
        if settings.pinned[i] { continue; }
        let depth = capsules.iter().map(|c| c.penetration(position, settings.vertex_radius[i])).fold(0.0f32, f32::max);
        if depth > 0.001 { count += 1; }
        worst = worst.max(depth);
    }
    (count, worst)
}

fn escape_overlapping_capsules(position: Vec3, target: Vec3, rest: Vec3, radius: f32, capsules: &[ClothCollider]) -> Vec3 {
    let clear = |p| capsules.iter().all(|c| c.penetration(p, radius) <= 1e-5);
    if clear(position) { return position; }
    // PORT: alternating projections can cycle between overlapping leg capsules. Find the
    // shortest bounded escape along pose directions or world axes; never change the pins.
    let bound = 2.0 * capsules.iter().filter(|c| c.mode != 2).map(|c| c.radius + radius).fold(0.0f32, f32::max);
    let mut best = position;
    let mut best_distance = f32::INFINITY;
    for direction in [target - position, rest - position, Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
        let Some(direction) = direction.try_normalize() else { continue; };
        for step in 1..=32 {
            let mut high = bound * step as f32 / 32.0;
            if high >= best_distance { break; }
            if !clear(position + direction * high) { continue; }
            let mut low = bound * (step - 1) as f32 / 32.0;
            for _ in 0..10 {
                let middle = (low + high) * 0.5;
                if clear(position + direction * middle) { high = middle; } else { low = middle; }
            }
            best = position + direction * high;
            best_distance = high;
            break;
        }
    }
    best
}

fn surface_contacts(settings: &ClothSettings, positions: &mut [Vec3], targets: &[Vec3], capsules: &[ClothCollider], apply: bool) -> usize {
    let mut count = 0;
    for &triangle in &settings.triangles {
        if triangle.iter().all(|&i| settings.pinned[i]) { continue; }
        for capsule in capsules {
            if capsule.mode == 2 { continue; }
            let vertices = triangle.map(|i| positions[i]);
            let c = crate::cloth_contacts::segment_triangle(capsule.local_start, capsule.local_end, vertices);
            let radius = capsule.radius + (0..3).map(|k| settings.vertex_radius[triangle[k]] * c.weights[k]).sum::<f32>();
            let distance = c.point.distance(c.axis);
            if distance >= radius - 0.001 { continue; }
            count += 1;
            if !apply { continue; }
            let target: Vec3 = (0..3).map(|k| targets[triangle[k]] * c.weights[k]).sum();
            let normal = (vertices[1] - vertices[0]).cross(vertices[2] - vertices[0]).normalize_or_zero();
            let outward = target - capsule.closest(target);
            let normal = if normal.dot(outward) < 0.0 { -normal } else { normal };
            let direction = (c.point - c.axis).try_normalize().unwrap_or_else(|| outward.try_normalize().unwrap_or(normal));
            crate::cloth_contacts::distribute_bounded(positions, triangle, &settings.pinned, c.weights, direction * (radius - distance), 0.02);
        }
    }
    count
}

fn fold_contacts(settings: &ClothSettings, positions: &mut [Vec3], previous: &[Vec3], apply: bool) -> usize {
    let mut count = 0;
    for (i, &a) in settings.triangles.iter().enumerate() {
        for &b in &settings.triangles[i + 1..] {
            if a.iter().any(|v| b.contains(v)) { continue; }
            if a.iter().chain(&b).all(|&v| settings.pinned[v]) { continue; }
            let av = a.map(|v| positions[v]);
            let bv = b.map(|v| positions[v]);
            let amin = av[0].min(av[1]).min(av[2]) - Vec3::splat(0.002);
            let amax = av[0].max(av[1]).max(av[2]) + Vec3::splat(0.002);
            let bmin = bv[0].min(bv[1]).min(bv[2]);
            let bmax = bv[0].max(bv[1]).max(bv[2]);
            if amin.cmpgt(bmax).any() || bmin.cmpgt(amax).any() { continue; }
            let crossing = [(a, b, av, bv), (b, a, bv, av)].into_iter().find_map(|(edge_ids, face_ids, edge_vertices, face_vertices)| {
                (0..3).find_map(|k| {
                    let c = crate::cloth_contacts::segment_triangle(edge_vertices[k], edge_vertices[(k + 1) % 3], face_vertices);
                    if c.point.distance_squared(c.axis) > 1e-10 { return None; }
                    let edge = edge_vertices[(k + 1) % 3] - edge_vertices[k];
                    if edge.length_squared() <= 1e-12 { return None; }
                    let t = ((c.axis - edge_vertices[k]).dot(edge) / edge.length_squared()).clamp(0.0, 1.0);
                    let mut weights = Vec3::ZERO; weights[k] = 1.0 - t; weights[(k + 1) % 3] = t;
                    Some((c, weights, edge_ids, face_ids, face_vertices))
                })
            });
            let Some((c, weights, edge_ids, face_ids, face_vertices)) = crossing else { continue; };
            count += 1;
            if !apply { continue; }
            let Some(mut normal) = (face_vertices[1] - face_vertices[0]).cross(face_vertices[2] - face_vertices[0]).try_normalize() else { continue; };
            let old_a: Vec3 = (0..3).map(|k| previous[edge_ids[k]] * weights[k]).sum();
            let old_b: Vec3 = (0..3).map(|k| previous[face_ids[k]] * c.weights[k]).sum();
            if normal.dot(old_a - old_b) < 0.0 { normal = -normal; }
            // PORT: move the penetrating endpoint, not only the intersection point.
            // A small offset at the intersection can leave the same edge crossing the face.
            let endpoint = (0..3).filter(|&k| weights[k] > 0.0).min_by(|&j, &k|
                normal.dot(positions[edge_ids[j]] - c.point).total_cmp(&normal.dot(positions[edge_ids[k]] - c.point))).unwrap();
            let depth = (0.002 - normal.dot(positions[edge_ids[endpoint]] - c.point)).max(0.002);
            let mut endpoint_weights = Vec3::ZERO; endpoint_weights[endpoint] = 1.0;
            // PORT: stronger pinned-side separation is validated only with the complete skirt pose.
            if crate::visual_pose::skirt_modifiers_enabled() {
                crate::cloth_contacts::separate_bounded(positions, edge_ids, face_ids, &settings.pinned, endpoint_weights, c.weights, normal * depth, 0.01);
            } else {
                crate::cloth_contacts::distribute_bounded(positions, edge_ids, &settings.pinned, endpoint_weights, normal * depth * 0.5, 0.01);
                crate::cloth_contacts::distribute_bounded(positions, face_ids, &settings.pinned, c.weights, -normal * depth * 0.5, 0.01);
            }
        }
    }
    count
}

impl ClothState {
    fn native_dispatch(&mut self) -> bool {
        *self.native_dispatch.get_or_insert_with(|| std::env::var("AC_CLOTH_NATIVE_DISPATCH").map_or(CLOTH_NATIVE_DISPATCH, |v| v != "0"))
    }
    fn sample_entity_motion(&mut self, anchor: Vec3, rotation: Quat, dt: f32) {
        let dispatch = self.native_dispatch();
        let reset = self.motion_anchor.is_none_or(|old| old.distance_squared(anchor) > 4.0);
        let steps = ((self.accumulator + dt.clamp(0.0, 0.1)) * 30.0).floor();
        if !reset && ((!dispatch && steps < 1.0) || dt <= 0.0) { return; }
        if reset {
            self.motion = Vec3::ZERO;
        } else if dt > 0.0 {
            // 0x5B4F00 samples entity motion once per component update using 0.033333 s.
            // PORT: the legacy comparison path aggregates motion across accumulated 30 Hz ticks.
            let interval = if dispatch { NATIVE_CLOTH_STEP } else { steps / 30.0 };
            let velocity = (anchor - self.motion_anchor.unwrap()) / interval;
            let turn = self.orientation.map_or(0.0, |old| turn_motion(old, rotation, interval));
            self.motion = Vec3::new(velocity.length(), turn, velocity.y);
        }
        self.motion_anchor = Some(anchor);
        self.orientation = Some(rotation);
    }

    #[cfg(test)]
    pub fn advance(&mut self, settings: &ClothSettings, targets: &[Vec3], rigid_rest: &[Vec3], capsules: &[ClothCollider], anchor: Vec3, dt: f32) -> &[Vec3] {
        self.advance_in_wind(settings, targets, rigid_rest, capsules, anchor, Vec3::ZERO, dt)
    }

    /// `force` is the environment sample at the owner entity (ForceField__SampleForce in SoftBody__Simulate 0x4D21F0).
    pub fn advance_in_wind(&mut self, settings: &ClothSettings, targets: &[Vec3], rigid_rest: &[Vec3], capsules: &[ClothCollider], anchor: Vec3, force: Vec3, dt: f32) -> &[Vec3] {
        // Native component passes literal 0.033333 s once per dispatch (0x577220; RE/09 §8.11).
        // PORT: legacy comparison uses accumulated render time and bounded catch-up.
        let dispatch = self.native_dispatch();
        let step = if dispatch { NATIVE_CLOTH_STEP } else if CLOTH_NATIVE_TIMING { 1.0 / 30.0 } else { 1.0 / 60.0 };
        let reset = self.current.len() != targets.len() || self.anchor.is_none_or(|p| p.distance_squared(anchor) > 4.0);
        self.anchor = Some(anchor);
        if reset {
            self.current = targets.to_vec();
            self.previous = targets.to_vec();
            self.lengths_squared = settings.edges.iter().map(|[a, b]| rigid_rest[*a].distance_squared(rigid_rest[*b])).collect();
            self.accumulator = 0.0;
            self.action_pull = 0.0;
            // Cloth__ResetToSkin 0x6C93E0 exports the skinned pose, which gives the first normals.
            self.normals = vertex_normals(&settings.triangles, &self.current);
        }
        // 0x5B55B0 frame guard → 0x577220: one dispatch, independent of accumulated render dt.
        // PORT: visible Bevy update replaces the native task/LOD dispatch; teleport reset above remains.
        if dispatch { self.accumulator = if dt > 0.0 { step } else { 0.0 }; }
        else { self.accumulator += dt.clamp(0.0, 0.1); }
        while self.accumulator >= step {
            self.accumulator -= step;
            if CLOTH_NATIVE_TIMING {
                // 0x6C9800–0x6C98BC: speed/turn pull, decaying previous pull and action strength.
                let desired = settings.pull_motion.dot(self.motion.truncate());
                let decayed = self.action_pull - settings.pull_decay * step;
                let pull = desired.max(decayed).clamp(0.0, 1.0);
                self.action_pull = 1.0 - (1.0 - pull) * settings.action_strength;
            }
            let upward = if CLOTH_NATIVE_TIMING {
                (settings.upward_damping + self.motion.z * settings.upward_motion.x).max(0.0).min(settings.upward_motion.y)
            } else { settings.upward_damping };
            for (i, &target) in targets.iter().enumerate() {
                let old = self.current[i];
                if settings.pinned[i] { self.current[i] = target; }
                else {
                    let velocity = old - self.previous[i];
                    let vertical_damping = if velocity.y >= 0.0 { upward } else { settings.damping };
                    let predicted = old + velocity * Vec3::new(1.0 - settings.damping, 1.0 - vertical_damping, 1.0 - settings.damping)
                        + self.acceleration(settings, i, force) * step * step;
                    self.current[i] = predicted.lerp(target, settings.pull[i].max(self.action_pull));
                }
                self.previous[i] = old;
            }
            let lengths: Vec<_> = settings.edges.iter().zip(&self.lengths_squared).map(|([a, b], &base)|
                adaptive_length_squared(base, [rigid_rest[*a], rigid_rest[*b]], [targets[*a], targets[*b]], [self.current[*a], self.current[*b]])).collect();
            // Native default: contacts before each edge pass (0x4D398D; RE/09 §8.2).
            for _ in 0..settings.iterations {
                for (i, position) in self.current.iter_mut().enumerate() {
                    if settings.pinned[i] { continue; }
                    for capsule in capsules {
                        *position = capsule.correct(*position, targets[i], settings.vertex_radius[i], settings.pull[i].max(self.action_pull));
                    }
                }
                for (edge, &rest) in settings.edges.iter().zip(&lengths) {
                    let [a, b] = *edge;
                    let delta = self.current[b] - self.current[a];
                    let denominator = delta.length_squared() + rest;
                    if denominator <= 1e-12 { continue; }
                    let correction = delta * (rest / denominator - 0.5);
                    match (settings.pinned[a], settings.pinned[b]) {
                        (false, false) => { self.current[a] -= correction; self.current[b] += correction; }
                        (true, false) => self.current[b] += 2.0 * correction,
                        (false, true) => self.current[a] -= 2.0 * correction,
                        _ => {}
                    }
                }
            }
            // SoftBody__RunSteps 0x4CE660 exports after each step (+76); the next step reads these normals.
            self.normals = vertex_normals(&settings.triangles, &self.current);
        }
        // Pins follow the skeleton even on render frames with no simulation step.
        for (i, &target) in targets.iter().enumerate() { if settings.pinned[i] { self.current[i] = target; } }
        self.contact_before = contact_report(settings, &self.current, capsules);
        if self.render_guard.unwrap_or(CLOTH_RENDER_CONTACT_GUARD) && (dt > 0.0 || reset) {
            for (i, position) in self.current.iter_mut().enumerate() {
                if settings.pinned[i] { continue; }
                let before = *position;
                for _ in 0..8 {
                    let mut changed = false;
                    for capsule in capsules {
                        if capsule.penetration(*position, settings.vertex_radius[i]) <= 1e-5 { continue; }
                        let closest = capsule.closest(*position);
                        let direction = (*position - closest).try_normalize().or_else(|| (targets[i] - closest).try_normalize()).unwrap_or(Vec3::X);
                        *position = closest + direction * (capsule.radius + settings.vertex_radius[i]);
                        changed = true;
                    }
                    if !changed { break; }
                }
                *position = escape_overlapping_capsules(*position, targets[i], rigid_rest[i], settings.vertex_radius[i], capsules);
                // PORT: presentation correction must not inject an extra Verlet velocity.
                self.previous[i] += *position - before;
            }
        }
        let surface_guard = *self.surface_guard.get_or_insert_with(|| CLOTH_SURFACE_CONTACT_GUARD && std::env::var_os("AC_CLOTH_NO_SURFACE_GUARD").is_none());
        if surface_guard && (dt > 0.0 || reset) {
            let before = self.current.clone();
            let initial_folds = fold_contacts(settings, &mut self.current, &self.previous, false);
            let mut best_body = surface_contacts(settings, &mut self.current, targets, capsules, false);
            let mut best_folds = initial_folds;
            let mut best = before.clone();
            for _ in 0..8 {
                surface_contacts(settings, &mut self.current, targets, capsules, true);
                fold_contacts(settings, &mut self.current, &self.previous, true);
                for (i, p) in self.current.iter_mut().enumerate() {
                    if !settings.pinned[i] { *p = escape_overlapping_capsules(*p, targets[i], rigid_rest[i], settings.vertex_radius[i], capsules); }
                }
                let body = surface_contacts(settings, &mut self.current, targets, capsules, false);
                let folds = fold_contacts(settings, &mut self.current, &self.previous, false);
                // PORT: retain only a pass that improves contacts without adding fold crossings.
                // Pinned cloth and overlapping leg volumes can make all constraints incompatible.
                if folds <= initial_folds && (body < best_body || (body == best_body && folds < best_folds)) {
                    best.clone_from(&self.current); best_body = body; best_folds = folds;
                }
                if body == 0 && folds == 0 { break; }
            }
            self.current = best;
            for (i, p) in self.current.iter().enumerate() { self.previous[i] += *p - before[i]; }
        }
        self.contact_after = contact_report(settings, &self.current, capsules);
        &self.current
    }
}

fn turn_motion(old: Quat, current: Quat, dt: f32) -> f32 {
    // ClothComponent__UpdateData 0x5B514A–0x5B51A0 rotates unit X, not a diagonal.
    // Model conversion maps native X to -X; the displacement magnitude is unchanged.
    ((old.inverse() * current) * Vec3::X - Vec3::X).length() / dt
}

pub fn update_cloth(time: Res<Time>, wind: Res<crate::wind::WindField>, transforms: Query<&GlobalTransform>, mut cloth: Query<&mut CharacterCloth>, mut meshes: ResMut<Assets<Mesh>>, animations: Query<&crate::anim::AnimPlayer>, mut diagnostics: Local<Option<bool>>, mut report_time: Local<f32>) {
    let diagnostics = *diagnostics.get_or_insert_with(|| std::env::var_os("AC_CLOTH_DIAGNOSTICS").is_some());
    *report_time += time.delta_secs();
    let report = diagnostics && *report_time >= 0.5;
    if report { *report_time = 0.0; }
    for mut cloth in &mut cloth {
        let Ok(player) = transforms.get(cloth.player) else { continue; };
        let matrices: Option<Vec<_>> = cloth.joints.iter().zip(&cloth.inverse_bindposes)
            .map(|(&joint, inverse)| transforms.get(joint).ok().map(|t| t.to_matrix() * *inverse)).collect();
        let Some(matrices) = matrices else { continue; };
        let targets: Vec<Vec3> = cloth.rest.iter().enumerate().map(|(i, &p)| (0..4)
            .map(|k| matrices[cloth.palette[i][k] as usize].transform_point3(p) * cloth.weights[i][k]).sum()).collect();
        let mut settings = cloth.settings.clone();
        if CLOTH_NATIVE_TIMING {
            cloth.state.sample_entity_motion(player.translation(), player.rotation(), time.delta_secs());
            // 0x5B55B0: the action from 0x723D10; none (a transition between actions) leaves the strength at 1
            let action = animations.get(cloth.player).ok().and_then(|a| a.selected_action());
            settings.action_strength = action.and_then(|id| settings.action_settings.iter().find(|(key, _)| *key == id)).map_or(1.0, |(_, strength)| *strength);
        }
        let capsules: Option<Vec<_>> = settings.colliders.iter().zip(&cloth.collider_joints).map(|(c, &joint)| {
            let transform = transforms.get(joint).ok()?.to_matrix();
            Some(ClothCollider { bone_id: c.bone_id, local_start: transform.transform_point3(c.local_start),
                local_end: transform.transform_point3(c.local_end), radius: c.radius * transform.x_axis.truncate().length(), mode: c.mode, threshold: c.threshold })
        }).collect();
        let Some(capsules) = capsules else { continue; };
        let inverse = player.to_matrix().inverse();
        let rigid_rest: Vec<_> = cloth.rest.iter().map(|&p| player.to_matrix().transform_point3(p)).collect();
        let force = if settings.force_enabled { wind.sample(player.translation(), settings.force_scale) } else { Vec3::ZERO };
        let positions: Vec<[f32; 3]> = cloth.state.advance_in_wind(&settings, &targets, &rigid_rest, &capsules, player.translation(), force, time.delta_secs()).iter()
            .map(|&p| inverse.transform_point3(p).to_array()).collect();
        if report {
            let lag = cloth.state.current.iter().zip(&targets).map(|(p, t)| p.distance(*t)).fold(0.0f32, f32::max);
            let stretch = settings.edges.iter().map(|[a, b]| cloth.state.current[*a].distance(cloth.state.current[*b])
                / targets[*a].distance(targets[*b]).max(1e-6)).fold(0.0f32, f32::max);
            let free: Vec<f32> = cloth.state.current.iter().zip(&targets).enumerate().filter(|(i, _)| !settings.pinned[*i]).map(|(_, (p, t))| p.distance(*t)).collect();
            let mean = free.iter().sum::<f32>() / free.len().max(1) as f32;
            eprintln!("cloth shape: max target lag {lag:.4} m; mean free lag {mean:.4} m; max animated-edge stretch {stretch:.2}x; wind {:.2?}", wind.sample(player.translation(), 1.0));
            eprintln!("cloth t={:.2} contacts>1mm: {} -> {}; max depth: {:.4} -> {:.4} m", time.elapsed_secs(),
                cloth.state.contact_before.0, cloth.state.contact_after.0, cloth.state.contact_before.1, cloth.state.contact_after.1);
            let mut positions = cloth.state.current.clone();
            let body = surface_contacts(&settings, &mut positions, &targets, &capsules, false);
            let folds = fold_contacts(&settings, &mut positions, &cloth.state.previous, false);
            eprintln!("cloth surfaces: {body} triangle/capsule contacts; {folds} non-adjacent face crossings");
            // Collision margins are not visible body surfaces. Report the authored volumes separately.
            let mut body_settings = settings.clone();
            body_settings.vertex_radius.fill(0.0);
            let body = surface_contacts(&body_settings, &mut positions, &targets, &capsules, false);
            let (lo, hi) = cloth.rest.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.y), hi.max(p.y)));
            body_settings.triangles.retain(|tri| tri.iter().any(|&i| cloth.rest[i].y < (lo + hi) * 0.5));
            let hem = surface_contacts(&body_settings, &mut positions, &targets, &capsules, false);
            eprintln!("cloth body volumes without margin: {body} surface contacts; {hem} lower-robe contacts");
        }
        let Some(mut mesh) = meshes.get_mut(&cloth.mesh) else { continue; };
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        // PORT: rebuild the shading frame from deformed geometry instead of the game's normal solver.
        mesh.compute_smooth_normals();
        let _ = mesh.generate_tangents();
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_turn_input_measures_a_unit_direction() {
        let old = Quat::IDENTITY;
        assert!((turn_motion(old, Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), 1.0) - 2.0f32.sqrt()).abs() < 1e-6);
        assert!(turn_motion(old, Quat::from_rotation_x(1.0), 1.0) < 1e-6);
    }
    #[test]
    fn component_motion_accumulates_translation_and_turn_between_ticks() {
        let cfg = settings();
        let rest = vec![Vec3::ZERO, Vec3::X, Vec3::Y];
        let mut state = ClothState { native_dispatch: Some(false), ..default() };
        state.sample_entity_motion(Vec3::ZERO, Quat::IDENTITY, 0.0);
        for frame in 1..=120 {
            let t = frame as f32 / 60.0;
            let anchor = Vec3::X * (6.0 * t);
            state.sample_entity_motion(anchor, Quat::from_rotation_y(t), 1.0 / 60.0);
            if frame % 2 == 0 {
                assert!((state.motion.x - 6.0).abs() < 1e-4);
                assert!((state.motion.y - 2.0 * (1.0f32 / 60.0).sin() * 30.0).abs() < 1e-4);
            }
            let targets: Vec<_> = rest.iter().map(|p| *p + anchor).collect();
            state.advance(&cfg, &targets, &targets, &[], anchor, 1.0 / 60.0);
        }
    }
    #[test]
    fn native_path_leaves_contacts_and_history_unchanged_between_ticks() {
        let mut cfg = settings();
        cfg.edges.clear();
        let rest = vec![Vec3::ZERO, Vec3::X * 0.02, Vec3::X * 0.03];
        let capsule = ClothCollider { bone_id: 0, local_start: -Vec3::Y, local_end: Vec3::Y, radius: 0.1, mode: 1, threshold: 0.0 };
        let mut state = ClothState { native_dispatch: Some(false), ..default() };
        state.advance(&cfg, &rest, &rest, std::slice::from_ref(&capsule), Vec3::ZERO, 0.0);
        let previous = state.previous.clone();
        assert_eq!(state.advance(&cfg, &rest, &rest, std::slice::from_ref(&capsule), Vec3::ZERO, 1.0 / 120.0), rest);
        assert_eq!(state.previous, previous);
        assert!(state.contact_after.0 > 0, "native contacts wait for the solver tick");
    }
    fn settings() -> ClothSettings {
        ClothSettings { source_positions: Vec::new(), source_weights: Vec::new(), source_palette: Vec::new(), pinned: vec![true, false, false], pull: vec![1.0, 0.015, 0.015], edges: vec![[0, 1], [1, 2]], triangles: Vec::new(), damping: 0.1, upward_damping: 0.75, gravity: -9.8, iterations: 3, vertex_radius: vec![0.01; 3], colliders: Vec::new(), pull_motion: Vec2::ZERO, pull_decay: 0.99, upward_motion: Vec2::new(0.0, 1.0), action_settings: Vec::new(), action_strength: 1.0,
            force_enabled: false, force_scale: 1.0, force_tangent: 0.3, force_random: 1.0 }
    }

    #[test]
    fn sampled_force_splits_into_normal_and_tangent_shares() {
        let mut cfg = settings();
        cfg.force_enabled = true; cfg.force_random = 0.0; cfg.gravity = 0.0;
        let mut state = ClothState { normals: vec![Vec3::Y; 3], ..default() };
        let a = state.acceleration(&cfg, 1, Vec3::new(2.0, 4.0, 0.0));
        assert!(a.distance(Vec3::new(0.6, 4.0, 0.0)) < 1e-6, "normal part kept, 0.3 of the tangent part");
        assert_eq!(state.acceleration(&cfg, 1, -Vec3::new(2.0, 4.0, 0.0)), -a, "the normal's sign does not matter");
        cfg.force_random = 1.0;
        for _ in 0..100 {
            let a = state.acceleration(&cfg, 1, Vec3::Y);
            assert!((0.0..=2.0).contains(&a.y) && a.x == 0.0, "amplitude scales the force by 1 ± r");
        }
        cfg.force_enabled = false; cfg.gravity = -9.8;
        assert_eq!(state.acceleration(&cfg, 1, Vec3::X * 50.0), Vec3::Y * -9.8);
    }
    #[test]
    fn normals_are_area_weighted_face_sums() {
        let p = [Vec3::ZERO, Vec3::X, Vec3::Z, Vec3::new(1.0, 0.0, 1.0)];
        let n = vertex_normals(&[[0, 1, 2], [1, 3, 2]], &p);
        assert!(n.iter().all(|n| (n.y.abs() - 1.0).abs() < 1e-6));
    }

    #[test]
    fn component_dispatches_one_native_step_for_each_positive_frame() {
        let mut cfg = settings();
        cfg.edges.clear(); cfg.pull.fill(0.0); cfg.damping = 0.0; cfg.upward_damping = 0.0;
        let rest = vec![Vec3::ZERO, Vec3::X, Vec3::Y];
        let mut a = ClothState { native_dispatch: Some(true), ..default() };
        let mut b = ClothState { native_dispatch: Some(true), ..default() };
        a.advance(&cfg,&rest,&rest,&[],Vec3::ZERO,0.0);
        b.advance(&cfg,&rest,&rest,&[],Vec3::ZERO,0.0);
        for _ in 0..4 {
            a.advance(&cfg,&rest,&rest,&[],Vec3::ZERO,1.0/120.0);
            b.advance(&cfg,&rest,&rest,&[],Vec3::ZERO,0.1);
            assert_eq!(a.current,b.current,"one dispatch even for a long frame; no catch-up batch");
        }
        assert!(a.current[1].y < -0.05,"short positive frames still solve");
        let previous = a.previous.clone(); let current = a.current.clone();
        a.advance(&cfg,&rest,&rest,&[],Vec3::ZERO,0.0);
        assert_eq!(a.current,current); assert_eq!(a.previous,previous);
    }

    #[test]
    fn component_dispatch_samples_each_frame_using_native_interval() {
        let mut state = ClothState { native_dispatch: Some(true), ..default() };
        state.sample_entity_motion(Vec3::ZERO,Quat::IDENTITY,0.0);
        state.sample_entity_motion(Vec3::X*0.1,Quat::from_rotation_y(0.1),1.0/120.0);
        assert!((state.motion.x-0.1/NATIVE_CLOTH_STEP).abs()<1e-5);
        assert!((state.motion.y-turn_motion(Quat::IDENTITY,Quat::from_rotation_y(0.1),NATIVE_CLOTH_STEP)).abs()<1e-5);
        state.sample_entity_motion(Vec3::X*0.2,Quat::from_rotation_y(0.2),0.1);
        assert!((state.motion.x-0.1/NATIVE_CLOTH_STEP).abs()<1e-5);
    }
    #[test]
    fn cloth_follows_pins_with_inertia_and_resets_on_teleport() {
        let settings = settings();
        let mut state = ClothState::default();
        let mut targets = vec![Vec3::ZERO, -Vec3::Y * 0.5, -Vec3::Y];
        state.advance(&settings, &targets, &targets, &[], Vec3::ZERO, 0.0);
        for frame in 0..240 {
            let anchor = Vec3::X * (frame as f32 / 120.0).min(1.0);
            targets = vec![anchor, anchor - Vec3::Y * 0.5, anchor - Vec3::Y];
            let current = state.advance(&settings, &targets, &targets, &[], anchor, 1.0 / 60.0);
            assert_eq!(current[0], anchor);
            assert!(current.iter().all(|p| p.is_finite() && p.distance(anchor) < 1.2));
            if frame == 30 { assert!(current[2].x < anchor.x - 0.01, "free hem must lag the body"); }
        }
        let anchor = Vec3::X * 50.0;
        let targets = vec![anchor, anchor - Vec3::Y * 0.5, anchor - Vec3::Y];
        assert_eq!(state.advance(&settings, &targets, &targets, &[], anchor, 0.0), targets);
    }
    #[test]
    fn legacy_accumulated_cloth_is_stable_across_render_frame_rates() {
        let settings = settings();
        let targets = vec![Vec3::ZERO, Vec3::new(0.5, -0.2, 0.0), Vec3::new(1.0, -0.4, 0.0)];
        let mut a = ClothState { native_dispatch: Some(false), ..default() };
        let mut b = ClothState { native_dispatch: Some(false), ..default() };
        for _ in 0..120 { a.advance(&settings, &targets, &targets, &[], Vec3::ZERO, 1.0 / 60.0); }
        for _ in 0..240 { b.advance(&settings, &targets, &targets, &[], Vec3::ZERO, 1.0 / 120.0); }
        for (a, b) in a.current.iter().zip(b.current.iter()) { assert!(a.distance(*b) < 1e-4); }
    }
    #[test]
    fn cloth_contacts_keep_free_vertices_outside_capsules_without_moving_pins() {
        let mut settings = settings();
        settings.edges.clear();
        settings.gravity = 0.0;
        let capsule = ClothCollider { bone_id: 0, local_start: -Vec3::Y, local_end: Vec3::Y, radius: 0.1, mode: 1, threshold: 0.0 };
        // Off the axis: a vertex exactly on it has no direction and the game leaves it (0x48DEA0).
        let targets = vec![Vec3::ZERO, Vec3::X * 0.02, Vec3::Z * 0.001];
        let mut state = ClothState::default();
        for frame in 0..120 {
            let positions = state.advance(&settings, &targets, &targets, std::slice::from_ref(&capsule), Vec3::ZERO, 1.0 / 60.0);
            assert_eq!(positions[0], targets[0]);
            for p in &positions[1..] {
                assert!(p.is_finite());
                if frame > 0 { assert!(Vec2::new(p.x, p.z).length() >= 0.1076); }
            }
        }
    }
    #[test]
    fn native_action_pull_uses_motion_decay_and_target_lock() {
        let mut cfg = settings();
        cfg.pull_motion = Vec2::new(0.03, 0.015);
        let rest = vec![Vec3::ZERO, Vec3::X, Vec3::Y];
        let mut state = ClothState { motion: Vec3::new(10.0, 0.0, 0.0), ..default() };
        state.advance(&cfg, &rest, &rest, &[], Vec3::ZERO, 1.0 / 30.0);
        assert!((state.action_pull - 0.3).abs() < 1e-6);
        state.motion = Vec3::ZERO;
        state.advance(&cfg, &rest, &rest, &[], Vec3::ZERO, 1.0 / 30.0);
        assert!((state.action_pull - 0.267).abs() < 1e-6);
        cfg.action_strength = 0.0;
        state.advance(&cfg, &rest, &rest, &[], Vec3::ZERO, 1.0 / 30.0);
        assert_eq!(state.action_pull, 1.0);
        assert_eq!(state.current, rest);
        let pull = state.action_pull;
        state.advance(&cfg, &rest, &rest, &[], Vec3::ZERO, 0.0);
        assert_eq!(state.action_pull, pull);
    }

    #[test]
    fn action_settings_reject_truncation_and_nonfinite_strengths() {
        let mut data = Vec::new();
        for value in [1, crate::assets::forge::crc32("ClothActionSettings"), 1, 0, 1319343419, 42, 0.5f32.to_bits()] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(decode_action_settings(&data).unwrap(), vec![(42, 0.5)]);
        assert!(decode_action_settings(&data[..data.len()-1]).is_err());
        data[24..28].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode_action_settings(&data).is_err());
    }

    #[test]
    fn malformed_cloth_data_is_rejected() {
        for n in 0..100 {
            let bytes = vec![0; n];
            assert!(decode_settings(&bytes, &bytes, &[]).is_err());
            assert!(decode_colliders(&bytes, &bytes).is_err());
        }
    }
    #[test]
    fn adaptive_edges_blend_rest_and_animated_lengths_by_pose_deviation() {
        let rest = [Vec3::ZERO, Vec3::X];
        let targets = [Vec3::ZERO, Vec3::X * 2.0];
        assert_eq!(adaptive_length_squared(1.0, rest, targets, rest), 1.0);
        assert_eq!(adaptive_length_squared(1.0, rest, targets, targets), 4.0);
        assert_eq!(adaptive_length_squared(1.0, rest, targets, [Vec3::ZERO, Vec3::X * 1.5]), 2.5);
        assert_eq!(adaptive_length_squared(1.0, rest, rest, rest), 1.0);
    }
    #[test]
    fn degenerate_capsule_contacts_apply_no_correction() {
        let capsule = ClothCollider { bone_id: 0, local_start: -Vec3::Y, local_end: Vec3::Y, radius: 0.1, mode: 1, threshold: 0.0 };
        assert_eq!(capsule.correct(Vec3::ZERO, Vec3::X, 0.0, 0.0), Vec3::ZERO);
        // Raw-axis parallel test: a 2 m axis never reads as parallel, so the perpendicular branch runs and,
        // with the escape along the axis, its zero cross product gives no correction.
        let mut directed = capsule.clone();
        directed.mode = 0;
        let p = Vec3::new(0.05, 0.0, 0.0);
        assert_eq!(directed.correct(p, Vec3::Y * 3.0, 0.0, 0.0), p);
    }
    #[test]
    fn directed_capsule_contacts_escape_toward_the_skinned_side() {
        let mut capsule = ClothCollider { bone_id: 0, local_start: -Vec3::Y, local_end: Vec3::Y, radius: 0.1, mode: 0, threshold: 0.0 };
        let position = Vec3::new(-0.04, 0.0, 0.03);
        let target = Vec3::X * 0.3;
        let directed = capsule.correct(position, target, 0.0, 0.015);
        assert!(directed.x > 0.09);
        assert!((directed.z - position.z).abs() < 1e-6);
        assert!((Vec2::new(directed.x, directed.z).length() - 0.1).abs() < 1e-6);
        capsule.threshold = 0.5;
        assert!(capsule.correct(position, target, 0.0, 0.015).x < 0.0, "threshold selects radial response");
        capsule.mode = 2;
        assert_eq!(capsule.correct(position, target, 0.0, 0.015), position);
        capsule.mode = 0;
        let near_surface = Vec3::new(0.099, 0.0, 0.0);
        assert_eq!(capsule.correct(near_surface, target, 0.0, 0.015), near_surface);
    }
    #[test]
    fn render_guard_handles_fast_turns_and_jumps_between_solver_steps() {
        let mut settings = settings();
        settings.edges.clear();
        let rest = vec![Vec3::Y, Vec3::X * 0.16, Vec3::new(0.16, -0.5, 0.0)];
        let mut state = ClothState { render_guard: Some(true), ..default() };
        state.advance(&settings, &rest, &rest, &[], Vec3::ZERO, 0.0);
        for frame in 0..480 {
            let t = frame as f32 / 240.0;
            let angle = if t < 0.5 { 0.0 } else { ((t - 0.5) * 12.0).min(std::f32::consts::PI) };
            let rotation = Quat::from_rotation_y(angle);
            let anchor = Vec3::Y * (t * std::f32::consts::PI).sin().max(0.0);
            let targets: Vec<_> = rest.iter().map(|&p| anchor + rotation * p).collect();
            let capsule = ClothCollider { bone_id: 0, local_start: anchor - Vec3::Y, local_end: anchor + Vec3::Y,
                radius: 0.2, mode: 0, threshold: 0.0 };
            let current = state.advance(&settings, &targets, &targets, std::slice::from_ref(&capsule), anchor, 1.0 / 240.0);
            assert_eq!(current[0], targets[0], "pins follow every frame");
            for &p in &current[1..] {
                assert!(p.is_finite());
                assert!(capsule.penetration(p, 0.01) <= 1.1e-5, "contact on a frame without a solver step");
            }
        }
    }
    #[test]
    fn render_guard_cleans_up_penetration_reintroduced_by_edges() {
        let mut settings = settings();
        settings.pinned = vec![true, false, true];
        let rest = vec![Vec3::ZERO, Vec3::X * 0.05, Vec3::X * 0.1];
        let capsule = ClothCollider { bone_id: 0, local_start: -Vec3::Y, local_end: Vec3::Y, radius: 0.2, mode: 1, threshold: 0.0 };
        let mut state = ClothState { render_guard: Some(true), ..default() };
        for _ in 0..120 {
            let p = state.advance(&settings, &rest, &rest, std::slice::from_ref(&capsule), Vec3::ZERO, 1.0 / 60.0);
            assert_eq!(p[0], rest[0]);
            assert_eq!(p[2], rest[2]);
            assert!(capsule.penetration(p[1], 0.01) <= 1.1e-5);
        }
        assert!(state.contact_before.0 > 0, "native edge pass reproduces penetration");
        assert_eq!(state.contact_after.0, 0);
        let before = state.current.clone();
        state.advance(&settings, &rest, &rest, std::slice::from_ref(&capsule), Vec3::ZERO, 0.0);
        assert_eq!(state.current, before, "paused screenshots must not evolve the cloth");
    }
    #[test]
    fn overlapping_leg_capsules_do_not_trap_the_render_projection() {
        let capsules: Vec<_> = [-0.15, 0.15].into_iter().map(|x| ClothCollider {
            bone_id: 0, local_start: Vec3::new(x, -1.0, 0.0), local_end: Vec3::new(x, 1.0, 0.0),
            radius: 0.2, mode: 1, threshold: 0.0,
        }).collect();
        let escaped = escape_overlapping_capsules(Vec3::ZERO, Vec3::Z * 0.3, Vec3::Z * 0.3, 0.01, &capsules);
        assert!(escaped.is_finite() && escaped.length() < 0.15);
        assert!(capsules.iter().all(|c| c.penetration(escaped, 0.01) <= 1.1e-5));
    }
    #[test]
    fn surface_guard_separates_triangle_interiors_without_moving_pins() {
        let mut settings = settings();
        settings.pinned = vec![false; 3];
        settings.triangles = vec![[0, 1, 2]];
        let targets = vec![Vec3::new(-0.3, 0.0, -0.3), Vec3::new(0.3, 0.0, -0.3), Vec3::new(0.0, 0.0, 0.3)];
        let capsule = ClothCollider { bone_id: 0, local_start: Vec3::ZERO, local_end: Vec3::ZERO, radius: 0.1, mode: 1, threshold: 0.0 };
        let mut positions = targets.clone();
        assert_eq!(surface_contacts(&settings, &mut positions, &targets, std::slice::from_ref(&capsule), false), 1);
        for _ in 0..32 { surface_contacts(&settings, &mut positions, &targets, std::slice::from_ref(&capsule), true); }
        assert_eq!(surface_contacts(&settings, &mut positions, &targets, std::slice::from_ref(&capsule), false), 0);
        settings.pinned[0] = true;
        positions = targets.clone();
        surface_contacts(&settings, &mut positions, &targets, std::slice::from_ref(&capsule), true);
        assert_eq!(positions[0], targets[0]);
        assert!(positions.iter().zip(&targets).all(|(p,t)| p.distance(*t) <= 0.02001));
    }
    #[test]
    fn fold_guard_detects_crossings_and_excludes_connected_faces() {
        let mut settings = settings();
        settings.pinned = vec![false; 6];
        settings.triangles = vec![[0, 1, 2], [3, 4, 5]];
        let mut p = vec![Vec3::new(-0.2, 0.0, -0.2), Vec3::new(0.2, 0.0, -0.2), Vec3::new(0.0, 0.0, 0.2),
            Vec3::new(0.0, -0.1, -0.1), Vec3::new(0.0, 0.1, -0.1), Vec3::new(0.0, 0.0, 0.1)];
        let previous = p.clone();
        assert_eq!(fold_contacts(&settings, &mut p, &previous, false), 1);
        settings.pinned[0] = true;
        fold_contacts(&settings, &mut p, &previous, true);
        assert_eq!(p[0], previous[0]);
        assert!(p.iter().all(|p| p.is_finite()));
        for _ in 0..32 { fold_contacts(&settings, &mut p, &previous, true); }
        assert_eq!(fold_contacts(&settings, &mut p, &previous, false), 0);
        settings.triangles[1] = [0, 4, 5];
        assert_eq!(fold_contacts(&settings, &mut p, &previous, false), 0);
    }
}
