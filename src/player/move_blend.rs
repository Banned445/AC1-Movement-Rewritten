//! Ground locomotion blend — `HumanGround__UpdateMoveBlend` 0xDA0810 (RE/02 §4.1).
//!
//! The game does not move the character at a "speed per band". It moves it by the root motion of the
//! ground locomotion action `0x05923BDB`. That action has two items, one per leading foot, and each item
//! blends 17 clips. MoveBlend sets the 17 weights every frame from:
//! - the speed parameter (HG+0x5E8);
//! - the hip-lean angle (Data+0xD4, crowd avoidance) in the walk band;
//! - the bank angle (Data+0xD8, desired vs current heading) from jog upwards;
//! - the jog "slowdown" timer (HG+0x6CC) and the sprint "settle" weight (HG+0x6D0).

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

/// The ground locomotion action (exe compares the playing action against 93469659 = 0x05923BDB).
pub const ACT_GROUND_LOCOMOTION: u32 = 0x0592_3BDB;

/// Slot order of both items of action 0x05923BDB (RE/data/action_graph_movement.txt).
pub const SLOT_SLOWWALK: u8 = 0; // +1 hipl, +2 hipr
pub const SLOT_WALK: u8 = 3; // +1 hipl, +2 hipr
pub const SLOT_JOG: u8 = 6; // +1 bank left, +2 bank right
pub const SLOT_JOG_SLOWDOWN: usize = 9;
pub const SLOT_RUN: u8 = 10; // +1 bank left, +2 bank right
pub const SLOT_SPRINT: u8 = 13; // +1/+2 = the run bank clips again
pub const SLOT_SPRINT_IMPULSION: usize = 16;

/// One step of each slot's clip: (root displacement m, duration s), [footl item, footr item]. MEASURED
/// from the game's clips (DISPLACEMENT end value and duration, RE/data/anim_root_motion_gamefix.txt). Every
/// locomotion clip has a 2-key displacement track, so its speed is constant inside the step.
pub const SLOT_STEP: [[(f32, f32); 17]; 2] = [
    [
        (0.170, 1.6667), (0.269, 2.0), (0.269, 2.0), // xx_l_walk_slow_hip{m,l,r}_footl
        (1.012, 0.5333), (0.992, 0.6), (0.992, 0.6), // xx_l_walk_hip{m,l,r}_footl
        (1.651, 0.4667), (1.707, 0.4667), (1.707, 0.4667), (1.651, 0.4667), // xx_h_jog_{hipm,bank_left,bank_right,slowdown}_footl
        (1.707, 0.3333), (1.707, 0.3333), (1.707, 0.3333), // xx_h_run_{hipm,bank_left,bank_right}_footl
        (1.674, 0.2667), (1.707, 0.3333), (1.707, 0.3333), // xx_h_sprint_hipm, xx_h_run_bank_{left,right}
        (1.674, 0.2667), // xx_h_sprint_impultion_footl
    ],
    [
        (0.150, 1.4667), (0.269, 2.0), (0.269, 2.0),
        (0.885, 0.4667), (0.992, 0.6), (0.992, 0.6),
        (1.651, 0.4667), (1.707, 0.4667), (1.707, 0.4667), (1.651, 0.4667),
        (1.707, 0.3333), (1.707, 0.3333), (1.707, 0.3333),
        (1.674, 0.2667), (1.707, 0.3333), (1.707, 0.3333),
        (1.674, 0.2667),
    ],
];

/// Deceleration curve (`scimitar::ResponseCurve` at HG+0x63C): speed parameter → fall rate per second.
/// Keys added in `HumanGround__OnEnterInit` 0xDA7E62..0xDA7ED0.
pub const DECEL_CURVE: [(f32, f32); 5] = [(0.0, 1.0), (0.333, 1.0), (0.4, 0.3), (0.666, 0.2), (1.0, 1.0)];

/// `ResponseCurve__Evaluate` 0x5631D0: piecewise linear. Every segment that contains `x` is evaluated in
/// key order and the last one wins; outside the keys the result is 0.
pub fn response_curve(keys: &[(f32, f32)], x: f32) -> f32 {
    let mut out = 0.0;
    for w in keys.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x >= x0 && x1 >= x {
            let (dx, dy) = (x1 - x0, y1 - y0);
            let slope = if dx == 0.0 { if dy == 0.0 { 0.0 } else { f32::MAX } } else { dy / dx };
            out = (x - x0) * slope + y0;
        }
    }
    out
}

