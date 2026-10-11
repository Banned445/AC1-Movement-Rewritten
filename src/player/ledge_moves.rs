//! Ledge moves beyond the shimmy and the hand steps (RE/03 §7.6, decoded 2026-10-01):
//! - **corner turns** (`HumanLedge__TryTurnCorner` 0xDD3BB0, candidates from `HumanLedge__FindCornerEdges`
//!   0xDD0600): inner corner (the wall ahead) and outer corner (around the end of the wall);
//! - **side jumps between ledges** (`HumanLedge__TrySideJumpToLedge` 0xDDD490 → `StartLedgeJump` 0xDDCE40,
//!   table 0x1A2C780 / 0x1A2C980);
//! - **the swing back** to an overhang's front edge behind the climber (`HumanLedge__TryWallJumpUp` 0xDD62A0 → the
//!   blended `StartLedgeJump`; the game's name says "jump up", but its boxes search out from the wall, RE/24 §5).
//!
//! The move plays its action(s) while the root travels from the current hang to the target hang. Ledge
//! jumps follow the clips' blended displacement plus a linear correction to the target (hypothesis: the
//! same root interpolation mode as the InAir jump, RE/13 §3); corners use the root interpolator over the
//! clip length (0xDD3BB0 → sub_711130, flag 0).

use bevy::prelude::*;

use super::jump_blend::{self, ActionBlend};
use super::ledge::{hang_root_at, hang_root, LedgeHangType};
use super::right_of;
use crate::collision::CollisionWorld;
use crate::guidance::{GuidanceWorld, MASK_CLIMB_HOLDS};
use crate::tuning::*;

/// Ledge-jump table (0x1A2C780 wall hang / 0x1A2C980 free hang; filled by HumanLedge__StaticInitTables
/// 0xDCC2A0, read from the emulated init `RE/data/ledge_init.pkl`). Index `(type·2 + long)·4 + side`;
/// entry = (landing category, start action, loop action, end action). Types: 0 climb holds, 1 wall-hang
/// ledge, 2 free-hang ledge, 3 ladder. Sides: 0 up, 1 down, 2 left, 3 right.
#[rustfmt::skip]
pub const LEDGE_JUMP_TABLE: [[(u8, [u32; 3]); 32]; 2] = [
    [
        (0, [0x491228AE, 0x491228AF, 0x491228B0]), (0, [0; 3]), (0, [0x491228FE, 0x491228FF, 0x49122900]), (0, [0x49122904, 0x49122905, 0x49122906]),
        (0, [0x491228B1, 0x491228B2, 0x491228B3]), (0, [0; 3]), (0, [0x49122901, 0x49122902, 0x49122903]), (0, [0x49122907, 0x49122908, 0x49122909]),
        (0, [0; 3]), (0, [0; 3]), (1, [0x491228C6, 0x491228C7, 0x491228C8]), (1, [0x491228CC, 0x491228CD, 0x491228CE]),
        (0, [0; 3]), (0, [0; 3]), (1, [0x491228C9, 0x491228CA, 0x491228CB]), (1, [0x491228CF, 0x491228D0, 0x491228D1]),
        (0, [0; 3]), (0, [0; 3]), (2, [0x491228DE, 0x491228DF, 0x491228E0]), (2, [0x491228E6, 0x491228E7, 0x491228E8]),
        (0, [0; 3]), (0, [0; 3]), (2, [0x491228E1, 0x491228E2, 0x491228E3]), (2, [0x491228E9, 0x491228EA, 0x491228EB]),
        (0, [0; 3]), (0, [0; 3]), (3, [0x491228FE, 0x491228FF, 0x52557360]), (3, [0x49122904, 0x49122905, 0x52557362]),
        (0, [0; 3]), (0, [0; 3]), (3, [0x49122901, 0x49122902, 0x52557361]), (3, [0x49122907, 0x49122908, 0x52557363]),
    ],
    [
        (0, [0; 3]), (0, [0; 3]), (0, [0x4C460359, 0x4C46035A, 0x4C46035B]), (0, [0x4C46035F, 0x4C460360, 0x4C460361]),
        (0, [0; 3]), (0, [0; 3]), (0, [0x4C46035C, 0x4C46035D, 0x4C46035E]), (0, [0x4C460362, 0x4C460363, 0x4C460364]),
        (0, [0; 3]), (0, [0; 3]), (1, [0x49122916, 0x49122917, 0x49122918]), (1, [0x4912291C, 0x4912291D, 0x4912291E]),
        (0, [0; 3]), (0, [0; 3]), (1, [0x49122919, 0x4912291A, 0x4912291B]), (1, [0x4912291F, 0x49122920, 0x49122921]),
        (0, [0; 3]), (0, [0; 3]), (2, [0x4C460372, 0x4C460373, 0x4C460374]), (2, [0x4C46037A, 0x4C46037B, 0x4C46037C]),
        (0, [0; 3]), (0, [0; 3]), (2, [0x4C460375, 0x4C460376, 0x4C460377]), (2, [0x4C46037D, 0x4C46037E, 0x4C46037F]),
        (0, [0; 3]), (0, [0; 3]), (3, [0x4C460359, 0x4C46035A, 0x52557360]), (3, [0x4C46035F, 0x4C460360, 0x52557362]),
        (0, [0; 3]), (0, [0; 3]), (3, [0x4C46035C, 0x4C46035D, 0x52557361]), (3, [0x4C460362, 0x4C460363, 0x52557363]),
    ],
];

/// The jump off's rebound turn (0xDD2CA0): [`xx_h_hangwall_tr_rebound_footl` (the jump to the character's right),
/// `…_footr` (to the left)].
pub const REBOUND: [u32; 2] = [0x011F_FC1A, 0x011F_FC1B];

/// `sub_DD2CA0` 0xDD2CA0, event 2's action on a wall hang (the jump off, RE/24 LEDGE-16): the rebound by the side of the
/// jump direction `jd` (the signed angle from it to straight back ≥ 0, i.e. to the character's left → `_footr`), the
/// root interpolated linearly over the action (`sub_711130` without an action) from the hang root to itself, lowered
/// by 0.3·min(|angle|, 45°)/45° for a jump to the right.
pub fn rebound_move(from: Vec3, n: Vec3, jd: Vec3, hand_l: Vec3, hand_r: Vec3) -> LedgeMove {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let away = -facing;
    // angle ≥ 0 (the left, and straight back) → `_footr`
    let left = jd.dot(right_of(facing)) <= 1e-6;
    let a = away.angle_between(Vec3::new(jd.x, 0.0, jd.z).normalize_or(away));
    let id = REBOUND[left as usize];
    let seq = [single(id, 0), None, None, None];
    let durations = seq_durations(&seq);
    let drop = if left { 0.0 } else { 0.3 * a.min(std::f32::consts::FRAC_PI_4) / std::f32::consts::FRAC_PI_4 };
    LedgeMove {
        kind: MoveKind::Rebound,
        seq,
        durations: if durations.iter().sum::<f32>() > 0.0 { durations } else { [0.3, 0.0, 0.0, 0.0] },
        t: 0.0,
        from,
        to: from - Vec3::Y * drop,
        facing_from: facing,
        facing_to: facing,
        follow_disp: false,
        lead: 0.0,
        table: None,
        root_time: None,
        end_free: false,
        end_wall: true,
        end_stand: false,
        hand_l,
        hand_r,
        normal: n,
    }
}

/// Corner actions (0xDD3BB0: 510126209 + k). Free hang: `hangfree_corner_{left,right}_090_{in,out}`; wall
/// hang: the two-item strafe open + close actions with the hand targets on the new edge.
/// [free, wall] × [left in, left out, right in, right out].
pub const CORNER_ACTIONS: [[u32; 4]; 2] = [
    [0x1E67_E881, 0x1E67_E882, 0x1E67_E883, 0x1E67_E884],
    [0x1E67_E885, 0x1E67_E886, 0x1E67_E887, 0x1E67_E888],
];
/// Blended swing back (0xDDCE40 with flag 1861&2): `hangwall_to_swingback_up_{min,max}_{200,300}_a`, then
/// (0xDDAB00) `…_b` with the same weights while the root moves to the new ledge; it ends in a free hang.
pub const SWING_BACK: u32 = 0x4CD6_8F4F;
pub const SWING_BACK_B: u32 = 0x4CD6_8F50;

pub const DUMPED_ACTIONS: &[u32] = &[
    RECEPTION_WALL_OTHER, REBOUND[0], REBOUND[1],
    0x491228AE, 0x491228AF, 0x491228B0, 0x491228B1, 0x491228B2, 0x491228B3,
    0x491228C6, 0x491228C7, 0x491228C8, 0x491228C9, 0x491228CA, 0x491228CB,
    0x491228CC, 0x491228CD, 0x491228CE, 0x491228CF, 0x491228D0, 0x491228D1,
    0x491228DE, 0x491228DF, 0x491228E0, 0x491228E1, 0x491228E2, 0x491228E3,
    0x491228E6, 0x491228E7, 0x491228E8, 0x491228E9, 0x491228EA, 0x491228EB,
    0x49122916, 0x49122917, 0x49122918, 0x49122919, 0x4912291A, 0x4912291B,
    0x4912291C, 0x4912291D, 0x4912291E, 0x4912291F, 0x49122920, 0x49122921,
    0x4C460372, 0x4C460373, 0x4C460374, 0x4C460375, 0x4C460376, 0x4C460377,
    0x4C46037A, 0x4C46037B, 0x4C46037C, 0x4C46037D, 0x4C46037E, 0x4C46037F,
    0x1E67_E881, 0x1E67_E882, 0x1E67_E883, 0x1E67_E884, 0x1E67_E885, 0x1E67_E886, 0x1E67_E887, 0x1E67_E888,
    SWING_BACK, SWING_BACK_B,
    // hang-type switches (0xDE1060)
    TO_WALL[0], TO_WALL[1], TO_FREE[0], TO_FREE[1], TO_FREE[2], TO_FREE[3],
    // straight jump to a hand target (0xB21DA0) and its arrivals (0xE07D00)
    0x0129_0ECF, 0x0127_2A69, 0x0127_1631, 0x0127_1639, 0x0121_A598, 0x0121_A8B1,
    0x0129_0ED0, 0x0127_2A6A, 0x0127_163A, 0x0127_1632, 0x0121_B072, 0x0127_23A5,
    // < 0.7 m band: collide_full → freestep flight and its reception
    0x012B_291B, RECEPTION_STEP_UP,
    ACT_WAIST_TO_KNEE, ACT_KNEE_TO_WAIT, ACT_KNEE_TO_FREESTEP[0], ACT_KNEE_TO_FREESTEP[1],
    // running jump onto a ledge: wall reception and free-hang swing (0xE07D00 generic branch)
    RECEPTION_SURFACE_WALL, SWING_RECEPTION,
    // pull-down (0xDDE4D0 / 0xDDE980)
    PULLDOWN_ORIENT[0], PULLDOWN_ORIENT[1], PULLDOWN_DESCENT,
    PULLDOWN_BEAM_ORIENT[0], PULLDOWN_BEAM_ORIENT[1], PULLDOWN_BEAM_ORIENT[2], PULLDOWN_BEAM_ORIENT[3],
    PULLDOWN_BEAM_DESCENT[0], PULLDOWN_BEAM_DESCENT[1], PULLDOWN_BEAM_DESCENT[2], PULLDOWN_BEAM_DESCENT[3],
    PULLDOWN_WALL[0], PULLDOWN_WALL[1], PULLDOWN_FREE[0], PULLDOWN_FREE[1],
    // ledge stop (HumanGround sub-state 38, 0xD93C60 / 0xD7D9D0)
    LEDGE_STOP_START, LEDGE_STOP_END,
    // pull-up (Pullup_Start 0xDDBE80)
    ACT_PULLUP_WALL, ACT_PULLUP_FREE,
    // climb-out over the top (TryReachLedgeAbove 0xDF1730)
    CLIMB_OUT[0], CLIMB_OUT[1],
    // SecondHandGrab chains (0xDCD5E0): loop X → X+1, X+2, X+3
    0x4912_2885, 0x4912_2886, 0x4912_2887, 0x4912_2888, 0x4912_2889, 0x4912_288D, 0x4912_288E, 0x4912_288F, 0x4912_2890, 0x4912_2891,
    0x4912_28E4, 0x4912_28E5, 0x4912_28EC, 0x4912_28ED, 0x4C46_0378, 0x4C46_0379, 0x4C46_0380, 0x4C46_0381,
    // one-hand pull-ups (0xDDBE80 / 0xDD1120)
    ACT_PULLUP_WALL_ONEHAND, ACT_PULLUP_FREE_ONEHAND, ACT_WAIST_TO_KNEE_ONEHAND, ACT_KNEE_ONEHAND_TO_FREESTEP,
    // the climb's reaches into a hang (CLIMB_REACH_TABLE types 1 / 2)
    0x4912_2876, 0x4912_2877, 0x4912_2878, 0x4912_287C, 0x4912_287D, 0x4912_287E,
    0x4912_2879, 0x4912_287A, 0x4912_287B, 0x4912_287F, 0x4912_2880, 0x4912_2881,
    0x4912_2882, 0x4912_2883, 0x4912_2884, 0x4912_288A, 0x4912_288B, 0x4912_288C,
];

/// The climb's reach table (filled by `HumanClimb__StaticInitTables` 0xDE4F80 at 0x1A2E540, read by
/// `HumanClimb__StartReachMove` 0xDF6810): the entry at 0x1A2E540 + 16·((type·2 + long)·10 + dir) is {landing
/// category (= type), start, loop, end}. Listed here for the side directions only (dir 4 left / 5 right), as
/// [type][long][right] → [start, loop, end]. Types: 0 climb holds (`climb1m_tr_climb1m_{left,right}_{2,3}`), 1 wall hang
/// (`climb1m_tr_hangwall_…`), 2 free hang (`climb1m_tr_hangfree_…`), 3 ladder (the climb → climb start and loop, then
/// the ladder's end 0x52557360 / 0x52557362; from `HumanClimb__TrySideReach` 0xDF2EC0). Long = the `_3` actions.
#[rustfmt::skip]
pub const CLIMB_REACH_TABLE: [[[[u32; 3]; 2]; 2]; 4] = [
    [[[0x4912_285E, 0x4912_285F, 0x4912_2860], [0x4912_2864, 0x4912_2865, 0x4912_2866]],
     [[0x4912_2861, 0x4912_2862, 0x4912_2863], [0x4912_2867, 0x4912_2868, 0x4912_2869]]],
    [[[0x4912_2876, 0x4912_2877, 0x4912_2878], [0x4912_287C, 0x4912_287D, 0x4912_287E]],
     [[0x4912_2879, 0x4912_287A, 0x4912_287B], [0x4912_287F, 0x4912_2880, 0x4912_2881]]],
    [[[0x4912_2882, 0x4912_2883, 0x4912_2884], [0x4912_288A, 0x4912_288B, 0x4912_288C]],
     [[0x4912_2885, 0x4912_2886, 0x4912_2887], [0x4912_288D, 0x4912_288E, 0x4912_288F]]],
    [[[0x4912_285E, 0x4912_285F, 0x5255_7360], [0x4912_2864, 0x4912_2865, 0x5255_7362]],
     [[0x4912_285E, 0x4912_285F, 0x5255_7360], [0x4912_2864, 0x4912_2865, 0x5255_7362]]],
];

/// Ledge stop: `xx_h_ledge_stop_start_footl` (played on entry, 0xD93C60) and `xx_h_ledge_stop_end_footl` (played
/// when the start action is done, 0xD7D9D0; its transition 0x06E8BD7E leads to wait).
pub const LEDGE_STOP_START: u32 = 0x06E8_BD7F;
pub const LEDGE_STOP_END: u32 = 0x06E8_BD7D;

