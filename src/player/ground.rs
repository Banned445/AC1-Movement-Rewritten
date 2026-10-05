//! Ground locomotion context (`HumanGround`, ActorContextID 4) — RE/02.
//!
//! - One speed parameter (0..1) split into bands Walk/Jog/Run/Sprint (GetSpeedBand 0xD807B0).
//! - Target = base + 0.25·stick, base 0 (low profile) / 0.5 (high profile) / 0.75 (sprint).
//! - Parameter rises at 1.0/s and falls through the deceleration ResponseCurve (0xDA0810, `move_blend`).
//! - Heading turns toward the wanted direction at 360°/s (0xD95290).
//! - Translation: root motion of the 17-clip locomotion blend (action 0x05923BDB, `move_blend`).
//! - Ground loss → InAir fall (0xD87720 / 0xD8C380); jump request → InAir jump-to-target.

use bevy::prelude::*;

use super::air::{FallOrigin, InAirEntry, Landing, LandingType};
use super::climb::{ClimbEntry, ClimbEntryType};

use super::jump_blend::{self, ActionBlend};
use super::move_blend::MoveBlend;
use super::targets::{edge_ahead, find_jump_target, JumpTarget};
use super::{
    heading_of, switch_context, ActorContextId, Body, HumanDataBundle, Locomotion, Player, SpawnPoint, TransitionSetup,
};
use crate::camera::CameraRig;
use crate::collision::CollisionWorld;
use crate::guidance::GuidanceWorld;
use crate::input::PadInput;
use crate::tuning::*;

/// `HumanGroundData::HumanGroundSubState` (values from the exe, desc 0x19961C8).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HumanGroundSubState {
    #[default]
    Movement = 0,
    Fight = 1,
    FreeRun = 2,
    OrientedMove = 3,
    Hurt = 4,
    ObstacleCollision = 5,
}

/// Speed band as returned by `IHumanGround::GetSpeedBand` (same order as AssassinAbilitySet::MaxSpeed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeedBand {
    None,
    Walk,
    Jog,
    Run,
    Sprint,
}

pub fn speed_band(param: f32) -> SpeedBand {
    if param <= 0.0 {
        SpeedBand::None
    } else if param <= BAND_WALK {
        SpeedBand::Walk
    } else if param <= BAND_JOG {
        SpeedBand::Jog
    } else if param <= BAND_RUN {
        SpeedBand::Run
    } else {
        SpeedBand::Sprint
    }
}

/// Runtime data of the ground context (subset of the reflected HumanGroundData, RE/07).
#[derive(Debug, Default)]
pub struct HumanGroundData {
    pub sub_state: HumanGroundSubState,
    /// HG+0x5E8: the 0..1 speed parameter.
    pub speed_param: f32,
    /// Turn attenuation factor (interpreter +0x10D0).
    pub turn_atten: f32,
    /// `Sprint` flag (HumanGroundData+0x123, name recovered by CRC32).
    pub sprint: bool,
    pub high_profile: bool,
    /// The landing / reception action playing after an InAir landing (0xE05940 / 0xE07D00): its root
    /// motion moves the character and input waits until it ends (its transitions lead back to the
    /// locomotion action 0x05923BDB or wait).
    pub oneshot: Option<GroundOneShot>,
    /// Played when `oneshot` ends (the run stop's settle into the wait).
    pub oneshot_next: Option<ActionBlend>,
    pub last_landing: Option<Landing>,
    /// Incremented on every landing (lets the animator play the landing clip once).
    pub landing_seq: u32,
    /// `HumanGround__UpdateMoveBlend` state: blend weights, timers, lean/bank, step cycle.
    pub blend: MoveBlend,
    /// LedgeStop sub-state (38): the edge it stopped at.
    pub ledge_stop: Option<LedgeStop>,
    /// PORT: no new ledge stop until the stick lets go or turns away from this edge normal (the game's event 69
    /// sender is not traced).
    pub ledge_stop_lock: Option<Vec3>,
    /// ObstacleCollision (sub-state 5, event 42): the collide / lean on an obstacle (`collide`).
    pub collide: Option<super::collide::Collide>,
    /// Look-down at an edge (sub-state 9, event 119, `HumanGround__LookDown_Enter` 0xD9FC80).
    pub look_down: Option<LookDown>,
    pub pose_seq: u32,
    /// Facing applied when the next one-shot starts (the lean exits end turned, `collide`).
    pub face_next: Option<f32>,
    /// Side of the turn in progress (+1 / -1, 0 = none): the 180 deg tie-break.
    pub turn_sign: f32,
    /// Stick-to-ground residual of the last frame (controller +384, 0x57D240): how far the support was from the feet.
    pub snap_residual: f32,
}

/// `HumanGround__LookDown_Enter` 0xD9FC80: `xx_l_ledge_lookdown_{front,left,right}_foot{l,r}` (by the leading foot)
/// blended by the angle between the facing and the point 1 m past the edge (report point + normal): [1 − k, left k,
/// right k], k = min(|a|, 90°) / 90°.
pub const LOOK_DOWN: [u32; 2] = [0x2669_E0F7, 0x2669_E0F8];

/// Pivot actions (table 0x1A2C120, filled by 0xDB6E50): [left, right] x [from low, high] x [to low, high] x [foot l, r],
/// each `*_wait_hipm_foot?_to_?_waitturn_{left,right}_{090,180}_foot?` (2 clips).
pub const PIVOT: [[[[u32; 2]; 2]; 2]; 2] = [
    [[[0x082F_BC7C, 0x082F_BC87], [0x1ABA_2384, 0x1ABA_2385]], [[0x1ABA_2388, 0x1ABA_2389], [0x09A0_9DF1, 0x09A0_A217]]],
    [[[0x082F_BC88, 0x082F_BC89], [0x1ABA_2386, 0x1ABA_2387]], [[0x1ABA_238A, 0x1ABA_238B], [0x09A0_9DF2, 0x09A0_A218]]],
];
/// Start from standing (`HumanGround__PlayStartMove` 0xD98990): the locomotion action entered through these items,
/// [low, high profile] x [foot l, r] (by `Human__GetLeadingFoot` 0xB18850). Clips [wait_tr_walk_slow, wait_tr_walk,
/// wait_tr_jog, impulsion_to_sprint]; the exe weights walk (low) or jog (high) and sets the speed parameter HG+0x5E8 to
/// 0.25 / 0.5 at once.
pub const START_MOVE: [[u32; 2]; 2] = [[0x09A0_8AC6, 0x09A0_A426], [0x09A0_AA2D, 0x09A0_AA2E]];

