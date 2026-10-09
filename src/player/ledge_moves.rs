//! Ledge moves beyond the shimmy and the hand steps (RE/03 §7.6, decoded 2026-10-01):
//! - **corner turns** (`HumanLedge__TryTurnCorner` 0xDD3BB0, candidates from `HumanLedge__FindCornerEdges`
//!   0xDD0600): inner corner (the wall ahead) and outer corner (around the end of the wall);
//! - **side jumps between ledges** (`HumanLedge__TrySideJumpToLedge` 0xDDD490 → `StartLedgeJump` 0xDDCE40,
//!   table 0x1A2C780 / 0x1A2C980);
//! - **hop up to a ledge above** (`HumanLedge__TryWallJumpUp` 0xDD62A0 → the blended `StartLedgeJump`).
//!
//! The move plays its action(s) while the root travels from the current hang to the target hang. Ledge
//! jumps follow the clips' blended displacement plus a linear correction to the target (hypothesis: the
//! same root interpolation mode as the InAir jump, RE/13 §3); corners use the root interpolator over the
//! clip length (0xDD3BB0 → sub_711130, flag 0).

use bevy::prelude::*;

use super::jump_blend::{self, ActionBlend};
use super::ledge::{hang_root_at, hang_root, hang_type_at, LedgeHangType};
use super::right_of;
use crate::collision::CollisionWorld;
use crate::guidance::GuidanceWorld;
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

/// Corner actions (0xDD3BB0: 510126209 + k). Free hang: `hangfree_corner_{left,right}_090_{in,out}`; wall
/// hang: the two-item strafe open + close actions with the hand targets on the new edge.
/// [free, wall] × [left in, left out, right in, right out].
pub const CORNER_ACTIONS: [[u32; 4]; 2] = [
    [0x1E67_E881, 0x1E67_E882, 0x1E67_E883, 0x1E67_E884],
    [0x1E67_E885, 0x1E67_E886, 0x1E67_E887, 0x1E67_E888],
];
/// Blended hop up (0xDDCE40 with flag 1861&2): `hangwall_to_swingback_up_{min,max}_{200,300}_a`, then
/// (0xDDAB00) `…_b` with the same weights while the root moves to the new ledge; it ends in a free hang.
pub const HOP_UP: u32 = 0x4CD6_8F4F;
pub const HOP_UP_B: u32 = 0x4CD6_8F50;

pub const DUMPED_ACTIONS: &[u32] = &[
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
    HOP_UP, HOP_UP_B,
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
    HopUp,
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
        let p = if self.follow_disp {
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

fn seq_durations(seq: &[Option<ActionBlend>; 4]) -> [f32; 4] {
    let mut d = [0.0; 4];
    for (i, a) in seq.iter().enumerate() {
        d[i] = a.map(|a| a.duration()).unwrap_or(0.0);
    }
    d
}

/// `HumanLedge__FindCornerEdges` 0xDD0600 (same-height row) + `HumanLedge__TryTurnCorner` 0xDD3BB0.
/// `right` = moving right. Probe: inner = hands mid + 0.6·move − 0.6·facing; outer = hands mid +
/// 0.3·facing + 0.2·move; radius 0.25, vertical 0.4, edges whose wall the character would face within 50°.
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
    let side = if right { right_of(facing) } else { -right_of(facing) };
    let mut base = (hand_l + hand_r) * 0.5;
    base.y = hand_l.y.min(hand_r.y);
    let (p, new_facing) = if inner {
        (base + side * 0.6 - facing * 0.6, side)
    } else {
        (base + facing * 0.3 + side * 0.2, -side)
    };
    let hit = guidance.probe(p, 0.25, 0.4, Some(new_facing), 50f32.to_radians())?;
    let nn = hit.wall_normal;
    let new_hang = hang_type_at(hit.point, nn, collision);
    // a wall hang needs foot holds on the new wall (sub_B16130)
    if hang == LedgeHangType::Wall && new_hang != LedgeHangType::Wall {
        return None;
    }
    // hands: both on the corner point during the turn; SecondHandGrab (state 17, ±0.25 m) re-spreads them
    // (hypothesis: to the normal hand spacing around the point)
    let r = right_of(-nn);
    let (hl, hr) = (hit.point - r * HAND_SPACING * 0.5, hit.point + r * HAND_SPACING * 0.5);
    let to = hang_root_at(hl, hr, nn, new_hang, collision);
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
        end_free: false,
        end_wall: false,
        end_stand: false,
        hand_l: hl,
        hand_r: hr,
        normal: nn,
    })
}

