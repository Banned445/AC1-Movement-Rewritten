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
    /// Landing history for diagnostics; activation effects are owned by `pending_landing`.
    pub last_landing: Option<Landing>,
    /// Entry event, consumed once by the first Ground update and replaced on every entry.
    pending_landing: Option<Landing>,
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
    pub extra: Option<super::ground_extras::ExtraAction>,
    pub crowd_avoid: Option<Vec3>,
    pub crouch: Option<super::crouch::Crouch>,
    /// Data+284 and HG+1784, set by the social helper bit-5 interpreter branch (0xEE7048).
    /// PORT boundary: the producer of that social status remains under RE. No invented crouch button.
    pub crouch_requested: bool,
    pub crouch_hide_hint: bool,
    /// Input timer +4408, reset every frame Legs is held (0xEEE08C).
    pub legs_released_ago: f32,
    /// The transition action playing in front of the locomotion or the wait (MoveBlend's transition path, RE/02 §4.6).
    pub tr: Option<super::ground_tree::MoveTransition>,
    pub tr_seq: u32,
    /// The Movement state that owns the playing one-shot (its update decides what follows it, RE/02 §4.6).
    pub owner: TreeState,
    /// HG+688 (0x2B0): the pivot's turn angle, read by the turn start's layouts 2 / 3.
    pub pivot_turn: f32,
    /// The foot column of Idle's wait (`HumanGround__PickLocomotionFoot` on the item Idle was entered from: 0 left
    /// ahead, 1 right ahead, 2 parallel), RE/02 §4.6.
    pub wait_foot: usize,
}

/// The Movement child states that end a one-shot by their own rules (`HumanGround__Movement_Update` 0xDAD1C0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TreeState {
    #[default]
    Other,
    /// State 21 (entry mode 9, `HumanGround__Landing_Update` 0xDA8F50).
    Landing,
    /// State 22 (entry mode 10, `HumanGround__DamageLanding_Update` 0xDA8FC0).
    DamageLanding,
    /// State 18 (`HumanGround__RunStop_Update` 0xDA8D70).
    RunStop,
    /// State 24, the skid turn (`HumanGround__RunTurn_Update` 0xDA90A0).
    RunTurn,
    /// State 25 (`HumanGround__Pivot_Update` 0xDA9120).
    Pivot,
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

