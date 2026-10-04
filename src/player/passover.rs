//! Pass-over: vaulting a thin wall with a hand plant (RE/04 §4.1.13).

/// Flights to a pass-over target (type 2), [footl, footr]: 5 clips [front 050/300/550, up 050/300]
/// `xx_h_air_*_foot{l,r}_to_passover` (0xB1EC40).
pub const FLIGHT_PASSOVER: [u32; 2] = [0x09A0_A58D, 0x09A0_A58E];
/// Arrival receptions (0xE07D00 case 2) after a [footl, footr] flight: `…_to_passover_tr_passover_hand{r,l}`.
pub const RECEPTION_PASSOVER: [u32; 2] = [0x0A4C_9776, 0x0B56_40DB];
/// The vault (HandPassOver, `HumanLedge__HandPassOver_Update` 0xDDB800): [hand l, hand r], 2 clips
/// `xx_h_passover_hand{l,r}_{030,100}cm` blended by the wall-top thickness.
pub const VAULT: [u32; 2] = [0x09A0_A7E1, 0x09A0_A7E2];
/// No floor beyond: InAir with `passover_hand{l,r}_{030,100}cm_tr_fall` (0xDDBC60).
pub const VAULT_TO_FALL: [u32; 2] = [0x0109_B7C8, 0x0109_BB53];

pub const DUMPED_ACTIONS: &[u32] = &[
    FLIGHT_PASSOVER[0], FLIGHT_PASSOVER[1], RECEPTION_PASSOVER[0], RECEPTION_PASSOVER[1], VAULT[0], VAULT[1], VAULT_TO_FALL[0], VAULT_TO_FALL[1],
];

use bevy::prelude::*;

use super::jump_blend::{self, ActionBlend};
use crate::collision::CollisionWorld;
use crate::guidance::{GuidanceSubType, GuidanceWorld};

/// The arrival on a pass-over target (0xE07D00 case 2).
#[derive(Clone, Copy, Debug)]
pub struct PassOverEntry {
    /// Near edge point on the wall top, and its outward normal (toward where the jump came from).
    pub edge: Vec3,
    pub normal: Vec3,
    pub from: Vec3,
    pub flight_foot_left: bool,
    pub flight_w: [f32; 5],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassOverPhase {
    /// The reception (root to target + 0.5 m forward over 0.067 s).
    Reception,
    /// HandPassOverSubState 1 (PassOver): the vault across the top.
    Vault,
}

#[derive(Clone, Copy, Debug)]
pub struct HandPassOver {
    pub edge: Vec3,
    pub fwd: Vec3,
    pub phase: PassOverPhase,
    pub action: ActionBlend,
    pub t: f32,
    from: Vec3,
    to: Vec3,
    warp: f32,
    /// Thickness blend (LedgeData+308): 30 cm ↔ 100 cm.
    pub w: f32,
    /// The vault's hand (0 left, 1 right).
    pub hand: usize,
    pub flight_foot_left: bool,
    pub seq: u32,
}

/// 0xE07D00 case 2 → Ledge SubState 12: the reception after a [footl, footr] flight with the flight's weights; the
/// root goes to the target + 0.5·forward = the edge (the target sits 0.5 m before it, 0xB1EC40) over 0.067 s.
pub fn enter(e: PassOverEntry, seq: u32) -> Option<HandPassOver> {
    let fwd = -Vec3::new(e.normal.x, 0.0, e.normal.z).normalize_or_zero();
    let id = RECEPTION_PASSOVER[(!e.flight_foot_left) as usize];
    jump_blend::action_items(id)?;
    let action = ActionBlend::new(id, 0, &e.flight_w);
    Some(HandPassOver { edge: e.edge, fwd, phase: PassOverPhase::Reception, action, t: 0.0, from: e.from, to: e.edge, warp: 0.067, w: 0.0, hand: e.flight_foot_left as usize, flight_foot_left: e.flight_foot_left, seq: seq.wrapping_add(1) })
}

/// `sub_B18FB0` (as used by 0xDDB800): the wall top along `fwd` from the near edge: the far LedgeGrab edge (normal
/// along +fwd, same height) within 1.8 m. Returns (far edge point, thickness).
pub fn far_edge(edge: Vec3, fwd: Vec3, guidance: &GuidanceWorld) -> Option<(Vec3, f32)> {
    guidance
        .edges
        .iter()
        .filter(|f| f.subtype == GuidanceSubType::LedgeGrab && Vec3::new(f.n1.x, 0.0, f.n1.z).normalize_or_zero().dot(fwd) > 0.9 && (f.p0.y - edge.y).abs() < 0.1)
        .map(|f| f.closest_point(edge + fwd * 0.9))
        .map(|q| (q, Vec3::new(q.x - edge.x, 0.0, q.z - edge.z).dot(fwd)))
        .filter(|(q, d)| *d > 0.02 && *d <= 1.8 && Vec2::new(q.x - (edge.x + fwd.x * d), q.z - (edge.z + fwd.z * d)).length() < 0.1)
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

pub enum PassOverOut {
    Stay,
    /// Vault done (release, 0xDE0720): Ground at the root (fill 0xDCFA40).
    Ground,
    /// No floor under the root: InAir with `passover_*_tr_fall` [1 − w, w] (0xDDBC60).
    Fall(ActionBlend),
}

/// `HumanLedge__HandPassOver_Update` 0xDDB800 + `StateHandPassOver_Update` 0xDE0720.
pub fn update(p: &mut HandPassOver, dt: f32, feet: &mut Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> PassOverOut {
    p.t += dt;
    let k = (p.t / p.warp.max(1e-3)).min(1.0);
    *feet = p.from.lerp(p.to, k);
    match p.phase {
        PassOverPhase::Reception => {
            if k < 1.0 {
                return PassOverOut::Stay;
            }
            // warp done → the vault by the leading foot of the reception (footl flight → `tr_passover_handr` →
            // hand r); thickness blend w = min(thickness, 1); root to the far edge + 0.3·(1 − w)·forward over the
            // vault (hypothesis: the point sub_B0FC50 derives from the probe is the far edge); none → edge + 0.3·fwd
            let (to, w) = match far_edge(p.edge, p.fwd, guidance) {
                Some((far, thick)) => {
                    let w = thick.min(1.0);
                    (far + p.fwd * 0.3 * (1.0 - w), w)
                }
                None => (p.edge + p.fwd * 0.3, 0.0),
            };
            let hand = if p.flight_foot_left { 1 } else { 0 };
            let Some(_) = jump_blend::action_items(VAULT[hand]) else { return PassOverOut::Ground };
            p.action = ActionBlend::new(VAULT[hand], 0, &[1.0 - w, w]);
            p.hand = hand;
            p.w = w;
            p.phase = PassOverPhase::Vault;
            p.from = *feet;
            p.to = Vec3::new(to.x, p.edge.y, to.z);
            p.warp = p.action.duration().max(0.05);
            p.t = 0.0;
            p.seq = p.seq.wrapping_add(1);
            PassOverOut::Stay
        }
        PassOverPhase::Vault => {
            // PORT: the release point (sub_5017B0) is taken as the action's end
            if p.t < p.action.duration() {
                return PassOverOut::Stay;
            }
            if collision.ground_height(*feet + Vec3::Y * 0.05, 0.4).is_some() {
                PassOverOut::Ground
            } else {
                PassOverOut::Fall(ActionBlend::new(VAULT_TO_FALL[p.hand], 0, &[1.0 - p.w, p.w]))
            }
        }
    }
}
