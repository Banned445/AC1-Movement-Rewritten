//! Crouch movement division: 0xD84C10, 0xDAAD80, 0xDA6E90, 0xD8E340 (RE/02 §9.2).
use super::{
    ground_extras::*,
    jump_blend::{self, ActionBlend},
};
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CrouchMode {
    Wait,
    Start,
    Walk,
    Pivot { from: f32, to: f32, time: f32 },
}
#[derive(Clone, Copy, Debug)]
pub struct Crouch {
    pub mode: CrouchMode,
    pub action: ActionBlend,
    pub time: f32,
    pub entering: bool,
    pub hide: bool,
    pub sequence: u32,
    pub fade: f32,
}
#[derive(Clone, Copy, Debug)]
pub struct CrouchInput {
    pub request: bool,
    pub hide: bool,
    pub high: bool,
    pub moving: bool,
    pub want_heading: f32,
    pub can_stand: bool,
    pub anim_allows_exit: bool,
    pub blend_progress: Option<f32>,
    pub item_clock: Option<(f32, f32)>,
    /// Legs held (the interpreter's slower rates) and k of the turn band (`UpdateHeading` 0xD95290).
    pub legs: bool,
    pub turn_k: f32,
    /// HG+1780, the interpreter's quick reversal: the crouch walk pivots on it (0xDA6E90, RE/24 GROUND-9).
    pub quick_turn: bool,
}
impl Crouch {
    pub fn enter(foot: usize, hide: bool, sequence: u32) -> Self {
        Self {
            mode: CrouchMode::Wait,
            action: ActionBlend::new(
                if hide {
                    CROUCH_HIDE_ENTER[foot]
                } else {
                    CROUCH_WAIT[foot][0]
                },
                0,
                &[1.0],
            ),
            time: 0.0,
            entering: true,
            hide,
            sequence,
            fade: 0.4,
        }
    }
    /// Move -> crouch (0xDA76E0 / 0xD7EDA0): enter the walk directly, with a 0.3 s progressive blend.
    pub fn enter_moving(foot: usize, hide: bool, sequence: u32) -> Self {
        Self {
            mode: CrouchMode::Walk,
            action: ActionBlend::new(CROUCH_WALK, foot, &[1.0]),
            time: 0.0,
            entering: false,
            hide,
            sequence,
            fade: 0.3,
        }
    }
    pub fn phase(&self) -> f32 {
        (self.time / self.action.duration().max(0.0001)).min(1.0)
    }
    fn play(&mut self, mode: CrouchMode, id: u32, item: usize) {
        self.fade = 0.2;
        self.mode = mode;
        self.action = ActionBlend::new(id, item, &[1.0]);
        self.time = 0.0;
        self.sequence = self.sequence.wrapping_add(1);
    }
    /// Returns None to restore the normal Ground division. Capsule height is 1 m while Some.
    pub fn update(
        &mut self,
        input: CrouchInput,
        foot: usize,
        heading: &mut f32,
        dt: f32,
    ) -> Option<Vec3> {
        // 0xD97E30 caches the heading gap, then rotates before the division update.
        let pre_turn_gap = super::ground::heading_delta(input.want_heading, *heading);
        if matches!(self.mode, CrouchMode::Start | CrouchMode::Walk)
            && !super::anim_gate::anim_turns(&self.action)
        {
            // `UpdateHeading` 0xD95290 for every Movement child, crouch included: the interpreter's rates and band
            // (RE/24 GROUND-11)
            let step = crate::tuning::player_turn_rate(pre_turn_gap.abs(), input.legs, input.turn_k) * dt;
            *heading += pre_turn_gap.clamp(-step, step);
        }
        match self.mode {
            CrouchMode::Wait => {
                if !input.request && input.anim_allows_exit && input.can_stand {
                    return None;
                }
                if crouch_can_start(
                    input.moving,
                    self.entering,
                    input.blend_progress,
                    input.item_clock,
                ) {
                    self.play(CrouchMode::Start, CROUCH_START[foot], 0);
                    self.entering = false;
                } else if input.hide != self.hide {
                    self.hide = input.hide;
                    self.play(CrouchMode::Wait, CROUCH_WAIT[foot][self.hide as usize], 0);
                }
                // Wanted high profile's FreeRun transition is handled by the Ground owner.
            }
            CrouchMode::Start | CrouchMode::Walk => {
                if !input.moving {
                    self.play(CrouchMode::Wait, CROUCH_WAIT[foot][input.hide as usize], 0);
                    self.hide = input.hide;
                } else if (!input.request || input.high) && input.can_stand {
                    return None;
                } else {
                    if input.quick_turn || pre_turn_gap.abs() > 120f32.to_radians() {
                        let d = super::ground::heading_delta(input.want_heading, *heading);
                        self.play(
                            CrouchMode::Pivot {
                                from: *heading,
                                to: *heading + d,
                                time: 0.0,
                            },
                            CROUCH_PIVOT,
                            0,
                        );
                    }
                }
            }
            _ => {}
        }
        if let CrouchMode::Pivot { from, to, mut time } = self.mode {
            time = (time + dt).min(0.3);
            *heading = from + (to - from) * (time / 0.3);
            self.mode = CrouchMode::Pivot { from, to, time };
            if time >= 0.3 {
                self.play(CrouchMode::Wait, CROUCH_WAIT[foot][input.hide as usize], 0);
                self.hide = input.hide;
            }
            return Some(Vec3::ZERO);
        }
        // the simulation's root clock samples the selected item directly; while the animation blends into it from
        // another action the caller scales the displacement by the blend's shares (`anim::root_weight`, the game's
        // `BlendNode__GatherDisplacement` 0x774350; A's share stands for the same motion, RE/36 A8 / A20).
        let mut left = dt;
        let mut delta = Vec3::ZERO;
        while left > 0.000001 {
            let duration = self.action.duration().max(0.0001);
            let before = self.action.disp(self.phase());
            let step = left.min((duration - self.time).max(0.0));
            self.time += step;
            left -= step;
            let after = self.action.disp(self.phase());
            delta += Vec3::new(after[0] - before[0], 0.0, after[1] - before[1]);
            // Native action/item completion requires positive leftover time (0x730E50 / 0x778010).
            if self.time < duration || left <= 0.0 {
                break;
            }
            match self.mode {
                CrouchMode::Start => self.play(CrouchMode::Walk, CROUCH_WALK, foot),
                CrouchMode::Walk => {
                    let items =
                        jump_blend::action_items(CROUCH_WALK).map_or(2, |items| items.len());
                    self.action.item = (self.action.item + 1) % items;
                    self.time = 0.0;
                }
                _ => {
                    if CROUCH_HIDE_ENTER.contains(&self.action.id) {
                        self.play(CrouchMode::Wait, CROUCH_WAIT[foot][self.hide as usize], 0);
                    } else {
                        self.time = 0.0;
                    }
                }
            }
        }
        Some(delta)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> CrouchInput {
        CrouchInput {
            request: true,
            hide: false,
            high: false,
            moving: false,
            want_heading: 0.0,
            can_stand: true,
            anim_allows_exit: true,
            blend_progress: None,
            item_clock: None,
            legs: false,
            turn_k: 0.0,
            quick_turn: false,
        }
    }
    #[test]
    fn no_standing_through_a_ceiling() {
        let mut c = Crouch::enter(0, false, 0);
        let mut h = 0.0;
        assert!(c
            .update(
                CrouchInput {
                    request: false,
                    can_stand: false,
                    ..input()
                },
                0,
                &mut h,
                0.01
            )
            .is_some());
        assert!(c
            .update(
                CrouchInput {
                    request: false,
                    ..input()
                },
                0,
                &mut h,
                0.01
            )
            .is_none());
    }
    #[test]
    fn starting_waits_for_entry_progress_then_walks_and_stops() {
        let mut c = Crouch::enter(0, false, 0);
        let mut h = 0.0;
        c.update(
            CrouchInput {
                moving: true,
                blend_progress: Some(0.75),
                ..input()
            },
            0,
            &mut h,
            0.01,
        );
        assert_eq!(c.mode, CrouchMode::Wait);
        c.update(
            CrouchInput {
                moving: true,
                blend_progress: Some(0.751),
                ..input()
            },
            0,
            &mut h,
            0.01,
        );
        assert_eq!(c.mode, CrouchMode::Start);
        for _ in 0..120 {
            c.update(
                CrouchInput {
                    moving: true,
                    ..input()
                },
                0,
                &mut h,
                1.0 / 60.0,
            );
        }
        assert_eq!(c.mode, CrouchMode::Walk);
        c.update(input(), 1, &mut h, 0.01);
        assert_eq!(c.mode, CrouchMode::Wait);
        assert_eq!(c.action.id, CROUCH_WAIT[1][0]);
    }
    #[test]
    fn moving_exit_respects_ceiling_and_stop_has_priority() {
        let mut c = Crouch::enter_moving(0, false, 0);
        let mut h = 0.0;
        let moving = CrouchInput {
            moving: true,
            request: false,
            can_stand: false,
            ..input()
        };
        assert!(c.update(moving, 0, &mut h, 0.01).is_some());
        assert_eq!(c.mode, CrouchMode::Walk);
        assert!(c
            .update(
                CrouchInput {
                    moving: false,
                    ..moving
                },
                0,
                &mut h,
                0.01
            )
            .is_some());
        assert_eq!(c.mode, CrouchMode::Wait);
        assert!(c
            .update(
                CrouchInput {
                    request: false,
                    ..input()
                },
                0,
                &mut h,
                0.01
            )
            .is_none());
    }
    #[test]
    fn pivot_angle_is_strictly_above_one_hundred_twenty_degrees() {
        for (angle, pivot) in [
            (120f32.to_radians(), false),
            (120f32.to_radians() + 0.0001, true),
        ] {
            let mut c = Crouch::enter_moving(0, false, 0);
            let mut h = 0.0;
            c.update(
                CrouchInput {
                    moving: true,
                    want_heading: angle,
                    ..input()
                },
                0,
                &mut h,
                0.01,
            );
            assert_eq!(matches!(c.mode, CrouchMode::Pivot { .. }), pivot);
        }
    }
    #[test]
    fn pivot_interpolates_heading_for_point_three_seconds() {
        let mut c = Crouch::enter(0, false, 0);
        c.mode = CrouchMode::Walk;
        c.action = ActionBlend::new(CROUCH_WALK, 0, &[1.0]);
        let mut h = 0.0;
        let i = CrouchInput {
            moving: true,
            want_heading: std::f32::consts::PI,
            ..input()
        };
        c.update(i, 0, &mut h, 0.15);
        // the walk turns 4.0 rad/s x 0.15 s first (k = 0: the max rate), then the pivot covers half the rest
        assert!((h - 1.870796).abs() < 0.00001, "{h}");
        c.update(i, 0, &mut h, 0.15);
        assert_eq!(c.mode, CrouchMode::Wait);
        assert!((h - std::f32::consts::PI).abs() < 0.00001);
    }
}
