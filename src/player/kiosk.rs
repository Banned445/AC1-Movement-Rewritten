//! HumanKiosk (context 19): the market-stall frames ("kiosks") jumped through on the way down from the roofs
//! (RE/18 §4).
//!
//! - **Entry** (InAir `CheckJumpTargetArrival` 0xE07D00, a target of type 0x400 = a Kiosk guidance piece): the root
//!   is interpolated to the target over 0.067 s; InAir+228 = entry type FreeStep (0), +232 = the side from the
//!   kiosk's axis and the jump direction: within 10 degrees along the axis (dot > 0.9848) LateralLeft, against it
//!   LateralRight, otherwise FrontLeft / FrontRight. `HumanInAir__RequestKiosk` 0xE015A0 → context 19
//!   (`TransitionSetupDataToHumanKiosk`).
//! - **Actions** (table 0xE3DAE0 at 0x1A2F280, index 2·side + entry type; the two entry types share their clips):
//!   stage 0 the entry, stage 1 the monkey bar (front sides only), stage 2 its `_tr_fall` exit.
//! - **Front** (0xE3E320 → 0xE3E390): the entry plays to its end, then one monkey-bar cycle (0xE3DD40), then InAir
//!   with the exit action (0xE3DDB0).
//! - **Lateral**: the side free step, then a free-step jump out (0xE3BF90: the candidate query along the kiosk's
//!   direction with beams and the scorer, else the free-jump target; `sub_B27180` kind 1).

use bevy::prelude::*;

use super::air::InAirEntry;
use super::jump_blend::ActionBlend;
use super::{switch_context, ActorContextId, Body, HumanDataBundle, Locomotion, Player, TransitionSetup};
use crate::collision::CollisionWorld;
use crate::guidance::{GuidanceSubType, GuidanceWorld};

/// [stage][side] (FrontLeft, FrontRight, LateralLeft, LateralRight); 0 = none (0x1A2F280, -1 in the game).
pub const KIOSK_ACTIONS: [[u32; 4]; 3] = [
    [0x1E5E_6287, 0x2084_B612, 0x2288_984A, 0x2288_984B],
    [0x1E5E_6288, 0x2084_B613, 0, 0],
    [0x1E5E_6289, 0x2084_B614, 0, 0],
];
/// The lateral exit's roll onto the ground (`sub_E3DA00`: `side_freestep_to_roll` 0x2288984C).
pub const SIDE_ROLL: u32 = 0x2288_984C;
/// The lateral exit's phase after which a ground contact rolls (`sub_E3D9A0` 0xE3D9BD, flt_17020DC).
pub const SIDE_ROLL_PHASE: f32 = 0.73;
/// Root interpolation to the kiosk target at arrival (`flt_1700FC4`).
pub const ARRIVAL_INTERP: f32 = 0.067;
/// cos 10 degrees (0x16FEBC0 / 0x16D90A0).
const LATERAL_COS: f32 = 0.984_807_7;

/// `HumanKioskEntrySide` (desc 0x195EE68).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KioskSide {
    #[default]
    FrontLeft,
    FrontRight,
    LateralLeft,
    LateralRight,
}

impl KioskSide {
    pub fn lateral(self) -> bool {
        matches!(self, KioskSide::LateralLeft | KioskSide::LateralRight)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct KioskEntry {
    pub from: Vec3,
    pub target: Vec3,
    /// The kiosk's axis (the entity's forward). PORT: taken along its Kiosk guidance piece.
    pub axis: Vec3,
    /// The jump's direction.
    pub dir: Vec3,
    /// The player's facing at the arrival (the entity's forward row, `sub_4C2F90`).
    pub facing: Vec3,
}

/// The entry side (0xE08672): `d` = axis·dir (the kiosk entity's forward row against the jump direction);
/// d > cos 10° → LateralLeft, d < −cos 10° → LateralRight; otherwise axis·facing (the player's forward row) < 0 →
/// FrontLeft, else FrontRight. Verified live (RE/18 §4): the kiosk's forward runs along the bar; a front jump came in
/// exactly across it (d = 0.000, facing·axis 0.54 → FrontRight), a side jump at d = −0.987 → LateralRight.
pub fn entry_side(axis: Vec3, dir: Vec3, facing: Vec3) -> KioskSide {
    let a = Vec3::new(axis.x, 0.0, axis.z).normalize_or_zero();
    let d = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    let k = a.dot(d);
    if k > LATERAL_COS {
        KioskSide::LateralLeft
    } else if k < -LATERAL_COS {
        KioskSide::LateralRight
    } else if a.dot(Vec3::new(facing.x, 0.0, facing.z)) < 0.0 {
        KioskSide::FrontLeft
    } else {
        KioskSide::FrontRight
    }
}

/// The kiosk's axis at a target point: its nearest Kiosk guidance piece.
pub fn kiosk_axis(p: Vec3, guidance: &GuidanceWorld) -> Option<Vec3> {
    guidance
        .edges
        .iter()
        .filter(|e| e.subtype == GuidanceSubType::Kiosk)
        .map(|e| (e.closest_point(p).distance(p), e))
        .filter(|(d, _)| *d < 0.5)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, e)| Vec3::new(e.p1.x - e.p0.x, 0.0, e.p1.z - e.p0.z).normalize_or_zero())
}

#[derive(Debug, Default)]
pub struct HumanKioskData {
    pub side: KioskSide,
    /// 0 entry, 1 monkey bar.
    pub stage: usize,
    pub action: Option<ActionBlend>,
    pub t: f32,
    pub interp: super::RootInterp,
    pub heading: f32,
    pub applied: [f32; 3],
    pub axis: Vec3,
    pub dir: Vec3,
    pub seq: u32,
    /// The root at the start of the playing action (its displacement is added to it).
    pub base: Vec3,
}

