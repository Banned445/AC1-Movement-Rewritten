//! The action interpreter's own per-frame work (`GoAssassinActionInterpreter`, RE/01 §6, RE/24 INPUT-2 / 3 / 4).
//!
//! `GoAssassinActionInterpreter__ReadInput` 0xEEDE60 turns the pad into the values the contexts read, and the
//! interpreter's state machine follows the locomotion context (`sub_EE1850` → the state enter functions
//! 0xED8140…0xEDAE70), with side effects on entry. The port's contexts read `PadInput` directly; this system runs
//! first in the player chain and fills the interpreter-derived fields of `PadInput`:
//! - the latched actor-frame stick for the ledge, climb and ladder (+0x1300 / +0x1310, 0xEEE3E5–0xEEE483);
//! - the empty-hand buffer (+0x1129, 0xEEE0CC–0xEEE149) with the 0.2 s block after grabbing (+0x1180).

use bevy::prelude::*;

use super::{ActorContextId, Body, Locomotion, Player};
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
}

impl Default for Interpreter {
    fn default() -> Self {
        Self { latch_on: false, latch_move: LATCH_MOVE, ctx: None, hand_block: 0.0 }
    }
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
    mut q: Query<(&Locomotion, &Body, &mut Interpreter), With<Player>>,
) {
    let dt = time.delta_secs();
    let Ok((loco, body, mut it)) = q.single_mut() else { return };

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
    let stick = raw_stick(&pad, rig.yaw);
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
}
