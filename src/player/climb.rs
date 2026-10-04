//! Climb context (`HumanClimb`, ActorContextID 10) — RE/03 §1–§6.
//!
//! - Every Wait frame a local **hold grid** is built around the feet: columns 0.75 m, rows 0.6 m;
//!   7 or 8 columns/rows depending on the pose (BuildHoldGrid 0xDF6A40). Cells are FOOT cells; the
//!   hand of that side holds the cell two rows higher.
//! - The stick is quantised into 10 directions (QuantizeStickDirection 0xDEB8D0) and a move is looked
//!   up by (pose, direction) in the SHORT table, or first in the LONG table when the stick is pushed
//!   past 0.5 (ChooseMove 0xDFDE90). Tables dumped from StaticInitTables 0xDE4F80 (RE/03 §4.4).
//! - A move is valid when the destination foot cell and the hand cell above it both hold
//!   (IsGridMoveValid 0xDECD70). The root is interpolated over the move's animation length, or 0.5 s
//!   when there is no animation (StartMove 0xDFA0C0) — we have no animations yet, so always 0.5 s.
//! - No move up → transition to a ledge hang (TryTransitionToLedgeHang 0xDF4BA0); Legs → release.

use bevy::prelude::*;

use super::air::{FallOrigin, InAirEntry};
use super::ledge::{LedgeEntry, LedgeSubState};
use super::{heading_of, right_of, switch_context, ActorContextId, Body, HumanDataBundle, LimbTargets, Locomotion, Player, RootInterp, TransitionSetup};
use crate::guidance::GuidanceWorld;
use crate::input::PadInput;
use crate::tuning::*;

/// `HumanClimbData::EntryType` (desc 0x1997864).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClimbEntryType {
    #[default]
    Default = 0,
    FromLedge = 1,
    FromLedgeParallelJump = 2,
    FromGround = 3,
}

#[derive(Clone, Copy, Debug)]
pub struct ClimbEntry {
    pub entry_type: ClimbEntryType,
    pub hand_l: Vec3,
    pub hand_r: Vec3,
    pub foot_l: Vec3,
    pub foot_r: Vec3,
    pub normal: Vec3,
    pub from_feet: Vec3,
    /// The leading foot at the start (`Human__GetLeadingFoot` 0xB18850 == 2): picks the climb start's foot variant.
    pub foot_right: bool,
    /// The transition action played while the root moves into the climb (from a ledge: `LEDGE_TO_CLIMB`).
    pub action: Option<u32>,
}

/// `HumanLedge__TryTransitionToClimb` 0xDD46A0: the hang → climb transition, by direction (table 0x1A2C520 up,
/// 0x1A2C52C down; the stick-down flag picks the down table) and hang type [wall, free] (wall-free counts as free):
/// `xx_h_hang{wall,free}_{u,d}_climb_1m`. The root is interpolated over its length (sub_711130).
pub const LEDGE_TO_CLIMB: [[u32; 2]; 2] = [[0x01C3_2292, 0x01C3_2F9D], [0x01C3_2293, 0x01C3_2F9E]];

/// `HumanLedge__TryFreeHangDropToClimb` 0xDDF390 (free hang, stick down): the drop onto holds below and ahead.
/// [first, second] action: the first plays in place (Ledge state 14), the second (0xDDA120) while the root is
/// interpolated to the new pose (state 15, 0xDE0870). To climb holds: 0x1EB225EF / 0x1EB225F0, then Climb (fill
/// 0xDDFBF0); to a wall-hang ledge: 0x49CE7E53 / 0x49CE7E54, then the hang (0xDDA3D0).
pub const FREE_DROP_TO_CLIMB: [u32; 2] = [0x1EB2_25EF, 0x1EB2_25F0];
pub const FREE_DROP_TO_HANG: [u32; 2] = [0x49CE_7E53, 0x49CE_7E54];
/// The fall started when a hold is lost (`HumanClimb__FillInAirData_LostGrip` 0xDF3300: InAir animation state 33 with
/// action 287367547, blend 0.2).
pub const LOST_GRIP: u32 = 0x1120_E17B;
/// Climb → standing on the ground below (TryDropToLedgeBelow 0xDF1D20: 56562093 + apart), [1m, 2m]:
/// `xx_l_climb_{1m,2m}_tr_h_wait_hipm_footl_a`; Ground then starts with the `_b` actions (0xDE9C60: 56564212 /
/// 56564203).
pub const CLIMB_TO_GROUND: [u32; 2] = [0x035F_11AD, 0x035F_11AE];
pub const CLIMB_TO_GROUND_B: [u32; 2] = [0x035F_19F4, 0x035F_19EB];

/// `HumanLedge__TrySideMoveToClimbHolds` 0xDD48B0: the sideways hang → climb transitions
/// `xx_h_hang{wall,free}_{ul,l,dl,ur,r,dr}_climb_1m`, [left, right] × [up, level, down] × [wall, free]
/// (tables 0x1A2C538 / 0x1A2C544 / 0x1A2C550 left, 0x1A2C55C / 0x1A2C568 / 0x1A2C574 right).
pub const LEDGE_SIDE_TO_CLIMB: [[[u32; 2]; 3]; 2] = [
    [[0x01B7_0938, 0x01C3_2F9F], [0x01B7_0939, 0x01C3_2FA0], [0x01B7_093A, 0x01C3_2FA1]],
    [[0x01B7_093B, 0x01C3_2FA2], [0x01B7_093C, 0x01C3_2FA3], [0x01B7_093D, 0x01C3_2FA4]],
];

/// Pose table (0x1A2CD70): foot cells (col, row) of the left and right side.
pub const POSES: [((i32, i32), (i32, i32)); 6] = [
    ((3, 2), (3, 2)), // 0 same column, level ("1M")
    ((3, 3), (3, 2)), // 1 same column, left higher
    ((3, 2), (3, 3)), // 2 same column, right higher
    ((3, 2), (4, 2)), // 3 one column apart, level ("2M")
    ((3, 3), (4, 2)), // 4 apart, left higher
    ((3, 2), (4, 3)), // 5 apart, right higher
];

/// Which side(s) a move displaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    L,
    R,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveEntry {
    None,
    /// Look the move up again with another direction (table codes 9..14).
    Redirect(usize),
    /// (next pose, side, dx, dy) in cells.
    To(usize, Side, i32, i32),
}

use MoveEntry::{None as N, Redirect as Rd, To};
use Side::{Both, L, R};

