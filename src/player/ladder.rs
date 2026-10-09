//! HumanLadder (context 5), RE/05 §4.
//!
//! Actions from the HumanLadderData animation table (ctor 0xC7CAC0: entry = +392 + 4·MvtAnimState + 2·foot +
//! inclination). "Low" / "High" are the low / high profile (`xx_l_` / `xx_h_` clips). [foot l, foot r] pairs.

/// MvtAnimState 0 / 1: waits.
pub const WAIT: [[u32; 2]; 2] = [[0x0106_8FF7, 0x0106_8FF8], [0x0106_902E, 0x0106_902F]];
/// 2 / 4: climb up, 3 / 5: climb down (each 2 items, l / r), [low, high].
pub const CLIMB_UP: [u32; 2] = [0x0106_8FFF, 0x0106_8FF9];
pub const CLIMB_DOWN: [u32; 2] = [0x0106_9001, 0x010C_9CEE];
/// 7 / 8: enter from the ground, [low, high] × [foot l, foot r]; 3 clips [straight, left, right].
pub const ENTER_GROUND: [[u32; 2]; 2] = [[0x010A_34A7, 0x010A_34A6], [0x010A_251E, 0x010A_2520]];
/// 9 / 10: enter from the top (the pull-down onto it), then its transition into the wait (0xE25240).
pub const ENTER_TOP: [[u32; 2]; 2] = [[0x010A_396B, 0x010A_396C], [0x010A_251C, 0x010A_3493]];
pub const ENTER_TOP_TO_WAIT: [u32; 2] = [0x010A_250E, 0x010A_250F];
/// 11 / 12: exit to the ground, 13 / 14: exit to the top.
pub const EXIT_GROUND: [[u32; 2]; 2] = [[0x010A_2F5B, 0x010A_2F5C], [0x010C_A51A, 0x010C_A6C3]];
pub const EXIT_TOP: [[u32; 2]; 2] = [[0x010A_349C, 0x010A_349D], [0x010A_37FC, 0x010A_37FD]];
/// 15 / 16: release (→ falling), 17 / 18: jump (`wait_tr_rebound`).
pub const RELEASE: [[u32; 2]; 2] = [[0x010A_3485, 0x010A_3486], [0x010A_3483, 0x010A_3484]];
pub const JUMP: [u32; 2] = [0x0449_33FE, 0x0449_33FF];
/// Approached from behind: the turn (0xE266D0, `xx_h_ladder_turn_{l,r}_a` + `_b_tr_h_wait`).
pub const TURN: [u32; 2] = [0x0A64_400C, 0x0A64_400D];
/// Hang → ladder at the side (`HumanLedge__TrySideMoveToLadder` 0xDD4CD0): [wall, free] × [left, right]. Free:
/// 1446415952 + (dir != left); wall: 1446415954 + (dir == left).
pub const FROM_LEDGE_SIDE: [[u32; 2]; 2] = [[0x5636_8E53, 0x5636_8E52], [0x5636_8E50, 0x5636_8E51]];
/// Climb → ladder at the side (`HumanClimb__TryLadder` 0xDF10C0): dir 4 (left) / any other side dir.
pub const FROM_CLIMB_SIDE: [u32; 2] = [0x5636_8E60, 0x5636_8E61];
/// The air catch (`HumanInAir__CheckAirCatch` 0xE0BB70 after `HumanInAir__FindLadderCatch` 0xE04100).
pub const CATCH_AIR: u32 = 0x156D_623D;
/// The arrival on a ladder jump target (`HumanInAir__CheckJumpTargetArrival` 0xE07D00, case 0x1000).
pub const ARRIVE_TARGET: u32 = 0x0292_58AA;

/// The sideways move off the ladder onto a ledge hang beside it (event 4, `sub_E1FC80`): [wall hang, free hang] ×
/// [left, right] (0xE20057 / 0xE20035 / 0xE200E3 / 0xE200C1).
pub const TO_HANG_SIDE: [[u32; 2]; 2] = [[0x5636_8E56, 0x5636_8E57], [0x5636_8E54, 0x5636_8E55]];
/// The side jump off the ladder (event 4's second search `sub_E228E0`, played by 0xE27000 / 0xE20650; table
/// `qword_1A2ED80` filled by 0xE1E060): [climb holds, wall hang, free hang] × [near (< 0.8 m), far] × [left, right],
/// each (takeoff, landing, then).
pub const SIDE_JUMP: [[[[u32; 3]; 2]; 2]; 3] = [
    [[[0x4912_284C, 0x4912_285F, 0x4912_2860], [0x4912_2864, 0x4912_2865, 0x4912_2866]],
     [[0x4912_2861, 0x4912_2862, 0x4912_2863], [0x4912_2867, 0x4912_2868, 0x4912_2869]]],
    [[[0x4912_2876, 0x4912_2877, 0x4912_2878], [0x4912_287C, 0x4912_287D, 0x4912_287E]],
     [[0x4912_2879, 0x4912_287A, 0x4912_287B], [0x4912_287F, 0x4912_2880, 0x4912_2881]]],
    [[[0x4912_2882, 0x4912_2883, 0x4912_2884], [0x4912_2882, 0x4912_288B, 0x4912_288C]],
     [[0x4912_2885, 0x4912_2886, 0x4912_2887], [0x4912_288D, 0x4912_288E, 0x4912_288F]]],
];

