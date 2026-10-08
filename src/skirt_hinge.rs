//! Authored skirt angular dynamics (RE/09 §8.6; 0x654D20, 0x654090).
use bevy::prelude::*;
use crate::assets::ac_formats::parse_skeleton;

/// One authored HingeConstraint (HingeConstraint__Read 0x653550; 128 bytes at runtime).
#[derive(Clone, Debug)]
pub struct HingeConstraint {
    pub reference: Option<usize>,
    /// Owner rest direction in the reference bone's rest frame (HingeConstraint__InitReference 0x6547D0).
    pub reference_direction: Vec3,
    pub min: f32,
    pub max: f32,
    pub soft_min: f32,
    pub soft_max: f32,
}

#[derive(Clone, Debug)]
pub struct SkirtHinge {
    pub group: crate::visual_pose::Group,
    pub target: usize,
    pub force_reference: Option<usize>,
    pub inertia: f32,
    pub damping: f32,
    pub force: Vec3,
    pub environment_strength: f32,
    pub axis: usize,
    pub direction: usize,
    pub constraints: Vec<HingeConstraint>,
    pub rest: Transform,
}

#[derive(Clone, Default)]
pub struct HingeState {
    pub cached: Option<Mat4>,
    pub velocity: f32,
}

pub fn decode_hinges(data: &[u8]) -> Result<Vec<SkirtHinge>, String> {
    let bones = parse_skeleton(data);
    let word = |p: usize| data.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let scalar = |p| word(p).map(f32::from_bits).filter(|v| v.is_finite());
    let resolve = |id| bones.iter().find(|b| b.object_id == id);
    let pointer = |p: &mut usize| -> Option<Option<usize>> {
        let kind = *data.get(*p)?; *p += 1;
        match kind {
            3 => Some(None),
            2 => { let id = word(*p)?; *p += 4; Some(Some(resolve(id)?.bone_id as usize)) },
            _ => None,
        }
    };
    let class = crate::assets::forge::crc32("HingeBoneModifier");
    let mut result = Vec::new();
    for p in 5..data.len().saturating_sub(4) {
        if data[p - 5] != 0 || word(p) != Some(class) { continue; }
        let decode = || {
            let mut r = p + 4;
            let target = pointer(&mut r)??;
            if *data.get(r)? != 0 { return None; } r += 1;
            let inertia = scalar(r)?; let damping = scalar(r + 4)?;
            let force = Vec3::new(scalar(r + 8)?, scalar(r + 12)?, scalar(r + 16)?);
            if scalar(r + 20)? != 0.0 || inertia < 0.0 || !(0.0..=1.0).contains(&damping) { return None; } r += 24;
            let force_reference = pointer(&mut r)?;
            let environment_strength = scalar(r)?;
            let axis = word(r + 4)? as usize; let direction = word(r + 8)? as usize;
            let count = word(r + 12)? as usize;
            if axis > 2 || direction > 2 || axis == direction || !(1..=4).contains(&count) { return None; } r += 16;
            let owner = bones.iter().find(|b| b.bone_id as usize == target)?;
            let basis = [Vec3::X, Vec3::Y, Vec3::Z];
            let owner_direction = Quat::from_array(owner.global_rot).normalize() * basis[direction];
            let mut constraints = Vec::with_capacity(count);
            for _ in 0..count {
                if word(r + 4)? != 3181564545 { return None; } r += 8; // embedded constraint class, 0x654EA8 / 0x438EF0
                let reference = pointer(&mut r)?;
                let min = scalar(r)?; let max = scalar(r + 4)?;
                let soft_min = scalar(r + 8)?; let soft_max = scalar(r + 12)?;
                if min > max || soft_min < 0.0 || soft_max < 0.0 { return None; } r += 16;
                for k in 0..16 { scalar(r + 4 * k)?; } r += 64;
                // 0x6547D0 rewrites serialized reference frames from rest orientation.
                let reference_direction = match reference {
                    Some(id) => Quat::from_array(bones.iter().find(|b| b.bone_id as usize == id)?.global_rot).normalize().inverse() * owner_direction,
                    None => owner_direction,
                };
                constraints.push(HingeConstraint { reference, reference_direction, min, max, soft_min, soft_max });
            }
            let axis_vector = Vec3::new(scalar(r)?, scalar(r + 4)?, scalar(r + 8)?);
            if axis_vector.distance(basis[axis]) > 0.001 || scalar(r + 12)? != 0.0 { return None; }
            let rest = Transform::from_translation(Vec3::from_array(owner.local_pos)).with_rotation(Quat::from_array(owner.local_rot).normalize());
            Some(SkirtHinge { group: crate::visual_pose::Group::Skirt, target, force_reference, inertia, damping, force, environment_strength,
                axis, direction, constraints, rest })
        };
        result.push(decode().ok_or("unsupported skirt hinge layout")?);
    }
    Ok(result)
}

