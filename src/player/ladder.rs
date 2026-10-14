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

/// MvtAnimState 6 "Revolve" [left, right], `xx_h_ladder_turn_{l,r}_a` then `…_b_tr_h_wait_{l,r}`: the swing around
/// the ladder to its other side (`sub_E1D730` from event 5's post 0xE220C0; `HumanLadder__TryTurn` 0xE26440 on a
/// leaning ladder's underside). Its clip steps 0.5 m aside and 1 m forward while turning 180°.
pub const REVOLVE: [u32; 2] = [0x010A_24EE, 0x010A_24EF];

/// The revolve's room (`sub_E24D90`: a box with nothing in it but characters): centre root + 0.85·forward + 1.2 m up
/// ∓ 0.5 m to the side, half extents 0.6 / 0.55 / 0.65 (0xE262B0 / 0xE26440).
pub fn revolve_room(root: Vec3, fwd: Vec3, right: bool, collision: &CollisionWorld) -> bool {
    let f = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero();
    let r = super::right_of(f);
    let c = root + f * 0.85 + Vec3::Y * 1.2 + r * if right { 0.5 } else { -0.5 };
    // half extents (x 0.6, y 0.55, z 0.65) in the character's frame: 0.55 along the forward, 0.65 up (0xE24E4D, RE/24
    // LADDER-16)
    collision.obb_free(c, [r, Vec3::Y, f], Vec3::new(0.6, 0.65, 0.55))
}

pub const DUMPED_ACTIONS: &[u32] = &[
    REVOLVE[0], REVOLVE[1], WALL_RUN_TO_LADDER,
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

/// Root offset out from the ladder line (0xE266D0: attach point − 0.5·F, F = the facing toward the ladder: 0.5 m out on
/// the climber's side; RE/24 LADDER-5 settles the sign).
pub const ATTACH_OUT: f32 = 0.5;
/// The wall run onto a ladder (HumanWalling block, `sub_E202C0`: 0xA3511EAF).
pub const WALL_RUN_TO_LADDER: u32 = 0xA351_1EAF;
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
    /// The revolve (MvtAnimState 6, Main sub-state 7): item a, then b, then the far side (`HumanLadder__EndRevolve`
    /// 0xE21EF0).
    Revolve(usize),
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
    /// The ladder's buried part (`sub_E25B20`, LadderState +788), the entry from the ground's attach rung floor.
    pub buried: f32,
    /// The back-approach turn (`sub_E24FD0`: the attach box blocked on the approach side, RE/24 LADDER-4): `Some(true)`
    /// = `TURN[0]` (signed approach angle > 0), `Some(false)` = `TURN[1]`; `n` stays the approach side (live RE/25 #68).
    pub back_turn: Option<bool>,
}

/// The top entry's side (`StateEntry_Enter` 0xE26D7E–0xE26F23; RE/24 LADDER-23): the target is base + (H − 0.7)·Z
/// − 0.5·F on the side away from the character (`n`, from `find_ladder`), facing the ladder; a static box there (half 0.3
/// across, 0.4 along the facing, 1.0 up, centred on the target root, frame [up × −F, −F, up]: `IsBoxFreeOfStatic`
/// 0xE26EDF) blocked → the facing flips back and the target moves 1 m toward the character: the near side, `−n`.
pub fn top_entry_side(base: Vec3, top: Vec3, n: Vec3, collision: &CollisionWorld) -> Vec3 {
    let l = HumanLadderData { base, top, n: Vec3::new(n.x, 0.0, n.z).normalize_or_zero(), ..Default::default() };
    let target = l.root_at(l.len() - TOP_ENTRY_DROP);
    let (f, u) = (-l.out(), l.axis());
    if collision.obb_free(target, [f.cross(u).normalize_or(super::right_of(f)), f, u], Vec3::new(0.3, 0.4, 1.0)) {
        l.n
    } else {
        -l.n
    }
}

/// +908 & 0x20 (`Human__ProbeWallTopThickness` 0xB18FB0 from `SetupFromEntryType` 0xE22400: from base + (H − 0.75)·Z
/// + 1.5·Y back along −Y, 3 m, a wall top's near and far edges with a drop below each; RE/24 LADDER-8). PORT: the roof
/// edge right behind the ladder's top (a LedgeGrab edge facing out, up to 1 m below the top: the native tops stand
/// 0.37–0.73 m above their roofs) with a far edge within 1.5 m behind it.
pub fn wall_top_behind(top: Vec3, n: Vec3, guidance: &GuidanceWorld) -> bool {
    let n = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    guidance
        .edges
        .iter()
        .filter(|e| e.subtype == GuidanceSubType::LedgeGrab && Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero().dot(n) > 0.9)
        .map(|e| e.closest_point(top))
        .filter(|q| (-0.1..=1.0).contains(&(top.y - q.y)) && Vec3::new(q.x - top.x, 0.0, q.z - top.z).length() < 0.6)
        .any(|q| super::passover::far_edge(q, -n, guidance).is_some_and(|(_, t)| t <= 1.5))
}

/// `sub_E24FD0` 0xE24FD0 (from `StateEntry_Enter` 0xE26AFB): a static-geometry box at the attach point + 0.95 m up, half
/// 0.45 across / 0.45 along the facing / 0.35 up in the frame [F × up, F, up], F = the facing toward the ladder; blocked
/// → the back-approach turn (RE/24 LADDER-4).
pub fn attach_blocked(attach: Vec3, f: Vec3, collision: &CollisionWorld) -> bool {
    let f = Vec3::new(f.x, 0.0, f.z).normalize_or(Vec3::NEG_Z);
    !collision.obb_free(attach + Vec3::Y * 0.95, [super::right_of(f), Vec3::Y, f], Vec3::new(0.45, 0.35, 0.45))
}

