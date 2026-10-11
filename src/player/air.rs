//! In-air context (`HumanInAir`, ActorContextID 8) — RE/04.
//!
//! Jumps are NOT ballistic in AC1: a takeoff item and a flight item play back to back (chosen and
//! weighted by Human__ComputeJumpAnimBlend 0xB1EC40, `jump_blend`), and their blended root motion gets a
//! linear correction `(target − animEnd) · t/(T₁+T₂)`, so the jump ends exactly on the chosen target
//! (Human__SetupJumpToTarget 0xB20200, HumanInAir__UpdateJumpMotion 0xE0DEF0). Arrival is checked within
//! 0.01 m (0xE07D00).
//! Real physics only for: the over-drop tail (target > 5 m below → aim 5 m down, then free fall with
//! g = 9.8, horizontal steering ≤ 15 m/s) and plain falls (drift decays 4 m/s², ≤ 5 m/s).
//! Landing: fall height measured from the apex; player fixed damage table (0xED6280 / 0xE00FE0);
//! landing action by drop / distance / speed bucket, total drop > 3 m → damage or damage-roll + camera
//! shake (0xE05940, `jump_blend::landing`).

use bevy::prelude::*;

use super::jump_blend::{self, ActionBlend, TARGET_FREESTEP};
use super::ledge::{LedgeEntry, LedgeSubState};
use super::ledge_moves::{self, LedgeArrival};
use super::targets::JumpTarget;
use super::{switch_context, ActorContextId, Body, HumanDataBundle, Locomotion, Player, TransitionSetup};
use crate::collision::CollisionWorld;
use crate::guidance::{GuidanceHit, GuidanceWorld};
use crate::input::PadInput;
use crate::tuning::*;

/// `LandingEvent::LandingType` (values from the exe, desc 0x19919D0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LandingType {
    Safe = 0,
    SmallDamage = 1,
    HeavyDamage = 2,
    Fatal = 3,
}

#[derive(Clone, Copy, Debug)]
pub struct Landing {
    pub kind: LandingType,
    /// LandingEvent+16 (0xE04E40). PORT: native health/protection component is not yet integrated.
    pub damage: u32,
    /// Fall height from the apex (m).
    pub fall_height: f32,
    /// Total drop from the jump/fall start (m).
    pub total_drop: f32,
    pub roll: bool,
    /// The action the Ground context plays on entry (landing 0xE05940 or free-step reception 0xE07D00);
    /// its root motion moves the character until it ends.
    pub action: Option<ActionBlend>,
    /// Seconds of `action` already played on entry: the free-step reception starts at the arrival, under the root
    /// interpolation (`AirMode::Arrive`), so the Ground picks it up that far in (RE/24 AIR-2).
    pub skip: f32,
}

/// PORT: the capsule's radius against a greybox haystack's side (the native contact is the controller's).
const HAY_SIDE_R: f32 = 0.45;

/// Drop sub-state (4) animations (`HumanInAir__EnterDropState` 0xE064C0, tables 0x1A2EAE0 / 0x1A2EB30 filled by
/// 0xDFF680; HumanGround_Hurt block): the entry item by [drop type][side 0 front, 1 back], then the falling loop
/// `hurt_fall_{front,back}`. Ground loss sends types 0..6 (`HumanGround__TransitionToInAirFall` 0xD8C380).
pub const DROP_ENTRY: [[u32; 2]; 9] = [
    [0xC5DD_34F3, 0xBE67_B24A], // 0 stumble_air medium
    [0xC5DD_34F6, 0xBE67_B24D], // 1 stumble_air strong
    [0xC5DD_34F7, 0xBE67_B23E], // 2 hurt_fall_balanced short
    [0xC5DD_34FA, 0xBE67_B241], // 3 hurt_fall_balanced long
    [0x7E5D_E556, 0x7E5D_E557], // 4 hurt_fall_medium
    [0, 0],                     // 5 (none)
    [0x7F4A_71E4, 0x7F4A_71E5], // 6 hurt_fall_dive
    [0x8111_514B, 0x8111_514B], // 7 hurt_fall_stumble_dive
    [0x8111_7142, 0x8111_7141], // 8 hurt_fall_railing
];
pub const DROP_LOOP: [u32; 2] = [0x8111_514E, 0x8111_514F];

/// Drop phase length by type (0xE064C0): < 2 → 0.3 s, 2–3 → 0.6 s, 4–5 → 1.0 s, 6–7 → the animation's length,
/// 8 → 0.1 s; `None` = the animation's length.
pub fn drop_phase(ty: usize) -> Option<f32> {
    match ty {
        0 | 1 => Some(0.3),
        2 | 3 => Some(0.6),
        4 | 5 => Some(1.0),
        8 => Some(0.1),
        _ => None,
    }
}

/// `HumanGround__TransitionToInAirFall` 0xD8C380: fall type by the drop report's height `h` and the horizontal speed
/// `v`: h < 1 m (or < 8 m on a type-1 probe, PORT: not modelled) → 0 / 1 (2 / 3 when h ≥ 2 m), else 4 / 6; the second
/// of each pair at v ≥ 2.5 m/s.
pub fn fall_type(h: f32, v: f32) -> usize {
    let fast = v >= 2.5;
    if h < 1.0 {
        if h >= 2.0 { 2 + fast as usize } else { fast as usize }
    } else if fast {
        6
    } else {
        4
    }
}

/// `HumanInAirData::FallOrigin` (desc 0x199587C).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FallOrigin {
    #[default]
    Ground = 0,
    Climb = 1,
    HangWall = 2,
    HangFree = 3,
}

/// What the switching context asks InAir to do (TransitionSetupDataToInAir, RE/01 §4.2).
#[derive(Clone, Copy, Debug)]
pub enum InAirEntry {
    /// PORT: `speed_param` seeds the entry; 0xEDBAF0 updates the live air ratio on subsequent updates;
    /// `foot_left` = the leading foot (byte+60 bits 2–3 of the playing item, or sub_B18850).
    JumpToTarget { from: Vec3, target: JumpTarget, speed_param: f32, foot_left: bool },
    /// No target found: the free-jump target (`Human__MakeFreeJumpTarget` 0xB13630 → 0xB1E7F0, `FREE_JUMP_AHEAD` along
    /// `dir`, `FREE_JUMP_DOWN` below), jumped at like any target (type 1) and ended by the ground contact.
    /// `ahead` = how far along `dir` the target lies (`FREE_JUMP_AHEAD`; the step off an edge, event 68, aims
    /// 1 + 2·profile·speed m ahead, 0xD9D1F0).
    FreeJump { from: Vec3, dir: Vec3, ahead: f32, speed_param: f32, foot_left: bool },
    /// A free-step jump (jump kind 1: the `freestep_*_to_air` takeoff) from a beam or a pilotis to a target
    /// (NarrowObject event 4 → `Human__SetupJumpToTarget` 0xB20200 with kind 1, 0xE4D950).
    FreeStepJump { from: Vec3, target: JumpTarget, foot_left: bool },
    /// A jump without a target that plays one action's root motion, then falls (the beam's jump on the spot,
    /// 0xF73B80: `beam_jumpstraight_clear`, then InAir +416 = `…_clear_tr_fall`).
    OnPlace { from: Vec3, fwd: Vec3, action: ActionBlend, fall: Option<ActionBlend> },
    /// The swing jump from a bar (Ledge SwingReception event 7 -> `sub_DCD0B0`: `Human__SetupJumpToTarget` with jump
    /// kind 3, the `swing_cycle_*_to_air` takeoff). Without a target: the free target (IHuman vt68 mode 0, 0xEED0FB →
    /// 0xB1E7F0: `FREE_JUMP_AHEAD` along `dir`, `FREE_JUMP_DOWN` below, type 1), ended by the ground contact.
    SwingJump { from: Vec3, target: Option<JumpTarget>, dir: Vec3 },
    /// The rebound off a wall (jump kind 2, the `xx_h_rebound_*_to_air` takeoff: `HumanWalling__ReboundJump` 0xE365C0,
    /// `HumanLedge__ReboundEndJump` 0xDD0A40, `HumanClimb__ReboundEndJump` 0xDEAE30 via 0xB27180), the character
    /// already turned away from the wall. Without a target: 7 m along `dir`, 3 m down, type 1 (0xE36883), ended by the
    /// ground contact like the free jump (RE/24 AIR-5).
    ReboundJump { from: Vec3, target: Option<JumpTarget>, dir: Vec3, foot_left: bool },
    Fall { from: Vec3, velocity: Vec3, origin: FallOrigin, speed_param: f32 },
    /// The ledge let-go above a haystack (the interpreter's ledge state 0xEEC558: the vt84 search hit → ActorState 41,
    /// the HollywoodMode layer 28, IHumanLedge vt44(target, 2048) → event 4 → `sub_DD7810`: the let-go record, Data+506):
    /// the let-go plays, then the faith dive and the tail steers into the haystack (RE/24 CENSUS-2 / AIR-23).
    LetGoDive { from: Vec3, target: JumpTarget, origin: FallOrigin },
}