pub fn is_start(id: u32) -> bool {
    START_MOVE.iter().flatten().any(|&s| s == id)
}

pub const PIVOT_ACTIONS: [u32; 20] = [
    0x09A0_8AC6, 0x09A0_A426, 0x09A0_AA2D, 0x09A0_AA2E,
    0x082F_BC7C, 0x082F_BC87, 0x1ABA_2384, 0x1ABA_2385, 0x1ABA_2388, 0x1ABA_2389, 0x09A0_9DF1, 0x09A0_A217,
    0x082F_BC88, 0x082F_BC89, 0x1ABA_2386, 0x1ABA_2387, 0x1ABA_238A, 0x1ABA_238B, 0x09A0_9DF2, 0x09A0_A218,
];

#[derive(Clone, Copy, Debug)]
pub struct LookDown {
    pub action: ActionBlend,
    pub t: f32,
    pub point: Vec3,
    pub normal: Vec3,
}

/// The look-down weights for an edge at `p` / outward normal `n` seen from `feet` facing `forward`.
pub fn look_down_weights(feet: Vec3, forward: Vec3, p: Vec3, n: Vec3) -> [f32; 3] {
    let to = Vec3::new(p.x + n.x - feet.x, 0.0, p.z + n.z - feet.z).normalize_or_zero();
    let a = to.dot(forward).clamp(-1.0, 1.0).acos();
    let k = (a.min(std::f32::consts::FRAC_PI_2) / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0);
    // left when the edge is on the character's left
    if to.dot(super::right_of(forward)) < 0.0 { [1.0 - k, k, 0.0] } else { [1.0 - k, 0.0, k] }
}

/// `HumanGround__LedgeStop_Enter` 0xD93C60 / `HumanGround__LedgeStop_PlayEnd` 0xD7D9D0.
#[derive(Clone, Copy, Debug)]
pub struct LedgeStop {
    /// Edge report: point (+16) and outward normal (+32).
    pub point: Vec3,
    pub normal: Vec3,
    /// The end action is playing (the start action, 0x06E8BD7F, is done).
    pub ending: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct GroundOneShot {
    pub blend: ActionBlend,
    pub t: f32,
    pub duration: f32,
    /// Displacement already applied (animation space).
    pub applied: [f32; 3],
    /// Heading when the action started: its displacement and root yaw are in this frame (set on the first update).
    pub h0: Option<f32>,
}

impl HumanGroundData {
    /// Play a ground one-shot action with its root motion (landings, receptions, ledge stop).
    pub fn play_oneshot(&mut self, b: ActionBlend) {
        self.landing_seq = self.landing_seq.wrapping_add(1);
        self.oneshot = Some(GroundOneShot { blend: b, t: 0.0, duration: b.duration(), applied: [0.0; 3], h0: None });
    }

    /// `TransitionSetupDataToMovement::Apply` (0xC80310), simplified.
    pub fn enter(&mut self, landing: Option<Landing>) {
        self.sub_state = HumanGroundSubState::Movement;
        self.ledge_stop = None;
        self.collide = None;
        self.look_down = None;
        self.oneshot_next = None;
        if landing.is_none() {
            // PORT: entries without a landing (pull-up, beam / ladder / pass-over exits …) start standing. The game's
            // OnEnterInit 0xDA7D20 leaves HG+0x5E8 alone, but those contexts drive the parameter themselves; the port's
            // stale value from before the climb made the run stop slide on after the move (off roof edges).
            // LIVE: read HG+0x5E8 right after a pull-up.
            self.speed_param = 0.0;
            self.blend.speed_param = 0.0;
            self.oneshot = None;
        }
        if let Some(l) = landing {
            self.landing_seq = self.landing_seq.wrapping_add(1);
            self.oneshot = l.action.map(|b| GroundOneShot { blend: b, t: 0.0, duration: b.duration(), applied: [0.0; 3], h0: None });
            // The speed parameter HG+0x5E8 is not reset by OnEnterInit 0xDA7D20, so the landing's exit
            // into locomotion continues at the take-off speed. (hypothesis) heavy-damage landings stop.
            if l.kind == LandingType::HeavyDamage {
                self.speed_param = 0.0;
            }
            self.last_landing = Some(l);
        }
    }
}

/// Heading kept in (-pi, pi] (it used to grow without bound with every turn).
fn wrap_angle(a: f32) -> f32 {
    angle_diff(a, 0.0)
}

fn angle_diff(a: f32, b: f32) -> f32 {
    let mut d = (a - b) % std::f32::consts::TAU;
    if d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    } else if d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    d
}