/// Directions: 0 Up(L) 1 Up(R) 2 Down(L) 3 Down(R) 4 Left 5 Right 6 UpLeft 7 UpRight 8 DownLeft 9 DownRight.
/// SHORT table (0x1A2D070), RE/03 §4.4.
pub const SHORT: [[MoveEntry; 10]; 6] = [
    // pose 0
    [To(1, L, 0, 1), To(2, R, 0, 1), To(2, L, 0, -1), To(1, R, 0, -1), To(3, L, -1, 0), To(3, R, 1, 0),
     To(4, L, -1, 1), To(5, R, 1, 1), To(5, L, -1, -1), To(4, R, 1, -1)],
    // pose 1 (left higher)
    [To(0, R, 0, 1), To(0, R, 0, 1), To(0, L, 0, -1), To(0, L, 0, -1), To(4, L, -1, 0), To(4, R, 1, 0),
     Rd(4), To(3, R, 1, 1), To(3, L, -1, -1), Rd(5)],
    // pose 2 (right higher)
    [To(0, L, 0, 1), To(0, L, 0, 1), To(0, R, 0, -1), To(0, R, 0, -1), To(5, L, -1, 0), To(5, R, 1, 0),
     To(3, L, -1, 1), Rd(5), Rd(4), To(3, R, 1, -1)],
    // pose 3 (apart, level)
    [To(4, L, 0, 1), To(5, R, 0, 1), To(5, L, 0, -1), To(4, R, 0, -1), To(0, R, -1, 0), To(0, L, 1, 0),
     Rd(0), Rd(1), Rd(2), Rd(3)],
    // pose 4 (apart, left higher)
    [To(3, R, 0, 1), To(3, R, 0, 1), To(3, L, 0, -1), To(3, L, 0, -1), To(1, R, -1, 0), To(1, L, 1, 0),
     To(0, R, -1, 1), Rd(0), Rd(2), To(0, L, 1, -1)],
    // pose 5 (apart, right higher)
    [To(3, L, 0, 1), To(3, L, 0, 1), To(3, R, 0, -1), To(3, R, 0, -1), To(2, R, -1, 0), To(2, L, 1, 0),
     Rd(0), To(0, L, 1, 1), To(0, R, -1, -1), Rd(2)],
];

/// LONG table (0x1A2D7F0): strong push — hand-over-hand 1.2 m, or both sides shuffle.
pub const LONG: [[MoveEntry; 10]; 6] = [
    [N; 10],
    [To(2, R, 0, 2), To(2, R, 0, 2), To(2, L, 0, -2), To(2, L, 0, -2), N, N, N, N, N, N],
    [To(1, L, 0, 2), To(1, L, 0, 2), To(1, R, 0, -2), To(1, R, 0, -2), N, N, N, N, N, N],
    [N, N, N, N, To(3, Both, -1, 0), To(3, Both, 1, 0), N, N, N, N],
    [To(5, R, 0, 2), To(5, R, 0, 2), To(5, L, 0, -2), To(5, L, 0, -2), To(4, Both, -1, 0), To(4, Both, 1, 0), N, N, N, N],
    [To(4, L, 0, 2), To(4, L, 0, 2), To(4, R, 0, -2), To(4, R, 0, -2), To(5, Both, -1, 0), To(5, Both, 1, 0), N, N, N, N],
];

/// Action id per (pose, dir) of the SHORT table (0X1A2D070 + 24*(pose*10+dir) + 20), dumped by
/// emulating HumanClimb__StaticInitTables (RE/data/climb_init.pkl). 0 = none / redirect entry.
pub const SHORT_ACTIONS: [[u32; 10]; 6] = [
    [0x019A05F1, 0x019A05F2, 0x019A05F4, 0x019A05F3, 0x019A05F5, 0x019A05F6, 0x019A05F7, 0x019A05F8, 0x019A05F9, 0x019A05FA],
    [0x019A36CF, 0x019A36CF, 0x019A36D0, 0x019A36D0, 0x019A36D1, 0x019A36D2, 0xFFFFFFFF, 0x019A36D3, 0x019A36D4, 0xFFFFFFFF],
    [0x019A36E7, 0x019A36E7, 0x019A36E8, 0x019A36E8, 0x019A36E9, 0x019A36EA, 0x019A36EB, 0xFFFFFFFF, 0xFFFFFFFF, 0x019A36EC],
    [0x01A267EC, 0x01A267ED, 0x01A267EF, 0x01A267EE, 0x01A267F0, 0x01A267F2, 0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF],
    [0x01A26815, 0x01A26815, 0x01A26817, 0x01A26817, 0x01A26819, 0x01A2681B, 0x01A2681D, 0xFFFFFFFF, 0xFFFFFFFF, 0x01A2681E],
    [0x01A26833, 0x01A26833, 0x01A26835, 0x01A26835, 0x01A26837, 0x01A26839, 0xFFFFFFFF, 0x01A2683B, 0x01A2683C, 0xFFFFFFFF],
];
/// Action id per (pose, dir) of the LONG table (0X1A2D7F0 + 24*(pose*10+dir) + 20), dumped by
/// emulating HumanClimb__StaticInitTables (RE/data/climb_init.pkl). 0 = none / redirect entry.
pub const LONG_ACTIONS: [[u32; 10]; 6] = [
    [0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000],
    [0x019A36D5, 0x019A36D5, 0x019A36D6, 0x019A36D6, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000],
    [0x019A36ED, 0x019A36ED, 0x019A36EE, 0x019A36EE, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000],
    [0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x01A267F1, 0x01A267F3, 0x00000000, 0x00000000, 0x00000000, 0x00000000],
    [0x01A26816, 0x01A26816, 0x01A26818, 0x01A26818, 0x01A2681A, 0x01A2681C, 0x00000000, 0x00000000, 0x00000000, 0x00000000],
    [0x01A26834, 0x01A26834, 0x01A26836, 0x01A26836, 0x01A26838, 0x01A2683A, 0x00000000, 0x00000000, 0x00000000, 0x00000000],
];

/// Pose wait actions (pose table 0x1A2CD70 field animStateId), poses 0..5.
pub const POSE_ACTIONS: [u32; 6] = [0x012D_A39F, 0x012D_A3A0, 0x012D_A3A1, 0x012D_A3A2, 0x012D_A3A3, 0x012D_A3A4];

/// `HumanClimb__OnNoMoveFound` 0xDF4410: with the stick held and no move, the look-around toward it
/// (`xx_l_climb_1m_lookaround_*`): [up (dir 0/1), down (2/3), left (4/6/8), right (5/7/9)].
pub const LOOK_AROUND: [u32; 4] = [0x4A84_8EB4, 0x4A84_8EB5, 0x4759_3210, 0x4A84_8EB6];

/// The look-around action for a quantised direction (0xDF4410).
pub fn look_around(dir: usize) -> u32 {
    match dir {
        0 | 1 => LOOK_AROUND[0],
        4 | 6 | 8 => LOOK_AROUND[2],
        5 | 7 | 9 => LOOK_AROUND[3],
        _ => LOOK_AROUND[1],
    }
}

