//! The action interpreter's own per-frame work (`GoAssassinActionInterpreter`, RE/01 §6, RE/24 INPUT-2 / 3 / 4).
//!
//! `GoAssassinActionInterpreter__ReadInput` 0xEEDE60 turns the pad into the values the contexts read, and the
//! interpreter's state machine follows the locomotion context (`sub_EE1850` → the state enter functions
//! 0xED8140…0xEDAE70), with side effects on entry. The port's contexts read `PadInput` directly; this system runs
//! first in the player chain and fills the interpreter-derived fields of `PadInput`:
//! - the latched actor-frame stick for the ledge, climb and ladder (+0x1300 / +0x1310, 0xEEE3E5–0xEEE483);
//! - the empty-hand buffer (+0x1129, 0xEEE0CC–0xEEE149) with the 0.2 s block after grabbing (+0x1180);
//! - the stick tracker's two directions, snapped and response-curved (`sub_55E7E0`), and its magnitude bands;
//! - the ground's quick-reversal flag (+0x112C → HG+1780, `ProcessGroundMovement` 0xEE67FE–0xEE691E).

use bevy::prelude::*;

use super::{ActorContextId, Body, HumanDataBundle, Locomotion, Player};
use crate::camera::CameraRig;
use crate::input::PadInput;
use crate::tuning::{JUMP_BUFFER, STICK_DEADZONE};

/// `sub_EDB450` 0xEDB5C5: the stick's x is mirrored when dot(actor forward, flat(actor − camera)) is below this
/// (−0.17364818 = −sin 10°: the camera more than 100° round from behind).
const MIRROR_DOT: f32 = -0.173_648_18;
/// +0x1324: the latch is kept until the stick moves this far from the latched value (0x3E99999A = 0.3); on a ladder
/// 0x3FB33333 = 1.4 (`sub_ED82D0` 0xED8390), restored on every exit from the ladder (e.g. 0xED8245).
const LATCH_MOVE: f32 = 0.3;
const LATCH_MOVE_LADDER: f32 = 1.4;
/// The empty hand is ignored for this long after entering the Ladder / Ledge / Climb states (+0x1178 / +0x1180,
/// `sub_ED82D0` 0xED8398, `sub_ED8610` 0xED86D6, `sub_ED86F0` 0xED87A8; read at 0xEEE12C–0xEEE149).
const HAND_BLOCK: f32 = 0.2;
/// The interpreter's magnitude bands (`sub_EE1710` 0xEE181F: dword_17085D4 = 0x3EB33333, dword_17085D8 = 0x3F666666,
/// 1.0): the band is the first entry at or above |stick| (`sub_55EAE0` 0x55ED93).
const BANDS: [f32; 3] = [0.35, 0.9, 1.0];
/// The tracker's forward snap: a stick whose y (normalised) is above cos 5° reads straight ahead (tracker +0xA8,
/// `sub_55EDF0` 0x55EF09: 0x3F7F069E).
const SNAP_COS: f32 = 0.996_194_7;
/// The left stick's ResponseCurve (tracker +0x600, keys added at 0x55EF26–0x55EFD0; x = |angle|/π, y = radians,
/// evaluated piecewise linearly by `ResponseCurve__Evaluate` 0x5631D0): flt_168BA40/44, 38/3C, 30/34, 28/2C, 20/24.
const CURVE: [(f32, f32); 7] = [
    (0.0, 0.0),
    (0.027_777_78, 0.017_453_292),
    (0.055_555_56, 0.087_266_46),
    (0.111_111_11, 0.174_532_93),
    (0.166_666_67, 0.349_065_86),
    (0.222_222_22, 0.698_131_7),
    (1.0, std::f32::consts::PI),
];
/// The quick reversal (`ProcessGroundMovement` 0xEE68C3–0xEE6907): the stick entering a band more than 120° from the
/// facing (dword_16D908C = 2.0943952) raises +0x112C for 0.3 s (dword_1683460).
const QUICK_TURN_ANGLE: f32 = 2.094_395_2;
const QUICK_TURN_TIME: f32 = 0.3;

