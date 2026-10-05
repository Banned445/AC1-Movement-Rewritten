//! Character cloth targets, pin masks and edge constraints (RE/09 §8).

use bevy::prelude::*;

#[derive(Clone)]
pub struct ClothSettings {
    pub pinned: Vec<bool>,
    pub pull: Vec<f32>,
    pub edges: Vec<[usize; 2]>,
    pub damping: f32,
    pub upward_damping: f32,
    pub gravity: f32,
    pub iterations: usize,
    pub vertex_radius: Vec<f32>,
    pub colliders: Vec<ClothCollider>,
}

#[derive(Clone)]
pub struct ClothCollider {
    pub bone_id: u32,
    pub local_start: Vec3,
    pub local_end: Vec3,
    pub radius: f32,
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
            if !a.is_finite() || !b.is_finite() || !radius.is_finite() || radius <= 0.0 { return None; }
            colliders.push(ClothCollider { bone_id: *bone_id, local_start: frame.transform_point3(a), local_end: frame.transform_point3(b), radius });
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
        let n = positions.len() / 12;
        if n != compiled.len() || flags.len() != 4 * n || indices.len() != masks.len() * 6 { return None; }
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
        let class = crate::assets::forge::crc32("Cloth");
        let c = (4..entity.len().saturating_sub(4)).find(|&i| word(entity, i) == Some(class))?;
        // Rank 9's fixed-size base fields; layout traced through 0x4CFCF0 / 0x6C7CA0.
        let damping = float(entity, c + 28)?;
        let upward_damping = float(entity, c + 24)?;
        let pull_min = float(entity, c + 257)?;
        let pull_max = float(entity, c + 261)?;
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
        if !(1..=7).contains(&iterations) || ![damping, upward_damping, pull_min, pull_max, gravity].iter().all(|v| v.is_finite())
            || !(0.0..=1.0).contains(&damping) || !(0.0..=1.0).contains(&upward_damping)
            || !(0.0..=1.0).contains(&pull_min) || !(0.0..=1.0).contains(&pull_max) { return None; }
        let mut edges = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (tri, &mask) in indices.chunks_exact(6).zip(masks) {
            let v: Vec<usize> = tri.chunks_exact(2).map(|b| u16::from_le_bytes(b.try_into().unwrap()) as usize).collect();
            if v.iter().any(|&i| i >= n) { return None; }
            for k in 0..3 {
                if mask & (1 << k) == 0 { continue; }
                let (a, b) = (to_render[v[k]], to_render[v[(k + 1) % 3]]);
                let key = (a.min(b), a.max(b));
                if seen.insert(key) { edges.push([a, b]); }
            }
        }
        Some(ClothSettings { pinned, pull, edges, damping, upward_damping, gravity, iterations, vertex_radius, colliders: Vec::new() })
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
    current: Vec<Vec3>,
    previous: Vec<Vec3>,
    lengths_squared: Vec<f32>,
    accumulator: f32,
    anchor: Option<Vec3>,
}

