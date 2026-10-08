//! Ground extras scalar rules and action IDs, verified in RE/02 §9.
use super::air::LandingType;
use bevy::prelude::*;

/// 0xDB6E50 tables; IDs 89 / 30 / 31 resolve through Action+8 (0x6E9A30).
pub const FREERUN_WAIT: [u32; 2] = [0x0DF0_3385, 0x0EED_41D3];
pub const STATIC_PREPARE: [u32; 2] = [0x00F0_A538, 0x0109_9C96];
pub const STATIC_JUMP: [u32; 2] = [0x00D8_1408, 0x0109_B1AC];
pub const STATIC_FALL: u32 = 0x00D8_144B;
pub const CROUCH_WAIT: [[u32; 2]; 2] = [[0x0106_E33F, 0x2949_AAAE], [0x0106_E340, 0x2949_AAAF]];
pub const CROUCH_START: [u32; 2] = [0x0106_E341, 0x0106_E342];
pub const CROUCH_WALK: u32 = 0x0106_E343;
pub const CROUCH_PIVOT: u32 = 0x12B9_D6F3;
pub const CROUCH_HIDE_ENTER: [u32; 2] = [0x2949_AAB4, 0x2949_AAB5];
pub const DUMPED_ACTIONS: &[u32] = &[
    FREERUN_WAIT[0],
    FREERUN_WAIT[1],
    STATIC_PREPARE[0],
    STATIC_PREPARE[1],
    STATIC_JUMP[0],
    STATIC_JUMP[1],
    STATIC_FALL,
    CROUCH_WAIT[0][0],
    CROUCH_WAIT[0][1],
    CROUCH_WAIT[1][0],
    CROUCH_WAIT[1][1],
    CROUCH_START[0],
    CROUCH_START[1],
    CROUCH_WALK,
    CROUCH_PIVOT,
    CROUCH_HIDE_ENTER[0],
    CROUCH_HIDE_ENTER[1],
];

/// 0xD7F640: the entry-transition flag gates a move until progress is strictly >0.75.
pub fn crouch_can_start(
    moving: bool,
    entering: bool,
    blend_progress: Option<f32>,
    item_time: Option<(f32, f32)>,
) -> bool {
    if !moving {
        return false;
    }
    if !entering {
        return true;
    }
    if let Some(progress) = blend_progress {
        return progress > 0.75;
    }
    item_time.is_none_or(|(time, phase)| time.abs() <= 0.0005 || phase > 0.75)
}

/// 0xDA35B0: upper standing capsule clearance; endpoints are sphere centres.
pub fn can_stand(feet: Vec3, world: &crate::collision::CollisionWorld) -> bool {
    world.capsule_cast_free(feet + Vec3::Y * 0.04, 1.76, 0.38, Vec3::ZERO)
}