#[allow(clippy::too_many_arguments)]
pub fn update_ground(
    time: Res<Time>,
    mut pad: ResMut<PadInput>,
    collision: Res<CollisionWorld>,
    guidance: Res<GuidanceWorld>,
    spawn: Res<SpawnPoint>,
    mut rig: ResMut<CameraRig>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (mut loco, mut body, mut data) in &mut q {
        let g = &mut data.ground;
        if loco.current != ActorContextId::Ground {
            continue;
        }
        if loco.just_switched {
            // AIActor::Update skips a context's first update after it was switched in.
            loco.just_switched = false;
            if let Some(l) = g.last_landing {
                // drop > 3 m: camera shake (drop − 3) / 7 (0xE05940)
                if l.total_drop > 3.0 {
                    rig.shake = ((l.total_drop - 3.0) / 7.0).min(1.0);
                }
                if l.kind == LandingType::Fatal {
                    // "desynchronisation": respawn
                    body.feet = spawn.0;
                    body.velocity = Vec3::ZERO;
                }
            }
            // the controller keeps integrating the velocity the switch left (no one-frame freeze)
            body.grounded = true;
            super::coast(&mut body, &collision, dt);
            continue;
        }

        // ---------------------------------------------------------------- input → wanted motion
        // the start item is the locomotion action itself (entered through a transition): steering and the other
        // requests stay live while it plays; a released stick ends it
        let starting = g.oneshot.is_some_and(|o| is_start(o.blend.id));
        if starting && pad.speed01 <= 0.0 {
            g.oneshot = None;
        }
        let starting = starting && pad.speed01 > 0.0;
        // The anim gate of Idle → Move (0xD84AC0) and of the pivot (0xD84B10): the playing item has ended, or its word
        // allows leaving it for moving in this profile and it is not locked (`HumanGround__AnimAllowsModeExit`
        // 0xD80010, `anim_gate`). A one-shot that allows it is left at once for the locomotion (its transition blends,
        // RE/13 §7); locked ones (landings, run stops, pivots) play out. Not in the ground's own sub-states (the ledge
        // stop, sub-state 38; the obstacle collision, 5): they leave by their own rules.
        if !starting
            && pad.speed01 > 0.0
            && g.ledge_stop.is_none()
            && g.collide.is_none()
            && g.oneshot.is_some_and(|o| super::anim_gate::allows_mode_exit(&o.blend, true, pad.high_profile))
        {
            g.oneshot = None;
            g.oneshot_next = None;
        }
        let busy = g.oneshot.is_some() && !starting;
        let moving = pad.speed01 > 0.0 && !busy;
        let prev_high = g.high_profile;
        g.high_profile = pad.high_profile;
        g.sprint = pad.high_profile && pad.legs_held; // sprint = high profile + legs (RE/01 §6.2)
        g.sub_state = if g.sprint { HumanGroundSubState::FreeRun } else { HumanGroundSubState::Movement };

        // turn attenuation (interpreter 0xEE65A0): beyond 45° slow down; forced to 1 in low profile
        let want_heading = if moving { heading_of(pad.dir) } else { body.heading };
        let off = angle_diff(want_heading, body.heading).abs();
        let atten_target = if !g.high_profile || off <= TURN_ATTEN_START {
            1.0
        } else {
            (1.0 - (off - TURN_ATTEN_START) / TURN_ATTEN_RANGE).max(TURN_ATTEN_FLOOR)
        };
        g.turn_atten = if atten_target > g.turn_atten {
            (g.turn_atten + TURN_ATTEN_UP_RATE * dt).min(atten_target)
        } else {
            (g.turn_atten - TURN_ATTEN_DOWN_RATE * dt).max(atten_target)
        };

        // speed parameter: target = base + 0.25·stick (RE/02 §1)
        let base = if g.sprint {
            BASE_SPRINT
        } else if g.high_profile {
            BASE_HIGH_PROFILE
        } else {
            BASE_LOW_PROFILE
        };
        // the target follows the stick even while a landing action plays (MoveBlend's transition path keeps
        // ramping toward it, 0xDA0810)
        let target = if pad.speed01 > 0.0 { (base + STICK_SPAN * pad.speed01 * g.turn_atten).min(1.0) } else { 0.0 };
        // stick released (desired mode HG+0x5D8 = 0): the game leaves the Move state instead of decelerating
        // (the curve at HG+0x63C only runs while still moving toward a slower band):
        // - jog or faster (HG+0x5DC, speed > 0.25) → RunStop 0xD98E30 (guard 0xD7EC90): the run-stop action by the
        //   leading foot, its root motion, then its settle into the wait;
        // - walk band → Idle 0xD8B220 (guard 0xD7ED30): the wait with a 0.2 s blend, i.e. stopped at once.
        // Stick released while a landing / reception plays: its transition leads into the wait, not into the
        // locomotion (the action's exits, RE/13), so no run stop follows it. Before, the speed kept from the jump
        // started a run stop once the landing ended: a second slide after the landing.
        if pad.speed01 <= 0.0 && g.speed_param > 0.0 && g.oneshot.is_some_and(|o| !jump_blend::RUN_STOP.contains(&o.blend.id) && !jump_blend::RUN_STOP_TO_WAIT.contains(&o.blend.id)) {
            g.speed_param = 0.0;
            g.blend.speed_param = 0.0;
        }
        // The run stop is allowed by Data+0x11F, which the input interpreter sets (IHumanGround vt136 0xDB31B0, from
        // 0xEE6783 / 0xEE67EE / 0xEE67FC): 1 with the stick released, and with the stick held more than 135 deg from the
        // facing (dot < -0.7071); 0 otherwise. Its guard (0xD7EC90) also needs the high profile (HG+1500). So
        // pulling the stick back at a run skids (run stop), and the Idle that follows pivots (state 25): the skid turn.
        let reversed = pad.speed01 > 0.0 && pad.dir.dot(body.forward()) < -std::f32::consts::FRAC_1_SQRT_2;
        if reversed && g.high_profile && g.speed_param > 0.25 && g.oneshot.is_none() {
            let foot = (g.blend.foot != 0) as usize;
            if let Some(b) = jump_blend::action_items(jump_blend::RUN_STOP[foot]).map(|_| ActionBlend::new(jump_blend::RUN_STOP[foot], 0, &jump_blend::run_stop_weights(g.speed_param))) {
                g.play_oneshot(b);
                g.oneshot_next = None;
                g.speed_param = 0.0;
                g.blend.speed_param = 0.0;
            }
        }
        if pad.speed01 <= 0.0 && g.speed_param > 0.0 && g.oneshot.is_none() {
            if g.speed_param > 0.25 {
                let foot = (g.blend.foot != 0) as usize;
                if let Some(b) = jump_blend::action_items(jump_blend::RUN_STOP[foot]).map(|_| ActionBlend::new(jump_blend::RUN_STOP[foot], 0, &jump_blend::run_stop_weights(g.speed_param))) {
                    g.play_oneshot(b);
                    g.oneshot_next = jump_blend::action_items(jump_blend::RUN_STOP_TO_WAIT[foot]).map(|_| ActionBlend::new(jump_blend::RUN_STOP_TO_WAIT[foot], 0, &[1.0]));
                }
            }
            g.speed_param = 0.0;
            g.blend.speed_param = 0.0;
        }
        // moving again during the run stop: the stop item is locked (0x20) and plays out; its settle into the wait
        // allows leaving for moving (0x200 / 0x800), which the anim gate above does
        // pivot (Movement state 25, `HumanGround__Pivot_Enter` 0xDA6150): the wanted heading more than 90 deg (HG+0x720)
        // from the current one, from standing (guard 0xD84B10) or from a low-profile walk (Move guard 0xD84F10: the
        // current profile HG+1500 low). The turn action from the table at 0x1A2C120 by side, [from, to] profile and
        // leading foot, blending its 90 / 180 deg clips by (|a| - 90 deg) / 90 deg; the clip's root yaw turns the body.
        if moving && !busy && g.oneshot.is_none() && g.collide.is_none() && g.ledge_stop.is_none() && off > std::f32::consts::FRAC_PI_2 && (g.speed_param <= 0.0 || (!prev_high && g.speed_param <= BAND_WALK)) {
            let left = pad.dir.dot(super::right_of(body.forward())) < 0.0;
            let id = PIVOT[(!left) as usize][prev_high as usize][g.high_profile as usize][(g.blend.foot != 0) as usize];
            let w = ((off - std::f32::consts::FRAC_PI_2) / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0);
            if jump_blend::action_items(id).is_some() {
                g.play_oneshot(ActionBlend::new(id, 0, &[1.0 - w, w]));
                g.speed_param = 0.0;
                g.blend.speed_param = 0.0;
                body.velocity = Vec3::ZERO;
                continue;
            }
        }

        // The interpreter's own edge stop (0xEE7BDA–0xEE7CB4), low profile: the stick-direction edge report within
        // 0.16 m with a drop of more than 2 m and its normal within 70 deg of the facing (and of the stick) zeroes the
        // wanted speed: the walk halts at the edge without a clip (drops of 2–5 m; deeper ones get the ledge stop),
        // and with no move request Idle does not start again.
        let edge_halt = !g.high_profile
            && moving
            && edge_report_drop(body.feet, body.forward(), EDGE_HALT_REACH, EDGE_HALT_COS, 2.0, &guidance, &collision)
                .is_some_and(|(_, n, d)| (d <= LEDGE_STOP_DROP || g.ledge_stop_lock.is_some()) && pad.dir.dot(Vec3::new(n.x, 0.0, n.z).normalize_or_zero()) > EDGE_HALT_COS);
        // start from standing (Idle → Move, 0xD84AC0 → `HumanGround__PlayStartMove` 0xD98990)
        if moving && !busy && !starting && !edge_halt && g.oneshot.is_none() && g.speed_param <= 0.0 && g.collide.is_none() && g.ledge_stop.is_none() && g.ledge_stop_lock.is_none() {
            let id = START_MOVE[g.high_profile as usize][(g.blend.foot != 0) as usize];
            if jump_blend::action_items(id).is_some() {
                g.play_oneshot(ActionBlend::new(id, 0, if g.high_profile { &[0.0, 0.0, 1.0, 0.0] } else { &[0.0, 1.0, 0.0, 0.0] }));
                g.speed_param = if g.high_profile { 0.5 } else { 0.25 };
                g.blend.speed_param = g.speed_param;
            }
        }

        // speed parameter, lean/bank and blend weights (MoveBlend 0xDA0810). The heading snapshot is the
        // heading before this frame's turn (HG+0x600, Movement_PreUpdate 0xD97E30).
        g.blend.speed_param = g.speed_param;
        g.blend.update_speed(target, dt);
        g.blend.update_angles(body.heading, moving.then_some(want_heading), None, false, dt);
        g.blend.update_weights(target, dt);
        g.speed_param = g.blend.speed_param;
        // the run stop (state 18) does not run MoveBlend: the parameter stays 0 while it plays
        if g.oneshot.is_some_and(|o| jump_blend::RUN_STOP.contains(&o.blend.id) || jump_blend::RUN_STOP_TO_WAIT.contains(&o.blend.id)) {
            g.speed_param = 0.0;
            g.blend.speed_param = 0.0;
        }

        // heading: rotate toward wanted at the player turn rate (0xD95290), unless the playing item turns the body
        // itself (0x10)
        if moving && !g.oneshot.is_some_and(|o| super::anim_gate::anim_turns(&o.blend)) {
            let mut d = angle_diff(want_heading, body.heading);
            // PORT: near 180 deg the shorter way flips sign with tiny stick changes, so the character turned back
            // and forth (jitter when reversing). Keep the side a turn already started on (RotateTowards 0xD94F30's
            // tie rule is not traced).
            if d.abs() > 170f32.to_radians() && g.turn_sign != 0.0 && d.signum() != g.turn_sign {
                d += g.turn_sign * std::f32::consts::TAU;
            }
            let step = PLAYER_TURN_RATE * dt;
            let turn = d.clamp(-step, step);
            g.turn_sign = if d.abs() > 1e-3 { turn.signum() } else { 0.0 };
            body.heading = wrap_angle(body.heading + turn);
        } else {
            g.turn_sign = 0.0;
        }

        // ---------------------------------------------------------------- obstacle collision / lean (sub-state 5)
        if let Some(mut c) = g.collide {
            g.sub_state = HumanGroundSubState::ObstacleCollision;
            let stick = (pad.speed01 > 0.0).then_some(pad.dir);
            let (mut feet, mut heading) = (body.feet, body.heading);
            let out = super::collide::update(&mut c, dt, stick, pad.high_profile, &mut feet, &mut heading);
            body.feet = feet;
            body.heading = heading;
            match out {
                super::collide::CollideOut::Stay => g.collide = Some(c),
                super::collide::CollideOut::Leave { first, then, speed, face } => {
                    g.collide = None;
                    // the exit clips turn the body by their root yaw (back 180 deg, side 90 deg); the stick-based facing
                    // is only a fallback for an exit without one
                    g.face_next = if first.is_some_and(|b| b.yaw(1.0).abs() > 0.05) { None } else { face.map(heading_of) };
                    g.speed_param = speed;
                    g.blend.speed_param = speed;
                    if let Some(b) = first {
                        g.play_oneshot(b);
                        g.oneshot_next = then;
                    }
                }
            }
            body.velocity = Vec3::ZERO;
            continue;
        }

        // ---------------------------------------------------------------- look-down at an edge (event 119)
        // PORT trigger: standing still (the event's sender and its guard's mode value, 0xD7E590, are not traced)
        if moving || busy || g.speed_param > 0.0 {
            g.look_down = None;
        } else if let Some(mut ld) = g.look_down {
            ld.t += dt;
            g.look_down = Some(ld);
        } else if let Some((p, n)) = look_down_edge(body.feet, body.forward(), &guidance, &collision) {
            let w = look_down_weights(body.feet, body.forward(), p, n);
            if let Some(a) = jump_blend::action_items(LOOK_DOWN[(g.blend.foot != 0) as usize]).map(|_| ActionBlend::new(LOOK_DOWN[(g.blend.foot != 0) as usize], 0, &w)) {
                g.pose_seq = g.pose_seq.wrapping_add(1);
                g.look_down = Some(LookDown { action: a, t: 0.0, point: p, normal: n });
            }
        }

        // ---------------------------------------------------------------- wall run (Walling, event 49)
        // Input handler 0xEE65A0 (tested before the jumps and the grab): high profile, Legs pressed, the stick
        // pushed within 45° of the facing (> 0.35), ability Walling, and IHumanGround vt112 = event 49's guard
        // (0xDA54F0: the playing item allows a mode exit, then the wall test 0xE18390).
        if g.high_profile && pad.jump_buffered() && moving && pad.dir.dot(body.forward()) >= 45f32.to_radians().cos() {
            if let Some((contact, normal)) = super::walling::wall_ahead(body.feet, body.forward(), &collision) {
                pad.consume_jump();
                let from = body.feet;
                switch_context(&mut loco, &mut data, TransitionSetup::ToWalling(super::walling::WallingEntry { contact, normal, from }));
                continue;
            }
        }

        // ---------------------------------------------------------------- beam (NarrowObject, event 72)
        // Guard 0xD9F4C0: a beam in the box ahead (±0.75 m, 0–1 m, ±0.53 m). PORT trigger: walking at it (the
        // decision layer's event 72 sender is not traced).
        if moving && !busy {
            if let Some(entry) = super::narrow::try_mount_beam(body.feet, body.forward(), &guidance) {
                switch_context(&mut loco, &mut data, TransitionSetup::ToBeam(entry));
                continue;
            }
        }

        // ---------------------------------------------------------------- ladder (event 38)
        // Guard `sub_B239D0` (0xD83970): a ladder within reach, the character on its front within 90°; the entry from
        // the top within 1.5 m of it. PORT triggers (event 38's sender is not traced): walking into the ladder's foot
        // facing it; from the top, low profile + Legs at its top facing out over it.
        {
            let top_req = !g.high_profile && pad.jump_buffered();
            if (moving || top_req) && !busy {
                if let Some((base, top, n, from_top)) = super::ladder::find_ladder(body.feet, body.forward(), 0.8, &guidance) {
                    if from_top == top_req {
                        if from_top {
                            pad.consume_jump();
                        }
                        let e = super::ladder::LadderEntry { base, top, n, from: body.feet, facing: body.forward(), from_top, high: g.high_profile, foot: (g.blend.foot != 0) as usize, from_ledge: false, action: None, ..Default::default() };
                        switch_context(&mut loco, &mut data, TransitionSetup::ToLadder(e));
                        continue;
                    }
                }
            }
        }

        // ---------------------------------------------------------------- climb / grab requests
        // interpreter vt736/740 (grab wall) and vt764/768 (climb start): high profile + Legs into a wall
        let forward = body.forward();
        if g.high_profile && pad.legs_held && moving {
            // leading foot (Human__GetLeadingFoot 0xB18850): the playing locomotion item, footl = left ahead
            if let Some(setup) = try_wall_grab(body.feet, forward, g.blend.foot != 0, &guidance, &collision) {
                pad.consume_jump();
                switch_context(&mut loco, &mut data, setup);
                continue;
            }
        }

        // ---------------------------------------------------------------- ledge stop (event 69 / sub-state 38)
        if let Some(ls) = g.ledge_stop {
            // pull-down EdgeStop (LedgeStop_HandleEvent 0xDA4D90, event 70): only while the start action plays
            // (guard 0xD9D580). PORT trigger as for Wait: Legs.
            if !ls.ending && pad.jump_buffered() {
                if let Some(entry) = pulldown_entry(ls.point, ls.normal, body.feet, false, &guidance, &collision) {
                    pad.consume_jump();
                    g.ledge_stop = None;
                    g.oneshot = None;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(entry));
                    continue;
                }
            }
            if g.oneshot.is_none() {
                // end done → Movement (Locomotion_Update 0xDAF2D0, sub_5017B0); start → end is in the move step
                g.ledge_stop = None;
            }
        }
        if let Some(n) = g.ledge_stop_lock {
            if pad.speed01 <= 0.0 || pad.dir.dot(Vec3::new(n.x, 0.0, n.z).normalize_or_zero()) < FRONT_COS {
                g.ledge_stop_lock = None;
            }
        }
        // Movement event 69 (0xDB1470): guard 0xDA5DE0 = front edge (ClassifyEdgeSide 0xD9D7F0 → 1), drop > 2 m,
        // body space → ToLedgeStop 0xDA99F0. Side edges (3/4, guard 0xDA5D90) go to another state (not ported).
        // Sender: the input interpreter (0xEE8899 → IHumanGround vt748 `HumanGround__DoLedgeStop` 0xDBCAD0): a front
        // edge (vt744) within 0.15 m (0.0225 = 0.15²) with a drop of more than 5 m, in either profile. Free running
        // (high profile + Legs) jumps first (the interpreter's jump branch returns before it).
        let busy = g.oneshot.is_some_and(|o| !is_start(o.blend.id));
        if moving && !busy && g.ledge_stop_lock.is_none() && !(g.high_profile && pad.legs_held) {
            if let Some((p, n)) = edge_report_drop(body.feet, body.forward(), LEDGE_STOP_REACH, FRONT_COS, LEDGE_STOP_DROP, &guidance, &collision).map(|r| (r.0, r.1)) {
                if let Some(b) = super::ledge_moves::single_item(super::ledge_moves::LEDGE_STOP_START, 0) {
                    g.ledge_stop = Some(LedgeStop { point: p, normal: n, ending: false });
                    g.ledge_stop_lock = Some(n);
                    g.speed_param = 0.0;
                    g.blend.speed_param = 0.0;
                    g.play_oneshot(b);
                    continue;
                }
            }
        }

        // ---------------------------------------------------------------- pull-down (event 70)
        // The game's decision layer sends event 70 (not traced); guard 0xD9D6C0: facing along the edge's outward
        // normal, a drop of more than 2 m, body space. PORT trigger: Legs in low profile at an edge.
        if !g.high_profile && pad.jump_buffered() && !busy {
            if let Some(entry) = try_pulldown(body.feet, body.forward(), &guidance, &collision) {
                pad.consume_jump();
                switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(entry));
                continue;
            }
        }

        // ---------------------------------------------------------------- jump requests
        // vt24 JumpToGuidanceTarget: high profile + jump buffer + stick > dead-zone (RE/01 §6.3).
        // While free-running with Legs held the game also jumps when it reaches an edge
        // (hypothesis from gameplay; modelled as: Legs held + no floor ahead).
        let forward = body.forward();
        let want_jump = g.high_profile
            && moving
            && (pad.jump_buffered() || (pad.legs_held && edge_ahead(body.feet, forward, &collision)));
        if want_jump && !busy {
            pad.consume_jump();
            let entry = match find_jump_target(body.feet, if moving { pad.dir } else { forward }, &guidance, &collision) {
                // leading foot: the playing locomotion item (footl item = left ahead) (hypothesis)
                Some(t) => InAirEntry::JumpToTarget { from: body.feet, target: t, speed_param: g.speed_param, foot_left: g.blend.foot == 0 },
                // vt28: free jump without a target
                None => InAirEntry::FreeJump { from: body.feet, dir: forward, speed_param: g.speed_param },
            };
            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
            continue;
        }

        // ---------------------------------------------------------------- move (blended clip root motion)
        let stopping = g.oneshot.is_some_and(|o| jump_blend::RUN_STOP.contains(&o.blend.id) || jump_blend::RUN_STOP_TO_WAIT.contains(&o.blend.id));
        let (delta, speed) = if let Some(mut os) = g.oneshot {
            // landing / reception action: its blended root motion (FROMANIM)
            os.t += dt;
            // the action's displacement and root yaw are in the heading it started with (FROMANIM)
            // (the start item follows the steering: its frame is the current heading)
            let h0 = if is_start(os.blend.id) { body.heading } else { *os.h0.get_or_insert(body.heading) };
            let yaw = os.blend.yaw(os.t / os.duration.max(1e-4));
            if yaw.abs() > 1e-4 {
                body.heading = wrap_angle(h0 + yaw);
            }
            let d = os.blend.disp(os.t / os.duration.max(1e-4));
            let step = [d[0] - os.applied[0], d[1] - os.applied[1]];
            os.applied = d;
            g.oneshot = (os.t < os.duration).then_some(os);
            if g.oneshot.is_none() {
                if let Some(next) = g.oneshot_next.take() {
                    g.play_oneshot(next);
                }
                if let Some(h) = g.face_next.take() {
                    body.heading = h;
                }
                if let Some(mut ls) = g.ledge_stop.filter(|l| !l.ending) {
                    // start done → end action, same frame (HumanGround__LedgeStop_PlayEnd 0xD7D9D0)
                    ls.ending = true;
                    g.ledge_stop = Some(ls);
                    if let Some(b) = super::ledge_moves::single_item(super::ledge_moves::LEDGE_STOP_END, 0) {
                        g.play_oneshot(b);
                    }
                }
            }
            let f0 = Vec3::new(-h0.sin(), 0.0, -h0.cos());
            let right = super::right_of(f0);
            let delta = right * step[0] + f0 * step[1];
            (delta, delta.length() / dt.max(1e-4))
        } else {
            let speed = if g.speed_param > 0.0 { g.blend.advance(dt) } else { 0.0 };
            (forward * speed * dt, speed)
        };
        // PORT: after a ledge stop, still pushing into the same edge holds the character at it (the game re-sends
        // event 69 from its untraced sender; the port does not loop stop / step back)
        // The interpreter's own edge stop (0xEE7BDA–0xEE7CB4), low profile: the stick-direction edge report within
        // 0.16 m with a drop of more than 2 m and its normal within 70 deg of the facing (and of the stick) zeroes the
        // wanted speed: the walk halts at the edge without a clip (drops of 2–5 m; deeper ones get the ledge stop).
        let held = g.oneshot.is_none()
            && (edge_halt
                || (g.ledge_stop_lock.is_some() && edge_report_drop(body.feet, forward, LEDGE_STOP_REACH, FRONT_COS, LEDGE_STOP_DROP, &guidance, &collision).is_some()));
        let (delta, speed) = if held {
            g.speed_param = 0.0;
            g.blend.speed_param = 0.0;
            (Vec3::ZERO, 0.0)
        } else {
            (delta, speed)
        };
        let before = body.feet;
        let mut r = { let b = &mut *body; collision.move_capsule(&mut b.proxy, b.feet, delta, true, dt) };
        // PORT: the run stop's root motion (≈0.5 m of slide) does not carry the character off a roof edge; it stops at
        // the last supported position. The game's guard for this is not traced (LIVE: run stop next to an edge).
        if stopping && collision.ground_support(r.position).is_none() {
            r.position = before;
        }
        // Running into a wall does not lean: ObstacleCollision (event 42) is only sent from the interpreter's jump
        // branch behind a flag the jump-target scorer never sets, so the player cannot reach it in v1.02 (RE/02 §4.5).
        // The state itself (`collide`) is kept as decoded; nothing in the port enters it.
        if let Some(ls) = g.ledge_stop {
            // PORT: the stop clip's root motion may not carry the feet past the edge (the game places the edge
            // report so the clip ends on it; its sender is not traced)
            let past = (r.position - ls.point).dot(ls.normal) + LEDGE_STOP_MARGIN;
            if past > 0.0 {
                r.position -= Vec3::new(ls.normal.x, 0.0, ls.normal.z) * past;
            }
        }
        // the controller's velocity is what it actually moved (blocked by a wall → slower, sliding → along it)
        let moved = Vec3::new(r.position.x - before.x, 0.0, r.position.z - before.z) / dt.max(1e-4);
        body.velocity = if moved.length() < speed { moved } else { forward * speed };
        body.feet = r.position;

        // stick to ground (0x57D240): the capsule set on the support up to 0.37 m above / 0.58 m below the feet. In
        // Movement the only way off the ground is the fall-off rule (`HumanGround__State1_Update` 0xDB46B0 →
        // `Human__ShouldFallOffSupport` 0xB23CB0): no contact flatter than 45° and no floor 0.8 m below, i.e. the
        // capsule's rounded bottom has rolled off the rim (its centre ≈ r·sin 45° = 0.28 m past the edge). The edge-line
        // ground loss (`ground_loss`, 0xD87720) is gated by GroundData+760, which only the fight's grabbed reaction sets.
        match collision.ground_support(body.feet) {
            Some(s) => {
                g.snap_residual = body.feet.y - s.y;
                body.feet.y = s.y;
                body.grounded = true;
            }
            None => {
                body.grounded = false;
                // `HumanGround__TransitionToInAirOffSupport` 0xD8ADB0: InAir kind 3 (sub-state 3 keeps the current clip
                // for its remaining length, 0xE00EF0, then the fall), no drop report, so no drop sub-state or drop steer.
                // PORT: the animator still enters the fall through `walk_to_fall` / `run_to_fall` chosen by name.
                let entry = InAirEntry::Fall { from: body.feet, velocity: body.velocity, origin: FallOrigin::Ground, speed_param: g.speed_param };
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
            }
        }
    }
}