/// `HumanLedge__TrySideJumpToLedge` 0xDDD490 (table types 0 / 1 / 2): from the hands' midpoint (highest hand)
/// + 0.9·move, search up to 1.6 m further at hand-height offsets 0, +0.6, −0.6 (radius 0.35, vertical 0.3) for an
/// edge on the same side of the wall. `long` = distance / 1.6 ≥ 0.5. Feet on climb holds 1.2 m below the new edge →
/// type 0: the start and loop play here, the end item in the Climb context (the returned `ClimbEntry`, as
/// `try_jump_up_to_climb`); otherwise a hang (1 wall, 2 free).
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
        let mut s = 0.0;
        while s <= 1.6 {
            let p = start + side * s + Vec3::Y * dz;
            if let Some(hit) = guidance.probe(p, 0.35, 0.3, Some(facing), 45f32.to_radians()) {
                // must be a different stretch of edge than the one we hang from (the shimmy would reach it)
                let along = (hit.point - base).dot(side);
                if along > 0.9 && guidance.on_edge(base + side * (along * 0.5), n, 0.1).is_none() {
                    let nn = hit.wall_normal;
                    let r = right_of(-nn);
                    // both hands on the new edge (sub_B153C0 finds a hand pair): the first hand lands on the
                    // nearest point, the other one HAND_SPACING further along the move
                    let c = hit.point + side * HAND_SPACING * 0.5;
                    let (hl, hr) = (c - r * HAND_SPACING * 0.5, c + r * HAND_SPACING * 0.5);
                    // `TrySideJumpToLedge` 0xDDD490 classifies the landing by the feet search 1.2 m below the hands
                    // (the three passes, feet −1.2 / −0.6 / −1.8 from the hand base): feet on climb holds → type 0
                    // (the climb pose from the holds, `Human__ComputeClimbPoseFromHolds` 0xB1CC20), on a wall surface →
                    // 1, none → 2 (RE/18 §7.2). PORT: the feet holds are a guidance probe, as in `try_climb_reach`.
                    let feet_hold = guidance
                        .probe(hit.point - Vec3::Y * 1.2, 0.35, 0.3, Some(facing), 45f32.to_radians())
                        .filter(|f| (f.point.y - (hit.point.y - 1.2)).abs() <= 0.3 && f.wall_normal.dot(nn) > 0.9);
                    if let Some(f) = feet_hold {
                        let fc = f.point + side * HAND_SPACING * 0.5;
                        let (fl, fr) = (fc - r * HAND_SPACING * 0.5, fc + r * HAND_SPACING * 0.5);
                        let target = super::climb::climb_root_at(hl, hr, fl, fr, nn);
                        if !collision.capsule_cast_free(root - facing * 0.15, 1.8, 0.35, target - root) {
                            return None;
                        }
                        let dist = Vec2::new(hit.point.x - start.x, hit.point.z - start.z).length();
                        let long = (dist / 1.6).clamp(0.0, 1.0) >= 0.5;
                        let entry = LEDGE_JUMP_TABLE[(hang == LedgeHangType::Free) as usize][long as usize * 4 + 2 + right as usize];
                        let [st, lp, end] = entry.1;
                        let seq = [single(st, 0), single(lp, 0), None, None];
                        let durations = seq_durations(&seq);
                        let found = durations.iter().sum::<f32>() > 0.0;
                        // the end item plays in the climb (as `try_jump_up_to_climb`, 0xDD8CF0): the jump part ends
                        // where the end item's own displacement starts
                        let end_disp = single(end, 0).map(|a| a.disp(1.0)).unwrap_or([0.0; 3]);
                        let fto = -Vec3::new(nn.x, 0.0, nn.z).normalize_or_zero();
                        let to = target - (right_of(fto) * end_disp[0] + fto * end_disp[1] + Vec3::Y * end_disp[2]);
                        let mv = LedgeMove {
                            kind: MoveKind::SideJump { long },
                            seq,
                            durations: if found { durations } else { [SIDE_JUMP_FALLBACK_TIME, 0.0, 0.0, 0.0] },
                            t: 0.0,
                            from: root,
                            to,
                            facing_from: facing,
                            facing_to: fto,
                            follow_disp: found,
                            lead: 0.0,
                            end_free: false,
                            end_wall: false,
                            end_stand: false,
                            hand_l: hl,
                            hand_r: hr,
                            normal: nn,
                        };
                        let climb = super::climb::ClimbEntry {
                            entry_type: super::climb::ClimbEntryType::FromLedgeParallelJump,
                            hand_l: hl,
                            hand_r: hr,
                            foot_l: fl,
                            foot_r: fr,
                            normal: nn,
                            from_feet: to,
                            foot_right: false,
                            action: Some(end),
                        };
                        return Some((mv, Some(climb)));
                    }
                    let new_hang = hang_type_at(c, nn, collision);
                    let to = hang_root_at(hl, hr, nn, new_hang, collision);
                    if !collision.capsule_fits(to + Vec3::Y * 0.05) {
                        return None;
                    }
                    let dist = Vec2::new(hit.point.x - start.x, hit.point.z - start.z).length();
                    let long = (dist / 1.6).clamp(0.0, 1.0) >= 0.5;
                    let ty = if new_hang == LedgeHangType::Wall { 1 } else { 2 };
                    let entry = LEDGE_JUMP_TABLE[(hang == LedgeHangType::Free) as usize][(ty * 2 + long as usize) * 4 + 2 + right as usize];
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
                        follow_disp: found,
                        lead: 0.0,
                        end_free: false,
                        end_wall: false,
                        end_stand: false,
                        hand_l: hl,
                        hand_r: hr,
                        normal: nn,
                    }, None));
                }
            }
            s += 0.1;
        }
    }
    None
}

