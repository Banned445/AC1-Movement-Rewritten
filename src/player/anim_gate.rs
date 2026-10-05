//! The game's per-item animation gates (RE/13 §7.6): every `ActionItem` carries a word at +60 whose bits the
//! locomotion contexts read for the item playing on the main channel (`sub_5021F0(0)` → `sub_723E00`).
//! - 0x10: the animation turns the body; the code heading is not applied (`HumanGround__UpdateHeading` 0xD95290,
//!   0xD7CA00).
//! - 0x20: locked; no context starts a new move or mode while it plays (`HumanGround__AnimAllowsModeExit`
//!   0xD80010, `HumanClimb__CanStartGridMove` 0xDE7FA0, `CanStartReachMove` 0xDE8020, `WantsLedgeContext`
//!   0xDE8070, the ground's run stop guard 0xD7EC90, …).
//! - 0x40 / 0x80: the item may be left for standing (desired mode 0) in low / high profile; 0x200 / 0x800: for
//!   moving (desired mode 2). 0x100 / 0x400 belong to mode 1, which the player's `UpdateDesiredMoveMode` 0xD7D4B0
//!   never sets (0 or 2 only).
//! - 0x1000: set on the jump takeoffs (`*_to_air`); 0x2000 on the `*_tr_fall` items; 0x4000 / 0x8000 only in fight
//!   blocks (meanings not decoded).
//! - bits 0–1 / 2–3: the feet before / after (`ACTFeetPosition`: 1 left ahead, 2 right ahead, 3 parallel).

use super::item_flags::ITEM_FLAGS;
use super::jump_blend::ActionBlend;

pub const ANIM_TURNS: u16 = 0x10;
pub const LOCKED: u16 = 0x20;
pub const EXIT_STAND_LOW: u16 = 0x40;
pub const EXIT_STAND_HIGH: u16 = 0x80;
pub const EXIT_MOVE_LOW: u16 = 0x200;
pub const EXIT_MOVE_HIGH: u16 = 0x800;
pub const TAKEOFF: u16 = 0x1000;
pub const TO_FALL: u16 = 0x2000;

/// The item word of `item` of action `id` (the movement blocks' actions; `item_flags.rs`).
pub fn item_word(id: u32, item: usize) -> Option<u16> {
    let k = ITEM_FLAGS.binary_search_by_key(&id, |e| e.0).ok()?;
    ITEM_FLAGS[k].1.get(item).copied()
}

/// The word of a playing action blend.
pub fn word(b: &ActionBlend) -> Option<u16> {
    item_word(b.id, b.item)
}

/// The playing item is locked (0x20).
pub fn locked(b: &ActionBlend) -> bool {
    word(b).is_some_and(|w| w & LOCKED != 0)
}

/// The playing item turns the body itself (0x10): the code heading waits.
pub fn anim_turns(b: &ActionBlend) -> bool {
    word(b).is_some_and(|w| w & ANIM_TURNS != 0)
}

/// `HumanGround__AnimAllowsModeExit` 0xD80010: the playing item may be left for the desired mode (moving or
/// standing) in the given profile. False while locked. An action missing from the table counts as not allowing
/// it (the caller then waits for its end, as before).
pub fn allows_mode_exit(b: &ActionBlend, moving: bool, high: bool) -> bool {
    let Some(w) = word(b) else { return false };
    if w & LOCKED != 0 {
        return false;
    }
    let bit = match (moving, high) {
        (false, false) => EXIT_STAND_LOW,
        (false, true) => EXIT_STAND_HIGH,
        (true, false) => EXIT_MOVE_LOW,
        (true, true) => EXIT_MOVE_HIGH,
    };
    w & bit != 0
}
