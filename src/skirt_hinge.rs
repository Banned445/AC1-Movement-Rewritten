//! Authored skirt angular dynamics (RE/09 §8.6; 0x654D20, 0x654090).
use bevy::prelude::*;
use crate::assets::ac_formats::parse_skeleton;

#[derive(Clone, Debug)]
pub struct SkirtHinge {
    pub equipment: bool,
    pub target: usize,
    pub force_reference: Option<usize>,
    pub constraint_reference: Option<usize>,
    pub inertia: f32,
    pub damping: f32,
    pub force: Vec3,
    pub environment_strength: f32,
    pub axis: usize,
    pub direction: usize,
    pub min: f32,
    pub max: f32,
    pub soft_min: f32,
    pub soft_max: f32,
    pub rest: Transform,
    pub reference_direction: Vec3,
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
            // PORT: support one authored constraint; multi-constraint intersection remains unported.
            if axis > 2 || direction > 2 || axis == direction || word(r + 12)? != 1 { return None; } r += 16;
            if word(r + 4)? != 3181564545 { return None; } r += 8; // embedded constraint class, 0x654EA8 / 0x438EF0
            let constraint_reference = pointer(&mut r)?;
            let min = scalar(r)?; let max = scalar(r + 4)?;
            let soft_min = scalar(r + 8)?; let soft_max = scalar(r + 12)?;
            if min > max || soft_min < 0.0 || soft_max < 0.0 { return None; } r += 16;
            for k in 0..16 { scalar(r + 4 * k)?; } r += 64;
            let basis = [Vec3::X, Vec3::Y, Vec3::Z];
            let axis_vector = Vec3::new(scalar(r)?, scalar(r + 4)?, scalar(r + 8)?);
            if axis_vector.distance(basis[axis]) > 0.001 || scalar(r + 12)? != 0.0 { return None; }
            let owner = bones.iter().find(|b| b.bone_id as usize == target)?;
            let rest = Transform::from_translation(Vec3::from_array(owner.local_pos)).with_rotation(Quat::from_array(owner.local_rot).normalize());
            // 0x6547D0 rewrites serialized reference frames from rest orientation.
            let owner_direction = Quat::from_array(owner.global_rot).normalize() * basis[direction];
            let reference_direction = constraint_reference.map_or(owner_direction, |id| {
                let b = bones.iter().find(|b| b.bone_id as usize == id).unwrap();
                Quat::from_array(b.global_rot).normalize().inverse() * owner_direction
            });
            Some(SkirtHinge { equipment: false, target, force_reference, constraint_reference, inertia, damping, force, environment_strength,
                axis, direction, min, max, soft_min, soft_max, rest, reference_direction })
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
    pub fn solve(&mut self, h: &SkirtHinge, base: Mat4, owner: Mat4, force: Vec3, reference_direction: Option<Vec3>, environment: Vec3, dt: f32, reset: bool) -> Mat4 {
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
            let mut angle = signed_projected_angle(basis(base, h.direction), direction, axis);
            // Native SSE cross order is direction cross force (0x65437A / 0x6543C2).
            let torque = direction.cross(force).dot(axis);
            self.velocity += direction.cross(inertia + force * step + environment * h.environment_strength).dot(axis);
            if torque * self.velocity < 0.0 { self.velocity *= 1.0 - h.damping; }
            let offset = reference_direction.map_or(0.0, |r| -signed_projected_angle(basis(base, h.direction), r, axis));
            let predicted = self.velocity * step + offset + angle;
            let clamped = predicted.clamp(h.min, h.max);
            self.velocity = constrained_velocity(predicted, self.velocity, h.min, h.max, h.soft_min, h.soft_max);
            angle = clamped - offset;
            solved = base * Mat4::from_quat(Quat::from_axis_angle([Vec3::X, Vec3::Y, Vec3::Z][h.axis], angle));
            self.cached = Some(solved);
        }
        solved
    }
}

