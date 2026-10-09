//! HumanDead (context 20), its drowning state (RE/18 §5).
//!
//! - **Entry** (`HumanDead__Enter` 0xE3FDC0 → 0xE3EF70): HumanDeadData+12 picks the drown state (+103) over the
//!   plain death (+100). Every transition into Dead that the port knows sets it: Ground event 113 (0xDB9890,
//!   `TransitionSetupDataToDead` with the water entity) and InAir event 2 (0xE09C20 / 0xE0A740 → 0xE00690). Their
//!   common sender is the damage system (`DNADamageType_Drown`), fed by the levels' water; not traced (`PORT`).
//! - **Drown** (`HumanDead__EnterDrown` 0xE3F9C0): the action by the up axis of a bone (`sub_E3F3B0`): within 45
//!   degrees of upright `0x27CE2279` (`xx_fall_tr_drowning`, then the `xx_drowning` loop), lying face down / up the
//!   `xx_drown_hurt_fall_{front,back}_tr_drowning` variants (`0x47593E69` / `0x47593E6A`); the `_dead` variants
//!   (`0x828C43DD`..DF) need the actor to be dead already. With the water height known (+13) the root is
//!   interpolated to it over 0.2 s (`sub_711130`); ActorState 45 (Drown).

use bevy::prelude::*;

use super::jump_blend::ActionBlend;
use super::{ActorContextId, Body, HumanDataBundle, Locomotion, Player};

/// `0x27CE2279`, `0x47593E69` (front), `0x47593E6A` (back) (`sub_E3F3B0`, Human_Environment block).
pub const DROWN: u32 = 0x27CE_2279;
pub const DROWN_FRONT: u32 = 0x4759_3E69;
pub const DROWN_BACK: u32 = 0x4759_3E6A;
/// The root interpolation onto the water surface (0xE3F9C0).
pub const SURFACE_INTERP: f32 = 0.2;

#[derive(Clone, Copy, Debug)]
pub struct DeadEntry {
    pub drown: bool,
    /// The water surface height (HumanDeadData +8 / +13).
    pub water_y: Option<f32>,
    pub from: Vec3,
    /// The axis the game reads from a bone (its global pose row, 0xE3F3B0); PORT: the body's forward (with its tilt).
    pub axis: Vec3,
}

#[derive(Debug, Default)]
pub struct HumanDeadData {
    pub drown: bool,
    pub action: Option<ActionBlend>,
    pub t: f32,
    pub interp: super::RootInterp,
    pub seq: u32,
}

/// `sub_E3F3B0` for a living actor: the bone axis within 45 degrees of level (|up part| < 0.7071) → the plain
/// drowning; pointing down → face down (front), up → face up (back).
pub fn drown_action(axis: Vec3) -> u32 {
    if axis.y.abs() < std::f32::consts::FRAC_1_SQRT_2 {
        DROWN
    } else if axis.y >= -std::f32::consts::FRAC_1_SQRT_2 {
        DROWN_BACK
    } else {
        DROWN_FRONT
    }
}

impl HumanDeadData {
    pub fn enter(&mut self, e: DeadEntry) {
        self.seq = self.seq.wrapping_add(1);
        self.drown = e.drown;
        self.t = 0.0;
        let id = drown_action(e.axis);
        self.action = super::jump_blend::action_items(id).filter(|i| !i.is_empty()).map(|_| ActionBlend::new(id, 0, &[1.0]));
        let to = e.water_y.map(|y| Vec3::new(e.from.x, y, e.from.z)).unwrap_or(e.from);
        self.interp = super::RootInterp::new(e.from, to, SURFACE_INTERP);
    }
}

pub fn update_dead(time: Res<Time>, spawn: Option<Res<super::SpawnPoint>>, mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle), With<Player>>) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (mut loco, mut body, mut data) in &mut q {
        if loco.current != ActorContextId::Dead {
            continue;
        }
        if loco.just_switched {
            loco.just_switched = false;
            continue;
        }
        let d = &mut data.dead;
        d.t += dt;
        // PORT: the game desynchronises and reloads (not ported); the port respawns once the drowning loop has played
        // through once, as its fatal landings respawn at once
        if d.action.is_some_and(|a| a.item > 0 && d.t >= a.duration()) {
            body.feet = spawn.as_ref().map(|s| s.0).unwrap_or(body.feet);
            body.velocity = Vec3::ZERO;
            body.grounded = true;
            super::switch_context(&mut loco, &mut data, super::TransitionSetup::ToMovement { landing: None });
            continue;
        }
        // the action's items in turn: the fall into the water, then the drowning (its last item loops)
        if let Some(a) = d.action {
            let n = super::jump_blend::action_items(a.id).map(|i| i.len()).unwrap_or(1);
            if d.t >= a.duration() && a.item + 1 < n {
                d.t -= a.duration();
                d.action = Some(ActionBlend::new(a.id, a.item + 1, &[1.0]));
                d.seq = d.seq.wrapping_add(1);
            }
        }
        body.velocity = Vec3::ZERO;
        body.grounded = false;
        body.feet = d.interp.advance(dt).0;
    }
}

/// PORT: the drown trigger. The game's sender (the damage system's drown damage from the levels' water) is not
/// traced; the port drowns the body once its feet are inside a deep-water volume (`GuidanceWorld::water`, the box's
/// top = the surface).
pub fn in_deep_water(feet: Vec3, water: &[crate::collision::Aabb3]) -> Option<f32> {
    water
        .iter()
        .find(|w| feet.x >= w.min.x && feet.x <= w.max.x && feet.z >= w.min.z && feet.z <= w.max.z && feet.y < w.max.y && feet.y >= w.min.y - 0.5)
        .map(|w| w.max.y)
}