impl InAirEntry {
    /// The same entry from another start point (a target stored before a rebound, 0xDD2CA0).
    pub fn starting_at(self, p: Vec3) -> Self {
        match self {
            InAirEntry::JumpToTarget { target, speed_param, foot_left, .. } => InAirEntry::JumpToTarget { from: p, target, speed_param, foot_left },
            InAirEntry::FreeJump { dir, ahead, speed_param, foot_left, .. } => InAirEntry::FreeJump { from: p, dir, ahead, speed_param, foot_left },
            InAirEntry::FreeStepJump { target, foot_left, .. } => InAirEntry::FreeStepJump { from: p, target, foot_left },
            InAirEntry::OnPlace { fwd, action, fall, .. } => InAirEntry::OnPlace { from: p, fwd, action, fall },
            InAirEntry::SwingJump { target, dir, .. } => InAirEntry::SwingJump { from: p, target, dir },
            InAirEntry::ReboundJump { target, dir, foot_left, .. } => InAirEntry::ReboundJump { from: p, target, dir, foot_left },
            InAirEntry::Fall { velocity, origin, speed_param, .. } => InAirEntry::Fall { from: p, velocity, origin, speed_param },
            InAirEntry::LetGoDive { target, origin, .. } => InAirEntry::LetGoDive { from: p, target, origin },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AirMode {
    #[default]
    Idle,
    /// Target-driven jump with linear correction.
    Jump {
        from: Vec3,
        clip_end: Vec3,
        aim: Vec3,
        duration: f32,
        t: f32,
        /// After the clip, continue as a free fall toward this point (over-drop or free jump).
        then_fall_to: Option<Vec3>,
        /// Takeoff item duration T₁ (HumanInAirData+0x1E4).
        t_takeoff: f32,
        /// Character forward at takeoff (the jump origin matrix +0x10).
        fwd: Vec3,
    },
    /// Ballistic fall (optionally steered toward `steer_to`).
    Fall { steer_to: Option<Vec3> },
    /// The free-step arrival's root interpolation (`CheckJumpTargetArrival` 0xE07D00 case 1 / 0x10000, 0xE09A13 →
    /// `sub_711130` 0xE09B26): from the arrival point to the stored (pulled-back) target + 0.5·forward over
    /// `FREESTEP_ARRIVE` s while the reception plays; then the landing (RE/24 AIR-2).
    Arrive { from: Vec3, to: Vec3, t: f32 },
}

/// The let-go from a free hang (`sub_DD7810`; 0x1120E17B from a wall hang is `climb::LOST_GRIP`).
pub const LET_GO_FREE: u32 = 0x1120_E179;

/// The free-step arrival's root interpolation time (0xE09A13: 0.0667 s).
pub const FREESTEP_ARRIVE: f32 = 0.066_666_67;

/// `Human__ComputeJumpAnimBlend` 0xB1EC40, branch a15 == 0 (0xB1F0A0–0xB1F0E4): the jump target is pulled back
/// 0.5·k (k = entity +0x7C, 1 for the player) along the flat direction from the start, before the distance class, the
/// correction and the stored target (Data+560) read it (RE/24 AIR-2). The hang, ladder and surface types (0x40, 0x80,
/// 0x1000, 0x2000, 0x4000, swing) replace it with their own aim (0xB1F60E, AIR-3); the port's aim for those is already
/// the hang root. PORT: the setup byte a15 (+212 / HG+996, AIR-19) is taken as 0: its producer is untraced.
pub fn pull_back(from: Vec3, aim: Vec3, target_type: u32) -> Vec3 {
    if !matches!(target_type, 1 | 0x10000 | 0x100 | 2 | 0x200 | 0x400 | 0x8000 | 0x800) {
        return aim;
    }
    let dir = Vec3::new(aim.x - from.x, 0.0, aim.z - from.z).normalize_or_zero();
    aim - dir * 0.5
}

/// The over-drop rule `sub_B1B8C0` (0xB1B9A8 / 0xB1BA76–0xB1BB42): a target more than `thr` below the start is aimed at
/// `thr` down, pulled back horizontally by min(dist, 1.5)·k with k = clamp((dz − thr) / −30, 0, 2) (RE/24 AIR-20); the
/// free fall then steers to the true target. Returns (aim, the fall's target).
fn over_drop(from: Vec3, target: Vec3, thr: f32) -> (Vec3, Option<Vec3>) {
    let dz = target.y - from.y;
    if dz >= -thr {
        return (target, None);
    }
    let flat = Vec3::new(target.x - from.x, 0.0, target.z - from.z);
    let k = ((dz + thr) / -30.0).clamp(0.0, 2.0);
    let aim = target - flat.normalize_or_zero() * (flat.length().min(1.5) * k);
    (Vec3::new(aim.x, from.y - thr, aim.z), Some(target))
}

/// Runtime data of the InAir context (subset of reflected HumanInAirData, RE/07).
#[derive(Debug, Default)]
pub struct HumanInAirData {
    /// Incremented on every entry (lets the animator restart the jump clip).
    pub seq: u32,
    /// The jump target (+0x230), kept so arrival can hand off to the right context.
    pub target: Option<JumpTarget>,
    /// Seconds since entering InAir (used to avoid re-catching the ledge just released).
    pub time_in_air: f32,
    /// Last catch (fall height ≥ 3 m selects the long catch animations in the game, 0xE0BB70).
    pub long_catch: bool,
    /// InAirData +0x190: the hold let go of (a ledge or climb release); catches must be 0.5 m below it (0xE0A990).
    pub release_hold: Option<Vec3>,
    /// The edge landing (InAir +225 bit 2) has played in this air stay.
    pub edge_landed: bool,
    pub mode: AirMode,
    /// JumpApexPosition (+0xD0): where descent started — the fall-height reference.
    pub apex_y: f32,
    pub apex_reached: bool,
    /// Height at the jump/fall start (total-drop reference).
    pub start_y: f32,
    pub fall_origin: FallOrigin,
    /// Jump target type flags (+0x290).
    pub target_flags: u32,
    pub prev_y: f32,
    /// PORT adapter: entry velocity is applied by the body-owning system before the skipped update.
    pub entry_velocity: Option<Vec3>,
    /// HumanInAir +0x50: the over-drop tail's velocity in m / frame (0xE0F12A, 0xE0FA4B; RE/24 AIR-4).
    pub tail_v: Vec3,
    /// The let-go dive: the let-go's first item still playing (s left); the tail waits for it (0xE0F772).
    pub dive_wait: Option<f32>,
    /// The HollywoodMode layer (28) of the let-go dive: it collides with almost nothing (0xEEC622). PORT: the dive's
    /// step is set directly, without the capsule.
    pub hollywood: bool,
    /// HumanInAir+0x16C: live air-control speed ratio (0xEDBAF0 -> 0xE102B0).
    pub speed_ratio: f32,
    /// Takeoff and flight items of the current jump (+0x1A4 / +0x1A8 and the weight arrays +0x1BC..).
    pub takeoff: Option<ActionBlend>,
    pub flight: Option<ActionBlend>,
    pub foot_left: bool,
    /// Jump / fall start (JumpOrigin +0x10 translation).
    pub start: Vec3,
    /// The action played once the jump turns into a fall (HumanInAirData+416), and the time since.
    pub fall_action: Option<ActionBlend>,
    pub fall_t: f32,
    /// The jump is at the free-jump target (no target found): the ground contact ends it, as it does in the game
    /// (0xE05200 on the controller contact) long before the target 3 m down is reached.
    pub free_target: bool,
    /// A free-step arrival cancelled by a ceiling contact (`sub_E009F0`): the fall plays with a 0.3 s blend (RE/24 AIR-10).
    pub free_step_abort: bool,
    /// Ground loss: the drop sub-state's (type, side) (InAirData+944 +64 / +68) and the edge's outward normal.
    pub drop: Option<(usize, usize, Option<Vec3>)>,
    /// First steep contact and rag-fall requirement (0xE038A0 / 0xE05200).
    pub slope_slide: super::ground_extras::SlopeSlide,
    /// `HumanInAir__AntiStuckNudge` 0xE0B1E0.
    pub anti_stuck: super::falls::AntiStuck,
    /// A run jump turns from the takeoff facing to its destination over the takeoff (set up on the first update).
    turn_pending: bool,
    /// (takeoff heading, destination heading) of that turn.
    pub turn: Option<(f32, f32)>,
    /// 0xB145B0's sweep for the down variants (start, target), cast on the first update (`apply_down_variants`).
    down_check: Option<(Vec3, Vec3)>,
    /// InAirData+452: the arrival reception's [up, down, long] weights (`sub_B1EAA0`), `None` = the flight's.
    pub arrival_w: Option<[f32; 3]>,
    /// A side-takeoff jump (kinds 1 / 2 / 3) set up again on the first update, when the start facing is known:
    /// (from, aim, target type, fall target, kind).
    side_setup: Option<(Vec3, Vec3, u32, Option<Vec3>, u8)>,
}

impl HumanInAirData {
    pub fn enter(&mut self, entry: InAirEntry) {
        self.seq = self.seq.wrapping_add(1);
        self.turn_pending = false;
        self.turn = None;
        self.down_check = None;
        self.arrival_w = None;
        self.side_setup = None;
        self.dive_wait = None;
        self.hollywood = false;
        self.apex_reached = false;
        self.fall_origin = FallOrigin::Ground;
        self.target_flags = 0;
        self.target = None;
        self.time_in_air = 0.0;
        self.fall_action = None;
        self.fall_t = 0.0;
        self.free_target = false;
        self.free_step_abort = false;
        self.drop = None;
        self.slope_slide = default();
        self.anti_stuck.reset(self.seq.wrapping_mul(2_654_435_761));
        self.entry_velocity = None;
        self.release_hold = None;
        self.edge_landed = false;
        match entry {
            InAirEntry::JumpToTarget { from, target, speed_param, foot_left } => {
                self.speed_ratio = speed_param;
                self.foot_left = foot_left;
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                self.target_flags = target.type_flags;
                self.target = Some(target);
                // over-drop rule (0xB1B8C0): target > 5 m below → aim 5 m down, then free-fall.
                let mut aim = target.position;
                let mut then_fall_to = None;
                // haystack: −3 m, or −30 m when deeper (Leap of Faith)
                let thr = if target.type_flags == jump_blend::TARGET_HAYSTACK {
                    if target.position.y - from.y <= -jump_blend::FAITH_MIN_DROP { jump_blend::FAITH_DOWN } else { jump_blend::FAITH_MIN_DROP }
                } else {
                    OVERDROP
                };
                if target.straight.is_none() {
                    (aim, then_fall_to) = over_drop(from, target.position, thr);
                } else if target.position.y < from.y - thr {
                    aim.y = from.y - thr;
                    then_fall_to = Some(target.position);
                }
                self.mode = match (target.straight, target.hang) {
                    (Some(j), Some((_, n))) => self.straight_jump(from, aim, j, n),
                    _ => self.real_jump(from, aim, target.type_flags, then_fall_to, 0),
                };
            }
            InAirEntry::FreeStepJump { from, target, foot_left } => {
                self.speed_ratio = 0.0;
                self.foot_left = foot_left;
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                self.target_flags = target.type_flags;
                self.target = Some(target);
                let (aim, then_fall_to) = over_drop(from, target.position, OVERDROP);
                self.mode = self.real_jump(from, aim, target.type_flags, then_fall_to, 1);
            }
            InAirEntry::SwingJump { from, target, dir } => {
                self.speed_ratio = 0.0;
                self.foot_left = true;
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                match target {
                    Some(target) => {
                        self.target_flags = target.type_flags;
                        self.target = Some(target);
                        let (aim, then_fall_to) = over_drop(from, target.position, OVERDROP);
                        self.mode = self.real_jump(from, aim, target.type_flags, then_fall_to, 3);
                    }
                    None => {
                        let flat = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::NEG_Z);
                        let aim = from + flat * FREE_JUMP_AHEAD - Vec3::Y * FREE_JUMP_DOWN;
                        self.target_flags = TARGET_FREESTEP;
                        self.mode = self.real_jump(from, aim, TARGET_FREESTEP, Some(aim), 3);
                        self.free_target = true;
                    }
                }
            }
            InAirEntry::ReboundJump { from, target, dir, foot_left } => {
                self.speed_ratio = 0.0;
                self.foot_left = foot_left;
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                match target {
                    Some(target) => {
                        self.target_flags = target.type_flags;
                        self.target = Some(target);
                        let (aim, then_fall_to) = over_drop(from, target.position, OVERDROP);
                        self.mode = self.real_jump(from, aim, target.type_flags, then_fall_to, 2);
                    }
                    None => {
                        let flat = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::NEG_Z);
                        let aim = from + flat * 7.0 - Vec3::Y * FREE_JUMP_DOWN;
                        self.target_flags = TARGET_FREESTEP;
                        self.mode = self.real_jump(from, aim, TARGET_FREESTEP, Some(aim), 2);
                        self.free_target = true;
                    }
                }
            }
            InAirEntry::OnPlace { from, fwd, action, fall } => {
                self.speed_ratio = 0.0;
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                self.takeoff = None;
                self.flight = Some(action);
                self.fall_action = fall;
                let fwd = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or(Vec3::NEG_Z);
                let clip_end = from + to_world(action.disp(1.0), fwd);
                let d = action.duration().max(0.1);
                self.mode = AirMode::Jump { from, clip_end, aim: clip_end, duration: d, t: 0.0, then_fall_to: Some(clip_end), t_takeoff: 0.0, fwd };
            }
            InAirEntry::FreeJump { from, dir, ahead, speed_param, foot_left } => {
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                self.speed_ratio = speed_param;
                self.foot_left = foot_left;
                // the game's free-jump target (0xB1E7F0): 8 m ahead, 3 m down, type 1, jumped at through
                // `Human__SetupJumpToTarget` like any target; on open ground the contact ends the jump long before
                // it (live: 0.66 s, 4.8 m), off a roof it falls on from there
                let flat = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::NEG_Z);
                let aim = from + flat * ahead - Vec3::Y * FREE_JUMP_DOWN;
                self.mode = self.real_jump(from, aim, TARGET_FREESTEP, Some(aim), 0);
                self.free_target = true;
            }
            InAirEntry::LetGoDive { from, target, origin } => {
                self.entry_velocity = Some(Vec3::ZERO);
                self.takeoff = None;
                self.flight = None;
                self.speed_ratio = 0.0;
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                self.fall_origin = origin;
                self.target_flags = target.type_flags;
                self.target = Some(target);
                self.tail_v = Vec3::ZERO;
                // the let-go (0x1120E17B wall / 0x1120E179 free) plays its first item before the dive
                let id = if origin == FallOrigin::HangFree { LET_GO_FREE } else { super::climb::LOST_GRIP };
                let d = [id, super::climb::LOST_GRIP].into_iter().map(|a| ActionBlend::new(a, 0, &[1.0]).duration()).find(|d| *d > 0.0).unwrap_or(0.3);
                self.dive_wait = Some(d);
                self.hollywood = true;
                self.mode = AirMode::Fall { steer_to: Some(target.position) };
            }
            InAirEntry::Fall { from, velocity, origin, speed_param } => {
                self.entry_velocity = Some(velocity);
                self.takeoff = None;
                self.flight = None;
                self.speed_ratio = speed_param;
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                self.fall_origin = origin;
                self.mode = AirMode::Fall { steer_to: None };
            }
        }
    }
}