fn basis(m: Mat4, axis: usize) -> Vec3 { [m.x_axis, m.y_axis, m.z_axis][axis].truncate() }
fn signed_projected_angle(a: Vec3, b: Vec3, axis: Vec3) -> f32 {
    // 0x653FA2 / 0x653FCB: nearly axis-aligned directions have no measurable angle.
    let a_axis = a.dot(axis);
    let b_axis = b.dot(axis);
    if (a_axis - 1.0).abs() <= 0.0005 || (b_axis - 1.0).abs() <= 0.0005 { return 0.0; }
    let Some(a) = (a - axis * a.dot(axis)).try_normalize() else { return 0.0; };
    let Some(b) = (b - axis * b.dot(axis)).try_normalize() else { return 0.0; };
    // Coordinate-equivalent signed angle to 0x653F00 / 0x55E570.
    let dot = a.dot(b);
    // 0x55E4B8 / 0x55E4CB return endpoints before inspecting the cross-product sign.
    if dot >= 1.0 { return 0.0; }
    if dot <= -1.0 { return std::f32::consts::PI; }
    let angle = dot.acos();
    if a.cross(b).dot(axis) < 0.0 { -angle } else { angle }
}

impl HingeState {
    /// HingeBoneModifier__Update 0x654090. `references` holds each constraint's world reference direction.
    #[allow(clippy::too_many_arguments)]
    pub fn solve(&mut self, h: &SkirtHinge, base: Mat4, owner: Mat4, force: Vec3, references: &[Option<Vec3>], environment: Vec3, dt: f32, reset: bool) -> Mat4 {
        if dt <= 0.0 { return owner; }
        // Native discontinuity path: reset angular velocity, three passes at five times dt.
        let continuous = self.cached.is_some() && !reset;
        if !continuous { self.cached = Some(owner); self.velocity = 0.0; }
        let inertia = if continuous { (self.cached.unwrap().w_axis - base.w_axis).truncate() * (h.inertia / dt) } else { Vec3::ZERO };
        let step = if continuous { dt } else { dt * 5.0 };
        let mut solved = base;
        for _ in 0..if continuous { 1 } else { 3 } {
            let cached = self.cached.unwrap();
            let axis = basis(cached, h.axis);
            let direction = basis(cached, h.direction);
            let base_direction = basis(base, h.direction);
            let mut angle = signed_projected_angle(base_direction, direction, axis);
            // Native SSE cross order is direction cross force (0x65437A / 0x6543C2).
            let torque = direction.cross(force).dot(axis);
            self.velocity += direction.cross(inertia + force * step + environment * h.environment_strength).dot(axis);
            if torque * self.velocity < 0.0 { self.velocity *= 1.0 - h.damping; }
            let offsets: Vec<f32> = (0..h.constraints.len()).map(|k| references.get(k).copied().flatten()
                .map_or(0.0, |r| -signed_projected_angle(base_direction, r, axis))).collect();
            (angle, self.velocity) = apply_constraints(&h.constraints, &offsets, angle, self.velocity, step);
            solved = base * Mat4::from_quat(Quat::from_axis_angle([Vec3::X, Vec3::Y, Vec3::Z][h.axis], angle));
            self.cached = Some(solved);
        }
        solved
    }
}

