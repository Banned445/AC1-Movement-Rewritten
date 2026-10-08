//! Social-stealth request boundary (0xB7D8F0 / 0xB01A50 / 0xB01AB0 / 0xEE704C).
use super::{HumanDataBundle, Player};
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocialSignal {
    /// 0xB05360 bit 7: notify nearby actors through 0xB001C0; not a crouch request.
    NearbyDanger,
    Actor201(u8),
    Cleared,
}

/// A registered actor's SocialStealthHelper and the timed-status part of ToleranceManager.
/// PORT: world/script producers (including status 5) and inventory/AI authority are not imported yet.
#[derive(Component, Debug)]
pub struct SocialHelper {
    pub bits: u32,
    /// ToleranceManager vt40 (0xB02F00); supplied by the actor's native authority.
    pub allowed: bool,
    pub weapon_interface: bool,
    pub weapon_drawn: bool,
    pub sheath_requested: bool,
    pub owner_special: bool,
    pub pending: Vec<SocialSignal>,
    timed_bit: u8,
    expires: f64,
}
impl Default for SocialHelper {
    fn default() -> Self {
        Self {
            bits: 0,
            allowed: false,
            weapon_interface: true,
            weapon_drawn: false,
            sheath_requested: false,
            owner_special: false,
            pending: Vec::new(),
            timed_bit: 0,
            expires: 0.0,
        }
    }
}
impl SocialHelper {
    /// StatusBits constructor 0xB7D8F0: masks [~1544,1544,~1088], category 0 drives callbacks.
    pub fn category(&self, category: usize) -> bool {
        let mask = match category {
            0 => !1544u32,
            1 => 1544,
            2 => !1088u32,
            _ => 0,
        };
        self.bits & mask != 0
    }
    pub fn test(&self, bit: u8) -> bool {
        bit < 32 && self.bits & (1u32 << bit) != 0
    }
    fn change(&mut self, bit: u8, set: bool) {
        if bit == 0 || bit >= 32 {
            return;
        } // 0xB7D820 / 0xB7D840
        let before = self.category(0);
        if set {
            self.bits |= 1u32 << bit;
        } else {
            self.bits &= !(1u32 << bit);
        }
        let after = self.category(0);
        if before == after {
            return;
        }
        if !after {
            self.pending.push(SocialSignal::Cleared);
            return;
        }
        // Callback priority in 0xB05360; status 1's special-owner branch is supplied by its actor authority.
        let signal = if self.test(7) {
            Some(SocialSignal::NearbyDanger)
        } else if self.test(4) {
            Some(SocialSignal::Actor201(0))
        } else if self.test(1) {
            Some(SocialSignal::Actor201(if self.owner_special {
                4
            } else {
                1
            }))
        } else if self.test(5) {
            Some(SocialSignal::Actor201(3))
        } else if self.test(2) {
            Some(SocialSignal::Actor201(2))
        } else {
            None
        };
        if let Some(signal) = signal {
            self.pending.push(signal);
        }
    }
    /// ToleranceManager helper override 0xB01A50: bits 6/7 bypass the authority check.
    pub fn set(&mut self, bit: u8) {
        if self.allowed || bit == 6 || bit == 7 {
            self.change(bit, true);
        }
    }
    pub fn clear(&mut self, bit: u8) {
        self.change(bit, false);
    }
    /// 0xB01AB0: a live timer prevents replacement; expiration clears only its saved status.
    pub fn set_timed(&mut self, bit: u8, duration: f32, now: f64) {
        if (self.allowed || bit == 6 || bit == 7) && now >= self.expires {
            self.change(bit, true);
            self.timed_bit = bit;
            self.expires = now + duration as f64;
        }
    }
    pub fn advance(&mut self, now: f64) {
        if now >= self.expires && self.timed_bit != 0 {
            let bit = self.timed_bit;
            self.clear(bit);
            self.timed_bit = 0;
        }
    }
}

/// Interpreter 0xEE704C: bit 5 + sheathed weapon requests crouch/hide; no synthetic input button.
pub fn update_social(
    time: Res<Time>,
    mut q: Query<(&mut SocialHelper, &mut HumanDataBundle), With<Player>>,
) {
    if !crate::tuning::GAME_GROUND_EXTRAS {
        return;
    }
    for (mut helper, mut data) in &mut q {
        helper.advance(time.elapsed_secs_f64());
        if helper.test(5) {
            if helper.weapon_interface {
                if helper.weapon_drawn {
                    helper.sheath_requested = true;
                } else {
                    data.ground.crouch_requested = true;
                    data.ground.crouch_hide_hint = true;
                }
            }
        } else {
            data.ground.crouch_requested = false;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_masks_and_callbacks_follow_native_priority() {
        let mut s = SocialHelper {
            allowed: true,
            ..default()
        };
        s.set(3);
        assert!(!s.category(0));
        assert!(s.category(1));
        assert!(s.pending.is_empty());
        s.set(5);
        assert_eq!(s.pending, [SocialSignal::Actor201(3)]);
        s.set(4);
        assert_eq!(
            s.pending.len(),
            1,
            "callbacks only when category zero changes"
        );
        s.clear(5);
        assert_eq!(s.pending.len(), 1);
        s.clear(4);
        assert_eq!(s.pending.last(), Some(&SocialSignal::Cleared));
        s.set(0);
        assert!(!s.test(0));
        s.allowed = false;
        s.set(5);
        assert!(!s.test(5));
        s.set(7);
        assert_eq!(s.pending.last(), Some(&SocialSignal::NearbyDanger));
    }
    #[test]
    fn timed_status_cannot_be_replaced_and_expires_at_the_boundary() {
        let mut s = SocialHelper {
            allowed: true,
            ..default()
        };
        s.set_timed(5, 0.5, 1.0);
        s.set_timed(2, 5.0, 1.1);
        assert!(s.test(5));
        assert!(!s.test(2));
        s.advance(1.49999);
        assert!(s.test(5));
        s.advance(1.5);
        assert!(!s.test(5));
        s.set_timed(2, 1.0, 1.5);
        assert!(s.test(2));
        assert!(!s.test(5));
    }
}