impl HumanInAirData {
    /// The item word of the jump's playing item (the takeoff, then the flight; `item+60`). PORT: `None` outside a jump
    /// (the falls' loops are not in the table's movement blocks).
    pub fn playing_word(&self) -> Option<u16> {
        let AirMode::Jump { t, t_takeoff, .. } = self.mode else { return None };
        let b = if t < t_takeoff { self.takeoff } else { self.flight }?;
        super::anim_gate::word(&b)
    }

    /// `HumanInAir__IsLeapOfFaithDiveAnim` 0xDFEFC0 (IHumanInAir slot 18): slot 0 plays a faith action, the takeoffs
    /// 0x23A949B1 / 0x23A949B7, the flight 0x23A949B2 or the fall 0x23A949B5. The port's faith jump and let-go dive hold
    /// the flight in `flight` (the Fall mode plays the faith fall from it, anim.rs); the let-go's own item before the
    /// dive is not one.
    pub fn faith_dive(&self) -> bool {
        [self.takeoff, self.flight].into_iter().flatten().any(|a| {
            jump_blend::TAKEOFF_FAITH.contains(&a.id) || a.id == jump_blend::FLIGHT_FAITH || a.id == jump_blend::FALL_FAITH
        })
    }

    /// Human__SetupJumpToTarget 0xB20200 for a ground running jump (kind 0): pick and weight the takeoff and
    /// flight items (0xB1EC40), T₁/T₂ = their Σw·T durations, anim end = their summed displacement.
    fn real_jump(&mut self, from: Vec3, aim: Vec3, target_type: u32, then_fall_to: Option<Vec3>, kind: u8) -> AirMode {
        if matches!(kind, 1 | 2 | 3) {
            // set up again on the first update with the start facing (the side angle, `begin_turn`)
            self.side_setup = Some((from, aim, target_type, then_fall_to, kind));
        }
        self.jump_setup(from, aim, target_type, then_fall_to, kind, None)
    }

    /// 0xB20200 / 0xB1EC40 with the start facing `heading` when known: the side angle (kinds 1 / 2 / 3) picks the
    /// takeoff's side groups and the clips play in the character's own frame (no destination turn); kind 0 turns
    /// toward the target over the takeoff (`begin_turn`). Without the heading: the provisional set-up, angle 0.
    fn jump_setup(&mut self, from: Vec3, aim: Vec3, target_type: u32, then_fall_to: Option<Vec3>, kind: u8, heading: Option<f32>) -> AirMode {
        // the distance to the pulled-back target, the height of the target itself (0xB1F502 / 0xB1FAA6)
        let to = Vec3::new(aim.x - from.x, 0.0, aim.z - from.z).length();
        self.arrival_w = jump_blend::arrival_weights(target_type, kind, aim.y - from.y, (to - 0.5).max(0.0));
        let aim = pull_back(from, aim, target_type);
        let flat = Vec3::new(aim.x - from.x, 0.0, aim.z - from.z);
        let dir = flat.normalize_or(Vec3::NEG_Z);
        // the side angle: from the start facing to the target, positive to the right (0xB1F593)
        let start_fwd = heading.map(|h| Vec3::new(-h.sin(), 0.0, -h.cos()));
        let angle = start_fwd.map_or(0.0, |f| dir.dot(super::right_of(f)).atan2(dir.dot(f)));
        let fwd = start_fwd.unwrap_or(dir);
        let b = jump_blend::compute_kind(aim.y - from.y, flat.length(), target_type, self.foot_left, 1.0, kind, angle);
        let (takeoff_id, flight_id) = (b.takeoff, b.flight);
        if aim.y - from.y < -jump_blend::DOWN_VARIANT_DROP && jump_blend::down_variants(takeoff_id, flight_id) != (takeoff_id, flight_id) {
            self.down_check = Some((from, aim));
        }
        let takeoff = ActionBlend::new(takeoff_id, 0, &b.takeoff_w);
        let flight = ActionBlend::new(flight_id, 0, &b.flight_w);
        let (t1, t2) = (takeoff.duration(), flight.duration());
        self.takeoff = Some(takeoff);
        self.flight = Some(flight);
        let end = compose(&takeoff, &flight);
        let clip_end = from + to_world(end, fwd);
        // 0xB20200 keeps the character's own matrix as the jump frame (InAirData +16 / +320) and stores a destination
        // orientation (+256, flag +507) only for run jumps (kind 0 / 4) and the Leap of Faith; 0xE0ECDC slerps toward
        // it. The frame is taken from the body on the first update (`begin_turn`).
        self.turn_pending = kind == 0;
        if std::env::var_os("AC_ANIM_LOG").is_some() {
            info!("target jump: type {target_type:#x} kind {kind} dz {:.2} dist {:.2}: takeoff {takeoff_id:#010x} {t1:.2}s, flight {flight_id:#010x} {t2:.2}s, clip end {:.2} vs aim {:.2}",
                aim.y - from.y, flat.length(), (clip_end - from).length(), (aim - from).length());
        }
        AirMode::Jump { from, clip_end, aim, duration: (t1 + t2).max(1e-3), t: 0.0, then_fall_to, t_takeoff: t1, fwd }
    }
}

impl HumanInAirData {
    /// Human__SetupJumpToHandTarget 0xB21DA0: no takeoff item (+0x1E4 = 0); the band's 2-clip flight, its Σw·T
    /// as the duration (+0x1E8), JumpType 0, and the linear correction to the hang / knee root.
    fn straight_jump(&mut self, from: Vec3, aim: Vec3, j: super::ledge_moves::HangJumpIn, n: Vec3) -> AirMode {
        let fwd = -Vec3::new(n.x, 0.0, n.z).normalize_or(Vec3::NEG_Z);
        let flight = ActionBlend::new(j.flight, 0, &[1.0 - j.b, j.b]);
        self.takeoff = None;
        self.flight = Some(flight);
        let d = flight.duration();
        let clip_end = from + to_world(flight.disp(1.0), fwd);
        if std::env::var_os("AC_ANIM_LOG").is_some() {
            info!("straight jump: flight {:#010x} b {:.2} {d:.2}s, clip end {:.2} vs aim {:.2}", j.flight, j.b, (clip_end - from).length(), (aim - from).length());
        }
        // PORT: the 0.2 s floor (no game source, RE/24 AIR-36) only guards a flight whose clips are not loaded; the
        // game's duration is the flight's Σw·T (+0x1E8)
        AirMode::Jump { from, clip_end, aim, duration: d.max(0.2), t: 0.0, then_fall_to: None, t_takeoff: 0.0, fwd }
    }
}

impl HumanInAirData {
    /// `Human__UseBeamFreestepJumpVariants` 0xB145B0 (RE/24 AIR-1), on the first update (it needs the collision): with
    /// the target more than 0.4 m below the start, a ray from the start + 0.2 m up toward the target, as long as its
    /// crossing of the plane 0.4 m below the start, on the main character's layer (21; 5 for other characters). It
    /// swaps to the down variants only when the ray **hits** (`Human__RayClear` 0xE1C820 → 0xE1C470 returns 1 when
    /// clear): a jump onto solid ground ahead (the no-target jump on flat ground), not one off an edge over open air.
    fn apply_down_variants(&mut self, collision: &CollisionWorld) {
        let Some((start, target)) = self.down_check.take() else { return };
        let o = start + Vec3::Y * 0.2;
        let dir = (target - o).normalize_or_zero();
        if dir.y >= -1e-4 {
            return;
        }
        let len = 0.6 / -dir.y;
        if collision.ray_distance(o, dir, len, crate::layers::MAIN_CHARACTER) >= len - 1e-4 {
            return;
        }
        let (Some(mut takeoff), Some(mut flight)) = (self.takeoff, self.flight) else { return };
        (takeoff.id, flight.id) = jump_blend::down_variants(takeoff.id, flight.id);
        self.takeoff = Some(takeoff);
        self.flight = Some(flight);
        if let AirMode::Jump { from, clip_end, duration, t_takeoff, fwd, .. } = &mut self.mode {
            let (t1, t2) = (takeoff.duration(), flight.duration());
            *duration = (t1 + t2).max(1e-3);
            *t_takeoff = t1;
            *clip_end = *from + to_world(compose(&takeoff, &flight), *fwd);
        }
    }