/// Actions whose clip lengths and root tracks are dumped into `jump_clips.rs` (the sim times grid moves by them).
pub const DUMPED_ACTIONS: &[u32] = &[
    0x019A05F1, 0x019A05F2, 0x019A05F3, 0x019A05F4, 0x019A05F5, 0x019A05F6, 0x019A05F7, 0x019A05F8, 0x019A05F9, 0x019A05FA,
    0x019A36CF, 0x019A36D0, 0x019A36D1, 0x019A36D2, 0x019A36D3, 0x019A36D4, 0x019A36D5, 0x019A36D6,
    0x019A36E7, 0x019A36E8, 0x019A36E9, 0x019A36EA, 0x019A36EB, 0x019A36EC, 0x019A36ED, 0x019A36EE,
    0x01A267EC, 0x01A267ED, 0x01A267EE, 0x01A267EF, 0x01A267F0, 0x01A267F1, 0x01A267F2, 0x01A267F3,
    0x01A26815, 0x01A26816, 0x01A26817, 0x01A26818, 0x01A26819, 0x01A2681A, 0x01A2681B, 0x01A2681C, 0x01A2681D, 0x01A2681E,
    0x01A26833, 0x01A26834, 0x01A26835, 0x01A26836, 0x01A26837, 0x01A26838, 0x01A26839, 0x01A2683A, 0x01A2683B, 0x01A2683C,
    0x4A84_8EB4, 0x4A84_8EB5, 0x4759_3210, 0x4A84_8EB6,
    // climb jump up to an overhang (TryBackEject 0xDF2F50 → StartRelease 0xDE96A0 → 0xDF9650)
    CLIMB_IMPULSE, CLIMB_JUMP_SWINGBACK,
    // free hang → drop onto climb holds / a wall ledge below (TryFreeHangDropToClimb 0xDDF390, 0xDDA120)
    FREE_DROP_TO_CLIMB[0], FREE_DROP_TO_CLIMB[1], FREE_DROP_TO_HANG[0], FREE_DROP_TO_HANG[1],
    // lost grip (HumanClimb__FillInAirData_LostGrip 0xDF3300)
    LOST_GRIP,
    // climb → free hang on holds that stick out (TryTransitionToLedgeHang 0xDF4BA0)
    OVERHANG_TO_HANG[0], OVERHANG_TO_HANG[1],
    // corner moves onto the ground and their Ground follow-ups (flag 4612, StartMove 0xDFA0C0, fill 0xDE9C60)
    CORNER_TO_GROUND[0][0], CORNER_TO_GROUND[0][1], CORNER_TO_GROUND[1][0], CORNER_TO_GROUND[1][1],
    CORNER_TO_GROUND_B[0][0], CORNER_TO_GROUND_B[0][1], CORNER_TO_GROUND_B[1][0], CORNER_TO_GROUND_B[1][1],
    // climb → ground (TryDropToLedgeBelow 0xDF1D20, fill 0xDE9C60)
    CLIMB_TO_GROUND[0], CLIMB_TO_GROUND[1], CLIMB_TO_GROUND_B[0], CLIMB_TO_GROUND_B[1],
    // hang → climb (TryTransitionToClimb 0xDD46A0)
    0x01C3_2292, 0x01C3_2F9D, 0x01C3_2293, 0x01C3_2F9E,
    // hang → climb sideways (TrySideMoveToClimbHolds 0xDD48B0)
    0x01B7_0938, 0x01C3_2F9F, 0x01B7_0939, 0x01C3_2FA0, 0x01B7_093A, 0x01C3_2FA1,
    0x01B7_093B, 0x01C3_2FA2, 0x01B7_093C, 0x01C3_2FA3, 0x01B7_093D, 0x01C3_2FA4,
];

/// `xx_h_climbing_1m_impultion` (StartRelease 0xDE96A0).
pub const CLIMB_IMPULSE: u32 = 0x1E38_0CEB;
/// `xx_h_climbing_1m_impultion_to_swingback_{min,max}_{200,300}cm` (0xDF9650), then the free hang.
pub const CLIMB_JUMP_SWINGBACK: u32 = 0x1EB2_1811;

/// A grid move's length: its action's blended duration (StartMove 0xDFA0C0 → `sub_507650`), 0.5 s with no action.
pub fn move_time(action: Option<u32>) -> f32 {
    action
        .and_then(|id| {
            let items = super::jump_blend::action_items(id)?;
            let d: f32 = (0..items.len())
                .map(|i| {
                    let mut w = vec![0.0; items[i].len().max(1)];
                    w[0] = 1.0;
                    super::jump_blend::ActionBlend::new(id, i, &w).duration()
                })
                .sum();
            (d > 0.0).then_some(d)
        })
        .unwrap_or(MOVE_TIME_NO_ACTION)
}

/// TryBackEject 0xDF2F50: the search point sits 2.25 m above the root, 0.8 m out from the hands (away from the wall).
const CLIMB_JUMP_UP: f32 = 2.25;
const CLIMB_JUMP_OUT: f32 = 0.8;
/// PORT: the probe standing in for the game's box query around that point.
const CLIMB_JUMP_PROBE_R: f32 = 0.5;
const CLIMB_JUMP_PROBE_VTOL: f32 = 0.75;

/// StartMove 0xDFA0C0: a move without an action is interpolated over 0.5 s (blend 0.5).
pub const MOVE_TIME_NO_ACTION: f32 = 0.5;

/// Direction after following redirects (the move's own table entry, which carries its action id).
pub fn resolve_dir(table: &[[MoveEntry; 10]; 6], pose: usize, dir: usize) -> usize {
    let mut d = dir;
    for _ in 0..3 {
        match table[pose][d] {
            MoveEntry::Redirect(nd) => d = nd,
            _ => return d,
        }
    }
    d
}

/// Resolve redirects (codes 9..14 in the game).
pub fn lookup(table: &[[MoveEntry; 10]; 6], pose: usize, dir: usize) -> Option<(usize, Side, i32, i32)> {
    let mut d = dir;
    for _ in 0..3 {
        match table[pose][d] {
            MoveEntry::None => return None,
            MoveEntry::Redirect(nd) => d = nd,
            MoveEntry::To(p, s, dx, dy) => return Some((p, s, dx, dy)),
        }
    }
    None
}

/// 10-way stick quantization (0xDEB8D0). `a` = signed angle from facing; negative = left.
pub fn quantize(stick: Vec3, facing: Vec3) -> usize {
    let a = stick.dot(right_of(facing)).atan2(stick.dot(facing)).to_degrees();
    match a {
        a if (-22.5..0.0).contains(&a) => 0,
        a if (0.0..22.5).contains(&a) => 1,
        a if (-67.5..-22.5).contains(&a) => 6,
        a if (-112.5..-67.5).contains(&a) => 4,
        a if (-157.5..-112.5).contains(&a) => 8,
        a if (22.5..67.5).contains(&a) => 7,
        a if (67.5..112.5).contains(&a) => 5,
        a if (112.5..157.5).contains(&a) => 9,
        a if a < 0.0 => 2,
        _ => 3,
    }
}

/// Runtime data of the Climb context (subset of reflected HumanClimbData, RE/03 §2.2).
#[derive(Debug, Default)]
pub struct HumanClimbData {
    pub entry_type: ClimbEntryType,
    /// HumanClimb+0x38: current pose (0..5).
    pub pose: usize,
    pub foot_l: Vec3,
    pub foot_r: Vec3,
    pub hand_l: Vec3,
    pub hand_r: Vec3,
    /// The root's wall normal (out of the wall, horizontal): −F of the climb pose, flattened.
    pub normal: Vec3,
    /// The holds' own flattened wall normals [L hand, R hand, L foot, R foot] (grid +1360 frames).
    pub hold_n: [Vec3; 4],
    /// The root frame of the current (or target) pose (sub_B1CC20).
    pub frame: ClimbPose,
    /// The root's pitch at the start of the running move (it turns to `pose.tilt()` with the root interpolation).
    pub tilt_from: Quat,
    pub last_action: &'static str,
    pub moving: Option<RootInterp>,
    /// Action (animation graph id) of the running grid move, from the SHORT/LONG table entry.
    pub move_action: Option<u32>,
    /// Incremented per grid move (restarts the move clip).
    pub move_seq: u32,
    /// The look-around playing while the stick finds no move (OnNoMoveFound 0xDF4410).
    pub look: Option<u32>,
    /// +86: the running move ends standing on the ground (TryDropToLedgeBelow 0xDF1D20 → 0xDEA4D0 → 0xDE9C60).
    pub to_ground: bool,
    /// The Ground follow-up of the running move when it is a corner move (flag 4612): `CORNER_TO_GROUND_B`.
    pub ground_follow: Option<u32>,
    /// A corner move turns the root from one heading to another over the move (StartMove 0xDFA0C0: the target
    /// facing is into the corner hold's wall).
    pub turn: Option<(f32, f32)>,
}