/// The drop report of `IHuman` vt104 (`Human__ReportDropAtFeet` 0xB248B0, called with a 0.5 m minimum): the nearest
/// LedgeGrab edge in a 0.75 m sphere zone around the feet (|dz| < 0.3) whose drop beyond it is at least the minimum,
/// or, with no such edge, the drop straight under the feet. Returns (signed horizontal distance from the feet to the
/// edge, negative once past it; the edge's outward normal, `None` when there was no edge).
pub fn drop_report(feet: Vec3, min_drop: f32, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<(f32, Option<Vec3>, f32)> {
    use crate::guidance::GuidanceSubType;
    let mut best: Option<(f32, f32, Vec3, f32)> = None; // (distance to the zone centre, signed distance, normal, drop)
    for e in guidance.edges.iter().filter(|e| e.subtype == GuidanceSubType::LedgeGrab) {
        let q = e.closest_point(feet);
        if (q.y - feet.y).abs() >= 0.3 {
            continue;
        }
        let flat = Vec3::new(q.x - feet.x, 0.0, q.z - feet.z);
        let d = flat.length();
        if d > 0.75 {
            continue;
        }
        let n = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
        if n == Vec3::ZERO {
            continue;
        }
        // drop measure 0xB19620: from just past the edge down to the first floor (≤ 7 m)
        let drop = collision.floor_height_below(q + n * 0.05 + Vec3::Y * 0.01, 7.0).map_or(7.0, |h| q.y - h);
        if drop < min_drop {
            continue;
        }
        let signed = if n.dot(feet - q) > 0.0 { -d } else { d };
        if best.is_none_or(|b| d < b.0) {
            best = Some((d, signed, n, drop));
        }
    }
    if let Some((_, s, n, drop)) = best {
        return Some((s, Some(n), drop));
    }
    // no edge: the drop straight under the feet (flag +56)
    let drop = collision.floor_height_below(feet + Vec3::Y * 0.01, 7.0).map_or(7.0, |h| feet.y - h);
    (drop >= min_drop).then_some((0.0, None, drop))
}

/// `HumanGround__CheckGroundLoss` 0xD87720: with the drop report (vt104, minimum 0.5 m), the ground is lost once the
/// feet are on or past the edge (signed distance < 0.01) and either there was no edge (a drop right under the feet) or
/// the horizontal velocity does not point back from the edge (dot(normal, v) ≥ 0). So the fall starts at the edge
/// line, not when the capsule's rounded bottom slides off the rim. Its guard (`HumanGroundData__IsGrabbedGroundLoss`
/// 0xC7F150, GroundData+760) is only set while grabbed in a fight (`HumanGround__Fight_EnterGrabbed` 0xD83B80), so
/// normal movement never uses it; the fight is not ported. (The cached report of Human+2801 is not modelled.)
#[allow(dead_code)]
pub fn ground_loss(feet: Vec3, velocity: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> bool {
    let Some((dist, n, _)) = drop_report(feet, 0.5, guidance, collision) else { return false };
    if dist >= 0.01 {
        return false;
    }
    let v = Vec3::new(velocity.x, 0.0, velocity.z).normalize_or_zero();
    n.is_none_or(|n| n.dot(v) >= 0.0)
}

/// Ground → Climb / Ledge / jump-to-ledge when pushing into a wall with high profile + Legs.
/// Order (hypothesis from the interpreter's request order, RE/01 §6.3):
/// 1. climb start: hand holds 1.8–2.4 m up and foot holds 1.2 m below them (vt764/768, FromGround);
/// 2. a ledge with the hands 0.7–3.0 m up → the standing straight jump at it (0xB21DA0 bands: knee / waist
///    heights pull straight up onto the top, higher ones end hanging).
fn try_wall_grab(feet: Vec3, forward: Vec3, foot_right: bool, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<TransitionSetup> {
    // The game's probe (input handler 0xEE65A0 → IHuman vt132 / vt136): guidance within 0.75 m of the character's
    // position (box height 0.45), front hemisphere (cone π about the facing). The edge must face the character.
    let reach = |h: f32| {
        guidance
            .probe(feet + Vec3::Y * h, GRAB_PROBE_RADIUS, 0.225, Some(forward), std::f32::consts::FRAC_PI_2)
            .filter(|hit| (hit.point - feet).dot(forward) > 0.0)
    };
    // 1. climb start
    for h in [1.8f32, 2.4] {
        if let Some(hand) = reach(h) {
            if hand.point.y - feet.y < 1.5 {
                continue;
            }
            let foot = guidance.probe(hand.point - Vec3::Y * CLIMB_ROW * CLIMB_HAND_ROWS as f32, 0.2, 0.15, Some(forward), 0.785);
            if let Some(foot) = foot {
                return Some(TransitionSetup::ToClimb(ClimbEntry {
                    entry_type: ClimbEntryType::FromGround,
                    hand_l: hand.point,
                    hand_r: hand.point,
                    foot_l: foot.point,
                    foot_r: foot.point,
                    normal: hand.wall_normal,
                    from_feet: feet,
                    foot_right,
                    action: None,
                }));
            }
        }
    }
    // 2. a ledge whose hands are 0.53–3.0 m above the feet (guard 0xD84190): the standing straight jump at it
    //    (HumanGround 0xD85550 → Human__SetupJumpToHandTarget 0xB21DA0), band by the hand height
    let target = straight_hand_target(feet, forward, guidance, collision, false)?;
    Some(TransitionSetup::ToInAir(InAirEntry::JumpToTarget { from: feet, target, speed_param: 0.0, foot_left: true }))
}

/// The hand target of a standing straight jump (0xB21DA0): a ledge in the 0.75 m grab probe whose hands are
/// 0.53–3.0 m above the feet; `beam` selects the `beam_jumpstraight_*` flights (jump from the beam impulsion).
pub fn straight_hand_target(feet: Vec3, forward: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld, beam: bool) -> Option<JumpTarget> {
    let reach = |h: f32| {
        guidance
            .probe(feet + Vec3::Y * h, GRAB_PROBE_RADIUS, 0.225, Some(forward), std::f32::consts::FRAC_PI_2)
            .filter(|hit| (hit.point - feet).dot(forward) > 0.0)
    };
    let hand = [0.6f32, 0.9, 1.3, 1.7, 2.1, 2.5, 2.9]
        .into_iter()
        .filter_map(reach)
        .find(|h| (GRAB_MIN_HEIGHT..=STRAIGHT_JUMP_MAX).contains(&(h.point.y - feet.y)))?;
    let n = hand.wall_normal;
    // both hands on the edge (not past its end)
    let point = guidance.fit_hands(hand.point, n);
    let wall = super::ledge::hang_type_at(point, n, collision) == super::ledge::LedgeHangType::Wall;
    let dz = point.y - feet.y;
    let j = if beam { super::ledge_moves::hang_jump_in_beam(dz, wall)? } else { super::ledge_moves::hang_jump_in(dz, wall)? };
    Some(JumpTarget { position: point + n * j.out - Vec3::Y * j.down, type_flags: j.flags, hang: Some((point, n)), straight: Some(j), pass: None, ladder: None })
}

/// Pull-down type Wait (1) from Movement (0xDB1470 event 70 → fill 0xD843E0 → PullDown_Enter 0xDDE4D0): a
/// LedgeGrab edge at the feet within 0.8 m ahead whose outward normal points along the facing (dot > 0),
/// with more than 2.0 m of drop below it (edge report +48).
fn try_pulldown(feet: Vec3, forward: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<super::ledge::LedgeEntry> {
    let (p, n) = edge_report(feet, forward, 0.5, 0.0, guidance, collision)?;
    pulldown_entry(p, n, feet, true, guidance, collision)
}

/// Grab / climb probe radius around the character (0xEE65A0: IHuman vt136 radius 0.75).
const GRAB_PROBE_RADIUS: f32 = 0.75;
/// Event 68 guard 0xD84190: the edge must be at least 0.53 m above the feet.
const GRAB_MIN_HEIGHT: f32 = 0.53;

/// The ledge stop's edge distance: the interpreter passes 0.0225 (= 0.15²) to `ClassifyEdgeSide` (vt744, 0xEE8899).
const LEDGE_STOP_REACH: f32 = 0.15;
/// Event 69 is only sent for a drop of more than 5 m (0xEE88A9).
const LEDGE_STOP_DROP: f32 = 5.0;
/// The interpreter's low-profile edge halt: within 0.16 m, normal within 70 deg (0xEE7C1A / 0xEE7C5C).
const EDGE_HALT_REACH: f32 = 0.16;
const EDGE_HALT_COS: f32 = 0.342_020_1;
/// PORT: the feet stay this far behind the edge during the ledge stop.
const LEDGE_STOP_MARGIN: f32 = 0.05;
/// ClassifyEdgeSide 0xD9D7F0: front = within 60° (120° with the report flag +68).
const FRONT_COS: f32 = 0.5;

/// An edge report for a front edge (`HumanGround__ClassifyEdgeSide` 0xD9D7F0 → 1): a LedgeGrab edge within
/// `reach` ahead whose outward normal is within 60° of the facing, with more than 2.0 m of drop
/// (guards 0xDA5DE0 / 0xD9D6C0; the classifier itself needs > 1.3 m). Returns (point, outward normal).
fn edge_report(feet: Vec3, forward: Vec3, reach: f32, min_cos: f32, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<(Vec3, Vec3)> {
    edge_report_drop(feet, forward, reach, min_cos, 2.0, guidance, collision).map(|r| (r.0, r.1))
}

/// `edge_report` with the drop limit and the drop: a front LedgeGrab edge whose closest point is within `reach` of the
/// feet horizontally. Returns (point, outward normal, drop).
fn edge_report_drop(feet: Vec3, forward: Vec3, reach: f32, min_cos: f32, min_drop: f32, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<(Vec3, Vec3, f32)> {
    let hit = guidance.probe(feet + forward * reach, reach, 0.2, None, std::f32::consts::PI)?;
    if Vec2::new(hit.point.x - feet.x, hit.point.z - feet.z).length() > reach + 1e-3 {
        return None;
    }
    let n = hit.wall_normal;
    let nf = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    if nf.dot(forward) <= min_cos {
        return None;
    }
    let below = collision.ground_height(hit.point + n * 0.6 - Vec3::Y * 0.05, 50.0).unwrap_or(hit.point.y - 100.0);
    let drop = hit.point.y - below;
    (drop > min_drop).then_some((hit.point, n, drop))
}

/// An edge to look down at while standing: a LedgeGrab edge within 0.6 m of the feet, not behind the character
/// (|angle| ≤ 90°), with more than 2 m of drop beyond it (the ledge stop's report rules).
fn look_down_edge(feet: Vec3, forward: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<(Vec3, Vec3)> {
    let hit = guidance.probe(feet, 0.6, 0.2, None, std::f32::consts::PI)?;
    let n = hit.wall_normal;
    let nf = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    if nf.dot(forward) < -0.05 {
        return None;
    }
    let below = collision.ground_height(hit.point + n * 0.6 - Vec3::Y * 0.05, 50.0).unwrap_or(hit.point.y - 100.0);
    (hit.point.y - below > 2.0).then_some((hit.point, n))
}

/// Pull-down from the ground at edge `p` / normal `n`: type Wait (from Movement) or EdgeStop (from the ledge stop).
fn pulldown_entry(p: Vec3, n: Vec3, feet: Vec3, wait: bool, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<super::ledge::LedgeEntry> {
    let moves = super::ledge_moves::pulldown(p, n, feet, wait, guidance, collision)?;
    Some(pulldown_ledge_entry(moves, n, feet))
}

/// The Ledge entry (SubState 11 PullDown, `PullDown_Enter` 0xDDE4D0) that plays the pull-down's three stages.
pub(super) fn pulldown_ledge_entry([orient, descent, reception]: [super::ledge_moves::LedgeMove; 3], n: Vec3, feet: Vec3) -> super::ledge::LedgeEntry {
    let mut e = super::ledge::LedgeEntry::at((orient.hand_l + orient.hand_r) * 0.5, n, feet, super::ledge::LedgeSubState::PullDown);
    e.hand_l = orient.hand_l;
    e.hand_r = orient.hand_r;
    e.entry_move = Some(orient);
    e.entry_rest = [Some(descent), Some(reception)];
    e
}

trait AnyHit {
    fn any_hit(self, f: impl Fn(f32) -> bool) -> bool;
}
impl AnyHit for std::ops::RangeInclusive<f32> {
    fn any_hit(self, f: impl Fn(f32) -> bool) -> bool {
        let (a, b) = (*self.start(), *self.end());
        (0..=5).any(|i| f(a + (b - a) * i as f32 / 5.0))
    }
}
