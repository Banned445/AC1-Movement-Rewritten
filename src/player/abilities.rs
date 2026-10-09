//! The player's ability stack (`AssassinAbilitySet`, RE/18 §6): which actions the input interpreter may request.
//!
//! - **Layout** (reflection 0x19B55C8): +8 a 64-bit set of flags in declaration order, +0x10 `MaxSpeed`
//!   (0 NoMovementAllowed, 1 Walk, 2 Jog, 3 Run, 4 Sprint), +0x14 bit 0 HighProfile, bit 1 AllActions. The getters
//!   (0xD324C0 … 0xD32B80, one per bit, `shrd`/`shr` by the index) confirm the order: 0 Jump, 1 Crouch, 2 PassOver,
//!   3 Walling, 4 Climb, 5 Ladder, 6 Grasp, 15 LeapOfFaith, 29 LookDown.
//! - **The stack** (`AbilityStack__Allows` 0xEF0320): a base set and the top of a pushed list; an ability is allowed
//!   when every set has AllActions and the flag. `AbilityStack__GetMaxSpeed` 0xEF02B0: the smaller MaxSpeed (0 when a
//!   set lacks AllActions; 4 with no set).
//! - **MaxSpeed in the interpreter** (0xEE65A0, RE/01 §6): 0 → speed 0; Jog in high profile → speed ≤ 0.3.
//! - **PassOver** only gates the wall run's pass-over request (0xEE72DA sets interpreter +0x1128, passed to the
//!   wall-run start IHumanGround vt116 0xDBBD40 → WallingData +0x20); that exit is dead in v1.02, so nothing in the
//!   port reads it. Pass-over jump targets are not gated.
//!
//! PORT: the game's sets come from the player's progression and mission scripts (not ported). The port has one set,
//! everything allowed by default; `AC_ABILITIES=<hex>` sets the flags and `AC_MAX_SPEED=<0..4>` the speed for tests.

use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ability {
    Jump = 0,
    Crouch = 1,
    PassOver = 2,
    Walling = 3,
    Climb = 4,
    Ladder = 5,
    Grasp = 6,
    LeapOfFaith = 15,
    LookDown = 29,
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct AbilitySet {
    pub flags: u64,
    pub max_speed: u8,
    pub all_actions: bool,
}

impl Default for AbilitySet {
    fn default() -> Self {
        let flags = std::env::var("AC_ABILITIES").ok().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()).unwrap_or(u64::MAX);
        let max_speed = std::env::var("AC_MAX_SPEED").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
        Self { flags, max_speed, all_actions: true }
    }
}

impl AbilitySet {
    /// `AbilityStack__Allows` 0xEF0320 with one set.
    pub fn allows(&self, a: Ability) -> bool {
        self.all_actions && self.flags & (1u64 << a as u32) != 0
    }
    /// `AbilityStack__GetMaxSpeed` 0xEF02B0.
    pub fn max_speed(&self) -> u8 {
        if self.all_actions { self.max_speed } else { 0 }
    }
    /// The interpreter's clamp of the stick speed (0xEE65A0).
    pub fn clamp_speed(&self, speed01: f32, high_profile: bool) -> f32 {
        match self.max_speed() {
            0 => 0.0,
            2 if high_profile => speed01.min(0.3),
            _ => speed01,
        }
    }
}

/// The abilities in effect: the resource when present, else everything allowed.
pub fn of(set: Option<&AbilitySet>) -> AbilitySet {
    set.copied().unwrap_or(AbilitySet { flags: u64::MAX, max_speed: 4, all_actions: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flags_and_speed() {
        let s = AbilitySet { flags: 1 << Ability::Jump as u32, max_speed: 2, all_actions: true };
        assert!(s.allows(Ability::Jump));
        assert!(!s.allows(Ability::Walling));
        assert_eq!(s.clamp_speed(0.9, true), 0.3);
        assert_eq!(s.clamp_speed(0.9, false), 0.9);
        let none = AbilitySet { all_actions: false, ..s };
        assert!(!none.allows(Ability::Jump));
        assert_eq!(none.clamp_speed(0.9, false), 0.0);
    }
}