fn constrained_velocity(angle: f32, velocity: f32, min: f32, max: f32, soft_min: f32, soft_max: f32) -> f32 {
    // HingeBoneModifier__Update 0x654090: clamp outward velocity, then authored soft-zone ramps.
    if angle < min { return if velocity <= 0.0 { 0.0 } else { velocity }; }
    if angle < min + soft_min { return if velocity < 0.0 { (angle-min)*velocity/soft_min } else { velocity }; }
    if angle > max { return if velocity >= 0.0 { 0.0 } else { velocity }; }
    if angle > max - soft_max && velocity >= 0.0 { return (max-angle)*velocity/soft_max; }
    velocity
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
        SkirtHinge { equipment: false, target: 1, force_reference: None, constraint_reference: None,
            inertia: 0.2, damping: 0.5, force: Vec3::ZERO, environment_strength: 0.0,
            axis: 2, direction: 0, min: -0.4, max: 0.6, soft_min: 0.0, soft_max: 0.0, rest: Transform::IDENTITY, reference_direction: Vec3::X }
    }
    #[test]
    fn soft_limits_damp_only_outward_motion_with_native_branch_order() {
        assert_eq!(constrained_velocity(-0.75, -4.0, -1.0, 1.0, 0.5, 0.5), -2.0);
        assert_eq!(constrained_velocity(-0.75, 4.0, -1.0, 1.0, 0.5, 0.5), 4.0);
        assert_eq!(constrained_velocity(0.75, 4.0, -1.0, 1.0, 0.5, 0.5), 2.0);
        assert_eq!(constrained_velocity(0.75, -4.0, -1.0, 1.0, 0.5, 0.5), -4.0);
        assert_eq!(constrained_velocity(-1.1, -4.0, -1.0, 1.0, 0.0, 0.0), 0.0);
        assert_eq!(constrained_velocity(1.1, 4.0, -1.0, 1.0, 0.0, 0.0), 0.0);
        assert_eq!(constrained_velocity(0.0, 4.0, -1.0, 1.0, 0.0, 0.0), 4.0);
        // Native checks the lower soft zone before the upper bound for overlapping authored zones.
        assert_eq!(constrained_velocity(1.1, 4.0, -1.0, 1.0, 3.0, 0.0), 4.0);
    }
    #[test]
    fn authored_limits_stop_outward_velocity_and_keep_position() {
        let h = hinge();
        let base = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let mut state = HingeState { cached: Some(base), velocity: 100.0 };
        let result = state.solve(&h, base, base, Vec3::ZERO, None, Vec3::ZERO, 1.0 / 60.0, false);
        assert!(result.w_axis.distance(base.w_axis) < 1e-6);
        assert!((signed_projected_angle(Vec3::X, result.x_axis.truncate(), Vec3::Z) - 0.6).abs() < 1e-5);
        assert_eq!(state.velocity, 0.0);
    }
    #[test]
    fn referenced_limit_tracks_the_reference_frame() {
        let h = hinge();
        let mut state = HingeState { cached: Some(Mat4::IDENTITY), velocity: -100.0 };
        let reference = Quat::from_rotation_z(0.3) * Vec3::X;
        let result = state.solve(&h, Mat4::IDENTITY, Mat4::IDENTITY, Vec3::ZERO, Some(reference), Vec3::ZERO, 0.01, false);
        assert!((signed_projected_angle(Vec3::X, result.x_axis.truncate(), Vec3::Z) + 0.1).abs() < 1e-5);
        assert_eq!(state.velocity, 0.0);
    }
    #[test]
    fn reset_pause_and_motion_inertia_are_finite() {
        let h = hinge();
        let mut state = HingeState::default();
        let base = Mat4::IDENTITY;
        state.solve(&h, base, base, Vec3::ZERO, None, Vec3::ZERO, 0.016, true);
        let old = state.cached;
        let paused = state.solve(&h, base, base, Vec3::Y, None, Vec3::ZERO, 0.0, false);
        assert_eq!(paused, base);
        assert_eq!(state.cached, old);
        let moved = Mat4::from_translation(Vec3::Y * 0.02);
        let result = state.solve(&h, moved, moved, Vec3::ZERO, None, Vec3::ZERO, 0.016, false);
        assert!(result.is_finite());
        assert!(state.velocity < 0.0, "cached anchor lags the new position");
        state.solve(&h, base, base, Vec3::ZERO, None, Vec3::ZERO, 0.016, true);
        assert_eq!(state.velocity, 0.0);
    }
}
