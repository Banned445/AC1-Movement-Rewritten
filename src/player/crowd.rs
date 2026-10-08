//! Crowd request/evaluator rules (0xEDF390 / 0xF93E80 / 0xF201D0 / 0xD9E5A0).
use super::{Body, HumanDataBundle, Player};
use bevy::prelude::*;

pub const PUSH_ACTION: [u32; 2] = [0x011C_1298, 0x011C_1553];
pub const ARM_BONE: [u32; 2] = [0xeb83_0ada, 0x6bb3_f727];
pub const CLAVICLE_BONE: [u32; 2] = [0xb898_b609, 0x63d8_9144];
pub const PUSH_SLOT: [u8; 2] = [2, 3];
pub const PUSH_EFFECTOR: [u8; 2] = [7, 12];
pub const PUSH_BLEND_IN: f32 = 0.35;
pub const PUSH_BLEND_OUT: f32 = 0.4;

/// PushResponseEvent +48 (0xF93E80).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushResponse {
    Ignore,
    Accept,
    Refuse,
}

/// Native actor/AI guards, supplied by the registered recipient; no guessed civilian AI defaults.
#[derive(Clone, Copy, Debug)]
pub struct PushReceiver {
    pub valid: bool,
    pub relationship_blocks: bool, // 0xAFE500
    pub state_63_87_81: bool,
    pub state_77: bool,
    pub protected: bool,      // vt1212 or 0xF93C40
    pub context_blocks: bool, // vt1100
    pub move_mode: u32,
    pub special_gentle: bool, // AI vt308 skips reaction guards for strength zero
    pub event_allowed: bool,
    pub minimum_reaction: u32, // HumanGroundHurtState +100, vt1036 0xD95950
}
pub fn reaction(strength: u32) -> u32 {
    match strength {
        0..=3 => strength + 1,
        _ => 0,
    }
} // 0xB2FA80
impl PushReceiver {
    pub fn response(self, strength: u32, player_sender: bool) -> PushResponse {
        if !self.valid {
            return PushResponse::Ignore;
        }
        if self.relationship_blocks
            || self.state_63_87_81
            || (player_sender && self.state_77)
            || (strength == 0 && self.protected)
            || self.context_blocks
            || (self.move_mode == 9 && strength < 3)
        {
            return PushResponse::Refuse;
        }
        if (strength != 0 || !self.special_gentle)
            && (!self.event_allowed || self.minimum_reaction > reaction(strength))
        {
            return PushResponse::Refuse;
        }
        PushResponse::Accept
    }
}

/// Live registration for the crowd-avoid reference. Native actor spawning/streaming remains a separate data boundary.
/// PORT: ECS iteration replaces the game's actor partitioner; its eligibility flags must come from the actor importer.
#[derive(Component, Clone, Copy, Debug)]
pub struct CrowdActor {
    pub proximity_eligible: bool,
    pub receiver: PushReceiver,
}

pub fn gentle_requested(
    high: bool,
    legs: bool,
    hand: bool,
    hand_captured: bool,
    ground_blocks: bool,
) -> bool {
    !high && !legs && hand && !hand_captured && !ground_blocks // 0xEDF390 low-profile branch
}

