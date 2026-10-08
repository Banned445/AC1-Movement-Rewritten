//! Secondary skirt pose modifiers (RE/09 §8.6).
use bevy::prelude::*;
use crate::assets::ac_formats::parse_skeleton;

// The player's skeleton runs its authored modifiers in gameplay (LOD 5, +332 bit 0x10; RE/09 §8.16).
// AC_NO_SKIRT_ROTATION_COPIES=1 compares the old rest-pose skirt.
pub const SKIRT_ROTATION_COPIES: bool = true;
// Authored hood/sword-tag modifiers, independently fenced from the skirt (RE/09 §8.13).
const CHARACTER_EQUIPMENT_DYNAMICS: bool = true;

pub fn skirt_modifiers_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| (SKIRT_ROTATION_COPIES || std::env::var_os("AC_SKIRT_ROTATION_COPIES").is_some()) && std::env::var_os("AC_NO_SKIRT_ROTATION_COPIES").is_none())
}

fn equipment_modifiers_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("AC_CHARACTER_EQUIPMENT_DYNAMICS").map_or(CHARACTER_EQUIPMENT_DYNAMICS, |v| v != "0"))
}

#[derive(Clone, Debug)]
pub struct SkirtCompression {
    pub target: usize,
    pub sources: [usize; 2],
    pub position_weight: f32,
    pub rotation_weight: f32,
    pub position: bool,
    pub rotation: bool,
}

#[derive(Clone, Debug)]
pub struct SkirtLookAt {
    pub equipment: bool,
    pub target: usize,
    pub aim: usize,
    pub aim_axis: usize,
}