/// 0x65446C–0x6546F1: each later constraint narrows the previous limits through its own reference offset.
/// Checks run lower hard, lower soft, upper hard, upper soft; crossed limits favour the lower bound.
/// Every constraint adds velocity·dt again to the angle carried from the previous one, as the game does.
fn apply_constraints(constraints: &[HingeConstraint], offsets: &[f32], mut angle: f32, mut velocity: f32, step: f32) -> (f32, f32) {
    let (mut lo, mut hi) = constraints.first().map_or((0.0, 0.0), |c| (c.min, c.max));
    for (k, c) in constraints.iter().enumerate() {
        let offset = offsets[k];
        let mut predicted = velocity * step + offset + angle;
        let (lo_k, hi_k) = if k == 0 { (lo, hi) } else { ((offset + lo).max(c.min), (offset + hi).min(c.max)) };
        let v = velocity;
        if lo_k > predicted {
            predicted = lo_k;
            if v <= 0.0 { velocity = 0.0; }
        } else if c.soft_min + lo_k > predicted {
            if v < 0.0 { velocity = (predicted - lo_k) * v / c.soft_min; }
        } else if predicted > hi_k {
            predicted = hi_k;
            if v >= 0.0 { velocity = 0.0; }
        } else if predicted > hi_k - c.soft_max && v >= 0.0 {
            velocity = (hi_k - predicted) * v / c.soft_max;
        }
        angle = predicted - offset;
        lo = lo_k - offset;
        hi = hi_k - offset;
    }
    (angle, velocity)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projected_angle_ignores_nearly_axis_aligned_directions() {
        let near = Vec3::new(0.01, 0.0, 1.0).normalize();
        assert_eq!(signed_projected_angle(near, Vec3::Y, Vec3::Z), 0.0);
        assert_eq!(signed_projected_angle(Vec3::Y, near, Vec3::Z), 0.0);
        let outside = Vec3::new(0.04, 0.0, 1.0).normalize();
        assert!((signed_projected_angle(outside, Vec3::Y, Vec3::Z) - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert_eq!(signed_projected_angle(Vec3::X, -Vec3::X, Vec3::Z), std::f32::consts::PI);
    }
    fn hinge() -> SkirtHinge {
        SkirtHinge { group: crate::visual_pose::Group::Skirt, target: 1, force_reference: None,
            inertia: 0.2, damping: 0.5, force: Vec3::ZERO, environment_strength: 0.0, axis: 2, direction: 0,
            constraints: vec![HingeConstraint { reference: None, reference_direction: Vec3::X, min: -0.4, max: 0.6, soft_min: 0.0, soft_max: 0.0 }],
            rest: Transform::IDENTITY }
    }
    #[test]
    fn soft_limits_damp_only_outward_motion_with_native_branch_order() {
        let c = |min, max, soft_min, soft_max| vec![HingeConstraint { reference: None, reference_direction: Vec3::X, min, max, soft_min, soft_max }];
        let v = |angle, velocity, soft| apply_constraints(&c(-1.0, 1.0, soft, soft), &[0.0], angle, velocity, 0.0).1;
        assert_eq!(v(-0.75, -4.0, 0.5), -2.0);
        assert_eq!(v(-0.75, 4.0, 0.5), 4.0);
        assert_eq!(v(0.75, 4.0, 0.5), 2.0);
        assert_eq!(v(0.75, -4.0, 0.5), -4.0);
        assert_eq!(v(-1.1, -4.0, 0.0), 0.0);
        assert_eq!(v(1.1, 4.0, 0.0), 0.0);
        assert_eq!(v(0.0, 4.0, 0.0), 4.0);
        // Native checks the lower soft zone before the upper bound for overlapping authored zones.
        assert_eq!(apply_constraints(&c(-1.0, 1.0, 3.0, 0.0), &[0.0], 1.1, 4.0, 0.0).1, 4.0);
    }
    #[test]
    fn second_constraint_intersects_limits_through_its_reference_offset() {
        let mut cs = vec![HingeConstraint { reference: None, reference_direction: Vec3::X, min: -2.0, max: -0.1, soft_min: 0.0, soft_max: 0.0 }];
        cs.push(HingeConstraint { reference: None, reference_direction: Vec3::X, min: -1.0, max: 0.0, soft_min: 0.0, soft_max: 0.0 });
        // Offset 0.5: the first limit [-2, -0.1] becomes [-1.5, 0.4] in the second frame, met with [-1, 0] → [-1, 0].
        let (angle, velocity) = apply_constraints(&cs, &[0.0, 0.5], -1.8, -3.0, 0.0);
        assert!((angle - (-1.5)).abs() < 1e-6, "{angle}");
        assert_eq!(velocity, 0.0);
        let (angle, _) = apply_constraints(&cs, &[0.0, 0.5], -0.05, 0.0, 0.0);
        assert!((angle - (-0.5)).abs() < 1e-6, "{angle}");
    }
    #[test]
    fn authored_limits_stop_outward_velocity_and_keep_position() {
        let h = hinge();
        let base = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let mut state = HingeState { cached: Some(base), velocity: 100.0 };
        let result = state.solve(&h, base, base, Vec3::ZERO, &[None], Vec3::ZERO, 1.0 / 60.0, false);
        assert!(result.w_axis.distance(base.w_axis) < 1e-6);
        assert!((signed_projected_angle(Vec3::X, result.x_axis.truncate(), Vec3::Z) - 0.6).abs() < 1e-5);
        assert_eq!(state.velocity, 0.0);
    }
    #[test]
    fn referenced_limit_tracks_the_reference_frame() {
        let h = hinge();
        let mut state = HingeState { cached: Some(Mat4::IDENTITY), velocity: -100.0 };
        let reference = Quat::from_rotation_z(0.3) * Vec3::X;
        let result = state.solve(&h, Mat4::IDENTITY, Mat4::IDENTITY, Vec3::ZERO, &[Some(reference)], Vec3::ZERO, 0.01, false);
        assert!((signed_projected_angle(Vec3::X, result.x_axis.truncate(), Vec3::Z) + 0.1).abs() < 1e-5);
        assert_eq!(state.velocity, 0.0);
    }
    #[test]
    fn reset_pause_and_motion_inertia_are_finite() {
        let h = hinge();
        let mut state = HingeState::default();
        let base = Mat4::IDENTITY;
        state.solve(&h, base, base, Vec3::ZERO, &[None], Vec3::ZERO, 0.016, true);
        let old = state.cached;
        let paused = state.solve(&h, base, base, Vec3::Y, &[None], Vec3::ZERO, 0.0, false);
        assert_eq!(paused, base);
        assert_eq!(state.cached, old);
        let moved = Mat4::from_translation(Vec3::Y * 0.02);
        let result = state.solve(&h, moved, moved, Vec3::ZERO, &[None], Vec3::ZERO, 0.016, false);
        assert!(result.is_finite());
        assert!(state.velocity < 0.0, "cached anchor lags the new position");
        state.solve(&h, base, base, Vec3::ZERO, &[None], Vec3::ZERO, 0.016, true);
        assert_eq!(state.velocity, 0.0);
    }
}