#[derive(Debug, Default)]
pub struct HumanLadderData {
    pub base: Vec3,
    pub top: Vec3,
    pub n: Vec3,
    /// +908 & 0x20: a wall top behind the ladder's top (`Human__ProbeWallTopThickness` 0xB18FB0 at entry, from
    /// `SetupFromEntryType` 0xE22400; RE/24 LADDER-8). `None` until probed on the first update.
    pub thick_top: Option<bool>,
    /// ReachedTop (+84): updated only while climbing up (MvtAnimState 2 / 4), kept in the wait (`UpdateReachedTopBottom`
    /// 0xE1D490; RE/24 LADDER-9).
    pub reached_top: bool,
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
    /// The back of the character's facing at the press: the takeoff faces it (`sub_52BD40` 0xE1EC34).
    pub jump_back: Vec3,
    /// The jump off's target, searched at the press (`sub_E22050` copies it to +800; RE/24 LADDER-12).
    pub jump_target: Option<Option<super::targets::JumpTarget>>,
    /// Playback rate of the running action: 0.5 for a low-profile climb with the stick in the walk band (0xE24540).
    pub rate: f32,
    /// The entry action's items play in order (`LadderEntry::chain`).
    chain: bool,
    pub blend_time: f32,
    /// Independent RootInterp timer (0xE23F70), preserved across action items.
    warp: Option<(Vec3, Vec3, f32, f32, f32, f32)>,
    /// The running exit's path over its whole action (`HumanLadder__PlayExit` 0xE254C0): its summed item duration, the
    /// time of the items already played, the ground snap (+728) and the body's pitch at the start.
    exit: Option<ExitPath>,
}