/// The interpreter's own state (GoAssassinActionInterpreter fields).
#[derive(Component, Debug)]
pub struct Interpreter {
    /// +0x1320: the stick is latched.
    pub latch_on: bool,
    /// +0x1324: how far the stick may move before it is latched again.
    pub latch_move: f32,
    /// The context the interpreter's state machine last followed (`sub_EE1850`).
    pub ctx: Option<ActorContextId>,
    /// Seconds left of the empty-hand block (+0x1180).
    pub hand_block: f32,
    /// +0x112C: the quick-reversal flag, and its timer (+0xFA8, seconds left).
    pub quick_turn: bool,
    pub quick_timer: f32,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self { latch_on: false, latch_move: LATCH_MOVE, ctx: None, hand_block: 0.0, quick_turn: false, quick_timer: 0.0 }
    }
}

fn band_of(magnitude: f32) -> u8 {
    BANDS.iter().position(|&b| b >= magnitude).unwrap_or(BANDS.len() - 1) as u8
}

/// `ResponseCurve__Evaluate` 0x5631D0 on the left stick's curve.
fn curve(x: f32) -> f32 {
    for w in CURVE.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x >= x0 && x <= x1 {
            return y0 + (x - x0) * (y1 - y0) / (x1 - x0);
        }
    }
    CURVE[CURVE.len() - 1].1
}