fn wrap(a: f32) -> f32 {
    let mut d = (a + PI).rem_euclid(TAU) - PI;
    if d <= -PI {
        d += TAU;
    }
    d
}

/// `Math__LerpHeadingAngle` 0xD4C390: lerp between two directions in angle space along the short way.
fn lerp_heading(from: f32, to: f32, k: f32) -> f32 {
    from + wrap(to - from) * k
}

/// `HumanGround__UpdateLeanAngle` 0xD94C80, in the port's heading convention (heading grows to the left;
/// a positive angle means the filtered direction is to the right of the body, which selects the right
/// clips). The filtered direction is `heading − angle`. With a target it moves towards the target by the
/// fraction 5·dt; without one (zero vector, |v| ≤ 0.001) it moves back to the body heading by **0.1 per
/// update** (a per-frame constant in the exe). The result is clamped to ±`max`.
pub fn update_lean_angle(angle: f32, heading: f32, target: Option<f32>, max: f32, dt: f32) -> f32 {
    let filtered = heading - angle;
    let filtered = match target {
        Some(t) => lerp_heading(filtered, t, dt * 5.0),
        None => lerp_heading(filtered, heading, 0.1),
    };
    wrap(heading - filtered).clamp(-max, max)
}

/// MoveBlend runtime (the HumanGround fields it owns).
#[derive(Clone, Debug)]
pub struct MoveBlend {
    /// HG+0x5E8.
    pub speed_param: f32,
    /// HG+0x6CC: rises at 1/s while decelerating; falls 8/s while accelerating and 4/s when steady.
    /// Weight of `xx_h_jog_slowdown` against `xx_h_jog_hipm`.
    pub slowdown: f32,
    /// HG+0x6D0: 1 below the sprint band; in the sprint band it falls at 1/s once the band is full
    /// (fraction > 0.99) or the speed is above its target. Weight of `sprint_impultion` against `sprint_hipm`.
    pub settle: f32,
    /// Data+0xD4: hip lean (crowd avoidance), ±π/2.
    pub lean: f32,
    /// Data+0xD8: bank, ±π/2 (±π/4 when IHumanGroundAccess vt1120 says so — "careful" mode, not traced).
    pub bank: f32,
    /// Which item of the action is playing: 0 = footl, 1 = footr. PORT: the game alternates the items in
    /// the animation player; the port runs the step cycle here so the sim knows the root motion.
    pub foot: usize,
    /// Normalised phase in the current step.
    pub phase: f32,
    pub weights: [f32; 17],
}

impl Default for MoveBlend {
    fn default() -> Self {
        Self { speed_param: 0.0, slowdown: 0.0, settle: 0.0, lean: 0.0, bank: 0.0, foot: 0, phase: 0.0, weights: [0.0; 17] }
    }
}

impl MoveBlend {
    /// Speed parameter step (0xDA1700–0xDA18A2, script-forced speed HG+0x6C8 not modelled).
    pub fn update_speed(&mut self, target: f32, dt: f32) {
        let target = target.min(1.0);
        let s = self.speed_param;
        if (target - s).abs() <= 0.000_5 {
            self.slowdown = (self.slowdown - dt * 4.0).max(0.0);
        } else if s <= target {
            self.speed_param = (s + dt).min(target);
            self.slowdown = (self.slowdown - dt * 8.0).max(0.0);
        } else {
            let rate = response_curve(&DECEL_CURVE, s);
            self.speed_param = (s - rate * dt).max(target);
            self.slowdown = (self.slowdown + dt).min(1.0);
        }
    }

    /// Lean and bank (0xDA1A3F–0xDA1C2A). `heading` is this frame's body heading snapshot (HG+0x600);
    /// `dest` the wanted heading (DestHeading Data+0x50) when there is one; `avoid` the crowd-avoid
    /// direction (Data+0x60), which the port does not produce yet.
    pub fn update_angles(&mut self, heading: f32, dest: Option<f32>, avoid: Option<f32>, careful: bool, dt: f32) {
        self.lean = update_lean_angle(self.lean, heading, avoid, FRAC_PI_2, dt);
        self.bank = update_lean_angle(self.bank, heading, dest, if careful { FRAC_PI_4 } else { FRAC_PI_2 }, dt);
    }