pub const DUMPED_ACTIONS: &[u32] = &[
    TO_HANG_SIDE[0][0], TO_HANG_SIDE[0][1], TO_HANG_SIDE[1][0], TO_HANG_SIDE[1][1],
    SIDE_JUMP[0][0][0][0], SIDE_JUMP[0][0][0][1], SIDE_JUMP[0][0][0][2], SIDE_JUMP[0][0][1][0], SIDE_JUMP[0][0][1][1], SIDE_JUMP[0][0][1][2],
    SIDE_JUMP[0][1][0][0], SIDE_JUMP[0][1][0][1], SIDE_JUMP[0][1][0][2], SIDE_JUMP[0][1][1][0], SIDE_JUMP[0][1][1][1], SIDE_JUMP[0][1][1][2],
    SIDE_JUMP[1][0][0][0], SIDE_JUMP[1][0][0][1], SIDE_JUMP[1][0][0][2], SIDE_JUMP[1][0][1][0], SIDE_JUMP[1][0][1][1], SIDE_JUMP[1][0][1][2],
    SIDE_JUMP[1][1][0][0], SIDE_JUMP[1][1][0][1], SIDE_JUMP[1][1][0][2], SIDE_JUMP[1][1][1][0], SIDE_JUMP[1][1][1][1], SIDE_JUMP[1][1][1][2],
    SIDE_JUMP[2][0][0][0], SIDE_JUMP[2][0][0][1], SIDE_JUMP[2][0][0][2], SIDE_JUMP[2][0][1][1], SIDE_JUMP[2][0][1][2],
    SIDE_JUMP[2][1][0][0], SIDE_JUMP[2][1][0][1], SIDE_JUMP[2][1][0][2], SIDE_JUMP[2][1][1][0], SIDE_JUMP[2][1][1][1], SIDE_JUMP[2][1][1][2],
    WAIT[0][0], WAIT[0][1], WAIT[1][0], WAIT[1][1], CLIMB_UP[0], CLIMB_UP[1], CLIMB_DOWN[0], CLIMB_DOWN[1],
    ENTER_GROUND[0][0], ENTER_GROUND[0][1], ENTER_GROUND[1][0], ENTER_GROUND[1][1],
    ENTER_TOP[0][0], ENTER_TOP[0][1], ENTER_TOP[1][0], ENTER_TOP[1][1], ENTER_TOP_TO_WAIT[0], ENTER_TOP_TO_WAIT[1],
    EXIT_GROUND[0][0], EXIT_GROUND[0][1], EXIT_GROUND[1][0], EXIT_GROUND[1][1],
    EXIT_TOP[0][0], EXIT_TOP[0][1], EXIT_TOP[1][0], EXIT_TOP[1][1],
    RELEASE[0][0], RELEASE[0][1], RELEASE[1][0], RELEASE[1][1], JUMP[0], JUMP[1], TURN[0], TURN[1],
    FROM_LEDGE_SIDE[0][0], FROM_LEDGE_SIDE[0][1], FROM_LEDGE_SIDE[1][0], FROM_LEDGE_SIDE[1][1],
    FROM_CLIMB_SIDE[0], FROM_CLIMB_SIDE[1], CATCH_AIR, ARRIVE_TARGET,
];

use bevy::prelude::*;

use super::air::InAirEntry;
use super::jump_blend::{self, ActionBlend};
use super::{switch_context, ActorContextId, Body, HumanDataBundle, Locomotion, Player, TransitionSetup};
use crate::collision::CollisionWorld;
use crate::guidance::{GuidanceSubType, GuidanceWorld};
use crate::input::PadInput;