/// `tracker_dirs` returned as rotations of the pad's own camera-relative direction, so that where the game leaves
/// the direction as it is (no snap; 40° and beyond on the curve) it is bit for bit the same.
pub fn tracker_dirs_of(dir: Vec3, stick: Vec2, cam_yaw: f32) -> (Vec3, Vec3) {
    let n = stick.normalize_or_zero();
    if n == Vec2::ZERO || dir == Vec3::ZERO {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    let a = n.x.atan2(n.y);
    let c = curve(a.abs() / std::f32::consts::PI) * a.signum();
    let snap = if n.y > SNAP_COS { Vec3::new(-cam_yaw.sin(), 0.0, -cam_yaw.cos()) } else { dir };
    // stick angles grow toward the camera's right, i.e. clockwise seen from above: a rotation by -(c - a) about +Y
    // (a rotation below 1e-5 rad is float noise of the rebuilt stick, not a curve)
    let moved = if (c - a).abs() < 1e-5 { dir } else { (Quat::from_rotation_y(-(c - a)) * dir).normalize_or_zero() };
    (snap, moved)
}

/// `sub_55E7E0`: the snapped and the curved directions of a dead-zoned stick, through the camera transform.
pub fn tracker_dirs(stick: Vec2, cam_yaw: f32) -> (Vec3, Vec3) {
    let n = stick.normalize_or_zero();
    if n == Vec2::ZERO {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    let forward = Vec3::new(-cam_yaw.sin(), 0.0, -cam_yaw.cos());
    let right = Vec3::new(-forward.z, 0.0, forward.x);
    let world = |s: Vec2| (forward * s.y + right * s.x).normalize_or_zero();
    let snapped = if n.y > SNAP_COS { Vec2::Y } else { n };
    let a = n.x.atan2(n.y);
    let c = curve(a.abs() / std::f32::consts::PI) * a.signum();
    (world(snapped), world(Vec2::new(c.sin(), c.cos())))
}

/// The dead-zoned stick in pad axes (x right, y up) from the camera-relative direction the pad carries: the inverse
/// of `read_pad`'s camera transform (stick up = away from the camera). Exact for live input and recordings, which
/// store the direction, the magnitude and the camera yaw.
pub fn raw_stick(pad: &PadInput, cam_yaw: f32) -> Vec2 {
    if pad.magnitude < STICK_DEADZONE || pad.dir == Vec3::ZERO {
        return Vec2::ZERO;
    }
    let forward = Vec3::new(-cam_yaw.sin(), 0.0, -cam_yaw.cos());
    let right = Vec3::new(-forward.z, 0.0, forward.x);
    Vec2::new(pad.dir.dot(right), pad.dir.dot(forward)) * pad.magnitude
}

pub fn update_interpreter(
    time: Res<Time>,
    rig: Res<CameraRig>,
    mut pad: ResMut<PadInput>,
    mut q: Query<(&mut Locomotion, &Body, &mut HumanDataBundle, &mut Interpreter), With<Player>>,
) {
    let dt = time.delta_secs();
    let Ok((mut loco, body, mut data, mut it)) = q.single_mut() else { return };
    // a new frame: a switch made last frame no longer skips anything (`AIActor__UpdateContexts` 0x55F6A9 cleared the
    // byte right after the update that switched; RE/24 INPUT-1)
    loco.switched_this_frame = false;
    // +0x1244 (0xEEE250): the stick's angle to the facing, only while the stick is past 0.35
    if pad.magnitude > 0.35 {
        let (s, f) = (Vec3::new(pad.dir.x, 0.0, pad.dir.z).normalize_or_zero(), body.forward());
        if s != Vec3::ZERO {
            pad.stick_facing_angle = s.cross(f).y.atan2(s.dot(f));
        }
    }

    // the state machine follows the locomotion context; the enter functions' input side effects (RE/24 INPUT-3)
    if it.ctx != Some(loco.current) {
        let from = it.ctx;
        it.ctx = Some(loco.current);
        if from == Some(ActorContextId::Ladder) {
            it.latch_move = LATCH_MOVE;
        }
        match loco.current {
            ActorContextId::Ladder | ActorContextId::Ledge | ActorContextId::Climb => {
                pad.hand_buffer = false;
                it.hand_block = HAND_BLOCK;
                if loco.current == ActorContextId::Ladder {
                    it.latch_move = LATCH_MOVE_LADDER;
                }
            }
            // Walling entry clears the Legs buffer (+0x1126, `sub_ED87C0` 0xED8868)
            ActorContextId::Walling => pad.consume_jump(),
            // Ground entry clears the quick-reversal flag (`sub_ED72E0`: +0x112C = 0)
            ActorContextId::Ground => {
                it.quick_turn = false;
                it.quick_timer = 0.0;
            }
            _ => {}
        }
    }

    // the empty-hand buffer (+0x1129): set on the press; otherwise cleared unless Legs (button 0, pushed as `ebx` at
    // 0xEEE101 / 0xEEE11B) was pressed within 0.3 s; forced off during the block
    if pad.hand_just_pressed() {
        pad.hand_buffer = true;
    } else if pad.legs_pressed_ago > JUMP_BUFFER {
        pad.hand_buffer = false;
    }
    if it.hand_block > 0.0 {
        it.hand_block -= dt;
        pad.hand_buffer = false;
    }

    // the latched stick (0xEEE3E5–0xEEE483): above the dead zone, latch when unlatched or once the stick has moved
    // more than +0x1324 from the latched value, and store the mirrored copy (`sub_EDB450`) with it; below, follow the
    // stick unlatched
    let input_yaw = pad.input_yaw.unwrap_or(rig.yaw);
    let stick = raw_stick(&pad, input_yaw);
    if pad.magnitude >= STICK_DEADZONE {
        if (stick - pad.stick_latched).length() > it.latch_move {
            it.latch_on = false;
        }
        if !it.latch_on {
            pad.stick_latched = stick;
            // the camera's look direction stands for flat(actor − camera) (the rig looks at the character)
            let cam_forward = Vec3::new(-rig.yaw.sin(), 0.0, -rig.yaw.cos());
            let mirror = body.forward().dot(cam_forward) < MIRROR_DOT;
            pad.stick_actor = if mirror { Vec2::new(-stick.x, stick.y) } else { stick };
            it.latch_on = true;
        }
    } else {
        pad.stick_latched = stick;
        pad.stick_actor = stick;
        it.latch_on = false;
    }

    // the stick tracker (`sub_55EAE0` / `sub_55E7E0`): bands and the two directions
    pad.band_prev = pad.band;
    pad.band = band_of(pad.magnitude);
    let (snap, moved) = tracker_dirs_of(pad.dir, stick, input_yaw);
    pad.dir_snap = snap;
    pad.dir_move = moved;

    // the quick reversal (ground state only, `ProcessGroundMovement` 0xEE67FE–0xEE691E): on the frame the stick
    // enters band 1 or 2, raise the flag when it is clear and the curved direction is more than 120° from the facing;
    // on other frames drop it once its 0.3 s ran out or the ground cleared HG+1780 (vt980); every frame vt976 copies it
    // to HG+1780
    if loco.current == ActorContextId::Ground {
        let entered = (pad.band == 1 || pad.band == 2) && pad.band != pad.band_prev;
        it.quick_timer -= dt;
        if entered {
            if !it.quick_turn && moved != Vec3::ZERO && moved.angle_between(body.forward()) > QUICK_TURN_ANGLE {
                it.quick_turn = true;
                it.quick_timer = QUICK_TURN_TIME;
            }
        } else if it.quick_timer <= 0.0 || !data.ground.quick_turn {
            it.quick_turn = false;
        }
        data.ground.quick_turn = it.quick_turn;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deg(v: Vec3, cam_yaw: f32) -> f32 {
        // the angle of a world direction from the camera's forward, positive to the camera's right
        let f = Vec3::new(-cam_yaw.sin(), 0.0, -cam_yaw.cos());
        let r = Vec3::new(-f.z, 0.0, f.x);
        v.dot(r).atan2(v.dot(f)).to_degrees()
    }

    #[test]
    fn the_response_curve_softens_small_stick_angles() {
        // keys 5° → 1°, 10° → 5°, 20° → 10°, 30° → 20°, 40° and beyond unchanged (0x55EF26–0x55EFD0)
        for (stick, want) in [(5.0f32, 1.0f32), (10.0, 5.0), (15.0, 7.5), (20.0, 10.0), (30.0, 20.0), (40.0, 40.0), (90.0, 90.0), (-20.0, -10.0)] {
            let a = stick.to_radians();
            let (_, curved) = tracker_dirs(Vec2::new(a.sin(), a.cos()), 0.3);
            assert!((deg(curved, 0.3) - want).abs() < 0.01, "{stick}° → {}° (want {want}°)", deg(curved, 0.3));
        }
    }

    #[test]
    fn the_search_direction_snaps_straight_ahead_within_five_degrees() {
        let a = 4f32.to_radians();
        let (snap, _) = tracker_dirs(Vec2::new(a.sin(), a.cos()) * 0.8, 1.0);
        assert!(deg(snap, 1.0).abs() < 1e-3);
        let a = 6f32.to_radians();
        let (snap, _) = tracker_dirs(Vec2::new(a.sin(), a.cos()), 1.0);
        assert!((deg(snap, 1.0) - 6.0).abs() < 1e-3);
        assert_eq!(tracker_dirs(Vec2::ZERO, 0.0), (Vec3::ZERO, Vec3::ZERO));
    }

    #[test]
    fn the_magnitude_bands_are_0_35_0_9_1_0() {
        assert_eq!(band_of(0.0), 0);
        assert_eq!(band_of(0.35), 0);
        assert_eq!(band_of(0.351), 1);
        assert_eq!(band_of(0.9), 1);
        assert_eq!(band_of(0.901), 2);
        assert_eq!(band_of(1.0), 2);
    }

    #[test]
    fn the_turn_band_slows_small_turns_once_k_has_grown() {
        use crate::tuning::player_turn_rate;
        // k = 0 (just entered Ground): the max rate at any angle
        assert_eq!(player_turn_rate(0.05, false, 0.0), 4.0);
        // k = 1: under 10° the min rate, from 60° the max, linear between (RotateTowards 0xD94F30)
        assert_eq!(player_turn_rate(5f32.to_radians(), false, 1.0), 1.5);
        assert_eq!(player_turn_rate(70f32.to_radians(), false, 1.0), 4.0);
        assert!((player_turn_rate(35f32.to_radians(), false, 1.0) - 2.75).abs() < 1e-3);
        assert_eq!(player_turn_rate(5f32.to_radians(), true, 1.0), 0.975);
    }
}

#[cfg(test)]
mod rotation_tests {
    use super::*;
    #[test]
    fn the_rotated_directions_match_the_transformed_ones() {
        for yaw in [0.0f32, 0.7, -2.3, 3.0] {
            for a in [-120.0f32, -35.0, -12.0, -3.0, 0.0, 4.0, 8.0, 25.0, 39.0, 60.0, 170.0] {
                let r = a.to_radians();
                let stick = Vec2::new(r.sin(), r.cos());
                let f = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
                let right = Vec3::new(-f.z, 0.0, f.x);
                let dir = (f * stick.y + right * stick.x).normalize();
                let (s1, m1) = tracker_dirs(stick, yaw);
                let (s2, m2) = tracker_dirs_of(dir, stick, yaw);
                assert!(s1.distance(s2) < 1e-5 && m1.distance(m2) < 1e-5, "yaw {yaw} a {a}: {s1} {s2} / {m1} {m2}");
            }
        }
    }
}
