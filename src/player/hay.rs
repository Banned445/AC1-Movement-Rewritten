//! HumanHayStack (context 21): landing in a haystack after a Leap of Faith, waiting inside, hopping out
//! (RE/04 §4.1.12).
//!
//! - Entry (`HumanHayStack__Enter` 0xE43140): from a faith jump `EnterTop_FaithLanding` 0xE41D50 plays
//!   `0x23A9666C` `xx_h_faith_jump_landing` (blend 0.3 s), other entries from the air `EnterFromAir` 0xE42700
//!   play `0x7750D212` `xx_h_air_to_haystack` (0.1 s). Both move the root to the haystack's position with the
//!   interpolator (`sub_711130`) over clamp(distance / speed, 0.1, 0.4) s.
//! - Wait (`ChooseWait` 0xE416B0 → `PlayWaitHigh` 0xE408C0): `0x23A9666D` `xx_h_haystack_wait`.
//! - Hop out: event 3 in the wait (`Wait_HandleEvent` 0xE43BD0), guard `Guard_HopOut` 0xE434E0: the ray along
//!   the wanted direction leaves the haystack's footprint; the exit point (+0.5 m along it, 1.25 m up) must
//!   have room for the body (sphere 0.35 / 0.5 / 0.75). `ToHopOut` 0xE41D00 → `PlayHopOut` 0xE41890 faces the
//!   direction and plays `0x2C4C2431` `xx_l_haystack_hop_out` (root motion), whose transitions lead to wait.

use bevy::prelude::*;

use super::jump_blend::ActionBlend;
use super::{switch_context, ActorContextId, Body, HumanDataBundle, Locomotion, Player, TransitionSetup};
use crate::collision::{Aabb3, CollisionWorld};
use crate::input::PadInput;

pub const HAYSTACK_FAITH_LANDING: u32 = 0x23A9_666C;
pub const HAYSTACK_WAIT: u32 = 0x23A9_666D;
pub const HAYSTACK_FROM_AIR: u32 = 0x7750_D212;
pub const HAYSTACK_HOP_OUT: u32 = 0x2C4C_2431;

#[derive(Clone, Copy, Debug)]
pub struct HayStackEntry {
    pub stack: Aabb3,
    /// Arrived by a Leap of Faith (faith landing) rather than another air entry.
    pub faith: bool,
    pub from: Vec3,
    /// Speed at arrival (the interpolator time is distance / speed).
    pub speed: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HayPhase {
    #[default]
    Entering,
    Waiting,
}

#[derive(Debug, Default)]
pub struct HumanHayStackData {
    pub stack: Option<Aabb3>,
    pub phase: HayPhase,
    /// The playing action (entry, then wait) and its time.
    pub action: Option<ActionBlend>,
    pub t: f32,
    pub from: Vec3,
    pub to: Vec3,
    /// Root interpolation time (0xE41D50: clamp(dist / speed, 0.1, 0.4)).
    pub interp: f32,
    pub seq: u32,
}

impl HumanHayStackData {
    pub fn enter(&mut self, e: HayStackEntry) {
        self.seq = self.seq.wrapping_add(1);
        self.stack = Some(e.stack);
        self.phase = HayPhase::Entering;
        let id = if e.faith { HAYSTACK_FAITH_LANDING } else { HAYSTACK_FROM_AIR };
        self.action = single(id);
        self.t = 0.0;
        self.from = e.from;
        // the haystack entity's position: its base centre
        self.to = Vec3::new((e.stack.min.x + e.stack.max.x) * 0.5, e.stack.min.y, (e.stack.min.z + e.stack.max.z) * 0.5);
        let d = (self.to - e.from).length();
        self.interp = if e.speed > 5e-4 { (d / e.speed).clamp(0.1, 0.4) } else { 0.1 };
    }
}

fn single(id: u32) -> Option<ActionBlend> {
    super::jump_blend::action_items(id).filter(|i| !i.is_empty()).map(|_| ActionBlend::new(id, 0, &[1.0]))
}

/// `Guard_HopOut` 0xE434E0 (reduced): where a hop out along `dir` leaves the stack, if there is room.
pub fn hop_out_exit(stack: &Aabb3, centre: Vec3, dir: Vec3, collision: &CollisionWorld) -> Option<Vec3> {
    let dir = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }
    // ray from the centre to the footprint's boundary
    let tx = if dir.x.abs() > 1e-5 { ((if dir.x > 0.0 { stack.max.x } else { stack.min.x }) - centre.x) / dir.x } else { f32::INFINITY };
    let tz = if dir.z.abs() > 1e-5 { ((if dir.z > 0.0 { stack.max.z } else { stack.min.z }) - centre.z) / dir.z } else { f32::INFINITY };
    let edge = centre + dir * tx.min(tz);
    let probe = edge + dir * 0.5 + Vec3::Y * 1.25;
    if collision.point_inside(probe) || collision.point_inside(probe - Vec3::Y * 0.75) {
        return None;
    }
    let h = collision.ground_height(probe, 3.0)?;
    Some(Vec3::new(probe.x, h, probe.z))
}

pub fn update_hay(
    time: Res<Time>,
    pad: Res<PadInput>,
    collision: Res<CollisionWorld>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (mut loco, mut body, mut data) in &mut q {
        if loco.current != ActorContextId::HayStack {
            continue;
        }
        if loco.just_switched {
            loco.just_switched = false;
            continue;
        }
        let h = &mut data.hay;
        h.t += dt;
        body.velocity = Vec3::ZERO;
        body.grounded = true;
        match h.phase {
            HayPhase::Entering => {
                body.feet = h.from.lerp(h.to, (h.t / h.interp).min(1.0));
                let dur = h.action.map(|a| a.duration()).unwrap_or(0.5);
                if h.t >= dur.max(h.interp) {
                    h.phase = HayPhase::Waiting;
                    h.action = single(HAYSTACK_WAIT);
                    h.t = 0.0;
                    h.seq = h.seq.wrapping_add(1);
                }
            }
            HayPhase::Waiting => {
                body.feet = h.to;
                // PORT: event 3 (hop out) comes from the untraced decision layer; the port sends it when the
                // stick is pushed after the wait started
                if pad.speed01 > 0.0 && h.t > 0.2 {
                    let Some(stack) = h.stack else { continue };
                    if hop_out_exit(&stack, h.to, pad.dir, &collision).is_some() {
                        body.heading = super::heading_of(pad.dir);
                        let hop = single(HAYSTACK_HOP_OUT);
                        switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: None });
                        if let Some(b) = hop {
                            data.ground.play_oneshot(b);
                        }
                    }
                }
            }
        }
    }
}