impl HumanClimbData {
    pub fn enter(&mut self, e: ClimbEntry) {
        if e.entry_type == ClimbEntryType::FromGround {
            self.enter_from_ground(e);
            return;
        }
        self.set_entry_holds(&e);
        // EnterCommon 0xDE97B0: pose 3 for a "2M" entry (sides one column apart), else pose 0.
        self.pose = if (e.foot_r - e.foot_l).length() > CLIMB_COL * 0.6 { 3 } else { 0 };
        self.last_action = "enter";
        self.look = None;
        self.move_action = e.action;
        self.move_seq += 1;
        self.moving = Some(RootInterp::new(e.from_feet, self.frame.root, move_time(e.action)));
        if e.action.is_none() && e.from_feet.distance(self.frame.root) < 1e-3 {
            // the Ledge context already brought the root onto the pose (free-hang drop, 0xDE0870 → 0xDDFBF0)
            self.moving = None;
        }
    }
}

/// Climb start from standing: `xx_h_wait_hipm_foot{l,r}_tr_climbing_1m` (HumanGround actions 0x01BC382C /
/// 0x01BC382D, FROMAI: the root is interpolated over the clip to the climb root).
pub const CLIMB_FROM_GROUND: [u32; 2] = [0x01BC_382C, 0x01BC_382D];

impl HumanClimbData {
    fn enter_from_ground(&mut self, e: ClimbEntry) {
        self.set_entry_holds(&e);
        self.pose = if (e.foot_r - e.foot_l).length() > CLIMB_COL * 0.6 { 3 } else { 0 };
        self.last_action = "enter";
        // sub_B26080 (0xB261A8): 0x01BC382C + (Human__GetLeadingFoot() != 1), i.e. footr with the right foot ahead
        let id = CLIMB_FROM_GROUND[e.foot_right as usize];
        let dur = super::jump_blend::action_items(id)
            .map(|_| super::jump_blend::ActionBlend::new(id, 0, &[1.0]).duration())
            .filter(|d| *d > 0.0)
            .unwrap_or(CLIMB_MOVE_TIME);
        self.move_action = Some(id);
        self.move_seq += 1;
        self.moving = Some(RootInterp::new(e.from_feet, self.frame.root, dur));
    }

    /// The entry's holds share its wall normal; the root frame comes from them (sub_B1CC20).
    fn set_entry_holds(&mut self, e: &ClimbEntry) {
        self.entry_type = e.entry_type;
        self.foot_l = e.foot_l;
        self.foot_r = e.foot_r;
        self.hand_l = e.hand_l;
        self.hand_r = e.hand_r;
        self.hold_n = [e.normal; 4];
        self.tilt_from = Quat::IDENTITY;
        self.turn = None;
        self.ground_follow = None;
        self.set_frame();
    }

    /// Root frame from the four current holds (ComputeRootFromMove 0xDEC6E0 → sub_B1CC20).
    pub fn set_frame(&mut self) {
        let fallback = -Vec3::new(self.normal.x, 0.0, self.normal.z).normalize_or(Vec3::Z);
        self.frame = climb_pose([self.hand_l, self.hand_r, self.foot_l, self.foot_r], self.hold_n, fallback);
        self.normal = self.frame.normal();
    }
}

/// The climb root frame (`sub_B1CC20`, called by ComputeRootFromMove 0xDEC6E0 and the ledge → climb fills).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClimbPose {
    /// Root (animation origin).
    pub root: Vec3,
    /// F: the facing into the wall. It tilts with the wall when the holds lean out or in.
    pub forward: Vec3,
    /// U: the root's up, perpendicular to F (Y on a vertical wall).
    pub up: Vec3,
}

impl Default for ClimbPose {
    fn default() -> Self {
        Self { root: Vec3::ZERO, forward: Vec3::NEG_Z, up: Vec3::Y }
    }
}

impl ClimbPose {
    /// The root's horizontal facing (yaw), and its wall normal (out of the wall).
    pub fn normal(&self) -> Vec3 {
        let n = -Vec3::new(self.forward.x, 0.0, self.forward.z);
        n.normalize_or(Vec3::Z)
    }
    /// The pitch of the root about its side axis: Y turned onto U.
    pub fn tilt(&self) -> Quat {
        Quat::from_rotation_arc(Vec3::Y, self.up)
    }
}

/// `sub_116D7C0(a, b, n)`: `n` made perpendicular to a → b (n·|d|² − d·(d·n), normalised); `n` itself when a ≈ b.
fn perp_to(a: Vec3, b: Vec3, n: Vec3) -> Vec3 {
    let d = b - a;
    if d.abs().max_element() <= 0.0005 {
        return n;
    }
    d.cross(n).cross(d).normalize_or_zero()
}

/// `sub_B1CC20`: the climb root from the four holds and their (flattened) wall normals, in limb order L hand, R hand,
/// L foot, R foot.
/// - Spreads: the horizontal distances between the feet (sf) and between the hands (sh), each mapped to a weight
///   clamp((s − 0.25)·2, 0, 1).
/// - The side normal: the mean of the two hand normals made perpendicular to hand → foot of their side. Its angle
///   from up is kept; its direction comes from a row normal mix (0.5·w(sf)·hands row + 0.5·w(sh)·feet row, each the
///   first normal of the row made perpendicular to the row) when that mix is not zero.
/// - F = mix of −(mean of the four normals, with the side normal's vertical part) and −(side normal) by
///   w(max(sf, sh)); S = F × up; U = S × F.
/// - The root: the feet midpoint − 0.5·F, moved along U onto the plane of the lower foot − 0.5·F, then sideways
///   (along S) by half the hands' offset from the feet, clamped to [L foot − 0.1, R foot + 0.1].
pub fn climb_pose(holds: [Vec3; 4], normals: [Vec3; 4], fallback_forward: Vec3) -> ClimbPose {
    let [hl, hr, fl, fr] = holds;
    let [nhl, nhr, nfl, nfr] = normals;
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z);
    let w = |s: f32| ((s - 0.25) * 2.0).clamp(0.0, 1.0);
    let (sf, sh) = (flat(fl - fr).length(), flat(hl - hr).length());
    let feet_row = perp_to(fl, fr, nfl);
    let hands_row = perp_to(hl, hr, nhl);
    let mix = hands_row * (0.5 * w(sf)) + feet_row * (0.5 * w(sh));
    let mut side = ((perp_to(hl, fl, nhl) + perp_to(hr, fr, nhr)) * 0.5).normalize_or_zero();
    if mix.abs().max_element() > 0.0005 {
        // up turned toward the mix's direction by the side normal's angle from up (sub_55E390)
        let angle = side.dot(Vec3::Y).clamp(-1.0, 1.0).acos();
        let dir = (mix - Vec3::Y * mix.dot(Vec3::Y)).normalize_or_zero();
        side = (Vec3::Y * angle.cos() + dir * angle.sin()).normalize_or_zero();
    }
    let into = -side;
    let mut mean = -(nhl + nhr + nfl + nfr) * 0.25;
    mean.y = into.y;
    let t = w(sf.max(sh));
    let mut forward = (mean * (1.0 - t) + into * t).normalize_or_zero();
    if forward == Vec3::ZERO {
        forward = fallback_forward;
    }
    let side_axis = forward.cross(Vec3::Y).normalize_or_zero();
    let up = side_axis.cross(forward).normalize_or_zero();
    let base = (fl + fr) * 0.5 - forward * CLIMB_ROOT_OUT;
    let lower = if fr.y <= fl.y { fr } else { fl } - forward * CLIMB_ROOT_OUT;
    let p = base - up * (base - lower).dot(up);
    let x = |q: Vec3| (q - p).dot(side_axis);
    let feet_mid = (x(fl) + x(fr)) * 0.5;
    let off = (((x(hl) + x(hr)) * 0.5 + feet_mid) * 0.5 - feet_mid).max(x(fl) - 0.1).min(x(fr) + 0.1);
    ClimbPose { root: p + side_axis * off, forward, up }
}