/// One item of an action with weight 1 (for the ground's one-shot actions).
pub fn single_item(id: u32, item: usize) -> Option<ActionBlend> {
    single(id, item)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveKind {
    Corner { inner: bool },
    SideJump { long: bool },
    /// The swing back to an edge behind (`TryWallJumpUp` 0xDD62A0).
    SwingBack,
    /// Table jump type 0 up onto climb holds (0xDD5E10); the Climb context takes over for the end item.
    JumpUpToClimb,
    /// Free hang → holds below and ahead (0xDDF390): onto climb holds, or into a wall hang.
    FreeHangDrop { to_climb: bool },
    /// Hang → ladder at the side (0xDD4CD0).
    SideToLadder,
    /// Climb → a hang to the side (`HumanClimb__TryReachOtherSurface` 0xDF9B30, the climb's reach table).
    ClimbReach { long: bool },
    /// Received after a jump at a ledge (0xE07D00).
    Arrival,
    /// Wall ↔ free hang (0xDE1060).
    SwitchHang { to_wall: bool },
    /// Onto the top (Pullup_Start 0xDDBE80).
    Pullup,
    /// Ground → hang over an edge (0xDDE4D0 / 0xDDE980): orientation, descent, reception.
    PullDown { stage: u8 },
    /// The jump off a wall hang: the rebound turn before the jump (event 2 → 0xDD2CA0, SubState 5, state 10).
    Rebound,
}

/// A running ledge move.
#[derive(Clone, Copy, Debug)]
pub struct LedgeMove {
    pub kind: MoveKind,
    /// The action items played in sequence.
    pub seq: [Option<ActionBlend>; 4],
    pub durations: [f32; 4],
    pub t: f32,
    pub from: Vec3,
    pub to: Vec3,
    /// Facing (into the wall) at the start and at the end.
    pub facing_from: Vec3,
    pub facing_to: Vec3,
    /// The root follows the clips' displacement (+ correction) instead of the interpolator.
    pub follow_disp: bool,
    /// Interpolator only: the root holds still for this long first (the hop's `_a` item).
    pub lead: f32,
    /// A table ledge jump's root path (RE/24 LEDGE-14): (start items, linear items). The start items' own displacement
    /// moves the root (no interpolator, `StartLedgeJump` 0xDDCE40); the next items interpolate it linearly to `to`
    /// (`sub_DD8550` → `sub_711130` in spline mode over the loop's blended duration); any later item holds it there
    /// (the end action is the hang idle's entry, `sub_DCF280`). None: `follow_disp` / the interpolator as usual.
    pub table: Option<(usize, usize)>,
    /// The receptions' root (`HumanInAir__PlayLedgeReception` 0xE02BA0, `sub_711130` mode 0): linear onto `to` over this
    /// long, starting after `lead`, then held while the actions play on (overrides `follow_disp` and the interpolator).
    /// None: as above.
    pub root_time: Option<f32>,
    /// The move ends in a free hang whatever the wall below (the hop, 0xDDAB00 sets LedgeHangType 1).
    pub end_free: bool,
    /// The move ends in a wall hang (free → wall switch, wall receptions).
    pub end_wall: bool,
    /// The move ends standing on top (knee / waist arrivals: Ledge SubState 4 → pull-up → Ground).
    pub end_stand: bool,
    /// Hands and wall normal once the move ends.
    pub hand_l: Vec3,
    pub hand_r: Vec3,
    pub normal: Vec3,
}

impl LedgeMove {
    pub fn duration(&self) -> f32 {
        self.durations.iter().sum::<f32>().max(1e-3)
    }

    /// Item playing at the current time and its phase.
    pub fn current(&self) -> Option<(ActionBlend, f32)> {
        let mut t0 = 0.0;
        for (i, a) in self.seq.iter().enumerate() {
            let Some(a) = a else { continue };
            let d = self.durations[i];
            if self.t < t0 + d || i == self.seq.len() - 1 || self.seq[i + 1..].iter().all(|x| x.is_none()) {
                return Some((*a, ((self.t - t0) / d.max(1e-4)).clamp(0.0, 1.0)));
            }
            t0 += d;
        }
        None
    }

    /// Summed displacement of the sequence up to time `t` (animation space).
    fn disp(&self, t: f32) -> [f32; 3] {
        let mut o = [0.0f32; 3];
        let mut t0 = 0.0;
        for (i, a) in self.seq.iter().enumerate() {
            let Some(a) = a else { continue };
            let d = self.durations[i];
            let ph = ((t - t0) / d.max(1e-4)).clamp(0.0, 1.0);
            let v = a.disp(ph);
            for k in 0..3 {
                o[k] += v[k];
            }
            t0 += d;
        }
        o
    }

    /// Advance; returns the root position and whether the move ended.
    pub fn advance(&mut self, dt: f32) -> (Vec3, bool) {
        let total = self.duration();
        self.t = (self.t + dt).min(total);
        let s = self.t / total;
        let p = if let Some(rt) = self.root_time {
            self.from.lerp(self.to, ((self.t - self.lead) / rt.max(1e-3)).clamp(0.0, 1.0))
        } else if let Some((k, m)) = self.table {
            let to_world = |d: [f32; 3]| right_of(self.facing_from) * d[0] + self.facing_from * d[1] + Vec3::Y * d[2];
            let t_start: f32 = self.durations.iter().take(k).sum();
            let t_loop: f32 = self.durations.iter().skip(k).take(m).sum();
            if self.t < t_start {
                self.from + to_world(self.disp(self.t))
            } else {
                let start_end = self.from + to_world(self.disp(t_start));
                start_end.lerp(self.to, ((self.t - t_start) / t_loop.max(1e-4)).clamp(0.0, 1.0))
            }
        } else if self.follow_disp {
            let to_world = |d: [f32; 3]| right_of(self.facing_from) * d[0] + self.facing_from * d[1] + Vec3::Y * d[2];
            let end = self.from + to_world(self.disp(total));
            self.from + to_world(self.disp(self.t)) + (self.to - end) * s
        } else {
            let s = ((self.t - self.lead) / (total - self.lead).max(1e-4)).clamp(0.0, 1.0);
            self.from.lerp(self.to, s)
        };
        (p, self.t >= total)
    }

    /// Where the clips' own displacement ends (no correction).
    pub fn natural_end(&self) -> Vec3 {
        let d = self.disp(self.duration());
        self.from + right_of(self.facing_from) * d[0] + self.facing_from * d[1] + Vec3::Y * d[2]
    }

    /// Summed root yaw of the sequence up to time `t` (radians, + = left).
    fn yaw(&self, t: f32) -> f32 {
        let mut o = 0.0;
        let mut t0 = 0.0;
        for (i, a) in self.seq.iter().enumerate() {
            let Some(a) = a else { continue };
            let d = self.durations[i];
            o += a.yaw(((t - t0) / d.max(1e-4)).clamp(0.0, 1.0));
            t0 += d;
        }
        o
    }

    /// Facing during the move (turned from the old wall to the new one). When the clips' root yaw makes the same turn
    /// (the corners' ±90°) the facing follows it, with the remainder spread linearly; otherwise it is interpolated.
    pub fn facing(&self) -> Vec3 {
        let total = self.duration();
        let s = (self.t / total).clamp(0.0, 1.0);
        let h0 = super::heading_of(self.facing_from);
        let mut turn = super::heading_of(self.facing_to) - h0;
        while turn > std::f32::consts::PI {
            turn -= std::f32::consts::TAU;
        }
        while turn < -std::f32::consts::PI {
            turn += std::f32::consts::TAU;
        }
        let clip_turn = self.yaw(total);
        if turn.abs() > 0.1 && (clip_turn - turn).abs() < 0.35 {
            let h = h0 + self.yaw(self.t) + (turn - clip_turn) * s;
            return Vec3::new(-h.sin(), 0.0, -h.cos());
        }
        self.facing_from.lerp(self.facing_to, s).normalize_or(self.facing_to)
    }
}

fn single(id: u32, item: usize) -> Option<ActionBlend> {
    jump_blend::action_items(id).filter(|i| i.len() > item).map(|_| ActionBlend::new(id, item, &[1.0]))
}

pub(crate) fn seq_durations(seq: &[Option<ActionBlend>; 4]) -> [f32; 4] {
    let mut d = [0.0; 4];
    for (i, a) in seq.iter().enumerate() {
        d[i] = a.map(|a| a.duration()).unwrap_or(0.0);
    }
    d
}

/// Five rows of edge hits at height offsets −1.8, −1.2, −0.6, 0, +0.6 m (the ledge searches' row layout: ProbeLateral
/// 0xDD9640 at +288, FindCornerEdges 0xDD0600 at +1904 / +2304, 80 bytes each).
pub type EdgeRows = [Option<crate::guidance::GuidanceHit>; 5];

/// The row height offsets (from the lead hand or the hands' midpoint).
pub const ROW_OFFSETS: [f32; 5] = [-1.8, -1.2, -0.6, 0.0, 0.6];

/// `HumanLedge__FindCornerEdges` 0xDD0600: the five rows of the corner's other face. The box (0x1170A70 on the ledge
/// report: half extents 0.25 / 0.4 / 0.3, 50°, the reference 0.2 m back, mask 50) faces that face: `inner` (near) =
/// hands mid (lower hand) + 0.6·move − 0.6·facing, facing along the move; outer (far) = hands mid + 0.2·move +
/// 0.3·facing, facing back against the move (base − move − 1.8 z, then + 0.3·forward + 1.2·move, 0xDD0797; RE/24
/// LEDGE-12).
pub fn corner_rows(inner: bool, right: bool, hand_l: Vec3, hand_r: Vec3, n: Vec3, guidance: &GuidanceWorld) -> EdgeRows {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let side = if right { right_of(facing) } else { -right_of(facing) };
    let mut base = (hand_l + hand_r) * 0.5;
    base.y = hand_l.y.min(hand_r.y);
    let (p, zone_forward) = if inner { (base + side * 0.6 - facing * 0.6, side) } else { (base + facing * 0.3 + side * 0.2, -side) };
    ROW_OFFSETS.map(|dy| guidance.probe_facing(p + Vec3::Y * dy, zone_forward, Vec3::new(0.25, 0.4, 0.3), 0.2, 0.872_664_6, MASK_CLIMB_HOLDS))
}

/// A capsule swept along `dir`: the nearest free distance of spheres (radius `r`) spaced at most 0.1 m along the segment
/// `a`–`b` shortened by `r` at both ends (`sub_B18C30` 0xB18C30). PORT: sampled spheres stand in for the capsule cast.
fn capsule_free_distance(a: Vec3, b: Vec3, dir: Vec3, r: f32, max: f32, collision: &CollisionWorld) -> f32 {
    let axis = b - a;
    let len = axis.length();
    let (a, b) = if len > 2.0 * r { (a + axis / len * r, b - axis / len * r) } else { ((a + b) * 0.5, (a + b) * 0.5) };
    let n = (((b - a).length() / 0.1).ceil() as usize).max(1);
    (0..=n).map(|k| collision.sphere_free_distance(a.lerp(b, k as f32 / n as f32), dir, r, max)).fold(max, f32::min)
}

/// `HumanLedge__ProbeLateral` 0xDD9640 for a shimmy step (`closing` = the alternation flag +133: the trailing hand
/// closes). The moving hand `mover`, the other one `other`, `dv` the move direction:
/// - the reach (not closing): a capsule (r 0.15, `sub_B18C30` 0xB18C30) from the moving hand pulled 0.15 m back,
///   0.75 m out from the wall and 0.2 m up, down to 1.1 m (wall hang) / 2.4 m (free hang) below the hand
///   (0xDD996B..0xDD99F1), swept 1.55 m along the move, gives d = free distance − 0.15; d < 0.7 is blocked (no rows);
///   the width w = min(d − 0.4, 1 − |handL − handR|) (RE/24 LEDGE-8: the body, not only the hands, must have room);
/// - the closing step: w = |handL − handR| − 0.15.
///
/// Five rows (moving hand −1.8 … +0.6 m): a box from 0.1 m beyond the moving hand to w + 0.1 m (half extents w/2 /
/// 0.5 / 0.3, 45°, mask 50, on the ledge report) with the reference at its far end, 0.2 m back from the wall: the
/// farthest reachable point.
///
/// The shimmy target (row 3, the moving hand's height): reaching, the hit at least 0.15 m beyond moving hand + 0.1·move;
/// closing, the hit unless it lies more than 0.3 m from the other hand, or none: then the other hand's own point (the
/// hands come together).
pub fn probe_lateral(mover: Vec3, other: Vec3, n: Vec3, dv: Vec3, closing: bool, hang: LedgeHangType, guidance: &GuidanceWorld, collision: &CollisionWorld) -> (EdgeRows, Option<Vec3>) {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let spacing = (mover - other).length();
    let width = if closing {
        spacing - 0.15
    } else {
        let top = mover - dv * SHIMMY_SWEEP_R + n * SHIMMY_SWEEP_OUT + Vec3::Y * SHIMMY_SWEEP_UP;
        let drop = if hang == LedgeHangType::Wall { 1.1 } else { 2.4 };
        let d = capsule_free_distance(top, top - Vec3::Y * (drop + SHIMMY_SWEEP_UP), dv, SHIMMY_SWEEP_R, SHIMMY_SWEEP_LEN, collision) - SHIMMY_SWEEP_R;
        if d < SHIMMY_MIN_FREE {
            return ([None; 5], None);
        }
        (d - SHIMMY_WALL_MARGIN).min(SHIMMY_MAX_SPACING - spacing)
    };
    if width <= 0.0 {
        return ([None; 5], if closing { Some(other) } else { None });
    }
    let half = Vec3::new(width * 0.5, 0.5, 0.3);
    let right = facing.cross(Vec3::Y);
    let rows = ROW_OFFSETS.map(|dy| {
        let centre = mover + dv * (width * 0.5 + 0.1) + Vec3::Y * dy;
        let reference = centre + dv * (width * 0.5) - facing * 0.2;
        guidance.probe_box(centre, right, facing, Vec3::Y, half, reference, 0.785_398_2, crate::guidance::PROBE_STEP, MASK_CLIMB_HOLDS)
    });
    let row = rows[3].map(|h| h.point);
    let flat = |a: Vec3, b: Vec3| Vec2::new(a.x - b.x, a.z - b.z).length();
    let target = if closing {
        Some(row.filter(|&p| flat(other, p) <= 0.3).unwrap_or(other))
    } else {
        row.filter(|&p| (mover + dv * 0.1).distance(p) >= SHIMMY_MIN_STEP)
    };
    (rows, target)
}

/// `HumanLedge__TrySideMoveToClimbHolds` 0xDD48B0 on a row source (ProbeLateral's or a corner's): `band` 0 up (flag 4:
/// feet row 2, hands row 4), 1 level (8: rows 1 / 3), 2 down (16: rows 0 / 2). Both hands go to the hand row's hit and
/// both feet to the feet row's (climb pose 0), then the transition `LEDGE_SIDE_TO_CLIMB` plays. Not during a closing
/// step (+133). PORT: the clearance tests (`Human__ClimbBodyBoxesFree` 0xB2DF90, `Human__LateralPathFree` 0xB2A410,
/// 1.8 m) are not modelled.
pub fn side_move_to_climb_holds(rows: &EdgeRows, band: usize, right: bool, hang: LedgeHangType, feet: Vec3) -> Option<super::climb::ClimbEntry> {
    let (fi, hi) = [(2usize, 4usize), (1, 3), (0, 2)][band];
    let (foot, hand) = (rows[fi]?, rows[hi]?);
    Some(super::climb::ClimbEntry {
        entry_type: super::climb::ClimbEntryType::FromLedge,
        pose_2m: None, end_action: None,
        hand_l: hand.point,
        hand_r: hand.point,
        foot_l: foot.point,
        foot_r: foot.point,
        normal: hand.wall_normal,
        from_feet: feet,
        foot_right: false,
        action: Some(super::climb::LEDGE_SIDE_TO_CLIMB[right as usize][band][(hang != LedgeHangType::Wall) as usize]),
    })
}

/// `sub_DD8F70`'s three vertical searches (`GuidanceWorld::vertical_search`, 0.4 m wide, the ledge report of
/// `sub_DDC620`: up −1.0 … +2.0 m, down −2.0 … 0 m around the higher hand), from the hands' midpoint at the lower hand
/// (up) or the higher one (down):
/// - `feet` (+688): up 0.3–0.9 m below, down 1.5–2.1 m below (the farthest along the step);
/// - `hands` (+768): up 0.3–0.9 m above, down 0.3–0.9 m below;
/// - `step` (+848, the hand step): up 0.1–0.8 m above, down 0.1–0.8 m below, the edge held cut out (capsule r 0.2 at
///   the hands) and a ±30° wedge about the facing (up) or its opposite (down).
///
/// PORT: when the hands are within 0.1 m in height the game picks the height by the last vertical hand (+1844); the
/// port keeps the lower / higher one.
#[derive(Clone, Copy, Debug, Default)]
pub struct VerticalRows {
    pub feet: Option<crate::guidance::GuidanceHit>,
    pub hands: Option<crate::guidance::GuidanceHit>,
    pub step: Option<crate::guidance::GuidanceHit>,
}

pub fn vertical_rows(hand_l: Vec3, hand_r: Vec3, n: Vec3, up: bool, guidance: &GuidanceWorld) -> VerticalRows {
    use crate::guidance::Wedge;
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mut mid = (hand_l + hand_r) * 0.5;
    mid.y = if up { hand_l.y.min(hand_r.y) } else { hand_l.y.max(hand_r.y) };
    let top = hand_l.y.max(hand_r.y);
    let window = if up { (top - 1.0, top + 2.0) } else { (top - 2.0, top) };
    let wedge = |cut_up: f32| Some(Wedge { angle: 0.523_598_8, cut_up });
    let search = |dy: f32, lo: f32, hi: f32, w: Option<Wedge>| guidance.vertical_search(mid + Vec3::Y * dy, up, facing, lo, hi, 0.4, 0.0, w, false, window);
    if up {
        VerticalRows { feet: search(-0.9, 0.0, 0.6, None), hands: search(0.3, 0.0, 0.6, None), step: search(0.0, 0.1, 0.8, wedge(0.0)) }
    } else {
        VerticalRows { feet: search(-2.1, 0.0, 0.6, None), hands: search(-0.9, 0.0, 0.6, None), step: search(-0.8, 0.0, 0.7, wedge(0.8)) }
    }
}

/// `HumanLedge__TryTurnCorner` 0xDD3BB0 on FindCornerEdges' same-height row (row 3). `right` = moving right.
pub fn try_corner(
    inner: bool,
    right: bool,
    hand_l: Vec3,
    hand_r: Vec3,
    n: Vec3,
    root: Vec3,
    hang: LedgeHangType,
    guidance: &GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<LedgeMove> {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let hit = corner_rows(inner, right, hand_l, hand_r, n, guidance)[3]?;
    let nn = hit.wall_normal;
    // TryTurnCorner never changes the hang type (+104 is not written): a free hang turns as a free hang (pose
    // `sub_B15AD0`); a wall hang needs foot holds on the new wall (`Human__FindWallHangFeetSupports` 0xB16130, pose
    // `sub_B157B0`). Both hand targets are the corner row point (0xDD3C17..0xDD3CE1). PORT: the body boxes and
    // `LateralPathFree` (2.4 / 1.1 m) are the capsule test at the target (RE/24 LEDGE-11).
    if hang == LedgeHangType::Wall && !super::ledge::feet_supports(hit.point, nn, collision) {
        return None;
    }
    let (hl, hr) = (hit.point, hit.point);
    let to = hang_root_at(hl, hr, nn, hang, collision);
    if !collision.capsule_fits(to + Vec3::Y * 0.05) {
        return None;
    }
    let col = (right as usize) * 2 + (!inner) as usize;
    let id = CORNER_ACTIONS[(hang == LedgeHangType::Wall) as usize][col];
    let seq = if hang == LedgeHangType::Wall { [single(id, 0), single(id, 1), None, None] } else { [single(id, 0), None, None, None] };
    let durations = seq_durations(&seq);
    let durations = if durations.iter().sum::<f32>() > 0.0 { durations } else { [CORNER_FALLBACK_TIME, 0.0, 0.0, 0.0] };
    Some(LedgeMove {
        kind: MoveKind::Corner { inner },
        seq,
        durations,
        t: 0.0,
        from: root,
        to,
        facing_from: facing,
        facing_to: -Vec3::new(nn.x, 0.0, nn.z).normalize_or_zero(),
        follow_disp: false,
        lead: 0.0,
        table: None,
        root_time: None,
        end_free: hang == LedgeHangType::Free,
        end_wall: hang == LedgeHangType::Wall,
        end_stand: false,
        hand_l: hl,
        hand_r: hr,
        normal: nn,
    })
}

/// `HumanLedge__TrySideJumpToLedge` 0xDDD490 (table types 0 / 1 / 2): three passes from the hands' midpoint (highest
/// hand) + 0.9·move, hand starts at height offsets 0, +0.6, −0.6 and the feet 1.2 m below them. Each pass is
/// `sub_B153C0` (`GuidanceWorld::step_search`): 0.35 m boxes (half extents 0.175 / 0.3 / 0.3) stepped up to 1.6 m along
/// the move, climb holds (feet, then hands 1.2 m above) before a hang (hands only). Feet on climb holds → type 0: the
/// start and loop play here, the end item in the Climb context (the returned `ClimbEntry`, as `try_jump_up_to_climb`);
/// otherwise a hang (1 wall, 2 free). A landing whose body sweep (`Human__CapsuleCastFree` 0xB136E0) fails moves on to
/// the next pass. `long` = flat distance from the pass's hand start / 1.6 ≥ 0.5.
///
/// PORT: the hang's feet support (`sub_11718B0`, 1.0 m below the hand box) is the collision foot rays
/// (`hang_type_at`); the hang's body sweep is the capsule test at the target.
/// `TrySideJumpToLedge` 0xDDD490's clearance (0xDDDF85): a capsule r 0.35 from Q = `sub_B18140`(root) − 0.2625·u +
/// 0.15·outward (u the flat unit vector toward the target pose; `sub_B18140` adds the horizontal part of the root's
/// pitched up axis, nothing for the upright root, RE/24 CLIMB-13), 1.8 / 1.0 / 2.0 m tall for jump types 0 / 1 / 2,
/// swept by the move to the target root; a blocked pass goes on to the next one (RE/24 LEDGE-28).
fn side_jump_clear(root: Vec3, target: Vec3, n: Vec3, ty: usize, collision: &CollisionWorld) -> bool {
    let out = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let u = Vec3::new(target.x - root.x, 0.0, target.z - root.z).normalize_or_zero();
    let q = root - u * 0.2625 + out * 0.15;
    collision.capsule_cast_free(q, [1.8, 1.0, 2.0][ty], 0.35, target - root)
}

pub fn try_side_jump(
    right: bool,
    hand_l: Vec3,
    hand_r: Vec3,
    n: Vec3,
    root: Vec3,
    hang: LedgeHangType,
    guidance: &GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<(LedgeMove, Option<super::climb::ClimbEntry>)> {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let side = if right { right_of(facing) } else { -right_of(facing) };
    let mut base = (hand_l + hand_r) * 0.5;
    base.y = hand_l.y.max(hand_r.y);
    let start = base + side * 0.9;
    for dz in [0.0f32, 0.6, -0.6] {
        let hs = start + Vec3::Y * dz;
        let Some(found) = guidance.step_search(hs, hs - Vec3::Y * 1.2, side, facing, 1.6, 0.35, Vec2::new(0.3, 0.3), true, true) else { continue };
        let hit = found.hands;
        let nn = hit.wall_normal;
        let r = right_of(-nn);
        // both hands on the new edge: the first hand lands on the nearest point, the other one HAND_SPACING further
        // along the move (PORT: the game puts both hand records on that one hold and derives the pose from it)
        let c = hit.point + side * HAND_SPACING * 0.5;
        let (hl, hr) = (c - r * HAND_SPACING * 0.5, c + r * HAND_SPACING * 0.5);
        let dist = Vec2::new(hit.point.x - hs.x, hit.point.z - hs.z).length();
        let long = (dist / 1.6).clamp(0.0, 1.0) >= 0.5;
        // feet on climb holds → type 0 (the climb pose from the holds, `Human__ComputeClimbPoseFromHolds` 0xB1CC20), on
        // a wall surface → 1, none → 2 (RE/18 §7.2)
        if let Some(f) = found.feet {
            let fc = f.point + side * HAND_SPACING * 0.5;
            let (fl, fr) = (fc - r * HAND_SPACING * 0.5, fc + r * HAND_SPACING * 0.5);
            let target = super::climb::climb_root_at(hl, hr, fl, fr, nn);
            if !side_jump_clear(root, target, n, 0, collision) {
                continue;
            }
            let entry = LEDGE_JUMP_TABLE[(hang == LedgeHangType::Free) as usize][long as usize * 4 + 2 + right as usize];
            let [st, lp, end] = entry.1;
            // category 0 (RE/24 LEDGE-14): the start in the Ledge context on its own displacement, the loop in the Climb
            // context with the linear interpolation to the climb pose (0xDD8290, as `try_jump_up_to_climb`)
            let seq = [single(st, 0), None, None, None];
            let durations = seq_durations(&seq);
            let found = durations.iter().sum::<f32>() > 0.0;
            let fto = -Vec3::new(nn.x, 0.0, nn.z).normalize_or_zero();
            let mut mv = LedgeMove {
                kind: MoveKind::SideJump { long },
                seq,
                durations: if found { durations } else { [SIDE_JUMP_FALLBACK_TIME * 0.5, 0.0, 0.0, 0.0] },
                t: 0.0,
                from: root,
                to: root,
                facing_from: facing,
                facing_to: fto,
                follow_disp: false,
                lead: 0.0,
                table: Some((1, 0)),
                root_time: None,
                end_free: false,
                end_wall: false,
                end_stand: false,
                hand_l: hl,
                hand_r: hr,
                normal: nn,
            };
            mv.to = mv.natural_end();
            let _ = target;
            let climb = super::climb::ClimbEntry {
                entry_type: super::climb::ClimbEntryType::FromLedgeParallelJump,
                pose_2m: None, end_action: single(end, 0).map(|_| end),
                hand_l: hl,
                hand_r: hr,
                foot_l: fl,
                foot_r: fr,
                normal: nn,
                from_feet: mv.to,
                foot_right: false,
                action: single(lp, 0).map(|_| lp),
            };
            return Some((mv, Some(climb)));
        }
        // the feet's wall support (sub_11718B0 1.0 m below the hand box, its depth 0.3 m)
        let new_hang = if super::ledge::support_ray(found.hand_centre - Vec3::Y, nn, 0.3, 0.0, collision) { LedgeHangType::Wall } else { LedgeHangType::Free };
        let to = hang_root_at(hl, hr, nn, new_hang, collision);
        let ty = if new_hang == LedgeHangType::Wall { 1 } else { 2 };
        if !side_jump_clear(root, to, n, ty, collision) {
            continue;
        }
        let entry = LEDGE_JUMP_TABLE[(hang == LedgeHangType::Free) as usize][(ty * 2 + long as usize) * 4 + 2 + right as usize];
        // categories 1 / 2 (RE/24 LEDGE-14): the start on its own displacement, the loop interpolating the root linearly to
        // the wall / free pose and setting +104 by it (`sub_DD8550` 0xDD8550), the end the hang idle's entry
        // (`sub_DCF280`, the root held)
        let seq = [single(entry.1[0], 0), single(entry.1[1], 0), single(entry.1[2], 0), None];
        let durations = seq_durations(&seq);
        let found = durations.iter().sum::<f32>() > 0.0;
        return Some((LedgeMove {
            kind: MoveKind::SideJump { long },
            seq,
            durations: if found { durations } else { [SIDE_JUMP_FALLBACK_TIME, 0.0, 0.0, 0.0] },
            t: 0.0,
            from: root,
            to,
            facing_from: facing,
            facing_to: -Vec3::new(nn.x, 0.0, nn.z).normalize_or_zero(),
            follow_disp: false,
            lead: 0.0,
            table: found.then_some((1, 1)),
            root_time: None,
            end_free: new_hang == LedgeHangType::Free,
            end_wall: new_hang == LedgeHangType::Wall,
            end_stand: false,
            hand_l: hl,
            hand_r: hr,
            normal: nn,
        }, None));
    }
    None
}

/// The climb's reach sideways to another surface (`HumanClimb__TryReachOtherSurface` 0xDF9B30 → `sub_DF7850`, dirs
/// 4–9). `diag` = 0 level, 1 up (dirs 6 / 7), 2 down (8 / 9). From the feet's midpoint (the lower foot's height) +
/// 0.9·move, one pass of `sub_B153C0` (`GuidanceWorld::step_search`, 0.35 m boxes, half extents 0.175 / 0.3 / 0.3, up to
/// 1.6 m along the move) with the (feet, hands) start offsets (0, +1.2) / (+0.6, +1.8) / (−0.6, +0.6) by `diag`.
/// ChooseMove 0xDFDE90 calls it twice: `climb` (climb holds only) before `HumanClimb__TryLedgeGrab` 0xDF0980, `hang`
/// (hangs only) after it.
/// What it finds sets the type (+2944): feet on climb holds → 0, feet on a wall surface → 1 (wall hang, `sub_B157B0`),
/// hands only → 2 (free hang, `sub_B15AD0`). A capsule (r 0.35, 1.8 / 1.0 / 2.0 m tall by type) from the root pulled
/// 0.2625 m back from the target and 0.15 m out from the wall must sweep to the target root (`Human__CapsuleCastFree`
/// 0xB136E0). +2924 = clamp(flat distance from the hand search start to the hand / 1.6); long when ≥ 0.5.
///
/// The start action plays in the climb (`StartReachMove` 0xDF6810, state 3); when it ends (`State3_Update` 0xDF5A70)
/// a hang target switches to Ledge (`sub_DF3E70`, fill `sub_DEF0C0`): the loop plays while the root is interpolated to
/// the hang over its length, the entry goes to LedgeData +312, SubState 14 (ParallelJump: Movement with a step
/// running, hang type = category ≠ 1). The long free-hang loops are SecondHandGrab triggers (`second_hand_grab`).
///
/// Type 0 (feet on climb holds) stays in the climb: both hands on the hand record and both feet on the foot record, the
/// climb pose of those holds (0xB1CC20); the start, then the loop while the root goes to that pose (`StartReachLoop`
/// 0xDEEE60), then the end in the Wait state (0xDE9B50), `climb::ClimbReach`.
///
/// PORT: a hang's wall or free comes from the collision foot rays (`hang_type_at`), standing in for the wall-support
/// search `sub_11718B0` 1.0 m below the hand box.
///
/// The hangs' root (RE/24 CLIMB-11): still through the start (`StartReachMove` 0xDF6810 starts no interpolation), then
/// linear onto the hang over the loop's blended length (`FillLedge_FromReach` 0xDEF0C0 → `sub_711130(…, 0, 0, 0, 0, 0)`
/// 0xDEF2F9), held through the end. PORT: the start plays in the Ledge move here, not in the climb's state 3.
#[allow(clippy::too_many_arguments)]
pub fn try_climb_reach(
    right: bool,
    diag: usize,
    foot_l: Vec3,
    foot_r: Vec3,
    n: Vec3,
    root: Vec3,
    climb: bool,
    hang: bool,
    guidance: &GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<ClimbReachTarget> {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let side = if right { right_of(facing) } else { -right_of(facing) };
    let mut base = (foot_l + foot_r) * 0.5;
    base.y = foot_l.y.min(foot_r.y);
    let (foot_off, hand_off) = [(0.0f32, 1.2f32), (0.6, 1.8), (-0.6, 0.6)][diag.min(2)];
    let feet_start = base + side * 0.9 + Vec3::Y * foot_off;
    let start = base + side * 0.9 + Vec3::Y * hand_off;
    let found = guidance.step_search(start, feet_start, side, facing, 1.6, 0.35, Vec2::new(0.3, 0.3), climb, hang)?;
    let hit = found.hands;
    let nn = hit.wall_normal;
    let fc = -Vec3::new(nn.x, 0.0, nn.z).normalize_or_zero();
    let u_back = |to: Vec3| root - (to - root).normalize_or_zero() * 0.2625;
    let dist = Vec2::new(hit.point.x - start.x, hit.point.z - start.z).length();
    let long = (dist / 1.6).clamp(0.0, 1.0) >= 0.5;
    // feet on climb holds → type 0: climb → climb
    if let Some(f) = found.feet {
        let holds = [hit.point, hit.point, f.point, f.point];
        let normals = [nn, nn, f.wall_normal, f.wall_normal];
        let to = super::climb::climb_pose(holds, normals, fc).root;
        let from = u_back(to);
        if !collision.capsule_cast_free(from - facing * 0.15, 1.8, 0.35, to - from) {
            return None;
        }
        let ids = CLIMB_REACH_TABLE[0][long as usize][right as usize];
        return Some(ClimbReachTarget::Climb(super::climb::ClimbReach::new(ids, holds, normals, None)));
    }
    // both hand records are the hit (`ReachOtherSurfaceSide` 0xDF7FBC copies it into +3280 and +3360; RE/24 CLIMB-8); the
    // second hand's reach (state 17) spreads them
    let (hl, hr) = (hit.point, hit.point);
    // the feet's wall support (sub_11718B0 1.0 m below the hand box, its depth 0.3 m)
    let hang = if super::ledge::support_ray(found.hand_centre - Vec3::Y, nn, 0.3, 0.0, collision) { LedgeHangType::Wall } else { LedgeHangType::Free };
    let to = hang_root_at(hl, hr, nn, hang, collision);
    let ty = if hang == LedgeHangType::Wall { 1 } else { 2 };
    // clearance: a capsule pulled back from the target and out from the wall, swept to the target root
    let u = (to - root).normalize_or_zero();
    let from = root - u * 0.2625 - facing * 0.15;
    let height = [1.8, 1.0, 2.0][ty];
    if !collision.capsule_cast_free(from, height, 0.35, to - (root - u * 0.2625)) {
        return None;
    }
    let ids = CLIMB_REACH_TABLE[ty][long as usize][right as usize];
    let seq = [single(ids[0], 0), single(ids[1], 0), single(ids[2], 0), None];
    let durations = seq_durations(&seq);
    let found = durations.iter().sum::<f32>() > 0.0;
    Some(ClimbReachTarget::Hang(LedgeMove {
        kind: MoveKind::ClimbReach { long },
        seq,
        durations: if found { durations } else { [SIDE_JUMP_FALLBACK_TIME, 0.0, 0.0, 0.0] },
        t: 0.0,
        from: root,
        to,
        facing_from: facing,
        facing_to: fc,
        follow_disp: false,
        lead: if found { durations[0] } else { 0.0 },
        table: None,
        root_time: found.then_some(durations[1]),
        end_free: ty == 2,
        end_wall: ty == 1,
        end_stand: false,
        hand_l: hl,
        hand_r: hr,
        normal: nn,
    }))
}

/// What the climb's sideways reach found: climb holds (type 0, the climb keeps the move) or a hang (types 1 / 2, the
/// Ledge context).
#[derive(Clone, Copy, Debug)]
pub enum ClimbReachTarget {
    Climb(super::climb::ClimbReach),
    Hang(LedgeMove),
}

/// SecondHandGrab (Ledge state 17): the loop items that end in a one-hand catch (`HumanLedge__CornerAnimFinished`
/// 0xDCD570). They are the long ("_3") jumps into a free hang: `climbing_climb1m_tr_hangfree_{left,right}_3_b`,
/// `climbing_hangwall_tr_hangfree_…_3_b`, `climbing_hangfree_tr_hangfree_…_3_b`. Even index = left jump.
pub const SECOND_HAND_LOOPS: [u32; 6] = [0x4912_2886, 0x4912_288E, 0x4912_28E2, 0x4912_28EA, 0x4C46_0376, 0x4C46_037E];

/// SecondHandGrab (0xDCD5E0 / 0xDDADE0 / 0xDD7310 / 0xDD14E0). When the jump's loop item X is playing and the root
/// interpolation has finished, the game plays X+1 (`_3_c`, the one-hand catch) in place, then `SecondHandGrab_Reach`:
/// the trailing hand (the right one after a left jump, +44) probes for the edge at root + 2.4 up ± 0.25·right (r 0.25,
/// 0.15 / 0.15, filter dword_193417C; on a miss it keeps its target), plays X+2 (`_3_d`) while the root is
/// interpolated to the two-hand free-hang pose (sub_B15AD0, sub_711130), then (0xDCF460) the free-hang wait 0x012719F1
/// with X+3 (`_3_e`) as its entry transition.
///
/// The table jump already ends with X+1. PORT: that part now ends in a one-hand hang under the catching hand, and the
/// returned move reaches with the second hand over X+2. X+3 is the idle's entry transition (not played by the port).
pub fn second_hand_grab(mut mv: LedgeMove, guidance: &GuidanceWorld, collision: &CollisionWorld) -> (LedgeMove, Option<LedgeMove>) {
    let Some(lp) = mv.seq[1].map(|a| a.id) else { return (mv, None) };
    let Some(k) = SECOND_HAND_LOOPS.iter().position(|x| *x == lp) else { return (mv, None) };
    let right_reaches = k % 2 == 0;
    let n = mv.normal;
    let r = right_of(-Vec3::new(n.x, 0.0, n.z).normalize_or_zero());
    let first = if right_reaches { mv.hand_l } else { mv.hand_r };
    let one_root = hang_root_at(first, first, n, LedgeHangType::Free, collision);
    let probe = first + r * if right_reaches { 0.25 } else { -0.25 };
    let second = guidance.on_edge(probe, n, 0.25).map(|h| h.point).unwrap_or(first);
    let (hl, hr) = if right_reaches { (first, second) } else { (second, first) };
    let to = hang_root_at(hl, hr, n, LedgeHangType::Free, collision);
    mv.to = one_root;
    let seq = [single(lp + 2, 0), None, None, None];
    let d = seq_durations(&seq);
    let tail = LedgeMove {
        seq,
        durations: if d[0] > 0.0 { d } else { [0.4, 0.0, 0.0, 0.0] },
        t: 0.0,
        from: one_root,
        to,
        facing_from: mv.facing_to,
        follow_disp: false,
        lead: 0.0,
        table: None,
        root_time: None,
        end_free: true,
        hand_l: hl,
        hand_r: hr,
        ..mv
    };
    mv.hand_l = first;
    mv.hand_r = first;
    (mv, Some(tail))
}

/// The blended swing back's weights over `swingback_up_{min,max}_{200,300}` (`StartLedgeJump` 0xDDCE40, blended):
/// [(1 − h)(1 − v), (1 − h)v, h(1 − v), hv]. v = clamp(the higher new hand − root − 2.0); h = the flat distance from the
/// current hands' midpoint `mid` to the new one projected onto the plane through `mid` whose normal is the new free
/// pose's right axis (`Vec_ProjectOntoPlane`, 0xDDD057), so the sideways offset does not count (RE/24 LEDGE-23).
pub fn swing_back_weights(hl: Vec3, hr: Vec3, nn: Vec3, mid: Vec3, root: Vec3) -> [f32; 4] {
    let point = (hl + hr) * 0.5;
    let v = (hl.y.max(hr.y) - root.y - 2.0).clamp(0.0, 1.0);
    let new_right = right_of(-Vec3::new(nn.x, 0.0, nn.z).normalize_or_zero());
    let d = Vec3::new(point.x - mid.x, 0.0, point.z - mid.z);
    let h = (d - new_right * d.dot(new_right)).length().clamp(0.0, 1.0);
    [(1.0 - h) * (1.0 - v), (1.0 - h) * v, h * (1.0 - v), h * v]
}

/// `HumanLedge__TryWallJumpUp` 0xDD62A0 (wall hang, no pending step, stick up): the swing back to the front edge of an
/// overhang behind, then the blended swing (0xDDCE40): weights over `swingback_up_{min,max}_{200,300}` by
/// v = clamp(targetZ − rootZ − 2.0) and h = clamp(horizontal distance of the hands).
///
/// The search (`sub_1171050`, mask 50, 45°, pieces shortened 0.1 m): a box out from the wall, whose edges must face
/// the same way as the wall hung from (the overhang's front), and the nearest edge to a reference for each hand:
/// 1. centre at the hands' x / z, root + 1.0 m up, 1.0 m out from the wall; half extents 0.375 / 0.3 / 0.5; hand
///    references (∓0.2, −0.5, −0.5) in the box frame (right, forward, up);
/// 2. else root + 2.25 m up, 0.75 m out; half extents 0.375 / 0.55 / 0.75; references (∓0.2, −0.5, −0.75).
///
/// Both hands must find an edge; the free-hang pose from them (sub_B15AD0) and its body boxes (sub_B2DD50) follow.
/// PORT: the body boxes are not tested.
pub fn try_swing_back(hand_l: Vec3, hand_r: Vec3, n: Vec3, root: Vec3, hang: LedgeHangType, guidance: &GuidanceWorld, _collision: &CollisionWorld) -> Option<LedgeMove> {
    if hang != LedgeHangType::Wall {
        return None;
    }
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mid = (hand_l + hand_r) * 0.5;
    let right = facing.cross(Vec3::Y);
    let zones = [(1.0f32, 1.0f32, Vec3::new(0.375, 0.3, 0.5), -0.5f32), (2.25, 0.75, Vec3::new(0.375, 0.55, 0.75), -0.75)];
    let found = zones.iter().find_map(|&(up, out, half, ref_up)| {
        let centre = Vec3::new(mid.x, root.y + up, mid.z) - facing * out;
        let local = |x: f32| centre + right * x - facing * 0.5 + Vec3::Y * ref_up;
        guidance.probe_facing_pair(centre, facing, half, [local(-0.2), local(0.2)], 0.785_398_2, crate::guidance::PROBE_STEP, MASK_CLIMB_HOLDS)
    });
    if let Some([a, b]) = found {
        let (hl, hr) = (a.point, b.point);
        let nn = a.wall_normal;
        let to = hang_root_at(hl, hr, nn, LedgeHangType::Free, _collision);
        let w = swing_back_weights(hl, hr, nn, mid, root);
        let a = jump_blend::action_items(SWING_BACK).map(|_| ActionBlend::new(SWING_BACK, 0, &w));
        let b = jump_blend::action_items(SWING_BACK_B).map(|_| ActionBlend::new(SWING_BACK_B, 0, &w));
        let (da, db) = (a.map(|a| a.duration()).unwrap_or(0.0), b.map(|b| b.duration()).unwrap_or(0.0));
        let (da, db) = if da + db > 0.0 { (da, db) } else { (0.0, JUMP_UP_TIME) };
        return Some(LedgeMove {
            kind: MoveKind::SwingBack,
            seq: [a, b, None, None],
            durations: [da, db, 0.0, 0.0],
            t: 0.0,
            from: root,
            to,
            facing_from: facing,
            facing_to: -Vec3::new(nn.x, 0.0, nn.z).normalize_or_zero(),
            // `_a` plays in place, `_b` moves the root with the interpolator (0xDDAB00 → sub_711130)
            follow_disp: false,
            lead: da,
            table: None,
            root_time: None,
            end_free: true,
            end_wall: false,
            end_stand: false,
            hand_l: hl,
            hand_r: hr,
            normal: nn,
        });
    }
    None
}

/// `HumanLedge__TryJumpUpToClimb` 0xDD5E10 (wall hang only, no pending hand step): a hold row 1.2 m above the
/// hands (the hands' midpoint at the higher hand) and a second row 1.2 m below that hit (sub_11713A0: half extents
/// 0.4 / 0.5 / 0.3, 45°, mask 50). Then the climb pose (sub_B1CC20), a body clearance sweep (r 0.35, from
/// root − 1.1 z and 0.65 m back, 1.8 m up, sub_B136E0) and the table ledge jump type 0, short, side up (wall
/// table entry 0: `0x491228AE` start, `…AF` loop, `…B0` end, landing category 0).
///
/// Landing (StateLedgeJump_Update 0xDDFC30 → sub_DCE1F0): once the end item plays, the context switches to Climb
/// (0xDD8CF0 → fill sub_DD8290: ClimbData EntryType 2 = FromLedgeParallelJump, the root interpolated to the
/// climb pose over the end item's blended duration). Returns the start + loop part (in the Ledge context) and
/// the climb entry carrying the end action.
///
/// PORT: both hands go to the upper hit and both feet to the lower one (as the side moves into the climb). The root
/// follows the start and loop clips' displacement plus a
/// linear correction onto the point the end item starts from (hypothesis, as for the other table jumps). The
/// clearance sweep is the capsule test at the climb root (stand-in).
#[allow(clippy::too_many_arguments)]
pub fn try_jump_up_to_climb(
    hand_l: Vec3,
    hand_r: Vec3,
    n: Vec3,
    root: Vec3,
    hang: LedgeHangType,
    guidance: &GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<(LedgeMove, super::climb::ClimbEntry)> {
    if hang != LedgeHangType::Wall {
        return None;
    }
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mut base = (hand_l + hand_r) * 0.5;
    base.y = hand_l.y.max(hand_r.y);
    // the two rows (0xDD5E10, called with 0 = up by 0xDE05E0): boxes 1.2 m above the hands and 1.2 m below that hit,
    // half extents 0.4 / 0.5 / 0.3, the reference 0.2 m back, 45°, mask 50. The lower row is usually the edge hung
    // from: the feet go where the hands were.
    let upper = guidance.probe_facing(base + Vec3::Y * JUMP_TO_CLIMB_UP, facing, JUMP_TO_CLIMB_HALF, 0.2, 0.785_398_2, MASK_CLIMB_HOLDS)?;
    let lower = guidance.probe_facing(upper.point - Vec3::Y * JUMP_TO_CLIMB_UP, facing, JUMP_TO_CLIMB_HALF, 0.2, 0.785_398_2, MASK_CLIMB_HOLDS)?;
    let nn = upper.wall_normal;
    let target = super::climb::climb_root_at(upper.point, upper.point, lower.point, lower.point, nn);
    // the clearance (`Human__CapsuleCastFree` 0xDD625F): a capsule r 0.35 from P = the hands' midpoint (the higher
    // hand's height) + 0.65·outward − 1.1 m, 1.8 m tall, swept by the move to the climb pose root (RE/24 LEDGE-28)
    let p = base + Vec3::new(n.x, 0.0, n.z).normalize_or_zero() * 0.65 - Vec3::Y * 1.1;
    if !collision.capsule_cast_free(p, 1.8, 0.35, target - root) {
        return None;
    }
    let entry = LEDGE_JUMP_TABLE[0][0];
    let [start, lp, end] = entry.1;
    // category 0 (RE/24 LEDGE-14): the start plays in the Ledge context, its own displacement moving the root
    // (`StartLedgeJump` 0xDDCE40 sets no interpolator); `FillClimb_AfterLedgeJump` 0xDD8290 then plays the loop in the
    // Climb context, interpolating the root linearly to the climb pose (the climb entry's action); the end (ClimbData+108,
    // `end_action`) plays in the climb's Wait state when that ends (+2916, `PlayReachEnd` 0xDE9B50; RE/24 CLIMB-24).
    let seq = [single(start, 0), None, None, None];
    let durations = seq_durations(&seq);
    let found = durations.iter().sum::<f32>() > 0.0;
    let _ = target;
    let mut mv = LedgeMove {
        kind: MoveKind::JumpUpToClimb,
        seq,
        durations: if found { durations } else { [SIDE_JUMP_FALLBACK_TIME * 0.5, 0.0, 0.0, 0.0] },
        t: 0.0,
        from: root,
        to: root,
        facing_from: facing,
        facing_to: -Vec3::new(nn.x, 0.0, nn.z).normalize_or_zero(),
        follow_disp: false,
        lead: 0.0,
        table: Some((1, 0)),
        root_time: None,
        end_free: false,
        end_wall: false,
        end_stand: false,
        hand_l: upper.point,
        hand_r: upper.point,
        normal: nn,
    };
    mv.to = mv.natural_end();
    let climb = super::climb::ClimbEntry {
        entry_type: super::climb::ClimbEntryType::FromLedgeParallelJump,
        pose_2m: None, end_action: single(end, 0).map(|_| end),
        hand_l: upper.point,
        hand_r: upper.point,
        foot_l: lower.point,
        foot_r: lower.point,
        normal: nn,
        from_feet: mv.to,
        foot_right: false,
        action: single(lp, 0).map(|_| lp),
    };
    Some((mv, climb))
}

/// `HumanLedge__TryFreeHangDropToClimb` 0xDDF390 (free hang, stick down, no pending step). From the hands' midpoint
/// (higher hand) a hold row 1.2 m below and 0.65 m ahead (sub_11713A0: half extents 0.3 / 0.65 / 0.6, 45°, mask
/// 50), then a second row 1.2 m below that and 0.3 m further ahead (0.3 / 0.65 / 0.3):
/// - both rows → climb pose (sub_B1CC20), clearance sub_B2DF90, action `FREE_DROP_TO_CLIMB`;
/// - no second row but wall supports for the feet (sub_B16130) → wall-hang pose (sub_B157B0), clearance sub_B2DC80,
///   action `FREE_DROP_TO_HANG`.
///
/// Weights over the four items {min,max}_{200,300}: v = clamp(handsZ − (2.0 climb | 1.6 hang) − min foot Z),
/// h = clamp(horizontal distance from hands + 0.3·forward to the new hand line / 0.7); w = [(1−v)(1−h), v(1−h),
/// h(1−v), hv]. The first action plays in place (state 14); when it ends the second plays with the same weights
/// while the root is interpolated to the pose over its length (0xDDA120 → sub_711130); then Climb (0xDE0870 →
/// 0xDDFBF0, when the second action is the climb one) or the wall hang (0xDDA3D0).
///
/// PORT: hands both on the upper hit (the climb) or spread by HAND_SPACING (the hang); clearance = capsule test at
/// the target.
pub fn try_free_hang_drop(
    hand_l: Vec3,
    hand_r: Vec3,
    n: Vec3,
    root: Vec3,
    hang: LedgeHangType,
    guidance: &GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<(LedgeMove, Option<super::climb::ClimbEntry>)> {
    use super::climb::{ClimbEntry, ClimbEntryType, FREE_DROP_TO_CLIMB, FREE_DROP_TO_HANG};
    if hang != LedgeHangType::Free {
        return None;
    }
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mut base = (hand_l + hand_r) * 0.5;
    base.y = hand_l.y.max(hand_r.y);
    // the rows (0xDDF390): boxes 1.2 m below the hands and 0.65 m ahead (half extents 0.3 / 0.65 / 0.6), then 1.2 m
    // below that hit and 0.3 m further ahead (0.3 / 0.65 / 0.3); the reference 0.3 m back, 45°, mask 50. The edge
    // hung from lies above the first box.
    let upper = guidance.probe_facing(base - Vec3::Y * 1.2 + facing * 0.65, facing, Vec3::new(0.3, 0.65, 0.6), 0.3, 0.785_398_2, MASK_CLIMB_HOLDS)?;
    let nn = upper.wall_normal;
    let lower = guidance.probe_facing(upper.point - Vec3::Y * 1.2 + facing * 0.3, facing, Vec3::new(0.3, 0.65, 0.3), 0.3, 0.785_398_2, MASK_CLIMB_HOLDS);
    let new_facing = -Vec3::new(nn.x, 0.0, nn.z).normalize_or_zero();
    let (ids, to, hl, hr, feet_z, climb) = if let Some(lower) = lower {
        let to = super::climb::climb_root_at(upper.point, upper.point, lower.point, lower.point, nn);
        let e = ClimbEntry {
            entry_type: ClimbEntryType::FromLedge,
            pose_2m: None, end_action: None,
            hand_l: upper.point,
            hand_r: upper.point,
            foot_l: lower.point,
            foot_r: lower.point,
            normal: nn,
            from_feet: to,
            foot_right: false,
            action: None,
        };
        (FREE_DROP_TO_CLIMB, to, upper.point, upper.point, lower.point.y, Some(e))
    } else {
        // wall supports for the feet (sub_B16130 under the hold)
        if !super::ledge::feet_supports(upper.point, nn, collision) {
            return None;
        }
        let r = right_of(new_facing);
        let (hl, hr) = (upper.point - r * HAND_SPACING * 0.5, upper.point + r * HAND_SPACING * 0.5);
        let to = hang_root_at(hl, hr, nn, LedgeHangType::Wall, collision);
        // the wall-hang feet (sub_B16130) sit about 1.1 m below the hands (the wall-hang root offset)
        (FREE_DROP_TO_HANG, to, hl, hr, to.y, None)
    };
    if !collision.capsule_fits(to + Vec3::Y * 0.05 + nn * 0.1) {
        return None;
    }
    let v = (base.y - if climb.is_some() { 2.0 } else { 1.6 } - feet_z).clamp(0.0, 1.0);
    let p = base + facing * 0.3;
    let d = Vec2::new(p.x - upper.point.x, p.z - upper.point.z);
    let along = right_of(new_facing);
    let h = ((d - Vec2::new(along.x, along.z) * d.dot(Vec2::new(along.x, along.z))).length() / 0.7).clamp(0.0, 1.0);
    let w = [(1.0 - v) * (1.0 - h), v * (1.0 - h), h * (1.0 - v), h * v];
    let a = jump_blend::action_items(ids[0]).map(|_| ActionBlend::new(ids[0], 0, &w));
    let b = jump_blend::action_items(ids[1]).map(|_| ActionBlend::new(ids[1], 0, &w));
    let (da, db) = (a.map(|a| a.duration()).unwrap_or(0.0), b.map(|b| b.duration()).unwrap_or(0.0));
    let (da, db) = if da + db > 0.0 { (da, db) } else { (0.0, SIDE_JUMP_FALLBACK_TIME) };
    let mv = LedgeMove {
        kind: MoveKind::FreeHangDrop { to_climb: climb.is_some() },
        seq: [a, b, None, None],
        durations: [da, db, 0.0, 0.0],
        t: 0.0,
        from: root,
        to,
        facing_from: facing,
        facing_to: new_facing,
        follow_disp: false,
        lead: da,
        table: None,
        root_time: None,
        end_free: false,
        end_wall: true,
        end_stand: false,
        hand_l: hl,
        hand_r: hr,
        normal: nn,
    };
    Some((mv, climb))
}

/// `HumanLedge__TrySideJumpToLadder` 0xDD55F0 (RE/18 §7.1), tried before the ledge side jump when the shimmy is blocked
/// or the edge ends (`HumanLedge__TryJumpUpOrTurnCorner` 0xDE05E0). The probe point is the root + 1.7 m to the side,
/// then −0.5 up + 0.35 forward (wall hang) or +0.5 up − 0.15 forward (free hang). A ladder piece is searched in the
/// box ±0.8 sideways, ±0.5 forward, ±0.5 up there (`Guidance__ProbeBox`, mask 8, any angle, step 0.1); the ladder
/// must lie along the wall (|n·forward| > 0.7071). Hands ±0.15 m across the ladder, 1 m up from the hit point.
/// A capsule (r 0.35, 1.8 m) from the root 0.15 m back from the wall must sweep to the ladder point 0.5 m out
/// (`Human__CapsuleCastFree` 0xB136E0). Ledge-jump table type 3 (ladder), weight = flat distance to the hand / 1.6.
/// The jump's end action (`…_ladder_*` 0x52557360..63) hands over to the Ladder context.
pub fn try_side_jump_to_ladder(
    right: bool,
    n: Vec3,
    root: Vec3,
    hang: LedgeHangType,
    guidance: &GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<(LedgeMove, super::ladder::LadderEntry)> {
    use super::ladder::LadderEntry;
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let side = if right { right_of(facing) } else { -right_of(facing) };
    let free = hang != LedgeHangType::Wall;
    let centre = root + side * 1.7 + if free { Vec3::Y * 0.5 - facing * 0.15 } else { -Vec3::Y * 0.5 + facing * 0.35 };
    let hit = guidance.probe_box(centre, right_of(facing), facing, Vec3::Y, Vec3::new(0.8, 0.5, 0.5), centre, std::f32::consts::PI, 0.1, crate::guidance::MASK_LADDER)?;
    let e = &guidance.edges[hit.edge];
    let ln = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
    if ln.dot(facing).abs() <= std::f32::consts::FRAC_1_SQRT_2 {
        return None;
    }
    let (base, top) = if e.p0.y <= e.p1.y { (e.p0, e.p1) } else { (e.p1, e.p0) };
    // the body's point on the ladder: 0.5 m out from the hit (the hands 1 m above it)
    let q = Vec3::new(hit.point.x, hit.point.y, hit.point.z);
    let to = q + ln * super::ladder::ATTACH_OUT;
    let back = root - facing * 0.15;
    if !collision.capsule_cast_free(back, 1.8, 0.35, to - root) {
        return None;
    }
    let across = right_of(-ln);
    let hand = q + Vec3::Y;
    let w = (Vec2::new(hand.x - root.x, hand.z - root.z).length() / 1.6).clamp(0.0, 1.0);
    let long = w >= 0.5;
    let entry = LEDGE_JUMP_TABLE[free as usize][(3 * 2 + long as usize) * 4 + 2 + right as usize];
    let seq = [single(entry.1[0], 0), single(entry.1[1], 0), single(entry.1[2], 0), None];
    let durations = seq_durations(&seq);
    let found = durations.iter().sum::<f32>() > 0.0;
    let mv = LedgeMove {
        kind: MoveKind::SideJump { long },
        seq,
        durations: if found { durations } else { [SIDE_JUMP_FALLBACK_TIME, 0.0, 0.0, 0.0] },
        t: 0.0,
        from: root,
        to,
        facing_from: facing,
        facing_to: -ln,
        follow_disp: false,
        lead: 0.0,
        table: None,
        root_time: None,
        end_free: false,
        end_wall: false,
        end_stand: false,
        hand_l: hand - across * 0.15,
        hand_r: hand + across * 0.15,
        normal: ln,
    };
    let height = (to.y - base.y).clamp(0.0, (top.y - base.y).max(0.0));
    let ladder = LadderEntry { base, top, n: ln, from: to, facing: -ln, from_top: false, high: false, foot: right as usize, from_ledge: true, height: Some(height), action: None, ..Default::default() };
    Some((mv, ladder))
}

/// `HumanLedge__TrySideMoveToLadder` 0xDD4CD0 (stick left/right, tried first after ProbeLateral). A ladder edge
/// (filter dword_1934114, r 0.6, 0.5 / 0.5, any angle) near the probe point root + 0.5 up − 0.15 forward (free hang)
/// or root − 0.5 up + 0.35 forward (wall hang), ± 0.6 m along the character's right; the ladder must face the same
/// way within 45° (|dot| > cos 45°). Clearance: the lateral capsule sweep sub_B2A410 (1.8 m). Action `FROM_LEDGE_SIDE`;
/// the root is interpolated over it to the ladder point − 0.5 m into the wall (sub_711130), the four limbs snap to
/// rungs (sub_B18600), and flag 1860|0x10 switches to the Ladder context when the move ends.
///
/// PORT: the probe is the horizontal distance to the ladder line (≤ 0.6 m) with the probe height inside the ladder
/// ± 0.5 m; no clearance sweep; the rung snap is left to the ladder wait.
pub fn try_side_to_ladder(
    right: bool,
    n: Vec3,
    root: Vec3,
    hang: LedgeHangType,
    guidance: &GuidanceWorld,
) -> Option<(LedgeMove, super::ladder::LadderEntry)> {
    use super::ladder::{LadderEntry, FROM_LEDGE_SIDE};
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let side = if right { right_of(facing) } else { -right_of(facing) };
    let free = hang != LedgeHangType::Wall;
    let p = if free { root + Vec3::Y * 0.5 - facing * 0.15 } else { root - Vec3::Y * 0.5 + facing * 0.35 } + side * 0.6;
    for e in &guidance.edges {
        if e.subtype != crate::guidance::GuidanceSubType::Ladder {
            continue;
        }
        let (base, top) = if e.p0.y <= e.p1.y { (e.p0, e.p1) } else { (e.p1, e.p0) };
        let ln = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
        if p.y < base.y - 0.5 || p.y > top.y + 0.5 || ln.dot(-facing).abs() <= std::f32::consts::FRAC_1_SQRT_2 {
            continue;
        }
        let q = Vec3::new(base.x, p.y.clamp(base.y, top.y), base.z);
        if Vec2::new(p.x - q.x, p.z - q.z).length() > 0.6 {
            continue;
        }
        let to = q + ln * super::ladder::ATTACH_OUT;
        let id = FROM_LEDGE_SIDE[free as usize][right as usize];
        let seq = [single(id, 0), None, None, None];
        let durations = seq_durations(&seq);
        let durations = if durations[0] > 0.0 { durations } else { [CORNER_FALLBACK_TIME, 0.0, 0.0, 0.0] };
        // the clip ends in `…_ladder_wait_{l,r}`: wall left / free right end on the right foot
        let foot = (free == right) as usize;
        let mv = LedgeMove {
            kind: MoveKind::SideToLadder,
            seq,
            durations,
            t: 0.0,
            from: root,
            to,
            facing_from: facing,
            facing_to: -ln,
            follow_disp: false,
            lead: 0.0,
            table: None,
            root_time: None,
            end_free: false,
            end_wall: false,
            end_stand: false,
            hand_l: q,
            hand_r: q,
            normal: ln,
        };
        let entry = LadderEntry { base, top, n: ln, from: to, facing: -ln, from_top: false, high: false, foot, from_ledge: true, action: None, ..Default::default() };
        return Some((mv, entry));
    }
    None
}

/// `HumanClimb__TryReachLedgeAbove` 0xDF1730: climbing out over the top. [1m pose 0, 2m pose 3]:
/// `xx_l_climb_{1m,2m}_tr_hangknee_footl_{a,b}` (56562095 + the pose record's "apart" flag, table 0x1A2CD78).
pub const CLIMB_OUT: [u32; 2] = [0x035F_11AF, 0x035F_11B0];

/// The climb-out (0xDF1730): the hangknee action plays while the root is interpolated onto the edge 0.1 m in
/// (sub_711130 over the action); the Ledge context starts in SubState 4 with a wall hang and `Pullup_Tick` 0xDE2EE0
/// keeps the interpolation running. The action is not in its list of follow-ups, so when it ends the default stand-up
/// applies (flag 0x40 → NarrowObject with `hangknee_tr_freestep_entry`, PORT: Ground after `ACT_KNEE_TO_FREESTEP`).
/// Returns the knee move and the free step onto the top.
/// `Human__KneeCatchRoomOnTop` 0xB2E240 (the climb out, 0xDF1730, and the knee catch, 0xE0D58B; RE/24 CLIMB-5): room
/// above an edge `target` for the kneeling body, `into` pointing over the top. No floor is required:
/// 1. a box 0.35–1.8 m above the edge from 1.0 m out to 0.3 m in (centre target − 0.35·f + 1.075 up, half 0.3 / 0.65 /
///    0.725) is empty (its first contact from the near face lies 1.3 m or more in, `sub_B2A070`);
/// 2. a thin column 0.1–1.8 m straight above the edge (half 0.1 / 0.1 / 0.85) is empty;
/// 3. a thin box tilted 30° about f × up (`sub_55E390` −0.5236 on both f and up), centred 0.4 m out from the edge along
///    the tilted f (target − 0.4·f′), half 0.1 / 0.1 / 0.5 along (across, f′, up′), is empty (`Human__BoxQueryEmpty`).
pub fn knee_room_on_top(target: Vec3, into: Vec3, collision: &CollisionWorld) -> bool {
    let f = Vec3::new(into.x, 0.0, into.z).normalize_or_zero();
    if f == Vec3::ZERO {
        return false;
    }
    let across = f.cross(Vec3::Y).normalize_or_zero();
    let axes = [across, f, Vec3::Y];
    if !collision.obb_free(target - f * 0.35 + Vec3::Y * 1.075, axes, Vec3::new(0.3, 0.65, 0.725)) {
        return false;
    }
    if !collision.obb_free(target + Vec3::Y * 0.95, axes, Vec3::new(0.1, 0.1, 0.85)) {
        return false;
    }
    let tilt = Quat::from_axis_angle(across, -0.523_598_8);
    let (tf, tu) = (tilt * f, tilt * Vec3::Y);
    collision.obb_free(target - tf * 0.4, [across, tf, tu], Vec3::new(0.1, 0.1, 0.5))
}

pub fn climb_out_moves(two_m: bool, from: Vec3, hand_l: Vec3, hand_r: Vec3, n: Vec3) -> (LedgeMove, LedgeMove) {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mid = (hand_l + hand_r) * 0.5;
    let id = CLIMB_OUT[two_m as usize];
    let seq = [single(id, 0), single(id, 1), None, None];
    let durations = seq_durations(&seq);
    let durations = if durations.iter().sum::<f32>() > 0.0 { durations } else { [PULLUP_WALL_TIME * 0.5, 0.0, 0.0, 0.0] };
    let knee = mid + facing * 0.1;
    let found = seq.iter().any(|a| a.is_some());
    let base = LedgeMove {
        kind: MoveKind::Pullup,
        seq,
        durations,
        t: 0.0,
        from,
        to: knee,
        facing_from: facing,
        facing_to: facing,
        // target + 0.1·into, `sub_711130` with the action as its 11th argument: the hangknee clip's own root displacement
        // plus a linear correction (0xDF1CBB → RootInterp+200, RE/24 CLIMB-5)
        follow_disp: found,
        lead: 0.0,
        table: None,
        root_time: None,
        end_free: false,
        end_wall: false,
        end_stand: false,
        hand_l,
        hand_r,
        normal: n,
    };
    let step = [single(ACT_KNEE_TO_FREESTEP[0], 0), None, None, None];
    let d = seq_durations(&step);
    let stand = LedgeMove {
        seq: step,
        durations: if d[0] > 0.0 { d } else { [0.4, 0.0, 0.0, 0.0] },
        from: knee,
        to: mid - n * PULLUP_IN,
        end_stand: true,
        ..base
    };
    (base, stand)
}

/// The knee catch from a fall (`CheckAirCatch` type 0, RE/19 §2.4): the catch action plays while the root is
/// interpolated onto the edge (sub_711130 over the action), then the stand-up onto the top as the climb-out's
/// (`ACT_KNEE_TO_FREESTEP`, PORT: Ground). Returns the catch move and the step onto the top.
pub fn knee_catch_moves(action: u32, from: Vec3, edge: Vec3, n: Vec3) -> (LedgeMove, LedgeMove) {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let seq = [single(action, 0), None, None, None];
    let durations = seq_durations(&seq);
    let durations = if durations.iter().sum::<f32>() > 0.0 { durations } else { [0.2, 0.0, 0.0, 0.0] };
    let r = right_of(facing);
    let (hand_l, hand_r) = (edge - r * HAND_SPACING * 0.5, edge + r * HAND_SPACING * 0.5);
    let knee = edge + facing * 0.1;
    let base = LedgeMove {
        kind: MoveKind::Pullup,
        seq,
        durations,
        t: 0.0,
        from,
        to: knee,
        facing_from: facing,
        facing_to: facing,
        follow_disp: false,
        lead: 0.0,
        table: None,
        root_time: None,
        end_free: false,
        end_wall: false,
        end_stand: false,
        hand_l,
        hand_r,
        normal: n,
    };
    let step = [single(ACT_KNEE_TO_FREESTEP[0], 0), None, None, None];
    let d = seq_durations(&step);
    let stand = LedgeMove {
        seq: step,
        durations: if d[0] > 0.0 { d } else { [0.4, 0.0, 0.0, 0.0] },
        from: knee,
        to: edge - n * PULLUP_IN,
        end_stand: true,
        ..base
    };
    (base, stand)
}

// ---------------------------------------------------------------- jumps into a hang

/// Pull-up chain pieces: hangwaist → hangknee (`xx_h_hangwaist_tr_hangknee_footl`), hangknee → wait.
pub const ACT_WAIST_TO_KNEE: u32 = 0x0127_199E;
/// hangknee → wait (`xx_h_hangknee_footl_tr_h_wait_footr_{a,b}`): only an exit listed in the graph; the game's code
/// ends the stand-up through `ACT_KNEE_TO_FREESTEP` instead.
#[allow(dead_code)]
pub const ACT_KNEE_TO_WAIT: u32 = 0x0106_C58B;
/// The stand-up's end (`HumanLedge__Pullup_Tick` 0xDE2EE0: at the hangknee action's release, flag 0x40 →
/// `SwitchToNarrowObjectContext` 0xDD2A50): `xx_h_hangknee_foot{l,r}_tr_freestep_entry_foot{l,r}`, by the playing
/// item's leading foot (+60 & 0xC == 4 → left), then NarrowObject SubState 6 (the transient free-step stay, back to
/// Ground on wide support). [footl, footr].
pub const ACT_KNEE_TO_FREESTEP: [u32; 2] = [0x248E_9730, 0x248E_9731];
/// Running jump onto a wall-hang ledge (target type 0x40): `air_surface_tr_hangwall_reception_{straight,
/// 30_out,45_in}_{min,max}` then `hangwall_reception_*_{a,b}` (3 items × 6 clips; 0xE07D00 → 0xE02BA0).
pub const RECEPTION_SURFACE_WALL: u32 = 0x011F_F16A;
/// Running jump onto a free-hang ledge (type 0x80) without foot holds: the swing cycle (0xE07D00, Ledge SubState 8).
pub const SWING_RECEPTION: u32 = 0x023E_0C60;
/// The wall reception of any other jump type with foot holds (0xE07D00 → 0xE02BA0, timed 0.1666 s; the free-hang jump
/// 0x80 landing on a wall): `hangwall_reception_*_{a,b}` (2 items × 6 clips, then `0x8E5444F0` (c) by the graph).
pub const RECEPTION_WALL_OTHER: u32 = 0x8E54_44EE;

/// The bands of `Human__SetupJumpToHandTarget` 0xB21DA0 (standing straight jump at a hand target) by the
/// hand height above the feet `dz`: flight action, its 2-clip blend weight, the root offset from the hand
/// point (out along the wall normal, down), target flags, the arrival reception and how it ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HangJumpIn {
    pub flight: u32,
    pub b: f32,
    pub out: f32,
    pub down: f32,
    pub flags: u32,
    pub reception: u32,
    pub end: HangEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HangEnd {
    /// Hang (wall / free / wall-free) after the reception.
    Hang(LedgeHangType),
    /// Knee height: the reception then hangknee → wait (Ledge SubState 4, pull-up).
    StandFromKnee,
    /// Waist height: the reception, hangwaist → hangknee, hangknee → wait.
    StandFromWaist,
    /// Below 0.7 m: a free-step target (flags 1). The reception `collide_full_*_to_freestep_*_tr_freestep_entry`
    /// steps onto the top (0xE07D00 → NarrowObject SubState 6, the transient free-step stay that returns to Ground).
    FreeStep,
}

/// The < 0.7 m band's reception (0xE09752: played when the arriving flight is 0x012B291B).
pub const RECEPTION_STEP_UP: u32 = 0x012B_33F1;

/// 0xB21DA0 bands (ground variant: the playing action is the straight-jump impulse; the `beam_*` variants are
/// `hang_jump_in_beam`). `wall` = the target's sub-type is 8 (hypothesis: a wall below the edge, i.e. a wall hang).
pub fn hang_jump_in(dz: f32, wall: bool) -> Option<HangJumpIn> {
    let c = |x: f32| x.clamp(0.0, 1.0);
    Some(if dz < 0.7 {
        // `collide_full_*_{050,070}cm_to_freestep_*`: b = (dz − 0.5) / 0.2, root +0.5 n, flags 1 (0xB21FCD)
        HangJumpIn { flight: 0x012B_291B, b: c((dz - 0.5) / 0.2), out: 0.5, down: 0.0, flags: 1, reception: RECEPTION_STEP_UP, end: HangEnd::FreeStep }
    } else if dz < 1.5 {
        HangJumpIn { flight: 0x0129_0ECF, b: c((dz - 0.7) / 0.8), out: 0.5, down: 0.0, flags: 4, reception: 0x0129_0ED0, end: HangEnd::StandFromKnee }
    } else if dz < 2.0 {
        HangJumpIn { flight: 0x0127_2A69, b: c((dz - 1.5) * 2.0), out: 0.5, down: 0.0, flags: 4, reception: 0x0127_2A6A, end: HangEnd::StandFromKnee }
    } else if dz < 2.5 {
        if wall {
            HangJumpIn { flight: 0x0127_1631, b: c((dz - 2.0) * 2.0), out: 0.5, down: 1.1, flags: 0x40, reception: 0x0127_1632, end: HangEnd::Hang(LedgeHangType::Wall) }
        } else {
            HangJumpIn { flight: 0x0127_1639, b: c((dz - 2.0) * 2.0), out: 0.5, down: 1.0, flags: 8, reception: 0x0127_163A, end: HangEnd::StandFromWaist }
        }
    } else if wall {
        HangJumpIn { flight: 0x0121_A8B1, b: c((dz - 2.5) * 2.0), out: 0.5, down: 2.4, flags: 0x80, reception: 0x0121_B072, end: HangEnd::Hang(LedgeHangType::Free) }
    } else {
        HangJumpIn { flight: 0x0121_A598, b: c((dz - 2.5) * 2.0), out: 0.0, down: 2.4, flags: 0x80, reception: 0x0127_23A5, end: HangEnd::Hang(LedgeHangType::Free) }
    })
}

/// 0xB21DA0 when the playing action is not the ground's straight-jump impulse (89 / 0x1099C96), e.g. from the
/// beam's impulsion (`HumanNarrowObjectBeam` 0xF73B80): the ≥ 1.5 m bands fly the `beam_jumpstraight_*`
/// flights 0x516D52DB…DF (same weights, offsets, flags; the receptions stay the ground ones, 0xE07D00).
pub fn hang_jump_in_beam(dz: f32, wall: bool) -> Option<HangJumpIn> {
    let mut j = hang_jump_in(dz, wall)?;
    j.flight = match j.flight {
        0x0127_2A69 => 0x516D_52DF, // → hangknee 150/200
        0x0127_1631 => 0x516D_52DD, // → hangwall 200/250
        0x0127_1639 => 0x516D_52DE, // → hangwaist 200/250
        0x0121_A8B1 => 0x516D_52DC, // → hangwallfree 250/300
        0x0121_A598 => 0x516D_52DB, // → hangfree 250/300
        f => f,                     // < 1.5 m: no beam variant
    };
    Some(j)
}

/// The free-hang jump (0x80) that finds foot holds (0xE07D00 → `HumanInAir__PlayLedgeReception` 0xE02BA0 with
/// `RECEPTION_WALL_OTHER` instead of `RECEPTION_SURFACE_WALL`): the wall reception's actions swapped for it, same clip
/// weights (`wall_reception_weights`). The action plays with a 0.1666 s blend and the root goes linearly onto the hang
/// in those 0.1666 s (0xE0949A -> 0xE02BA0 a12; bug-1791711526).
pub fn use_free_jump_wall_reception(m: &mut LedgeMove, w: [f32; 6]) {
    let item = |i: usize| jump_blend::action_items(RECEPTION_WALL_OTHER).filter(|it| it.len() > i).map(|_| ActionBlend::new(RECEPTION_WALL_OTHER, i, &w));
    let seq = [item(0), item(1), None, None];
    let durations = seq_durations(&seq);
    if durations.iter().sum::<f32>() > 0.0 {
        m.seq = seq;
        m.durations = durations;
    }
    m.lead = 0.0;
    m.root_time = Some(0.1666);
}

/// How a jump at a ledge is received when it arrives (CheckJumpTargetArrival 0xE07D00).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LedgeArrival {
    /// From the standing straight jump (0xB21DA0).
    Straight(HangJumpIn),
    /// From a running jump (0xB20200) onto a ledge: the wall reception with foot holds, else the swing (type 0x80).
    Surface { free: bool },
}

/// The Ledge context's entry move for an arrival: the reception actions while the root goes from `from` to
/// the hang root (or onto the top for knee / waist heights), following the clips' displacement plus a
/// correction (receptions are FROMANIM; the game interpolates the root over the action, 0xE07D00 →
/// sub_711130).
/// `sub_E02790` (from 0xE094D1, RE/24 AIR-8): the wall reception's six weights. The max set (slots 3–5) when the fall
/// height is above 2.5 m or the jump started more than 6 m from the hold (squared distance > 36), else the min set
/// (0–2); within the set the pose's lean a (its up about the lateral axis, from `sub_B157B0`): a ≥ 0 → [1 − w, w, 0],
/// w = min(a / 30°, 1); a < 0 → [1 − w, 0, w], w = min(−a / 45°, 1). PORT: the port's wall-hang pose is upright (a = 0).
pub fn wall_reception_weights(fall_height: f32, origin: Vec3, hold: Vec3, lean: f32) -> [f32; 6] {
    let k = if fall_height > 2.5 || origin.distance_squared(hold) > 36.0 { 3 } else { 0 };
    let mut w = [0.0f32; 6];
    if lean >= 0.0 {
        let t = (lean / 30f32.to_radians()).min(1.0);
        w[k] = 1.0 - t;
        w[k + 1] = t;
    } else {
        let t = (-lean / 45f32.to_radians()).min(1.0);
        w[k] = 1.0 - t;
        w[k + 2] = t;
    }
    w
}

pub fn arrival_move(arr: LedgeArrival, from: Vec3, hand_l: Vec3, hand_r: Vec3, n: Vec3, reception_w: [f32; 6], collision: &CollisionWorld) -> LedgeMove {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mid = (hand_l + hand_r) * 0.5;
    let top = Vec3::new(mid.x, mid.y, mid.z) - n * PULLUP_IN;
    let item = |id: u32, i: usize, w: &[f32]| jump_blend::action_items(id).filter(|it| it.len() > i).map(|_| ActionBlend::new(id, i, w));
    let (seq, to, end_free, end_stand) = match arr {
        LedgeArrival::Straight(j) => {
            let w = [1.0 - j.b, j.b];
            match j.end {
                HangEnd::Hang(h) => {
                    // the hang receptions start no interpolation (0xE08C9A) and their clips are in place (0x01271632,
                    // 0x0121B072, 0x012723A5: no displacement): the root stays where the flight ended, the target root
                    // (hand + out·n − down, `Human__SetupJumpToHandTarget` 0xB21DA0); the Ledge's Movement state never
                    // moves it (`StateMovement_Update` 0xDE38F0)
                    ([item(j.reception, 0, &w), item(j.reception, 1, &w), None, None], from, h == LedgeHangType::Free, false)
                }
                HangEnd::StandFromKnee => ([item(j.reception, 0, &w), item(j.reception, 1, &w), item(ACT_KNEE_TO_FREESTEP[0], 0, &[1.0]), None], top, false, true),
                // PORT: the reception's root (FROMANIM) is corrected onto the pull-up's stand point; where the game's
                // free-step stay leaves the root is not traced. Its exit transitions (low / high wait 0x69C24BB2 /
                // 0x0D9971F9, jog 0x0D9971FB) are chosen by the animation graph; the port hands back to Ground.
                HangEnd::FreeStep => ([item(j.reception, 0, &w), None, None, None], top, false, true),
                HangEnd::StandFromWaist => (
                    [item(j.reception, 0, &w), item(j.reception, 1, &w), item(ACT_WAIST_TO_KNEE, 0, &[1.0]), item(ACT_KNEE_TO_FREESTEP[0], 0, &[1.0])],
                    top,
                    false,
                    true,
                ),
            }
        }
        LedgeArrival::Surface { free: false } => {
            // 0xE02790 weights the 6 clips by min / max and the pose's lean (`wall_reception_weights`)
            let w = reception_w;
            let to = hang_root(hand_l, hand_r, n, LedgeHangType::Wall);
            ([item(RECEPTION_SURFACE_WALL, 0, &w), item(RECEPTION_SURFACE_WALL, 1, &w), item(RECEPTION_SURFACE_WALL, 2, &w), None], to, false, false)
        }
        LedgeArrival::Surface { free: true } => {
            // the swing: front up, front down (PORT: one swing, no SwingStrength decay)
            let to = hang_root_at(hand_l, hand_r, n, LedgeHangType::Free, collision);
            ([item(SWING_RECEPTION, 0, &[1.0]), item(SWING_RECEPTION, 1, &[1.0]), None, None], to, true, false)
        }
    };
    let durations = seq_durations(&seq);
    let found = durations.iter().sum::<f32>() > 0.0;
    // the running jump's receptions move the root linearly over the first item (0xE02BA0 with a12 = -1: item 0's weighted
    // clip length; the swing: `sub_501C90`(0), 0xE09623), not along the clips over the whole action (bug-1791711526).
    // The straight jump's receptions (0xE08E43–0xE08F31): the stand arrivals (0x01290ED0, 0x01272A6A, 0x0127163A; LedgeData
    // +76 = 4) interpolate the root in anim mode over the playing item (`sub_501C90`(0)) onto the point 0.1 m along the
    // arrival vector (the waist one 0.5 m out, 1 m down) and hold it (0xE08DD7); the hang receptions (0x0121B072,
    // 0x01271632, 0x012723A5; +76 = 1) start no interpolation (0xE08C9A) and are in place, so the root holds (`to` =
    // `from` above). PORT: the stand arrivals keep the clips over the whole sequence plus a linear correction onto `to`.
    let root_time = match arr {
        LedgeArrival::Surface { .. } if found => Some(durations[0].max(1e-3)),
        _ => None,
    };
    LedgeMove {
        kind: MoveKind::Arrival,
        seq,
        durations: if found { durations } else { [GRAB_TIME, 0.0, 0.0, 0.0] },
        t: 0.0,
        from,
        to,
        facing_from: facing,
        facing_to: facing,
        follow_disp: found,
        lead: 0.0,
        table: None,
        root_time,
        end_free,
        end_wall: false,
        end_stand,
        hand_l,
        hand_r,
        normal: n,
    }
}

/// The climb's jump up to an overhang (`HumanClimb__TryBackEject` 0xDF2F50 → StartRelease 0xDE96A0 →
/// `sub_DF9650`): `xx_h_climbing_1m_impultion` (0x1E380CEB) until it ends, then
/// `xx_h_climbing_1m_impultion_to_swingback_{min,max}_{200,300}cm` (0x1EB21811) weighted
/// [(1−h)(1−d), h(1−d), d(1−h), d·h] with h = clamp(target hands − root − 2.0, 0, 1) and d = the flat length of the
/// current hands' midpoint less the target's, projected onto the plane through it with normal S = F × U of the free-hang
/// pose (`Vec_ProjectOntoPlane` 0xDF9847): the depth along F only, clamped to [0, 1]. The impulse plays in place (the
/// Release state advances no interpolator, 0xDFA7E0); the root then goes linearly to the hang over the swingback
/// (`sub_711130`(…, 0, 0, 0, 0, 0) 0xDF9A54) and the Ledge context starts in a free hang (the action's exit is the
/// free-hang wait 0x012719F1) (RE/24 CLIMB-10).
pub fn climb_jump_move(from: Vec3, cur_hands: Vec3, hand_l: Vec3, hand_r: Vec3, n: Vec3, collision: &CollisionWorld) -> LedgeMove {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mid = (hand_l + hand_r) * 0.5;
    let h = (hand_l.y.max(hand_r.y) - from.y - 2.0).clamp(0.0, 1.0);
    let d = (mid - cur_hands).dot(facing).abs().clamp(0.0, 1.0);
    let w = [(1.0 - h) * (1.0 - d), h * (1.0 - d), d * (1.0 - h), d * h];
    let item = |id: u32, w: &[f32]| jump_blend::action_items(id).filter(|it| !it.is_empty()).map(|_| ActionBlend::new(id, 0, w));
    let seq = [item(crate::player::climb::CLIMB_IMPULSE, &[1.0]), item(crate::player::climb::CLIMB_JUMP_SWINGBACK, &w), None, None];
    let durations = seq_durations(&seq);
    let found = durations.iter().sum::<f32>() > 0.0;
    LedgeMove {
        kind: MoveKind::Arrival,
        seq,
        durations: if found { durations } else { [GRAB_TIME, 0.0, 0.0, 0.0] },
        t: 0.0,
        from,
        to: hang_root_at(hand_l, hand_r, n, LedgeHangType::Free, collision),
        facing_from: facing,
        facing_to: facing,
        follow_disp: false,
        lead: if found { durations[0] } else { 0.0 },
        table: None,
        root_time: None,
        end_free: true,
        end_wall: false,
        end_stand: false,
        hand_l,
        hand_r,
        normal: n,
    }
}

/// The climb's move onto an overhang (`HumanClimb__TryTransitionToLedgeHang` 0xDF4BA0 → Ledge fill `sub_DEEAC0`): the
/// hands' new holds become a free hang (LedgeData +104 = 1, SubState 9, no feet holds: `sub_B15AD0`) and
/// `xx_h_climb_1m_u_overhangfree_{1lu,1ru}` plays (`action`, FROMAI) while the root is interpolated to the hang over
/// its blended length (sub_711130).
pub fn climb_overhang_move(from: Vec3, hand_l: Vec3, hand_r: Vec3, n: Vec3, action: u32, collision: &CollisionWorld) -> LedgeMove {
    climb_to_hang_move(from, hand_l, hand_r, n, action, LedgeHangType::Free, collision)
}

/// The climb's transitions into a hang (the overhang 0xDF4BA0, the ledge grabs 0xDF0980): `action` plays (FROMAI)
/// while the root is interpolated to the hang of type `hang` over its blended length (sub_711130).
pub fn climb_to_hang_move(from: Vec3, hand_l: Vec3, hand_r: Vec3, n: Vec3, action: u32, hang: LedgeHangType, collision: &CollisionWorld) -> LedgeMove {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let seq = [single(action, 0), None, None, None];
    let durations = seq_durations(&seq);
    let found = durations.iter().sum::<f32>() > 0.0;
    LedgeMove {
        kind: MoveKind::Arrival,
        seq,
        durations: if found { durations } else { [GRAB_TIME, 0.0, 0.0, 0.0] },
        t: 0.0,
        from,
        to: hang_root_at(hand_l, hand_r, n, hang, collision),
        facing_from: facing,
        facing_to: facing,
        follow_disp: false,
        lead: 0.0,
        table: None,
        root_time: None,
        end_free: hang != LedgeHangType::Wall,
        end_wall: hang == LedgeHangType::Wall,
        end_stand: false,
        hand_l,
        hand_r,
        normal: n,
    }
}

// ---------------------------------------------------------------- pull-up

/// Pull-up actions (Pullup_Start 0xDDBE80): wall hang `xx_h_hangwall{,_45_in,_30_out}_tr_hangknee_footl_{a,b}`;
/// free hang `xx_h_hangfree_tr_hangwaist_{a,b}`, then hangwaist → hangknee; both end hangknee → wait.
pub const ACT_PULLUP_WALL: u32 = 0x0106_D2C5;
pub const ACT_PULLUP_FREE: u32 = 0x0127_19F2;

/// An item with all weight on its first clip (the straight variant), sized to the item's clip count.
fn first_clip(id: u32, item: usize) -> Option<ActionBlend> {
    let n = jump_blend::action_items(id)?.get(item)?.len();
    let mut w = vec![0.0; n.max(1)];
    w[0] = 1.0;
    Some(ActionBlend::new(id, item, &w))
}

/// One-hand pull-ups (Pullup_Start 0xDDBE80 when `HumanLedge__PullupNeedsOneHand` 0xDD1120): wall
/// `xx_h_hangwall_onehand_to_hangknee_onehand_footl_{a,b}` (957955002; items {straight, 45_in, 30_out}); free
/// `xx_h_hangfree_onehand_to_hangwaist_onehand_{a,b}` (966087941 = ACT_PULLUP_FREE + 0x386E3B13), then (Pullup_Tick
/// 0xDE2EE0) `xx_h_hangwaist_onehand_to_hangknee_onehand_footl` (966087942). Both end with
/// `xx_h_hangknee_onehand_footl_to_freestep_entry_footl` (957955003, SwitchToNarrowObject_Enter 0xDD2A50).
pub const ACT_PULLUP_WALL_ONEHAND: u32 = 0x3919_3BBA;
pub const ACT_PULLUP_FREE_ONEHAND: u32 = 0x3995_5505;
pub const ACT_WAIST_TO_KNEE_ONEHAND: u32 = 0x3995_5506;
pub const ACT_KNEE_ONEHAND_TO_FREESTEP: u32 = 0x3919_3BBB;

/// The pull-up onto the top: the root follows the clips' own displacement (FROMANIM: up first, then in over
/// the lip) and is corrected onto the stand point 0.5 m inside the edge (Pullup_Start 0xDDBE80). A straight
/// line from the hang to that point cuts through the wall. Returns the move and, for the free hang (5 clips),
/// the queued second part. `one_hand`: the one-hand variants (0xDD1120: the top drops away beside the hands).
pub fn pullup_move(hang: LedgeHangType, one_hand: bool, from: Vec3, hand_l: Vec3, hand_r: Vec3, n: Vec3, top_feet: Vec3) -> (LedgeMove, Option<LedgeMove>) {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mk = |seq: [Option<ActionBlend>; 4], from: Vec3, to: Vec3, end_stand: bool| {
        let durations = seq_durations(&seq);
        let found = durations.iter().sum::<f32>() > 0.0;
        LedgeMove {
            kind: MoveKind::Pullup,
            seq,
            durations: if found { durations } else { [GRAB_TIME, 0.0, 0.0, 0.0] },
            t: 0.0,
            from,
            to,
            facing_from: facing,
            facing_to: facing,
            follow_disp: found,
            lead: 0.0,
            table: None,
            root_time: None,
            end_free: false,
            end_wall: false,
            end_stand,
            hand_l,
            hand_r,
            normal: n,
        }
    };
    // the pull-up's hangknee clips are all footl
    let stand = first_clip(if one_hand { ACT_KNEE_ONEHAND_TO_FREESTEP } else { ACT_KNEE_TO_FREESTEP[0] }, 0);
    let (wall, free, waist) = if one_hand {
        (ACT_PULLUP_WALL_ONEHAND, ACT_PULLUP_FREE_ONEHAND, ACT_WAIST_TO_KNEE_ONEHAND)
    } else {
        (ACT_PULLUP_WALL, ACT_PULLUP_FREE, ACT_WAIST_TO_KNEE)
    };
    match hang {
        LedgeHangType::Wall => (
            mk([first_clip(wall, 0), first_clip(wall, 1), stand, None], from, top_feet, true),
            None,
        ),
        LedgeHangType::Free => {
            let mut a = mk([first_clip(free, 0), first_clip(free, 1), first_clip(waist, 0), None], from, from, false);
            // first part: the clips' path, corrected only by the second part
            a.to = a.natural_end();
            let b = mk([stand, None, None, None], a.to, top_feet, true);
            (a, Some(b))
        }
    }
}

// ---------------------------------------------------------------- hang-type switch

/// `HumanLedge__TrySwitchHangType` 0xDE1060 actions. Free → wall: [left, other] = `hangfree_tr_hangwall_{left,
/// right}` (up / down use the right one). Wall → free: [left, right, up, down] = `hangwall_tr_hangfree_{left,
/// right,up_left,down_left}`.
pub const TO_WALL: [u32; 2] = [0x01C3_2562, 0x01C3_2563];
pub const TO_FREE: [u32; 4] = [0x01C3_14BD, 0x01C3_14BE, 0x01C3_217A, 0x01C3_217C];

/// Ledge stick direction as the exe numbers it (QuantizeStickDirection 0xDD1920): 0 up, 1 down, 2 left, 3 right.
pub fn switch_move(dir: u8, to_wall: bool, from: Vec3, hand_l: Vec3, hand_r: Vec3, n: Vec3, collision: &CollisionWorld) -> LedgeMove {
    let id = if to_wall { TO_WALL[(dir != 2) as usize] } else { TO_FREE[match dir { 2 => 0, 3 => 1, 0 => 2, _ => 3 }] };
    let a = single(id, 0);
    let d = a.map(|a| a.duration()).unwrap_or(0.0);
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let hang = if to_wall { LedgeHangType::Wall } else { LedgeHangType::Free };
    LedgeMove {
        kind: MoveKind::SwitchHang { to_wall },
        seq: [a, None, None, None],
        durations: [if d > 0.0 { d } else { SHIMMY_OPEN_TIME }, 0.0, 0.0, 0.0],
        t: 0.0,
        from,
        to: hang_root_at(hand_l, hand_r, n, hang, collision),
        facing_from: facing,
        facing_to: facing,
        // the root interpolator over the action (0xDE1060 → sub_711130)
        follow_disp: false,
        lead: 0.0,
        table: None,
        root_time: None,
        end_free: !to_wall,
        end_wall: to_wall,
        end_stand: false,
        hand_l,
        hand_r,
        normal: n,
    }
}

/// `xx_h_hangwallfree_tr_hangwall`: what `HumanLedge__Pullup_Start` 0xDDBE80 plays instead of a pull-up from the
/// wall-free hang (LedgeHangType 2, a free hang with a wall below): the hang type becomes Wall (+104 = 0) and the legs go
/// onto the wall; when it is released the wall hang plays (flag 4, `Pullup_Tick` 0xDE2EE0) and the next pull-up is the
/// wall one.
pub const WALLFREE_TO_WALL: u32 = 0x0106_F2EA;

/// The wall-free hang's "pull-up" (0xDDBE80 case 2): `WALLFREE_TO_WALL` into the wall hang. The game plays it from the
/// animation (no root interpolator in that branch); the port follows the clip's displacement with a correction onto the
/// wall hang's root.
pub fn wallfree_to_wall_move(from: Vec3, hand_l: Vec3, hand_r: Vec3, n: Vec3) -> LedgeMove {
    let a = single(WALLFREE_TO_WALL, 0);
    let d = a.map(|a| a.duration()).unwrap_or(0.0);
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    LedgeMove {
        kind: MoveKind::SwitchHang { to_wall: true },
        seq: [a, None, None, None],
        durations: [if d > 0.0 { d } else { SHIMMY_OPEN_TIME }, 0.0, 0.0, 0.0],
        t: 0.0,
        from,
        to: super::ledge::hang_root(hand_l, hand_r, n, LedgeHangType::Wall),
        facing_from: facing,
        facing_to: facing,
        follow_disp: d > 0.0,
        lead: 0.0,
        table: None,
        root_time: None,
        end_free: false,
        end_wall: true,
        end_stand: false,
        hand_l,
        hand_r,
        normal: n,
    }
}

// ---------------------------------------------------------------- pull-down (ground → hang)

/// Pull-down table 0x1A2C3F0 (index side + 8·type; +4 = the descent action): the ground types have front
/// entries only. [EdgeStop (0), Wait (1)] orientation: `ledge_stop_start_footl_pulldown_front_orientation`,
/// `ledge_lookdown_front_pulldown_front_orientation`.
pub const PULLDOWN_ORIENT: [u32; 2] = [0xB5EB_D80B, 0x06E8_BD80];
/// Descent (types 0 and 1): `ledge_pulldown_soft_front`.
pub const PULLDOWN_DESCENT: u32 = 0x082F_8C53;
/// Reception (0xDDE980 stage 2): wall `pulldown_soft_to_hangwall_{straight,30_out,45_in}_a` (115916161), then its
/// transition action `_b` + `_tr` (0x082F820C); free `pulldown_soft_front_to_hangfree_a` (137338683), then
/// 0x082F9F3C (`_b` + `_tr`).
pub const PULLDOWN_WALL: [u32; 2] = [0x06E8_BD81, 0x082F_820C];
pub const PULLDOWN_FREE: [u32; 2] = [0x082F_9F3B, 0x082F_9F3C];
/// Type 3 (from a beam or a pilotis, `HumanNarrowObject__FillLedgePullDown` 0xE4EF70), by side [front, back, left,
/// right] (0xDCC2A0 fills 0x1A2C450 / 0x1A2C460): `beam_pilotis_to_pulldown_soft_{side}_orientation`, then
/// `beam_pilotis_to_pulldown_soft_{side}`.
pub const PULLDOWN_BEAM_ORIENT: [u32; 4] = [0x516D_5CFA, 0x516D_5D00, 0x516D_5D02, 0x516D_5D04];
pub const PULLDOWN_BEAM_DESCENT: [u32; 4] = [0x516D_5CF9, 0x516D_5D01, 0x516D_5D03, 0x516D_5D05];

/// `HumanLedge__PullDown_Enter` 0xDDE4D0 + `HumanLedge__PullDown_Update` 0xDDE980 (front side).
/// `p` = the edge point, `n` = the edge's outward normal (the character faces along it, guard 0xD9D6C0),
/// `wait` = type 1 (from Movement) instead of 0 (from the ledge stop).
/// 1. Orientation: the type's orientation action, root → p + 0.5·n, facing −n.
/// 2. Descent: hands found on the edge at p ± 0.25·side, the descent action, root → p + 1.0·n − 0.8 m. No hands:
///    PullDownSubState 4, ReleaseToInAir (`StatePullDown_Update` 0xDDFCF0 → 0xDDA100, the hang let-go 0xDD7810):
///    `Err` holds the orientation alone, which falls when it ends.
/// 3. Reception: foot support (`sub_B16130`) → wall reception (angle blend `pulldown_wall_weights`) to the wall-hang
///    root, else the free reception to the free-hang root.
pub fn pulldown(p: Vec3, n: Vec3, from: Vec3, wait: bool, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Result<[LedgeMove; 3], LedgeMove> {
    pulldown_with(p, n, from, PULLDOWN_ORIENT[wait as usize], PULLDOWN_DESCENT, guidance, collision)
}

/// The pull-down reception's angle blend (`HumanLedge__PullDown_Update` 0xDDF0AA): the signed angle of the line from
/// the foot support up to the hands, from vertical (positive with the hands further out than the wall at the feet);
/// a ≥ 0 → straight / `30_out` by a / 30°, a < 0 → straight / `45_in` by −a / 45° (both clamped to 1). Weights
/// [straight, 30_out, 45_in]. PORT: the foot support is the wall 1 m below the hands (the hang-type probe's height).
pub fn pulldown_wall_weights(mid: Vec3, n: Vec3, collision: &CollisionWorld) -> [f32; 3] {
    let out = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let origin = mid - Vec3::Y + out * 0.5;
    let Some((d, _)) = collision.ray_hit(origin, -out, 1.2, 0) else {
        return [1.0, 0.0, 0.0];
    };
    let wall = origin - out * d;
    let a = (mid - wall).dot(out).atan2(mid.y - wall.y);
    if a >= 0.0 {
        let w = (a / 30f32.to_radians()).min(1.0);
        [1.0 - w, w, 0.0]
    } else {
        let w = (-a / 45f32.to_radians()).min(1.0);
        [1.0 - w, 0.0, w]
    }
}

/// The pull-down with the type's orientation and descent actions (table 0x1A2C3F0).
pub fn pulldown_with(p: Vec3, n: Vec3, from: Vec3, orient_id: u32, descent_id: u32, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Result<[LedgeMove; 3], LedgeMove> {
    pulldown_blend(p, n, from, single(orient_id, 0), single(descent_id, 0), 0.0, guidance, collision)
}

/// The pass-over pull-down (type 4, `HumanLedge__PassOverToPullDown` 0xDE0220): from the wall top's far edge after
/// the vault with hand `hand` (0 left: the orientation `0x09A0B647`, else `0x09A0B649`; the descent `…48` / `…4A`),
/// both blended [1 − w, w] by the thickness, and the orientation and descent roots pulled back 0.3·(1 − w) along
/// the normal (0xDDE7FA / 0xDDED9E).
pub fn pulldown_passover(p: Vec3, n: Vec3, from: Vec3, hand: usize, w: f32, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Result<[LedgeMove; 3], LedgeMove> {
    use super::passover::{PULLDOWN_DESCENT as D, PULLDOWN_ORIENT as O};
    let blend = |id: u32| jump_blend::action_items(id).map(|_| ActionBlend::new(id, 0, &[1.0 - w, w]));
    pulldown_blend(p, n, from, blend(O[hand.min(1)]), blend(D[hand.min(1)]), 0.3 * (1.0 - w), guidance, collision)
}

/// The pull-down's three stages with the orientation and descent actions given; `inset` pulls both roots back
/// toward the wall.
pub fn pulldown_blend(p: Vec3, n: Vec3, from: Vec3, orient_action: Option<ActionBlend>, descent_action: Option<ActionBlend>, inset: f32, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Result<[LedgeMove; 3], LedgeMove> {
    let n = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let facing_out = n;
    let facing_in = -n;
    let r = right_of(facing_in);
    let hands = guidance.on_edge(p - r * 0.25, n, 0.3).zip(guidance.on_edge(p + r * 0.25, n, 0.3));
    let Some((hl, hr)) = hands.map(|(a, b)| (a.point, b.point)) else {
        // ReleaseToInAir: the orientation plays, then the let-go
        let seq = [orient_action, None, None, None];
        let durations = seq_durations(&seq);
        return Err(LedgeMove {
            kind: MoveKind::PullDown { stage: 4 },
            seq,
            durations: if durations.iter().sum::<f32>() > 0.0 { durations } else { [GRAB_TIME, 0.0, 0.0, 0.0] },
            t: 0.0,
            from,
            to: p + n * (0.5 - inset),
            facing_from: facing_out,
            facing_to: facing_in,
            follow_disp: false,
            lead: 0.0,
            table: None,
            root_time: None,
            end_free: false,
            end_wall: false,
            end_stand: false,
            hand_l: p - r * 0.25,
            hand_r: p + r * 0.25,
            normal: n,
        });
    };
    let mid = (hl + hr) * 0.5;
    let mk = |kind: u8, seq: [Option<ActionBlend>; 4], from: Vec3, to: Vec3, ff: Vec3, ft: Vec3, end_wall: bool, end_free: bool| {
        let durations = seq_durations(&seq);
        let found = durations.iter().sum::<f32>() > 0.0;
        LedgeMove {
            kind: MoveKind::PullDown { stage: kind },
            seq,
            durations: if found { durations } else { [GRAB_TIME, 0.0, 0.0, 0.0] },
            t: 0.0,
            from,
            to,
            facing_from: ff,
            facing_to: ft,
            follow_disp: false,
            lead: 0.0,
            table: None,
            root_time: None,
            end_free,
            end_wall,
            end_stand: false,
            hand_l: hl,
            hand_r: hr,
            normal: n,
        }
    };
    let item = |id: u32, i: usize, w: &[f32]| jump_blend::action_items(id).filter(|it| it.len() > i).map(|_| ActionBlend::new(id, i, w));
    let p1 = Vec3::new(mid.x, mid.y, mid.z) + n * (0.5 - inset);
    let orient = mk(1, [orient_action, None, None, None], from, p1, facing_out, facing_in, false, false);
    let p2 = mid + n * (1.0 - inset) - Vec3::Y * 0.8;
    let descent = mk(2, [descent_action, None, None, None], p1, p2, facing_in, facing_in, false, false);
    let wall = super::ledge::feet_supports(mid, n, collision);
    let w = pulldown_wall_weights(mid, n, collision);
    let reception = if wall {
        mk(
            3,
            [item(PULLDOWN_WALL[0], 0, &w), item(PULLDOWN_WALL[1], 0, &w), item(PULLDOWN_WALL[1], 1, &w), None],
            p2,
            hang_root(hl, hr, n, LedgeHangType::Wall),
            facing_in,
            facing_in,
            true,
            false,
        )
    } else {
        mk(
            3,
            [single(PULLDOWN_FREE[0], 0), single(PULLDOWN_FREE[1], 0), single(PULLDOWN_FREE[1], 1), None],
            p2,
            hang_root_at(hl, hr, n, LedgeHangType::Free, collision),
            facing_in,
            facing_in,
            false,
            true,
        )
    };
    Ok([orient, descent, reception])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RE/24 CLIMB-10: the jump up to an overhang weighs the depth only (a sideways offset changes nothing), plays the
    /// impulse in place and moves the root linearly over the swingback.
    #[test]
    fn the_overhang_jump_weighs_the_depth_and_holds_the_root_through_the_impulse() {
        let c = CollisionWorld::default();
        let (from, cur) = (Vec3::new(0.0, 1.0, 0.5), Vec3::new(0.0, 2.2, 0.1));
        let n = Vec3::Z; // the overhang's front faces +Z, 0.8 m out from the current hands
        let mv = |dx: f32| climb_jump_move(from, cur, Vec3::new(dx - 0.2, 3.5, 0.9), Vec3::new(dx + 0.2, 3.5, 0.9), n, &c);
        let (a, b) = (mv(0.0), mv(0.7));
        assert_eq!(a.seq[1].map(|x| x.w), b.seq[1].map(|x| x.w), "a sideways offset leaves the weights");
        let mut m = a;
        assert!(m.seq[0].is_some() && m.durations[0] > 0.02, "the impulse clip is dumped");
        {
            let (p, _) = m.advance(m.durations[0] * 0.5);
            assert_eq!(p, from, "the impulse plays in place");
            let (p, _) = m.advance(m.durations[0] * 0.5 + m.durations[1] * 0.5);
            assert!((p - from.lerp(m.to, 0.5)).length() < 1e-3, "linear over the swingback: {p}");
        }
    }

    /// RE/24 LEDGE-23: the swing back's horizontal weight counts the travel out from the wall only, not sideways.
    #[test]
    fn the_swing_back_ignores_the_sideways_offset() {
        let (mid, root) = (Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, 0.9, 0.5));
        let nn = Vec3::NEG_Z; // the new edge faces -Z, 0.6 m out behind; hands 2.4 m above the root
        let straight = swing_back_weights(Vec3::new(-0.2, 3.3, -0.6), Vec3::new(0.2, 3.3, -0.6), nn, mid, root);
        let shifted = swing_back_weights(Vec3::new(0.6, 3.3, -0.6), Vec3::new(1.0, 3.3, -0.6), nn, mid, root);
        assert_eq!(straight, shifted);
        // v reads the higher hand: 3.4 − 0.9 − 2.0 = 0.5
        let uneven = swing_back_weights(Vec3::new(-0.2, 3.4, -0.6), Vec3::new(0.2, 3.2, -0.6), nn, mid, root);
        assert!((uneven[1] + uneven[3] - 0.5).abs() < 1e-5, "{uneven:?}");
    }

    /// RE/24 AIR-8: min for a short jump, max for a fall above 2.5 m or a jump from more than 6 m away; the lean blends
    /// toward the 30° clip (leaning back) or the 45° clip (leaning in).
    #[test]
    fn the_wall_reception_picks_min_or_max_and_the_lean() {
        let o = Vec3::ZERO;
        assert_eq!(wall_reception_weights(1.0, o, Vec3::new(0.0, 2.0, -3.0), 0.0), [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(wall_reception_weights(3.0, o, Vec3::new(0.0, 2.0, -3.0), 0.0), [0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(wall_reception_weights(0.0, o, Vec3::new(0.0, 0.0, -6.5), 0.0)[3], 1.0);
        let w = wall_reception_weights(0.0, o, Vec3::new(0.0, 0.0, -1.0), 15f32.to_radians());
        assert!((w[0] - 0.5).abs() < 1e-5 && (w[1] - 0.5).abs() < 1e-5 && w[2] == 0.0);
        let w = wall_reception_weights(0.0, o, Vec3::new(0.0, 0.0, -1.0), -90f32.to_radians());
        assert_eq!([w[0], w[1], w[2]], [0.0, 0.0, 1.0]);
    }
    use crate::collision::Aabb3;
    use crate::guidance::{GuidanceEdge, GuidanceSubType};

    /// A 3 m wall (face z 0, normal -Z) with a ledge at 2.6 m, x -5..5, and optionally a crate at leg height beside the
    /// hang (x 0.6..1.4, 0.5..1.6 m up, against the wall).
    fn wall(crate_beside: bool) -> (CollisionWorld, GuidanceWorld) {
        let mut c = CollisionWorld::default();
        c.boxes.push(Aabb3 { min: Vec3::new(-5.0, 0.0, 0.0), max: Vec3::new(5.0, 2.6, 1.0) });
        if crate_beside {
            c.boxes.push(Aabb3 { min: Vec3::new(0.6, 0.5, -0.9), max: Vec3::new(1.4, 1.6, 0.0) });
        }
        let mut g = GuidanceWorld::default();
        g.edges.push(GuidanceEdge { p0: Vec3::new(-5.0, 2.6, 0.0), p1: Vec3::new(5.0, 2.6, 0.0), n0: Vec3::Y, n1: Vec3::NEG_Z, subtype: GuidanceSubType::LedgeGrab });
        (c, g)
    }

    /// RE/24 CLIMB-5: `Human__KneeCatchRoomOnTop` tests room above the edge, not a floor: a 0.2 m thin wall top has room
    /// (the old floor test refused it); a wall rising 0.5 m above the top within 0.3 m of the edge refuses; one set 0.8 m
    /// back does not.
    #[test]
    fn the_climb_out_needs_room_above_the_edge_not_a_floor() {
        let wall = |extra: Option<Aabb3>| {
            let mut c = CollisionWorld::default();
            c.boxes.push(Aabb3 { min: Vec3::new(-2.0, 0.0, 0.0), max: Vec3::new(2.0, 3.0, 0.2) });
            c.boxes.extend(extra);
            c
        };
        let (edge, into) = (Vec3::new(0.0, 3.0, 0.0), Vec3::Z);
        assert!(knee_room_on_top(edge, into, &wall(None)), "a thin wall top has room");
        let close = Aabb3 { min: Vec3::new(-2.0, 3.5, 0.1), max: Vec3::new(2.0, 5.0, 0.2) };
        assert!(!knee_room_on_top(edge, into, &wall(Some(close))), "a wall above the top within 0.3 m refuses");
        let far = Aabb3 { min: Vec3::new(-2.0, 3.0, 0.8), max: Vec3::new(2.0, 5.0, 1.2) };
        assert!(knee_room_on_top(edge, into, &wall(Some(far))), "a wall 0.8 m back leaves room");
    }

    /// RE/24 LEDGE-8: ProbeLateral's free-space test is a body-length capsule (0xDD996B, `sub_B18C30`): a crate at leg
    /// height beside a wall hang blocks the shimmy toward it; the hands alone would pass over it.
    #[test]
    fn an_obstacle_at_leg_height_blocks_the_shimmy() {
        let (mover, other, n) = (Vec3::new(0.15, 2.6, 0.0), Vec3::new(-0.15, 2.6, 0.0), Vec3::NEG_Z);
        let (c, g) = wall(false);
        assert!(probe_lateral(mover, other, n, Vec3::X, false, LedgeHangType::Wall, &g, &c).1.is_some(), "free wall: a step");
        let (c, g) = wall(true);
        assert!(probe_lateral(mover, other, n, Vec3::X, false, LedgeHangType::Wall, &g, &c).1.is_none(), "crate at the legs: blocked");
        // the crate is below the hands and 0.75 m out reaches it: the old sphere at hand height found it free
        let origin = mover - Vec3::X * SHIMMY_SWEEP_R + n * SHIMMY_SWEEP_OUT + Vec3::Y * SHIMMY_SWEEP_UP;
        assert!(c.sphere_free_distance(origin, Vec3::X, SHIMMY_SWEEP_R, SHIMMY_SWEEP_LEN) >= SHIMMY_SWEEP_LEN - 1e-3);
    }
}