    /// The 17 weights (0xDA18C0–0xDA1FC3). `target` is this frame's target speed parameter.
    pub fn update_weights(&mut self, target: f32, dt: f32) {
        let s = self.speed_param;
        // (upper slot, lower slot) of the band; bands are (0,0.25] (0.25,0.5] (0.5,0.75] (0.75,1]
        let (lower, upper, f) = if s > 0.25 {
            if s > 0.5 {
                if s > 0.75 {
                    (SLOT_RUN, SLOT_SPRINT, (s - 0.75) * 4.0)
                } else {
                    self.settle = 1.0;
                    (SLOT_JOG, SLOT_RUN, (s - 0.5) * 4.0)
                }
            } else {
                self.settle = 1.0;
                (SLOT_WALK, SLOT_JOG, (s - 0.25) * 4.0)
            }
        } else {
            // also calls sub_DC3FA0(1 − f, f) (not traced)
            self.slowdown = 0.0;
            self.settle = 1.0;
            (SLOT_SLOWWALK, SLOT_WALK, s * 4.0)
        };
        let f = f.min(1.0);
        let lean_t = (self.lean.abs() / FRAC_PI_2).min(1.0);
        let bank_t = (self.bank.abs() / FRAC_PI_2).min(1.0);
        let side = |a: f32| if a < 0.0 { 1 } else { 2 };
        // side weight of the lower and the upper clip, and which side clip
        let (lower_side, upper_side, lower_k, upper_k) = if s <= 0.25 {
            (lean_t, lean_t, side(self.lean), side(self.lean))
        } else if s > 0.5 {
            (bank_t, bank_t, side(self.bank), side(self.bank))
        } else {
            // walk → jog: the walk keeps the hip lean, the jog takes the bank
            ((1.0 - f) * lean_t, f * bank_t, side(self.lean), side(self.bank))
        };
        let total_side = if s > 0.25 && s <= 0.5 { lower_side + upper_side } else { lower_side };
        let straight = 1.0 - total_side;

        let mut w = [0.0f32; 17];
        let (lower_mid, upper_mid);
        if s <= 0.5 {
            // jog slowdown shares the upper (jog) weight; in the walk band the timer is 0
            w[SLOT_JOG_SLOWDOWN] = self.slowdown * f * straight;
            lower_mid = (1.0 - f) * straight;
            upper_mid = (1.0 - self.slowdown) * f * straight;
        } else if s > 0.75 {
            if f > 0.99 || s > target {
                self.settle = (self.settle - dt).max(0.0);
            }
            w[SLOT_SPRINT_IMPULSION] = self.settle * f * straight;
            lower_mid = (1.0 - f) * straight;
            upper_mid = (1.0 - self.settle) * f * straight;
        } else {
            // run band: the slowdown clip shares the lower (jog) weight
            w[SLOT_JOG_SLOWDOWN] = self.slowdown * (1.0 - f) * straight;
            lower_mid = (1.0 - self.slowdown) * (1.0 - f) * straight;
            upper_mid = f * straight;
        }
        w[lower as usize] = lower_mid;
        w[upper as usize] = upper_mid;
        w[(lower + lower_k) as usize] = lower_side * (1.0 - f);
        w[(upper + upper_k) as usize] = upper_side * f;
        self.weights = w;
    }