/// The climb root for holds that share one wall normal (the entries from a ledge, the jump to climb holds).
pub fn climb_root_at(hand_l: Vec3, hand_r: Vec3, foot_l: Vec3, foot_r: Vec3, n: Vec3) -> Vec3 {
    climb_pose([hand_l, hand_r, foot_l, foot_r], [n; 4], -n).root
}

/// The same with the hands right above the feet.
#[allow(dead_code)]
pub fn climb_root(foot_l: Vec3, foot_r: Vec3, n: Vec3) -> Vec3 {
    climb_root_at(foot_l, foot_r, foot_l, foot_r, n)
}

/// A grid cell's hold (BuildHoldGrid 0xDF6A40: +336 position, +1360 frame from the flattened hit normal, +2384 / +2640
/// guidance object and element, +260 the "centred" flag).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hold {
    pub pos: Vec3,
    /// Flattened wall normal (out of the wall).
    pub normal: Vec3,
    pub edge: usize,
    /// Within 0.3 m sideways and 0.15 m vertically of the cell centre.
    pub centred: bool,
    /// The hold's kind (grid +2640, hit record +68 from GuidanceHit__Set 0xB18600): 0 ledge grab, 1 surface, 2 rope,
    /// 3 pole.
    pub kind: u8,
}

/// The hold grid around the climber (BuildHoldGrid 0xDF6A40, RE/03 §4.2).
pub struct HoldGrid {
    origin: Vec3,
    right: Vec3,
    forward: Vec3,
    x0: f32,
    cols: i32,
    rows: i32,
    holds: Vec<Option<Hold>>,
}

impl HoldGrid {
    /// The frame: axes = the character's flattened facing, its right, and up. The origin is on the line through the
    /// character's position along the facing, at the mean depth of the feet midpoint and the hands midpoint (both
    /// flattened and projected onto that line), at the height of the lower foot. Each cell is probed with a box 0.375 m
    /// sideways, 1.0 m deep both ways along the facing and 0.3 m up and down (sub_11713A0, LedgeGrab / Pole / Rope
    /// edges facing the climber within 45°) for the edge point nearest a reference 0.2 m out of the cell centre. The
    /// depth window lets the grid follow a wall that leans in or out. A hit counts only in its own cell.
    ///
    /// Not modelled: the direction box the grid also queries (local x / z bounds by direction, depth from the limbs'
    /// −1.0 to +0.5 m). The cell probes do not read it; it feeds the runtime edges of capsules and barrels
    /// (sub_B27E80 / sub_B28180) and the corner candidates (sub_DE8380).
    pub fn build(g: &GuidanceWorld, d: &HumanClimbData, root: Vec3, facing: Vec3) -> Self {
        let forward = Vec3::new(facing.x, 0.0, facing.z).normalize_or(-d.normal);
        let right = right_of(forward);
        let (pl, pr) = POSES[d.pose];
        let apart = pl.0 != pr.0;
        let uneven = pl.1 != pr.1;
        let (cols, x0) = if apart { (8, -3.0) } else { (7, -2.625) };
        let rows = if uneven { 8 } else { 7 };
        let depth = |p: Vec3| Vec3::new(p.x - root.x, 0.0, p.z - root.z).dot(forward);
        let mean_depth = (depth((d.foot_l + d.foot_r) * 0.5) + depth((d.hand_l + d.hand_r) * 0.5)) * 0.5;
        let origin = Vec3::new(root.x, d.foot_l.y.min(d.foot_r.y), root.z) + forward * mean_depth;
        let top = -1.5 + CLIMB_ROW * rows as f32;
        let half = Vec3::new(CLIMB_PROBE_R, CLIMB_PROBE_DEPTH, CLIMB_PROBE_VTOL);
        let mut holds = vec![None; (cols * rows) as usize];
        for r in 0..rows {
            for c in 0..cols {
                let cx = x0 + CLIMB_COL * 0.5 + CLIMB_COL * c as f32;
                // rows start at -1.5 m with a +0.3 m centre offset → centre = -1.2 + 0.6·r (row 2 = feet)
                let cz = -1.2 + CLIMB_ROW * r as f32;
                let centre = origin + right * cx + Vec3::Y * cz;
                let reference = centre - forward * CLIMB_PROBE_BACK;
                let Some(h) = g.probe_box(centre, right, forward, Vec3::Y, half, reference, 0.785_398_2, CLIMB_PROBE_STEP, crate::guidance::MASK_CLIMB_HOLDS) else {
                    continue;
                };
                let lx = (h.point - origin).dot(right);
                let lz = h.point.y - origin.y;
                if !(lx > x0 && lx < x0 + CLIMB_COL * cols as f32 && lz > -1.5 && lz < top) {
                    continue;
                }
                let (hc, hr) = (((lx - x0) / CLIMB_COL) as i32, ((lz + 1.5) / CLIMB_ROW) as i32);
                if (hc, hr) != (c, r) {
                    continue;
                }
                let centred = (cx - lx).abs() <= 0.3 && (cz - lz).abs() <= 0.15;
                let kind = match g.edges[h.edge].subtype {
                    crate::guidance::GuidanceSubType::Surface => 1,
                    crate::guidance::GuidanceSubType::Rope => 2,
                    crate::guidance::GuidanceSubType::Pole => 3,
                    _ => 0,
                };
                holds[(r * cols + c) as usize] = Some(Hold { pos: h.point, normal: h.wall_normal, edge: h.edge, centred, kind });
            }
        }
        HoldGrid { origin, right, forward, x0, cols, rows, holds }
    }

    pub fn hold(&self, c: i32, r: i32) -> Option<Hold> {
        if c < 0 || r < 0 || c >= self.cols || r >= self.rows {
            return None;
        }
        self.holds[(r * self.cols + c) as usize]
    }

    /// A point in the grid frame: (sideways, depth along the facing, up).
    #[allow(dead_code)]
    pub fn local(&self, p: Vec3) -> Vec3 {
        let d = p - self.origin;
        Vec3::new(d.dot(self.right), d.dot(self.forward), d.y)
    }

    #[allow(dead_code)]
    pub fn debug_counts(&self) -> (usize, i32, i32, Vec3, Vec3, f32) {
        (self.holds.iter().filter(|h| h.is_some()).count(), self.cols, self.rows, self.origin, self.right, self.x0)
    }
}

/// The climb's corner moves onto the ground (`xx_l_climb_1m_to_groundentry_{left,right}{,_90}`, FROMAI), [left, right] ×
/// [straight, 90] (StartMove 0xDFA0C0: 506998937 / 514997086 left, 506998938 / 514997087 right).
pub const CORNER_TO_GROUND: [[u32; 2]; 2] = [[0x1E38_3099, 0x1EB2_3B5E], [0x1E38_309A, 0x1EB2_3B5F]];
/// Their Ground follow-ups (`HumanClimb__FillGround_FromClimb` 0xDE9C60, FROMANIM): `…_tr_h_wait_foot{l,r}`.
pub const CORNER_TO_GROUND_B: [[u32; 2]; 2] = [[0x1E38_309B, 0x1EB2_3B60], [0x1E38_309C, 0x1EB2_3B61]];