/// Root offset out from the ladder line (0xE266D0: attach point − 0.5 along the ladder direction, hypothesis on
/// its sign: out of the wall).
pub const ATTACH_OUT: f32 = 0.5;
/// `sub_B239D0`: within 1.5 m of the top → the entry from the top.
pub const TOP_ENTRY_BAND: f32 = 1.5;
/// 0xE266D0: the entry from the top ends 0.7 m below the top.
pub const TOP_ENTRY_DROP: f32 = 0.7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LadderPhase {
    EnterGround,
    /// An entry action in place (`LadderEntry::action`), then the wait.
    Entry,
    /// The pull-down from the top (0), then its transition into the wait (items a, b).
    EnterTop(usize),
    Wait,
    ClimbUp,
    ClimbDown,
    /// Exit to the top: item 0 (and for high profile item 1).
    ExitTop(usize),
    ExitGround,
    Jump,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LadderEntry {
    pub base: Vec3,
    pub top: Vec3,
    /// Front normal (out of the wall, the side it is climbed from).
    pub n: Vec3,
    pub from: Vec3,
    pub facing: Vec3,
    pub from_top: bool,
    pub high: bool,
    pub foot: usize,
    /// From a hang at the side (0xDD4CD0) or a climb's reach (0xDEF3A0): the Ledge or Climb context already moved the
    /// root onto the ladder (`from`).
    pub from_ledge: bool,
    /// The action that brings the limbs onto the rungs before the wait: the climb reach's end (fill 0xDEA680:
    /// `xx_h_climb_{l_lhand,r_rhand}_2_tr_ladder_wait_{r,l}`, ladder state 4), the side move from the climb (0xDF10C0,
    /// state 4), the air catch / target arrival (state 2), the wall run's climb up (state 3).
    pub action: Option<u32>,
    /// The action's item (the wall run's entry plays the high climb up's right-foot item).
    pub item: usize,
    /// Root height on the ladder where `action` ends (default: `from`'s).
    pub height: Option<f32>,
    /// The action's items play in order (a, then b): the air catch and the target arrival.
    pub chain: bool,
    /// The first item's clip weights (the target arrival takes the flight's: up / down / long).
    pub w: [f32; 3],
    /// 0xE04100: approach-side root normal; the wait subsequently aligns to `n`.
    pub catch_out: Option<Vec3>,
    /// Controller speed for the catch blend (0xE0BB70).
    pub catch_speed: f32,
}

#[derive(Debug, Default)]
pub struct HumanLadderData {
    pub base: Vec3,
    pub top: Vec3,
    pub n: Vec3,
    /// Root height above the base (HeightInLadder +0x50).
    pub height: f32,
    pub phase: Option<LadderPhase>,
    pub action: Option<ActionBlend>,
    pub t: f32,
    pub foot: usize,
    pub high: bool,
    pub seq: u32,
    from: Vec3,
    to: Vec3,
    /// The root follows the action's displacement plus a linear correction to `to`.
    follow: bool,
    fwd: Vec3,
    pub heading: Vec3,
    /// The jump's push-off direction (horizontal), set when the jump starts.
    pub jump_dir: Vec3,
    /// Playback rate of the running action: 0.5 for a low-profile climb with the stick in the walk band (0xE24540).
    pub rate: f32,
    /// The entry action's items play in order (`LadderEntry::chain`).
    chain: bool,
    pub blend_time: f32,
    /// Independent RootInterp timer (0xE23F70), preserved across action items.
    warp: Option<(Vec3, Vec3, f32, f32, f32, f32)>,
}

fn blend(id: u32, item: usize, w: &[f32]) -> Option<ActionBlend> {
    let n = jump_blend::action_items(id)?.get(item)?.len();
    let mut v = vec![0.0; n];
    v[..w.len().min(n)].copy_from_slice(&w[..w.len().min(n)]);
    if w.is_empty() {
        v[0] = 1.0;
    }
    Some(ActionBlend::new(id, item, &v))
}

impl HumanLadderData {
    pub fn len(&self) -> f32 {
        self.top.y - self.base.y
    }
    /// The root on the ladder at `h`.
    pub fn root_at(&self, h: f32) -> Vec3 {
        self.base + self.n * ATTACH_OUT + Vec3::Y * h
    }
    pub fn current(&self) -> Option<(ActionBlend, f32)> {
        let a = self.action?;
        let ph = self.t / a.duration().max(1e-4);
        Some((a, if self.phase == Some(LadderPhase::Wait) { ph.fract() } else { ph.min(1.0) }))
    }

    fn play(&mut self, phase: LadderPhase, a: Option<ActionBlend>, from: Vec3, to: Vec3, follow: bool) {
        self.phase = Some(phase);
        self.action = a;
        self.t = 0.0;
        self.from = from;
        self.to = to;
        self.follow = follow;
        self.rate = 1.0;
        self.seq = self.seq.wrapping_add(1);
    }

    /// 0xE23F70 / 0xE0BB70: interpolation advances independently of the action chain.
    pub fn advance_catch_root(&mut self, body: &mut Body, dt: f32) {
        if let Some((from,to,h0,h1,ref mut t,duration)) = self.warp {
            *t += dt;
            let k=(*t/duration).min(1.0);
            body.feet=from.lerp(to,k);
            let turn=(h1-h0+std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)-std::f32::consts::PI;
            body.heading=h0+turn*k;
            if k>=1.0 && self.phase==Some(LadderPhase::Wait) { self.warp=None; }
        }
    }

    fn wait(&mut self) {
        let h = self.height;
        let p = self.root_at(h);
        self.play(LadderPhase::Wait, blend(WAIT[self.high as usize][self.foot], 0, &[]), p, p, false);
    }

    /// `HumanLadder__StateEntry_Enter` 0xE266D0.
    pub fn enter(&mut self, e: LadderEntry) {
        self.warp = None;
        self.blend_time = 0.1;
        self.base = e.base;
        self.top = e.top;
        self.n = Vec3::new(e.n.x, 0.0, e.n.z).normalize_or_zero();
        self.foot = e.foot;
        self.high = e.high;
        self.fwd = -self.n;
        self.heading = -self.n;
        let hi = e.high as usize;
        if e.from_ledge {
            // the side move's clip ends in the ladder wait (`…_tr_l_ladder_wait_{l,r}`): wait where it left the root
            self.height = e.height.unwrap_or(e.from.y - e.base.y).clamp(0.0, self.len());
            self.chain = e.chain;
            let w: &[f32] = if e.w.iter().any(|x| *x > 0.0) { &e.w } else { &[] };
            match e.action.and_then(|id| blend(id, e.item, w)) {
                Some(a) => {
                    let p = self.root_at(self.height);
                    self.play(LadderPhase::Entry, Some(a), e.from, p, false);
                    if crate::tuning::AIR_CATCHES && matches!(a.id, CATCH_AIR | ARRIVE_TARGET) {
                        let out = e.catch_out.unwrap_or(self.n);
                        let to = self.base + Vec3::Y * self.height + out * ATTACH_OUT;
                        // 0x724DB0: the shipped catch item's blend kind is NONE, so only this item's
                        // duration is returned (DataPC ActionBlock HumanInAir, 0x156D623E).
                        let duration = if a.id == CATCH_AIR { a.duration() + 0.2 } else { 0.2 };
                        self.blend_time = if a.id == CATCH_AIR { super::air_catches::catch_blend(e.catch_speed) } else { 0.2 };
                        self.warp = Some((e.from, to, super::heading_of(e.facing), super::heading_of(-out), 0.0, duration));
                    }
                }
                None => self.wait(),
            }
        } else if e.from_top {
            // the pull-down: root interpolated to top − 0.7 m on the front over the action (sub_711130)
            self.height = self.len() - TOP_ENTRY_DROP;
            let to = self.root_at(self.height);
            self.heading = Vec3::new(e.facing.x, 0.0, e.facing.z).normalize_or(self.n);
            self.play(LadderPhase::EnterTop(0), blend(ENTER_TOP[hi][e.foot], 0, &[]), e.from, to, false);
        } else {
            // the [straight, left, right] blend by the approach angle / 90° (signed: left when the character is to
            // the ladder's left of its front)
            let to_char = Vec3::new(e.from.x - e.base.x, 0.0, e.from.z - e.base.z).normalize_or(self.n);
            let a = to_char.dot(self.n).clamp(-1.0, 1.0).acos().min(std::f32::consts::FRAC_PI_2);
            let k = a / std::f32::consts::FRAC_PI_2;
            let left = to_char.dot(super::right_of(-self.n)) < 0.0;
            let w = if left { [1.0 - k, k, 0.0] } else { [1.0 - k, 0.0, k] };
            self.height = 0.0;
            let to = self.root_at(0.0);
            self.play(LadderPhase::EnterGround, blend(ENTER_GROUND[hi][e.foot], 0, &w), e.from, to, true);
        }
    }
}

/// `HumanInAir__FindLadderCatch` 0xE04100: the catch height is the hit's height on the ladder rounded down to 0.5 m; it
/// must be at least 0.45 m and below the ladder's height − 1.95 m.
pub fn catch_height(h: f32, len: f32) -> Option<f32> {
    let h = if crate::tuning::AIR_CATCHES { (h * 2.0).trunc() * 0.5 } else { (h * 2.0 + 1e-3).floor() * 0.5 };
    (h >= 0.45 && h < len - 1.95).then_some(h)
}

/// `HumanInAir__FindLadderCatch` 0xE04100 (from `CheckAirCatch` 0xE0BB70, before the ledges): a ladder edge in a box
/// ahead of the root along the reach direction (centre `dir`·0.5 with the grab held, 0.25 without; half extents 0.4
/// across, 0.5 / 0.25 along, 0.3 up; any angle; mask 8). The catch height (`catch_height`), then the facing: the
/// direction from the root to the ladder point, turned to the ladder's front. A box 0.6 m out and 1 m up (half 0.4 /
/// 0.4 / 1.0) must be empty. The catch (action `CATCH_AIR`, blend (1 − min(speed / 10, 1))·0.14 + 0.06 s) interpolates
/// the root to the point + 0.5 m out over the action + 0.2 s; Ladder EntryType 1 (state 2) waits for it, then the low
/// wait (foot l). PORT: ladder owner height/up are adapted from the imported line endpoints.
pub fn find_ladder_catch(feet: Vec3, dir: Vec3, grab: bool, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<LadderEntry> {
    let dir = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }
    let depth = if grab { 0.5 } else { 0.25 };
    let centre = feet + dir * depth;
    let hit = guidance.probe_box(centre, super::right_of(dir), dir, Vec3::Y, Vec3::new(0.4, depth, 0.3), centre, std::f32::consts::PI, 0.1, 1 << GuidanceSubType::Ladder as u32)?;
    let e = &guidance.edges[hit.edge];
    let (base, top) = if e.p0.y <= e.p1.y { (e.p0, e.p1) } else { (e.p1, e.p0) };
    let n = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
    let h = catch_height(hit.point.y - base.y, top.y - base.y)?;
    let point = base + Vec3::Y * h;
    let to = Vec3::new(point.x - feet.x, 0.0, point.z - feet.z).normalize_or(-n);
    let out = if to.dot(n) >= 0.0 { to } else { -to };
    if !collision.obb_free(point + out * 0.6 + Vec3::Y, [super::right_of(-out), -out, Vec3::Y], Vec3::new(0.4, 0.4, 1.0)) {
        return None;
    }
    Some(LadderEntry { base, top, n, from: feet, facing: -out, catch_out: Some(out), from_ledge: true, height: Some(h), action: Some(CATCH_AIR), chain: true, ..Default::default() })
}

/// The wall run's ladder (`GoAssassinActionInterpreter__WallingState` 0xEE05E0 with the Ladder ability `sub_D32600`):
/// the search `sub_EDFBC0`(1) needs the stick (> 0) and takes a ladder whose segment (base … top + 0.5 m) passes within
/// √20 m of the root, its nearest point within 90° of the stick. IHumanWalling slot 7 (`sub_E36420` → `sub_E34160`) accepts
/// it in the Vertical sub-state once its step is done, with `Human__CanGrabLadder` 0xB239D0 (reach 0.6 m, the root on the
/// ladder's front within 90°); slot 8 (`sub_E364F0`) switches to the Ladder (fill `sub_E34D60`: EntryType 3, state 3:
/// the high climb up's right-foot item, then the main state). PORT: the grab test's segment is the ladder's own (the game
/// takes its height from a settings object, 3 m by default).
pub fn find_wall_run_ladder(feet: Vec3, stick: Vec3, guidance: &GuidanceWorld) -> Option<LadderEntry> {
    let stick = Vec3::new(stick.x, 0.0, stick.z).normalize_or_zero();
    if stick == Vec3::ZERO {
        return None;
    }
    for e in &guidance.edges {
        if e.subtype != GuidanceSubType::Ladder {
            continue;
        }
        let (base, top) = if e.p0.y <= e.p1.y { (e.p0, e.p1) } else { (e.p1, e.p0) };
        let n = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
        let q = Vec3::new(base.x, feet.y.clamp(base.y, top.y + 0.5), base.z);
        if feet.distance_squared(q) >= 20.0 {
            continue;
        }
        let to = Vec3::new(q.x - feet.x, 0.0, q.z - feet.z);
        if to.length() > 1e-3 && to.normalize().angle_between(stick) >= std::f32::consts::FRAC_PI_2 {
            continue;
        }
        // Human__CanGrabLadder: within 0.6 m of the segment, on its front
        let q = Vec3::new(base.x, feet.y.clamp(base.y, top.y), base.z);
        let d = Vec3::new(feet.x - q.x, 0.0, feet.z - q.z);
        if feet.distance(q) > 0.6 || (d.length() > 1e-3 && d.normalize().dot(n) < 0.0) {
            continue;
        }
        let height = (feet.y - base.y + 1.0).min(top.y - base.y);
        return Some(LadderEntry { base, top, n, from: feet, facing: -n, high: true, from_ledge: true, action: Some(CLIMB_UP[1]), item: 1, height: Some(height), ..Default::default() });
    }
    None
}

/// `sub_E240B0` (the top of the ladder is free to climb off): two boxes must be empty — top + 0.65 m up and 0.25 m out
/// (half 0.3 across, 0.25 along, 0.65 up), and top + 0.9 m up and 0.35 m in over it (0.3 / 0.35 / 0.4). With either one
/// blocked the climb stops at the top (ReachedTop, `sub_E1D490`, and the input value is zeroed in `sub_E24540`).
/// PORT: the port's ladder lines lie on the wall face and its tops on the wall top, so each box is 1 cm smaller (a box
/// touching the wall counts as blocked).
pub fn top_exit_free(top: Vec3, n: Vec3, collision: &CollisionWorld) -> bool {
    let f = -n;
    let axes = [super::right_of(f), f, Vec3::Y];
    let e = Vec3::splat(0.01);
    collision.obb_free(top + Vec3::Y * 0.65 - f * 0.25, axes, Vec3::new(0.3, 0.25, 0.65) - e)
        && collision.obb_free(top + Vec3::Y * 0.9 + f * 0.35, axes, Vec3::new(0.3, 0.35, 0.4) - e)
}

/// Ground event 38's guard `sub_B239D0` (PORT trigger: see `ground.rs`): a Ladder edge whose line passes within
/// `reach` of the feet, the character on its front side within 90°; `from_top` when the feet are within 1.5 m of
/// the top.
pub fn find_ladder(feet: Vec3, forward: Vec3, reach: f32, guidance: &GuidanceWorld) -> Option<(Vec3, Vec3, Vec3, bool)> {
    for e in &guidance.edges {
        if e.subtype != GuidanceSubType::Ladder {
            continue;
        }
        let (base, top) = if e.p0.y <= e.p1.y { (e.p0, e.p1) } else { (e.p1, e.p0) };
        let n = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
        let from_top = (feet.y - top.y).abs() <= TOP_ENTRY_BAND;
        let q = Vec3::new(base.x, feet.y.clamp(base.y, top.y), base.z);
        let d = Vec3::new(feet.x - q.x, 0.0, feet.z - q.z);
        if d.length() > reach || (feet.y - top.y) > TOP_ENTRY_BAND || feet.y < base.y - 0.3 {
            continue;
        }
        if from_top {
            // standing on top behind the ladder line, facing out over it
            if d.dot(n) > 0.05 || forward.dot(n) < 0.5 {
                continue;
            }
        } else if d.length() > 1e-3 && d.normalize().dot(n) < 0.0 || (-n).dot(forward) < 45f32.to_radians().cos() {
            continue;
        }
        return Some((base, top, n, from_top));
    }
    None
}

/// `HumanLadder__UpdateFSM` 0xE27D30: entry (0xE25240), Main (0xE278E0: climb / wait by MvtAnimState from the
/// table), exits. The player's input is the interpreter's ladder state (0xEEB570, RE/05 §4.2): the stick climbs, the
/// empty-hand button drops (QuickDrop), high profile + Legs jumps off (`wait_tr_rebound`).
/// What the ladder's sideways move (event 4) found.
pub enum LadderSide {
    /// `sub_E1FC80`: a ledge beside the ladder → the Ledge context (SubState 9, fill `sub_E20120`).
    Hang(super::ledge::LedgeEntry),
    /// `sub_E228E0` type 0: climb holds → the Climb context with the reach (the start, the loop, the end).
    Climb(super::climb::ClimbEntry, super::climb::ClimbReach),
    /// `sub_E228E0` types 1 / 2: a hang reached by the side jump → the Ledge context.
    Jump(super::ledge::LedgeEntry),
}

/// The ladder's sideways move (IHumanLadder slots 14 / 15, event 4, `HumanLadder__MainA_HandleEvent` 0xE27610; sent
/// by the interpreter 0xEEB570 with the Climb ability and the stick past 0.35 within 20° of the side, |dot(stick,
/// facing)| < 0.342). Side s = the stick against the ladder's right.
/// 1. `sub_E1FC80`: a hand pair on a ledge around the root + 1.5 m up + 0.8·s + 0.65 m toward the wall
///    (`sub_1171050`); foot support → a wall hang (`sub_B157B0`), else a free hang (`sub_B15AD0`), the hang body box
///    free (`Human__WallHangBodyBoxFree` 0xB2DC80) → `TO_HANG_SIDE` while the root goes to the hang (0xE20120).
/// 2. Else `sub_E228E0`: the climb's sideways reach (`HumanClimb__TrySideReach` 0xDF2EC0's search, `try_climb_reach`)
///    from the root + 0.5 m toward the wall, rows level, up, down; climb holds, a wall hang or a free hang, near or
///    far (+368 = flat distance / 1.6 ≥ 0.5). The actions are the ladder's own table (`SIDE_JUMP`, 0xE1E060): its start
///    plays first (0xE27000), then the loop and the end in the new context (0xE20650 Climb, ClimbData+36 = 2;
///    0xE20900 Ledge). PORT: the start plays in the new context too, as the climb's reach does in the port.
pub fn side_move(right: bool, root: Vec3, n: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<LadderSide> {
    use super::ledge::{hang_root_at, hang_type_at, LedgeEntry, LedgeHangType, LedgeSubState};
    let fwd = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let s = if right { super::right_of(fwd) } else { -super::right_of(fwd) };
    // 1. a ledge beside the ladder
    let p = root + Vec3::Y * 1.5 + s * 0.8 + fwd * 0.65;
    // PORT: the port's guidance has one edge type; the ledge filter of `sub_1171050` is a hold with room above it (a
    // ray 0.3 m above it from 0.5 m out back to the wall runs past the edge, as 0xE165D0's top test)
    let ledge = |mid: Vec3, wn: Vec3| {
        let wn = Vec3::new(wn.x, 0.0, wn.z).normalize_or_zero();
        collision.ray_distance(mid + wn * 0.5 + Vec3::Y * 0.3, -wn, 1.0, crate::layers::MAIN_CHARACTER) > 0.53
    };
    let hold = guidance
        .probe(p, 0.35, 0.3, Some(fwd), 45f32.to_radians())
        .filter(|h| h.wall_normal.dot(n) > 0.7)
        .map(|h| (guidance.fit_hands(h.point, h.wall_normal), h.wall_normal))
        .filter(|&(mid, wn)| ledge(mid, wn));
    if let Some((mid, wn)) = hold {
        let hang = hang_type_at(mid, wn, collision);
        let wall = hang == LedgeHangType::Wall;
        let e = LedgeEntry::at(mid, wn, root, LedgeSubState::TransitionInFromClimb);
        let to = hang_root_at(e.hand_l, e.hand_r, wn, hang, collision);
        let hf = -Vec3::new(wn.x, 0.0, wn.z).normalize_or(fwd);
        if super::climb::hang_boxes_free(collision, to, hf, wall) {
            let id = TO_HANG_SIDE[!wall as usize][right as usize];
            let mv = super::ledge_moves::climb_to_hang_move(root, e.hand_l, e.hand_r, wn, id, hang, collision);
            return Some(LadderSide::Hang(LedgeEntry { entry_move: Some(mv), ..e }));
        }
    }
    // 2. the side jump
    let base = root + fwd * 0.5;
    for diag in 0..3 {
        let Some(target) = super::ledge_moves::try_climb_reach(right, diag, base, base, n, root, guidance, collision) else { continue };
        return Some(match target {
            super::ledge_moves::ClimbReachTarget::Climb(r) => {
                let long = r.ids == super::ledge_moves::CLIMB_REACH_TABLE[0][1][right as usize];
                let ids = SIDE_JUMP[0][long as usize][right as usize];
                let [hl, hr, fl, fr] = r.holds;
                let entry = super::climb::ClimbEntry {
                    entry_type: super::climb::ClimbEntryType::Default,
                    hand_l: hl,
                    hand_r: hr,
                    foot_l: fl,
                    foot_r: fr,
                    normal: r.normals[0],
                    from_feet: root,
                    foot_right: false,
                    action: None,
                };
                LadderSide::Climb(entry, super::climb::ClimbReach::new(ids, r.holds, r.normals, None))
            }
            super::ledge_moves::ClimbReachTarget::Hang(mut mv) => {
                let long = matches!(mv.kind, super::ledge_moves::MoveKind::ClimbReach { long: true });
                let ty = if mv.end_wall { 1 } else { 2 };
                let ids = SIDE_JUMP[ty][long as usize][right as usize];
                for (k, id) in ids.iter().enumerate() {
                    if let Some(a) = mv.seq[k].as_mut() {
                        *a = blend(*id, 0, &[]).unwrap_or(*a);
                    }
                }
                mv.durations = super::ledge_moves::seq_durations(&mv.seq);
                LadderSide::Jump(LedgeEntry {
                    hand_l: mv.hand_l,
                    hand_r: mv.hand_r,
                    normal: mv.normal,
                    from_feet: root,
                    sub_state: LedgeSubState::ParallelJump,
                    entry_move: Some(mv),
                    entry_rest: [None, None],
                    catch: None,
                })
            }
        });
    }
    None
}

pub fn update_ladder(
    time: Res<Time>,
    mut pad: ResMut<PadInput>,
    collision: Res<CollisionWorld>,
    guidance: Res<GuidanceWorld>,
    abilities: Option<Res<super::abilities::AbilitySet>>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    let ab = super::abilities::of(abilities.as_deref());
    for (mut loco, mut body, mut data) in &mut q {
        if loco.current != ActorContextId::Ladder {
            continue;
        }
        if loco.just_switched {
            loco.just_switched = false;
            continue;
        }
        let l = &mut data.ladder;
        l.t += dt * if l.rate > 0.0 { l.rate } else { 1.0 };
        body.velocity = Vec3::ZERO;
        body.grounded = false;
        let Some(phase) = l.phase else { continue };
        let dur = l.action.map(|a| a.duration()).unwrap_or(0.3).max(1e-3);
        let k = (l.t / dur).min(1.0);
        let done = l.t >= dur;
        // root: the action's displacement (in the facing frame) + linear correction to the end, or a plain warp
        let p = if l.follow {
            let d = l.action.map(|a| a.disp(k)).unwrap_or([0.0; 3]);
            let dend = l.action.map(|a| a.disp(1.0)).unwrap_or([0.0; 3]);
            let w = |d: [f32; 3]| super::right_of(l.fwd) * d[0] + l.fwd * d[1] + Vec3::Y * d[2];
            l.from + w(d) + (l.to - (l.from + w(dend))) * k
        } else {
            l.from.lerp(l.to, k)
        };
        if l.warp.is_some() {
            l.advance_catch_root(&mut body, dt);
        } else if crate::tuning::AIR_CATCHES && phase == LadderPhase::Wait {
            // 0xE226D0: constant-speed 2 m/s alignment after approach-side entry.
            body.feet = body.feet.move_towards(p, 2.0 * dt);
            body.heading = super::heading_of(l.heading);
        } else {
            body.feet = p;
            body.heading = super::heading_of(l.heading);
        }

        // The interpreter's ladder state (`GoAssassinActionInterpreter__LadderState` 0xEEB570): above the 0.35 dead
        // zone, a stick within 60° of the facing axis (|dot(d, right)| ≤ 0.866) asks to climb (IHumanLadder slot 0, ±up:
        // up when it points along the facing, `sub_571960` ≥ 0); within 30° of the right axis it is the side step
        // (event 5, slots 8 / 9, not ported).
        let stick = pad.speed01 > 0.0;
        let fwd = -l.n;
        let vertical = stick && pad.dir.dot(super::right_of(fwd)).abs() <= 0.866;
        let along = if vertical { if pad.dir.dot(fwd) >= 0.0 { 1.0 } else { -1.0 } } else { 0.0 };
        let hi = l.high as usize;
        let mut leave: Option<TransitionSetup> = None;
        match phase {
            LadderPhase::EnterGround | LadderPhase::Entry => {
                if done {
                    // the entry action's next item (a → b) at the reached root, then the wait
                    let excess = (l.t-dur).max(0.0);
                    let next = l.action.filter(|_| l.chain && phase == LadderPhase::Entry).and_then(|a| blend(a.id, a.item + 1, if crate::tuning::AIR_CATCHES && a.id==ARRIVE_TARGET { a.weights() } else { &[] }));
                    match next {
                        Some(a) => {
                            let p = l.root_at(l.height);
                            l.play(LadderPhase::Entry, Some(a), p, p, false);
                            if crate::tuning::AIR_CATCHES {
                                l.t = excess;
                                if matches!(a.id,CATCH_AIR|ARRIVE_TARGET) { l.blend_time=0.0; } // DataPC item blend NONE.
                            }
                        }
                        None => l.wait(),
                    }
                }
            }
            LadderPhase::EnterTop(i) => {
                if done {
                    l.heading = -l.n;
                    if i < 2 {
                        if let Some(a) = blend(ENTER_TOP_TO_WAIT[hi], i, &[]) {
                            // a: the drop onto the rungs (its displacement), b: settle
                            let from = l.root_at(l.height);
                            let dz = a.disp(1.0)[2];
                            l.height = (l.height + dz).max(0.0);
                            let to = l.root_at(l.height);
                            l.play(LadderPhase::EnterTop(i + 1), Some(a), from, to, true);
                            continue;
                        }
                    }
                    l.wait();
                }
            }
            LadderPhase::Wait | LadderPhase::ClimbUp | LadderPhase::ClimbDown => {
                // QuickDrop (event 0, slots 6 / 7): the empty-hand button (interp +0x1129), on a ladder within 30° of
                // vertical (`sub_E1EAC0`) → the release (MvtAnimState 15 / 16), its action as the fall's animation
                if pad.hand_just_pressed() && (l.top - l.base).normalize_or(Vec3::Y).y > (30f32).to_radians().cos() {
                    pad.hand_pressed_ago = f32::INFINITY;
                    let fall = blend(RELEASE[l.high as usize][l.foot], 0, &[]);
                    let from = body.feet;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::Fall { from, velocity: Vec3::ZERO, origin: super::air::FallOrigin::Ground, speed_param: 0.0 }));
                    data.air.fall_action = fall;
                    continue;
                }
                // Jump off (event 1, slots 4 / 5): high profile, the Legs buffer (+0x1126) and a push-off direction: the
                // stick, or straight back with no stick; more than 135° from straight back (into the ladder) → none,
                // 90°–135° → turned to ±89°. PORT: the target search (IHuman vt56 / vt76 / vt68) happens when the
                // `wait_tr_rebound` clip ends.
                if pad.high_profile && pad.jump_buffered() {
                    let back = l.n;
                    let d = if stick { Vec3::new(pad.dir.x, 0.0, pad.dir.z).normalize_or(back) } else { back };
                    let a = back.angle_between(d);
                    if a <= 135f32.to_radians() {
                        let d = if a > std::f32::consts::FRAC_PI_2 {
                            let side = if back.cross(d).y >= 0.0 { 1.0 } else { -1.0 };
                            Quat::from_rotation_y(1.553_343_1 * side) * back
                        } else {
                            d
                        };
                        pad.consume_jump();
                        l.jump_dir = d;
                        let a = blend(JUMP[l.foot], 0, &[]);
                        let from = body.feet;
                        l.play(LadderPhase::Jump, a, from, from, true);
                        continue;
                    }
                }
                // the step in progress finishes first
                let stepping = matches!(phase, LadderPhase::ClimbUp | LadderPhase::ClimbDown) && !done;
                // the sideways move (event 4, slots 14 / 15): the stick within 20° of the side (|dot(d, facing)| <
                // 0.342, 0xEEBED9), the Climb ability; posted only while no step runs (PORT: the event queue between
                // the main state's sub-states is not modelled)
                if !stepping && stick && pad.dir.dot(fwd).abs() < 0.342 && ab.allows(super::abilities::Ability::Climb) {
                    let right = pad.dir.dot(super::right_of(fwd)) >= 0.0;
                    if let Some(side) = side_move(right, body.feet, l.n, &guidance, &collision) {
                        match side {
                            LadderSide::Hang(e) | LadderSide::Jump(e) => {
                                switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(e));
                            }
                            LadderSide::Climb(e, r) => {
                                let feet = body.feet;
                                switch_context(&mut loco, &mut data, TransitionSetup::ToClimb(e));
                                // the start plays in place, then the loop to the holds' pose and the end (the climb's reach)
                                let c = &mut data.climb;
                                c.move_action = Some(r.ids[0]);
                                c.move_seq += 1;
                                c.moving = Some(super::RootInterp::new(feet, feet, super::climb::move_time(Some(r.ids[0]))));
                                c.reach = Some(r);
                                c.last_action = "ladder side jump to climb holds";
                            }
                        }
                        continue;
                    }
                }
                if !stepping {
                    // the next step's foot. The game picks the exits, release and jump by the lower foot instead
                    // (`Human__GetLowerFoot` 0xB188F0 in `HumanLadder__PlayExit` 0xE254C0): at every step seam and in the
                    // waits that is this foot (the exit `climb_up_X_tr_…` starts where step X starts)
                    if phase != LadderPhase::Wait {
                        l.foot ^= 1;
                    }
                    l.high = pad.high_profile;
                    let hi = l.high as usize;
                    let len = l.len();
                    // low profile with the stick in the walk band: the climb plays at half rate (input value 0.5 from
                    // `sub_ED7180`, `sub_E24540`). PORT: the interpreter's band table is not read; the ground's walk band.
                    let rate = if !l.high && pad.speed01 <= crate::tuning::BAND_WALK { 0.5 } else { 1.0 };
                    let top_free = top_exit_free(l.top, l.n, &collision);
                    // ReachedTop (`sub_E1D490`): the root within 1.63 m of the top (1.95 m in high profile when the top is
                    // blocked)
                    let reached_top = l.height + if l.high && !top_free { 1.95 } else { 1.63 } >= len;
                    if along > 0.5 && reached_top && !top_free {
                        // the top is blocked: the climb stops (`sub_E240B0`)
                        if phase != LadderPhase::Wait || done {
                            l.wait();
                        }
                    } else if along > 0.5 {
                        let step = if l.high { 1.0 } else { 0.5 };
                        let rise = if l.high { 1.5 } else { 1.0 };
                        if l.height + rise >= len - 0.26 {
                            // exit to the top (13 / 14): ends on the top, 0.5 m in from the edge
                            let from = body.feet;
                            let top_feet = Vec3::new(l.top.x, l.top.y, l.top.z) - l.n * 0.5;
                            let end = if l.high { from + Vec3::Y * 1.0 } else { top_feet };
                            l.play(LadderPhase::ExitTop(0), blend(EXIT_TOP[hi][l.foot], 0, &[]), from, end, true);
                        } else {
                            let from = l.root_at(l.height);
                            l.height = (l.height + step).min(len);
                            let to = l.root_at(l.height);
                            l.play(LadderPhase::ClimbUp, blend(CLIMB_UP[hi], l.foot, &[]), from, to, true);
                            l.rate = rate;
                        }
                    } else if along < -0.5 {
                        let step = if l.high { 1.0 } else { 0.5 };
                        if l.height - step < -0.01 {
                            // exit to the ground (11 / 12)
                            let from = body.feet;
                            let to = l.root_at(0.0);
                            let floor = collision.ground_height(to + Vec3::Y * 0.3, 0.6).unwrap_or(to.y);
                            l.play(LadderPhase::ExitGround, blend(EXIT_GROUND[hi][l.foot], 0, &[]), from, Vec3::new(to.x, floor, to.z), true);
                        } else {
                            let from = l.root_at(l.height);
                            l.height -= step;
                            let to = l.root_at(l.height);
                            l.play(LadderPhase::ClimbDown, blend(CLIMB_DOWN[hi], l.foot, &[]), from, to, true);
                            l.rate = rate;
                        }
                    } else if phase != LadderPhase::Wait || done {
                        l.wait();
                    }
                }
            }
            LadderPhase::ExitTop(i) => {
                if done {
                    if l.high && i == 0 {
                        if let Some(a) = blend(EXIT_TOP[hi][l.foot], 1, &[]) {
                            let from = body.feet;
                            let top_feet = l.top - l.n * 0.5;
                            l.play(LadderPhase::ExitTop(1), Some(a), from, top_feet, true);
                            continue;
                        }
                    }
                    body.grounded = true;
                    leave = Some(TransitionSetup::ToMovement { landing: None });
                }
            }
            LadderPhase::ExitGround => {
                if done {
                    body.grounded = true;
                    leave = Some(TransitionSetup::ToMovement { landing: None });
                }
            }
            LadderPhase::Jump => {
                if done {
                    // a jump target along the push-off (the guidance search of the ground jumps), else a free jump
                    let from = body.feet;
                    let dir = l.jump_dir.normalize_or(l.n);
                    leave = Some(TransitionSetup::ToInAir(match super::targets::find_jump_target(from, dir, &guidance, &collision) {
                        Some(target) => InAirEntry::JumpToTarget { from, target, speed_param: 0.5, foot_left: l.foot == 0 },
                        None => InAirEntry::FreeJump { from, dir, ahead: crate::tuning::FREE_JUMP_AHEAD, speed_param: 0.5, foot_left: l.foot == 0 },
                    }));
                    body.heading = super::heading_of(dir);
                }
            }
        }
        if let Some(setup) = leave {
            switch_context(&mut loco, &mut data, setup);
        }
    }
}