    /// Advance the step cycle and return the root speed (m/s) of the blended clips. The item's clips share
    /// one normalised clock: the step lasts Σwᵢ·Tᵢ (0x507650 → 0x5B9480) and the root covers the blended
    /// displacement Σwᵢ·dᵢ (0x508260).
    pub fn advance(&mut self, dt: f32) -> f32 {
        let step = &SLOT_STEP[self.foot];
        let (mut d, mut t, mut wsum) = (0.0, 0.0, 0.0);
        for (k, w) in self.weights.iter().enumerate() {
            d += w * step[k].0;
            t += w * step[k].1;
            wsum += w;
        }
        // the item's clock runs over Σwᵢ·Tᵢ, not normalised (0x507650 → 0x5B9480; 0x507880 only rejects
        // an all-zero weight set)
        if t <= 1e-4 || wsum.abs() <= 0.000_5 {
            return 0.0;
        }
        self.phase += dt / t;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            self.foot ^= 1;
        }
        d / t
    }

    /// Steady straight-line root speed for a speed parameter (no lean/bank, timers settled).
    pub fn steady_speed(param: f32, foot: usize) -> f32 {
        let mut m = MoveBlend { speed_param: param, ..Default::default() };
        m.update_weights(param, 0.0);
        if param > 0.75 {
            m.settle = 0.0;
            m.update_weights(param, 0.0);
        }
        m.foot = foot;
        m.advance(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decel_curve_matches_keys() {
        assert_eq!(response_curve(&DECEL_CURVE, 0.0), 1.0);
        assert_eq!(response_curve(&DECEL_CURVE, 0.2), 1.0);
        assert!((response_curve(&DECEL_CURVE, 0.4) - 0.3).abs() < 1e-6);
        assert!((response_curve(&DECEL_CURVE, 0.533) - 0.25).abs() < 1e-3);
        assert!((response_curve(&DECEL_CURVE, 1.0) - 1.0).abs() < 1e-6);
        assert_eq!(response_curve(&DECEL_CURVE, 1.2), 0.0);
    }

    #[test]
    fn weights_sum_to_one_except_walk_to_jog_sides() {
        let mut m = MoveBlend::default();
        for i in 0..=100 {
            m.speed_param = i as f32 / 100.0;
            m.lean = 0.4;
            m.bank = -0.7;
            m.slowdown = 0.3;
            m.update_weights(m.speed_param, 0.0);
            let sum: f32 = m.weights.iter().sum();
            let s = m.speed_param;
            if s > 0.25 && s <= 0.5 {
                // the exe scales the side weights by the band fraction twice in this band
                let f = (s - 0.25) * 4.0;
                let (l, b) = (0.4 / FRAC_PI_2, 0.7 / FRAC_PI_2);
                let side = (1.0 - f) * l + f * b;
                let want = 1.0 - side + (1.0 - f) * (1.0 - f) * l + f * f * b;
                assert!((sum - want).abs() < 1e-5, "param {s} sum {sum} want {want}");
            } else {
                assert!((sum - 1.0).abs() < 1e-5, "param {s} sum {sum}");
            }
        }
    }

    #[test]
    fn band_tops_play_the_pure_clips() {
        for (p, slot) in [(0.25, 3), (0.5, 6), (0.75, 10)] {
            let mut m = MoveBlend { speed_param: p, ..Default::default() };
            m.update_weights(p, 0.0);
            assert!((m.weights[slot] - 1.0).abs() < 1e-6, "param {p}: {:?}", m.weights);
        }
        // full sprint: impulsion first, then the settle weight hands over to sprint_hipm at 1/s
        // (arriving from the run band, where the settle weight is held at 1)
        let mut m = MoveBlend { speed_param: 1.0, settle: 1.0, ..Default::default() };
        m.update_weights(1.0, 0.0);
        assert!((m.weights[SLOT_SPRINT_IMPULSION] - 1.0).abs() < 1e-6);
        for _ in 0..30 {
            m.update_weights(1.0, 1.0 / 30.0);
        }
        assert!(m.weights[SLOT_SPRINT as usize] > 0.99);
    }

    #[test]
    fn measured_speeds_at_band_tops() {
        let v = |p| MoveBlend::steady_speed(p, 0);
        assert!((v(0.25) - 1.898).abs() < 0.01);
        assert!((v(0.5) - 3.538).abs() < 0.01);
        assert!((v(0.75) - 5.121).abs() < 0.01);
        assert!((v(1.0) - 6.277).abs() < 0.01);
        // between bands the synchronised blend is slower than a linear speed mix
        let mid = v(0.625);
        assert!(mid > 3.9 && mid < 4.33, "{mid}");
    }

    #[test]
    fn decelerating_from_sprint_follows_the_curve() {
        let mut m = MoveBlend { speed_param: 1.0, ..Default::default() };
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        while m.speed_param > 0.5 {
            m.update_speed(0.5, dt);
            t += dt;
        }
        // 1.0 → 0.666 at a rate falling 1 → 0.2 (∫ds/rate = 0.334/0.8·ln 5 = 0.672 s), then
        // 0.666 → 0.5 at 0.2..0.262 per second (≈ 0.72 s)
        assert!(t > 1.33 && t < 1.43, "{t}");
        assert!(m.slowdown > 0.9);
    }

    #[test]
    fn bank_follows_the_wanted_heading() {
        // wanted heading to the right of the body (heading grows to the left)
        let mut b = 0.0;
        for _ in 0..60 {
            b = update_lean_angle(b, 0.0, Some(-0.6), FRAC_PI_2, 1.0 / 60.0);
        }
        assert!((b - 0.6).abs() < 0.01, "{b}");
        // no target: back towards the body by 0.1 per update
        let b2 = update_lean_angle(b, 0.0, None, FRAC_PI_2, 1.0 / 60.0);
        assert!((b2 - b * 0.9).abs() < 1e-4);
    }
}