/// 0xF201D0 / 0xF20810 use upper-arm and clavicle origins, not wrist contacts.
#[derive(Clone, Copy, Debug)]
pub struct PushPose {
    pub root: Vec3,
    pub forward: Vec3,
    pub arm: [Vec3; 2],
    pub clavicle: [Vec3; 2],
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandReport {
    pub point: Vec3,
    pub target_arm: usize,
}
fn arm_axis(pose: PushPose, side: usize) -> Option<Vec3> {
    let axis = (if side == 0 {
        pose.clavicle[side] - pose.arm[side]
    } else {
        pose.arm[side] - pose.clavicle[side]
    })
    .with_y(0.0);
    if axis.x.abs() <= 0.0005 && axis.z.abs() <= 0.0005 {
        return None;
    }
    let angle = f32::from_bits(if side == 0 { 0x4006_0a92 } else { 0x3f86_0a92 }); // 120 / 60 degrees, 0xF204FA / 0xF20513
    Some(Quat::from_rotation_y(angle) * axis.normalize())
}
/// Returns the evaluator report nearest the source upper-arm origin (0xF1F530).
pub fn evaluate_hand(owner: PushPose, target: PushPose, side: usize) -> Option<HandReport> {
    if side >= 2 {
        return None;
    }
    let to = (target.root - owner.root).with_y(0.0);
    if to.length_squared() > 1.4400001 {
        return None;
    }
    let front = to.dot(owner.forward);
    if !(0.1..=1.2).contains(&front) || to.dot(super::right_of(owner.forward)).abs() > 1.0 {
        return None;
    }
    let axis = arm_axis(owner, side)?;
    (0..2)
        .filter_map(|target_arm| {
            let delta = target.arm[target_arm] - owner.arm[side];
            let length = delta.length();
            if length < 0.1 || length - 0.1 > 0.55 || delta.normalize().dot(axis) < 0.0 {
                return None;
            }
            Some(HandReport {
                point: owner.arm[side] + delta.normalize() * (length - 0.1),
                target_arm,
            })
        })
        .min_by(|a, b| {
            a.point
                .distance_squared(owner.arm[side])
                .total_cmp(&b.point.distance_squared(owner.arm[side]))
        })
}
/// 0xF20810: signed, clamped projection of the target report; the displacement is not normalized.
pub fn push_weights(owner: PushPose, report: HandReport, side: usize) -> Option<[f32; 3]> {
    let axis = arm_axis(owner, side)?;
    let d = report.point - owner.arm[side];
    if d.x.abs() <= 0.0005 && d.y.abs() <= 0.0005 && d.z.abs() <= 0.0005 {
        return None;
    }
    let k = -d.dot(axis.cross(Vec3::Y)).clamp(-1.0, 1.0);
    Some(if k < 0.0 {
        [0.0, 1.0 + k, -k]
    } else {
        [k, 1.0 - k, 0.0]
    })
}

/// 0xF1FFD0 default push direction; callers may supply the explicit forced direction instead.
pub fn push_direction(root: Vec3, forward: Vec3, target: Vec3, forced: Option<Vec3>) -> Vec3 {
    if let Some(dir) = forced {
        return dir;
    }
    let delta = target - root;
    let right = super::right_of(forward);
    let side = if delta.dot(right) < 0.0 {
        -right
    } else {
        right
    };
    let front = delta.dot(forward);
    (side
        + if front > 0.0 {
            delta * front.clamp(0.0, 1.0)
        } else {
            Vec3::ZERO
        })
    .normalize_or_zero()
}

/// Native progress is the IKTask's reach blend (0xE12F80 / 0xF1F180), supplied rather than guessed from clip time.
#[derive(Clone, Copy, Debug, Default)]
pub struct HandInteraction {
    pub target: Option<Entity>,
    pub progress: f32,
    pub report: Option<HandReport>,
}
impl HandInteraction {
    pub fn start(&mut self, target: Entity, report: HandReport) {
        if self.target != Some(target)
            || self.report.map(|r| r.target_arm) != Some(report.target_arm)
        {
            self.progress = 0.0;
        }
        self.target = Some(target);
        self.report = Some(report);
    }
    pub fn update(&mut self, report: HandReport, progress: f32) -> bool {
        self.report = Some(report);
        self.progress = progress;
        self.target.is_some() && progress > 0.25 // 0xF20CCD; requested on every active update above the gate
    }
    pub fn stop(&mut self) -> Option<Entity> {
        let target = self.target.take();
        self.progress = 0.0;
        self.report = None;
        target
    }
}

/// Registered live actors drive Data+0x60 through the existing locomotion lean, once per update.
/// 0xEDF390 registers the nearest actor via 0xD6DF70(1.5); 0xD9E5A0 applies the local brush box.
pub fn update_crowd(
    actors: Query<(Entity, &Body, &CrowdActor), Without<Player>>,
    mut players: Query<(&Body, &mut HumanDataBundle), With<Player>>,
) {
    if !crate::tuning::GAME_GROUND_EXTRAS {
        return;
    }
    for (body, mut data) in &mut players {
        // PORT: static ECS proximity adapter; native actor partition order, active shove override and filters remain open.
        let nearest = actors
            .iter()
            .filter(|(_, other, a)| {
                a.proximity_eligible && other.feet.distance_squared(body.feet) < 1.5 * 1.5
            })
            .min_by(|a, b| {
                a.1.feet
                    .distance_squared(body.feet)
                    .total_cmp(&b.1.feet.distance_squared(body.feet))
            });
        data.ground.crowd_avoid = nearest.and_then(|(_, other, _)| {
            super::ground_extras::crowd_avoid(body.feet, body.forward(), other.feet)
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn receiver() -> PushReceiver {
        PushReceiver {
            valid: true,
            relationship_blocks: false,
            state_63_87_81: false,
            state_77: false,
            protected: false,
            context_blocks: false,
            move_mode: 0,
            special_gentle: false,
            event_allowed: true,
            minimum_reaction: 0,
        }
    }
    fn pose() -> PushPose {
        PushPose {
            root: Vec3::ZERO,
            forward: Vec3::NEG_Z,
            arm: [Vec3::new(-0.3, 1.3, 0.0), Vec3::new(0.3, 1.3, 0.0)],
            clavicle: [Vec3::new(-0.15, 1.3, 0.0), Vec3::new(0.15, 1.3, 0.0)],
        }
    }
    #[test]
    fn push_response_guards_and_strength_mapping() {
        for strength in 0..4 {
            assert_eq!(reaction(strength), strength + 1);
            assert_eq!(receiver().response(strength, true), PushResponse::Accept);
        }
        assert_eq!(reaction(4), 0);
        assert_eq!(
            PushReceiver {
                valid: false,
                ..receiver()
            }
            .response(0, true),
            PushResponse::Ignore
        );
        assert_eq!(
            PushReceiver {
                protected: true,
                ..receiver()
            }
            .response(0, true),
            PushResponse::Refuse
        );
        assert_eq!(
            PushReceiver {
                protected: true,
                ..receiver()
            }
            .response(1, true),
            PushResponse::Accept
        );
        assert_eq!(
            PushReceiver {
                state_77: true,
                ..receiver()
            }
            .response(0, true),
            PushResponse::Refuse
        );
        assert_eq!(
            PushReceiver {
                state_77: true,
                ..receiver()
            }
            .response(0, false),
            PushResponse::Accept
        );
        for strength in 0..3 {
            assert_eq!(
                PushReceiver {
                    move_mode: 9,
                    ..receiver()
                }
                .response(strength, true),
                PushResponse::Refuse
            );
        }
        assert_eq!(
            PushReceiver {
                move_mode: 9,
                ..receiver()
            }
            .response(3, true),
            PushResponse::Accept
        );
        assert_eq!(
            PushReceiver {
                minimum_reaction: 2,
                ..receiver()
            }
            .response(0, true),
            PushResponse::Refuse
        );
        assert_eq!(
            PushReceiver {
                minimum_reaction: 2,
                special_gentle: true,
                ..receiver()
            }
            .response(0, true),
            PushResponse::Accept
        );
    }
    #[test]
    fn gentle_push_needs_low_profile_empty_hand_and_released_legs() {
        assert!(gentle_requested(false, false, true, false, false));
        for flags in [
            (true, false, true, false, false),
            (false, true, true, false, false),
            (false, false, false, false, false),
            (false, false, true, true, false),
            (false, false, true, false, true),
        ] {
            assert!(!gentle_requested(
                flags.0, flags.1, flags.2, flags.3, flags.4
            ));
        }
    }
    #[test]
    fn evaluator_uses_arm_origins_and_keeps_the_native_point_one_tenth_from_target() {
        let owner = pose();
        let mut target = pose();
        target.root = Vec3::NEG_Z * 0.5;
        for arm in &mut target.arm {
            *arm += target.root;
        }
        for side in 0..2 {
            let report = evaluate_hand(owner, target, side).expect("both sides in reach");
            assert_eq!(report.target_arm, side);
            assert!((report.point.distance(target.arm[side]) - 0.1).abs() < 0.00001);
            let weights = push_weights(owner, report, side).unwrap();
            assert!((weights.iter().sum::<f32>() - 1.0).abs() < 0.00001);
        }
        target.root = Vec3::NEG_Z * 0.09999;
        assert!(evaluate_hand(owner, target, 0).is_none());
        target.root = Vec3::NEG_Z * 1.20001;
        assert!(evaluate_hand(owner, target, 0).is_none());
        target.root = Vec3::NEG_Z * 0.5;
        target.arm = [owner.arm[0] + Vec3::NEG_Z * 0.65001; 2];
        assert!(evaluate_hand(owner, target, 0).is_none());
    }
    #[test]
    fn interaction_uses_strict_reach_progress_and_target_lifetime() {
        let mut h = HandInteraction::default();
        let e = Entity::from_bits(1);
        let report = HandReport {
            point: Vec3::NEG_Z,
            target_arm: 0,
        };
        h.start(e, report);
        assert!(!h.update(report, 0.25));
        assert!(h.update(report, 0.25f32.next_up()));
        assert!(
            h.update(report, 0.3),
            "native sender repeats while the reach is active"
        );
        h.start(e, report);
        assert_eq!(h.progress, 0.3, "refresh same evaluator without restarting");
        assert_eq!(h.stop(), Some(e));
        assert!(!h.update(report, 1.0));
    }
}