#[derive(Clone, Copy, Debug)]
struct ExitPath {
    total: f32,
    before: f32,
    dz: f32,
    tilt: Quat,
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
    /// The ladder's axis (its frame's Z).
    pub fn axis(&self) -> Vec3 {
        (self.top - self.base).normalize_or(Vec3::Y)
    }
    /// LadderHeight: heights are 3D distances along the segment (HeightInLadder, `sub_E211D0`: the root's projection
    /// on base … base + H·Z; RE/24 LADDER-17).
    pub fn len(&self) -> f32 {
        (self.top - self.base).length()
    }
    /// The climb frame's out direction: the ladder's Y signed toward the climber, square to the axis (`sub_E212F0` /
    /// `sub_E21600`); the horizontal `n` on a vertical ladder.
    pub fn out(&self) -> Vec3 {
        let a = self.axis();
        (self.n - a * self.n.dot(a)).normalize_or(self.n)
    }
    /// The hands' rail goals (`sub_E20DC0`, Main's update pass 1; RE/24 LADDER-20): a hand bone above the ladder's top
    /// goes to the closest point of its rail, base … top offset ∓0.25 m along the ladder's X turned to the character's
    /// right (left hand −, right hand +); a hand at or below the top has none. (The right hand is also let go in the
    /// ladder attack, +909 & 2, `HumanLadder__EnterAttack` 0xE1D330: no combat in the port.) PORT: base … top is the
    /// guidance ladder edge, not the entity's origin and length (+76).
    pub fn rail_goals(&self, hands: [Vec3; 2], right: Vec3) -> [Option<Vec3>; 2] {
        let mut x = self.axis().cross(self.out()).normalize_or(right);
        if x.dot(right) < 0.0 {
            x = -x;
        }
        std::array::from_fn(|h| {
            let off = x * if h == 0 { -0.25 } else { 0.25 };
            (hands[h].y > self.top.y).then(|| {
                let (a, b) = (self.base + off, self.top + off);
                let ab = b - a;
                a + ab * ((hands[h] - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0)
            })
        })
    }

    /// The root on the ladder at `h`: the point on the axis − 0.5·F (`sub_E21600`).
    pub fn root_at(&self, h: f32) -> Vec3 {
        self.base + self.out() * ATTACH_OUT + self.axis() * h
    }
    /// HeightInLadder of a root (`sub_E211D0`).
    pub fn height_of(&self, p: Vec3) -> f32 {
        (p - self.base).dot(self.axis())
    }
    /// The body's pitch in the ladder frame (F × Z, F, Z): the climb's RequestMoveTo matrix (`sub_E21600` 0xE25F35–
    /// 0xE25FAB) and the wait's (`AlignRootWhileWaiting` 0xE22704) lean with the ladder (RE/24 LADDER-17).
    pub fn frame_tilt(&self) -> Quat {
        Quat::from_rotation_arc(Vec3::Y, self.axis())
    }
    /// `HumanLadder__TiltFromVertical` 0xE1EAC0: the ladder's angle from up.
    pub fn tilt(&self) -> f32 {
        (self.top - self.base).normalize_or(Vec3::Y).y.clamp(-1.0, 1.0).acos()
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

    /// The wait after an entry takes its column from the entry item's feet after (`sub_E1D7C0` 0xE1D7F6: 2·((word &
    /// 0xC) != 4), LEFTAHEAD → column l; RE/24 LADDER-7): 1 after 0x010A34A7 / 0x010A251E and the top entries, 0 after
    /// 0x010A34A6 / 0x010A2520.
    fn foot_from_entry(&mut self) {
        if let Some(w) = self.action.and_then(|a| super::anim_gate::word(&a)) {
            self.foot = ((w & 0xC) != 4) as usize;
        }
    }

    fn wait(&mut self) {
        let h = self.height;
        let p = self.root_at(h);
        self.play(LadderPhase::Wait, blend(WAIT[self.high as usize][self.foot], 0, &[]), p, p, false);
    }

    /// `HumanLadder__PlayExit` 0xE254C0: the exit by the lower foot, ReachedBottom → the ground exit, else the top exit
    /// (the high one in high profile without a wall top, +908 & 0x20; RE/24 LADDER-8). Its root is the clip's own
    /// motion from the current root (start matrix +592; correction +720 = 0) plus a vertical snap: a ray from the whole
    /// action's end + 0.5 m to 0.7 m below it (0xE2577F–0xE257DD) sets only +728 = hit − end, applied linearly over the
    /// action (`sub_E1F280` → `sub_E1EF50` rates 1.0 / 1.0) until its last item ends; the body turns upright to the
    /// character's flat facing (0xE256D6–0xE256E4) over the same span (RE/24 LADDER-10).
    fn play_exit(&mut self, top: bool, from: Vec3, collision: &CollisionWorld) {
        let (phase, id) = if top {
            (LadderPhase::ExitTop(0), EXIT_TOP[(self.high && !self.thick_top.unwrap_or(false)) as usize][self.foot])
        } else {
            (LadderPhase::ExitGround, EXIT_GROUND[self.high as usize][self.foot])
        };
        let items: Vec<ActionBlend> = (0..).map_while(|i| blend(id, i, &[])).collect();
        let w = self.exit_frame();
        let end = from + items.iter().map(|a| w(a.disp(1.0))).sum::<Vec3>();
        let dz = collision.floor_height_below(end + Vec3::Y * 0.5, 1.2).map_or(0.0, |y| y - end.y);
        let total = items.iter().map(|a| a.duration()).sum::<f32>().max(1e-3);
        self.exit = Some(ExitPath { total, before: 0.0, dz, tilt: self.frame_tilt() });
        self.play_exit_item(phase, items.first().copied(), from);
    }

    /// The exit frame: the root's at the exit's start (the ladder frame).
    fn exit_frame(&self) -> impl Fn([f32; 3]) -> Vec3 + use<> {
        let (f, u) = (-self.out(), self.axis());
        let r = f.cross(u).normalize_or(super::right_of(f));
        move |d: [f32; 3]| r * d[0] + f * d[1] + u * d[2]
    }

    /// One item of the exit: its clip motion plus its share of the snap.
    fn play_exit_item(&mut self, phase: LadderPhase, a: Option<ActionBlend>, from: Vec3) {
        let Some(x) = self.exit else { return };
        let w = self.exit_frame();
        let d = a.map_or(0.0, |a| a.duration());
        let to = from + a.map_or(Vec3::ZERO, |a| w(a.disp(1.0))) + Vec3::Y * (x.dz * d / x.total);
        self.play(phase, a, from, to, true);
    }

    /// `sub_E1D730` / `HumanLadder__EnterRevolve` 0xE219D0: the revolve's first item, its root motion in the facing frame
    /// with a correction onto the far side.
    fn start_revolve(&mut self, right: bool, from: Vec3) {
        let a = blend(REVOLVE[right as usize], 0, &[]);
        let far = self.root_at(self.height) - self.out() * (2.0 * ATTACH_OUT);
        self.fwd = -self.n;
        self.play(LadderPhase::Revolve(0), a, from, far, a.is_some());
    }

    /// `HumanLadder__StateEntry_Enter` 0xE266D0.
    pub fn enter(&mut self, e: LadderEntry) {
        self.warp = None;
        self.exit = None;
        self.thick_top = None;
        self.reached_top = false;
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
            self.height = e.height.unwrap_or_else(|| self.height_of(e.from)).clamp(0.0, self.len());
            self.chain = e.chain;
            let w: &[f32] = if e.w.iter().any(|x| *x > 0.0) { &e.w } else { &[] };
            match e.action.and_then(|id| blend(id, e.item, w)) {
                Some(a) => {
                    let p = self.root_at(self.height);
                    self.play(LadderPhase::Entry, Some(a), e.from, p, false);
                    if matches!(a.id, CATCH_AIR | ARRIVE_TARGET) {
                        let out = e.catch_out.unwrap_or(self.n);
                        let to = self.base + self.axis() * self.height + out * ATTACH_OUT;
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
            // the attach rung (`sub_E1F8E0` 0xE1F95D–0xE1F984): the nearest 0.5 m rung to the root's height, below the
            // ladder's height − 1.95 m; high profile 0.5 m higher (0xE26AB5–0xE26AE2, RE/24 LADDER-5). PORT: the buried
            // base (`sub_E25B20`) is not measured
            let h_in = self.height_of(e.from).max(0.0);
            let mut k = ((h_in.min(self.len() - 1.95).max(0.0) + 0.25) * 2.0).trunc();
            // a ladder buried more than 0.1 m starts at the rung 1 + trunc(2 × buried) at least (0xE1F95D: `1 − (int)(−2·b)`;
            // live RE/25: the Bureau ladder, 0.67 m below the ground, attached at 1.0 m)
            if e.buried > 0.1 {
                k = k.max(1.0 + (2.0 * e.buried).trunc());
            }
            self.height = 0.5 * k + if e.high { 0.5 } else { 0.0 };
            let to = self.root_at(self.height);
            if let Some(left) = e.back_turn {
                // the back-approach turn (+908 bit 2: 0x0A64400C / D, items a then b into the wait, single-weight blend;
                // RE/24 LADDER-4), ending on the approach side (live RE/25 #68). PORT: the heading snaps (the game slerps
                // at rate 1.6)
                self.chain = true;
                self.play(LadderPhase::Entry, blend(TURN[(!left) as usize], 0, &[]), e.from, to, false);
                return;
            }
            // the clip plays in the character's frame at the entry (start matrix +592, RE/24 LADDER-6)
            self.fwd = Vec3::new(e.facing.x, 0.0, e.facing.z).normalize_or(-self.n);
            self.play(LadderPhase::EnterGround, blend(ENTER_GROUND[hi][e.foot], 0, &w), e.from, to, true);
        }
    }
}

/// `sub_E25B20` (from the ground entry, 0xE26A75): how deep the ladder's base lies under what stands on it: the highest hit
/// of a ray from half-way up the ladder down to its base (layer 8, not the ladder), its height over the base over cos(tilt);
/// 0 with none above the base or a ladder tilted more than 60°.
pub fn buried_depth(base: Vec3, top: Vec3, collision: &CollisionWorld) -> f32 {
    let axis = top - base;
    let tilt = axis.normalize_or(Vec3::Y).y.clamp(-1.0, 1.0).acos();
    let from = base + axis * 0.5;
    let len = (from - base).length();
    if len < 1e-4 || tilt > 60f32.to_radians() {
        return 0.0;
    }
    match collision.ray_hit(from, (base - from) / len, len, crate::layers::NO_CHARACTER_COLLISION) {
        Some((d, _)) => {
            let hit = from + (base - from) / len * d;
            ((hit.y - base.y) / tilt.cos()).max(0.0)
        }
        None => 0.0,
    }
}

/// `HumanInAir__FindLadderCatch` 0xE04100: the catch height is the hit's height on the ladder rounded down to 0.5 m; it
/// must be at least 0.45 m and below the ladder's height − 1.95 m.
pub fn catch_height(h: f32, len: f32) -> Option<f32> {
    // the probe has already snapped the hit to a rung (GUIDE-1); the game truncates that value with no epsilon
    let h = (h * 2.0).trunc() * 0.5;
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
    // the out direction is the ladder's Y signed toward the root (0xE043AF–0xE04425: −Y when (rung − root)·Y > 0), the
    // climber's side of a two-sided ladder (RE/24 LADDER-14 / LADDER-3)
    let away = Vec3::new(feet.x - point.x, 0.0, feet.z - point.z);
    let out = if away.dot(n) >= 0.0 { n } else { -n };
    if !collision.obb_free(point + out * 0.6 + Vec3::Y, [super::right_of(-out), -out, Vec3::Y], Vec3::new(0.4, 0.4, 1.0)) {
        return None;
    }
    Some(LadderEntry { base, top, n: out, from: feet, facing: -out, catch_out: Some(out), from_ledge: true, height: Some(h), action: Some(CATCH_AIR), chain: true, ..Default::default() })
}

/// The wall run's ladder (`GoAssassinActionInterpreter__WallingState` 0xEE05E0 with the Ladder ability `sub_D32600`):
/// the search `sub_EDFBC0`(1) needs the stick (> 0) and takes a ladder whose segment (base … top + 0.5 m) passes within
/// √20 m of the root, its nearest point within 90° of the stick. IHumanWalling slot 7 (`sub_E36420` → `sub_E34160`) accepts
/// it in the Vertical sub-state once its step is done, with `Human__CanGrabLadder` 0xB239D0 (reach 0.6 m: the root or
/// the head, the character facing the ladder within 90°; RE/24 LADDER-2); slot 8 (`sub_E364F0`) switches to the Ladder (fill `sub_E34D60`: EntryType 3, state 3:
/// the high climb up's right-foot item, then the main state). The grab test's segment is the ladder's own (LadderHeight,
/// component 0x7BBFDBE3; 3 m only without one).
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
        // state 3 (`sub_E202C0` 0xE202C0): `xx_h_wallingfront_step1_footr_tr_h_ladder_up_l` with the root interpolated
        // onto the ladder line (− 0.5 m along the facing) at the root's own height over the action; it ends left foot
        // ahead, then the high climb (RE/24 LADDER-13). PORT: the climb up the game chains after it starts from the wait
        let height = (feet.y - base.y).clamp(0.0, top.y - base.y);
        return Some(LadderEntry { base, top, n, from: feet, facing: -n, high: true, from_ledge: true, action: Some(WALL_RUN_TO_LADDER), item: 0, height: Some(height), foot: 0, ..Default::default() });
    }
    None
}

/// `sub_E240B0` (the top of the ladder is free to climb off): two boxes must be empty — top + 0.65 m up and 0.25 m out
/// (half 0.3 across, 0.25 along, 0.65 up), and top + 0.9 m up and 0.35 m in over it (0.3 / 0.35 / 0.4). With either one
/// blocked the climb stops at the top (ReachedTop, `sub_E1D490`, and the input value is zeroed in `sub_E24540`).
/// PORT: the port's ladder lines lie on the wall face, so each box is 1 cm smaller (a box touching the wall counts as
/// blocked).
pub fn top_exit_free(top: Vec3, n: Vec3, collision: &CollisionWorld) -> bool {
    let f = -n;
    let axes = [super::right_of(f), f, Vec3::Y];
    let e = Vec3::splat(0.01);
    collision.obb_free(top + Vec3::Y * 0.65 - f * 0.25, axes, Vec3::new(0.3, 0.25, 0.65) - e)
        && collision.obb_free(top + Vec3::Y * 0.9 + f * 0.35, axes, Vec3::new(0.3, 0.35, 0.4) - e)
}

/// The interpreter's ground ladder test (`ProcessGroundMovement` 0xEE7143–0xEE723C, RE/24 LADDER-1) and event 38's guard
/// `Human__CanGrabLadder` 0xB239D0 (LADDER-2):
/// - `FindLadderNearby`(1) 0xEDFBC0: the stick moved; the nearest point of base … top + 0.5 m within √20 m, its flat
///   direction from the root within 90° of the stick; the first ladder that passes;
/// - `CanGrabLadder` with reach 0.6 (`dword_16834C4`): the root, or else the head, within reach of base … top (the
///   ladder's own height); the character faces the ladder within 90° (−forward against the closest point → root); the
///   ladder's side is not tested (LADDER-3: the climb frame is signed toward the climber);
/// - from the top when the root is within 1.5 m of the top's height.
/// Returns (base, top, the outward normal on the climb side, from the top). `head` is the Head bone (it leans ahead of
/// the root in the walk: live, RE/25, the walk-in entry came with the head 0.60 m from the ladder line and the root
/// 0.62–0.69 m). PORT: without a pose the head is the feet + 1.7 m.
pub fn find_ladder(feet: Vec3, head: Option<Vec3>, stick: Vec3, forward: Vec3, guidance: &GuidanceWorld) -> Option<(Vec3, Vec3, Vec3, bool)> {
    const REACH: f32 = 0.6;
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z);
    let closest = |a: Vec3, b: Vec3, p: Vec3| {
        let ab = b - a;
        let t = if ab.length_squared() < 1e-8 { 0.0 } else { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) };
        a + ab * t
    };
    let stick = flat(stick).normalize_or_zero();
    if stick == Vec3::ZERO {
        return None;
    }
    for e in &guidance.edges {
        if e.subtype != GuidanceSubType::Ladder {
            continue;
        }
        let (base, top) = if e.p0.y <= e.p1.y { (e.p0, e.p1) } else { (e.p1, e.p0) };
        let up = (top - base).normalize_or(Vec3::Y);
        // FindLadderNearby: base … top + 0.5 m along the ladder
        let near = closest(base, top + up * 0.5, feet);
        if (near - feet).length_squared() >= 20.0 {
            continue;
        }
        let to = flat(near - feet);
        if to.length() > 1e-4 && to.normalize().dot(stick) < 0.0 {
            continue;
        }
        // CanGrabLadder: the root or the head within reach of base … top
        let q = closest(base, top, feet);
        let head = head.unwrap_or(feet + Vec3::Y * 1.7);
        if (feet - q).length() > REACH && (head - closest(base, top, head)).length() > REACH {
            continue;
        }
        let away = flat(feet - q);
        if away.length() > 1e-4 && away.normalize().dot(-forward) < 0.0 {
            continue;
        }
        let from_top = (feet.y - top.y).abs() <= TOP_ENTRY_BAND;
        // the climb side: toward the climber from the foot (LADDER-3), away from him from the top (LADDER-23,
        // 0xE26DDD–0xE26DE7: the side opposite the character's, `top_entry_side` then tests its box)
        let mut n = flat(e.n1).normalize_or_zero();
        if (away.dot(n) < 0.0) != from_top && away.length() > 1e-4 {
            n = -n;
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
    // `sub_1171050` (mask 50, 45°): a box there with half extents 0.4 / 0.5 / 0.5, the nearest edge to each hand
    // reference (∓0.3, 0, 0) in the box frame; both hands must find one. Any LedgeGrab edge counts (the game has no
    // ledge filter here)
    let p = root + Vec3::Y * 1.5 + s * 0.8 + fwd * 0.65;
    let r = fwd.cross(Vec3::Y);
    let hold = guidance
        .probe_facing_pair(p, fwd, Vec3::new(0.4, 0.5, 0.5), [p - r * 0.3, p + r * 0.3], 0.785_398_2, crate::guidance::PROBE_STEP, crate::guidance::MASK_CLIMB_HOLDS)
        .map(|[a, b]| ((a.point + b.point) * 0.5, a.wall_normal));
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
        let Some(target) = super::ledge_moves::try_climb_reach(right, diag, base, base, n, root, true, true, guidance, collision) else { continue };
        return Some(match target {
            super::ledge_moves::ClimbReachTarget::Climb(r) => {
                let long = r.ids == super::ledge_moves::CLIMB_REACH_TABLE[0][1][right as usize];
                let ids = SIDE_JUMP[0][long as usize][right as usize];
                let [hl, hr, fl, fr] = r.holds;
                let entry = super::climb::ClimbEntry {
                    // `HumanLadder__FillClimb_SideJump` 0xE20650 writes EntryType 2 (RE/19 §6.1, RE/24 CLIMB-24)
                    entry_type: super::climb::ClimbEntryType::FromLedgeParallelJump,
                    pose_2m: None, end_action: None,
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
                // PORT: the ladder's side jump keeps the clips' displacement plus a linear correction; its root path
                // (the ladder side of 0xE228E0's jump) is undecoded, the climb reach's (CLIMB-11) is not assumed
                mv.follow_disp = mv.durations.iter().sum::<f32>() > 0.0;
                mv.lead = 0.0;
                mv.root_time = None;
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
    skeleton: Option<Res<crate::bone_pose::PoseSkeleton>>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle, &mut super::LimbTargets, Option<&crate::bone_pose::BonePose>), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    let ab = super::abilities::of(abilities.as_deref());
    for (mut loco, mut body, mut data, mut limbs, pose) in &mut q {
        if loco.current != ActorContextId::Ladder {
            continue;
        }
        if loco.just_switched {
            loco.just_switched = false;
            if loco.switched_this_frame {
                continue;
            }
        }
        let l = &mut data.ladder;
        l.t += dt * if l.rate > 0.0 { l.rate } else { 1.0 };
        body.velocity = Vec3::ZERO;
        body.grounded = false;
        let Some(phase) = l.phase else { continue };
        // the hands' rail goals in Main (the wait, the climbs, the revolve; the entries and exits are other states), from
        // last frame's hand bones at the root where it stands (`bone_pose`); none without the install
        let main = matches!(phase, LadderPhase::Wait | LadderPhase::ClimbUp | LadderPhase::ClimbDown | LadderPhase::Revolve(_));
        let bones = skeleton.as_deref().zip(pose).filter(|_| main).and_then(|(s, p)| {
            use crate::bone_pose::{LEFT_HAND, RIGHT_HAND};
            Some([p.world_pos(s, LEFT_HAND, &body)?, p.world_pos(s, RIGHT_HAND, &body)?])
        });
        limbs.rails = bones.map_or([None; 2], |h| l.rail_goals(h, super::right_of(body.forward())));
        let dur = l.action.map(|a| a.duration()).unwrap_or(0.3).max(1e-3);
        let k = (l.t / dur).min(1.0);
        let done = l.t >= dur;
        // root: the action's displacement (in the facing frame) + linear correction to the end, or a plain warp
        let p = if l.follow {
            let d = l.action.map(|a| a.disp(k)).unwrap_or([0.0; 3]);
            let dend = l.action.map(|a| a.disp(1.0)).unwrap_or([0.0; 3]);
            // the frame: the character's upright start frame for the ground entry (+592, RE/24 LADDER-6) and the revolve,
            // else the ladder frame (F × Z, F, Z) the root holds on the ladder (`sub_E21600`; RE/24 LADDER-17)
            let (f, u) = if matches!(phase, LadderPhase::EnterGround | LadderPhase::Revolve(_)) { (l.fwd, Vec3::Y) } else { (-l.out(), l.axis()) };
            let r = f.cross(u).normalize_or(super::right_of(f));
            let w = |d: [f32; 3]| r * d[0] + f * d[1] + u * d[2];
            // the ground entry drives the correction by min(2s, 1) (`sub_E1EF50`; RE/24 LADDER-6), the others linearly
            let c = if phase == LadderPhase::EnterGround { (2.0 * k).min(1.0) } else { k };
            l.from + w(d) + (l.to - (l.from + w(dend))) * c
        } else {
            l.from.lerp(l.to, k)
        };
        if l.warp.is_some() {
            l.advance_catch_root(&mut body, dt);
        } else if phase == LadderPhase::Wait {
            // `AlignRootWhileWaiting` 0xE226D0 in every wait: the nearest 0.5 m rung (`sub_E1F8E0`, capped at the
            // ladder's height − 1.95 m) at 2 m/s, snapping within one step (RE/24 LADDER-18)
            let rung = ((l.height.min(l.len() - 1.95).max(0.0) + 0.25) * 2.0).trunc() * 0.5;
            l.height = rung;
            let target = l.root_at(rung);
            body.feet = body.feet.move_towards(target, 2.0 * dt);
            body.heading = super::heading_of(l.heading);
        } else {
            body.feet = p;
            body.heading = super::heading_of(l.heading);
            if phase == LadderPhase::EnterGround {
                // the turn onto the ladder by min(2s, 1) from the start facing (`sub_E1EF50`, RE/24 LADDER-6)
                let (h0, h1) = (super::heading_of(l.fwd), super::heading_of(-l.n));
                let d = (h1 - h0 + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                body.heading = h0 + d * (2.0 * k).min(1.0);
            }
            if let (LadderPhase::Revolve(0), Some(a)) = (phase, l.action) {
                // the clip's root yaw turns the body half round
                body.heading = super::heading_of(l.fwd) + a.yaw(k);
            }
        }
        // the body's pitch (RE/24 LADDER-17): the ladder frame on the ladder; the ground entry slerps into it by min(2s, 1)
        // (+656 = the ladder frame, `sub_E1EF50`); the exits slerp out of it to the flat facing over the whole action
        // (0xE256D6); the revolve stays upright (PORT: its root frame on a leaning ladder is not decoded)
        body.tilt = match phase {
            LadderPhase::EnterGround => Quat::IDENTITY.slerp(l.frame_tilt(), (2.0 * k).min(1.0)),
            LadderPhase::ExitTop(_) | LadderPhase::ExitGround => match l.exit {
                Some(x) => x.tilt.slerp(Quat::IDENTITY, ((x.before + l.t) / x.total).min(1.0)),
                None => Quat::IDENTITY,
            },
            LadderPhase::Revolve(_) => Quat::IDENTITY,
            _ => l.frame_tilt(),
        };

        // The interpreter's ladder state (`GoAssassinActionInterpreter__LadderState` 0xEEB570): above the 0.35 dead
        // zone, a stick within 60° of the facing axis (|dot(d, right)| ≤ 0.866) asks to climb (IHumanLadder slot 0, ±up:
        // up when it points along the facing, `sub_571960` ≥ 0); within 30° of the right axis it is the side step
        // (event 5, slots 8 / 9: the revolve below).
        // The stick is the interpreter's latched one in the character's frame (+0x1310, 0xEEB696; RE/24 INPUT-4).
        let stick = pad.speed01 > 0.0;
        let fwd = -l.n;
        let sd = pad.actor_dir(body.forward(), false);
        let vertical = stick && sd.dot(super::right_of(fwd)).abs() <= 0.866;
        let along = if vertical { if sd.dot(fwd) >= 0.0 { 1.0 } else { -1.0 } } else { 0.0 };
        let hi = l.high as usize;
        let mut leave: Option<TransitionSetup> = None;
        // events 0 / 1 reach the climb sub-state (6) and the revolve sub-state (7: `HumanLadder__MainB_HandleEvent`
        // 0xE1F760 / `sub_E23860`, the same release fill and jump; RE/24 LADDER-15)
        if matches!(phase, LadderPhase::Wait | LadderPhase::ClimbUp | LadderPhase::ClimbDown | LadderPhase::Revolve(_)) {
            // QuickDrop (event 0, slots 6 / 7): the empty-hand button (interp +0x1129), on a ladder within 30° of
            // vertical (`sub_E1EAC0`) → the release (MvtAnimState 15 / 16), its action as the fall's animation
            if pad.hand_buffered() && l.tilt() < 30f32.to_radians() {
                pad.consume_hand();
                // always the low-profile release (+908 0x10 → `sub_E1D670`: 158 + incl + 2·lowerFoot, 0xE1D6AF; RE/24
                // LADDER-11)
                let fall = blend(RELEASE[0][l.foot], 0, &[]);
                let from = body.feet;
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::Fall { from, velocity: Vec3::ZERO, origin: super::air::FallOrigin::Ground, speed_param: 0.0 }));
                data.air.fall_action = fall;
                continue;
            }
            // Jump off (event 1, slots 4 / 5): high profile, the Legs buffer (+0x1126) and a push-off direction: the
            // stick, or straight back with no stick (the back of the character's facing); more than 135° from straight
            // back (into the ladder) → none, 90°–135° → turned to ±89°. The target is searched at the press (IHuman vt56
            // + the scorer, else the Leap of Faith vt76, else the free target vt68) and stored for the clip's end
            // (`sub_E22050` / `sub_E220F0`; RE/24 LADDER-12).
            if pad.high_profile && pad.jump_buffered() {
                let back = -body.forward();
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
                    l.jump_back = back;
                    l.jump_target = Some(super::targets::find_jump_target(body.feet, d, &guidance, &collision));
                    let a = blend(JUMP[l.foot], 0, &[]);
                    let from = body.feet;
                    l.play(LadderPhase::Jump, a, from, from, true);
                    continue;
                }
            }
        }
        match phase {
            LadderPhase::EnterGround | LadderPhase::Entry => {
                if done {
                    // the entry action's next item (a → b) at the reached root, then the wait
                    let excess = (l.t-dur).max(0.0);
                    let next = l.action.filter(|_| l.chain && phase == LadderPhase::Entry).and_then(|a| blend(a.id, a.item + 1, if a.id==ARRIVE_TARGET { a.weights() } else { &[] }));
                    match next {
                        Some(a) => {
                            let p = l.root_at(l.height);
                            l.play(LadderPhase::Entry, Some(a), p, p, false);
                            l.t = excess;
                            if matches!(a.id,CATCH_AIR|ARRIVE_TARGET) { l.blend_time=0.0; } // DataPC item blend NONE.
                        }
                        None => {
                            if phase == LadderPhase::EnterGround {
                                l.foot_from_entry();
                                // the climb frame again (the entry played in the character's start frame)
                                l.fwd = -l.n;
                            }
                            l.wait()
                        }
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
                    l.foot_from_entry();
                    l.wait();
                }
            }
            LadderPhase::Wait | LadderPhase::ClimbUp | LadderPhase::ClimbDown => {
                // `HumanLadder__TryTurn` 0xE26440 (every frame of the climb state, mid-step too: `StateClimb_Update`
                // 0xE27540; RE/24 LADDER-15): on a ladder leaning 10° or more over the climber (its up axis against the
                // facing), the revolve to the top side when there is room to the left (else the right), otherwise flag
                // +908 0x40 → the release (0xE21E80)
                let up = (l.top - l.base).normalize_or(Vec3::Y);
                if l.tilt() >= 10f32.to_radians() && up.dot(fwd) < 0.0 {
                    let side = [false, true].into_iter().find(|&r| revolve_room(body.feet, fwd, r, &collision));
                    match side {
                        Some(right) => {
                            l.start_revolve(right, body.feet);
                            continue;
                        }
                        None => {
                            let fall = blend(RELEASE[l.high as usize][l.foot], 0, &[]);
                            let from = body.feet;
                            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::Fall { from, velocity: Vec3::ZERO, origin: super::air::FallOrigin::Ground, speed_param: 0.0 }));
                            data.air.fall_action = fall;
                            continue;
                        }
                    }
                }
                // the revolve (event 5, slots 8 / 9, guard 0xE262B0): the stick within 30° of the side (|dot(d, right)|
                // > 0.866), in the wait (MvtAnimState ≤ 1), a ladder within 10° of vertical and room beside it
                if phase == LadderPhase::Wait && stick && sd.dot(super::right_of(fwd)).abs() > 0.866 && l.tilt() < 10f32.to_radians() {
                    let right = sd.dot(super::right_of(fwd)) > 0.0;
                    if revolve_room(body.feet, fwd, right, &collision) {
                        l.start_revolve(right, body.feet);
                        continue;
                    }
                }
                let stepping = matches!(phase, LadderPhase::ClimbUp | LadderPhase::ClimbDown) && !done;
                let thick_top = *l.thick_top.get_or_insert_with(|| wall_top_behind(l.top, l.n, &guidance));
                // ReachedTop / ReachedBottom (`UpdateReachedTopBottom` 0xE1D490) on the continuous height while climbing,
                // and the exits posted every frame (LadderState 0xEEBF87, events 2 / 3): mid-step too (RE/24 LADDER-9).
                // The top margin: 1.63 m in low profile or with a wall top (0x20), else 1.95 m (LADDER-8)
                let margin = if !l.high || thick_top { 1.63 } else { 1.95 };
                let len = l.len();
                let h_now = l.height_of(body.feet);
                if stepping && phase == LadderPhase::ClimbUp && h_now + margin >= len && top_exit_free(l.top, l.n, &collision) {
                    l.play_exit(true, body.feet, &collision);
                    continue;
                }
                if stepping && phase == LadderPhase::ClimbDown && (h_now <= 0.0 || collision.ground_height(body.feet + Vec3::Y * 0.05, 0.1).is_some()) {
                    l.play_exit(false, body.feet, &collision);
                    continue;
                }
                // the low profile reverses mid-step: `HumanLadder__ChooseClimbAnim` switches up ↔ down at once (MvtAnimState
                // 3 → 2 at 0xE249B9, 2 → 3 at 0xE24A88); the high profile only on the item's flag (+42 & 4) or its end
                // (RE/24 LADDER-15). The other clip back to the rung the step left, on the other foot. PORT: the item and
                // its start point come from the action graph's transitions (Anim__PlayAction(id, 0, 0, −1)); the high
                // profile's flag is not read, its steps finish
                if stepping && !l.high && ((phase == LadderPhase::ClimbUp && along < -0.5) || (phase == LadderPhase::ClimbDown && along > 0.5)) {
                    let up = along > 0.5;
                    let rate = if pad.band <= 1 { 0.5 } else { 1.0 };
                    l.foot ^= 1;
                    l.height = (l.height + if up { 0.5 } else { -0.5 }).clamp(0.0, len);
                    if !up {
                        l.reached_top = false;
                    }
                    let (from, to) = (body.feet, l.root_at(l.height));
                    let id = if up { CLIMB_UP[0] } else { CLIMB_DOWN[0] };
                    l.play(if up { LadderPhase::ClimbUp } else { LadderPhase::ClimbDown }, blend(id, l.foot, &[]), from, to, true);
                    l.rate = rate;
                    continue;
                }
                // the sideways move (event 4, slots 14 / 15): the stick within 20° of the side (|dot(d, facing)| <
                // 0.342, 0xEEBED9), the Climb ability; the climb sub-state takes it at any time, mid-step too (`sub_E1FC80`
                // has no MvtAnimState test; RE/24 LADDER-15)
                if stick && sd.dot(fwd).abs() < 0.342 && ab.allows(super::abilities::Ability::Climb) {
                    let right = sd.dot(super::right_of(fwd)) >= 0.0;
                    // `HumanLadder__FindSideJump` 0xE228E0 carves the ladder zones out of its query (`sub_1170880` at 0xE22C81;
                    // no steep removal; RE/24 GUIDE-2)
                    if let Some(side) = side_move(right, body.feet, l.n, &guidance.ledge_report(body.feet, 4.0, false), &collision) {
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
                    // low profile with the stick in band 1 (0.35–0.9): the climb plays at half rate (`StickSpeedValue`
                    // 0xED7180 sends 0.5, `HumanLadder__ChooseClimbAnim` 0xE248F2 halves the rate at ≤ 0.5; RE/24 INPUT-9)
                    let rate = if !l.high && pad.band <= 1 { 0.5 } else { 1.0 };
                    let top_free = top_exit_free(l.top, l.n, &collision);
                    // ReachedTop at the seam (the same rule as mid-step above, on the margin of LADDER-8)
                    let margin = if !l.high || thick_top { 1.63 } else { 1.95 };
                    // ReachedTop is kept from the climb up (the wait aligns below it, LADDER-18)
                    let reached_top = l.reached_top || l.height + margin >= len;
                    if along > 0.5 {
                        l.reached_top = reached_top;
                    } else if along < -0.5 {
                        l.reached_top = false;
                    }
                    if along > 0.5 && reached_top && !top_free {
                        // the top is blocked: the climb stops (`sub_E240B0`)
                        if phase != LadderPhase::Wait || done {
                            l.wait();
                        }
                    } else if along > 0.5 {
                        let step = if l.high { 1.0 } else { 0.5 };
                        if reached_top {
                            // exit to the top (13 / 14)
                            l.play_exit(true, body.feet, &collision);
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
                            l.play_exit(false, body.feet, &collision);
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
            LadderPhase::Revolve(i) => {
                if done {
                    if i == 0 {
                        if let Some(a) = l.action.and_then(|a| blend(a.id, 1, &[])) {
                            let p = body.feet;
                            l.heading = l.n;
                            l.play(LadderPhase::Revolve(1), Some(a), p, p, false);
                            continue;
                        }
                    }
                    // `HumanLadder__EndRevolve` 0xE21EF0 → 0xE21B10: the far side, 0.5 m out from the ladder, facing it
                    l.n = -l.n;
                    l.fwd = -l.n;
                    l.heading = -l.n;
                    l.wait();
                }
            }
            LadderPhase::ExitTop(_) | LadderPhase::ExitGround => {
                if done {
                    // the action's next item (the high top exit's b), until its last item ends (`sub_E1F280`)
                    if let Some(a) = l.action.and_then(|a| blend(a.id, a.item + 1, &[])) {
                        if let Some(x) = l.exit.as_mut() {
                            x.before += dur;
                        }
                        let next = match phase { LadderPhase::ExitTop(i) => LadderPhase::ExitTop(i + 1), p => p };
                        l.play_exit_item(next, Some(a), body.feet);
                        continue;
                    }
                    body.grounded = true;
                    leave = Some(TransitionSetup::ToMovement { landing: None });
                }
            }
            LadderPhase::Jump => {
                if done {
                    // the clip's end (`sub_E1EBB0` 0xE1EC71): facing away from the ladder (−character Y, `sub_52BD40`),
                    // then the stored target with jump kind 2 (`sub_B27180`), presentation 67 (RE/24 LADDER-12)
                    let from = body.feet;
                    let dir = l.jump_dir.normalize_or(l.n);
                    let target = l.jump_target.take().flatten();
                    body.heading = super::heading_of(l.jump_back.normalize_or(l.n));
                    leave = Some(TransitionSetup::ToInAir(InAirEntry::ReboundJump { from, target, dir, foot_left: l.foot == 0 }));
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

    /// RE/24 LADDER-20: a hand bone above the top goes to the closest point of its rail, 0.25 m to its side along the
    /// ladder's X turned to the character's right; a hand at or below the top has no rail goal.
    #[test]
    fn a_hand_above_the_top_goes_to_its_rail() {
        let l = HumanLadderData { base: Vec3::ZERO, top: Vec3::Y * 5.0, n: Vec3::Z, ..Default::default() };
        // facing the ladder (−Z): the right is −X
        let right = super::super::right_of(Vec3::NEG_Z);
        let [lg, rg] = l.rail_goals([Vec3::new(0.4, 5.3, 0.2), Vec3::new(-0.1, 5.2, 0.1)], right);
        let lg = lg.expect("left above the top");
        let rg = rg.expect("right above the top");
        assert!((lg - (Vec3::Y * 5.0 - right * 0.25)).length() < 1e-5, "left rail, clamped to the top: {lg}");
        assert!((rg - (Vec3::Y * 5.0 + right * 0.25)).length() < 1e-5, "right rail: {rg}");
        // the ladder's X turns to the character's side either way
        let [lg2, _] = l.rail_goals([Vec3::new(0.4, 5.3, 0.2), Vec3::ZERO], -right);
        assert!((lg2.unwrap() - (Vec3::Y * 5.0 + right * 0.25)).length() < 1e-5);
        assert_eq!(l.rail_goals([Vec3::new(0.0, 5.0, 0.0), Vec3::new(0.0, 4.0, 0.0)], right), [None, None], "at or below the top");
    }
    /// RE/24 LADDER-23: from the top the climb side is the one away from the character, unless its box is blocked.
    #[test]
    fn the_top_entry_takes_the_far_side_unless_its_box_is_blocked() {
        use crate::collision::Aabb3;
        use crate::guidance::GuidanceEdge;
        let (base, top) = (Vec3::ZERO, Vec3::Y * 5.5);
        let mut g = GuidanceWorld::default();
        g.edges.push(GuidanceEdge { p0: base, p1: top, n0: Vec3::Y, n1: Vec3::NEG_Z, subtype: GuidanceSubType::Ladder });
        // at the top's height on the guidance front (-Z), facing the ladder
        let (_, _, n, from_top) = find_ladder(Vec3::new(0.0, 5.0, -0.5), None, Vec3::Z, Vec3::Z, &g).expect("the ladder");
        assert!(from_top && n.dot(Vec3::Z) > 0.99, "the side away from the character: {n:?}");
        assert_eq!(top_entry_side(base, top, n, &CollisionWorld::default()), n);
        let mut wall = CollisionWorld::default();
        wall.boxes.push(Aabb3 { min: Vec3::new(-1.0, 0.0, 0.05), max: Vec3::new(1.0, 5.0, 2.0) });
        assert_eq!(top_entry_side(base, top, n, &wall), -n, "blocked: the near side");
    }
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