// Probe action list also includes the native Ground extras; kept here for the existing dump tool.
pub const PIVOT_ACTIONS: [u32; 37] = [
    super::ground_extras::FREERUN_WAIT[0], super::ground_extras::FREERUN_WAIT[1],
    super::ground_extras::STATIC_PREPARE[0], super::ground_extras::STATIC_PREPARE[1],
    super::ground_extras::STATIC_JUMP[0], super::ground_extras::STATIC_JUMP[1], super::ground_extras::STATIC_FALL,
    super::ground_extras::CROUCH_WAIT[0][0], super::ground_extras::CROUCH_WAIT[0][1],
    super::ground_extras::CROUCH_WAIT[1][0], super::ground_extras::CROUCH_WAIT[1][1],
    super::ground_extras::CROUCH_START[0], super::ground_extras::CROUCH_START[1],
    super::ground_extras::CROUCH_WALK, super::ground_extras::CROUCH_PIVOT,
    super::ground_extras::CROUCH_HIDE_ENTER[0], super::ground_extras::CROUCH_HIDE_ENTER[1],
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
        self.owner = TreeState::Other;
    }

    fn next_tr_seq(&mut self) -> u32 {
        self.tr_seq = self.tr_seq.wrapping_add(1);
        self.tr_seq
    }

    /// RunStop (state 18, `HumanGround__RunStop_Enter` 0xD98E30): the stop action of the leading foot (table 0x1A2C074 =
    /// legacy 86 / 87), weighted by the speed it stopped from. MoveBlend does not run while it plays.
    pub fn start_run_stop(&mut self) {
        let foot = (self.blend.foot != 0) as usize;
        if let Some(b) = jump_blend::action_items(jump_blend::RUN_STOP[foot]).map(|_| ActionBlend::new(jump_blend::RUN_STOP[foot], 0, &jump_blend::run_stop_weights(self.speed_param))) {
            self.play_oneshot(b);
            self.oneshot_next = None;
            self.tr = None;
            self.owner = TreeState::RunStop;
            self.speed_param = 0.0;
            self.blend.speed_param = 0.0;
        }
    }

    /// The transition action `action` (its item `item`) in front of the locomotion, handing over to locomotion item
    /// `dest`. `prev`: the weights the slot holds (the item that was playing), kept when the transition has as many
    /// clips (the slot's single weight array, `sub_725B30`).
    fn start_transition(&mut self, action: u32, item: usize, dest: usize, layout: u8, prev: &[f32], from: Option<(u32, usize)>) -> bool {
        let Some(items) = jump_blend::action_items(action) else { return false };
        let n = items.get(item).map(|c| c.len()).unwrap_or(0);
        if n == 0 {
            return false;
        }
        let mut w: Vec<f32> = if prev.len() == n { prev.to_vec() } else { (0..n).map(|k| if k == 0 { 1.0 } else { 0.0 }).collect() };
        // the layout's weights from the first frame (the exe sets them in the same update that plays the action)
        if let Some(lw) = super::ground_tree::transition_weights(layout, self.speed_param, self.pivot_turn, &w) {
            w = (0..n).map(|k| lw.get(k).copied().unwrap_or(0.0).clamp(0.0, 1.0)).collect();
        }
        let seq = self.next_tr_seq();
        self.tr = Some(super::ground_tree::MoveTransition::new(ActionBlend::new(action, item, &w), layout, dest, from, seq));
        true
    }

    /// `HumanGround__Move_EnterBody` 0xD94A60 entered from `from` without a start: the locomotion through the item's
    /// authored transition (the graph's lookup, `sub_5B98B0`), with the given MoveBlend layout. Without one the
    /// locomotion starts at once on the exit foot of the item (`HumanGround__PickLocomotionFoot` 0xD86760).
    fn enter_move(&mut self, from: ActionBlend, layout: u8) {
        use super::ground_tree::{authored_transition, pick_locomotion_foot};
        match authored_transition(from.id, from.item, super::move_blend::ACT_GROUND_LOCOMOTION) {
            Some((ta, ua, ub)) if self.start_transition(ta, ua, ub, layout, from.weights(), Some((from.id, from.item))) => {}
            _ => {
                self.tr = None;
                self.blend.foot = pick_locomotion_foot(super::anim_gate::word(&from), self.blend.foot).min(1);
                self.blend.phase = 0.0;
            }
        }
    }

    /// Idle entered from `from` (`HumanGround__Idle_Enter` 0xDA6AB0): the wait of the profile, through the item's authored
    /// transition into it when there is one (its root motion still moves the body); the foot follows that wait.
    fn enter_wait(&mut self, from: ActionBlend, high: bool) {
        self.speed_param = 0.0;
        self.blend.speed_param = 0.0;
        self.tr = None;
        let waits = super::ground_tree::WAITS[high as usize];
        // Idle's wait column: the exit foot of the item it is entered from (0xD94580 → 0xD86760)
        self.set_wait_foot(super::ground_tree::pick_locomotion_foot(super::anim_gate::word(&from), self.blend.foot));
        let parallel = super::ground_tree::WAITS_PARALLEL[high as usize];
        let found = super::jump_clips::GROUND_TRANSITIONS.iter().find(|t| t.0 == from.id && t.1 == from.item && (waits.contains(&t.4) || t.4 == parallel));
        if let Some(&(_, _, ta, ua, dest, _)) = found {
            if self.start_transition(ta, ua as usize, 0, 0, from.weights(), Some((from.id, from.item))) {
                if let Some(t) = self.tr.as_mut() {
                    t.to_wait = true;
                }
                self.set_wait_foot(if dest == parallel { 2 } else { (dest == waits[1]) as usize });
            }
        }
    }

    /// Idle's wait column; a foot ahead is also the leading foot the next start uses.
    pub fn set_wait_foot(&mut self, foot: usize) {
        self.wait_foot = foot.min(2);
        if foot < 2 {
            self.blend.foot = foot;
        }
    }

    /// `HumanGround__PlayStartMove` 0xD98990 (Idle → Move): the locomotion entered through the start transition of the
    /// profile and leading foot (0.2 s AROLLBROLL from B only), weights walk (low) / jog (high), the speed parameter
    /// set to 0.25 / 0.5 and layout 7. A left foot ahead hands over to locomotion item 1.
    pub fn play_start_move(&mut self) {
        let foot = (self.blend.foot != 0) as usize;
        let id = START_MOVE[self.high_profile as usize][foot];
        if jump_blend::action_items(id).is_none() {
            return;
        }
        let w: &[f32] = if self.high_profile { &[0.0, 0.0, 1.0, 0.0] } else { &[0.0, 1.0, 0.0, 0.0] };
        self.speed_param = if self.high_profile { 0.5 } else { 0.25 };
        self.blend.speed_param = self.speed_param;
        if !GAME_GROUND_TREE {
            self.play_oneshot(ActionBlend::new(id, 0, w));
            return;
        }
        let seq = self.next_tr_seq();
        let mut t = super::ground_tree::MoveTransition::new(ActionBlend::new(id, 0, w), 7, 1 - foot, None, seq);
        t.blend_in = Some(crate::assets::ac_actions::ActBlend { kind: 1, disp_src: 2, time: 0.2, ..crate::assets::ac_actions::ActBlend::DEFAULT });
        self.tr = Some(t);
    }

    /// RunTurn → Move (`HumanGround__PlayRunTurnExitMove` 0xD8F2B0), s := 0.5. Still turned (more than 120 deg): the
    /// heading is turned round (the skid-turn clip turned the skeleton, not the root) and the turn's own exit plays
    /// (legacy 96 / 97); otherwise the run stop's exit (93 after the left-foot turn, 92 after the right) with a 0.2 s
    /// ASTOPBROLL.
    pub fn run_turn_exit(&mut self, turn_item: ActionBlend, turned: bool, heading: &mut f32) {
        use super::ground_tree::{RUN_STOP_EXIT, RUN_TURN, RUN_TURN_EXIT};
        let footl = turn_item.id == RUN_TURN[0];
        self.speed_param = 0.5;
        self.blend.speed_param = 0.5;
        let (action, dest) = if turned {
            *heading = wrap_angle(*heading + std::f32::consts::PI);
            if footl { (RUN_TURN_EXIT[0], 0) } else { (RUN_TURN_EXIT[1], 1) }
        } else if footl {
            (RUN_STOP_EXIT[1], 0)
        } else {
            (RUN_STOP_EXIT[0], 1)
        };
        if self.start_transition(action, 0, dest, 1, &[], Some((turn_item.id, turn_item.item))) && !turned {
            if let Some(t) = self.tr.as_mut() {
                t.blend_in = Some(crate::assets::ac_actions::ActBlend { kind: 2, time: 0.2, ..crate::assets::ac_actions::ActBlend::DEFAULT });
            }
        }
    }

    /// What follows a ground one-shot that ends, by the Movement state that owns it (RE/02 §4.6). `stick`: a move is
    /// wanted (HG+1496 ≠ 0); `turn`: HG+1532.
    pub fn after_oneshot(&mut self, ended: ActionBlend, stick: bool, turn: f32, heading: &mut f32) {
        use super::ground_tree::{pick_locomotion_foot, LANDINGS_TO_WAIT, LANDING_DAMAGE, RUN_STOP_EXIT, RUN_TURN};
        let owner = std::mem::take(&mut self.owner);
        let high = self.high_profile;
        if std::env::var_os("AC_TREE_LOG").is_some() {
            info!("ground tree: {:#010x} ended, owner {owner:?}, stick {stick}, turn {:.0} deg, high {high}", ended.id, turn.to_degrees());
        }
        match owner {
            // state 21 (0xDA8F50): when the landing completes, the landings that end in the wait go to Idle (high
            // profile, 0xDA26C0); the others enter Move with layout 6 and s 0.25 / 0.5 / 1.0 by the wanted profile and
            // Sprint (0xD990A0). A released stick then leaves Move for Idle at once (0xD7ED30).
            TreeState::Landing if ended.id == jump_blend::RECEPTION_FREESTEP[0] || ended.id == jump_blend::RECEPTION_FREESTEP[1] => {
                // PORT: the game continues a free-step reception in NarrowObject; its exits lead to the same waits and
                // locomotion. The exit into the locomotion (`freestep_entry_tr_{l_walk, h_jog, h_sprint_impultion}`)
                // plays at the band of the profile and Sprint, as the landings do (layout 5, the three-clip pair):
                // live 2026-10-08 the high-profile run played the jog (797 ms) and free run the sprint (516 / 532 ms).
                if stick {
                    self.speed_param = if !high { 0.25 } else if !self.sprint { 0.5 } else { 1.0 };
                    self.blend.speed_param = self.speed_param;
                    self.enter_move(ended, 5);
                } else {
                    self.enter_wait(ended, high);
                }
            }
            TreeState::Landing if LANDINGS_TO_WAIT.contains(&ended.id) => self.enter_wait(ended, high),
            TreeState::Landing if stick => {
                self.speed_param = if !high { 0.25 } else if !self.sprint { 0.5 } else { 1.0 };
                self.blend.speed_param = self.speed_param;
                self.enter_move(ended, 6);
            }
            // state 22 (0xDA8FC0): Idle (0xDA77F0) or Move with s 0.25 / 0.5 by profile and layout 1 (0xD7F210)
            TreeState::DamageLanding if stick => {
                self.speed_param = if high { 0.5 } else { 0.25 };
                self.blend.speed_param = self.speed_param;
                self.enter_move(ended, 1);
            }
            TreeState::DamageLanding => {
                let _ = LANDING_DAMAGE;
                self.enter_wait(ended, high);
            }
            // state 18 (0xDA8D70): the item ended with the stick more than 120 deg away → the skid turn (state 24,
            // 0xD8B8E0, the turn of the exit foot); else Idle (0xDA7710) or Move through legacy 92 / 93 with s 0.5
            // (0xD8B3F0: the left-foot stop hands over to item 1)
            TreeState::RunStop if stick && turn.abs() > crate::tuning::RUN_TURN_ANGLE => {
                let foot = pick_locomotion_foot(super::anim_gate::word(&ended), self.blend.foot).min(1);
                if jump_blend::action_items(RUN_TURN[foot]).is_some() {
                    self.play_oneshot(ActionBlend::new(RUN_TURN[foot], 0, &[1.0]));
                    self.owner = TreeState::RunTurn;
                }
            }
            TreeState::RunStop if stick => {
                let footl = ended.id == jump_blend::RUN_STOP[0];
                self.speed_param = 0.5;
                self.blend.speed_param = 0.5;
                let (action, dest) = if footl { (RUN_STOP_EXIT[0], 1) } else { (RUN_STOP_EXIT[1], 0) };
                self.start_transition(action, 0, dest, 1, &[], Some((ended.id, ended.item)));
            }
            // state 24 completed (0xDA90A0)
            TreeState::RunTurn if stick => self.run_turn_exit(ended, turn.abs() > crate::tuning::RUN_TURN_ANGLE, heading),
            TreeState::RunTurn => {
                // 0xD8F210: the heading turned round, Idle
                *heading = wrap_angle(*heading + std::f32::consts::PI);
                self.enter_wait(ended, high);
            }
            // state 25 (0xDA9120) → `HumanGround__PlayTurnStartMove` 0xD99710: the locomotion through the pivot's
            // authored transition; high profile: layout 2, s 0.5; low: layout 3, s 0.25 (0.5 with the high profile
            // wanted, at the faster rate). The pivot's root yaw has already turned the body (the exe catches CurHeading
            // up by HG+688 here).
            TreeState::Pivot if stick => {
                let layout = if high { 2 } else { 3 };
                self.speed_param = if high { 0.5 } else { 0.25 };
                self.blend.speed_param = self.speed_param;
                self.enter_move(ended, layout);
            }
            TreeState::Pivot | TreeState::RunStop | TreeState::Landing => self.enter_wait(ended, high),
            // the other one-shots (climb / ledge / hay exits, lean exits …): the item's authored transition into the
            // locomotion with the default layout, PORT s ≥ 0.25 / 0.5 (TransitionSetupDataToMovement modes 3 / 4, the
            // exits' own setups are not traced), or into the wait
            // (the ledge stop's end action hands back to Movement when it ends, `Locomotion_Update` 0xDAF2D0)
            TreeState::Other if self.ledge_stop.is_none_or(|l| l.ending) && self.collide.is_none() && self.look_down.is_none() => {
                if stick {
                    if super::ground_tree::authored_transition(ended.id, ended.item, super::move_blend::ACT_GROUND_LOCOMOTION).is_some() {
                        self.speed_param = self.speed_param.max(if high { 0.5 } else { 0.25 });
                        self.blend.speed_param = self.speed_param;
                        self.enter_move(ended, 1);
                    }
                } else {
                    self.enter_wait(ended, high);
                }
            }
            TreeState::Other => {}
        }
    }

    /// `TransitionSetupDataToMovement::Apply` (0xC80310), simplified.
    pub fn enter(&mut self, landing: Option<Landing>) {
        self.pending_landing = landing;
        self.sub_state = HumanGroundSubState::Movement;
        self.ledge_stop = None;
        self.collide = None;
        self.look_down = None;
        self.extra = None;
        self.crouch = None;
        self.oneshot_next = None;
        self.tr = None;
        // entry mode 9 (landing) / 10 (heavy landing) put Movement in states 21 / 22 (`Movement_EnterBody` 0xD92C40)
        self.owner = match landing.and_then(|l| l.action) {
            Some(a) if a.id == jump_blend::LAND_DAMAGE || a.id == jump_blend::LAND_DAMAGE_ROLL => TreeState::DamageLanding,
            Some(_) => TreeState::Landing,
            None => TreeState::Other,
        };
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
            // into locomotion continues at the retained ground speed. No native heavy-damage speed reset.
            if !GAME_FALLS && l.kind == LandingType::HeavyDamage {
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

pub fn heading_delta(a:f32,b:f32)->f32 { angle_diff(a,b) }

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
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle, Option<&crate::anim::AnimPlayer>), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (mut loco, mut body, mut data, anim) in &mut q {
        let g = &mut data.ground;
        if loco.current != ActorContextId::Ground {
            continue;
        }
        if loco.just_switched {
            // AIActor::Update skips a context's first update after it was switched in.
            loco.just_switched = false;
            if let Some(l) = g.pending_landing.take() {
                // drop > 3 m: camera shake (drop − 3) / 7 (0xE05940)
                if l.total_drop > 3.0 {
                    rig.shake = ((l.total_drop - 3.0) / 7.0).min(1.0);
                }
                if l.kind == LandingType::Fatal {
                    // PORT: immediate respawn until native fatal ragdoll/Dead and desynchronisation are implemented.
                    body.feet = spawn.0;
                    body.velocity = Vec3::ZERO;
                }
            }
            // the controller keeps integrating the velocity the switch left (no one-frame freeze)
            body.grounded = true;
            super::coast(&mut body, &collision, dt);
            continue;
        }

        if GAME_GROUND_EXTRAS {
            if g.crouch.is_none() && g.crouch_requested && !pad.high_profile && g.extra.is_none()
                && g.oneshot.is_none() && g.collide.is_none() && g.ledge_stop.is_none()
            {
                g.pose_seq=g.pose_seq.wrapping_add(1);
                g.crouch=Some(if pad.speed01>0.0 && g.speed_param>0.0 {
                    super::crouch::Crouch::enter_moving(g.blend.foot,g.crouch_hide_hint,g.pose_seq)
                } else {super::crouch::Crouch::enter(g.blend.foot,g.crouch_hide_hint,g.pose_seq)});
                g.look_down=None;g.speed_param=0.0;g.blend.speed_param=0.0;
            }
            if let Some(mut crouch)=g.crouch {
                let (item_clock,blend_progress)=if let Some(anim)=anim {
                    (Some((anim.contacts.t,anim.phase)),
                        (anim.fade<anim.fade_time && anim.fade_time>0.0).then(||anim.fade/anim.fade_time))
                } else { (Some((crouch.time,crouch.phase())),None) };
                let moving=pad.magnitude>0.35;
                let exit=super::anim_gate::allows_mode_exit(&crouch.action,moving,pad.high_profile);
                let input=super::crouch::CrouchInput{request:g.crouch_requested,hide:g.crouch_hide_hint,
                    high:pad.high_profile,moving,want_heading:if moving {heading_of(pad.dir)} else {body.heading},
                    can_stand:super::ground_extras::can_stand(body.feet,&collision),anim_allows_exit:exit,
                    blend_progress,item_clock};
                let wait_high=matches!(crouch.mode,super::crouch::CrouchMode::Wait) && !moving && pad.high_profile;
                let mut heading=body.heading;
                let delta=crouch.update(input,g.blend.foot,&mut heading,dt);
                body.heading=heading;
                if wait_high && delta.is_some() {
                    // Wait -> FreeRun 0xDA9510: phase 1 starts on the impulsion wait item (0xD8B9D0).
                    g.pose_seq=g.pose_seq.wrapping_add(1);
                    let mut extra=super::ground_extras::ExtraAction::free_run(g.blend.foot,g.pose_seq);
                    extra.action.item=1;g.extra=Some(extra);g.crouch=None;body.proxy.height=None;
                } else if let Some(delta)=delta {
                    body.proxy.height=Some(1.0);g.crouch=Some(crouch);
                    let d=super::right_of(body.forward())*delta.x+body.forward()*delta.z;
                    let r={let b=&mut *body;collision.move_capsule(&mut b.proxy,b.feet,d,true,dt)};
                    body.feet=r.position;body.velocity=r.velocity;
                    if let Some(support)=collision.ground_support_height(body.feet,body.proxy.height.unwrap_or(CAPSULE_HEIGHT)) {
                        body.stick_residual=body.feet.y-support.y;body.stick_normal_y=Some(support.normal_y);
                        body.feet.y=support.y;body.grounded=true;
                    } else {
                        body.grounded=false;
                        let entry=InAirEntry::Fall{from:body.feet,velocity:body.velocity,origin:FallOrigin::Ground,speed_param:0.0};
                        switch_context(&mut loco,&mut data,TransitionSetup::ToInAir(entry));
                    }
                    continue;
                } else {g.crouch=None;body.proxy.height=None;}
            }
        }

        if GAME_GROUND_EXTRAS {
            g.legs_released_ago = if pad.legs_held { 0.0 } else { g.legs_released_ago+dt };
            // Event43: high profile, stationary, Legs held; native 0xEE8B93 checks button capture.
            // PORT: ability-stack/input-capture ownership is absent in the movement-only input adapter.
            if g.extra.is_none() && g.oneshot.is_none() && g.collide.is_none() && g.ledge_stop.is_none()
                && g.look_down.is_none() && pad.high_profile && pad.magnitude<=0.35 && pad.legs_held
            {
                g.pose_seq=g.pose_seq.wrapping_add(1);
                g.extra=Some(super::ground_extras::ExtraAction::free_run(g.blend.foot,g.pose_seq));
                g.speed_param=0.0;g.blend.speed_param=0.0;
            }
            if g.crouch_requested && !pad.high_profile {g.extra=None;}
            if let Some(mut extra)=g.extra {
                g.sub_state=HumanGroundSubState::FreeRun;
                g.high_profile=pad.high_profile;
                // 0xEE84CC: high, <=0.35 stick, Legs released after being held within the last 0.5 s.
                // 0x6D0F50 is the uncaptured held-button query, not the capture query.
                // Event47 takes a hand report first; event48 is the no-target fallback (0xEE84FE-0xEE858B).
                let request=pad.high_profile && !pad.legs_held && pad.magnitude<=0.35 && g.legs_released_ago<0.5;
                if request && extra.mode==super::ground_extras::ExtraMode::FreeRun && extra.elapsed+dt>=0.2 {
                    extra.target=static_hand_target(body.feet,body.forward(),&guidance,&collision);
                }
                let (out,delta)=extra.advance(dt,pad.magnitude>0.35,request,g.blend.foot);
                match out {
                    super::ground_extras::ExtraResult::Inactive=>{g.extra=None;},
                    super::ground_extras::ExtraResult::Jump{action,fall}=>{
                        pad.consume_jump();
                        let entry=if let Some(target)=extra.target {
                            InAirEntry::JumpToTarget{from:body.feet,target,speed_param:0.0,foot_left:g.blend.foot==0}
                        } else {InAirEntry::OnPlace { from:body.feet,fwd:body.forward(),
                            action:ActionBlend::new(action,0,&[1.0]),fall:Some(ActionBlend::new(fall,0,&[1.0])) }};
                        body.velocity=Vec3::ZERO;
                        switch_context(&mut loco,&mut data,TransitionSetup::ToInAir(entry));
                        continue;
                    },
                    super::ground_extras::ExtraResult::Stay=>{
                        g.extra=Some(extra);
                        let d=super::right_of(body.forward())*delta.x+body.forward()*delta.z;
                        let r={let b=&mut *body;collision.move_capsule(&mut b.proxy,b.feet,d,true,dt)};
                        body.feet=r.position;body.velocity=r.velocity;
                        if let Some(support)=collision.ground_support_height(body.feet,body.proxy.height.unwrap_or(CAPSULE_HEIGHT)) {
                            body.stick_residual=body.feet.y-support.y;body.stick_normal_y=Some(support.normal_y);
                            body.feet.y=support.y;body.grounded=true;
                        } else {
                            body.grounded=false;
                            let entry=InAirEntry::Fall{from:body.feet,velocity:body.velocity,origin:FallOrigin::Ground,speed_param:0.0};
                            switch_context(&mut loco,&mut data,TransitionSetup::ToInAir(entry));
                        }
                        continue;
                    },
                }
            }
        }

        // ---------------------------------------------------------------- input → wanted motion
        // The interpreter's wall slide (`GoAssassinActionInterpreter` 0xEE6B16 → 0xEDD7C0): a stick pushed into a
        // wall turns along it; pushed within 45 deg of straight into it, the wanted speed is 0 and the run stop allowed.
        // It changes what the interpreter hands the Ground (SetMoveDir / SetDestSpeedRatio); the raw stick keeps the
        // other requests.
        let (mv_dir, mv_speed) = if GAME_GROUND_TREE { wall_slide(&body, pad.dir, pad.speed01) } else { (pad.dir, pad.speed01) };
        // HG+1532: the signed angle from the current heading to the wanted one (Movement_PreUpdate 0xD97E30).
        let stick_turn = if mv_speed > 0.0 { angle_diff(heading_of(mv_dir), body.heading) } else { 0.0 };
        // A transition into the locomotion (the Move state's transition path) keeps steering and the other requests
        // live. A released stick leaves it like any Move: `HumanGround__Move_CanRunStop` 0xD7EC90 (high profile, the
        // run stop allowed, and the playing action is the locomotion, the skid-turn exits legacy 96 / 97, or past 0.33 s)
        // or else `HumanGround__Move_CanStop` 0xD7ED30 (the playing action is not the locomotion): the wait at once.
        if GAME_GROUND_TREE && mv_speed <= 0.0 {
            if let Some(t) = g.tr.filter(|t| !t.to_wait) {
                let elapsed = t.phase * t.action.duration();
                let can_run_stop = g.high_profile && g.speed_param > BAND_WALK && !super::anim_gate::locked(&t.action)
                    && (super::ground_tree::RUN_TURN_EXIT.contains(&t.action.id) || elapsed > 0.33);
                g.tr = None;
                if can_run_stop {
                    g.start_run_stop();
                } else {
                    g.set_wait_foot(super::ground_tree::pick_locomotion_foot(super::anim_gate::word(&t.action), g.blend.foot));
                    g.speed_param = 0.0;
                    g.blend.speed_param = 0.0;
                }
            }
        }
        // The anim gate of Idle → Move (0xD84AC0) and of the pivot (0xD84B10): the playing item has ended, or its word
        // allows leaving it for moving in this profile and it is not locked (`HumanGround__AnimAllowsModeExit`
        // 0xD80010, `anim_gate`). A one-shot that allows it is left at once for the locomotion (its transition blends,
        // RE/13 §7); locked ones (landings, run stops, pivots) play out. Not in the ground's own sub-states (the ledge
        // stop, sub-state 38; the obstacle collision, 5): they leave by their own rules. A transition into the wait is
        // Idle's: the same gate leaves it for the start.
        // The landing states (21, 22) and the skid turn (24) have no such gate: they leave when the action completes
        // (`sub_501760`). That includes the free-step reception (PORT: the game continues it in NarrowObject); its
        // authored exits (`freestep_entry_tr_*`, entered with a cut) start from its last frame.
        let gated = !GAME_GROUND_TREE || g.owner == TreeState::Other;
        if mv_speed > 0.0
            && gated
            && g.ledge_stop.is_none()
            && g.collide.is_none()
            && g.oneshot.is_some_and(|o| super::anim_gate::allows_mode_exit(&o.blend, true, pad.high_profile))
        {
            g.oneshot = None;
            g.oneshot_next = None;
            g.owner = TreeState::Other;
        }
        if mv_speed > 0.0 && g.tr.is_some_and(|t| t.to_wait && super::anim_gate::allows_mode_exit(&t.action, true, pad.high_profile)) {
            g.tr = None;
        }
        // The skid turn (state 24) is left for Move as soon as the stick asks to move within 120 deg again, or when it
        // completes (`HumanGround__Guard_RunTurnToMove` 0xD85390); its locked item does not hold it.
        if GAME_GROUND_TREE && g.owner == TreeState::RunTurn && mv_speed > 0.0 && stick_turn.abs() <= RUN_TURN_ANGLE {
            if let Some(os) = g.oneshot.take() {
                g.owner = TreeState::Other;
                g.run_turn_exit(os.blend, false, &mut body.heading);
            }
        }
        let busy = g.oneshot.is_some() || g.tr.is_some_and(|t| t.to_wait);
        let moving = mv_speed > 0.0 && !busy;
        let prev_high = g.high_profile;
        g.high_profile = pad.high_profile;
        g.sprint = pad.high_profile && pad.legs_held; // sprint = high profile + legs (RE/01 §6.2)
        g.sub_state = if !GAME_GROUND_EXTRAS && g.sprint { HumanGroundSubState::FreeRun } else { HumanGroundSubState::Movement };

        // turn attenuation (interpreter 0xEE65A0): beyond 45° slow down; forced to 1 in low profile
        let want_heading = if moving { heading_of(mv_dir) } else { body.heading };
        let off = angle_diff(want_heading, body.heading).abs();
        let atten_target = if !g.high_profile || off <= TURN_ATTEN_START {
            1.0
        } else {
            (1.0 - (off - TURN_ATTEN_START) / TURN_ATTEN_RANGE).max(TURN_ATTEN_FLOOR)
        };
        g.turn_atten = if GAME_GROUND_TREE {
            // exactly as the interpreter steps +4304 (0xEE6A34): at or below 1 − k it rises at 5/s up to 1, above it
            // falls at 10/s down to 0.1, so it hunts around the target instead of settling on it; low profile forces 1
            let k = ((off.min(std::f32::consts::FRAC_PI_2) - TURN_ATTEN_START) / TURN_ATTEN_RANGE).max(0.0);
            let a = if g.turn_atten <= 1.0 - k { (g.turn_atten + TURN_ATTEN_UP_RATE * dt).min(1.0) } else { (g.turn_atten - TURN_ATTEN_DOWN_RATE * dt).max(TURN_ATTEN_FLOOR) };
            if g.high_profile { a } else { 1.0 }
        } else if atten_target > g.turn_atten {
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
        let target = if mv_speed > 0.0 { (base + STICK_SPAN * mv_speed * g.turn_atten).min(1.0) } else { 0.0 };
        // stick released (desired mode HG+0x5D8 = 0): the game leaves the Move state instead of decelerating
        // (the curve at HG+0x63C only runs while still moving toward a slower band):
        // - jog or faster (HG+0x5DC, speed > 0.25) → RunStop 0xD98E30 (guard 0xD7EC90): the run-stop action by the
        //   leading foot, its root motion, then its settle into the wait;
        // - walk band → Idle 0xD8B220 (guard 0xD7ED30): the wait with a 0.2 s blend, i.e. stopped at once.
        // Stick released while a landing / reception plays: its transition leads into the wait, not into the
        // locomotion (the action's exits, RE/13), so no run stop follows it. Before, the speed kept from the jump
        // started a run stop once the landing ended: a second slide after the landing.
        if mv_speed <= 0.0 && g.speed_param > 0.0 && g.oneshot.is_some_and(|o| !jump_blend::RUN_STOP.contains(&o.blend.id) && !jump_blend::RUN_STOP_TO_WAIT.contains(&o.blend.id)) {
            g.speed_param = 0.0;
            g.blend.speed_param = 0.0;
        }
        // The run stop is allowed by Data+0x11F, which the input interpreter sets (IHumanGround vt136 0xDB31B0, from
        // 0xEE6783 / 0xEE67EE / 0xEE67FC): 1 with the stick released, and with the stick held more than 135 deg from the
        // facing (dot < -0.7071); 0 otherwise. Its guard (0xD7EC90) also needs the high profile (HG+1500). So
        // pulling the stick back at a run skids (run stop), and the Idle that follows pivots (state 25): the skid turn.
        let reversed = pad.speed01 > 0.0 && pad.dir.dot(body.forward()) < -std::f32::consts::FRAC_1_SQRT_2;
        // in front of the locomotion the guard also accepts the skid-turn exits (legacy 96 / 97) and any transition
        // past 0.33 s that is not locked (0xD7EC90)
        let tr_allows_stop = g.tr.is_none_or(|t| !t.to_wait && !super::anim_gate::locked(&t.action)
            && (super::ground_tree::RUN_TURN_EXIT.contains(&t.action.id) || t.phase * t.action.duration() > 0.33));
        if reversed && g.high_profile && g.speed_param > 0.25 && g.oneshot.is_none() && tr_allows_stop {
            g.start_run_stop();
        }
        if mv_speed <= 0.0 && g.speed_param > 0.0 && g.oneshot.is_none() && g.tr.is_none() {
            if GAME_GROUND_TREE {
                // Idle from Move (0xD8B220): the wait of the locomotion item's exit foot (its word, 0xD86760)
                let word = super::anim_gate::item_word(super::move_blend::ACT_GROUND_LOCOMOTION, g.blend.foot);
                g.set_wait_foot(super::ground_tree::pick_locomotion_foot(word, g.blend.foot));
            }
            if g.speed_param > 0.25 {
                g.start_run_stop();
                if !GAME_GROUND_TREE && g.oneshot.is_some() {
                    let foot = (g.blend.foot != 0) as usize;
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
            let left = mv_dir.dot(super::right_of(body.forward())) < 0.0;
            // the player's EntityDescriptorType is Main (entity+168 & 7 == 1): `Pivot_Enter` 0xDA6150 then sets the
            // destination profile HG+1504 to the current one HG+1500, so the player only plays the low → low and
            // high → high turns
            let to_high = if GAME_GROUND_TREE { prev_high } else { g.high_profile };
            let id = PIVOT[(!left) as usize][prev_high as usize][to_high as usize][(g.blend.foot != 0) as usize];
            let w = ((off - std::f32::consts::FRAC_PI_2) / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0);
            if jump_blend::action_items(id).is_some() {
                g.play_oneshot(ActionBlend::new(id, 0, &[1.0 - w, w]));
                g.tr = None;
                g.owner = TreeState::Pivot;
                // HG+688 := HG+1532 (`HumanGround__Pivot_Enter` 0xDA6150), read by the turn start's layouts
                g.pivot_turn = stick_turn;
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
        if moving && !busy && !edge_halt && g.oneshot.is_none() && g.tr.is_none() && g.speed_param <= 0.0 && g.collide.is_none() && g.ledge_stop.is_none() && g.ledge_stop_lock.is_none() {
            g.play_start_move();
        }

        // speed parameter, lean/bank and blend weights (MoveBlend 0xDA0810). The heading snapshot is the
        // heading before this frame's turn (HG+0x600, Movement_PreUpdate 0xD97E30). While a transition action plays in
        // front of the locomotion, the transition path runs instead (0xDA09B3): the speed parameter is steered into the
        // layout's band at 1/s and the transition's clips are weighted (`ground_tree`).
        g.blend.speed_param = g.speed_param;
        if let Some(t) = g.tr.filter(|t| !t.to_wait) {
            let goal = super::ground_tree::transition_target(t.layout, target, g.speed_param, g.sprint);
            g.blend.speed_param = super::ground_tree::transition_speed(g.speed_param, goal, dt);
            if matches!(t.layout, 0 | 5 | 6 | 7) {
                g.blend.settle = 1.0;
            }
        } else {
            g.blend.update_speed(target, dt);
        }
        g.blend.update_angles(body.heading, moving.then_some(want_heading), g.crowd_avoid.take().map(heading_of), false, dt);
        if g.tr.is_none() {
            g.blend.update_weights(target, dt);
        }
        // MoveBlend is the Move state's (11): the run stop (18), the landings (21, 22), the skid turn (24) and the
        // pivot (25) do not run it, so the parameter keeps what their exits set (0xDA8C80 is the only caller)
        let tree_owned = GAME_GROUND_TREE && g.oneshot.is_some() && g.owner != TreeState::Other;
        if !tree_owned {
            g.speed_param = g.blend.speed_param;
        } else {
            g.blend.speed_param = g.speed_param;
        }
        // the run stop (state 18) does not run MoveBlend: the parameter stays 0 while it plays
        if g.oneshot.is_some_and(|o| jump_blend::RUN_STOP.contains(&o.blend.id) || jump_blend::RUN_STOP_TO_WAIT.contains(&o.blend.id)) {
            g.speed_param = 0.0;
            g.blend.speed_param = 0.0;
        }

        // heading: rotate toward wanted at the player turn rate (0xD95290), unless the playing item turns the body
        // itself (0x10)
        if moving && !g.oneshot.is_some_and(|o| super::anim_gate::anim_turns(&o.blend)) && !g.tr.is_some_and(|t| super::anim_gate::anim_turns(&t.action)) {
            let mut d = angle_diff(want_heading, body.heading);
            // PORT: near 180 deg the shorter way flips sign with tiny stick changes, so the character turned back
            // and forth (jitter when reversing). Keep the side a turn already started on (RotateTowards 0xD94F30's
            // tie rule is not traced).
            if d.abs() > 170f32.to_radians() && g.turn_sign != 0.0 && d.signum() != g.turn_sign {
                d += g.turn_sign * std::f32::consts::TAU;
            }
            // the player's rates are set every frame by the interpreter (IHumanGround vt952 / vt956 → HG+1752 / +1756,
            // 0xEE6D3F–0xEE6D91): 1.5 / 4.0 rad/s, with Legs held 0.975 / 2.6 (0.375 / 3.0 in the vt1196 state, not
            // reached by the port). The angle band HG+1760 / +1764 stays 0 for the player (ctor 0xDB2D5F; only NPC goals
            // set it), so `RotateTowards` 0xD94F30 always turns at the max rate
            let rate = if GAME_GROUND_TREE { if pad.legs_held { PLAYER_TURN_RATE_LEGS } else { PLAYER_TURN_RATE_GAME } } else { PLAYER_TURN_RATE };
            let step = rate * dt;
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
        // Free running starts it without a press too (0xEE7E50, verified live): Legs held, the stick past the dead zone
        // and within 60 degrees of the facing (`flt_1694AC8`), then the same guard (RE/02 4.7).
        let pressed = pad.jump_buffered() && pad.dir.dot(body.forward()) >= 45f32.to_radians().cos();
        let held = pad.legs_held && pad.magnitude > crate::tuning::STICK_DEADZONE && pad.dir.dot(body.forward()) >= 60f32.to_radians().cos();
        if g.high_profile && moving && (pressed || held) {
            if let Some((contact, normal)) = super::walling::wall_ahead(body.feet, body.forward(), &collision) {
                if pressed {
                    pad.consume_jump();
                }
                let from = body.feet;
                switch_context(&mut loco, &mut data, TransitionSetup::ToWalling(super::walling::WallingEntry { contact, normal, from, warp_duration: None }));
                continue;
            }
        }

        // ---------------------------------------------------------------- beam (NarrowObject, event 72)
        // Guard 0xD9F4C0: a beam in the box ahead (±0.75 m, 0–1 m, ±0.53 m). PORT trigger: walking at it (the
        // decision layer's event 72 sender is not traced).
        if moving && !busy {
            if let Some(mut entry) = super::narrow::try_mount_beam(body.feet, body.forward(), &guidance) {
                // Mode 1 retains the incoming locomotion action (0xF7AAA0).
                if super::narrow::BEAM_COMPLETION {
                    entry.foot = g.blend.foot;
                    entry.action = Some(super::jump_blend::ActionBlend::new(super::move_blend::ACT_GROUND_LOCOMOTION, g.blend.foot, &g.blend.weights));
                }
                let phase = g.blend.phase;
                switch_context(&mut loco, &mut data, TransitionSetup::ToBeam(entry));
                if super::narrow::BEAM_COMPLETION {
                    // The incoming locomotion action carries on (0xF7AAA0 mode 1): keep its phase, not item start.
                    let n = &mut data.narrow;
                    n.t = phase * n.action.map(|a| a.duration()).unwrap_or(0.0);
                }
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
        // Climbing from the ground is the empty-hand press (0xEE7255: the Climb ability, `Pad__JustPressed(3)` ->
        // IHumanGround vt840 = CanHandleEvent(50) -> vt844, event 50 -> the Climb context, fill 0xD83950; RE/02 4.7).
        let forward = body.forward();
        if pad.hand_just_pressed() {
            // leading foot (Human__GetLeadingFoot 0xB18850): the playing locomotion item, footl = left ahead
            if let Some(entry) = climb_start_entry(body.feet, forward, g.blend.foot != 0, &guidance) {
                pad.consume_hand();
                switch_context(&mut loco, &mut data, TransitionSetup::ToClimb(entry));
                continue;
            }
        }
        // The straight jump at a ledge while moving (event 68, IHumanGround vt736 guard 0xD84190 / vt740, sent from
        // 0xEE88E9 under a probe flag and interpreter +0x112F, not fully decoded; the free-run target jump 0xEE7F7A is
        // not ported either). PORT: high profile + Legs held, moving at the ledge; after the wall run above, so it only
        // plays at walls the wall run refuses (below its 1.3 m ray). Standing still, the stationary free run's release
        // jump (0xEE84CC, `ground_extras`) is the game's own path to a hand target.
        if g.high_profile && pad.legs_held && moving {
            if let Some(target) = straight_hand_target(body.feet, forward, &guidance, &collision, false) {
                pad.consume_jump();
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::JumpToTarget { from: body.feet, target, speed_param: 0.0, foot_left: true }));
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
        // The interpreter's jump branch (0xEE817E): high profile, the Legs buffer (+0x1126: set on the press,
        // `GoAssassinActionInterpreter__ReadInput` 0xEEDF80, cleared 0.3 s after it) **with Legs no longer held**, and
        // the stick past the dead zone: a tap jumps when it is let go, holding Legs free-runs instead. While Legs is held
        // the free run jumps at an edge (`v142`, 0xEE7DCE: an edge report within 0.25 m with a drop over 0.5 m, within
        // 80° of the stick, Legs held or HG+588). PORT: the edge test is `edge_ahead` (no floor 0.6 m ahead).
        let forward = body.forward();
        let want_jump = g.high_profile
            && moving
            && ((pad.jump_buffered() && !pad.legs_held) || (pad.legs_held && edge_ahead(body.feet, forward, &collision)));
        if want_jump && !busy {
            pad.consume_jump();
            let entry = match find_jump_target(body.feet, if moving { pad.dir } else { forward }, &guidance, &collision) {
                // leading foot: the playing locomotion item (footl item = left ahead) (hypothesis)
                Some(t) => InAirEntry::JumpToTarget { from: body.feet, target: t, speed_param: g.speed_param, foot_left: g.blend.foot == 0 },
                // vt28: free jump without a target
                None => InAirEntry::FreeJump { from: body.feet, dir: forward, speed_param: g.speed_param, foot_left: g.blend.foot == 0 },
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
                if GAME_GROUND_TREE && g.oneshot.is_none() {
                    let mut heading = body.heading;
                    g.after_oneshot(os.blend, mv_speed > 0.0, stick_turn, &mut heading);
                    body.heading = heading;
                }
            }
            let f0 = Vec3::new(-h0.sin(), 0.0, -h0.cos());
            let right = super::right_of(f0);
            let delta = right * step[0] + f0 * step[1];
            (delta, delta.length() / dt.max(1e-4))
        } else if let Some(mut t) = g.tr {
            // the transition action's root motion (FROMANIM), in the current heading: the code turns the body while it
            // plays unless its item word has 0x10 (then its root yaw does)
            let (d, yaw, ended) = t.advance(g.speed_param, g.pivot_turn, dt);
            if yaw.abs() > 1e-6 {
                body.heading = wrap_angle(body.heading + yaw);
            }
            let f0 = body.forward();
            let delta = super::right_of(f0) * d[0] + f0 * d[1];
            g.tr = if ended {
                if !t.to_wait {
                    // the locomotion item the transition hands over to (ActionTransition +16), from its start; on the
                    // item's last frame MoveBlend already runs its locomotion path (0xDA09BD), so the weights are this
                    // frame's
                    g.blend.foot = t.dest_item.min(1);
                    g.blend.phase = 0.0;
                    g.blend.update_weights(target, 0.0);
                }
                None
            } else {
                Some(t)
            };
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
            && g.tr.is_none()
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
        body.stick_residual = 0.0;
        body.stick_normal_y = None;
        match collision.ground_support(body.feet) {
            Some(s) => {
                let residual = body.feet.y - s.y;
                body.stick_residual = if residual.abs() <= 0.0005 { 0.0 } else { residual };
                body.stick_normal_y = Some(s.normal_y);
                body.feet.y = s.y;
                body.grounded = true;
            }
            None => {
                body.grounded = false;
                // `HumanGround__TransitionToInAirOffSupport` 0xD8ADB0: InAir kind 3 (sub-state 3 keeps the current clip
                // for its remaining length, 0xE00EF0, then the fall), no drop report, so no drop sub-state or drop steer.
                // The animator completes the active item, then uses the upper-body falling blend (0xE00EF0).
                let entry = InAirEntry::Fall { from: body.feet, velocity: body.velocity, origin: FallOrigin::Ground, speed_param: g.speed_param };
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
            }
        }
    }
}

/// `GoAssassinActionInterpreter` wall slide (0xEDD7C0). Only with the stick-to-ground residual within 0.2 m: the
/// controller contact whose normal faces the stick most directly, within 90 deg (`sub_4F8EA0`, contact mask 4; PORT:
/// contacts steeper than 45 deg), at least 0.55 m above the feet. Its flat normal n against the stick d: within 135 deg
/// of n the stick turns along the wall; beyond, d is turned straight into the wall and the wanted speed is 0 (result
/// 2: SetDestSpeedRatio(0) and the run stop allowed).
pub fn wall_slide(body: &Body, dir: Vec3, speed01: f32) -> (Vec3, f32) {
    let d = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    if speed01 <= 0.0 || d == Vec3::ZERO || body.stick_residual.abs() > 0.2 {
        return (dir, speed01);
    }
    let mut best: Option<(f32, Vec3)> = None;
    for c in &body.proxy.manifold {
        if c.normal.y.abs() >= std::f32::consts::FRAC_1_SQRT_2 || c.pos.y < body.feet.y + 0.55 {
            continue;
        }
        let a = (-d).dot(c.normal).clamp(-1.0, 1.0).acos();
        if a <= std::f32::consts::FRAC_PI_2 && best.is_none_or(|(b, _)| a < b) {
            best = Some((a, c.normal));
        }
    }
    let Some((_, normal)) = best else { return (dir, speed01) };
    let n = Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero();
    if n == Vec3::ZERO {
        return (dir, speed01);
    }
    let into = n.dot(d).clamp(-1.0, 1.0).acos();
    if into <= std::f32::consts::PI - std::f32::consts::FRAC_PI_4 {
        let t = (d - n * d.dot(n)).normalize_or_zero();
        if t != Vec3::ZERO { (t, speed01) } else { (dir, speed01) }
    } else {
        (-n, 0.0)
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
/// The climb start from the ground (event 50, guard `sub_D83920` -> `sub_B2E860` on the interpreter's hold reports).
/// PORT: the reports are guidance probes in reach (0.75 m, front half): a hand hold 1.5 m or more above the feet at
/// 1.8 / 2.4 m and foot holds two rows below it.
fn climb_start_entry(feet: Vec3, forward: Vec3, foot_right: bool, guidance: &GuidanceWorld) -> Option<ClimbEntry> {
    let reach = |h: f32| {
        guidance
            .probe(feet + Vec3::Y * h, GRAB_PROBE_RADIUS, 0.225, Some(forward), std::f32::consts::FRAC_PI_2)
            .filter(|hit| (hit.point - feet).dot(forward) > 0.0)
    };
    for h in [1.8f32, 2.4] {
        if let Some(hand) = reach(h) {
            if hand.point.y - feet.y < 1.5 {
                continue;
            }
            let foot = guidance.probe(hand.point - Vec3::Y * CLIMB_ROW * CLIMB_HAND_ROWS as f32, 0.2, 0.15, Some(forward), 0.785);
            if let Some(foot) = foot {
                return Some(ClimbEntry {
                    entry_type: ClimbEntryType::FromGround,
                    hand_l: hand.point,
                    hand_r: hand.point,
                    foot_l: foot.point,
                    foot_r: foot.point,
                    normal: hand.wall_normal,
                    from_feet: feet,
                    foot_right,
                    action: None,
                });
            }
        }
    }
    None
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
    straight_target(feet,point,n,collision,beam)
}
fn straight_target(feet:Vec3,point:Vec3,n:Vec3,collision:&CollisionWorld,beam:bool)->Option<JumpTarget> {
    let wall = super::ledge::hang_type_at(point, n, collision) == super::ledge::LedgeHangType::Wall;
    let dz = point.y - feet.y;
    let j = if beam { super::ledge_moves::hang_jump_in_beam(dz, wall)? } else { super::ledge_moves::hang_jump_in(dz, wall)? };
    if std::env::var_os("AC_ANIM_LOG").is_some() {
        info!("straight target: hands {point:.2} n {n:.2} dz {dz:.2} wall {wall} -> out {:.2} down {:.2}", j.out, j.down);
    }
    Some(JumpTarget { position: point + n * j.out - Vec3::Y * j.down, type_flags: j.flags, hang: Some((point, n)), straight: Some(j), pass: None, ladder: None })
}

/// Stationary request's box is initialized by 0x1610920 / 0x1610970, consumed by 0xB10990.
/// PORT: immutable LedgeGrab reports replace the complete 0xE165D0 chain/limb-config and clearance classifier.
/// Rope/kiosk/pole reports and owner validity cannot be synthesized by this adapter.
fn static_hand_target(feet:Vec3,forward:Vec3,guidance:&GuidanceWorld,collision:&CollisionWorld)->Option<JumpTarget> {
    let right=super::right_of(forward);
    let mut candidates=Vec::new();
    for edge in &guidance.edges {
        let one=GuidanceWorld{edges:vec![edge.clone()],..Default::default()};
        let Some(hit)=one.probe_box(feet+forward*0.5+Vec3::Y*1.85,right,forward,Vec3::Y,
            Vec3::new(0.5,1.0,1.35),feet,120f32.to_radians(),0.0,1<<1) else {continue;};
        let point=guidance.fit_hands(hit.point,hit.wall_normal);
        if !(GRAB_MIN_HEIGHT..=STRAIGHT_JUMP_MAX).contains(&(point.y-feet.y)) {continue;}
        if let Some(target)=straight_target(feet,point,hit.wall_normal,collision,false) {
            candidates.push((point,hit.wall_normal,target));
        }
    }
    let reports:Vec<_>=candidates.iter().map(|c|(c.0,c.1)).collect();
    super::ground_extras::choose_static_report(feet,forward,&reports).map(|i|candidates[i].2)
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
