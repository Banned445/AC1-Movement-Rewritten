//! Secondary skirt pose modifiers (RE/09 §8.6).
use bevy::prelude::*;
use crate::assets::ac_formats::parse_skeleton;

// PORT: opt-in until the complete modifier chain removes the observed jump-contact regression.
pub const SKIRT_ROTATION_COPIES: bool = false;

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
        copy_rotations(&mut local, &rig.parents, &rig.copies);
        for &(target, _) in &rig.copies {
            if let Ok(mut transform) = transforms.get_mut(rig.joints[target]) { transform.rotation = local[target].rotation; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
