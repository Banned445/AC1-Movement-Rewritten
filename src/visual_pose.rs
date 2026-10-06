//! Secondary skirt pose modifiers (RE/09 §8.6).
use bevy::prelude::*;
use crate::assets::ac_formats::parse_skeleton;

// PORT: opt-in until the complete modifier chain removes the observed jump-contact regression.
pub const SKIRT_ROTATION_COPIES: bool = false;

#[derive(Clone, Debug)]
pub struct SkirtCompression {
    pub target: usize,
    pub sources: [usize; 2],
    pub position_weight: f32,
    pub rotation_weight: f32,
    pub position: bool,
    pub rotation: bool,
}

/// Rank 9 CompressBoneModifier (0x6C4B50): external references and identity offsets.
pub fn decode_compressions(data: &[u8]) -> Result<Vec<SkirtCompression>, String> {
    let bones = parse_skeleton(data);
    let word = |p: usize| data.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let scalar = |p| word(p).map(f32::from_bits).filter(|v| v.is_finite());
    let reference = |p| {
        if data.get(p) != Some(&2) { return None; }
        let id = word(p + 1)?;
        bones.iter().find(|b| b.object_id == id).map(|b| b.bone_id as usize)
    };
    let class = crate::assets::forge::crc32("CompressBoneModifier");
    let mut result = Vec::new();
    for p in 5..data.len().saturating_sub(4) {
        if data[p - 5] != 0 || word(p) != Some(class) { continue; }
        let decode = || {
            let b = p + 4;
            let target = reference(b)?;
            let sources = [reference(b + 14)?, reference(b + 19)?];
            let position_weight = scalar(b + 6)?;
            let rotation_weight = scalar(b + 10)?;
            // PORT: reject other outfits' offsets until quaternion composition is verified.
            for offset in [24, 40, 56, 72] {
                for k in 0..4 {
                    let expected = if offset < 56 && k == 3 { 1.0 } else { 0.0 };
                    if scalar(b + offset + k * 4)? != expected { return None; }
                }
            }
            let rotation = *data.get(b + 88)?;
            let position = *data.get(b + 89)?;
            if rotation > 1 || position > 1 || sources.contains(&target) { return None; }
            Some(SkirtCompression { target, sources, position_weight, rotation_weight, position: position != 0, rotation: rotation != 0 })
        };
        result.push(decode().ok_or("unsupported skirt compression layout")?);
    }
    Ok(result)
}

/// RotationPasteModifier deserialization: owner/base flag 0x697BD0, source 0x5FE7E0.
pub fn decode_rotation_copies(data: &[u8]) -> Result<Vec<(u32, u32)>, String> {
    let bones = parse_skeleton(data);
    let resolve = |id| bones.iter().find(|b| b.object_id == id).map(|b| b.bone_id);
    let word = |p: usize| data.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let class = crate::assets::forge::crc32("RotationPasteModifier");
    let mut copies = Vec::new();
    for p in 5..data.len().saturating_sub(4) {
        if data[p - 5] != 0 || word(p) != Some(class) { continue; }
        // PORT: this outfit uses external bone references; reject unknown inline/null layouts.
        if data.get(p + 4) != Some(&2) || data.get(p + 10) != Some(&2) {
            return Err("unsupported skirt rotation-copy references".into());
        }
        let owner = word(p + 5).and_then(resolve).ok_or("skirt rotation-copy owner missing")?;
        let source = word(p + 11).and_then(resolve).ok_or("skirt rotation-copy source missing")?;
        if owner == source || copies.iter().any(|(target, _)| *target == owner) {
            return Err("duplicate or cyclic skirt rotation copy".into());
        }
        copies.push((owner, source));
    }
    Ok(copies)
}

#[derive(Component)]
pub struct VisualRotationCopies {
    pub joints: Vec<Entity>,
    pub parents: Vec<Option<usize>>,
    pub copies: Vec<(usize, usize)>,
    pub compressions: Vec<SkirtCompression>,
}

fn world_pose(local: &[Transform], parents: &[Option<usize>]) -> Vec<Mat4> {
    let mut world = vec![Mat4::IDENTITY; local.len()];
    for i in 0..local.len() {
        world[i] = parents[i].map_or(Mat4::IDENTITY, |p| world[p]) * local[i].to_matrix();
    }
    world
}

fn compression_rotation(second: Quat, first: Quat, weight: f32) -> Quat {
    // Native shortest-arc interpolation and small-angle fallback (0x48F120).
    let dot = second.dot(first);
    if dot.abs() >= 1.0 { return first; }
    let angle = dot.abs().acos();
    if angle <= 0.0005 { return first; }
    let a = ((1.0 - weight) * angle).sin() / angle.sin();
    let b = (weight * angle).sin() / angle.sin() * if dot < 0.0 { -1.0 } else { 1.0 };
    (second * a + first * b).normalize()
}

fn compress_pose(local: &mut [Transform], parents: &[Option<usize>], compressions: &[SkirtCompression]) {
    for c in compressions {
        let world = world_pose(local, parents);
        let (_, first_rotation, first_position) = world[c.sources[0]].to_scale_rotation_translation();
        let (_, second_rotation, second_position) = world[c.sources[1]].to_scale_rotation_translation();
        let parent = parents[c.target].map_or(Mat4::IDENTITY, |p| world[p]);
        // 0x6C4FF0 blends source B toward source A and updates the owner hierarchy.
        if c.position {
            local[c.target].translation = parent.inverse().transform_point3(second_position.lerp(first_position, c.position_weight));
        }
        if c.rotation {
            local[c.target].rotation = (parent.to_scale_rotation_translation().1.inverse()
                * compression_rotation(second_rotation, first_rotation, c.rotation_weight)).normalize();
        }
    }
}

