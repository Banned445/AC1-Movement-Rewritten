//! Air-catch eligibility (0xE0BB70, timers 0xE01C00; RE/05 §5).
use bevy::prelude::*;
use crate::collision::CollisionWorld;
use super::targets::JumpTarget;

/// Manual ladder search remains height-gated even after the grasp-blend timer expires.
pub fn manual(hand: bool, fall_height: f32, drop_type: Option<usize>) -> bool {
    hand && fall_height > 0.3 && drop_type.is_none_or(|ty| ty == 2)
}

pub fn narrow(fall_height: f32, elapsed: f32, drop: bool) -> bool {
    !drop && fall_height < 9.0 && (fall_height > 0.3 || elapsed >= 0.4)
}

/// 0xE044D0: a clear path, regardless of which side of the root the target lies on.
pub fn target_ahead(root: Vec3, target: Option<JumpTarget>, world: &CollisionWorld) -> bool {
    let Some(target) = target.filter(|t| matches!(t.type_flags, 1 | 0x10000 | 2)) else { return false };
    // PORT: immutable jump targets have no native entity lifetime / cell handle to invalidate.
    let delta = target.position - root;
    let distance = delta.length();
    let slack = if target.type_flags == 0x10000 { 0.0005 } else { 0.0 };
    world.ray_distance(root, delta, distance, crate::layers::CHARACTER) >= distance - slack
}

pub fn automatic(elapsed: f32, drop: bool, target_flags: Option<u32>, facing: Vec3, normals: impl Iterator<Item = Vec3>) -> bool {
    !drop && elapsed >= 0.2 && target_flags.is_some_and(|ty| matches!(ty, 1 | 0x10000 | 0x40 | 0x400))
        // PORT: the proxy's static contacts adapt native type-4 controller reports; this actor is always Human.
        && normals.map(|n| Vec3::new(n.x, 0.0, n.z).normalize_or_zero()).any(|n| facing.dot(n) < -std::f32::consts::FRAC_1_SQRT_2)
}

pub fn catch_blend(speed: f32) -> f32 { (1.0 - (speed / 10.0).min(1.0)) * 0.14 + 0.06 }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn height_timer_and_drop_boundaries() {
        assert!(!manual(true, 0.3, None)); assert!(manual(true, 0.30001, None));
        assert!(!manual(false, 3.0, None)); assert!(!manual(true, 3.0, Some(0)));
        assert!(manual(true, 3.0, Some(2)));
        assert!(!narrow(0.3, 0.399, false)); assert!(narrow(0.3, 0.4, false));
        assert!(narrow(0.30001, 0.0, false)); assert!(!narrow(9.0, 1.0, false));
        assert!(narrow(8.999, 0.0, false)); assert!(!narrow(1.0, 1.0, true));
    }
    #[test]
    fn automatic_needs_target_timer_and_wall_contact() {
        let check = |t, drop, ty, n| automatic(t, drop, ty, Vec3::Z, [n].into_iter());
        assert!(!check(0.199, false, Some(1), Vec3::NEG_Z));
        assert!(check(0.2, false, Some(1), Vec3::NEG_Z));
        assert!(!check(0.2, true, Some(1), Vec3::NEG_Z));
        assert!(!check(0.2, false, Some(2), Vec3::NEG_Z));
        assert!(!check(0.2, false, Some(1), Vec3::X));
        assert!(!check(0.2, false, None, Vec3::NEG_Z));
    }
    #[test]
    fn ladder_blend_clamps_speed() {
        assert!((catch_blend(0.0)-0.2).abs()<1e-6);
        assert!((catch_blend(5.0)-0.13).abs()<1e-6);
        assert!((catch_blend(10.0)-0.06).abs()<1e-6);
        assert!((catch_blend(20.0)-0.06).abs()<1e-6);
    }
}

#[cfg(test)]
mod ray_tests {
    use super::*;
    use crate::collision::Aabb3;
    fn target(position:Vec3,ty:u32)->JumpTarget {JumpTarget{position,type_flags:ty,hang:None,straight:None,pass:None,ladder:None}}
    #[test]
    fn target_guard_is_a_ray_not_a_forward_cone() {
        let mut world=CollisionWorld::default();let root=Vec3::ZERO;
        assert!(target_ahead(root,Some(target(Vec3::NEG_Z*3.0,1)),&world));
        assert!(!target_ahead(root,Some(target(Vec3::Z*3.0,0x40)),&world));
        world.boxes.push(Aabb3{min:Vec3::new(-0.5,-0.5,1.0),max:Vec3::new(0.5,0.5,1.5)});
        assert!(!target_ahead(root,Some(target(Vec3::Z*3.0,1)),&world));
        assert!(target_ahead(root,Some(target(Vec3::Z,1)),&world));
        assert!(!target_ahead(root,Some(target(Vec3::Z*1.0004,1)),&world));
        assert!(target_ahead(root,Some(target(Vec3::Z*1.0004,0x10000)),&world));
        world.layers.push(crate::layers::NOTHING);
        assert!(target_ahead(root,Some(target(Vec3::Z*3.0,1)),&world));
    }
}