/// The corner moves' side (−1 left, +1 right) and row offset by direction (0xDF4670 / 0xDF28A0): 4 / 5 level, 6 / 7
/// one row up, 8 / 9 one row down.
fn corner_side(dir: usize) -> Option<(i32, i32)> {
    match dir {
        4 => Some((-1, 0)),
        5 => Some((1, 0)),
        6 => Some((-1, 1)),
        7 => Some((1, 1)),
        8 => Some((-1, -1)),
        9 => Some((1, -1)),
        _ => None,
    }
}

/// `HumanClimb__TryCornerToGround_Grid` 0xDF4670: the cell one column to the side of the left foot's column, in row
/// 2 + the row offset (the feet row), holds a plain ledge grab (kind 0). Reached after the SHORT move failed, so the
/// hand cell above it is empty: a top edge level with the feet with nothing to climb above it.
fn corner_from_grid(grid: &HoldGrid, pose: usize, dir: usize) -> Option<Hold> {
    let (side, row) = corner_side(dir)?;
    let (pl, _) = POSES[pose];
    grid.hold(pl.0 + side, 2 + row).filter(|h| h.kind == 0)
}

/// `HumanClimb__ProbeSideColumns` 0xDEB110 (near, as ChooseMove calls it before the near side jump) and
/// `HumanClimb__TryCornerToGround_Candidates` 0xDF28A0. From the feet midpoint at the lower foot, a box probe per row
/// (rows −0.6, 0, +0.6 … m) centred 0.5 m to the side and 0.6 m out from the wall, its depth axis pointing to the
/// side: 0.25 m across, 0.5 m along the side both ways, 0.3 m up and down, edges whose wall faces back against the
/// side within 50°, nearest to a point 0.2 m back toward the climber (sub_11713A0, mask 50). These are the top edges
/// of walls square to the climb face, ahead in the move. The row (1 + row offset) must hold a plain ledge grab.
/// PORT: the game's search filter `dword_193419C` (climb holds) is not modelled.
fn corner_from_candidates(g: &GuidanceWorld, d: &HumanClimbData, facing: Vec3, dir: usize) -> Option<Hold> {
    let (side, row) = corner_side(dir)?;
    let mut base = (d.foot_l + d.foot_r) * 0.5;
    base.y = d.foot_l.y.min(d.foot_r.y);
    let toward = right_of(facing) * side as f32;
    let centre = base + toward * 0.5 - facing * 0.6 + Vec3::Y * (-0.6 + CLIMB_ROW * (1 + row) as f32);
    let half = Vec3::new(0.25, 0.5, CLIMB_PROBE_VTOL);
    let h = g.probe_box(centre, right_of(toward), toward, Vec3::Y, half, centre - toward * CLIMB_PROBE_BACK, 0.872_664_6, CLIMB_PROBE_STEP, crate::guidance::MASK_CLIMB_HOLDS)?;
    let e = &g.edges[h.edge];
    (e.subtype == crate::guidance::GuidanceSubType::LedgeGrab).then_some(Hold { pos: h.point, normal: h.wall_normal, edge: h.edge, centred: false, kind: 0 })
}

/// `HumanClimb__CornerClearance` 0xDF0680, for the corner hold `h` on `side`:
/// - a capsule (r 0.35, 1.8 m) from the root − 0.15·facing is cast to the hold + 0.5·n + 0.1 up + 0.2·side·S
///   (sub_B136E0; n the hold's wall normal, F = −n flattened, S = F × up): the body can slide beside the hold;
/// - a box at the hold + 1.05 up + 0.2·side·S, axes S / F / up, half extents 0.375 / 0.25 / 0.75, holds nothing
///   (sub_B2D2A0): no wall above the hold, from 0.3 to 1.8 m.
fn corner_clearance(collision: &crate::collision::CollisionWorld, root: Vec3, facing: Vec3, h: &Hold, side: i32) -> bool {
    let fc = -Vec3::new(h.normal.x, 0.0, h.normal.z).normalize_or_zero();
    let s = fc.cross(Vec3::Y).normalize_or_zero();
    let shift = s * (0.2 * side as f32);
    let base = root - facing * 0.15;
    let target = h.pos - fc * 0.5 + Vec3::Y * 0.1 + shift;
    collision.capsule_cast_free(base, 1.8, 0.35, target - base)
        && collision.obb_free(h.pos + Vec3::Y * 1.05 + shift, [s, fc, Vec3::Y], Vec3::new(0.375, 0.25, 0.75))
}

fn wrap_angle(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    (a + std::f32::consts::PI).rem_euclid(t) - std::f32::consts::PI
}

/// `HumanClimb__LeanAngleInFacingPlane` 0xDF2CD0: `a` and `b` projected onto the vertical plane through the root along
/// the facing, then the signed angle (about the facing's right) from up to lower → higher. Positive = leaning out
/// (toward the climber's back).
fn lean_angle(a: Vec3, b: Vec3, facing: Vec3) -> f32 {
    let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or_zero();
    let side = right_of(f);
    let (a, b) = (a - side * a.dot(side), b - side * b.dot(side));
    let (lo, hi) = if a.y <= b.y { (a, b) } else { (b, a) };
    let d = hi - lo;
    if d.abs().max_element() <= 0.0005 {
        return 0.0;
    }
    let d = d.normalize();
    let angle = d.dot(Vec3::Y).clamp(-1.0, 1.0).acos();
    if Vec3::Y.cross(d).dot(side) < 0.0 { -angle } else { angle }
}

/// `xx_h_climb_1m_u_overhangfree_{1lu,1ru}` (0x1DBDB3D8 + right hand higher), FROMAI.
pub const OVERHANG_TO_HANG: [u32; 2] = [0x1DBD_B3D8, 0x1DBD_B3D9];

/// `HumanClimb__TryTransitionToLedgeHang` 0xDF4BA0, for a valid SHORT move (`sides` = the moving sides' new foot / hand
/// holds): only up directions (0, 1, 6, 7) from a level pose (record +4 = 0: poses 0 / 3). The four holds after the
/// move give the root frame (sub_B1CC20, F). body = lean(higher foot → lower hand), hands = clamp(lean(hand → hand),
/// 0, 45°); when hands − body < −30° the body would lean out 30° beyond the hands' line (holds that stick out), so the
/// climb becomes a free hang from the two new hand holds (LedgeData +104 = 1, SubState 9; +3504 / +3584 = 0: no
/// feet) with `OVERHANG_TO_HANG[the right hand is higher]` (+4616). Returns the hands, their wall normal and the
/// action.
fn overhang_hang(d: &HumanClimbData, sides: [Option<(Hold, Hold)>; 2], dir: usize) -> Option<(Vec3, Vec3, Vec3, u32)> {
    if !matches!(dir, 0 | 1 | 6 | 7) || !matches!(d.pose, 0 | 3) {
        return None;
    }
    let (mut holds, mut normals) = ([d.hand_l, d.hand_r, d.foot_l, d.foot_r], d.hold_n);
    for (i, s) in sides.iter().enumerate() {
        if let Some((f, h)) = s {
            (holds[i], normals[i]) = (h.pos, h.normal);
            (holds[2 + i], normals[2 + i]) = (f.pos, f.normal);
        }
    }
    let [hl, hr, fl, fr] = holds;
    let pose = climb_pose(holds, normals, -d.normal);
    let higher_foot = if fl.y <= fr.y { fr } else { fl };
    let left_lower = hl.y <= hr.y;
    let lower_hand = if left_lower { hl } else { hr };
    let body = lean_angle(higher_foot, lower_hand, pose.forward);
    let hands = lean_angle(hl, hr, pose.forward).clamp(0.0, std::f32::consts::FRAC_PI_4);
    if hands - body >= -std::f32::consts::FRAC_PI_6 {
        return None;
    }
    let n = (normals[0] + normals[1]).normalize_or(d.normal);
    Some((hl, hr, n, OVERHANG_TO_HANG[left_lower as usize]))
}