/// The climb's reach sideways to another surface (`HumanClimb__TryReachOtherSurface` 0xDF9B30 → `sub_DF7850`, dirs
/// 4–9; ChooseMove 0xDFDE90 tries it after the grid moves and the corner moves). `diag` = 0 level, 1 up (dirs 6 / 7), 2
/// down (8 / 9). From the feet's midpoint (the lower foot's height) + 0.9·move, `sub_B153C0` searches 1.6 m along the
/// move for a hand edge at foot offset + 1.2 m with the foot offsets 0, +0.6, −0.6 by `diag` (r 0.35, 0.3 / 0.3).
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
/// PORT: the search is guidance probes stepped 0.1 m (as `try_side_jump`); the feet are a probe 1.2 m below the hand
/// edge (a hit = climb holds, type 0); wall or free comes from the collision foot rays (`hang_type_at`), the greybox has
/// no Surface guidance. For the hangs the root follows the clips' displacement plus a linear correction (hypothesis, as
/// for the ledge jumps) instead of holding still through the start.
pub fn try_climb_reach(
    right: bool,
    diag: usize,
    foot_l: Vec3,
    foot_r: Vec3,
    n: Vec3,
    root: Vec3,
    guidance: &GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<ClimbReachTarget> {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let side = if right { right_of(facing) } else { -right_of(facing) };
    let mut base = (foot_l + foot_r) * 0.5;
    base.y = foot_l.y.min(foot_r.y);
    let foot_off = [0.0f32, 0.6, -0.6][diag.min(2)];
    let start = base + side * 0.9 + Vec3::Y * (foot_off + 1.2);
    let mut s = 0.0;
    while s <= 1.6 {
        let p = start + side * s;
        if let Some(hit) = guidance.probe(p, 0.35, 0.3, Some(facing), 45f32.to_radians()) {
            // not the climb wall the feet are on (its holds are the grid's)
            let off_wall = (hit.point - base).dot(side) > 0.9;
            if off_wall {
                let nn = hit.wall_normal;
                let fc = -Vec3::new(nn.x, 0.0, nn.z).normalize_or_zero();
                let u_back = |to: Vec3| root - (to - root).normalize_or_zero() * 0.2625;
                let dist = Vec2::new(hit.point.x - start.x, hit.point.z - start.z).length();
                let long = (dist / 1.6).clamp(0.0, 1.0) >= 0.5;
                // feet on climb holds 1.2 m below → type 0: climb → climb
                if let Some(f) = guidance.probe(hit.point - Vec3::Y * 1.2, 0.35, 0.3, Some(fc), 45f32.to_radians()) {
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
                let r = right_of(-nn);
                let c = hit.point + side * HAND_SPACING * 0.5;
                let (hl, hr) = (c - r * HAND_SPACING * 0.5, c + r * HAND_SPACING * 0.5);
                let hang = hang_type_at(c, nn, collision);
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
                return Some(ClimbReachTarget::Hang(LedgeMove {
                    kind: MoveKind::ClimbReach { long },
                    seq,
                    durations: if found { durations } else { [SIDE_JUMP_FALLBACK_TIME, 0.0, 0.0, 0.0] },
                    t: 0.0,
                    from: root,
                    to,
                    facing_from: facing,
                    facing_to: fc,
                    follow_disp: found,
                    lead: 0.0,
                    end_free: ty == 2,
                    end_wall: ty == 1,
                    end_stand: false,
                    hand_l: hl,
                    hand_r: hr,
                    normal: nn,
                }));
            }
        }
        s += 0.1;
    }
    None
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
        end_free: true,
        hand_l: hl,
        hand_r: hr,
        ..mv
    };
    mv.hand_l = first;
    mv.hand_r = first;
    (mv, Some(tail))
}

/// `HumanLedge__TryWallJumpUp` 0xDD62A0 (wall hang, up): an edge above the reach of a hand step, then the
/// blended hop (0xDDCE40): weights over `swingback_up_{min,max}_{200,300}` by v = clamp(targetZ − rootZ −
/// 2.0) and h = clamp(horizontal distance of the hands). (hypothesis) the search: edges 1.3–1.9 m above the
/// hands within 0.6 m horizontally (the box query's arguments are not fully recovered).
pub fn try_hop_up(hand_l: Vec3, hand_r: Vec3, n: Vec3, root: Vec3, hang: LedgeHangType, guidance: &GuidanceWorld, _collision: &CollisionWorld) -> Option<LedgeMove> {
    if hang != LedgeHangType::Wall {
        return None;
    }
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mid = (hand_l + hand_r) * 0.5;
    for k in [1.3f32, 1.6, 1.9] {
        let Some(hit) = guidance.probe(mid + Vec3::Y * k, 0.6, 0.15, Some(facing), 45f32.to_radians()) else { continue };
        let dy = hit.point.y - mid.y;
        if dy <= VSTEP_MAX {
            continue;
        }
        let nn = hit.wall_normal;
        let r = right_of(-nn);
        let (hl, hr) = (hit.point - r * HAND_SPACING * 0.5, hit.point + r * HAND_SPACING * 0.5);
        let to = hang_root_at(hl, hr, nn, LedgeHangType::Free, _collision);
        let v = (hit.point.y - root.y - 2.0).clamp(0.0, 1.0);
        let h = Vec2::new(hit.point.x - mid.x, hit.point.z - mid.z).length().clamp(0.0, 1.0);
        let w = [(1.0 - h) * (1.0 - v), (1.0 - h) * v, h * (1.0 - v), h * v];
        let a = jump_blend::action_items(HOP_UP).map(|_| ActionBlend::new(HOP_UP, 0, &w));
        let b = jump_blend::action_items(HOP_UP_B).map(|_| ActionBlend::new(HOP_UP_B, 0, &w));
        let (da, db) = (a.map(|a| a.duration()).unwrap_or(0.0), b.map(|b| b.duration()).unwrap_or(0.0));
        let (da, db) = if da + db > 0.0 { (da, db) } else { (0.0, JUMP_UP_TIME) };
        return Some(LedgeMove {
            kind: MoveKind::HopUp,
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
/// hands (the hands' midpoint at the higher hand) and a second row 1.2 m below that hit (sub_11713A0: r 0.4,
/// 0.5 / 0.3, 45°, filter dword_193419C). Then the climb pose (sub_B1CC20), a body clearance sweep (r 0.35, from
/// root − 1.1 z and 0.65 m back, 1.8 m up, sub_B136E0) and the table ledge jump type 0, short, side up (wall
/// table entry 0: `0x491228AE` start, `…AF` loop, `…B0` end, landing category 0).
///
/// Landing (StateLedgeJump_Update 0xDDFC30 → sub_DCE1F0): once the end item plays, the context switches to Climb
/// (0xDD8CF0 → fill sub_DD8290: ClimbData EntryType 2 = FromLedgeParallelJump, the root interpolated to the
/// climb pose over the end item's blended duration). Returns the start + loop part (in the Ledge context) and
/// the climb entry carrying the end action.
///
/// PORT: both hands go to the upper hit and both feet to the lower one (as the side moves into the climb); the
/// rows are guidance probes (r 0.4, vertical 0.5), and the lower row may not be the edge hung from. The root follows the start and loop clips' displacement plus a
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
    let upper = guidance.probe(base + Vec3::Y * JUMP_TO_CLIMB_UP, JUMP_TO_CLIMB_R, JUMP_TO_CLIMB_VTOL, Some(facing), 0.785)?;
    if upper.point.y - base.y <= VSTEP_MAX {
        return None;
    }
    let lower = guidance
        .probe(upper.point - Vec3::Y * JUMP_TO_CLIMB_UP, JUMP_TO_CLIMB_R, JUMP_TO_CLIMB_VTOL, Some(facing), 0.785)
        .filter(|h| upper.point.y - h.point.y > VSTEP_MIN)
        // PORT: the game's filter (dword_193419C) only takes climb holds, so the ledge hung from never counts as the
        // lower row; the port's guidance has one edge type, so that edge is excluded instead
        .filter(|h| guidance.on_edge((hand_l + hand_r) * 0.5, n, 0.1).is_none_or(|cur| cur.edge != h.edge))?;
    let nn = upper.wall_normal;
    let target = super::climb::climb_root_at(upper.point, upper.point, lower.point, lower.point, nn);
    if !collision.capsule_fits(target + Vec3::Y * 0.05 + nn * 0.1) {
        return None;
    }
    let entry = LEDGE_JUMP_TABLE[0][0];
    let [start, lp, end] = entry.1;
    let seq = [single(start, 0), single(lp, 0), None, None];
    let durations = seq_durations(&seq);
    let found = durations.iter().sum::<f32>() > 0.0;
    // the end item plays in the climb (sub_DD8290): the jump part ends where the end item's own displacement
    // starts
    let end_disp = single(end, 0).map(|a| a.disp(1.0)).unwrap_or([0.0; 3]);
    let to = target - (right_of(facing) * end_disp[0] + facing * end_disp[1] + Vec3::Y * end_disp[2]);
    let mv = LedgeMove {
        kind: MoveKind::JumpUpToClimb,
        seq,
        durations: if found { durations } else { [SIDE_JUMP_FALLBACK_TIME, 0.0, 0.0, 0.0] },
        t: 0.0,
        from: root,
        to,
        facing_from: facing,
        facing_to: -Vec3::new(nn.x, 0.0, nn.z).normalize_or_zero(),
        follow_disp: found,
        lead: 0.0,
        end_free: false,
        end_wall: false,
        end_stand: false,
        hand_l: upper.point,
        hand_r: upper.point,
        normal: nn,
    };
    let climb = super::climb::ClimbEntry {
        entry_type: super::climb::ClimbEntryType::FromLedgeParallelJump,
        hand_l: upper.point,
        hand_r: upper.point,
        foot_l: lower.point,
        foot_r: lower.point,
        normal: nn,
        from_feet: to,
        foot_right: false,
        action: Some(end),
    };
    Some((mv, climb))
}

/// `HumanLedge__TryFreeHangDropToClimb` 0xDDF390 (free hang, stick down, no pending step). From the hands' midpoint
/// (higher hand) a hold row 1.2 m below and 0.65 m ahead (sub_11713A0: r 0.3, 0.65 / 0.6, 45°, filter
/// dword_193419C), then a second row 1.2 m below that and 0.3 m further ahead (r 0.3, 0.65 / 0.3):
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
/// PORT: rows are guidance probes (r 0.3, vertical 0.65 / 0.3); hands both on the upper hit (the climb) or spread
/// by HAND_SPACING (the hang); the lower row may not be the edge hung from; clearance = capsule test at the target.
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
    let current = guidance.on_edge((hand_l + hand_r) * 0.5, n, 0.1).map(|h| h.edge);
    let upper = guidance
        .probe(base - Vec3::Y * 1.2 + facing * 0.65, 0.3, 0.65, Some(facing), 0.785)
        .filter(|h| Some(h.edge) != current && h.point.y < base.y - VSTEP_MIN)?;
    let nn = upper.wall_normal;
    let lower = guidance
        .probe(upper.point - Vec3::Y * 1.2 + facing * 0.3, 0.3, 0.3, Some(facing), 0.785)
        .filter(|h| Some(h.edge) != current && h.point.y < upper.point.y - VSTEP_MIN);
    let new_facing = -Vec3::new(nn.x, 0.0, nn.z).normalize_or_zero();
    let (ids, to, hl, hr, feet_z, climb) = if let Some(lower) = lower {
        let to = super::climb::climb_root_at(upper.point, upper.point, lower.point, lower.point, nn);
        let e = ClimbEntry {
            entry_type: ClimbEntryType::FromLedge,
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
        let (fl, fr) = super::ledge::feet_on_wall(upper.point, nn, collision);
        if !(fl && fr) {
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
    let hit = guidance.probe_box(centre, right_of(facing), facing, Vec3::Y, Vec3::new(0.8, 0.5, 0.5), centre, std::f32::consts::PI, 0.1, 8)?;
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
pub fn climb_out_moves(two_m: bool, from: Vec3, hand_l: Vec3, hand_r: Vec3, n: Vec3) -> (LedgeMove, LedgeMove) {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mid = (hand_l + hand_r) * 0.5;
    let id = CLIMB_OUT[two_m as usize];
    let seq = [single(id, 0), single(id, 1), None, None];
    let durations = seq_durations(&seq);
    let durations = if durations.iter().sum::<f32>() > 0.0 { durations } else { [PULLUP_WALL_TIME * 0.5, 0.0, 0.0, 0.0] };
    let knee = mid + facing * 0.1;
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
/// Running jump onto a free-hang ledge (type 0x80): the swing cycle (0xE07D00, Ledge SubState 8).
pub const SWING_RECEPTION: u32 = 0x023E_0C60;

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

/// How a jump at a ledge is received when it arrives (CheckJumpTargetArrival 0xE07D00).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LedgeArrival {
    /// From the standing straight jump (0xB21DA0).
    Straight(HangJumpIn),
    /// From a running jump (0xB20200) onto a ledge: wall reception (type 0x40) or the swing (0x80).
    Surface { free: bool },
}

/// The Ledge context's entry move for an arrival: the reception actions while the root goes from `from` to
/// the hang root (or onto the top for knee / waist heights), following the clips' displacement plus a
/// correction (receptions are FROMANIM; the game interpolates the root over the action, 0xE07D00 →
/// sub_711130).
pub fn arrival_move(arr: LedgeArrival, from: Vec3, hand_l: Vec3, hand_r: Vec3, n: Vec3, collision: &CollisionWorld) -> LedgeMove {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mid = (hand_l + hand_r) * 0.5;
    let top = Vec3::new(mid.x, mid.y, mid.z) - n * PULLUP_IN;
    let item = |id: u32, i: usize, w: &[f32]| jump_blend::action_items(id).filter(|it| it.len() > i).map(|_| ActionBlend::new(id, i, w));
    let (seq, to, end_free, end_stand) = match arr {
        LedgeArrival::Straight(j) => {
            let w = [1.0 - j.b, j.b];
            match j.end {
                HangEnd::Hang(h) => {
                    let to = hang_root_at(hand_l, hand_r, n, if h == LedgeHangType::Free || hang_type_at(mid, n, collision) == LedgeHangType::Free { LedgeHangType::Free } else { LedgeHangType::Wall }, collision);
                    ([item(j.reception, 0, &w), item(j.reception, 1, &w), None, None], to, h == LedgeHangType::Free, false)
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
            // 0xE02790 weights the 6 clips by the wall angle (straight / 30 out / 45 in) and min / max;
            // the port's ledges are straight; (hypothesis) min
            let w = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
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
/// [(1−h)(1−d), h(1−d), d(1−h), d·h] with h = clamp(target hands − root − 2.0, 0, 1) and d = the horizontal
/// distance from the current hands to the target, clamped to [0, 1]; the root goes to the hang over that action
/// (sub_711130) and the Ledge context starts in a free hang (the action's exit is the free-hang wait 0x012719F1).
pub fn climb_jump_move(from: Vec3, cur_hands: Vec3, hand_l: Vec3, hand_r: Vec3, n: Vec3, collision: &CollisionWorld) -> LedgeMove {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mid = (hand_l + hand_r) * 0.5;
    let h = (hand_l.y.max(hand_r.y) - from.y - 2.0).clamp(0.0, 1.0);
    let d = Vec2::new(mid.x - cur_hands.x, mid.z - cur_hands.z).length().clamp(0.0, 1.0);
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
        follow_disp: found,
        lead: 0.0,
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
    let n = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let facing_out = n;
    let facing_in = -n;
    let r = right_of(facing_in);
    let hands = guidance.on_edge(p - r * 0.25, n, 0.3).zip(guidance.on_edge(p + r * 0.25, n, 0.3));
    let Some((hl, hr)) = hands.map(|(a, b)| (a.point, b.point)) else {
        // ReleaseToInAir: the orientation plays, then the let-go
        let seq = [single(orient_id, 0), None, None, None];
        let durations = seq_durations(&seq);
        return Err(LedgeMove {
            kind: MoveKind::PullDown { stage: 4 },
            seq,
            durations: if durations.iter().sum::<f32>() > 0.0 { durations } else { [GRAB_TIME, 0.0, 0.0, 0.0] },
            t: 0.0,
            from,
            to: p + n * 0.5,
            facing_from: facing_out,
            facing_to: facing_in,
            follow_disp: false,
            lead: 0.0,
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
            end_free,
            end_wall,
            end_stand: false,
            hand_l: hl,
            hand_r: hr,
            normal: n,
        }
    };
    let item = |id: u32, i: usize, w: &[f32]| jump_blend::action_items(id).filter(|it| it.len() > i).map(|_| ActionBlend::new(id, i, w));
    let p1 = Vec3::new(mid.x, mid.y, mid.z) + n * 0.5;
    let orient = mk(1, [single(orient_id, 0), None, None, None], from, p1, facing_out, facing_in, false, false);
    let p2 = mid + n * 1.0 - Vec3::Y * 0.8;
    let descent = mk(2, [single(descent_id, 0), None, None, None], p1, p2, facing_in, facing_in, false, false);
    let wall = hang_type_at(mid, n, collision) == LedgeHangType::Wall;
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
