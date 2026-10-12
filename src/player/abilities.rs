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
//! - **The base set is the player's rank** (RE/24 CENSUS-9): DataPC.forge's stored files "Rank 0" … "Rank 9" each hold
//!   a rank object (class 0x76A91F10) with its `AssassinAbilitySet` embedded at +16 (class hash 0xBA317DB0), serialized
//!   as `MaxSpeed` (u32) then one byte per flag in reflection order (the 34 bits, `HighProfile`, `AllActions`, +0x15).
//!   Every rank allows every movement ability with `MaxSpeed` Sprint; the ranks differ in combat (Rank 0: no Fight,
//!   weapon, assassinations, counter or combo kills). Mission clips (`AbilitySetClip`) push further sets: no missions.
//!
//! The port loads the rank set from the install (`AC_RANK`, default 9; everything allowed without the install);
//! `AC_ABILITIES=<hex>` sets the flags and `AC_MAX_SPEED=<0..4>` the speed for tests.

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
    /// +0x14 bit 0: the high profile may be used (`AbilitySet__HighProfile` 0xD32480).
    pub high_profile: bool,
}

impl Default for AbilitySet {
    fn default() -> Self {
        let flags = std::env::var("AC_ABILITIES").ok().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()).unwrap_or(u64::MAX);
        let max_speed = std::env::var("AC_MAX_SPEED").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
        let high_profile = std::env::var_os("AC_NO_HIGH_PROFILE").is_none();
        Self { flags, max_speed, all_actions: true, high_profile }
    }
}

impl AbilitySet {
    /// `AbilityStack__Allows` 0xEF0320 with one set.
    pub fn allows(&self, a: Ability) -> bool {
        self.all_actions && self.flags & (1u64 << a as u32) != 0
    }
    /// The high profile allowed (`AbilityStack__Allows(AbilitySet__HighProfile)`): without it `ReadInput` 0xEEDEC4 forces
    /// the high-profile flag (+0x112B) off (RE/24 INPUT-10).
    pub fn allows_high_profile(&self) -> bool {
        self.all_actions && self.high_profile
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

/// The rank object's class and its embedded `AssassinAbilitySet`'s (CRC32 of the class names).
const CLASS_RANK: u32 = 0x76A9_1F10;
const CLASS_ABILITY_SET: u32 = 0xBA31_7DB0;
/// The ability bits the set serializes (reflection 0x19B55C8: bits 0 … 33).
const FLAG_COUNT: usize = 34;

/// An `AssassinAbilitySet` serialized at `d` (its class hash first).
pub fn parse_ability_set(d: &[u8]) -> Option<AbilitySet> {
    let u32_at = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if u32_at(0)? != CLASS_ABILITY_SET {
        return None;
    }
    let max_speed = u32_at(4)?.min(4) as u8;
    let bytes = d.get(8..8 + FLAG_COUNT + 2)?;
    let flags = bytes[..FLAG_COUNT].iter().enumerate().fold(0u64, |f, (i, b)| f | ((*b != 0) as u64) << i);
    Some(AbilitySet { flags, max_speed, high_profile: bytes[FLAG_COUNT] != 0, all_actions: bytes[FLAG_COUNT + 1] != 0 })
}

/// The ability set of rank `rank` from the install's DataPC.forge.
pub fn load_rank_set(dir: &std::path::Path, rank: u32) -> Option<AbilitySet> {
    let mut f = crate::assets::forge::Forge::open(&dir.join("DataPC.forge")).ok()?;
    let e = f.find(&format!("Rank {rank}"))?.clone();
    let r = f.resources(&e).ok()?.into_iter().find(|r| r.class_hash == CLASS_RANK)?;
    parse_ability_set(r.payload.get(16..)?)
}

/// Startup: the base set from the player's rank, unless the install is missing or `AC_ABILITIES` overrides it.
pub fn load_abilities(mut set: ResMut<AbilitySet>) {
    if std::env::var_os("AC_ABILITIES").is_some() || std::env::var_os("AC_MAX_SPEED").is_some() {
        return;
    }
    let rank = std::env::var("AC_RANK").ok().and_then(|s| s.parse().ok()).unwrap_or(9);
    match load_rank_set(&crate::assets::game_dir(), rank) {
        Some(s) => *set = AbilitySet { high_profile: s.high_profile && std::env::var_os("AC_NO_HIGH_PROFILE").is_none(), ..s },
        None => warn!("ability set of rank {rank} not loaded: everything allowed"),
    }
}

/// The abilities in effect: the resource when present, else everything allowed.
pub fn of(set: Option<&AbilitySet>) -> AbilitySet {
    set.copied().unwrap_or(AbilitySet { flags: u64::MAX, max_speed: 4, all_actions: true, high_profile: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flags_and_speed() {
        let s = AbilitySet { flags: 1 << Ability::Jump as u32, max_speed: 2, all_actions: true, high_profile: true };
        assert!(s.allows(Ability::Jump));
        assert!(!s.allows(Ability::Walling));
        assert_eq!(s.clamp_speed(0.9, true), 0.3);
        assert_eq!(s.clamp_speed(0.9, false), 0.9);
        let none = AbilitySet { all_actions: false, ..s };
        assert!(!none.allows(Ability::Jump));
        assert_eq!(none.clamp_speed(0.9, false), 0.0);
    }

    /// RE/24 CENSUS-9: every rank's set allows every movement ability at Sprint; Rank 0 lacks the combat ones.
    #[test]
    fn every_rank_allows_the_movement_abilities() {
        let dir = crate::assets::game_dir();
        let Some(r0) = load_rank_set(&dir, 0) else {
            eprintln!("skipped: game install not found");
            return;
        };
        for rank in 0..=9 {
            let s = load_rank_set(&dir, rank).expect("rank set");
            for a in [Ability::Jump, Ability::Crouch, Ability::PassOver, Ability::Walling, Ability::Climb, Ability::Ladder, Ability::Grasp, Ability::LeapOfFaith, Ability::LookDown] {
                assert!(s.allows(a), "rank {rank}: {a:?}");
            }
            assert!(s.max_speed == 4 && s.high_profile && s.all_actions, "rank {rank}: {s:?}");
        }
        // Fight (12) and the stealth assassination (9) only come later
        assert_eq!(r0.flags & (1 << 12 | 1 << 9), 0, "{:#x}", r0.flags);
        assert_eq!(load_rank_set(&dir, 9).unwrap().flags, (1u64 << 34) - 1);
    }
}