#[cfg(test)]
mod catch_tests {
    use super::*;
    #[test]
    fn rung_truncation_has_no_epsilon_and_strict_top_limit() {
        assert_eq!(catch_height(0.49999,5.0),None);
        assert_eq!(catch_height(0.5,5.0),Some(0.5));
        assert_eq!(catch_height(0.99999,5.0),Some(0.5));
        assert_eq!(catch_height(1.0,5.0),Some(1.0));
        assert_eq!(catch_height(3.0,4.95),None);
        assert_eq!(catch_height(3.0,4.951),Some(3.0));
        assert_eq!(catch_height(-0.2,5.0),None);
    }
    #[test]
    fn air_entry_has_approach_side_and_independent_current_item_warp() {
        let mut l=HumanLadderData::default();
        let from=Vec3::new(0.4,2.2,-0.5);let out=Vec3::new(0.4,0.0,-0.5).normalize();
        l.enter(LadderEntry{base:Vec3::ZERO,top:Vec3::Y*5.0,n:Vec3::NEG_Z,from,facing:Vec3::Z,from_ledge:true,
            action:Some(CATCH_AIR),height:Some(2.0),chain:true,catch_out:Some(out),catch_speed:5.0,..Default::default()});
        let (_,to,_,_,_,duration)=l.warp.unwrap();
        assert!(to.distance(Vec3::Y*2.0+out*0.5)<1e-6);
        assert!((duration-l.action.unwrap().duration()-0.2).abs()<1e-6);
        assert!((l.blend_time-0.13).abs()<1e-6);
        let warp=l.warp;l.play(LadderPhase::Entry,blend(CATCH_AIR,1,&[]),to,to,false);
        assert_eq!(l.warp,warp,"advancing the animation must not restart interpolation");
    }
}