    /// The run jump's frame (0xB20200: the character's own matrix at takeoff): the clips' root motion plays in the
    /// takeoff facing, the linear correction carries it onto the target, and the facing turns to the destination.
    /// PORT: the destination orientation (InAirData +256, from 0xB1EC40) is taken as the direction to the target.
    fn begin_turn(&mut self, heading: f32) {
        if let Some((from, aim, ty, then_fall_to, kind)) = self.side_setup.take() {
            self.mode = self.jump_setup(from, aim, ty, then_fall_to, kind, Some(heading));
            return;
        }
        if !std::mem::take(&mut self.turn_pending) {
            return;
        }
        let AirMode::Jump { from, clip_end, fwd, .. } = &mut self.mode else { return };
        let dest = super::heading_of(*fwd);
        let start_fwd = Vec3::new(-heading.sin(), 0.0, -heading.cos());
        let end = match (self.takeoff, self.flight) {
            (Some(a), Some(b)) => compose(&a, &b),
            _ => return,
        };
        *clip_end = *from + to_world(end, start_fwd);
        *fwd = start_fwd;
        self.turn = Some((heading, dest));
    }

    /// Facing during the jump (0xE0EF30): slerp(takeoff, destination, t / min(T₁, 0.4)), 1 once the takeoff item ends.
    /// Without the turn (kinds 1 / 2 / 3) the start facing turned by the clips' root yaw (the flight composed after
    /// the takeoff's end matrix, 0xB20D0C / 0xE0EB9E; RE/24 AIR-5).
    fn jump_heading(&self, t: f32, t_takeoff: f32, fwd: Vec3) -> f32 {
        let Some((h0, h1)) = self.turn else { return super::heading_of(fwd) + self.jump_yaw(t, t_takeoff) };
        let s = if t_takeoff <= 0.0 || t >= t_takeoff { 1.0 } else { (t / t_takeoff.min(0.4)).min(1.0) };
        let d = (h1 - h0 + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        h0 + d * s
    }
}

/// PORT: how fast a target jump closes a lag behind its path (1/s), and the lag at which it gives up and falls.
const JUMP_LAG_RECOVERY: f32 = 6.0;
const JUMP_BLOCKED_LAG: f32 = 0.6;

/// Signed angle from `f` to `to` about up (+ = to the left, the heading's positive sense).
fn right_of_signed(f: Vec3, to: Vec3) -> f32 {
    let r = super::right_of(f);
    (-to.dot(r)).atan2(to.dot(f))
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Animation space (x right, y forward, z up) → world, for a character facing `fwd`.
fn to_world(d: [f32; 3], fwd: Vec3) -> Vec3 {
    super::right_of(fwd) * d[0] + fwd * d[1] + Vec3::Y * d[2]
}

/// Blended root displacement of the jump at time `t` (takeoff item, then the flight item from its end).
fn jump_disp(air: &HumanInAirData, t: f32, t1: f32, duration: f32) -> [f32; 3] {
    jump_disp_clips(air, t, t1, duration)
}

fn jump_disp_clips(air: &HumanInAirData, t: f32, t1: f32, duration: f32) -> [f32; 3] {
    let Some(fl) = air.flight else { return [0.0; 3] };
    let Some(to) = air.takeoff else { return fl.disp(t / duration.max(1e-4)) };
    if t < t1 {
        to.disp(t / t1.max(1e-4))
    } else {
        add(to.disp(1.0), rotate_yaw(fl.disp((t - t1) / (duration - t1).max(1e-4)), to.yaw(1.0)))
    }
}

/// An animation-space displacement turned by a root yaw (+ = left; x right, y forward).
fn rotate_yaw(d: [f32; 3], yaw: f32) -> [f32; 3] {
    let (s, c) = yaw.sin_cos();
    [d[0] * c - d[1] * s, d[0] * s + d[1] * c, d[2]]
}

/// The takeoff then the flight, composed after the takeoff's end matrix (0xB20D0C `start·takeoff·flight`): the flight's
/// displacement turned by the takeoff's end yaw (a side takeoff carries up to 90°, RE/24 AIR-5).
fn compose(takeoff: &ActionBlend, flight: &ActionBlend) -> [f32; 3] {
    add(takeoff.disp(1.0), rotate_yaw(flight.disp(1.0), takeoff.yaw(1.0)))
}

impl HumanInAirData {
    /// The clips' root yaw at jump time `t` (takeoff, then its end yaw plus the flight's).
    fn jump_yaw(&self, t: f32, t1: f32) -> f32 {
        let (Some(to), Some(fl)) = (self.takeoff, self.flight) else { return 0.0 };
        if t < t1 {
            to.yaw(t / t1.max(1e-4))
        } else {
            let d = fl.duration().max(1e-4);
            to.yaw(1.0) + fl.yaw(((t - t1) / d).min(1.0))
        }
    }
}

fn classify_landing(apex_y: f32, apex_reached: bool, start_y: f32, land_y: f32) -> Landing {
    let fall_height = if apex_reached { (apex_y - land_y).max(0.0) } else { 0.0 };
    let total_drop = (start_y - land_y).max(0.0);
    let kind = if fall_height > FALL_FATAL {
        LandingType::Fatal
    } else if fall_height > FALL_HEAVY {
        LandingType::HeavyDamage
    } else {
        LandingType::Safe
    };
    // PORT: the native CanSurviveDamage/protection overrides (0xE00FE0) require the missing
    // synchronization/damage component. This returns the verified base table, not final health outcomes.
    let (kind, damage) = if GAME_FALLS { super::falls::landing_damage(fall_height) } else { (kind, if kind == LandingType::Fatal { 200 } else if kind == LandingType::HeavyDamage { 10 } else { 0 }) };
    Landing { kind, damage, fall_height, total_drop, roll: total_drop > ROLL_DROP && kind != LandingType::Fatal, action: None, skip: 0.0 }
}

/// CheckAirCatch 0xE0BB70 / FindLedgeCatch 0xE0A990: look for an edge in the hand box at the reach
/// height, facing the character within 70°.
/// PORT: approximate feet-height probes; native hand-bone pairs and origin/release clearance are not represented.
fn find_air_catch(feet: Vec3, facing: Vec3, guidance: &GuidanceWorld) -> Option<GuidanceHit> {
    for reach in [CATCH_REACH_WALL, CATCH_REACH_LEDGE] {
        let p = feet + Vec3::Y * reach + facing * 0.3;
        if let Some(h) = guidance.probe(p, CATCH_BOX_R, CATCH_BOX_V, Some(facing), CATCH_MAX_ANGLE) {
            return Some(h);
        }
    }
    None
}

/// The narrow-object part of `CheckAirCatch` 0xE0BB70 (fall height < 9 m, not while a target is still ahead):
/// first a pilotis under the feet (a support within 0.5 m, then 0xB2B600 from the air → soft landing,
/// PilotisReception), then a beam in the 0xE0B890 box (→ landing, BeamReception mode 7).
fn narrow_catch(feet: Vec3, facing: Vec3, foot: usize, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<TransitionSetup> {
    use super::narrow::*;
    if let Some(h) = collision.ground_height(feet + Vec3::Y * 0.01, 0.5) {
        let support = Vec3::new(feet.x, h, feet.z);
        if let Some(top) = find_pilotis(feet, support, facing, true, guidance, collision) {
            return Some(TransitionSetup::ToPilotis(PilotisEntry { top, from: feet, facing, kind: PilotisEntryType::FromInAir, foot }));
        }
    }
    let (p0, p1, point) = beam_at(feet, facing, guidance, collision)?;
    let axis = Vec3::new(p1.x - p0.x, 0.0, p1.z - p0.z).normalize_or_zero();
    let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or(Vec3::NEG_Z);
    Some(TransitionSetup::ToBeam(BeamEntry { p0, p1, point, from: feet, toward_p1: axis.dot(f) >= 0.0, mode: BeamEntryMode::Reception, foot, action: None, facing: f }))
}

pub fn update_air(
    time: Res<Time>,
    pad: Res<PadInput>,
    collision: Res<CollisionWorld>,
    guidance: Res<GuidanceWorld>,
    abilities: Option<Res<super::abilities::AbilitySet>>,
    skeleton: Option<Res<crate::bone_pose::PoseSkeleton>>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle, Option<&crate::bone_pose::BonePose>), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    // the InAir empty-hand catch needs Grasp (0xEDBC26)
    let grasp = super::abilities::of(abilities.as_deref()).allows(super::abilities::Ability::Grasp);
    for (mut loco, mut body, mut data, pose) in &mut q {
        if loco.current != ActorContextId::InAir {
            continue;
        }
        if loco.just_switched {
            if let Some(v) = data.air.entry_velocity.take() { body.velocity = v; }
            loco.just_switched = false;
            // the side set-up first (it re-picks the actions), then the down variants' sweep
            data.air.begin_turn(body.heading);
            data.air.apply_down_variants(&collision);
            if loco.switched_this_frame {
                // the controller keeps integrating the take-off velocity; the jump's path shifts with it so the
                // linear correction (0xE0DEF0) still ends on the target
                let before = body.feet;
                super::coast(&mut body, &collision, dt);
                let d = body.feet - before;
                if let AirMode::Jump { from, clip_end, .. } = &mut data.air.mode {
                    *from += d;
                    *clip_end += d;
                }
                continue;
            }
        }
        let air = &mut data.air;
        air.time_in_air += dt;
        if GAME_FALLS {
            air.speed_ratio = super::falls::speed_ratio(&pad, body.forward());
            // Record current feet before integration, on the first descending update (0xE0DFC7 / 0xE0DD15), not while
            // the playing item is a takeoff (word 0x1000, 0xE0DF99): a takeoff that already descends (the step off an
            // edge) records the apex after it (RE/24 AIR-31)
            if !air.apex_reached && body.velocity.y < -0.001 && !air.playing_word().is_some_and(|w| w & super::anim_gate::TAKEOFF != 0) {
                air.apex_reached = true;
                air.apex_y = body.feet.y;
            }
        }

        let mut landed_at: Option<f32> = None;
        // landed by arriving on a jump target (0xE07D00) rather than by ground contact (0xE05200)
        let mut on_target = false;
        let mut hang_on: Option<LedgeEntry> = None;
        let mut hay_on: Option<super::hay::HayStackEntry> = None;
        let mut kiosk_on: Option<super::kiosk::KioskEntry> = None;
        let mut narrow_on: Option<TransitionSetup> = None;
        let mut pass_on: Option<super::passover::PassOverEntry> = None;
        let mut swing_on = false;
        let mut ladder_on: Option<super::ladder::LadderEntry> = None;
        let mut climb_on: Option<super::climb::ClimbEntry> = None;
        let mut catch_hang: Option<(u32, bool, Option<bool>)> = None;
        let mut step_off: Option<(u32, Vec3)> = None;
        // a flat landing contact where the edge landing may be searched (after the catches)
        let mut edge_contact = false;
        let foot = (!air.foot_left) as usize;
        match air.mode {
            AirMode::Jump { from, clip_end, aim, duration, t, then_fall_to, t_takeoff, fwd } => {
                let t1 = (t + dt).min(duration);
                let s = t1 / duration;
                // the takeoff + flight items' blended root motion (0xE0DEF0)
                let clip_pos = from + to_world(jump_disp(air, t1, t_takeoff, duration), fwd);
                // linear correction toward the real target (0xE0DEF0)
                let path = |tt: f32| -> Vec3 {
                    let ss = (tt / duration).clamp(0.0, 1.0);
                    let cp = from + to_world(jump_disp(air, tt.clamp(0.0, duration), t_takeoff, duration), fwd);
                    cp + (aim - clip_end) * ss
                };
                let next = clip_pos + (aim - clip_end) * s;
                // the path's own velocity (not the collision-resolved displacement: a depenetration push is not
                // a velocity, using it flung the character up when a foot clipped the lip of a higher roof)
                let path_vel = (next - path(t1 - dt.max(1e-3))) / dt.max(1e-3);
                // The game drives the jump as a velocity through the character proxy (0xE0DEF0 → controller +0x90,
                // 0x57C7C0): the body moves by the path's step and slides along what it touches (a lip's rounded
                // contact lifts it over). PORT: a lag behind the path (a contact held the body back) is closed at
                // `JUMP_LAG_RECOVERY`/s so the arrival still lands on the target; driving the absolute position instead
                // made the body stall on a lip, then jump ahead to catch up.
                let lag = path(t) - body.feet;
                let mv = (next - path(t)) + lag * (dt * JUMP_LAG_RECOVERY).min(1.0);
                let r = { let b = &mut *body; collision.move_capsule(&mut b.proxy, b.feet, mv, false, dt) };
                body.velocity = path_vel;
                body.heading = air.jump_heading(t1, t_takeoff, fwd);
                body.feet = r.position;
                let blocked = (r.position - next).length() > JUMP_BLOCKED_LAG;
                if air.free_target && r.landed && path_vel.y <= 0.0 {
                    // the free jump comes down on the ground before its target (0xE05200, controller contact)
                    landed_at = Some(body.feet.y);
                } else if blocked {
                    // held back by a wall (anti-stuck 0xE0B1E0, simplified): fall from here with the path's
                    // horizontal velocity damped and no upward push. PORT: the 30 % kept has no game source; the game's
                    // support-loss re-entry (0xE01B70, RE/24 AIR-24) is not ported (RE/24 AIR-36)
                    air.mode = AirMode::Fall { steer_to: None };
                    body.velocity.x *= 0.3;
                    body.velocity.z *= 0.3;
                    body.velocity.y = body.velocity.y.min(0.0);
                } else if t1 >= duration {
                    // arrival (0xE07D00, within 0.01 m, 0xE07F2A): while Data+505 is set the tested point is the root
                    // requested from the controller (`CharacterController__RequestMoveTo` 0xE0F149, var_80), the path
                    // that ends on the target, not the body; the body is not moved onto it, the reception starts from
                    // where the controller left it (RE/24 AIR-33). The path's last step is the linear correction's full
                    // offset, so it ends on the aim and the test passes at the flight's end
                    debug_assert!((path(t1) - aim).length() < ARRIVAL_TOLERANCE, "{} {}", path(t1), aim);
                    // hand-off by target type (ledge targets → Ledge context)
                    let mut no_arrival = false;
                    if let Some((mid, normal)) = air.target.and_then(|t| t.hang) {
                        // the reception by the flight that arrived (CheckJumpTargetArrival 0xE07D00)
                        let t = air.target.unwrap();
                        let arrival = match t.straight {
                            Some(j) => LedgeArrival::Straight(j),
                            None => LedgeArrival::Surface { free: t.type_flags == super::targets::TARGET_LEDGE_FREE },
                        };
                        let mut e = LedgeEntry::at(mid, normal, body.feet, LedgeSubState::HangWallReception);
                        match arrival {
                            // the generic branch (0xE093CC): foot holds on the wall (`sub_B15C30`) → the wall
                            // reception (SubState 6) whatever the jump type; no holds → the swing (SubState 8) for a
                            // free-hang jump (0x80), else no arrival (the air catch takes over)
                            LedgeArrival::Surface { free } => {
                                if super::ledge::reception_feet(mid, normal, &collision) {
                                    let arrival = LedgeArrival::Surface { free: false };
                                    // the fall height (`GetFallHeight` 0xE07FB0) and the jump origin (Data+64) pick min / max
                                    let fall = if air.apex_reached { air.apex_y - body.feet.y } else { 0.0 };
                                    let w = ledge_moves::wall_reception_weights(fall, air.start, mid, 0.0);
                                    let mut m = ledge_moves::arrival_move(arrival, body.feet, e.hand_l, e.hand_r, normal, w, &collision);
                                    if free {
                                        ledge_moves::use_free_jump_wall_reception(&mut m, w);
                                    }
                                    e.entry_move = Some(m);
                                    hang_on = Some(e);
                                } else if free {
                                    swing_on = true;
                                    hang_on = Some(e);
                                } else {
                                    no_arrival = true;
                                }
                            }
                            LedgeArrival::Straight(_) => {
                                e.entry_move = Some(ledge_moves::arrival_move(arrival, body.feet, e.hand_l, e.hand_r, normal, [0.0; 6], &collision));
                                hang_on = Some(e);
                            }
                        }
                    }
                    // ladder target (0x1000): 0xE07D00 plays `ARRIVE_TARGET`, interpolates the root onto it (0.2 s) and
                    // sets Ladder EntryType 1 (state 2 waits for the action, then the low wait)
                    if let Some(mut e) = air.target.and_then(|t| t.ladder) {
                        e.from = body.feet;
                        // the arrival's first item blends [up, down, long] by InAirData+452 (sub_502E30): `sub_B1EAA0`'s
                        // triplet for a ladder target (RE/24 AIR-6)
                        if let Some(w) = air.arrival_w {
                            e.w = w;
                        } else if let Some(f) = air.flight {
                            for (k, x) in f.weights().iter().enumerate().take(3) {
                                e.w[k] = *x;
                            }
                        }
                        ladder_on = Some(e);
                    }
                    // kiosk target (0x400): 0xE07D00 sets InAir+225 bit 8, entry FreeStep and the side, then
                    // `HumanInAir__RequestKiosk` 0xE015A0 → context 19 (RE/18 §4)
                    if air.target_flags == 0x400 {
                        if let Some(t) = air.target {
                            if let Some(axis) = super::kiosk::kiosk_axis(t.position, &guidance) {
                                let dir = Vec3::new(t.position.x - air.start.x, 0.0, t.position.z - air.start.z).normalize_or(fwd);
                                kiosk_on = Some(super::kiosk::KioskEntry { from: body.feet, target: t.position, axis, dir, facing: body.forward() });
                            }
                        }
                    }
                    // pass-over target (type 2): 0xE07D00 case 2 → Ledge HandPassOver
                    if let Some((edge, normal)) = air.target.and_then(|t| t.pass) {
                        let mut fw = [0.0f32; 5];
                        if let Some(f) = air.flight {
                            for (k, x) in f.weights().iter().enumerate().take(5) {
                                fw[k] = *x;
                            }
                        }
                        pass_on = Some(super::passover::PassOverEntry { edge, normal, from: body.feet, flight_foot_left: air.foot_left, flight_w: fw, reception: None });
                    }
                    if air.target_flags == jump_blend::TARGET_HAYSTACK && then_fall_to.is_none() {
                        // PORT: the game requests the haystack on every arrival at the target (its owner entity); the
                        // port finds the stack by its box widened 0.3 m (no game source, RE/24 AIR-36)
                        if let Some(stack) = guidance.haystacks.iter().find(|s| {
                            let p = body.feet;
                            p.x >= s.min.x - 0.3 && p.x <= s.max.x + 0.3 && p.z >= s.min.z - 0.3 && p.z <= s.max.z + 0.3
                        }) {
                            // arrival on a haystack target (0xE07D00 clears InAir+340 → RequestHayStack 0xE01AF0 →
                            // 0xE00E30: entry Top, the faith landing, for every target arrival)
                            hay_on = Some(super::hay::HayStackEntry { stack: *stack, kind: super::hay::HayEntry::Top, from: body.feet, speed: body.velocity.length() });
                        }
                    }
                    // a jump without a real target (on the spot / free jump) can come down on a beam or a pilotis
                    if !AIR_CATCHES && air.target.is_none() && hang_on.is_none() {
                        narrow_on = narrow_catch(body.feet, fwd, foot, &guidance, &collision);
                    }
                    match then_fall_to {
                        _ if hang_on.is_some() || hay_on.is_some() || narrow_on.is_some() || pass_on.is_some() || ladder_on.is_some() || kiosk_on.is_some() => {}
                        // 0xE07D00 returns 0: the jump ends with no reception and the fall's air catch takes over
                        _ if no_arrival => air.mode = AirMode::Fall { steer_to: None },
                        // PORT: "no ground under the feet" (0.05 / 0.1 m) has no game source (RE/24 AIR-36)
                        Some(p) if collision.ground_height(body.feet + Vec3::Y * 0.05, 0.1).is_none() => {
                            // the tail's start velocity (0xE0F038–0xE0F12A): half of this frame's root step plus the
                            // part of the frame after the item ended times the last step, m / frame
                            let step = next - path(t);
                            let frac = (1.0 - (duration - t) / dt.max(1e-4)).clamp(0.0, 1.0);
                            air.tail_v = (step + step * frac) * 0.5;
                            air.mode = AirMode::Fall { steer_to: if p == aim { None } else { Some(p) } };
                        }
                        // a free-step arrival (case 1 / 0x10000) with a ceiling contact (a controller contact of class 8,
                        // `sub_4F8C40`(8), 0xE07F47–0xE07F59; the classes are `sub_71B840`'s) is cancelled into a fall
                        // (`sub_E009F0`: request 33, blend 0.3, vz ≤ 0, the target cleared) (RE/24 AIR-10)
                        None if air.target.is_some() && matches!(air.target_flags, 1 | 0x10000) && crate::proxy::has_contact(&body.proxy.manifold, 8) => {
                            air.target = None;
                            air.target_flags = 0;
                            body.velocity.y = body.velocity.y.min(0.0);
                            air.free_step_abort = true;
                            air.mode = AirMode::Fall { steer_to: None };
                        }
                        // a free-step arrival (case 1 / 0x10000) interpolates the root onto the target first
                        None if air.target.is_some() && matches!(air.target_flags, 1 | 0x10000) => {
                            air.mode = AirMode::Arrive { from: body.feet, to: aim + body.forward() * 0.5, t: 0.0 };
                        }
                        None => {
                            landed_at = Some(body.feet.y);
                            on_target = true;
                        }
                        _ => landed_at = Some(body.feet.y),
                    }
                } else {
                    air.mode = AirMode::Jump { from, clip_end, aim, duration, t: t1, then_fall_to, t_takeoff, fwd };
                }
            }
            AirMode::Fall { steer_to } => {
                body.velocity.y -= GRAVITY * dt;
                let mut h = Vec3::new(body.velocity.x, 0.0, body.velocity.z);
                match steer_to {
                    // the over-drop tail (Data+506, `UpdateJumpMotion` 0xE0F92D–0xE0FB43, RE/24 AIR-4), in m / frame:
                    // vz' = vz − 0.326·dt; t = `TimeToReachHeight`(z, min(target z, z + vz'), vz') with the m / frame
                    // vz'; the horizontal velocity is replaced by `SolveCorrectionAccel`'s c, or by
                    // normalize(c − v)·15·dt when it changes by more than that; the position becomes (x, z + vz', y) + v
                    // (the vertical step counted twice, 0xE0F9A5 / 0xE0FB21). At or below the target's height the game
                    // integrates nothing (the contact ends it). PORT: the step goes through the capsule move, and below
                    // the target's height the plain fall carries on (the port's contacts may not end it there).
                    // the let-go's first item plays (no tail, 0xE0F772); when it ends the faith dive starts (0x23A949B5,
                    // blend 0.6·t) and the tail steers to the haystack
                    Some(_) if air.dive_wait.is_some() => {
                        let w = air.dive_wait.unwrap_or(0.0) - dt;
                        if w > 0.0 {
                            air.dive_wait = Some(w);
                        } else {
                            air.dive_wait = None;
                            air.flight = Some(ActionBlend::new(jump_blend::FLIGHT_FAITH, 0, &[1.0]));
                        }
                    }
                    Some(p) if GAME_FALLS && body.feet.y > p.y => {
                        let v = air.tail_v;
                        let vz = v.y - dt * 0.326;
                        let new_y = body.feet.y + vz;
                        let t_left = super::falls::time_to_height(body.feet.y, p.y.min(new_y), vz);
                        let flat = |q: Vec3| Vec3::new(q.x, 0.0, q.z);
                        let v_old = flat(v);
                        let c = if t_left > 1e-6 { super::falls::correction(flat(body.feet), flat(p), v_old, t_left, dt) } else { v_old };
                        let change = c - v_old;
                        let hv = if change.length() > 15.0 * dt { change.normalize_or_zero() * (15.0 * dt) } else { c };
                        air.tail_v = Vec3::new(hv.x, vz, hv.z);
                        let to = Vec3::new(body.feet.x, new_y, body.feet.z) + air.tail_v;
                        body.velocity = (to - body.feet) / dt.max(1e-4);
                        h = Vec3::new(body.velocity.x, 0.0, body.velocity.z);
                    }
                    Some(p) => {
                        // steer horizontally onto the real target, ≤ 15 m/s (0xE0DEF0 / 0xE00730)
                        if GAME_FALLS {
                            let t_left = super::falls::time_to_height(body.feet.y, p.y, body.velocity.y);
                            if t_left > 0.0 {
                                // PORT: Body stores m/s and integrates variable dt; native tail stores m/frame.
                                let target = Vec3::new(p.x, body.feet.y, p.z);
                                h += super::falls::correction(body.feet, target, h, t_left, dt);
                            }
                            h = h.clamp_length_max(FREEFALL_MAX_HORIZONTAL);
                        } else {
                            let t_left = (2.0 * (body.feet.y - p.y).max(0.01) / GRAVITY).sqrt().max(dt);
                            h = (Vec3::new(p.x - body.feet.x, 0.0, p.z - body.feet.z) / t_left).clamp_length_max(FREEFALL_MAX_HORIZONTAL);
                        }
                    }
                    None => {
                        // Non-target drift 0xE0F643–0xE0F707, constants 0x19BA434/438. PORT: the game drifts only while
                        // `AnimComponent__GetSlotMotionMode`(0) == 1 and no BounceOffHuman deadline runs (`sub_DFEEE0`,
                        // HumanInAir+0x1A8). Mode 1 is written only by the physics blend at Human+0x830 in its states 2 / 3
                        // (`HumanPhysicsBlend__SetPhysicsDriven` 0xB31530; drowning requests state 2), its init writes 0
                        // (RE/24 AIR-21). Hypothesis: a normal fall runs in mode 0 and skips this decay; LIVE #14 decides,
                        // until then the port decays.
                        // 0x4F8C40: decay only without controller contacts. PORT: use the proxy manifold
                        // as the native contact-list adapter.
                        // PORT: a fall entered faster than the cap (a free jump at sprint speed, 6.3 m/s) decays onto it
                        // at the same 4 m/s² instead of being clamped in one frame (a visible lurch). The game reaches
                        // this path from its own entries only (LIVE, RE/20 #14: horizontal speed of a sprint off an edge).
                        let len = h.length();
                        let new_len = (len - DRIFT_DECEL * dt).max(0.0);
                        let new_len = if len > DRIFT_MAX { new_len.max(DRIFT_MAX) } else { new_len.min(DRIFT_MAX) };
                        h = if GAME_FALLS { super::falls::drift(h, dt, !body.proxy.manifold.is_empty()) } else if len > 1e-4 { h * (new_len / len) } else { Vec3::ZERO };
                    }
                }
                body.velocity.x = h.x;
                body.velocity.z = h.z;
                // `HumanInAir__DropSteerAwayFromWall` 0xE04B60: for the first 0.3 s of a drop the facing turns toward the
                // edge normal (away from it with the back to the drop) at 3 rad/s until within 30 deg (fully, with no edge)
                if let Some((_, side, n)) = air.drop.filter(|_| air.fall_t < 0.3) {
                    let want = n.map(|n| if side == 1 { -n } else { n }).unwrap_or(body.forward());
                    let want = Vec3::new(want.x, 0.0, want.z).normalize_or(body.forward());
                    let cone = if n.is_some() { 30f32.to_radians() } else { 0.0 };
                    let f = body.forward();
                    let a = right_of_signed(f, want);
                    if a.abs() > cone {
                        let step = (a.abs() - cone).min(3.0 * dt) * a.signum();
                        body.heading += step;
                    }
                }
                air.fall_t += dt;
                // narrow objects are caught before the ground contact (0xE0BB70 runs before 0xE05200)
                if !AIR_CATCHES && body.velocity.y <= 0.0 && air.apex_y - body.feet.y < 9.0 && air.time_in_air > 0.1 {
                    narrow_on = narrow_catch(body.feet + body.velocity * dt, body.forward(), foot, &guidance, &collision);
                }
                let before = body.feet;
                air.anti_stuck.tick(dt);
                let r = if air.hollywood {
                    // the HollywoodMode layer: the dive passes through (`CollisionFilterInfo__SetLayer(28)` 0xEEC622)
                    crate::collision::MoveResult { position: body.feet + body.velocity * dt, velocity: body.velocity, hit_wall: false, hit_ceiling: false, landed: false }
                } else {
                    let b = &mut *body;
                    collision.move_capsule(&mut b.proxy, b.feet, b.velocity * dt, false, dt)
                };
                body.feet = r.position;
                // PORT: static proxy contacts have no character bodies. While the anti-stuck nudge's gate runs,
                // contacts within 0.3 m below the start do not count (0xE038A0, Data+528)
                let gated = air.anti_stuck.ignores_contacts(air.start_y, body.feet.y);
                let contact = if GAME_GROUND_EXTRAS {
                    super::ground_extras::ground_contact(body.proxy.manifold.iter().map(|c| c.normal.y))
                } else { super::ground_extras::GroundContact::None };
                if contact == super::ground_extras::GroundContact::Steep {
                    air.slope_slide.contact(contact, body.feet.y);
                    let height = if air.apex_reached { air.apex_y-body.feet.y } else { 0.0 };
                    air.slope_slide.check(height,body.feet.y,false);
                    // PORT: articulated RagFall is a logged dependency; retain the required flag instead of
                    // inventing a ragdoll/recovery animation (native 0xE05200 -> 0xDCE480).
                    body.velocity = r.velocity;
                }
                if narrow_on.is_some() {
                    // caught: the entry warps the root onto the beam / top
                } else if r.hit_wall && contact != super::ground_extras::GroundContact::Steep {
                    body.velocity.x = 0.0;
                    body.velocity.z = 0.0;
                }
                if r.hit_ceiling && body.velocity.y > 0.0 {
                    body.velocity.y = 0.0;
                }
                if GAME_FALLS && narrow_on.is_none() {
                    // 0xE05490 checks hay before ordinary landing: a controller contact with a haystack (type 3,
                    // descriptor 8 / 9); InAir+340 = the contact normal's z ≤ 0.9, so through the top → Top, against a
                    // side → SideJump (`EnterFromAir` 0xE42700, `xx_h_air_to_haystack`). The game takes it only while
                    // InAir knows the haystack (+336, `sub_E03A90`: within 3.5 m in height), i.e. from a jump at a
                    // haystack target that ends in a free fall (0xB1B8C0) or a let-go record (0xDD7810). PORT: the
                    // greybox haystacks are not solid; their box stands in for the contact (the top crossed, or a side
                    // entered by the capsule's 0.45 m), and every fall takes it (LIVE, RE/20 #1: a plain fall onto a haystack).
                    let next = body.feet;
                    let previous = next - body.velocity * dt;
                    let top = guidance.haystacks.iter().find(|stack| {
                        body.velocity.y < 0.0 && previous.y >= stack.max.y && next.y <= stack.max.y
                            && next.x >= stack.min.x && next.x <= stack.max.x
                            && next.z >= stack.min.z && next.z <= stack.max.z
                    });
                    let side = |p: Vec3, s: &crate::collision::Aabb3| {
                        p.x >= s.min.x - HAY_SIDE_R && p.x <= s.max.x + HAY_SIDE_R && p.z >= s.min.z - HAY_SIDE_R && p.z <= s.max.z + HAY_SIDE_R
                    };
                    hay_on = top
                        .map(|stack| (stack, super::hay::HayEntry::Top))
                        .or_else(|| {
                            guidance
                                .haystacks
                                .iter()
                                .find(|s| side(next, s) && !side(previous, s) && next.y < s.max.y && next.y + 1.8 > s.min.y)
                                .map(|stack| (stack, super::hay::HayEntry::SideJump))
                        })
                        .map(|(stack, kind)| super::hay::HayStackEntry { stack: *stack, kind, from: body.feet, speed: body.velocity.length() });
                }
                // InAir sub-state 3, the edge landing (0xE0D684): a flat contact with edges at the feet, under 9 m; it is
                // searched last, after the catches (below, RE/24 AIR-16)
                let fall_h = if air.apex_reached { air.apex_y - body.feet.y } else { 0.0 };
                // not during the faith dive (`!vt72()` 0xE0D684, RE/24 AIR-32)
                edge_contact = AIR_CATCHES && narrow_on.is_none() && hay_on.is_none() && r.landed && body.velocity.y <= 0.0 && !air.edge_landed && !air.faith_dive()
                    // PORT: the proxy's contact list is empty on the landing frame; a landing that is not a steep
                    // contact is the flat contact (GetGroundContactType = 1)
                    && fall_h < 9.0 && contact != super::ground_extras::GroundContact::Steep;
                if narrow_on.is_some() || hay_on.is_some() {
                } else if r.landed && body.velocity.y <= 0.0 && !gated {
                    landed_at = Some(body.feet.y);
                } else if let Some(v) = (dt > 0.0).then(|| (body.feet - before) / dt).and_then(|moved| {
                    // `HumanInAir__AntiStuckNudge` 0xE0B1E0: after every landing check failed, a body held still (or
                    // sliding sideways on a contact) for 0.5 s is pushed out
                    let has_contacts = !body.proxy.manifold.is_empty();
                    // the sideways case reads Data+505 (0xE0B73D), the target-driven part of a jump, which the flight's
                    // end clears (0xE0F268): never set in the Fall mode, even with the target kept for the tail
                    // (RE/24 AIR-21)
                    air.anti_stuck.update(dt, moved, false, has_contacts, body.feet, body.forward(), |p| collision.capsule_fits(p))
                }) {
                    body.velocity = v;
                } else if !AIR_CATCHES && grasp && if GAME_FALLS { pad.hand_held && air.apex_reached && air.apex_y - body.feet.y > 0.3 } else { pad.legs_held && body.velocity.y <= 0.5 && air.time_in_air > 0.3 } {
                    // grab requested (SetGrabRequested 0xE102D0) → a ladder first (FindLadderCatch 0xE04100), then a ledge
                    // in reach
                    if let Some(e) = super::ladder::find_ladder_catch(body.feet, body.forward(), true, &guidance, &collision) {
                        ladder_on = Some(e);
                    } else if let Some(h) = find_air_catch(body.feet, body.forward(), &guidance) {
                        air.long_catch = air.apex_y - body.feet.y >= 3.0;
                        hang_on = Some(LedgeEntry { catch: Some(air.long_catch), ..LedgeEntry::at(guidance.fit_hands(h.point, h.wall_normal), h.wall_normal, body.feet, LedgeSubState::HangWallReception) });
                    }
                }
            }
            AirMode::Arrive { from, to, t } => {
                let t1 = (t + dt).min(FREESTEP_ARRIVE);
                let next = from.lerp(to, t1 / FREESTEP_ARRIVE);
                let r = { let b = &mut *body; collision.move_capsule(&mut b.proxy, b.feet, next - b.feet, false, dt) };
                body.feet = r.position;
                body.velocity = Vec3::ZERO;
                if t1 >= FREESTEP_ARRIVE {
                    landed_at = Some(body.feet.y);
                    on_target = true;
                } else {
                    air.mode = AirMode::Arrive { from, to, t: t1 };
                }
            }
            AirMode::Idle => {
                air.mode = AirMode::Fall { steer_to: None };
            }
        }

        // no catch during the free-step arrival: the game has already arrived (it switched context with the root
        // interpolation running, 0xE07D00)
        let arriving = matches!(air.mode, AirMode::Arrive { .. });
        if AIR_CATCHES && !arriving && !on_target && ladder_on.is_none() && hang_on.is_none() && pass_on.is_none() {
            let fall_height = if air.apex_reached { air.apex_y - body.feet.y } else { 0.0 };
            // the empty-hand catch needs `!vt72()`: none during the faith dive (0xE0BF71, RE/24 AIR-32)
            let manual = grasp && !air.faith_dive() && super::air_catches::manual(pad.hand_held, fall_height, air.drop.map(|d| d.0));
            // Data+505: set by the jump's set-up, cleared when its flight ends without an arrival (0xE0F268)
            let jumping = matches!(air.mode, AirMode::Jump { .. });
            let automatic = super::air_catches::automatic(air.time_in_air, air.drop.is_some(), jumping, air.target.map(|t| t.type_flags), body.forward(), body.proxy.manifold.iter().map(|c| c.normal));
            // 0xE0BB70: raw reach direction (+32), seeded from facing when the stick is zero.
            let reach = if manual && pad.speed01 > 0.0 && pad.dir_snap != Vec3::ZERO { pad.dir_snap } else { body.forward() };
            if manual || automatic {
                if let Some(mut e) = super::ladder::find_ladder_catch(body.feet, reach, manual, &guidance, &collision) {
                    e.facing = body.forward();
                    e.catch_speed = body.velocity.length();
                    ladder_on = Some(e);
                } else if let Some(found) = super::air_catches::ledge_search(body.feet, reach, body.forward(), fall_height, manual,
                    // the Hips bone of last frame's pose at the root where it stands (`bone_pose`)
                    skeleton.as_deref().zip(pose).and_then(|(s, p)| p.world_pos(s, crate::bone_pose::HIPS, &body)),
                    air.release_hold, &guidance, &collision) {
                    // CheckAirCatch 0xE0BB70: the ledge, climb and free-hang catches (RE/19 §2)
                    air.long_catch = fall_height >= 3.0;
                    match found {
                        super::air_catches::CatchFound::Climb(e) => climb_on = Some(e),
                        super::air_catches::CatchFound::Hang { mid, normal, wall, action, one_hand } => {
                            let sub = if wall { LedgeSubState::HangWallReception } else { LedgeSubState::HangFreeReception };
                            hang_on = Some(LedgeEntry { catch: Some(air.long_catch), ..LedgeEntry::at(mid, normal, body.feet, sub) });
                            catch_hang = Some((action, wall, one_hand));
                        }
                    }
                }
            }
            if ladder_on.is_none() && hang_on.is_none() && climb_on.is_none() && super::air_catches::narrow(fall_height, air.time_in_air, air.drop.is_some()) {
                let ahead = super::air_catches::target_ahead(body.feet, air.target, &collision);
                // the beam too only with no jump target ahead: its call (0xE0D41D) sits outside the pilotis'
                // `IsJumpTargetAhead` block, but `HumanInAir__FindBeamCatch` 0xE0B890 runs the same test itself. The
                // gate's InAir+104 is a deadline armed once at the InAir entry (0.4 s, 0xE01D4A) (RE/24 AIR-16)
                if !ahead { narrow_on = narrow_catch(body.feet, body.forward(), foot, &guidance, &collision); }
                // the knee catch (type 0, 0xE0D58B): an edge at the feet with a drop in front of it and room on top
                if narrow_on.is_none() && !ahead {
                    if let Some((edge, n)) = super::air_catches::knee_catch(body.feet, body.forward(), &guidance, &collision) {
                        let action = super::air_catches::KNEE_CATCH[(fall_height >= 3.0) as usize];
                        let (knee, stand) = super::ledge_moves::knee_catch_moves(action, body.feet, edge, n);
                        let mut e = LedgeEntry::at(edge, n, body.feet, LedgeSubState::Pullup);
                        e.entry_move = Some(knee);
                        e.entry_rest = [Some(stand), None];
                        hang_on = Some(e);
                    }
                }
            }
            // the edge landing last (`CheckAirCatch` LABEL_140), behind the same 0.3 m / 0.4 s gate (LABEL_56) as the
            // narrow catches (RE/24 AIR-16)
            let gate = fall_height > 0.3 || air.time_in_air >= 0.4;
            if edge_contact && gate && ladder_on.is_none() && hang_on.is_none() && climb_on.is_none() && narrow_on.is_none() {
                step_off = super::air_catches::edge_landing(body.feet, body.forward(), &guidance, &collision);
            }
        }

        // apex tracking: first frame vertical velocity < -0.001 (RE/04 §1)
        if !GAME_FALLS && !air.apex_reached && body.feet.y - air.prev_y < -0.001 * dt {
            air.apex_reached = true;
            air.apex_y = air.prev_y;
        }
        if !air.apex_reached {
            air.apex_y = air.apex_y.max(body.feet.y);
        }
        air.prev_y = body.feet.y;
        body.grounded = false;

        if let Some(e) = climb_on {
            // the climb catch (type 1): the catch action plays as the climb's entry while the root goes to the holds'
            // pose. PORT: the game enters the Ledge context (SubState 13, catch type 1) first.
            air.mode = AirMode::Idle;
            body.velocity = Vec3::ZERO;
            switch_context(&mut loco, &mut data, TransitionSetup::ToClimb(e));
        } else if let Some((action, facing)) = step_off {
            // the edge landing: the step-off action, then the fall (request 33), facing the chosen way
            body.heading = super::heading_of(facing);
            body.velocity.y = 0.0;
            let blend = super::ledge_moves::single_item(action, 0);
            let fall = super::ledge_moves::single_item(action, 1);
            if let Some(a) = blend {
                // LABEL_140 stays in InAir (sub-state 3): the apex point becomes the feet (Data+208, `sub_DFF500`), the
                // jump origin (Data+16) stays and the release record is cleared (Data+500 = 0, 0xE0D8CF) (RE/24 AIR-17)
                let (start, start_y) = (air.start, air.start_y);
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::OnPlace { from: body.feet, fwd: facing, action: a, fall }));
                data.air.edge_landed = true;
                data.air.start = start;
                data.air.start_y = start_y;
                data.air.release_hold = None;
            }
        } else if let Some(e) = ladder_on {
            air.mode = AirMode::Idle;
            body.velocity = Vec3::ZERO;
            switch_context(&mut loco, &mut data, TransitionSetup::ToLadder(e));
            if AIR_CATCHES { data.ladder.advance_catch_root(&mut body, dt); }
        } else if let Some(entry) = hang_on {
            air.mode = AirMode::Idle;
            body.velocity = Vec3::ZERO;
            let flight = air.flight;
            switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(entry));
            if let Some((action, wall, one_hand)) = catch_hang {
                data.ledge.catch_action = Some(action);
                data.ledge.catch_free = !wall;
                data.ledge.catch_one_hand = one_hand;
            }
            if swing_on {
                let n = entry.normal;
                let root = super::ledge::hang_root_at(entry.hand_l, entry.hand_r, n, super::ledge::LedgeHangType::Free, &collision);
                let fwd = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
                let seq = data.ledge.swing.map(|s| s.seq).unwrap_or(0);
                data.ledge.swing = super::swing::enter(body.feet, root, fwd, (entry.hand_l + entry.hand_r) * 0.5, flight, seq);
                let on_pole = [entry.hand_l, entry.hand_r].iter().all(|h| guidance.on_edge(*h, n, 0.15).is_some_and(|hit| guidance.edges[hit.edge].subtype == crate::guidance::GuidanceSubType::Pole));
                if let Some(s) = data.ledge.swing.as_mut() {
                    s.on_pole = on_pole;
                }
                data.ledge.hang_type = super::ledge::LedgeHangType::Free;
                data.ledge.hang_set = true;
            }
        } else if let Some(e) = pass_on {
            air.mode = AirMode::Idle;
            body.velocity = Vec3::ZERO;
            switch_context(&mut loco, &mut data, TransitionSetup::ToPassOver(e));
        } else if let Some(setup) = narrow_on {
            air.mode = AirMode::Idle;
            body.velocity = Vec3::ZERO;
            switch_context(&mut loco, &mut data, setup);
            if AIR_CATCHES { data.narrow.advance_catch_root(&mut body, dt); }
        } else if let Some(e) = kiosk_on {
            air.mode = AirMode::Idle;
            body.velocity = Vec3::ZERO;
            switch_context(&mut loco, &mut data, TransitionSetup::ToKiosk(e));
        } else if let Some(e) = hay_on {
            air.mode = AirMode::Idle;
            body.velocity = Vec3::ZERO;
            switch_context(&mut loco, &mut data, TransitionSetup::ToHayStack(e));
        } else if let Some(y) = landed_at {
            body.grounded = true;
            let impact_velocity = body.velocity;
            body.velocity.y = 0.0;
            let mut landing = classify_landing(air.apex_y, air.apex_reached, air.start_y, y);
            landing.action = Some(if on_target && matches!(air.target_flags, 1 | 0x10000) {
                // free-step reception: the flight's weights, normal / `_fast` by +0x16C (0xE07D00).
                // PORT: the game continues in NarrowObject (on the edge); the port stays in Ground.
                let fw = air.flight.map(|f| f.weights().to_vec()).unwrap_or_default();
                let id = jump_blend::RECEPTION_FREESTEP[(!air.foot_left) as usize];
                landing.skip = FREESTEP_ARRIVE;
                ActionBlend::new(id, 0, &jump_blend::reception_weights(&fw, air.speed_ratio))
            } else {
                // ground landing (0xE05940): drop and horizontal distance from the jump/fall start; the
                // forward/straight test uses the trajectory pitch, not the pad (0xE05B64).
                let horiz = Vec2::new(body.feet.x - air.start.x, body.feet.z - air.start.z).length();
                let stick_forward = if GAME_FALLS { super::falls::forward_landing(impact_velocity) } else { pad.speed01 > 0.0 && pad.dir.dot(body.forward()) >= 75f32.to_radians().cos() };
                let (id, w) = jump_blend::landing(air.start_y - y, horiz, stick_forward, air.speed_ratio, air.foot_left);
                ActionBlend::new(id, 0, &w)
            });
            landing.roll = landing.action.is_some_and(|a| a.id == jump_blend::LAND_DAMAGE_ROLL);
            air.mode = AirMode::Idle;
            // A free-step arrival continues in NarrowObject (0xE07D00: SubState 6 → Movement); its Movement state
            // tries a pilotis first (`TryPilotisFreeStep` 0xE50190), then a beam (`TryMountBeam` 0xE52AD0), and
            // returns to Ground on wide support (0xE51190, the port's default).
            // Both mounts wait for the reception's end (IHumanNarrowObject vt152 0xE4C670: the playing action done;
            // RE/24 NARROW-4): the port plays the reception in Ground and tries them there (`ground::narrow_pending`).
            // the beam and the pilotis pick their clips by the reception's exit feet: `(word & 0xC) != 4` → footr, else
            // footl (PlayWait 0xF704B9, TryPilotisFreeStep 0xE502BA; RE/24 NARROW-7): 0x010DE1FE (after a footl flight)
            // ends right foot ahead, 0x010DF150 left foot ahead
            let reception = jump_blend::RECEPTION_FREESTEP[(!air.foot_left) as usize];
            let exit_foot = super::anim_gate::item_word(reception, 0).map_or(air.foot_left as usize, |w| ((w & 0xC) != 4) as usize);
            let pending = (on_target && matches!(air.target_flags, 1 | 0x10000)).then(|| (exit_foot, body.forward(), body.feet));
            switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: Some(landing) });
            data.ground.narrow_pending = pending;
        }
    }
}