fn action(id: u32) -> Option<ActionBlend> {
    (id != 0).then_some(())?;
    super::jump_blend::action_items(id).filter(|i| !i.is_empty()).map(|_| ActionBlend::new(id, 0, &[1.0]))
}

impl HumanKioskData {
    pub fn enter(&mut self, e: KioskEntry) {
        self.seq = self.seq.wrapping_add(1);
        self.side = entry_side(e.axis, e.dir, e.facing);
        self.stage = 0;
        self.axis = e.axis;
        self.dir = Vec3::new(e.dir.x, 0.0, e.dir.z).normalize_or(Vec3::NEG_Z);
        self.heading = super::heading_of(self.dir);
        self.action = action(KIOSK_ACTIONS[0][self.side as usize]);
        self.t = 0.0;
        self.applied = [0.0; 3];
        self.interp = super::RootInterp::new(e.from, e.target, ARRIVAL_INTERP);
        self.base = e.target;
    }
}

pub fn update_kiosk(
    time: Res<Time>,
    collision: Res<CollisionWorld>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (mut loco, mut body, mut data) in &mut q {
        if loco.current != ActorContextId::Kiosk {
            continue;
        }
        if loco.just_switched {
            loco.just_switched = false;
            if loco.switched_this_frame {
                continue;
            }
        }
        let k = &mut data.kiosk;
        body.velocity = Vec3::ZERO;
        body.grounded = false;
        body.heading = k.heading;
        // the arrival's root interpolation, then the action's root motion (FROMANIM) from the target
        let start = if k.interp.done() { k.base } else { k.interp.advance(dt).0 };
        k.t += dt;
        let fwd = Vec3::new(-k.heading.sin(), 0.0, -k.heading.cos());
        let (dur, disp) = match k.action {
            Some(a) => (a.duration(), a.disp((k.t / a.duration().max(1e-4)).min(1.0))),
            None => (0.0, [0.0; 3]),
        };
        body.feet = start + super::right_of(fwd) * disp[0] + fwd * disp[1] + Vec3::Y * disp[2];
        // the lateral side (state +113, `sub_E3E3F0`; RE/24 NARROW-18): past 73 % of the side entry with a controller
        // contact underfoot it rolls onto the ground (`sub_E3E0E0` → Ground with `sub_E3DA00`: `side_freestep_to_roll`);
        // at its end without one, it falls (`sub_E3DE20`). The "jump out" 0xE3BF90 belongs to HumanRiding.
        if k.side.lateral() && k.interp.done() {
            let phase = if dur > 1e-4 { k.t / dur } else { 1.0 };
            let ground = collision.ground_height(body.feet + Vec3::Y * 0.05, 0.1);
            if phase > SIDE_ROLL_PHASE {
                if let Some(y) = ground {
                    body.feet.y = y;
                    body.grounded = true;
                    let landing = super::air::Landing {
                        kind: super::air::LandingType::Safe,
                        damage: 0,
                        fall_height: 0.0,
                        total_drop: 0.0,
                        roll: true,
                        action: Some(ActionBlend::new(SIDE_ROLL, 0, &[1.0])).filter(|a| a.duration() > 0.0),
                        skip: 0.0,
                    };
                    switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: Some(landing) });
                    continue;
                }
            }
            if k.t >= dur {
                // PORT: the InAir set-up's fields 9 / 3 (`sub_E3DE20`) are not decoded: a plain fall
                let here = body.feet;
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::Fall { from: here, velocity: Vec3::ZERO, origin: super::air::FallOrigin::Ground, speed_param: 0.0 }));
            }
            continue;
        }
        if k.t < dur || !k.interp.done() {
            continue;
        }
        let here = body.feet;
        if k.stage == 0 {
            // the entry ended: one monkey-bar cycle (0xE3E320 → 0xE3DD40)
            k.stage = 1;
            k.base = here;
            k.t = 0.0;
            k.applied = [0.0; 3];
            k.action = action(KIOSK_ACTIONS[1][k.side as usize]);
            k.seq = k.seq.wrapping_add(1);
            continue;
        }
        // the monkey bar ended: InAir with the exit action (0xE3E390 → 0xE3DDB0)
        let exit = action(KIOSK_ACTIONS[2][k.side as usize]);
        let entry = match exit {
            Some(a) => InAirEntry::OnPlace { from: here, fwd, action: a, fall: None },
            None => InAirEntry::Fall { from: here, velocity: Vec3::ZERO, origin: super::air::FallOrigin::Ground, speed_param: 0.0 },
        };
        switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sides_follow_the_kiosk_axis() {
        assert_eq!(entry_side(Vec3::Z, Vec3::Z, Vec3::Z), KioskSide::LateralLeft);
        assert_eq!(entry_side(Vec3::Z, Vec3::NEG_Z, Vec3::NEG_Z), KioskSide::LateralRight);
        assert!(!entry_side(Vec3::Z, Vec3::X, Vec3::X).lateral());
        // across the bar, the facing's lean along the axis picks the front side
        assert_eq!(entry_side(Vec3::Z, Vec3::X, Vec3::new(1.0, 0.0, 0.5)), KioskSide::FrontRight);
        assert_eq!(entry_side(Vec3::Z, Vec3::X, Vec3::new(1.0, 0.0, -0.5)), KioskSide::FrontLeft);
    }
}