impl ClothState {
    pub fn advance(&mut self, settings: &ClothSettings, targets: &[Vec3], capsules: &[ClothCollider], anchor: Vec3, dt: f32) -> &[Vec3] {
        // PORT: fixed 60 Hz, bounded catch-up and reset on teleports; original scheduler not ported.
        let reset = self.current.len() != targets.len() || self.anchor.is_none_or(|p| p.distance_squared(anchor) > 4.0);
        self.anchor = Some(anchor);
        if reset {
            self.current = targets.to_vec();
            self.previous = targets.to_vec();
            self.lengths_squared = settings.edges.iter().map(|[a, b]| targets[*a].distance_squared(targets[*b])).collect();
            self.accumulator = 0.0;
        }
        self.accumulator += dt.clamp(0.0, 0.1);
        while self.accumulator >= 1.0 / 60.0 {
            self.accumulator -= 1.0 / 60.0;
            for (i, &target) in targets.iter().enumerate() {
                let old = self.current[i];
                if settings.pinned[i] { self.current[i] = target; }
                else {
                    let velocity = old - self.previous[i];
                    let vertical_damping = if velocity.y >= 0.0 { settings.upward_damping } else { settings.damping };
                    let predicted = old + velocity * Vec3::new(1.0 - settings.damping, 1.0 - vertical_damping, 1.0 - settings.damping)
                        + Vec3::Y * settings.gravity / (60.0 * 60.0);
                    self.current[i] = predicted.lerp(target, settings.pull[i]);
                }
                self.previous[i] = old;
            }
            // Squared-length correction and pin handling from SoftBody__sub_4D3950 0x4D3950.
            for _ in 0..settings.iterations {
                for (edge, &rest) in settings.edges.iter().zip(&self.lengths_squared) {
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
            // Cloth__sub_6C8260 / 0x6C9520. PORT: radial response also replaces mode 0's
            // cached escape direction; authored capsule frames/radii are retained (RE/09 §8.1).
            for (i, position) in self.current.iter_mut().enumerate() {
                if settings.pinned[i] { continue; }
                for capsule in capsules {
                    let axis = capsule.local_end - capsule.local_start;
                    let length = axis.length_squared();
                    let t = if length > 1e-12 { ((*position - capsule.local_start).dot(axis) / length).clamp(0.0, 1.0) } else { 0.0 };
                    let closest = capsule.local_start + axis * t;
                    let delta = *position - closest;
                    let radius = capsule.radius + settings.vertex_radius[i];
                    if delta.length_squared() < radius * radius {
                        // PORT: use the skinned target to choose a stable escape at the capsule axis.
                        let normal = delta.try_normalize().or_else(|| (targets[i] - closest).try_normalize()).unwrap_or(Vec3::X);
                        *position = closest + normal * radius;
                    }
                }
            }
        }
        // Pins follow the skeleton even on render frames with no simulation step.
        for (i, &target) in targets.iter().enumerate() { if settings.pinned[i] { self.current[i] = target; } }
        &self.current
    }
}

pub fn update_cloth(time: Res<Time>, transforms: Query<&GlobalTransform>, mut cloth: Query<&mut CharacterCloth>, mut meshes: ResMut<Assets<Mesh>>) {
    for mut cloth in &mut cloth {
        let Ok(player) = transforms.get(cloth.player) else { continue; };
        let matrices: Option<Vec<_>> = cloth.joints.iter().zip(&cloth.inverse_bindposes)
            .map(|(&joint, inverse)| transforms.get(joint).ok().map(|t| t.to_matrix() * *inverse)).collect();
        let Some(matrices) = matrices else { continue; };
        let targets: Vec<Vec3> = cloth.rest.iter().enumerate().map(|(i, &p)| (0..4)
            .map(|k| matrices[cloth.palette[i][k] as usize].transform_point3(p) * cloth.weights[i][k]).sum()).collect();
        let settings = cloth.settings.clone();
        let capsules: Option<Vec<_>> = settings.colliders.iter().zip(&cloth.collider_joints).map(|(c, &joint)| {
            let transform = transforms.get(joint).ok()?.to_matrix();
            Some(ClothCollider { bone_id: c.bone_id, local_start: transform.transform_point3(c.local_start),
                local_end: transform.transform_point3(c.local_end), radius: c.radius * transform.x_axis.truncate().length() })
        }).collect();
        let Some(capsules) = capsules else { continue; };
        let inverse = player.to_matrix().inverse();
        let positions: Vec<[f32; 3]> = cloth.state.advance(&settings, &targets, &capsules, player.translation(), time.delta_secs()).iter()
            .map(|&p| inverse.transform_point3(p).to_array()).collect();
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
    fn settings() -> ClothSettings {
        ClothSettings { pinned: vec![true, false, false], pull: vec![1.0, 0.015, 0.015], edges: vec![[0, 1], [1, 2]], damping: 0.1, upward_damping: 0.75, gravity: -9.8, iterations: 3, vertex_radius: vec![0.01; 3], colliders: Vec::new() }
    }
    #[test]
    fn cloth_follows_pins_with_inertia_and_resets_on_teleport() {
        let settings = settings();
        let mut state = ClothState::default();
        let mut targets = vec![Vec3::ZERO, -Vec3::Y * 0.5, -Vec3::Y];
        state.advance(&settings, &targets, &[], Vec3::ZERO, 0.0);
        for frame in 0..240 {
            let anchor = Vec3::X * (frame as f32 / 120.0).min(1.0);
            targets = vec![anchor, anchor - Vec3::Y * 0.5, anchor - Vec3::Y];
            let current = state.advance(&settings, &targets, &[], anchor, 1.0 / 60.0);
            assert_eq!(current[0], anchor);
            assert!(current.iter().all(|p| p.is_finite() && p.distance(anchor) < 1.2));
            if frame == 30 { assert!(current[2].x < anchor.x - 0.01, "free hem must lag the body"); }
        }
        let anchor = Vec3::X * 50.0;
        let targets = vec![anchor, anchor - Vec3::Y * 0.5, anchor - Vec3::Y];
        assert_eq!(state.advance(&settings, &targets, &[], anchor, 0.0), targets);
    }
    #[test]
    fn cloth_is_stable_across_render_frame_rates() {
        let settings = settings();
        let targets = vec![Vec3::ZERO, Vec3::new(0.5, -0.2, 0.0), Vec3::new(1.0, -0.4, 0.0)];
        let mut a = ClothState::default();
        let mut b = ClothState::default();
        for _ in 0..120 { a.advance(&settings, &targets, &[], Vec3::ZERO, 1.0 / 60.0); }
        for _ in 0..240 { b.advance(&settings, &targets, &[], Vec3::ZERO, 1.0 / 120.0); }
        for (a, b) in a.current.iter().zip(b.current.iter()) { assert!(a.distance(*b) < 1e-4); }
    }
    #[test]
    fn cloth_contacts_keep_free_vertices_outside_capsules_without_moving_pins() {
        let mut settings = settings();
        settings.edges.clear();
        settings.gravity = 0.0;
        let capsule = ClothCollider { bone_id: 0, local_start: -Vec3::Y, local_end: Vec3::Y, radius: 0.1 };
        let targets = vec![Vec3::ZERO, Vec3::X * 0.02, Vec3::ZERO];
        let mut state = ClothState::default();
        for _ in 0..120 {
            let positions = state.advance(&settings, &targets, std::slice::from_ref(&capsule), Vec3::ZERO, 1.0 / 60.0);
            assert_eq!(positions[0], targets[0]);
            for p in &positions[1..] {
                assert!(p.is_finite());
                assert!(Vec2::new(p.x, p.z).length() >= 0.10999);
            }
        }
    }
    #[test]
    fn malformed_cloth_data_is_rejected() {
        for n in 0..100 {
            let bytes = vec![0; n];
            assert!(decode_settings(&bytes, &bytes, &[]).is_err());
            assert!(decode_colliders(&bytes, &bytes).is_err());
        }
    }
}