#[cfg(test)]
mod fall_tests {
    use super::*;

    /// RE/24 AIR-2 / AIR-20: the aim is pulled 0.5 m back toward the jumper for the free-step types, not for hangs;
    /// a target past the over-drop is aimed `thr` down and pulled back by min(dist, 1.5)·k.
    #[test]
    fn the_aim_is_pulled_back_toward_the_jumper() {
        let from = Vec3::ZERO;
        let roof = Vec3::new(0.0, 1.0, -4.0);
        assert!((pull_back(from, roof, 1) - Vec3::new(0.0, 1.0, -3.5)).length() < 1e-5);
        assert_eq!(pull_back(from, roof, 0x40), roof, "hang targets keep their own aim");
        let (aim, fall) = over_drop(from, Vec3::new(0.0, -20.0, -6.0), OVERDROP);
        let k = (-20.0f32 + OVERDROP) / -30.0;
        assert!((aim - Vec3::new(0.0, -OVERDROP, -6.0 + 1.5 * k)).length() < 1e-5, "{aim}");
        assert_eq!(fall, Some(Vec3::new(0.0, -20.0, -6.0)));
        assert_eq!(over_drop(from, roof, OVERDROP), (roof, None));
    }

    /// RE/24 AIR-1: the free jump's target lies 3 m down. On solid ground ahead the sweep hits and the jump flies the
    /// down variants (the live no-target jump); off the edge of a roof over open air it keeps the free-step flight.
    #[test]
    fn the_down_variants_need_a_blocked_sweep() {
        use crate::collision::Aabb3;
        let flight_of = |floor_ahead: f32| {
            let mut c = CollisionWorld::default();
            c.boxes.push(Aabb3 { min: Vec3::new(-5.0, -1.0, -floor_ahead), max: Vec3::new(5.0, 0.0, 5.0) });
            let mut air = HumanInAirData::default();
            air.enter(InAirEntry::FreeJump { from: Vec3::ZERO, dir: Vec3::NEG_Z, ahead: FREE_JUMP_AHEAD, speed_param: 0.5, foot_left: true });
            air.apply_down_variants(&c);
            air.flight.map(|f| f.id).unwrap()
        };
        assert!(jump_blend::FLIGHT_FREESTEP_DOWN.contains(&flight_of(20.0)), "solid ground ahead: the down flight");
        assert!(jump_blend::FLIGHT_FREESTEP.contains(&flight_of(0.1)), "over open air: the free-step flight");
    }
    #[test]
    fn a_run_jump_turns_to_its_target_over_the_takeoff() {
        // bug 1791501015: the facing snapped 42° in one frame at takeoff. 0xE0EF30 slerps the jump frame from the
        // takeoff facing to the destination over min(T₁, 0.4 s).
        let mut air = HumanInAirData::default();
        let start = 0.0f32; // facing −Z
        let dir = Quat::from_rotation_y(-0.75) * Vec3::NEG_Z; // 43° to the right
        air.enter(InAirEntry::FreeJump { from: Vec3::ZERO, dir, ahead: FREE_JUMP_AHEAD, speed_param: 0.5, foot_left: true });
        air.begin_turn(start);
        let AirMode::Jump { t_takeoff, fwd, from, clip_end, .. } = air.mode else { panic!("not a jump") };
        assert!(t_takeoff > 0.1, "{t_takeoff}");
        assert!(fwd.distance(Vec3::NEG_Z) < 1e-5, "the clips play in the takeoff frame: {fwd}");
        let ahead = clip_end - from;
        assert!(Vec3::new(ahead.x, 0.0, ahead.z).normalize().dot(Vec3::NEG_Z) > 0.99, "clip end ahead of the takeoff facing: {ahead}");
        let h = |t: f32| air.jump_heading(t, t_takeoff, fwd);
        let dest = super::super::heading_of(dir);
        assert!((h(0.0) - start).abs() < 1e-5);
        let mid = h(t_takeoff.min(0.4) * 0.5);
        assert!((mid - (start + dest) * 0.5).abs() < 1e-4, "half way at half the turn: {mid} of {dest}");
        assert!((h(t_takeoff) - dest).abs() < 1e-5 && (h(t_takeoff + 0.3) - dest).abs() < 1e-5);
    }
    #[test]
    fn fall_entry_clears_previous_flight_and_keeps_all_native_origins() {
        for origin in [FallOrigin::Ground,FallOrigin::Climb,FallOrigin::HangWall,FallOrigin::HangFree] {
            let mut air=HumanInAirData::default();
            air.flight=Some(ActionBlend::new(1,0,&[1.0]));
            air.apex_reached=true;
            let from=Vec3::new(2.0,10.0,4.0); let velocity=Vec3::new(3.0,2.0,0.0);
            air.enter(InAirEntry::Fall{from,velocity,origin,speed_param:0.5});
            assert_eq!(air.fall_origin,origin);
            assert_eq!(air.entry_velocity,Some(velocity));
            assert!(!air.apex_reached && air.flight.is_none() && air.takeoff.is_none());
            assert_eq!(air.apex_y,from.y);
        }
    }
    /// RE/24 NARROW-7: the free-step receptions' exit feet: after a footl flight 0x010DE1FE ends right foot ahead (the
    /// beam's footr clips), after a footr flight 0x010DF150 left foot ahead (footl).
    #[test]
    fn the_free_step_receptions_exit_on_the_other_foot() {
        let exit = |id: u32| super::super::anim_gate::item_word(id, 0).map(|w| (w >> 2) & 3);
        assert_eq!(exit(jump_blend::RECEPTION_FREESTEP[0]), Some(2), "right foot ahead");
        assert_eq!(exit(jump_blend::RECEPTION_FREESTEP[1]), Some(1), "left foot ahead");
    }