fn copy_rotations(local: &mut [Transform], parents: &[Option<usize>], copies: &[(usize, usize)]) {
    // Native matrix copy retains owner position (0x5FEA10); convert to parent-local rotation.
    let mut world = vec![Mat4::IDENTITY; local.len()];
    for &(target, source) in copies {
        for i in 0..local.len() {
            world[i] = parents[i].map_or(Mat4::IDENTITY, |p| world[p]) * local[i].to_matrix();
        }
        let source_rotation = world[source].to_scale_rotation_translation().1;
        let parent_rotation = parents[target].map_or(Quat::IDENTITY, |p| world[p].to_scale_rotation_translation().1);
        local[target].rotation = (parent_rotation.inverse() * source_rotation).normalize();
    }
}

pub fn update_rotation_copies(rigs: Query<&VisualRotationCopies>, mut transforms: Query<&mut Transform>, mut enabled: Local<Option<bool>>) {
    let enabled = *enabled.get_or_insert_with(|| (SKIRT_ROTATION_COPIES || std::env::var_os("AC_SKIRT_ROTATION_COPIES").is_some()) && std::env::var_os("AC_NO_SKIRT_ROTATION_COPIES").is_none());
    if !enabled { return; }
    for rig in &rigs {
        let Some(mut local) = rig.joints.iter().map(|&joint| transforms.get(joint).ok().copied()).collect::<Option<Vec<_>>>() else { continue; };
        // PORT: evaluate the root compression before copies; complete native scheduling remains open.
        compress_pose(&mut local, &rig.parents, &rig.compressions);
        copy_rotations(&mut local, &rig.parents, &rig.copies);
        for c in &rig.compressions {
            if let Ok(mut transform) = transforms.get_mut(rig.joints[c.target]) { *transform = local[c.target]; }
        }
        for &(target, _) in &rig.copies {
            if let Ok(mut transform) = transforms.get_mut(rig.joints[target]) { transform.rotation = local[target].rotation; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compression_blends_world_frames_and_preserves_primary_joints() {
        let mut local = vec![Transform::from_xyz(4.0, 2.0, 0.0).with_rotation(Quat::from_rotation_z(0.4)),
            Transform::from_xyz(1.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(0.8)),
            Transform::from_xyz(-1.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(-0.8)),
            Transform::from_xyz(0.0, 1.0, 0.0)];
        let before = local.clone();
        let parents = [None, Some(0), Some(0), Some(0)];
        let world = world_pose(&local, &parents);
        let c = SkirtCompression { target: 3, sources: [1, 2], position_weight: 0.25, rotation_weight: 0.75, position: true, rotation: true };
        compress_pose(&mut local, &parents, &[c]);
        assert_eq!(local[..3], before[..3]);
        let actual = world_pose(&local, &parents)[3];
        assert!(actual.w_axis.truncate().distance(world[2].w_axis.truncate().lerp(world[1].w_axis.truncate(), 0.25)) < 1e-5);
        assert!(local[3].rotation.angle_between(Quat::from_rotation_y(0.4)) < 1e-5);
        assert_eq!(local[3].scale, Vec3::ONE);
    }
    #[test]
    fn compression_flags_preserve_disabled_channels() {
        let mut local = vec![Transform::from_rotation(Quat::from_rotation_z(0.5)), Transform::from_xyz(1.0, 0.0, 0.0), Transform::from_xyz(0.0, 1.0, 0.0)];
        let parents = [None, Some(0), Some(0)];
        let mut c = SkirtCompression { target: 2, sources: [0, 1], position_weight: 0.5, rotation_weight: 0.5, position: false, rotation: true };
        let before = local[2];
        compress_pose(&mut local, &parents, &[c.clone()]);
        assert_eq!(local[2].translation, before.translation);
        c.position = true;
        c.rotation = false;
        let before = local[2].rotation;
        compress_pose(&mut local, &parents, &[c]);
        assert_eq!(local[2].rotation, before);
        let q = Quat::from_rotation_x(0.6);
        assert!(compression_rotation(q, -q, 0.3).angle_between(q) < 1e-5);
    }
    #[test]
    fn rotation_copy_preserves_position_and_primary_pose() {
        let mut local = vec![Transform::from_rotation(Quat::from_rotation_z(0.6)),
            Transform::from_xyz(0.2, 0.0, 0.0).with_rotation(Quat::from_rotation_x(0.9)),
            Transform::from_xyz(0.0, 0.4, 0.0).with_rotation(Quat::from_rotation_y(0.8))];
        let before = local.clone();
        let old_world = before[0].to_matrix() * before[1].to_matrix() * before[2].to_matrix();
        copy_rotations(&mut local, &[None, Some(0), Some(1)], &[(2, 0)]);
        assert_eq!(local[..2], before[..2]);
        assert_eq!(local[2].translation, before[2].translation);
        let world = local[0].to_matrix() * local[1].to_matrix() * local[2].to_matrix();
        assert!(world.w_axis.distance(old_world.w_axis) < 1e-6);
        assert!(world.to_scale_rotation_translation().1.angle_between(local[0].rotation) < 1e-5);
    }
    #[test]
    fn chained_copies_use_the_updated_source_pose() {
        let mut local = vec![Transform::from_rotation(Quat::from_rotation_y(0.7)),
            Transform::from_rotation(Quat::from_rotation_x(0.4)), Transform::from_rotation(Quat::from_rotation_z(0.5))];
        copy_rotations(&mut local, &[None, Some(0), Some(1)], &[(1, 0), (2, 1)]);
        assert!(local[1].rotation.angle_between(Quat::IDENTITY) < 1e-5);
        assert!(local[2].rotation.angle_between(Quat::IDENTITY) < 1e-5);
    }
}
