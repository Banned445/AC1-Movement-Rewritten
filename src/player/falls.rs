//! Verified scalar fall rules: 0xE00FE0, 0xEDBAF0, 0xE05940, 0xDFFC90 and 0xE00730.
//! Ragdoll simulation, damage-component configuration and world respawn remain separate dependencies.
use bevy::prelude::*;
use super::air::LandingType;
use crate::input::PadInput;

/// Player air entry enables the fixed table (0xED6280 -> IHumanInAir slot 13 / 0xE102F0).
/// Strict comparisons in 0xE00FE0: equality remains in the lower damage band.
pub fn landing_damage(height: f32) -> (LandingType, u32) {
    if height > 20.0 { (LandingType::Fatal, 200) }
    else if height > 18.0 { (LandingType::HeavyDamage, 160) }
    else if height > 16.0 { (LandingType::HeavyDamage, 120) }
    else if height > 14.0 { (LandingType::HeavyDamage, 80) }
    else if height > 12.0 { (LandingType::HeavyDamage, 40) }
    else if height > 10.0 { (LandingType::SmallDamage, 20) }
    else if height > 8.0 { (LandingType::SmallDamage, 10) }
    else { (LandingType::Safe, 0) }
}

/// GoAssassinActionInterpreter air update 0xEDBAF0 -> slot 12 / 0xE102B0, every frame.
/// PORT: the movement-only pad has no ability-stack or input-capture ownership gates.
pub fn speed_ratio(pad: &PadInput, facing: Vec3) -> f32 {
    if pad.magnitude > 0.35 && pad.dir.dot(facing) < (std::f32::consts::PI / 3.0).cos() { return 0.0; }
    match (pad.high_profile, pad.magnitude > 0.5, pad.legs_held) {
        (false, false, _) => 0.09,
        (false, true, _) => 0.49,
        (true, false, _) => 0.19,
        (true, true, false) => 0.89,
        (true, true, true) => 1.0,
    }
}

/// 0xE05940 at 0xE05B64: straight when nearly stationary/vertical or descending >75 degrees.
pub fn forward_landing(velocity: Vec3) -> bool {
    if velocity.abs().max_element() <= 0.001 { return false; }
    let horizontal = Vec3::new(velocity.x, 0.0, velocity.z);
    if horizontal.abs().max_element() <= 0.0005 { return false; }
    let downward = Vec3::new(velocity.x, velocity.y.min(0.0), velocity.z);
    horizontal.normalize().dot(downward.normalize()) >= 75f32.to_radians().cos()
}

/// Ballistic__TimeToReachHeight 0xDFFC90 (up-positive velocity, downward target).
pub fn time_to_height(y: f32, target_y: f32, vy: f32) -> f32 {
    (vy + (vy * vy - 19.6 * (target_y - y).min(0.0)).sqrt()) / 9.8
}

/// Ballistic__SolveCorrectionAccel 0xE00730, expressed in the port's metres/second velocity units.
pub fn correction(position: Vec3, target: Vec3, velocity: Vec3, remaining: f32, dt: f32) -> Vec3 {
    2.0 * (target - position - velocity * remaining) * (dt / (remaining * remaining))
}

/// 0xE0F643-0xE0F707, no-contact branch: clamp >5 immediately; otherwise subtract 4*dt.
pub fn drift(horizontal: Vec3, dt: f32, has_contacts: bool) -> Vec3 {
    let speed = horizontal.length();
    if speed <= 0.0005 { return horizontal; }
    let next = if speed > 5.0 { 5.0 } else if has_contacts { speed } else { (speed - 4.0 * dt).max(0.0) };
    horizontal * (next / speed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_table_boundaries_are_strict() {
        let limits = [8.0f32, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0];
        let bands = [(LandingType::Safe,0), (LandingType::SmallDamage,10), (LandingType::SmallDamage,20),
            (LandingType::HeavyDamage,40), (LandingType::HeavyDamage,80), (LandingType::HeavyDamage,120),
            (LandingType::HeavyDamage,160), (LandingType::Fatal,200)];
        for (i, h) in limits.into_iter().enumerate() {
            assert_eq!(landing_damage(h.next_down()), bands[i]);
            assert_eq!(landing_damage(h), bands[i]);
            assert_eq!(landing_damage(h.next_up()), bands[i+1]);
        }
    }
    #[test]
    fn player_air_speed_bands_and_angle_gate() {
        let mut p = PadInput { dir: Vec3::NEG_Z, magnitude: 0.5, ..default() };
        assert_eq!(speed_ratio(&p, Vec3::NEG_Z), 0.09);
        p.magnitude = 0.5f32.next_up(); assert_eq!(speed_ratio(&p, Vec3::NEG_Z), 0.49);
        p.high_profile = true; assert_eq!(speed_ratio(&p, Vec3::NEG_Z), 0.89);
        p.legs_held = true; assert_eq!(speed_ratio(&p, Vec3::NEG_Z), 1.0);
        p.magnitude = 0.5; assert_eq!(speed_ratio(&p, Vec3::NEG_Z), 0.19);
        p.dir = Vec3::Z; assert_eq!(speed_ratio(&p, Vec3::NEG_Z), 0.0);
        p.magnitude = 0.35; assert_eq!(speed_ratio(&p, Vec3::NEG_Z), 0.19);
    }
    #[test]
    fn landing_trajectory_pitch_selects_forward_or_straight() {
        assert!(!forward_landing(Vec3::ZERO));
        assert!(!forward_landing(Vec3::new(0.0005,-10.0,0.0)));
        assert!(forward_landing(Vec3::new(5.0,-5.0,0.0)));
        assert!(!forward_landing(Vec3::new(1.0,-10.0,0.0)));
        assert!(forward_landing(Vec3::new(1.0,10.0,0.0)));
        let t = 75f32.to_radians();
        assert!(forward_landing(Vec3::new(t.cos(),-t.sin(),0.0)));
        assert!(!forward_landing(Vec3::new((t+0.001).cos(),-(t+0.001).sin(),0.0)));
    }
    #[test]
    fn falling_time_includes_existing_vertical_velocity() {
        for vy in [-12.0,0.0,4.0] {
            let t = time_to_height(10.0,0.0,vy);
            assert!((10.0 + vy*t - 4.9*t*t).abs() < 0.0001);
        }
        assert_eq!(time_to_height(0.0,1.0,-2.0),0.0);
    }
    #[test]
    fn steering_is_acceleration_correction_and_drift_caps_immediately() {
        assert_eq!(correction(Vec3::ZERO,Vec3::X*4.0,Vec3::X,2.0,0.1),Vec3::X*0.1);
        assert!((drift(Vec3::X*6.3,1.0/60.0,false).length()-5.0).abs()<1e-6);
        assert!((drift(Vec3::X*5.0,0.1,false).length()-4.6).abs()<1e-6);
        assert_eq!(drift(Vec3::X*0.1,0.1,false),Vec3::ZERO);
        assert_eq!(drift(Vec3::X*4.0,0.1,true),Vec3::X*4.0);
        assert_eq!(drift(Vec3::X*6.0,0.1,true),Vec3::X*5.0);
    }
}
