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

/// `HumanInAir__AntiStuckNudge` 0xE0B1E0, the tail of the fall updates (`UpdateDropMotion` 0xE0DCD0 after every
/// landing check failed): HumanInAir +408 (the timer), +404 (the nudges so far, cleared on entry by 0xE01C00) and
/// InAirData +528 (`gate`, the deadline it arms; while it runs `GetGroundContactType` 0xE038A0 ignores contacts within
/// 0.3 m below the start).
#[derive(Clone, Copy, Debug, Default)]
pub struct AntiStuck {
    pub timer: f32,
    pub nudges: u8,
    pub gate: f32,
    /// PORT: the game draws from the CRT `rand` (`sub_430830`); the port's draws are a per-entry xorshift.
    pub seed: u32,
}

impl AntiStuck {
    pub fn reset(&mut self, seed: u32) {
        *self = AntiStuck { seed: seed | 1, ..default() };
    }

    /// The gate's countdown (once per frame, before the landing checks).
    pub fn tick(&mut self, dt: f32) {
        self.gate = (self.gate - dt).max(0.0);
    }

    /// `GetGroundContactType`'s gate: contacts do not count while the gate runs and the body is within 0.3 m below
    /// the start (InAirData +64).
    pub fn ignores_contacts(&self, start_y: f32, y: f32) -> bool {
        self.gate > 0.0 && start_y - y <= 0.3
    }

    fn rand(&mut self) -> f32 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// A random push at 3 m/s: x and z drawn in [-1, 1], `up` up (0.5, or 1 when the four spots have no room),
    /// normalised (the extra random turn about up, `sub_55E390` with an angle in [-pi, pi], keeps the distribution).
    fn random(&mut self, up: f32) -> Vec3 {
        let (x, z) = (self.rand(), self.rand());
        Vec3::new(x, up, z).normalize() * 3.0
    }

    fn fire(&mut self, v: Vec3) -> Option<Vec3> {
        self.gate = 0.5;
        self.nudges = self.nudges.saturating_add(1);
        Some(v)
    }

    /// One frame. `velocity` is the controller's (the body's real motion), `to_target` InAirData +505 (a jump
    /// target), `fits` the standing-capsule test of `sub_E0B050` (0.35 m radius, 1.2 m tall, 0x19BA428 / 0x19BA42C).
    /// Returns the velocity the nudge sets.
    pub fn update(&mut self, dt: f32, velocity: Vec3, to_target: bool, has_contacts: bool, feet: Vec3, forward: Vec3, fits: impl Fn(Vec3) -> bool) -> Option<Vec3> {
        if self.gate > 0.0 {
            self.timer = 0.0;
            return None;
        }
        if velocity.length() >= 0.25 {
            // moving, but sideways on a contact for 0.5 s (no target, |vertical| < 0.25): a random push
            if !to_target && velocity.y.abs() < 0.25 && has_contacts {
                self.timer += dt;
                if self.timer >= 0.5 {
                    self.timer = 0.0;
                    let v = self.random(0.5);
                    return self.fire(v);
                }
                return None;
            }
            self.timer = 0.0;
            return None;
        }
        // held still for 0.5 s
        self.timer += dt;
        if self.timer < 0.5 {
            return None;
        }
        self.timer = 0.0;
        if self.nudges >= 2 {
            let v = self.random(0.5);
            return self.fire(v);
        }
        // the four spots 0.5 m round the feet + 0.15 m: ahead, left, right, behind; the first with room is moved
        // toward at its offset per second
        let f = Vec3::new(forward.x, 0.0, forward.z).normalize_or(Vec3::NEG_Z);
        let r = super::right_of(f);
        let base = Vec3::Y * 0.15;
        for off in [f * 0.5, -r * 0.5, r * 0.5, -f * 0.5] {
            if fits(feet + base + off) {
                return self.fire(base + off);
            }
        }
        let v = self.random(1.0);
        self.fire(v)
    }
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

    #[test]
    fn anti_stuck_tries_the_four_spots_then_pushes_at_random() {
        let mut a = AntiStuck::default();
        a.reset(7);
        let f = Vec3::NEG_Z;
        // held still: nothing before 0.5 s, then the spot ahead (it has room)
        assert!(a.update(0.3, Vec3::ZERO, false, true, Vec3::ZERO, f, |_| true).is_none());
        let v = a.update(0.25, Vec3::ZERO, false, true, Vec3::ZERO, f, |_| true).unwrap();
        assert!((v - Vec3::new(0.0, 0.15, -0.5)).length() < 1e-5, "{v:?}");
        // the gate holds for 0.5 s and keeps contacts within 0.3 m of the start from counting
        assert!(a.ignores_contacts(1.0, 0.8) && !a.ignores_contacts(1.0, 0.6));
        assert!(a.update(0.6, Vec3::ZERO, false, true, Vec3::ZERO, f, |_| true).is_none());
        a.tick(0.6);
        // no room ahead or to the left: the right
        let r = super::super::right_of(f);
        let v = a.update(0.6, Vec3::ZERO, false, true, Vec3::ZERO, f, |p| p.dot(r) > 0.4).unwrap();
        assert!((v - (r * 0.5 + Vec3::Y * 0.15)).length() < 1e-5, "{v:?}");
        a.tick(0.6);
        // the third nudge is random, 3 m/s
        let v = a.update(0.6, Vec3::ZERO, false, true, Vec3::ZERO, f, |_| true).unwrap();
        assert!((v.length() - 3.0).abs() < 1e-4 && v.y > 0.0, "{v:?}");
        // sliding sideways on a contact for 0.5 s: random too
        let mut b = AntiStuck::default();
        b.reset(3);
        assert!(b.update(0.6, Vec3::new(1.0, 0.0, 0.0), false, true, Vec3::ZERO, f, |_| true).is_some_and(|v| (v.length() - 3.0).abs() < 1e-4));
        // not when flying to a target or falling
        b.reset(3);
        assert!(b.update(0.6, Vec3::new(1.0, 0.0, 0.0), true, true, Vec3::ZERO, f, |_| true).is_none());
        assert!(b.update(0.6, Vec3::new(1.0, -2.0, 0.0), false, true, Vec3::ZERO, f, |_| true).is_none());
    }
}