/// IsGridMoveValid 0xDECD70: every moving side needs a foot hold and a hand hold two rows up.
fn try_move(grid: &HoldGrid, pose: usize, m: (usize, Side, i32, i32)) -> Option<(usize, [Option<(Hold, Hold)>; 2])> {
    let (next, side, dx, dy) = m;
    let (pl, pr) = POSES[pose];
    let mut out = [None, None];
    for (i, (c, r)) in [pl, pr].into_iter().enumerate() {
        let moves = matches!((side, i), (Side::Both, _) | (Side::L, 0) | (Side::R, 1));
        if !moves {
            continue;
        }
        let (nc, nr) = (c + dx, r + dy);
        let foot = grid.hold(nc, nr)?;
        let hand = grid.hold(nc, nr + CLIMB_HAND_ROWS)?;
        out[i] = Some((foot, hand));
    }
    Some((next, out))
}

pub fn update_climb(
    time: Res<Time>,
    mut pad: ResMut<PadInput>,
    guidance: Res<GuidanceWorld>,
    collision: Res<crate::collision::CollisionWorld>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle, &mut LimbTargets), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (mut loco, mut body, mut data, mut limbs) in &mut q {
        if loco.current != ActorContextId::Climb {
            // the root frame's pitch belongs to the climb
            if body.tilt != Quat::IDENTITY {
                body.tilt = Quat::IDENTITY;
            }
            continue;
        }
        if loco.just_switched {
            loco.just_switched = false;
            continue;
        }
        let d = &mut data.climb;
        let n = d.normal;
        // Update 0xDFE7F0, every state: `sub_E55A10(human+1328)` checks that the objects behind the four limbs' holds
        // still exist (handle flag 0x10000000 and a live pointer, or no hold). A destroyed or unloaded hold drops the
        // character (fill HumanClimb__FillInAirData_LostGrip 0xDF3300). PORT: the hold must still lie on a grab edge.
        let lost = [d.hand_l, d.hand_r, d.foot_l, d.foot_r].iter().zip(d.hold_n).any(|(p, hn)| guidance.on_edge(*p, hn, CLIMB_HOLD_TOL).is_none());
        if lost {
            limbs.hands = None;
            limbs.feet = None;
            d.last_action = "lost grip";
            let entry = InAirEntry::Fall { from: body.feet, velocity: Vec3::ZERO, origin: FallOrigin::Climb, speed_param: 0.0 };
            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
            data.air.fall_action = super::ledge_moves::single_item(LOST_GRIP, 0);
            continue;
        }
        let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
        body.heading = heading_of(facing);
        body.velocity = Vec3::ZERO;
        body.grounded = false;
        limbs.hands = Some((d.hand_l, d.hand_r));
        limbs.feet = Some((d.foot_l, d.foot_r));
        limbs.normal = n;
        limbs.adjust = crate::ik::ADJUST_SLOPE | crate::ik::ADJUST_HEAD;
        // a moving limb lands on its new hold a little before the root settles
        limbs.transit = d.moving.map(|m| m.duration).unwrap_or(CLIMB_MOVE_TIME) * 0.8;

        // Move state: advance the root interpolation (StateMove_Update 0xDF5990); the root's pitch turns with it
        if let Some(m) = d.moving.as_mut() {
            let (p, s) = m.advance(dt);
            body.feet = p;
            body.tilt = d.tilt_from.slerp(d.frame.tilt(), s);
            if let Some((from, to)) = d.turn {
                body.heading = from + wrap_angle(to - from) * s;
            }
            if m.done() {
                d.moving = None;
                if d.to_ground {
                    // Move state end with +86 → sub_DEA4D0 → fill sub_DE9C60: Ground with `climb_{1m,2m}_tr_h_wait_b`
                    // (FROMANIM, left foot ahead)
                    d.to_ground = false;
                    d.turn = None;
                    let b = d.ground_follow.take().unwrap_or(CLIMB_TO_GROUND_B[(d.pose == 3) as usize]);
                    limbs.hands = None;
                    limbs.feet = None;
                    body.grounded = true;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: None });
                    if let Some(a) = super::ledge_moves::single_item(b, 0) {
                        data.ground.play_oneshot(a);
                    }
                }
            }
            continue;
        }

        // Wait state (StateWait_Update 0xDFE760)
        body.tilt = d.frame.tilt();
        if pad.jump_buffered() {
            pad.consume_jump();
            limbs.hands = None;
            limbs.feet = None;
            let back = pad.high_profile && pad.speed01 > 0.0 && matches!(quantize(pad.dir, facing), 2 | 3 | 8 | 9);
            let entry = if back {
                // back eject (TryBackEject 0xDF2F50) — hypothesis: high profile + Legs + stick away
                InAirEntry::FreeJump { from: body.feet, dir: n, speed_param: 0.5 }
            } else {
                // release (StartRelease 0xDE96A0 → InAir, FallOrigin_Climb)
                InAirEntry::Fall { from: body.feet, velocity: Vec3::ZERO, origin: FallOrigin::Climb, speed_param: 0.0 }
            };
            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
            continue;
        }
        if pad.speed01 <= 0.0 {
            // no stick: the pose's wait action (0xDF4410, blend 0.5)
            d.last_action = "hold";
            d.look = None;
            continue;
        }
        let dir = quantize(pad.dir, facing);
        let grid = HoldGrid::build(&guidance, d, body.feet, facing);
        // ChooseMove 0xDFDE90: LONG first when the stick is pushed hard, then SHORT
        let mut chosen = None;
        if pad.magnitude > CLIMB_LONG_STICK {
            chosen = lookup(&LONG, d.pose, dir).and_then(|m| try_move(&grid, d.pose, m).map(|r| (r, m, LONG_ACTIONS[d.pose][resolve_dir(&LONG, d.pose, dir)])));
        }
        if chosen.is_none() {
            chosen = lookup(&SHORT, d.pose, dir).and_then(|m| try_move(&grid, d.pose, m).map(|r| (r, m, SHORT_ACTIONS[d.pose][resolve_dir(&SHORT, d.pose, dir)])));
            // a valid SHORT move up onto holds that stick out: hang from them instead (TryTransitionToLedgeHang 0xDF4BA0,
            // ChooseMove step 4; 4613 + 2920 → the Ledge context, fill sub_DEEAC0)
            if let Some(((_, sides), _, _)) = chosen {
                if let Some((hl, hr, hn, action)) = overhang_hang(d, sides, dir) {
                    let mv = super::ledge_moves::climb_overhang_move(body.feet, hl, hr, hn, action, &collision);
                    let e = LedgeEntry {
                        hand_l: hl,
                        hand_r: hr,
                        normal: hn,
                        from_feet: body.feet,
                        sub_state: LedgeSubState::TransitionInFromClimb,
                        entry_move: Some(mv),
                        entry_rest: [None, None],
                    };
                    d.look = None;
                    d.last_action = "overhang";
                    switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(e));
                    continue;
                }
            }
        }
        if let Some(((next, sides), _, action)) = chosen {
            d.move_action = (action != 0 && action != u32::MAX).then_some(action);
            d.move_seq += 1;
            if let Some((f, h)) = sides[0] {
                (d.foot_l, d.hand_l) = (f.pos, h.pos);
                (d.hold_n[2], d.hold_n[0]) = (f.normal, h.normal);
            }
            if let Some((f, h)) = sides[1] {
                (d.foot_r, d.hand_r) = (f.pos, h.pos);
                (d.hold_n[3], d.hold_n[1]) = (f.normal, h.normal);
            }
            d.pose = next;
            d.look = None;
            // StartMove 0xDFA0C0: the root frame of the new holds (0xDEC6E0), interpolated over the move action's length
            d.tilt_from = d.frame.tilt();
            d.set_frame();
            d.moving = Some(RootInterp::new(body.feet, d.frame.root, move_time(d.move_action)));
            d.last_action = match dir {
                0 | 1 => "climb up",
                2 | 3 => "climb down",
                4 => "climb left",
                5 => "climb right",
                _ => "climb diagonal",
            };
            continue;
        }
        // no grid move sideways: a corner move onto the ground beside the climb (flag 4612; ChooseMove tries the grid route
        // TryCornerToGround_Grid 0xDF4670 first, then, after the near side jump, the side candidates 0xDF28A0)
        if let Some((side, _)) = corner_side(dir) {
            let hold = corner_from_grid(&grid, d.pose, dir)
                .filter(|h| corner_clearance(&collision, body.feet, facing, h, side))
                .or_else(|| corner_from_candidates(&guidance, d, facing, dir).filter(|h| corner_clearance(&collision, body.feet, facing, h, side)));
            if let Some(h) = hold {
                let fc = -Vec3::new(h.normal.x, 0.0, h.normal.z).normalize_or(-n);
                // StartMove 0xDFA0C0 (+4612): the `_90` variant when the new facing turns 45° or more
                let ninety = facing.dot(fc) <= std::f32::consts::FRAC_1_SQRT_2;
                let (i, j) = ((side > 0) as usize, ninety as usize);
                let id = CORNER_TO_GROUND[i][j];
                // the moving side's foot goes onto the corner hold (slot 215 / 220), the other limbs keep theirs
                if side < 0 {
                    (d.foot_l, d.hold_n[2]) = (h.pos, h.normal);
                } else {
                    (d.foot_r, d.hold_n[3]) = (h.pos, h.normal);
                }
                d.move_action = Some(id);
                d.move_seq += 1;
                d.look = None;
                d.tilt_from = d.frame.tilt();
                d.frame = ClimbPose { root: h.pos, forward: fc, up: Vec3::Y };
                d.turn = Some((body.heading, heading_of(fc)));
                d.ground_follow = Some(CORNER_TO_GROUND_B[i][j]);
                // the root is interpolated onto the corner hold, facing into its wall, over the action (sub_711130)
                d.moving = Some(RootInterp::new(body.feet, h.pos, move_time(Some(id))));
                d.to_ground = true;
                d.last_action = "corner to ground";
                continue;
            }
        }
        // no grid move: going up with the hands on a top edge → climb out (TryReachLedgeAbove 0xDF1730). Only from the
        // level poses (the pose record's "uneven" field must be 0: poses 0 / 3); 2m (pose 3) picks the 2m clip.
        // PORT: the game's standable test sub_B2E240 is the collision ground test at the top.
        if matches!(dir, 0 | 1 | 6 | 7) && matches!(d.pose, 0 | 3) {
            let mid = (d.hand_l + d.hand_r) * 0.5;
            let top = Vec3::new(mid.x, mid.y, mid.z) - n * PULLUP_IN;
            let top_edge = collision.ground_height(top + Vec3::Y * 0.05, 0.3).is_some_and(|h| (h - mid.y).abs() < 0.25);
            if top_edge {
                let (knee, stand) = super::ledge_moves::climb_out_moves(d.pose == 3, body.feet, d.hand_l, d.hand_r, n);
                let entry = LedgeEntry {
                    hand_l: d.hand_l,
                    hand_r: d.hand_r,
                    normal: n,
                    from_feet: body.feet,
                    sub_state: LedgeSubState::Pullup,
                    entry_move: Some(knee),
                    entry_rest: [Some(stand), None],
                };
                d.last_action = "climb out";
                switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(entry));
                continue;
            }
        }
        // TryDropToLedgeBelow 0xDF1D20 (down dirs, level poses 0 / 3): ground within 1.3 m of a ray from the root + 0.5 m
        // → `climb_{1m,2m}_tr_h_wait_hipm_footl_a` while the root moves onto it, then Ground. PORT: the first search
        // (sub_DECEA0, a landing point ahead with a 1.7 m ray) is not decoded; only the ray under the root is used.
        if matches!(dir, 2 | 3 | 8 | 9) && matches!(d.pose, 0 | 3) {
            let from = body.feet + Vec3::Y * 0.5;
            if let Some(h) = collision.ground_height(from, 1.5).filter(|h| from.y - h <= 1.3) {
                let id = CLIMB_TO_GROUND[(d.pose == 3) as usize];
                d.move_action = Some(id);
                d.move_seq += 1;
                d.moving = Some(RootInterp::new(body.feet, Vec3::new(body.feet.x, h, body.feet.z), move_time(Some(id))));
                d.to_ground = true;
                d.look = None;
                d.last_action = "climb down to the ground";
                continue;
            }
        }
        // TryBackEject 0xDF2F50 (up directions, last in ChooseMove): an edge around the hands' midpoint at the root +
        // 2.25 m, 0.8 m out from the wall → the impulse (StartRelease 0xDE96A0) and the jump up into a free hang
        // (0xDF9650). PORT: the game's box query (sub_1171050, extents ±0.2 / 0.5 / 0.75) and its body clearance
        // test (sub_B2DD50) are approximated by a 0.5 m guidance probe with 0.75 m vertical tolerance.
        if matches!(dir, 0 | 1 | 6 | 7) {
            let hands = (d.hand_l + d.hand_r) * 0.5;
            let at = Vec3::new(hands.x, body.feet.y + CLIMB_JUMP_UP, hands.z) - facing * CLIMB_JUMP_OUT;
            let hit = guidance
                .probe(at, CLIMB_JUMP_PROBE_R, CLIMB_JUMP_PROBE_VTOL, Some(facing), 0.785)
                .filter(|h| h.point.y > hands.y + 0.5 && Vec2::new(h.point.x - hands.x, h.point.z - hands.z).length() > 0.3);
            if let Some(h) = hit {
                let n = h.wall_normal;
                let point = guidance.fit_hands(h.point, n);
                let mut e = LedgeEntry::at(point, n, body.feet, LedgeSubState::HangWallReception);
                e.entry_move = Some(super::ledge_moves::climb_jump_move(body.feet, hands, e.hand_l, e.hand_r, n, &collision));
                d.look = None;
                d.last_action = "jump up";
                switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(e));
                continue;
            }
        }
        // OnNoMoveFound 0xDF4410: look around toward the stick
        d.last_action = "blocked";
        d.look = Some(look_around(dir));
    }
}