    /// RE/24 AIR-31: the takeoff item carries word bit 0x1000 (no apex record while it plays); the flight does not.
    #[test]
    fn the_takeoff_item_holds_the_apex_record() {
        let mut air = HumanInAirData::default();
        air.enter(InAirEntry::FreeJump { from: Vec3::ZERO, dir: Vec3::NEG_Z, ahead: FREE_JUMP_AHEAD, speed_param: 0.5, foot_left: true });
        let takeoff = |air: &HumanInAirData| air.playing_word().is_some_and(|w| w & super::super::anim_gate::TAKEOFF != 0);
        assert!(takeoff(&air), "{:?}", air.playing_word());
        let AirMode::Jump { t_takeoff, .. } = &mut air.mode else { panic!() };
        let t1 = *t_takeoff;
        if let AirMode::Jump { t, .. } = &mut air.mode { *t = t1 + 0.01 }
        assert!(!takeoff(&air), "{:?}", air.playing_word());
    }

    /// RE/24 AIR-32: the faith jump and the let-go's dive are `IsLeapOfFaithDiveAnim`; a plain fall and the let-go's
    /// own item before the dive are not.
    #[test]
    fn the_faith_dive_is_the_faith_actions() {
        let target = JumpTarget { position: Vec3::new(0.0, -10.0, -4.0), type_flags: jump_blend::TARGET_HAYSTACK, hang: None, straight: None, pass: None, ladder: None };
        let mut air = HumanInAirData::default();
        air.enter(InAirEntry::JumpToTarget { from: Vec3::ZERO, target, speed_param: 0.5, foot_left: true });
        assert!(air.faith_dive());
        air.enter(InAirEntry::Fall { from: Vec3::ZERO, velocity: Vec3::ZERO, origin: FallOrigin::Ground, speed_param: 0.0 });
        assert!(!air.faith_dive());
        air.enter(InAirEntry::LetGoDive { from: Vec3::ZERO, target, origin: FallOrigin::HangWall });
        assert!(!air.faith_dive(), "the let-go plays first");
        air.flight = Some(ActionBlend::new(jump_blend::FLIGHT_FAITH, 0, &[1.0]));
        assert!(air.faith_dive());
    }
    #[test]
    fn fall_height_is_zero_until_the_apex_flag() {
        assert_eq!(classify_landing(25.0,false,25.0,0.0).kind,LandingType::Safe);
        assert_eq!(classify_landing(25.0,true,25.0,0.0).kind,LandingType::Fatal);
    }
}