/// Rank 9's null-reference, zero-offset skirt Z/Y and hood X/Z look-at (0x648B90).
pub fn decode_look_at(data: &[u8]) -> Result<Vec<SkirtLookAt>, String> {
    let bones = parse_skeleton(data);
    let word = |p: usize| data.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let reference = |p| {
        if data.get(p) != Some(&2) { return None; }
        let id = word(p + 1)?;
        bones.iter().find(|b| b.object_id == id).map(|b| b.bone_id as usize)
    };
    let class = crate::assets::forge::crc32("LookAtBoneModifier");
    let mut result = Vec::new();
    for p in 5..data.len().saturating_sub(4) {
        if data[p - 5] != 0 || word(p) != Some(class) { continue; }
        let decode = || {
            let b = p + 4;
            let target = reference(b)?;
            // PORT: reject other layouts rather than inventing fallback/axis conventions.
            if !matches!(data.get(b + 5), Some(0 | 1)) || data.get(b + 6) != Some(&3) { return None; }
            let aim = reference(b + 7)?;
            let aim_axis = word(b + 28)? as usize;
            if target == aim || (0..4).any(|k| word(b + 12 + k * 4) != Some(0))
                || !matches!((aim_axis, word(b + 32)?), (2, 1) | (0, 2)) { return None; }
            Some(SkirtLookAt { equipment: false, target, aim, aim_axis })
        };
        result.push(decode().ok_or("unsupported skirt look-at layout")?);
    }
    Ok(result)
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

/// The SkeletonComponent's embedded force config (SkeletonComponent__Read 0x4E4E30 → ForceConfig__Read 0x5C6B20):
/// Some(scale) when enabled. PORT: located by its class marker inside the component.
pub fn decode_skeleton_force(entity: &[u8]) -> Option<f32> {
    let word = |p: usize| entity.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let component = (4..entity.len().saturating_sub(4)).find(|&p| word(p) == Some(crate::assets::forge::crc32("SkeletonComponent")))?;
    let mut configs = (component..(component + 400).min(entity.len().saturating_sub(4))).filter(|&p| word(p) == Some((-403070396i32) as u32));
    let p = configs.next()?;
    if configs.next().is_some() || entity.get(p + 9..p + 11)? != [0, 0] { return None; }
    let scale = word(p + 5).map(f32::from_bits).filter(|v| v.is_finite())?;
    (*entity.get(p + 4)? == 1).then_some(scale)
}

#[derive(Component)]
pub struct VisualRotationCopies {
    pub joints: Vec<Entity>,
    /// Authored local rest of every joint; secondary joints restart from it each frame (0x4E0650).
    pub rest: Vec<Transform>,
    /// Movement joints come first; the animator owns them.
    pub primary: usize,
    pub force_scale: Option<f32>,
    /// The previous frame's environment sample at bone 0 (0x4E7550 stores it after the modifiers).
    pub environment: Vec3,
    pub parents: Vec<Option<usize>>,
    pub copies: Vec<(usize, usize)>,
    pub compressions: Vec<SkirtCompression>,
    pub look_at: Vec<SkirtLookAt>,
    pub hinges: Vec<crate::skirt_hinge::SkirtHinge>,
    pub hinge_states: Vec<crate::skirt_hinge::HingeState>,
    pub root: Entity,
    pub previous_anchor: Option<Vec3>,
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

fn look_at_pose(local: &mut [Transform], parents: &[Option<usize>], modifiers: &[SkirtLookAt]) {
    for m in modifiers {
        let world = world_pose(local, parents);
        let owner = world[m.target];
        let Some(direction) = (world[m.aim].w_axis - owner.w_axis).truncate().try_normalize() else { continue; };
        // 0x648E94 uses the owner's current Z row when it has a parent.
        let reference = owner.z_axis.truncate();
        let Some(side) = reference.cross(direction).try_normalize() else { continue; };
        let up = direction.cross(side);
        // Native 0x648D80: skirt axes 2/1 = side/up/aim; hood axes 0/2 = aim/side/up.
        let basis = if m.aim_axis == 0 { Mat3::from_cols(direction, side, up) } else { Mat3::from_cols(side, up, direction) };
        let rotation = Quat::from_mat3(&basis);
        let parent = parents[m.target].map_or(Quat::IDENTITY, |p| world[p].to_scale_rotation_translation().1);
        // PORT: keep the preceding pose for coincident/parallel directions instead of a singular frame.
        local[m.target].rotation = (parent.inverse() * rotation).normalize();
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

pub fn update_rotation_copies(time: Res<Time>, wind: Res<crate::wind::WindField>, mut rigs: Query<(Entity, &mut VisualRotationCopies)>, mut transforms: Query<&mut Transform>) {
    let skirt = skirt_modifiers_enabled();
    let equipment = equipment_modifiers_enabled();
    if !skirt && !equipment { return; }
    if time.delta_secs() <= 0.0 { return; }
    for (player, mut rig) in &mut rigs {
        let (Ok(player_pose), Ok(root_pose)) = (transforms.get(player), transforms.get(rig.root)) else { continue; };
        let frame = player_pose.to_matrix() * root_pose.to_matrix();
        let gravity_frame = root_pose.rotation;
        let anchor = frame.w_axis.truncate();
        // PORT: bounded reset on teleports/long stalls in place of native frame-id continuity.
        let reset = time.delta_secs() > 0.1 || rig.previous_anchor.is_none_or(|old| old.distance_squared(anchor) > 4.0);
        rig.previous_anchor = Some(anchor);
        // SkeletonComponent__BeginFrame 0x4E0650 copies the authored base pose into the live pose every frame,
        // so modifiers never read their own previous output.
        for i in rig.primary..rig.joints.len() {
            if let Ok(mut transform) = transforms.get_mut(rig.joints[i]) { *transform = rig.rest[i]; }
        }
        let Some(mut local) = rig.joints.iter().map(|&joint| transforms.get(joint).ok().copied()).collect::<Option<Vec<_>>>() else { continue; };
        // Root's authored list is compression then look-at; native preserves owner-list order (0x4E6820).
        // PORT: native pose-slot blending is still unported.
        if skirt { compress_pose(&mut local, &rig.parents, &rig.compressions); }
        let look_at: Vec<_> = rig.look_at.iter().filter(|m| if m.equipment { equipment } else { skirt }).cloned().collect();
        look_at_pose(&mut local, &rig.parents, &look_at);
        if skirt { copy_rotations(&mut local, &rig.parents, &rig.copies); }
        let mut last_target = None;
        for i in 0..rig.hinges.len() {
            let h = &rig.hinges[i];
            if !(if h.equipment { equipment } else { skirt }) { continue; }
            let world: Vec<_> = world_pose(&local, &rig.parents).into_iter().map(|m| frame * m).collect();
            let parent = rig.parents[h.target].map_or(frame, |p| world[p]);
            let base = parent * h.rest.to_matrix();
            let owner = world[h.target];
            let force = h.force_reference.map_or(gravity_frame * h.force, |r| world[r].transform_vector3(h.force));
            let reference = h.constraint_reference.map(|r| world[r].transform_vector3(h.reference_direction));
            let target = h.target;
            let h = h.clone();
            let environment = rig.environment;
            let solved = rig.hinge_states[i].solve(&h, base, owner, force, reference, environment, time.delta_secs(), reset);
            // 0x697C80 composes later hinges as current * inverse(base) * solved.
            let applied = if last_target == Some(target) { owner * base.inverse() * solved } else { solved };
            let (_, rotation, translation) = (parent.inverse() * applied).to_scale_rotation_translation();
            local[target].translation = translation;
            local[target].rotation = rotation.normalize();
            last_target = Some(target);
        }
        // SkeletonComponent__UpdateSecondary 0x4E7550: sample at global bone 0 (0x4E1CA0) after the modifiers.
        let root = frame * world_pose(&local, &rig.parents)[0];
        rig.environment = rig.force_scale.map_or(Vec3::ZERO, |scale| wind.sample(root.w_axis.truncate(), scale));
        for h in &rig.hinges {
            if !(if h.equipment { equipment } else { skirt }) { continue; }
            if let Ok(mut transform) = transforms.get_mut(rig.joints[h.target]) { *transform = local[h.target]; }
        }
        for c in &rig.compressions {
            if !skirt { continue; }
            if let Ok(mut transform) = transforms.get_mut(rig.joints[c.target]) { *transform = local[c.target]; }
        }
        for m in &look_at {
            if let Ok(mut transform) = transforms.get_mut(rig.joints[m.target]) { transform.rotation = local[m.target].rotation; }
        }
        for &(target, _) in &rig.copies {
            if !skirt { continue; }
            if let Ok(mut transform) = transforms.get_mut(rig.joints[target]) { transform.rotation = local[target].rotation; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skeleton_force_config_rejects_truncation_and_disabled_configs() {
        let mut data = vec![0u8; 8];
        data.extend(crate::assets::forge::crc32("SkeletonComponent").to_le_bytes());
        data.extend([0u8; 12]);
        data.extend(((-403070396i32) as u32).to_le_bytes());
        data.push(1);
        data.extend(1.0f32.to_le_bytes());
        data.extend([0, 0, 1]);
        assert_eq!(decode_skeleton_force(&data), Some(1.0));
        let p = data.len() - 8;
        data[p] = 0;
        assert_eq!(decode_skeleton_force(&data), None, "a disabled config samples nothing");
        assert_eq!(decode_skeleton_force(&data[..data.len() - 5]), None);
    }
    #[test]
    fn look_at_uses_owner_reference_preserves_position_and_primary_pose() {
        let mut local = vec![Transform::from_xyz(3.0, 1.0, 0.0).with_rotation(Quat::from_rotation_z(0.4)),
            Transform::from_xyz(0.0, 2.0, 0.0), Transform::from_xyz(0.0, 0.5, 0.0).with_rotation(Quat::from_rotation_x(0.7))];
        let parents = [None, Some(0), Some(0)];
        let before = local.clone();
        let old_world = world_pose(&local, &parents);
        look_at_pose(&mut local, &parents, &[SkirtLookAt { equipment: false, target: 2, aim: 1, aim_axis: 2 }]);
        assert_eq!(local[..2], before[..2]);
        assert_eq!(local[2].translation, before[2].translation);
        let world = world_pose(&local, &parents);
        let direction = (world[1].w_axis - world[2].w_axis).truncate().normalize();
        assert!(world[2].z_axis.truncate().distance(direction) < 1e-5);
        assert!(world[2].w_axis.distance(old_world[2].w_axis) < 1e-5);
        let side = old_world[2].z_axis.truncate().cross(direction).normalize();
        assert!(world[2].x_axis.truncate().distance(side) < 1e-5);
        assert!((world[2].determinant() - 1.0).abs() < 1e-5);
    }
    #[test]
    fn look_at_degenerate_directions_retain_preceding_pose() {
        let mut local = vec![Transform::IDENTITY, Transform::IDENTITY, Transform::from_xyz(0.0, 0.0, 1.0)];
        let parents = [None, Some(0), Some(0)];
        let before = local.clone();
        look_at_pose(&mut local, &parents, &[SkirtLookAt { equipment: false, target: 1, aim: 0, aim_axis: 2 }, SkirtLookAt { equipment: false, target: 1, aim: 2, aim_axis: 2 }]);
        assert_eq!(local, before);
    }
    #[test]
    fn hood_look_at_aims_x_and_keeps_a_right_handed_frame() {
        let mut local = vec![Transform::from_xyz(3.0, 1.0, 0.0).with_rotation(Quat::from_rotation_y(0.4)),
            Transform::from_xyz(0.0, 2.0, 0.0), Transform::from_xyz(0.0, 0.5, 0.0).with_rotation(Quat::from_rotation_x(0.7))];
        let parents = [None, Some(0), Some(0)];
        let before = local.clone();
        let old_world = world_pose(&local, &parents);
        look_at_pose(&mut local, &parents, &[SkirtLookAt { equipment: false, target: 2, aim: 1, aim_axis: 0 }]);
        let world = world_pose(&local, &parents);
        let aim = (world[1].w_axis - world[2].w_axis).truncate().normalize();
        let side = old_world[2].z_axis.truncate().cross(aim).normalize();
        assert!(world[2].x_axis.truncate().distance(aim) < 1e-5);
        assert!(world[2].y_axis.truncate().distance(side) < 1e-5);
        assert!(world[2].z_axis.truncate().distance(aim.cross(side)) < 1e-5);
        assert!((world[2].determinant()-1.0).abs() < 1e-5);
        assert!(world[2].w_axis.distance(old_world[2].w_axis) < 1e-5);
        assert_eq!(local[..2], before[..2]);
        assert_eq!(local[2].translation, before[2].translation);
    }
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