/// 0xD9E5A0: a normalized direction toward the registered crowd actor, consumed once by 0xDA0810.
pub fn crowd_avoid(feet: Vec3, forward: Vec3, target: Vec3) -> Option<Vec3> {
    let to = (target - feet).with_y(0.0);
    if to.x.abs() <= 0.0005 && to.z.abs() <= 0.0005 {
        return None;
    }
    if to.length() >= 2.0 {
        return None;
    }
    let local = to;
    let front = local.dot(forward);
    let right = local.dot(super::right_of(forward));
    (front > -0.5 && front < 1.5 && right.abs() < 1.0).then(|| to.normalize())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroundContact {
    #[default]
    None,
    Flat,
    Steep,
    Wall,
}

/// 0xE038A0. The port's manifold contains static contacts only; character filtering belongs to actor collision.
pub fn ground_contact(normals: impl IntoIterator<Item = f32>) -> GroundContact {
    let mut result = GroundContact::None;
    for up in normals {
        if up >= std::f32::consts::FRAC_1_SQRT_2 {
            return GroundContact::Flat;
        }
        if up >= 70f32.to_radians().cos() {
            result = GroundContact::Steep;
        } else if result == GroundContact::None && up >= 135f32.to_radians().cos() {
            result = GroundContact::Wall;
        }
    }
    result
}

/// InAir+226 bit 0x10 and +64 position (0xE038A0 / 0xE03A20).
#[derive(Clone, Copy, Debug, Default)]
pub struct SlopeSlide {
    pub start_y: Option<f32>,
    pub ragfall_required: bool,
}
impl SlopeSlide {
    /// First steep contact is retained throughout this air entry, including gaps in contact.
    pub fn contact(&mut self, kind: GroundContact, y: f32) {
        if kind == GroundContact::Steep && self.start_y.is_none() {
            self.start_y = Some(y);
        }
    }
    pub fn drop(&self, y: f32) -> f32 {
        self.start_y.map_or(0.0, |start| start - y)
    }
    /// 0xE05200 steep branch: impact excludes distance already slid. Strict >10 m rag-fall gate.
    pub fn check(&mut self, total_height: f32, y: f32, dead: bool) -> (LandingType, u32) {
        let height = if self.start_y.is_some() {
            total_height - self.drop(y)
        } else {
            0.0
        };
        let damage = super::falls::landing_damage(height);
        self.ragfall_required |= dead || damage.0 == LandingType::Fatal || self.drop(y) > 10.0;
        damage
    }
}

/// 0xE97720: prefer orientation by >30 degrees, otherwise prefer the higher report.
pub fn choose_static_report(root: Vec3, forward: Vec3, reports: &[(Vec3, Vec3)]) -> Option<usize> {
    let mut best = None;
    let mut best_angle = std::f32::consts::PI;
    let mut best_height = 0.0;
    let margin = 30f32.to_radians();
    for (i, (point, normal)) in reports.iter().enumerate() {
        let angle = (-normal.with_y(0.0).normalize_or_zero())
            .dot(forward.with_y(0.0).normalize_or_zero())
            .clamp(-1.0, 1.0)
            .acos();
        let height = point.y - root.y;
        if best_angle + margin > angle && (best_angle - margin > angle || height > best_height) {
            best = Some(i);
            best_angle = angle;
            best_height = height;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_report_prefers_height_within_thirty_degrees_and_orientation_outside_it() {
        let root = Vec3::ZERO;
        let f = Vec3::NEG_Z;
        let n = |degrees: f32| Quat::from_rotation_y(degrees.to_radians()) * Vec3::Z;
        assert_eq!(
            choose_static_report(root, f, &[(Vec3::Y, n(0.0)), (Vec3::Y * 2.0, n(20.0))]),
            Some(1)
        );
        assert_eq!(
            choose_static_report(root, f, &[(Vec3::Y * 2.0, n(70.0)), (Vec3::Y, n(0.0))]),
            Some(1)
        );
        assert_eq!(choose_static_report(root, f, &[]), None);
    }
    #[test]
    fn crouch_entry_waits_for_strict_three_quarters() {
        for phase in [0.75f32.next_down(), 0.75] {
            assert!(!crouch_can_start(true, true, Some(phase), None));
            assert!(!crouch_can_start(true, true, None, Some((0.1, phase))));
        }
        assert!(crouch_can_start(true, true, Some(0.75f32.next_up()), None));
        assert!(crouch_can_start(true, true, None, Some((0.0005, 0.0))));
        assert!(crouch_can_start(true, true, None, None));
        assert!(crouch_can_start(true, false, Some(0.0), None));
        assert!(!crouch_can_start(false, false, None, None));
    }
    #[test]
    fn standing_clearance_checks_full_native_capsule() {
        use crate::collision::{Aabb3, CollisionWorld};
        let mut world = CollisionWorld {
            boxes: vec![Aabb3 {
                min: Vec3::new(-2.0, -1.0, -2.0),
                max: Vec3::new(2.0, 0.0, 2.0),
            }],
            ..default()
        };
        assert!(can_stand(Vec3::ZERO, &world));
        world.boxes.push(Aabb3 {
            min: Vec3::new(-2.0, 1.6, -2.0),
            max: Vec3::new(2.0, 2.0, 2.0),
        });
        assert!(!can_stand(Vec3::ZERO, &world));
    }
    #[test]
    fn static_jump_waits_for_free_run_timer_and_preparation() {
        for foot in 0..2 {
            let mut e = ExtraAction::free_run(foot, 0);
            let (out, _) = e.advance(0.199, false, true, foot);
            assert_eq!(out, ExtraResult::Stay);
            assert_eq!(e.mode, ExtraMode::FreeRun);
            e.advance(0.001, false, true, foot);
            assert_eq!(e.mode, ExtraMode::StaticPrepare);
            let mut result = ExtraResult::Stay;
            for _ in 0..120 {
                result = e.advance(1.0 / 60.0, false, true, foot).0;
                if matches!(result, ExtraResult::Jump { .. }) {
                    break;
                }
            }
            assert_eq!(
                result,
                ExtraResult::Jump {
                    action: STATIC_JUMP[foot],
                    fall: STATIC_FALL
                }
            );
        }
        let mut e = ExtraAction::free_run(0, 0);
        assert_eq!(e.advance(0.2, true, false, 0).0, ExtraResult::Inactive);
    }
    #[test]
    fn static_prepare_completes_only_after_positive_leftover_time() {
        let mut e = ExtraAction::free_run(0, 0);
        e.mode = ExtraMode::StaticPrepare;
        e.action = super::super::jump_blend::ActionBlend::new(STATIC_PREPARE[0], 0, &[1.0]);
        let duration = e.action.duration();
        assert_eq!(e.advance(duration, false, false, 0).0, ExtraResult::Stay);
        assert_eq!(e.advance(0.0, false, false, 0).0, ExtraResult::Stay);
        assert!(matches!(
            e.advance(0.00001, false, false, 0).0,
            ExtraResult::Jump { .. }
        ));
    }
    #[test]
    fn crowd_bounds_and_direction() {
        let f = Vec3::NEG_Z;
        for z in [0.5, -1.5] {
            assert!(crowd_avoid(Vec3::ZERO, f, Vec3::new(0.0, 0.0, z)).is_none());
        }
        assert_eq!(crowd_avoid(Vec3::ZERO, f, Vec3::NEG_Z), Some(Vec3::NEG_Z));
        assert!(crowd_avoid(Vec3::ZERO, f, Vec3::X).is_none());
        assert!(crowd_avoid(Vec3::ZERO, f, Vec3::new(0.0005, 0.0, 0.0005)).is_none());
        assert!(crowd_avoid(Vec3::ZERO, f, Vec3::new(0.0, 9.0, 0.0005f32.next_up())).is_some());
    }
    #[test]
    fn contact_angle_boundaries_and_flat_priority() {
        let flat = std::f32::consts::FRAC_1_SQRT_2;
        let steep = 70f32.to_radians().cos();
        let wall = 135f32.to_radians().cos();
        assert_eq!(ground_contact([flat]), GroundContact::Flat);
        assert_eq!(ground_contact([flat.next_down()]), GroundContact::Steep);
        assert_eq!(ground_contact([steep]), GroundContact::Steep);
        assert_eq!(ground_contact([steep.next_down()]), GroundContact::Wall);
        assert_eq!(ground_contact([wall]), GroundContact::Wall);
        assert_eq!(ground_contact([wall.next_down()]), GroundContact::None);
        assert_eq!(ground_contact([wall, steep, flat]), GroundContact::Flat);
        assert_eq!(ground_contact([steep, wall]), GroundContact::Steep);
    }
    #[test]
    fn slope_reference_survives_contact_loss_and_impact_excludes_slide() {
        let mut s = SlopeSlide::default();
        s.contact(GroundContact::Steep, 12.0);
        s.contact(GroundContact::None, 11.0);
        s.contact(GroundContact::Steep, 10.0);
        assert_eq!(s.start_y, Some(12.0));
        assert_eq!(s.check(12.0, 10.0, false), (LandingType::SmallDamage, 10));
        assert!(!s.ragfall_required);
        s.check(20.0, 2.0, false);
        assert!(!s.ragfall_required);
        s.check(20.0, 1.99999, false);
        assert!(s.ragfall_required);
    }
    #[test]
    fn fatal_steep_impact_and_dead_require_ragfall() {
        let mut s = SlopeSlide::default();
        s.contact(GroundContact::Steep, 0.0);
        assert_eq!(s.check(20.0f32.next_up(), 0.0, false).0, LandingType::Fatal);
        assert!(s.ragfall_required);
        let mut s = SlopeSlide::default();
        s.check(0.0, 0.0, true);
        assert!(s.ragfall_required);
    }
}

/// Simulation clock for discrete Ground actions; loops only the last item of the impulsion wait.
#[derive(Clone, Copy, Debug)]
pub struct ExtraAction {
    pub action: super::jump_blend::ActionBlend,
    pub time: f32,
    pub mode: ExtraMode,
    pub sequence: u32,
    pub elapsed: f32,
    pub target: Option<super::targets::JumpTarget>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExtraMode {
    FreeRun,
    StaticPrepare,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExtraResult {
    Inactive,
    Stay,
    Jump { action: u32, fall: u32 },
}
impl ExtraAction {
    pub fn free_run(foot: usize, sequence: u32) -> Self {
        Self {
            action: super::jump_blend::ActionBlend::new(FREERUN_WAIT[foot], 0, &[1.0]),
            time: 0.0,
            mode: ExtraMode::FreeRun,
            sequence,
            elapsed: 0.0,
            target: None,
        }
    }
    pub fn phase(&self) -> f32 {
        (self.time / self.action.duration().max(0.0001)).min(1.0)
    }
    /// Native FreeRun 0xD8B9D0 / 0xDAF2D0, event48 0xDAB320 and prepare completion 0xD85510.
    /// Returns horizontal root delta in clip coordinates; OnPlace owns subsequent vertical motion.
    pub fn advance(
        &mut self,
        dt: f32,
        moving: bool,
        request: bool,
        foot: usize,
    ) -> (ExtraResult, Vec3) {
        self.elapsed += dt;
        if self.mode == ExtraMode::FreeRun && self.elapsed >= 0.2 {
            if request {
                self.action = super::jump_blend::ActionBlend::new(STATIC_PREPARE[foot], 0, &[1.0]);
                self.mode = ExtraMode::StaticPrepare;
                self.time = 0.0;
                self.sequence = self.sequence.wrapping_add(1);
            } else if moving {
                return (ExtraResult::Inactive, Vec3::ZERO);
            }
        }
        let mut before = self.action.disp(self.phase());
        let mut remaining = dt;
        let mut delta = Vec3::ZERO;
        loop {
            let duration = self.action.duration().max(0.0001);
            let step = remaining.min((duration - self.time).max(0.0));
            self.time += step;
            remaining -= step;
            let after = self.action.disp(self.phase());
            delta += Vec3::new(after[0] - before[0], 0.0, after[1] - before[1]);
            // Native slot completion needs positive leftover time (0x730E50 / 0x778010).
            if self.time < duration || remaining <= 0.0 {
                break;
            }
            if self.mode == ExtraMode::StaticPrepare {
                return (
                    ExtraResult::Jump {
                        action: STATIC_JUMP[foot],
                        fall: STATIC_FALL,
                    },
                    delta,
                );
            }
            let count =
                super::jump_blend::action_items(self.action.id).map_or(1, |items| items.len());
            self.action.item = (self.action.item + 1).min(count - 1);
            self.time = 0.0;
            // The extra fraction is consumed on the next item without a seam blend (0x72FC30).
            if remaining <= 0.000001 {
                break;
            }
            before = self.action.disp(0.0);
        }
        (ExtraResult::Stay, delta)
    }
}
