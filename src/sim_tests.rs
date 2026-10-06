//! Headless simulation tests: run the real ground/air systems on the greybox level with scripted
//! pad input at a fixed 60 Hz, and check behaviour against the reverse-engineered rules.

use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use crate::camera::CameraRig;
use crate::input::PadInput;
use crate::level;
use crate::player::air::LandingType;
use crate::player::ground::{speed_band, HumanGroundData, SpeedBand};
use crate::player::{air, climb, ground, ledge, player_components, ActorContextId, Body, HumanDataBundle, Locomotion, SpawnPoint};

const SPAWN: Vec3 = Vec3::new(0.0, 0.0, 4.0);

struct Sim {
    app: App,
    player: Entity,
    saw_air: bool,
}

impl Sim {
    fn new(feet: Vec3, heading: f32) -> Self {
        let mut s = Self::new_raw(feet, heading);
        s.app.update(); // first frame has dt = 0
        s
    }

    /// As `new`, without running the first frame (a recording replays its own first frame).
    fn new_raw(feet: Vec3, heading: f32) -> Self {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 60.0)))
            .insert_resource(PadInput { legs_pressed_ago: f32::INFINITY, hand_pressed_ago: f32::INFINITY, ..default() })
            .insert_resource(SpawnPoint(SPAWN))
            .init_resource::<CameraRig>()
            .add_systems(Update, (ground::update_ground, air::update_air, ledge::update_ledge, climb::update_climb, crate::player::hay::update_hay, crate::player::walling::update_walling, crate::player::narrow::update_narrow, crate::player::ladder::update_ladder, crate::player::release_limbs).chain());
        let (c, g) = level::geometry();
        app.insert_resource(c).insert_resource(g);
        let player = app.world_mut().spawn(player_components(feet, heading)).id();
        Sim { app, player, saw_air: false }
    }

    fn pad(&mut self, dir: Vec3, stick: f32, high: bool, legs: bool) {
        let mut p = self.app.world_mut().resource_mut::<PadInput>();
        p.dir = dir.normalize_or_zero();
        p.magnitude = stick;
        p.speed01 = if stick <= 0.35 { 0.0 } else { ((stick - 0.35) / 0.65).min(1.0) };
        p.high_profile = high;
        p.legs_held = legs;
    }

    fn run(&mut self, seconds: f32) {
        let frames = (seconds * 60.0) as usize;
        for _ in 0..frames {
            // Legs buffer bookkeeping normally done by read_pad
            {
                let mut p = self.app.world_mut().resource_mut::<PadInput>();
                p.legs_pressed_ago += 1.0 / 60.0;
                p.hand_pressed_ago += 1.0 / 60.0;
            }
            self.app.update();
            if self.loco().current == ActorContextId::InAir {
                self.saw_air = true;
            }
        }
    }

    fn loco(&self) -> &Locomotion {
        self.app.world().get::<Locomotion>(self.player).unwrap()
    }
    fn body(&self) -> &Body {
        self.app.world().get::<Body>(self.player).unwrap()
    }
    fn data(&self) -> &HumanDataBundle {
        self.app.world().get::<HumanDataBundle>(self.player).unwrap()
    }
    fn ground(&self) -> &HumanGroundData {
        &self.data().ground
    }
    /// Press Legs (fills the jump buffer, like read_pad on a fresh press).
    fn press_legs(&mut self) {
        self.app.world_mut().resource_mut::<PadInput>().legs_pressed_ago = 0.0;
    }
    /// Press the empty-hand button: it reads as pressed on the next frame.
    fn press_hand(&mut self) {
        self.app.world_mut().resource_mut::<PadInput>().hand_pressed_ago = -1.0 / 60.0;
    }
    /// Run until `pred` holds or `max_s` elapses; returns whether it held.
    fn run_until(&mut self, max_s: f32, pred: impl Fn(&Sim) -> bool) -> bool {
        for _ in 0..(max_s * 60.0) as usize {
            self.run(1.0 / 60.0 + 1e-4);
            if pred(self) {
                return true;
            }
        }
        false
    }
}

#[test]
fn low_profile_full_stick_is_walk_band() {
    let mut s = Sim::new(SPAWN, std::f32::consts::PI);
    s.pad(Vec3::Z, 1.0, false, false);
    s.run(2.0);
    assert_eq!(speed_band(s.ground().speed_param), SpeedBand::Walk);
    assert!((s.ground().speed_param - 0.25).abs() < 1e-3, "param {}", s.ground().speed_param);
    assert!(s.body().feet.z > SPAWN.z + 2.0, "walked forward: {:?}", s.body().feet);
}

#[test]
fn high_profile_runs_and_legs_sprints() {
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(1.5);
    assert_eq!(speed_band(s.ground().speed_param), SpeedBand::Run);
    s.pad(Vec3::NEG_Z, 1.0, true, true);
    s.run(1.5);
    assert_eq!(speed_band(s.ground().speed_param), SpeedBand::Sprint);
    assert!(s.ground().speed_param > 0.99);
}

#[test]
fn speed_param_rises_at_one_per_second() {
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, true);
    // the start from standing sets it to 0.5 in high profile (0xD98990), then it rises at 1.0/s toward 1.0
    s.run(1.0 / 60.0 + 1e-4);
    let p0 = s.ground().speed_param;
    assert!((p0 - 0.5).abs() < 0.03, "start sets 0.5: {p0}");
    s.run(0.25);
    let p = s.ground().speed_param;
    assert!((p - 0.75).abs() < 0.05, "after 0.25 s more param = {p}");
}

#[test]
fn ground_moves_by_the_blended_clip_root_motion() {
    // run band top (param 0.75 = pure xx_h_run_hipm, 1.707 m per 0.3333 s step)
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(1.5);
    let z0 = s.body().feet.z;
    s.run(1.0);
    let v = z0 - s.body().feet.z;
    assert!((v - 5.121).abs() < 0.05, "run speed {v} m/s");
    // sprint top settles on xx_h_sprint_hipm (1.674 m per 0.2667 s)
    s.pad(Vec3::NEG_Z, 1.0, true, true);
    s.run(2.5);
    let z0 = s.body().feet.z;
    s.run(1.0);
    let v = z0 - s.body().feet.z;
    assert!((v - 6.277).abs() < 0.05, "sprint speed {v} m/s");
    assert!(s.ground().blend.weights[13] > 0.99);
}

#[test]
fn releasing_sprint_decelerates_through_the_curve() {
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, true);
    s.run(1.5);
    // back to high-profile run (target 0.75). On the curve's last segment ds/dt = −(0.2 + 2.395·(s − 0.666)),
    // so from 1.0: s(0.2 s) = 0.4175·e^(−0.479) + 0.5825 = 0.841
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(0.2);
    let p = s.ground().speed_param;
    assert!((p - 0.841).abs() < 0.01, "param after 0.2 s = {p}");
    s.run(1.0);
    assert!((s.ground().speed_param - 0.75).abs() < 1e-3);
}

#[test]
fn free_run_jumps_a_rooftop_gap_and_lands_on_target() {
    // roof A: x 5..11, h 3.5 ; roof B: x 14.5..20.5, h 3.0  (3.5 m gap)
    let mut s = Sim::new(Vec3::new(7.0, 3.5, 12.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true); // sprint toward +X
    for _ in 0..240 {
        s.run(1.0 / 60.0 + 1e-4);
        if s.saw_air && s.loco().current == ActorContextId::Ground {
            s.pad(Vec3::X, 0.0, false, false); // let go once landed
            s.run(1.0);
            break;
        }
    }
    assert!(s.saw_air, "never jumped");
    assert_eq!(s.loco().current, ActorContextId::Ground);
    let f = s.body().feet;
    assert!(f.x > 14.5 && (f.y - 3.0).abs() < 0.05, "should be standing on roof B, feet {f:?}");
    let l = s.ground().last_landing.expect("landing recorded");
    assert_eq!(l.kind, LandingType::Safe);
}

#[test]
fn chained_free_run_crosses_several_roofs() {
    // holding sprint keeps jumping: roof A (h 3.5) → B (h 3.0, 3.5 m gap) → C (h 4.0, 5 m gap, +1 m up)
    let mut s = Sim::new(Vec3::new(7.0, 3.5, 12.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    let mut on_b = false;
    let mut on_c = false;
    for _ in 0..360 {
        s.run(1.0 / 60.0 + 1e-4);
        let f = s.body().feet;
        let grounded = s.loco().current == ActorContextId::Ground;
        on_b |= grounded && (14.5..20.5).contains(&f.x) && (f.y - 3.0).abs() < 0.05;
        on_c |= grounded && (25.5..31.5).contains(&f.x) && (f.y - 4.0).abs() < 0.05;
    }
    assert!(on_b && on_c, "landed on B: {on_b}, landed on C: {on_c}");
}

#[test]
fn eight_metre_drop_is_fatal_and_respawns() {
    // tower at (-12, 14), 5x5, h 8: run off its +X edge at 65 deg (a drop > 5 m straight ahead would ledge-stop,
    // 0xEE8899; at 65 deg it is not a front edge)
    let mut s = Sim::new(Vec3::new(-11.0, 8.0, 12.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::new(0.4226, 0.0, 0.9063), 1.0, true, false);
    let landed = s.run_until(4.0, |s| s.saw_air && s.ground().last_landing.is_some());
    assert!(landed && s.saw_air);
    s.pad(Vec3::X, 0.0, false, false);
    s.run(0.2);
    let l = s.ground().last_landing.expect("landed");
    assert_eq!(l.kind, LandingType::Fatal, "{l:?}");
    assert!((s.body().feet - SPAWN).length() < 3.0, "respawned near spawn: {:?}", s.body().feet);
}

#[test]
fn six_metre_drop_rolls_without_heavy_damage() {
    // 6 m is below the 6.3 m heavy threshold but above the 3 m roll threshold
    let mut s = Sim::new(Vec3::new(-11.0, 6.0, 2.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::new(0.4226, 0.0, 0.9063), 1.0, true, false); // at 65 deg: no ledge stop
    s.run(4.0);
    let l = s.ground().last_landing.expect("landed");
    assert_eq!(l.kind, LandingType::Safe, "{l:?}");
    assert!(l.roll, "drop > 3 m should roll: {l:?}");
}

#[test]
fn step_up_low_obstacle() {
    // 0.3 m block at (6, 0): under the 0.37 m step offset of the lifted capsule (0xDAE6E0)
    let mut s = Sim::new(Vec3::new(3.0, 0.0, 0.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, false);
    s.run(2.0);
    assert!(s.body().feet.x > 7.0, "blocked by 0.3 m step: {:?}", s.body().feet);
    assert_eq!(s.loco().current, ActorContextId::Ground);
}

#[test]
fn wall_blocks_movement() {
    // wall: x -7..7, z -8.3..-7.7, h 2.4
    let mut s = Sim::new(Vec3::new(0.0, 0.0, -5.0), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(2.0);
    // wall face at z = -7.7, capsule radius 0.3 → should stop at about z = -7.4
    assert!(s.body().feet.z > -7.45, "went through wall: {:?}", s.body().feet);
}

// ============================================================== stage 2: ledges and climbing

const FACE_PZ: f32 = std::f32::consts::PI; // heading that faces +Z

#[test]
fn climb_tower_to_the_top_and_pull_up() {
    // tower face at z = 27 (normal -Z), roof at 9.6 m
    let mut s = Sim::new(Vec3::new(-20.5, 0.0, 26.5), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Climb), "did not start climbing");
    assert_eq!(s.data().climb.entry_type, climb::ClimbEntryType::FromGround);
    let reached = s.run_until(30.0, |s| s.loco().current == ActorContextId::Ground && s.body().feet.y > 9.5);
    let f = s.body().feet;
    assert!(reached, "never stood on the roof: ctx {:?} feet {f:?} climb {:?} ledge {:?}", s.loco().current, s.data().climb.last_action, s.data().ledge.last_action);
    assert!(f.z > 27.0, "should be on the roof, feet {f:?}");
}

#[test]
fn a_hang_over_climb_holds_climbs_back_down_with_the_transition_clip() {
    // hang from the tower's top edge (9.6 m), then push down: TryTransitionToClimb 0xDD46A0 with the down table
    // (climbing up to the edge now climbs out over it, TryReachLedgeAbove 0xDF1730)
    let mut s = hang_at(Vec3::new(-20.5, 9.6, 27.0), Vec3::NEG_Z);
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none() && s.data().ledge.last_action == "idle"), "hang never settled");
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Wall);
    s.pad(Vec3::NEG_Z, 1.0, false, false);
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Climb), "no transition to the climb: {:?}", s.data().ledge.last_action);
    assert_eq!(s.data().climb.entry_type, climb::ClimbEntryType::FromLedge);
    assert_eq!(s.data().climb.move_action, Some(climb::LEDGE_TO_CLIMB[1][0]), "xx_h_hangwall_d_climb_1m");
    let m = s.data().climb.moving.expect("transition move");
    assert!((m.duration - climb::move_time(Some(climb::LEDGE_TO_CLIMB[1][0]))).abs() < 1e-4, "timed by the clip: {}", m.duration);
}

#[test]
fn a_hang_over_climb_holds_moves_sideways_into_the_climb() {
    // the tower's top edge with bands every 0.6 m below: pushing sideways tries climb holds down (feet 1.8 m, hands
    // 0.6 m below), then level, before the shimmy (0xDE29E0, preference 0); the bands make "down" available
    for stick in [Vec3::X, Vec3::NEG_X] {
        let mut s = hang_at(Vec3::new(-20.5, 9.6, 27.0), Vec3::NEG_Z);
        s.pad(Vec3::ZERO, 0.0, false, false);
        assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none() && s.data().ledge.last_action == "idle"));
        let hands_y = s.data().ledge.hand_l.y;
        s.pad(stick, 1.0, false, false);
        assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Climb), "no side move into the climb: {:?}", s.data().ledge.last_action);
        let c = &s.data().climb;
        let down = climb::LEDGE_SIDE_TO_CLIMB[(stick == Vec3::X) as usize][2][0];
        let other = climb::LEDGE_SIDE_TO_CLIMB[(stick != Vec3::X) as usize][2][0];
        assert!(c.move_action == Some(down) || c.move_action == Some(other), "xx_h_hangwall_d{{l,r}}_climb_1m: {:?}", c.move_action);
        assert!((c.hand_l.y - (hands_y - 0.6)).abs() < 0.05 && (c.foot_l.y - (hands_y - 1.8)).abs() < 0.05, "hands 0.6 m, feet 1.8 m below: {:?} {:?}", c.hand_l, c.foot_l);
        assert!((c.hand_l.x - s.data().ledge.hand_l.x).abs() > 0.1, "moved sideways");
    }
}

#[test]
fn climb_start_from_the_ground_uses_the_leading_foot() {
    // sub_B26080 (0xB261A8): 0x01BC382C + (Human__GetLeadingFoot() != 1)
    let mut seen = std::collections::HashSet::new();
    for (from, foot_right) in [(Vec3::new(-20.5, 0.0, 26.5), false), (Vec3::new(-20.5, 0.0, 26.5), true)] {
        let mut s = Sim::new(from, FACE_PZ);
        s.app.world_mut().get_mut::<HumanDataBundle>(s.player).unwrap().ground.blend.foot = foot_right as usize;
        s.pad(Vec3::Z, 1.0, true, true);
        let mut lead = None;
        for _ in 0..60 {
            if s.loco().current == ActorContextId::Ground {
                lead = Some(s.ground().blend.foot != 0);
            }
            s.run(1.0 / 60.0 + 1e-4);
            if s.loco().current == ActorContextId::Climb {
                break;
            }
        }
        assert_eq!(s.loco().current, ActorContextId::Climb, "did not start climbing");
        let lead = lead.expect("never on the ground");
        assert_eq!(s.data().climb.move_action, Some(climb::CLIMB_FROM_GROUND[lead as usize]), "leading foot right = {lead}");
        seen.insert(lead);
    }
    assert_eq!(seen.len(), 2, "both feet exercised");
}

#[test]
fn climb_is_blocked_by_missing_holds() {
    // column x = -18.0 has no holds between 2.9 and 4.3 m
    let mut s = Sim::new(Vec3::new(-18.0, 0.0, 26.5), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Climb));
    s.run(8.0);
    assert_eq!(s.loco().current, ActorContextId::Climb);
    let top_hand = s.data().climb.hand_l.y.max(s.data().climb.hand_r.y);
    assert!(top_hand < 2.95, "climbed into the gap: hands at {top_hand}");
    assert_eq!(s.data().climb.last_action, "blocked");
    // OnNoMoveFound 0xDF4410: pushing up into the gap looks up
    assert_eq!(s.data().climb.look, Some(climb::LOOK_AROUND[0]));
    s.pad(Vec3::Z, 0.0, true, false);
    s.run(0.1);
    assert_eq!(s.data().climb.look, None, "no stick: the pose's wait");
}

#[test]
fn climb_jumps_up_to_an_overhang_when_the_holds_run_out() {
    use crate::player::ledge_moves::MoveKind;
    // the tower's left gap (x -22.6..-21.4, 2.9..4.3 m) under a slab 1 m out from the face, top 3.5 m
    let mut s = Sim::new(Vec3::new(-22.0, 0.0, 26.5), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Climb));
    s.pad(Vec3::Z, 1.0, true, false);
    assert!(s.run_until(10.0, |s| s.loco().current == ActorContextId::Ledge), "never jumped: {:?} {:?}", s.data().climb.last_action, s.body().feet);
    assert_eq!(s.data().climb.last_action, "jump up");
    let mv = s.data().ledge.mv.expect("the jump");
    assert_eq!(mv.kind, MoveKind::Arrival);
    assert_eq!(mv.seq.map(|a| a.map(|a| a.id)), [Some(climb::CLIMB_IMPULSE), Some(climb::CLIMB_JUMP_SWINGBACK), None, None]);
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(4.0, |s| s.data().ledge.mv.is_none()), "jump never ended");
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Free);
    let hands = (s.data().ledge.hand_l + s.data().ledge.hand_r) * 0.5;
    assert!((hands.y - 3.5).abs() < 0.05 && (hands.z - 26.0).abs() < 0.05, "hands on the slab's outer edge: {hands:?}");
}

#[test]
fn climb_grid_finds_holds_that_stick_out() {
    // BuildHoldGrid 0xDF6A40: the cell probe reaches 1.0 m both ways along the facing, so with the hands on wall K's
    // 3.0 m band the bay's 3.6 m band, 0.9 m further out, is a hold in the hand row above
    let (_, guidance) = crate::level::geometry();
    let mut d = climb::HumanClimbData::default();
    let n = Vec3::NEG_Z;
    let z = -10.58;
    (d.hand_l, d.hand_r, d.foot_l, d.foot_r) = (Vec3::new(-30.0, 3.0, z), Vec3::new(-30.0, 3.0, z), Vec3::new(-30.0, 1.8, z), Vec3::new(-30.0, 1.8, z));
    d.hold_n = [n; 4];
    d.normal = n;
    d.set_frame();
    let grid = climb::HoldGrid::build(&guidance, &d, d.frame.root, Vec3::Z);
    let (pl, _) = climb::POSES[0];
    let up = grid.hold(pl.0, pl.1 + 1 + crate::tuning::CLIMB_HAND_ROWS).expect("the bay's band above the hands");
    assert!((up.pos.y - 3.6).abs() < 1e-3 && (up.pos.z - (-11.48)).abs() < 1e-3, "{up:?}");
    let hand = grid.hold(pl.0, pl.1 + crate::tuning::CLIMB_HAND_ROWS).expect("the current hand hold");
    assert!((hand.pos - d.hand_l).length() < 1e-3, "{hand:?}");
}

#[test]
fn climb_root_frame_follows_the_holds() {
    // sub_B1CC20: on a vertical wall the root sits 0.5 m out from the feet, at the lower foot, facing the wall
    let n = Vec3::NEG_Z;
    let p = climb::climb_pose([Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, 1.8, 0.0), Vec3::new(0.0, 1.8, 0.0)], [n; 4], Vec3::Z);
    assert!((p.root - Vec3::new(0.0, 1.8, -0.5)).length() < 1e-4, "{p:?}");
    assert!((p.forward - Vec3::Z).length() < 1e-4 && (p.up - Vec3::Y).length() < 1e-4, "{p:?}");
    // hands 0.9 m further out than the feet: the root leans back with them
    let p = climb::climb_pose([Vec3::new(-0.2, 3.6, -0.9), Vec3::new(0.2, 3.6, -0.9), Vec3::new(-0.2, 2.4, 0.0), Vec3::new(0.2, 2.4, 0.0)], [n; 4], Vec3::Z);
    assert!(p.up.z < -0.3 && p.forward.y > 0.3, "the frame tilts: {p:?}");
}

#[test]
fn climb_onto_holds_that_stick_out_becomes_a_free_hang() {
    use crate::player::ledge_moves::MoveKind;
    // wall K (face z -10.5) with the bay 0.9 m out above 3.3 m: climbing straight up, the hands reach the bay's
    // bands while the feet stay on the wall; once the body leans out 30° more than the hands' line the climb hangs
    // from the bay (TryTransitionToLedgeHang 0xDF4BA0)
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -11.4), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Climb), "never climbed: {:?}", s.body().feet);
    // a light push (≤ 0.5): SHORT moves only, one side at a time
    s.pad(Vec3::Z, 0.45, true, false);
    assert!(s.run_until(15.0, |s| s.loco().current == ActorContextId::Ledge), "never hung: {:?} {:?} hands {:?}", s.data().climb.last_action, s.body().feet, s.data().climb.hand_l);
    assert_eq!(s.data().climb.last_action, "overhang");
    let mv = s.data().ledge.mv.expect("the overhang move");
    assert_eq!(mv.kind, MoveKind::Arrival);
    assert!(climb::OVERHANG_TO_HANG.contains(&mv.seq[0].expect("the action").id));
    assert!(mv.hand_l.z < -11.4 && mv.hand_r.z < -11.4, "both hands on the bay: {:?} {:?}", mv.hand_l, mv.hand_r);
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()), "move never ended");
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Free);
}

/// Climb wall M from the ground at `x` and push up (light, SHORT moves) until both feet stand on the 2.4 m band.
fn climb_wall_m_to_feet_at(x: f32) -> Sim {
    let mut s = Sim::new(Vec3::new(x, 0.0, -11.4), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Climb), "never climbed: {:?}", s.body().feet);
    s.pad(Vec3::Z, 0.45, true, false);
    let level = |s: &Sim| {
        let c = &s.data().climb;
        c.moving.is_none() && (c.foot_l.y - 2.4).abs() < 0.01 && (c.foot_r.y - 2.4).abs() < 0.01
    };
    assert!(s.run_until(10.0, level), "feet never reached 2.4 m: {:?} {:?}", s.data().climb.foot_l, s.data().climb.foot_r);
    s.pad(Vec3::Z, 0.0, true, false);
    s.run(0.2);
    s
}

#[test]
fn climb_steps_sideways_onto_a_roof_level_with_the_feet() {
    // TryCornerToGround_Grid 0xDF4670: block N's top edge (2.4 m) beside wall M is a hold level with the feet with no
    // hand hold above it; pushing right (-X) steps onto it with `groundentry_right` and ends standing in Ground
    let mut s = climb_wall_m_to_feet_at(-41.0);
    s.pad(Vec3::NEG_X, 0.45, true, false);
    assert!(s.run_until(4.0, |s| s.data().climb.last_action == "corner to ground"), "no corner move: {:?} feet {:?}", s.data().climb.last_action, s.data().climb.foot_r);
    assert_eq!(s.data().climb.move_action, Some(climb::CORNER_TO_GROUND[1][0]));
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground), "never stood: {:?}", s.loco().current);
    s.run(1.5);
    let f = s.body().feet;
    assert!(f.x < -42.0 && (f.y - 2.4).abs() < 0.05, "standing on block N: {f:?}");
}

#[test]
fn climb_turns_onto_a_wall_square_to_the_climb() {
    // TryCornerToGround_Candidates 0xDF28A0: block O stands square to wall M on the left (+X); its top edge (2.4 m,
    // facing -X) is a side candidate level with the feet, so pushing left turns onto it with `groundentry_left_90`
    let mut s = climb_wall_m_to_feet_at(-38.9);
    s.pad(Vec3::X, 0.45, true, false);
    assert!(s.run_until(4.0, |s| s.data().climb.last_action == "corner to ground"), "no corner move: {:?} feet {:?}", s.data().climb.last_action, s.data().climb.foot_l);
    assert_eq!(s.data().climb.move_action, Some(climb::CORNER_TO_GROUND[0][1]));
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground), "never stood: {:?}", s.loco().current);
    s.run(1.5);
    let f = s.body().feet;
    assert!(f.x > -38.0 && (f.y - 2.4).abs() < 0.05, "standing on block O: {f:?}");
}

#[test]
fn climb_moves_last_as_long_as_their_actions() {
    // StartMove 0xDFA0C0: the root is interpolated over the move action's length, not a fixed time
    for id in [0x019A_05F1u32, 0x019A_36CF, 0x01A2_6815] {
        let t = climb::move_time(Some(id));
        assert!(t > 0.1 && t != climb::MOVE_TIME_NO_ACTION, "action {id:#x}: {t} s");
    }
    assert_eq!(climb::move_time(None), climb::MOVE_TIME_NO_ACTION);
    assert_eq!(climb::move_time(Some(0x0BAD_0BAD)), climb::MOVE_TIME_NO_ACTION, "unknown action");
}

/// Jump-up wall: top edge 2.6 m, face z = 41.7 (normal -Z), x 8..16, pillar at x 15.5..16.5.
fn hang_on_jump_up_wall() -> Sim {
    let mut s = Sim::new(Vec3::new(12.0, 0.0, 40.6), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    let hung = s.run_until(3.0, |s| s.loco().current == ActorContextId::Ledge);
    assert!(hung, "never hung: ctx {:?} feet {:?}", s.loco().current, s.body().feet);
    s.pad(Vec3::Z, 0.0, false, false);
    s.run(0.5);
    s
}

#[test]
fn jump_up_to_a_ledge_then_pull_up() {
    let mut s = hang_on_jump_up_wall();
    assert!(s.saw_air, "should have jumped up to the 2.6 m ledge");
    // hands 2.6 m above the feet with a wall below: the straight jump's top band, `jumpstraight_to_hangwallfree`,
    // which the arrival turns into a free hang (0xB21DA0 / 0xE07D00: LedgeHangType 1)
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()), "reception never ended");
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Free);
    let hands = s.data().ledge.hand_l.y;
    assert!((hands - 2.6).abs() < 0.05, "hands on the edge: {hands}");
    let f = s.body().feet;
    assert!((f.y - 0.2).abs() < 0.05, "free-hang root 2.4 m below the hands: {f:?}");
    s.pad(Vec3::Z, 1.0, false, false); // hold up at the top edge → pull-up
    let on_top = s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground);
    assert!(on_top && (s.body().feet.y - 2.6).abs() < 0.05, "pull-up failed: {:?} ledge {}", s.body().feet, s.data().ledge.last_action);
}

#[test]
fn shimmy_is_stopped_by_the_pillar() {
    let mut s = hang_on_jump_up_wall();
    // facing +Z the player's left is +X, toward the pillar
    s.pad(Vec3::X, 1.0, false, false);
    s.run(8.0);
    assert_eq!(s.loco().current, ActorContextId::Ledge);
    let max_x = s.data().ledge.hand_l.x.max(s.data().ledge.hand_r.x);
    assert!(max_x > 13.5, "should have shimmied toward the pillar, hands at x {max_x}");
    assert!(max_x < 15.5, "went into the pillar: x {max_x}");
    assert!(s.data().ledge.last_action.contains("blocked"), "{}", s.data().ledge.last_action);
}

/// Free-hang balcony slab: top 3.0 m, x -1..5, front edge z 35.4 (normal -Z), nothing below.
fn hang_on_balcony() -> Sim {
    let mut s = Sim::new(Vec3::new(2.0, 0.0, 33.0), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, false);
    s.run(0.3);
    s.press_legs();
    let hung = s.run_until(3.0, |s| s.loco().current == ActorContextId::Ledge);
    assert!(hung, "never reached the balcony: ctx {:?} feet {:?}", s.loco().current, s.body().feet);
    s.pad(Vec3::Z, 0.0, false, false);
    // the arrival's reception (swing) plays before the hang takes input
    assert!(s.run_until(4.0, |s| s.data().ledge.mv.is_none() && s.data().ledge.swing.is_none()), "reception never ended");
    s.run(0.2);
    s
}

#[test]
fn free_hang_shimmy_ends_with_the_outer_corner() {
    let mut s = hang_on_balcony();
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Free);
    let f = s.body().feet;
    assert!((f.y - 0.6).abs() < 0.05, "free hang root 2.4 m below the hands: {f:?}");
    // the player's right (facing +Z) is -X; the slab ends at x = -1
    s.pad(Vec3::NEG_X, 1.0, false, false);
    s.run(10.0);
    let min_x = s.data().ledge.hand_l.x.min(s.data().ledge.hand_r.x);
    assert_eq!(s.loco().current, ActorContextId::Ledge);
    assert!(min_x >= -1.01 && min_x < 0.2, "hands should stop at the slab end: {min_x}");
    // at the end the free hang turns the outer corner onto the slab's -X side (0xDD3BB0, `corner_090_out`)
    assert!(s.data().ledge.normal.dot(Vec3::NEG_X) > 0.99, "no outer corner: normal {:?}", s.data().ledge.normal);
}

#[test]
fn let_go_from_a_hang_falls_and_lands() {
    let mut s = hang_on_balcony();
    s.press_legs();
    assert!(s.run_until(0.5, |s| s.loco().current == ActorContextId::InAir), "did not let go");
    assert_eq!(s.data().air.fall_origin, air::FallOrigin::HangFree);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground));
    assert_eq!(s.ground().last_landing.map(|l| l.kind), Some(LandingType::Safe));
}

#[test]
#[ignore]
fn trace_climb() {
    let mut s = Sim::new(Vec3::new(-20.5, 0.0, 26.5), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    let mut last = String::new();
    for i in 0..(30 * 60) {
        s.run(1.0 / 60.0 + 1e-4);
        let d = s.data();
        let line = format!("{:?} pose {} climb:{} ledge:{} {:?}", s.loco().current, d.climb.pose, d.climb.last_action, d.ledge.last_action, d.ledge.sub_state);
        if line != last {
            eprintln!("t={:5.2} feet y {:5.2} hands {:.2}/{:.2} | {line}", i as f32 / 60.0, s.body().feet.y, d.climb.hand_l.y, d.climb.hand_r.y);
            last = line;
        }
        if s.loco().current == ActorContextId::Ground && s.body().feet.y > 9.5 { break; }
    }
}

#[test]
fn rooftop_jump_plays_the_game_items_and_free_step_reception() {
    use crate::player::air::AirMode;
    use crate::player::jump_blend::{RECEPTION_FREESTEP, TAKEOFF_RUN};
    // roof A: x 5..11, h 3.5 ; roof B: x 14.5..20.5, h 3.0
    let mut s = Sim::new(Vec3::new(7.0, 3.5, 12.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::InAir), "never jumped");
    let air = &s.data().air;
    let AirMode::Jump { real, duration, t_takeoff, .. } = air.mode else { panic!("not a jump: {:?}", air.mode) };
    assert!(real);
    let (to, fl) = (air.takeoff.unwrap(), air.flight.unwrap());
    assert!(TAKEOFF_RUN.contains(&to.id));
    // durations are the items' Σw·T (takeoff 0.13–0.33 s, flight 0.2–0.6 s for these clips)
    assert!((t_takeoff - to.duration()).abs() < 1e-5 && (duration - t_takeoff - fl.duration()).abs() < 1e-5);
    assert!(duration > 0.3 && duration < 1.2, "jump lasts {duration}");
    let sum: f32 = fl.weights().iter().sum();
    assert!((sum - 1.0).abs() < 1e-4);
    // arrival → Ground with the free-step reception playing, then free movement
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ground), "never landed");
    let os = s.ground().oneshot.expect("reception playing");
    assert!(RECEPTION_FREESTEP.contains(&os.blend.id));
    assert!(os.duration > 0.1);
    assert!(s.run_until(2.0, |s| s.ground().oneshot.is_none()), "reception never ended");
    assert!(s.ground().speed_param > 0.75, "kept the sprint speed: {}", s.ground().speed_param);
}

#[test]
fn small_drop_lands_with_the_forward_landing_blend() {
    use crate::player::jump_blend::{LAND_FORWARD_MOVE, LAND_STRAIGHT_MOVE};
    // walk off roof B's far edge (3.0 m high, x 14.5..20.5) at jog speed: drop 3.0 m ≤ 3 → landing, b = 1 (hard)
    let mut s = Sim::new(Vec3::new(19.0, 3.0, 12.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, false);
    assert!(s.run_until(4.0, |s| s.saw_air && s.loco().current == ActorContextId::Ground), "never landed");
    let os = s.ground().oneshot.expect("landing action");
    assert!(LAND_FORWARD_MOVE.contains(&os.blend.id) || os.blend.id == LAND_STRAIGHT_MOVE, "{:#x}", os.blend.id);
    // speed ratio ~0.75 → bucket 2 (jog exit): weights in slots 1 (soft) and 4 (hard)
    let w = os.blend.weights();
    assert!(w[1] + w[4] > 0.99, "{w:?}");
}

/// Start hanging (wall hang) with the hands centred on `mid`, facing into the wall whose normal is `n`.
fn hang_at(mid: Vec3, n: Vec3) -> Sim {
    use crate::player::ledge::{LedgeEntry, LedgeSubState};
    use crate::player::{switch_context, TransitionSetup};
    let feet = mid - Vec3::Y * 1.1 + n * 0.5;
    let mut s = Sim::new(feet, crate::player::heading_of(-n));
    {
        let player = s.player;
        let w = s.app.world_mut();
        let mut q = w.query::<(&mut Locomotion, &mut HumanDataBundle)>();
        let (mut loco, mut data) = q.get_mut(w, player).unwrap();
        switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(LedgeEntry::at(mid, n, feet, LedgeSubState::Movement)));
    }
    s.run(1.0);
    assert_eq!(s.loco().current, ActorContextId::Ledge);
    s
}

#[test]
fn inner_corner_turns_onto_the_perpendicular_wall() {
    // L-wall: A's -Z face (z 49.7), B sticks out toward -Z at x 36 (its -X face is the inner corner)
    let mut s = hang_at(Vec3::new(34.0, 2.6, 49.7), Vec3::NEG_Z);
    s.pad(Vec3::X, 1.0, false, false); // facing +Z the player's left is +X
    let turned = s.run_until(8.0, |s| s.data().ledge.normal.dot(Vec3::NEG_X) > 0.99 && s.data().ledge.mv.is_none());
    assert!(turned, "no corner: {} hands {:?}", s.data().ledge.last_action, s.data().ledge.hand_l);
    let l = &s.data().ledge;
    assert!((l.hand_l.x - 35.92).abs() < 0.05 && (l.hand_l.y - 2.6).abs() < 0.05, "hands on B's stone ledge: {:?}", l.hand_l);
    assert_eq!(s.loco().current, ActorContextId::Ledge);
}

#[test]
fn side_jump_crosses_the_gap_between_ledges() {
    // C (x 17..23) and D (x 24..27): same edge line, 1 m gap
    let mut s = hang_at(Vec3::new(22.0, 2.6, 49.7), Vec3::NEG_Z);
    s.pad(Vec3::X, 1.0, false, false);
    let jumped = s.run_until(6.0, |s| s.data().ledge.last_action == "side jump");
    assert!(jumped, "no side jump: {}", s.data().ledge.last_action);
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()));
    let x = s.data().ledge.hand_l.x.min(s.data().ledge.hand_r.x);
    assert!(x > 23.9, "should hang on D: hands x {x}");
    assert_eq!(s.loco().current, ActorContextId::Ledge);
}

#[test]
fn outer_corner_wraps_around_the_wall_end() {
    // C's -X end (x 17): moving -X (the player's right) wraps onto C's -X face
    let mut s = hang_at(Vec3::new(18.0, 2.6, 49.7), Vec3::NEG_Z);
    s.pad(Vec3::NEG_X, 1.0, false, false);
    let turned = s.run_until(8.0, |s| s.data().ledge.normal.dot(Vec3::NEG_X) > 0.99 && s.data().ledge.mv.is_none());
    assert!(turned, "no outer corner: {} hands {:?}", s.data().ledge.last_action, s.data().ledge.hand_l);
    assert!((s.data().ledge.hand_l.x - 17.0).abs() < 0.05);
}

#[test]
fn hop_up_reaches_the_ledge_above_and_swings_into_a_free_hang() {
    use crate::player::ledge_moves::{MoveKind, HOP_UP};
    let mut s = hang_at(Vec3::new(42.0, 2.6, 49.7), Vec3::NEG_Z);
    s.pad(Vec3::Z, 1.0, false, false); // up
    let hopped = s.run_until(2.0, |s| s.data().ledge.mv.is_some_and(|m| m.kind == MoveKind::HopUp));
    assert!(hopped, "no hop: {}", s.data().ledge.last_action);
    let mv = s.data().ledge.mv.unwrap();
    assert_eq!(mv.seq[0].map(|a| a.id), Some(HOP_UP));
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()));
    let l = &s.data().ledge;
    assert!((l.hand_l.y - 4.2).abs() < 0.05, "hands on E2: {:?}", l.hand_l);
    assert_eq!(l.hang_type, ledge::LedgeHangType::Free);
    assert!((s.body().feet.y - (4.2 - 2.4)).abs() < 0.05, "free-hang root: {:?}", s.body().feet);
}

/// Walk into a wall (facing +Z) with high profile + Legs from `feet`; return once the straight jump started.
fn straight_jump_at(feet: Vec3) -> Sim {
    let mut s = Sim::new(feet, FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir), "never jumped: {:?}", s.body().feet);
    s.pad(Vec3::Z, 0.0, false, false);
    s
}

#[test]
fn knee_height_block_jumps_and_stands_on_top() {
    use crate::player::ledge_moves::{HangEnd, ACT_KNEE_TO_FREESTEP};
    let mut s = straight_jump_at(Vec3::new(50.0, 0.0, 49.0));
    let j = s.data().air.target.and_then(|t| t.straight).expect("straight jump band");
    assert_eq!((j.flight, j.end), (0x0127_2A69, HangEnd::StandFromKnee), "1.6 m: jumpstraight_to_hangknee");
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ledge));
    let mv = s.data().ledge.mv.expect("reception");
    assert_eq!(mv.seq[2].map(|a| a.id), Some(ACT_KNEE_TO_FREESTEP[0]), "hangknee -> free-step entry (0xDE2EE0)");
    assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::Ground), "never stood up");
    assert!((s.body().feet.y - 1.6).abs() < 0.05, "on top of the block: {:?}", s.body().feet);
}

#[test]
fn a_box_below_seventy_centimetres_is_a_free_step_up() {
    use crate::player::ledge_moves::{HangEnd, RECEPTION_STEP_UP};
    // the 0.6 m box at (9, 0), 1.5 m square: the < 0.7 m band of 0xB21DA0 (collide_full_*_to_freestep, flags 1)
    let mut s = straight_jump_at(Vec3::new(9.0, 0.0, -1.6));
    let j = s.data().air.target.and_then(|t| t.straight).expect("straight jump band");
    assert_eq!((j.flight, j.end, j.flags), (0x012B_291B, HangEnd::FreeStep, 1));
    assert!((j.b - 0.5).abs() < 0.01, "0.6 m: halfway between the 50 and 70 cm clips: {}", j.b);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ledge));
    assert_eq!(s.data().ledge.mv.and_then(|m| m.seq[0]).map(|a| a.id), Some(RECEPTION_STEP_UP));
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground), "never stood up");
    let f = s.body().feet;
    assert!((f.y - 0.6).abs() < 0.05 && f.z > -0.75, "on top of the box: {f:?}");
}

#[test]
fn two_metre_wall_jumps_into_a_wall_hang() {
    let mut s = straight_jump_at(Vec3::new(56.0, 0.0, 49.0));
    let j = s.data().air.target.and_then(|t| t.straight).expect("straight jump band");
    assert_eq!(j.flight, 0x0127_1631, "2.2 m with a wall below: jumpstraight_to_hangwall");
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ledge));
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()));
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Wall);
    assert!((s.body().feet.y - 1.1).abs() < 0.05, "wall-hang root 1.1 m below the hands: {:?}", s.body().feet);
}


#[test]
fn shimmy_from_a_free_hang_switches_to_a_wall_hang() {
    use crate::player::ledge_moves::{MoveKind, TO_WALL};
    // the 2.6 m straight jump arrives in a free hang; the first step onto wall below plays hangfree_tr_hangwall
    let mut s = hang_on_jump_up_wall();
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()));
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Free);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(1.0, |s| s.data().ledge.mv.is_some()));
    let mv = s.data().ledge.mv.unwrap();
    assert_eq!(mv.kind, MoveKind::SwitchHang { to_wall: true });
    assert_eq!(mv.seq[0].map(|a| a.id), Some(TO_WALL[0]), "moving left: hangfree_tr_hangwall_left");
    s.pad(Vec3::X, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().ledge.mv.is_none()));
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Wall);
    assert!((s.body().feet.y - 1.5).abs() < 0.05, "wall-hang root: {:?}", s.body().feet);
}

#[test]
fn shimmy_onto_an_overhang_switches_to_a_free_hang() {
    use crate::player::ledge_moves::{MoveKind, TO_FREE};
    // wall F (x 60..63) continues as an overhang slab (x 63..66) without wall below
    let mut s = hang_at(Vec3::new(62.3, 2.6, 49.7), Vec3::NEG_Z);
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Wall);
    s.pad(Vec3::X, 1.0, false, false); // the player's left
    let switched = s.run_until(6.0, |s| s.data().ledge.mv.is_some_and(|m| m.kind == MoveKind::SwitchHang { to_wall: false }));
    assert!(switched, "no switch: {} hands {:?}", s.data().ledge.last_action, s.data().ledge.hand_l);
    assert_eq!(s.data().ledge.mv.unwrap().seq[0].map(|a| a.id), Some(TO_FREE[0]), "hangwall_tr_hangfree_left");
    s.pad(Vec3::X, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().ledge.mv.is_none()));
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Free);
    assert!((s.body().feet.y - 0.2).abs() < 0.05, "free-hang root: {:?}", s.body().feet);
}

#[test]
fn pull_down_from_a_roof_edge_into_a_wall_hang() {
    use crate::player::ledge_moves::{MoveKind, PULLDOWN_DESCENT, PULLDOWN_ORIENT};
    // roof A: x 5..11, h 3.5; stand near its +X edge facing +X (over the drop)
    let mut s = Sim::new(Vec3::new(10.6, 3.5, 12.0), -std::f32::consts::FRAC_PI_2);
    s.run(0.2);
    s.press_legs();
    assert!(s.run_until(0.5, |s| s.loco().current == ActorContextId::Ledge), "no pull-down: {:?}", s.loco().current);
    let mv = s.data().ledge.mv.expect("orientation");
    assert_eq!(mv.kind, MoveKind::PullDown { stage: 1 });
    assert_eq!(mv.seq[0].map(|a| a.id), Some(PULLDOWN_ORIENT[1]));
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_some_and(|m| m.kind == MoveKind::PullDown { stage: 2 })));
    assert_eq!(s.data().ledge.mv.unwrap().seq[0].map(|a| a.id), Some(PULLDOWN_DESCENT));
    assert!(s.run_until(4.0, |s| s.data().ledge.mv.is_none()), "never settled: {:?}", s.data().ledge.mv.map(|m| m.kind));
    let l = &s.data().ledge;
    assert_eq!(l.hang_type, ledge::LedgeHangType::Wall);
    assert!(l.normal.dot(Vec3::X) > 0.99, "hanging on the +X face: {:?}", l.normal);
    assert!((l.hand_l.y - 3.5).abs() < 0.05);
    let f = s.body().feet;
    assert!((f.y - 2.4).abs() < 0.05 && (f.x - 11.5).abs() < 0.05, "wall-hang root: {f:?}");
    assert_eq!(s.loco().current, ActorContextId::Ledge);
}

#[test]
fn walking_into_a_roof_edge_stops_then_steps_back() {
    use crate::player::ledge_moves::{LEDGE_STOP_END, LEDGE_STOP_START};
    // the 6 m block (x -14.5..-9.5): a drop of more than 5 m (0xEE88A9); walk toward its +X edge
    let edge = -9.5f32;
    let mut s = Sim::new(Vec3::new(-12.0, 6.0, 4.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(3.0, |s| s.ground().ledge_stop.is_some()), "never stopped: {:?}", s.body().feet);
    assert_eq!(s.ground().oneshot.map(|o| o.blend.id), Some(LEDGE_STOP_START));
    assert!(s.run_until(1.0, |s| s.ground().ledge_stop.is_some_and(|l| l.ending)));
    assert_eq!(s.ground().oneshot.map(|o| o.blend.id), Some(LEDGE_STOP_END));
    let at_end = s.body().feet;
    assert!(at_end.x < edge - 0.03, "feet stayed behind the edge: {at_end:?}");
    assert!(s.run_until(1.5, |s| s.ground().ledge_stop.is_none()));
    // the end action steps back (~0.5 m), still on the roof, no fall
    assert!(s.body().feet.x < at_end.x - 0.3, "stepped back: {:?}", s.body().feet);
    assert_eq!(s.loco().current, ActorContextId::Ground);
    // still pushing into the edge: no new stop until the stick lets go (PORT lock)
    s.run(1.0);
    assert!(s.ground().ledge_stop.is_none());
    assert_eq!(s.loco().current, ActorContextId::Ground, "held at the edge, no fall");
    assert!(s.body().feet.x < edge - 0.03, "held behind the edge: {:?}", s.body().feet);
    s.pad(Vec3::X, 0.0, false, false);
    s.run(0.1);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(1.0, |s| s.ground().ledge_stop.is_some()), "stops again after letting go");
}

#[test]
fn pull_down_from_the_ledge_stop_uses_the_edge_stop_orientation() {
    use crate::player::ledge_moves::{MoveKind, PULLDOWN_ORIENT};
    let mut s = Sim::new(Vec3::new(-11.0, 6.0, 4.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(3.0, |s| s.ground().ledge_stop.is_some()));
    s.press_legs();
    assert!(s.run_until(0.3, |s| s.loco().current == ActorContextId::Ledge), "no pull-down: {:?}", s.loco().current);
    let mv = s.data().ledge.mv.expect("orientation");
    assert_eq!(mv.kind, MoveKind::PullDown { stage: 1 });
    assert_eq!(mv.seq[0].map(|a| a.id), Some(PULLDOWN_ORIENT[0]));
    assert!(s.run_until(4.0, |s| s.data().ledge.mv.is_none() && s.data().ledge.queue.is_empty()));
    assert_eq!(s.loco().current, ActorContextId::Ledge);
}

#[test]
fn leap_of_faith_into_the_haystack_then_hop_out() {
    use crate::player::hay::{HayPhase, HAYSTACK_FAITH_LANDING, HAYSTACK_HOP_OUT, HAYSTACK_WAIT};
    use crate::player::jump_blend::{FLIGHT_FAITH, TAKEOFF_FAITH};
    // high block: x 27..33, roof 9.5 m; haystack at (37.5, 26), 2.2 m wide, 1 m high
    let mut s = Sim::new(Vec3::new(30.5, 9.5, 26.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir), "never jumped");
    let air = &s.data().air;
    assert_eq!(air.target_flags, 0x800);
    assert_eq!(air.flight.map(|f| f.id), Some(FLIGHT_FAITH));
    assert!(air.takeoff.is_some_and(|t| TAKEOFF_FAITH.contains(&t.id)));
    s.pad(Vec3::X, 0.0, false, false);
    assert!(s.run_until(5.0, |s| s.loco().current == ActorContextId::HayStack), "never reached the haystack: {:?}", s.loco().current);
    assert_eq!(s.data().hay.action.map(|a| a.id), Some(HAYSTACK_FAITH_LANDING));
    assert!(s.run_until(2.0, |s| s.data().hay.phase == HayPhase::Waiting));
    assert_eq!(s.data().hay.action.map(|a| a.id), Some(HAYSTACK_WAIT));
    let f = s.body().feet;
    assert!((f - Vec3::new(37.5, 0.0, 26.0)).length() < 0.05, "inside the stack: {f:?}");
    // no fall damage: the haystack takes the landing
    assert!(s.ground().last_landing.is_none());
    s.run(0.3);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Ground));
    assert_eq!(s.ground().oneshot.map(|o| o.blend.id), Some(HAYSTACK_HOP_OUT));
    s.run(0.6);
    assert!(s.body().feet.x > 38.0 && s.body().feet.y.abs() < 0.05, "hopped out: {:?}", s.body().feet);
}

#[test]
fn releasing_the_stick_stops_quickly_like_the_game() {
    use crate::player::jump_blend::{RUN_STOP, RUN_STOP_TO_WAIT};
    // run (high profile), release: RunStop action + its settle into the wait (~0.7 s), short slide
    let mut s = Sim::new(Vec3::new(0.0, 0.0, -30.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, false);
    s.run(2.0);
    let x0 = s.body().feet.x;
    s.pad(Vec3::X, 0.0, false, false);
    s.run(1.0 / 60.0 + 1e-4);
    assert!(s.ground().oneshot.is_some_and(|o| RUN_STOP.contains(&o.blend.id)), "run stop playing");
    assert_eq!(s.ground().speed_param, 0.0);
    assert!(s.run_until(1.2, |s| s.ground().oneshot.is_none()), "stop + settle within 1.2 s");
    let slide = s.body().feet.x - x0;
    assert!((0.2..2.0).contains(&slide), "run stop slide {slide}");
    let _ = RUN_STOP_TO_WAIT;
    // walk, release: stopped at once
    let mut s = Sim::new(Vec3::new(0.0, 0.0, -30.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, false, false);
    s.run(1.5);
    let x0 = s.body().feet.x;
    s.pad(Vec3::X, 0.0, false, false);
    s.run(0.3);
    assert!(s.body().feet.x - x0 < 0.05, "walk stop slid {}", s.body().feet.x - x0);
    assert!(s.ground().oneshot.is_none());
}

#[test]
fn pull_up_does_not_cut_through_the_wall() {
    // the root follows the pull-up clips (up, then in over the lip): body points stay outside the wall
    let mut s = hang_on_jump_up_wall();
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()));
    let c = crate::level::geometry().0;
    s.pad(Vec3::Z, 1.0, false, false);
    let mut worst = 0usize;
    let mut frames = 0;
    for _ in 0..240 {
        s.run(1.0 / 60.0 + 1e-4);
        if s.loco().current != ActorContextId::Ledge {
            break;
        }
        frames += 1;
        let f = s.body().feet;
        worst += [0.3f32, 0.9, 1.5].iter().filter(|&&h| c.point_inside(f + Vec3::Y * h)).count();
    }
    assert!(frames > 10, "pull-up ran {frames} frames");
    assert_eq!(worst, 0, "body inside the wall on {worst} samples");
    assert_eq!(s.loco().current, ActorContextId::Ground);
    assert!((s.body().feet.y - 2.6).abs() < 0.05);
}

#[test]
fn free_hang_against_a_wall_keeps_the_body_out_of_it() {
    // the 2.6 m jump-up wall: free hang with wall under it = WallFree, root 0.5 m out (hangwallfree_wait)
    let mut s = hang_on_jump_up_wall();
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()));
    let l = &s.data().ledge;
    let f = s.body().feet;
    let out = (f - (l.hand_l + l.hand_r) * 0.5).dot(l.normal);
    assert!((out - 0.5).abs() < 0.05, "root 0.5 m out from the edge: {out}");
    let c = crate::level::geometry().0;
    // body in the clip: feet 0.30-0.39 m and hips 0.36 m in front of the root, i.e. 0.11-0.20 m off the wall
    for h in [0.3f32, 1.2, 1.8] {
        assert!(!c.point_inside(f + Vec3::Y * h - l.normal * 0.3), "body point {h} m up inside the wall");
    }
}

/// Run at a wall face (z 59.25) with high profile, press Legs, keep holding it.
fn wall_run_at(x: f32) -> Sim {
    let mut s = Sim::new(Vec3::new(x, 0.0, 55.5), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, false);
    // the wall ray is 1.5 m long from the chest (0xE18390): press Legs once within reach
    assert!(s.run_until(2.0, |s| s.body().feet.z > 58.0));
    s.pad(Vec3::Z, 1.0, true, true);
    s.press_legs();
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Walling), "no wall run: {:?} at {:?}", s.loco().current, s.body().feet);
    s
}

#[test]
fn wall_run_pulls_up_onto_a_low_wall() {
    use crate::player::walling::{WallingSubState, ENTRY_A};
    let mut s = wall_run_at(70.0);
    assert_eq!(s.data().walling.action.map(|a| a.id), Some(ENTRY_A));
    assert_eq!(s.data().walling.sub_state, WallingSubState::EntryA);
    // EntryA ends 0.5 m out from the wall, 1.0 m up (the entry clip's rise = the warp target, 0xE18390)
    assert!(s.run_until(0.5, |s| s.data().walling.sub_state == WallingSubState::EntryB));
    let f = s.body().feet;
    assert!((f.y - 1.0).abs() < 0.05 && (59.25 - f.z - 0.5).abs() < 0.05, "entry root: {f:?}");
    // probe A (ledge 0.8 m above the root): entry_footl_tr_hangknee → hangknee → wait, standing on top
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground), "never on top: {:?}", s.loco().current);
    assert!((s.body().feet.y - 1.8).abs() < 0.05, "on the 1.8 m top: {:?}", s.body().feet);
}

#[test]
fn wall_run_steps_up_then_hangs_from_a_higher_edge() {
    use crate::player::walling::WallingSubState;
    let mut s = wall_run_at(76.0);
    assert!(s.run_until(1.0, |s| s.data().walling.sub_state == WallingSubState::Vertical), "no vertical step");
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ledge), "never hung: {:?}", s.loco().current);
    assert!(s.run_until(2.0, |s| s.data().ledge.mv.is_none()));
    let l = &s.data().ledge;
    assert!((l.hand_l.y - 3.8).abs() < 0.05, "hands on the 3.8 m edge: {:?}", l.hand_l);
}

#[test]
fn wall_run_without_a_ledge_drops_back() {
    use crate::player::walling::WallingSubState;
    let mut s = wall_run_at(82.0);
    assert!(s.run_until(1.0, |s| s.data().walling.sub_state == WallingSubState::VerticalEnd), "no vertical end");
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground), "never landed: {:?}", s.loco().current);
    assert!(s.body().feet.y.abs() < 0.05);
}

#[test]
fn releasing_legs_on_the_wall_keeps_running() {
    use crate::player::walling::WallingSubState;
    // the interpreter's walling state (0xEE05E0) always sends command 0: letting go of Legs does not drop off the wall
    let mut s = wall_run_at(82.0);
    s.pad(Vec3::Z, 1.0, true, false);
    assert!(s.run_until(1.5, |s| s.data().walling.sub_state == WallingSubState::VerticalEnd), "fell off: {:?}", s.loco().current);
}

#[test]
fn pressing_legs_on_the_wall_rebounds() {
    use crate::player::walling::WallingSubState;
    // ReboundJump 0xE365C0: a fresh Legs press while running up (Vertical) pushes off along the stick (pulled away from
    // the wall); with nothing to land on, toward the fallback target 7 m out, 3 m down
    let mut s = wall_run_at(82.0);
    assert!(s.run_until(1.0, |s| s.data().walling.sub_state == WallingSubState::Vertical), "no vertical step");
    s.pad(Vec3::NEG_Z, 1.0, true, true);
    s.press_legs();
    assert!(s.run_until(0.1, |s| s.loco().current == ActorContextId::InAir), "no rebound: {:?}", s.loco().current);
    let z0 = s.body().feet.z;
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground));
    assert!(s.body().feet.z < z0 - 2.0, "pushed off away from the wall: {:?}", s.body().feet);
}

#[test]
fn pressing_legs_with_the_stick_at_the_wall_rebounds_straight_back() {
    use crate::player::walling::WallingSubState;
    // the stick within 50° of the facing (into the wall) pushes straight back from it (0xEE05E0)
    let mut s = wall_run_at(82.0);
    assert!(s.run_until(1.0, |s| s.data().walling.sub_state == WallingSubState::Vertical), "no vertical step");
    let x0 = s.body().feet.x;
    s.pad((Vec3::Z + Vec3::X * 0.5).normalize(), 1.0, true, true);
    s.press_legs();
    assert!(s.run_until(0.1, |s| s.loco().current == ActorContextId::InAir), "no rebound");
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground));
    assert!((s.body().feet.x - x0).abs() < 0.3, "straight back, not sideways: {:?} from x {x0}", s.body().feet);
}

#[test]
fn walk_across_a_beam_onto_the_far_platform() {
    use crate::player::narrow::{BeamState, BEAM_WALK};
    // platform A x 68..72 (top 4 m), beam x 72..78 at 4 m, platform B x 78..82
    let mut s = Sim::new(Vec3::new(71.0, 4.0, 70.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::NarrowObject), "never mounted: {:?} {:?}", s.loco().current, s.body().feet);
    assert!(s.run_until(2.0, |s| s.data().narrow.action.is_some_and(|a| a.id == BEAM_WALK)), "never walked: {:?}", s.data().narrow.state);
    // on the line, at the beam's height
    let f = s.body().feet;
    assert!((f.z - 70.0).abs() < 0.01 && (f.y - 4.0).abs() < 0.01, "on the beam: {f:?}");
    assert!(s.run_until(8.0, |s| s.loco().current == ActorContextId::Ground), "never stepped off: {:?} {:?} {:?}", s.loco().current, s.data().narrow.state, s.body().feet);
    s.run(0.5);
    let f = s.body().feet;
    assert!(f.x > 78.0 && (f.y - 4.0).abs() < 0.05, "on platform B: {f:?}");
    let _ = BeamState::Walk;
}

#[test]
fn turn_around_on_a_beam() {
    use crate::player::narrow::BeamState;
    let mut s = Sim::new(Vec3::new(71.0, 4.0, 70.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::NarrowObject));
    s.run(1.5);
    s.pad(Vec3::X, 0.0, false, false);
    assert!(s.run_until(1.0, |s| s.data().narrow.state == BeamState::Wait));
    s.pad(Vec3::NEG_X, 1.0, false, false);
    assert!(s.run_until(0.5, |s| s.data().narrow.state == BeamState::Turn), "no turn: {:?}", s.data().narrow.state);
    assert!(s.run_until(1.0, |s| s.data().narrow.state != BeamState::Turn));
    assert!(!s.data().narrow.toward_p1, "now facing back toward p0");
    assert!(s.body().forward().dot(Vec3::NEG_X) > 0.99);
}

// ---------------------------------------------------------------- beams from the air, beam jumps, pilotis (RE/05 §2.8)

/// Free-run off platform P (x 68..72, top 3 m) toward the posts at x 74.5 / 77 / 79.5 (z 80).
fn on_pilotis_row() -> Sim {
    let mut s = Sim::new(Vec3::new(69.0, 3.0, 80.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    s
}

/// Switch the player straight into a context (test setup).
fn force(s: &mut Sim, setup: crate::player::TransitionSetup) {
    let w = s.app.world_mut();
    let mut e = w.entity_mut(s.player);
    let (mut loco, mut data) = (e.take::<Locomotion>().unwrap(), e.take::<HumanDataBundle>().unwrap());
    crate::player::switch_context(&mut loco, &mut data, setup);
    e.insert((loco, data));
}

#[test]
fn free_run_onto_a_pilotis_and_hop_along_the_posts() {
    use crate::player::narrow::{BeamState, NarrowKind};
    let mut s = on_pilotis_row();
    assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::NarrowObject), "never reached a post: {:?} {:?}", s.loco().current, s.body().feet);
    assert_eq!(s.data().narrow.kind, NarrowKind::Pilotis);
    s.pad(Vec3::X, 0.0, true, false);
    assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::PilotisWait), "no wait: {:?}", s.data().narrow.state);
    let f = s.body().feet;
    assert!((f - Vec3::new(74.5, 3.0, 80.0)).length() < 0.05, "on the first post top: {f:?}");
    // hop to the next posts with high profile + Legs + the stick
    for x in [77.0f32, 79.5] {
        s.pad(Vec3::X, 1.0, true, false);
        s.press_legs();
        assert!(s.run_until(0.3, |s| s.loco().current == ActorContextId::InAir), "no jump from the post");
        s.pad(Vec3::X, 0.0, true, false);
        assert!(
            s.run_until(3.0, |s| s.data().narrow.state == BeamState::PilotisWait && s.loco().current == ActorContextId::NarrowObject),
            "no arrival at {x}: {:?} {:?}",
            s.loco().current,
            s.body().feet
        );
        let f = s.body().feet;
        assert!((f - Vec3::new(x, 3.0, 80.0)).length() < 0.05, "on the post at {x}: {f:?}");
    }
    // and off onto platform Q
    s.pad(Vec3::X, 1.0, true, false);
    s.press_legs();
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground), "never landed on Q: {:?}", s.loco().current);
    let f = s.body().feet;
    assert!(f.x > 82.0 && (f.y - 3.0).abs() < 0.05, "on platform Q: {f:?}");
}

#[test]
fn pilotis_impulsion_and_jump_on_the_spot() {
    use crate::player::narrow::{BeamState, PilotisEntry, PilotisEntryType};
    let top = Vec3::new(74.5, 3.0, 80.0);
    let mut s = Sim::new(top, -std::f32::consts::FRAC_PI_2);
    force(&mut s, crate::player::TransitionSetup::ToPilotis(PilotisEntry { top, from: top, facing: Vec3::X, kind: PilotisEntryType::FromInAir, foot: 0 }));
    assert!(s.run_until(1.5, |s| s.data().narrow.state == BeamState::PilotisWait));
    s.pad(Vec3::ZERO, 0.0, true, false);
    s.press_legs();
    assert!(s.run_until(0.3, |s| s.data().narrow.state == BeamState::ImpulseIn), "no impulsion: {:?}", s.data().narrow.state);
    assert!(s.run_until(1.0, |s| s.data().narrow.state == BeamState::ImpulseWait));
    s.press_legs();
    assert!(s.run_until(0.5, |s| s.loco().current == ActorContextId::InAir), "no jump on the spot: {:?}", s.data().narrow.state);
    let mut top_y: f32 = 0.0;
    for _ in 0..120 {
        s.run(1.0 / 60.0 + 1e-4);
        top_y = top_y.max(s.body().feet.y);
        if s.loco().current == ActorContextId::NarrowObject {
            break;
        }
    }
    assert!((top_y - 4.0).abs() < 0.05, "rose 1.0 m (beam_jumpstraight_clear): {top_y}");
    assert_eq!(s.loco().current, ActorContextId::NarrowObject, "caught back on the post");
    assert!(s.run_until(1.5, |s| s.data().narrow.state == BeamState::PilotisWait));
    assert!((s.body().feet - top).length() < 0.05);
}

#[test]
fn running_jump_onto_a_beam_mounts_it_straight() {
    use crate::player::narrow::{BeamEntryMode, BeamState, NarrowKind};
    // platform B (x 78..82, top 4) → the free beam x 84.5..90.5 at z 70
    let mut s = Sim::new(Vec3::new(79.0, 4.0, 70.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::NarrowObject), "never reached the beam: {:?} {:?}", s.loco().current, s.body().feet);
    let n = &s.data().narrow;
    assert_eq!(n.kind, NarrowKind::Beam);
    assert_eq!(n.entry_mode, BeamEntryMode::Straight);
    assert!(n.toward_p1);
    let f = s.body().feet;
    assert!((f.z - 70.0).abs() < 0.01 && (f.y - 4.0).abs() < 0.01 && f.x > 84.5, "on the beam line: {f:?}");
    // keeps walking along it while the stick is held
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::Walk));
}

#[test]
fn falling_onto_a_beam_is_caught() {
    use crate::player::narrow::{BeamEntryMode, BeamState};
    // drop from 1.5 m above the free beam (5 cm off its line)
    let from = Vec3::new(86.0, 5.5, 70.05);
    let mut s = Sim::new(from, -std::f32::consts::FRAC_PI_2);
    force(&mut s, crate::player::TransitionSetup::ToInAir(air::InAirEntry::Fall { from, velocity: Vec3::ZERO, origin: air::FallOrigin::Ground, speed_param: 0.0 }));
    assert!(s.run_until(2.0, |s| s.loco().current != ActorContextId::InAir));
    assert_eq!(s.loco().current, ActorContextId::NarrowObject, "caught on the beam, not a ground landing");
    assert_eq!(s.data().narrow.entry_mode, BeamEntryMode::Reception);
    assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::Wait));
    let f = s.body().feet;
    assert!((f.z - 70.0).abs() < 0.01 && (f.y - 4.0).abs() < 0.01, "on the line: {f:?}");
}

#[test]
fn beam_jump_at_a_ledge_above() {
    use crate::player::narrow::{BeamEntry, BeamEntryMode, BeamState, BEAM_IMPULSE_TO_JUMP};
    // on the free beam at x 87.4 facing +X; the slab edge (x 88, top 6.3) is 2.3 m above
    let point = Vec3::new(87.4, 4.0, 70.0);
    let mut s = Sim::new(point, -std::f32::consts::FRAC_PI_2);
    let (p0, p1) = (Vec3::new(84.5, 4.0, 70.0), Vec3::new(90.5, 4.0, 70.0));
    force(&mut s, crate::player::TransitionSetup::ToBeam(BeamEntry { p0, p1, point, from: point, toward_p1: true, mode: BeamEntryMode::Straight, foot: 0, action: None, facing: Vec3::X }));
    s.run(0.2);
    s.pad(Vec3::ZERO, 0.0, true, false);
    s.press_legs();
    assert!(s.run_until(1.0, |s| s.data().narrow.state == BeamState::ImpulseWait), "no impulsion: {:?}", s.data().narrow.state);
    s.press_legs();
    assert!(s.run_until(0.3, |s| s.data().narrow.state == BeamState::JumpOnPlace));
    assert!(s.data().narrow.action.is_some_and(|a| a.id == BEAM_IMPULSE_TO_JUMP), "hand target found");
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::InAir));
    let flight = s.data().air.flight.unwrap().id;
    assert_eq!(flight, 0x516D_52DE, "beam_jumpstraight_to_hangwaist");
    assert!(s.run_until(5.0, |s| s.loco().current == ActorContextId::Ground), "never stood on the slab: {:?} {:?}", s.loco().current, s.body().feet);
    let f = s.body().feet;
    assert!(f.x > 88.0 && (f.y - 6.3).abs() < 0.05, "on top of the slab: {f:?}");
}

// ---------------------------------------------------------------- beam input: 90° waits, pull-down, wall run (RE/05 §2.10)

/// Standing on the free beam (x 84.5..90.5, z 70, 4 m) at `x`, facing +X.
fn on_free_beam(x: f32) -> Sim {
    use crate::player::narrow::{BeamEntry, BeamEntryMode};
    let point = Vec3::new(x, 4.0, 70.0);
    let mut s = Sim::new(point, -std::f32::consts::FRAC_PI_2);
    let (p0, p1) = (Vec3::new(84.5, 4.0, 70.0), Vec3::new(90.5, 4.0, 70.0));
    force(&mut s, crate::player::TransitionSetup::ToBeam(BeamEntry { p0, p1, point, from: point, toward_p1: true, mode: BeamEntryMode::Straight, foot: 0, action: None, facing: Vec3::X }));
    s.run(0.2);
    s
}

#[test]
fn stick_to_the_side_on_a_beam_turns_to_the_90_wait_and_back() {
    use crate::player::narrow::{BeamState, BEAM_WAIT_90};
    // Idle +91 → TurnTo90 (0xF77640) → Wait90 +100 facing across; then the stick along the beam turns back
    // (0xF78630) and the stick back turns around across it (0xF78740)
    let mut s = on_free_beam(87.0);
    s.pad(Vec3::Z, 1.0, false, false);
    assert!(s.run_until(0.3, |s| s.data().narrow.state == BeamState::TurnTo90), "no turn: {:?}", s.data().narrow.state);
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::Wait90), "no 90° wait: {:?}", s.data().narrow.state);
    assert!(s.data().narrow.action.is_some_and(|a| a.id == BEAM_WAIT_90));
    assert!(s.body().forward().dot(Vec3::Z) > 0.99, "facing across toward +Z: {:?}", s.body().forward());
    let f = s.body().feet;
    assert!((f - Vec3::new(87.0, 4.0, 70.0)).length() < 0.01, "stays on the line: {f:?}");
    // back: turn around across the beam
    s.pad(Vec3::NEG_Z, 1.0, false, false);
    assert!(s.run_until(0.3, |s| s.data().narrow.state == BeamState::Turn90), "no turn-around: {:?}", s.data().narrow.state);
    s.pad(Vec3::NEG_Z, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::Wait90));
    assert!(s.body().forward().dot(Vec3::NEG_Z) > 0.99, "facing -Z: {:?}", s.body().forward());
    // to a side: back along the beam toward -X
    s.pad(Vec3::NEG_X, 1.0, false, false);
    assert!(s.run_until(0.3, |s| s.data().narrow.state == BeamState::TurnBack), "no turn back: {:?}", s.data().narrow.state);
    s.pad(Vec3::NEG_X, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::Wait), "no wait: {:?}", s.data().narrow.state);
    assert!(!s.data().narrow.toward_p1 && s.data().narrow.across.is_none());
    assert!(s.body().forward().dot(Vec3::NEG_X) > 0.99, "facing -X: {:?}", s.body().forward());
}

#[test]
fn empty_hand_on_a_beam_pulls_down_to_a_hang_under_it() {
    use crate::player::ledge_moves::{MoveKind, PULLDOWN_BEAM_DESCENT, PULLDOWN_BEAM_ORIENT};
    // event 9 (0xE504D0 → 0xE4EF70): the side edge toward the stick, 4 m above the ground → Ledge pull-down type 3
    let mut s = on_free_beam(87.0);
    s.pad(Vec3::NEG_Z, 1.0, false, false);
    s.press_hand();
    assert!(s.run_until(0.2, |s| s.loco().current == ActorContextId::Ledge), "no pull-down: {:?} {:?}", s.loco().current, s.data().narrow.state);
    let m = s.data().ledge.mv.expect("orientation stage");
    assert_eq!(m.kind, MoveKind::PullDown { stage: 1 });
    assert_eq!(m.seq[0].map(|a| a.id), Some(PULLDOWN_BEAM_ORIENT[0]), "beam_pilotis_to_pulldown_soft_front_orientation");
    assert!(m.normal.dot(Vec3::NEG_Z) > 0.99, "the -Z side edge: {:?}", m.normal);
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_some_and(|m| m.kind == MoveKind::PullDown { stage: 2 })));
    assert_eq!(s.data().ledge.mv.unwrap().seq[0].map(|a| a.id), Some(PULLDOWN_BEAM_DESCENT[0]));
    assert!(s.run_until(4.0, |s| s.data().ledge.mv.is_none()), "never settled: {:?}", s.data().ledge.mv.map(|m| m.kind));
    assert_eq!(s.loco().current, ActorContextId::Ledge);
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Free, "nothing below a beam: free hang");
    let hands = (s.data().ledge.hand_l + s.data().ledge.hand_r) * 0.5;
    assert!((hands.y - 4.0).abs() < 0.05 && (hands.z - 69.9).abs() < 0.05, "hands on the beam's -Z edge: {hands:?}");
    assert!(s.body().feet.y < 3.0, "hanging below the beam: {:?}", s.body().feet);
}

#[test]
fn empty_hand_on_a_pilotis_pulls_down_to_a_hang() {
    use crate::player::ledge_moves::{MoveKind, PULLDOWN_BEAM_ORIENT};
    use crate::player::narrow::{BeamState, PilotisEntry, PilotisEntryType};
    let top = Vec3::new(77.0, 3.0, 80.0);
    let mut s = Sim::new(top, -std::f32::consts::FRAC_PI_2);
    // No stick: the native interpreter picks the camera direction, here the +X edge.
    s.app.world_mut().resource_mut::<CameraRig>().yaw = std::f32::consts::FRAC_PI_2;
    force(&mut s, crate::player::TransitionSetup::ToPilotis(PilotisEntry { top, from: top, facing: Vec3::X, kind: PilotisEntryType::FromInAir, foot: 0 }));
    assert!(s.run_until(1.5, |s| s.data().narrow.state == BeamState::PilotisWait));
    s.press_hand();
    assert!(s.run_until(0.2, |s| s.loco().current == ActorContextId::Ledge), "no pull-down: {:?}", s.loco().current);
    let m = s.data().ledge.mv.expect("orientation stage");
    assert_eq!(m.kind, MoveKind::PullDown { stage: 1 });
    assert_eq!(m.seq[0].map(|a| a.id), Some(PULLDOWN_BEAM_ORIENT[0]));
    assert!(m.normal.dot(Vec3::X) > 0.99, "the post's +X edge, ahead (no stick: the facing): {:?}", m.normal);
    assert!(s.run_until(4.0, |s| s.data().ledge.mv.is_none()), "never hung");
    assert_eq!(s.loco().current, ActorContextId::Ledge);
}

#[test]
fn high_profile_legs_on_a_beam_runs_up_the_wall_ahead() {
    use crate::player::narrow::BeamState;
    // platform C → beam x 102..108.8 → wall Y at x 109: walking along it in high profile, Legs within 1.5 m of the
    // wall starts the wall run (event 15, 0xF77DC0 → Walling)
    let mut s = Sim::new(Vec3::new(101.0, 4.0, 70.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::NarrowObject), "never mounted: {:?}", s.loco().current);
    assert!(s.run_until(8.0, |s| s.body().feet.x > 107.5), "never walked near the wall: {:?} {:?}", s.data().narrow.state, s.body().feet);
    assert!(matches!(s.data().narrow.state, BeamState::Walk | BeamState::Start), "still walking: {:?}", s.data().narrow.state);
    s.pad(Vec3::X, 1.0, true, false);
    s.press_legs();
    assert!(s.run_until(0.3, |s| s.loco().current == ActorContextId::Walling), "no wall run: {:?} {:?}", s.loco().current, s.data().narrow.state);
}

#[test]
fn jogging_a_beam_with_the_stick_at_a_branch_hops_onto_it() {
    use crate::player::narrow::{BeamEntry, BeamEntryMode, BeamState, BEAM_CORNER_HOP};
    // beam x 102..108.8 (z 70) with a branch at x 105 toward +Z (from z 70.3): jogging +X, the stick to +Z near
    // the branch → event 7 → the corner hop onto it (0xF7D2F0 → +151 / +154 / +157), then on along it
    let point = Vec3::new(102.8, 4.0, 70.0);
    let mut s = Sim::new(point, -std::f32::consts::FRAC_PI_2);
    let (p0, p1) = (Vec3::new(102.0, 4.0, 70.0), Vec3::new(108.8, 4.0, 70.0));
    force(&mut s, crate::player::TransitionSetup::ToBeam(BeamEntry { p0, p1, point, from: point, toward_p1: true, mode: BeamEntryMode::Straight, foot: 0, action: None, facing: Vec3::X }));
    s.run(0.2);
    s.pad(Vec3::X, 1.0, true, false);
    assert!(s.run_until(3.0, |s| s.body().feet.x > 104.2), "never jogged up to the branch: {:?} {:?}", s.data().narrow.state, s.body().feet);
    s.pad(Vec3::Z, 1.0, true, false);
    assert!(s.run_until(0.2, |s| s.data().narrow.state == BeamState::HopStart), "no corner hop: {:?}", s.data().narrow.state);
    let h = s.data().narrow.hop.expect("hop target");
    assert!((h.point - Vec3::new(105.0, 4.0, 71.0)).length() < 0.05, "lands where the branch leaves the search box: {:?}", h.point);
    assert!(h.dir.dot(Vec3::Z) > 0.99);
    assert!(h.w[1] > 0.0 && h.w[3] > 0.0 && h.w[0] == 0.0, "right / front-right blend: {:?}", h.w);
    assert!(s.data().narrow.action.is_some_and(|a| a.id == BEAM_CORNER_HOP[0]));
    assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::HopEnd), "no landing: {:?}", s.data().narrow.state);
    let f = s.body().feet;
    assert!((f - Vec3::new(105.0, 4.0, 71.0)).length() < 0.05, "on the branch: {f:?}");
    s.run(0.1);
    assert!(s.body().feet.z > f.z + 0.02, "the landing action must carry root motion: {:?}", s.body().feet);
    assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::Walk), "no jog after: {:?}", s.data().narrow.state);
    assert!(s.run_until(2.0, |s| s.body().feet.z > 72.0), "jogs on along the branch: {:?}", s.body().feet);
    assert!((s.body().feet.x - 105.0).abs() < 0.01 && s.body().forward().dot(Vec3::Z) > 0.99);
}

#[test]
fn walking_a_bent_beam_carries_on_round_the_bend() {
    use crate::player::narrow::{BeamEntry, BeamEntryMode, BeamState};
    // the branch (x 105, z 70.3..76) bends 30° onto (105, 76) → (106.5, 78.6): the next segment ahead
    // (0xF753A0) takes over instead of the end stop
    let point = Vec3::new(105.0, 4.0, 74.0);
    let mut s = Sim::new(point, std::f32::consts::PI);
    let (p0, p1) = (Vec3::new(105.0, 4.0, 70.3), Vec3::new(105.0, 4.0, 76.0));
    force(&mut s, crate::player::TransitionSetup::ToBeam(BeamEntry { p0, p1, point, from: point, toward_p1: true, mode: BeamEntryMode::Straight, foot: 0, action: None, facing: Vec3::Z }));
    s.run(0.2);
    s.pad(Vec3::Z, 1.0, false, false);
    assert!(s.run_until(6.0, |s| s.data().narrow.p0 == Vec3::new(105.0, 4.0, 76.0)), "never switched segments: {:?} {:?} p0 {:?} p1 {:?} s {}", s.data().narrow.state, s.body().feet, s.data().narrow.p0, s.data().narrow.p1, s.data().narrow.s);
    s.pad((Vec3::new(1.5, 0.0, 2.6)).normalize(), 1.0, false, false);
    assert!(s.run_until(6.0, |s| s.body().feet.z > 77.5), "never walked on: {:?} {:?}", s.data().narrow.state, s.body().feet);
    let f = s.body().feet;
    let d = Vec3::new(1.5, 0.0, 2.6).normalize();
    let off = (f - Vec3::new(105.0, 4.0, 76.0)).cross(d).length();
    assert!(off < 0.01 && (f.y - 4.0).abs() < 0.01, "on the second segment's line: {f:?}");
    assert_eq!(s.loco().current, ActorContextId::NarrowObject);
    let _ = BeamState::Walk;
}

#[test]
fn beam_support_loss_enters_air_and_descends() {
    let mut s = on_free_beam(87.0);
    s.app.world_mut().resource_mut::<crate::guidance::GuidanceWorld>().edges
        .retain(|e| e.subtype != crate::guidance::GuidanceSubType::Beam);
    s.app.world_mut().resource_mut::<crate::collision::CollisionWorld>().boxes
        .retain(|b| !(b.min.x < 87.0 && b.max.x > 87.0 && b.min.z < 70.0 && b.max.z > 70.0 && (b.max.y-4.0).abs()<0.01));
    assert!(s.run_until(0.1, |s| s.loco().current == ActorContextId::InAir));
    let y = s.body().feet.y;
    s.run(0.3);
    assert!(s.body().feet.y < y && !s.body().grounded);
}

#[test]
fn beam_detection_does_not_snap_a_displaced_character_back() {
    let mut s = on_free_beam(87.0);
    s.app.world_mut().get_mut::<Body>(s.player).unwrap().feet.z += 3.0;
    assert!(s.run_until(0.1, |s| s.loco().current == ActorContextId::InAir));
    assert!(s.body().feet.z > 72.5);
}

#[test]
fn empty_hand_on_a_beam_reaches_climb_holds() {
    let mut s = on_free_beam(87.0);
    for y in [4.6, 5.8] {
        s.app.world_mut().resource_mut::<crate::guidance::GuidanceWorld>().edges.push(crate::guidance::GuidanceEdge {
            p0:Vec3::new(87.95,y,69.5), p1:Vec3::new(87.95,y,70.5), n0:Vec3::Y, n1:Vec3::NEG_X,
            subtype:crate::guidance::GuidanceSubType::LedgeGrab });
    }
    s.pad(Vec3::X,1.0,false,false); s.press_hand();
    assert!(s.run_until(0.2,|s|s.loco().current==ActorContextId::Climb), "no beam climb: {:?}",s.loco().current);
    assert!((s.data().climb.hand_l.y-5.8).abs()<0.01);
    assert!((s.data().climb.foot_l.y-4.6).abs()<0.01);
    s.run(1.0);
    assert!(s.body().feet.is_finite());
}

#[test]
fn walking_a_beam_into_an_obstacle_jumps_to_its_hand_target() {
    let mut s = on_free_beam(87.5);
    s.app.world_mut().resource_mut::<crate::collision::CollisionWorld>().boxes.push(crate::collision::Aabb3 {
        min:Vec3::new(88.0,0.0,69.5),max:Vec3::new(89.0,6.0,70.5) });
    s.app.world_mut().resource_mut::<crate::guidance::GuidanceWorld>().edges.push(crate::guidance::GuidanceEdge {
        p0:Vec3::new(88.0,6.0,69.5),p1:Vec3::new(88.0,6.0,70.5),n0:Vec3::Y,n1:Vec3::NEG_X,
        subtype:crate::guidance::GuidanceSubType::LedgeGrab });
    s.pad(Vec3::X,1.0,false,false);
    assert!(s.run_until(0.5,|s|s.loco().current==ActorContextId::InAir),"no obstacle hand jump");
    assert!(s.data().air.target.is_some_and(|t| t.hang.is_some()));
}

#[test]
fn beam_impulsion_keeps_side_and_back_input_but_resumes_forward() {
    use crate::player::narrow::BeamState;
    let mut s = on_free_beam(87.0);
    s.pad(Vec3::ZERO, 0.0, true, true);
    s.press_legs();
    assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::ImpulseWait));
    s.pad(Vec3::Z, 1.0, true, false); s.run(0.3);
    assert_eq!(s.data().narrow.state, BeamState::ImpulseWait);
    s.pad(Vec3::NEG_X, 1.0, true, false); s.run(0.3);
    assert_eq!(s.data().narrow.state, BeamState::ImpulseWait);
    s.pad(Vec3::X, 1.0, true, false);
    assert!(s.run_until(0.2, |s| s.data().narrow.state == BeamState::Walk));
}

#[test]
fn beam_step_off_travels_instead_of_teleporting_to_ground() {
    use crate::player::narrow::{BeamEntry, BeamEntryMode, BeamState};
    let point = Vec3::new(77.5, 4.0, 70.0);
    let mut s = Sim::new(point, crate::player::heading_of(Vec3::X));
    force(&mut s, crate::player::TransitionSetup::ToBeam(BeamEntry { p0: Vec3::new(72.0,4.0,70.0), p1: Vec3::new(78.0,4.0,70.0),
        point, from: point, toward_p1: true, mode: BeamEntryMode::Straight, foot: 0, action: None, facing: Vec3::X }));
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::StepOff));
    let start = s.body().feet;
    assert_eq!(s.loco().current, ActorContextId::NarrowObject);
    for _ in 0..120 {
        let before = s.body().feet;
        s.run(1.0/60.0);
        assert!(before.distance(s.body().feet) < 0.1, "discontinuous step-off");
        if s.loco().current == ActorContextId::Ground { break; }
    }
    assert_eq!(s.loco().current, ActorContextId::Ground);
    assert!((s.body().feet.x - start.x - 0.8).abs() < 0.06);
}

#[test]
fn beam_bend_heading_changes_over_multiple_frames() {
    use crate::player::narrow::{BeamEntry, BeamEntryMode};
    let point = Vec3::new(105.0,4.0,75.4);
    let mut s = Sim::new(point, crate::player::heading_of(Vec3::Z));
    force(&mut s, crate::player::TransitionSetup::ToBeam(BeamEntry { p0: Vec3::new(105.0,4.0,70.3), p1: Vec3::new(105.0,4.0,76.0),
        point, from: point, toward_p1: true, mode: BeamEntryMode::Straight, foot: 0, action: None, facing: Vec3::Z }));
    s.pad(Vec3::Z,1.0,false,false);
    let mut turned = false;
    for _ in 0..120 {
        let heading = s.body().heading;
        s.run(1.0/60.0);
        let delta = (s.body().heading-heading+std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)-std::f32::consts::PI;
        assert!(delta.abs() < 0.08, "abrupt bend: {delta}");
        turned |= delta.abs() > 0.001;
    }
    assert!(turned);
}

// ---------------------------------------------------------------- obstacle collision / lean, look-down (RE/02 §4.2)

/// Send event 42 the way the game's (player-unreachable) sender would: the obstacle just ahead of the character →
/// `HumanGround__ObstacleCollision_Enter` 0xD9CB90.
fn send_obstacle_collision(s: &mut Sim) -> bool {
    let (feet, heading, fwd) = (s.body().feet, s.body().heading, s.body().forward());
    let Some((contact, n, height)) = crate::player::collide::obstacle_ahead(feet, fwd, s.app.world().resource::<crate::collision::CollisionWorld>()) else { return false };
    let seq = s.ground().pose_seq;
    let Some(c) = crate::player::collide::enter(feet, heading, contact, n, height, seq) else { return false };
    let mut data = s.app.world_mut().get_mut::<HumanDataBundle>(s.player).unwrap();
    data.ground.pose_seq = c.seq;
    data.ground.collide = Some(c);
    data.ground.speed_param = 0.0;
    data.ground.blend.speed_param = 0.0;
    true
}

#[test]
fn running_into_walls_does_not_lean() {
    // event 42 is unreachable from player input (RE/02 §4.5): the 1.1 m wall, the 0.6 m box and the 2.4 m wall at
    // 30 deg only block the run
    for (from, dir) in [(Vec3::new(30.0, 0.0, -1.0), Vec3::NEG_Z), (Vec3::new(9.0, 0.0, 3.0), Vec3::NEG_Z), (Vec3::new(-3.0, 0.0, -4.0), Vec3::new(0.5, 0.0, -0.866))] {
        let mut s = Sim::new(from, 0.0);
        s.pad(dir, 1.0, true, false);
        for _ in 0..180 {
            s.run(1.0 / 60.0 + 1e-4);
            assert!(s.ground().collide.is_none(), "leaned at {:?}", s.body().feet);
        }
        assert_eq!(s.loco().current, ActorContextId::Ground);
    }
}

#[test]
fn obstacle_collision_leans_on_a_low_wall_and_releasing_stands_up() {
    use crate::player::collide::{CollideKind, CollidePhase, LEAN_TO_WAIT};
    // the 1.1 m wall at z -4 (x 28..32, 0.4 thick, face at z -3.8): standing against it facing -Z, event 42
    let mut s = Sim::new(Vec3::new(30.0, 0.0, -3.38), 0.0);
    s.run(0.2);
    assert!(send_obstacle_collision(&mut s), "no obstacle ahead: {:?}", s.body().feet);
    s.pad(Vec3::NEG_Z, 1.0, false, false);
    let c = s.ground().collide.unwrap();
    assert_eq!(c.kind, CollideKind::Hand);
    assert!((c.h - 0.5).abs() < 0.02, "1.1 m → halfway between the 70 and 150 cm clips: {}", c.h);
    // keeps leaning while pushing into the wall
    assert!(s.run_until(3.0, |s| s.ground().collide.is_some_and(|c| c.phase == CollidePhase::Wait)), "no lean wait");
    s.run(1.0);
    assert!(s.ground().collide.is_some(), "still leaning");
    let f = s.body().feet;
    assert!((f.z - (-3.8 + 0.4)).abs() < 0.02, "root 0.4 m out of the face: {f:?}");
    assert!(s.body().forward().dot(Vec3::NEG_Z) > 0.99, "facing the wall");
    // release: lean → wait, then standing
    s.pad(Vec3::ZERO, 0.0, false, false);
    s.run(2.0 / 60.0);
    assert!(s.ground().collide.is_none());
    assert!(s.ground().oneshot.is_some_and(|o| o.blend.id == LEAN_TO_WAIT[0]), "lean_*_wait_tr_l_wait");
}

#[test]
fn obstacle_collision_then_pushing_sideways_walks_off_through_the_exit() {
    use crate::player::collide::{CollidePhase, LEAN_EXIT, LEAN_EXIT_TR};
    let mut s = Sim::new(Vec3::new(30.0, 0.0, -3.38), 0.0);
    s.run(0.2);
    assert!(send_obstacle_collision(&mut s));
    s.pad(Vec3::NEG_Z, 1.0, false, false);
    assert!(s.run_until(4.0, |s| s.ground().collide.is_some_and(|c| c.phase == CollidePhase::Wait)));
    // stick to the character's left (facing -Z, left = -X)
    s.pad(Vec3::NEG_X, 1.0, false, false);
    s.run(2.0 / 60.0);
    let os = s.ground().oneshot.expect("exit action");
    assert_eq!(os.blend.id, LEAN_EXIT[0], "the left exit");
    let w = os.blend.weights();
    assert!(w[2] + w[3] > 0.99 && w[4..].iter().all(|x| *x == 0.0), "side walk clips: {w:?}");
    assert!(s.run_until(3.0, |s| s.ground().oneshot.is_some_and(|o| o.blend.id == LEAN_EXIT_TR[0])), "exit transition");
    assert!(s.run_until(3.0, |s| s.ground().oneshot.is_none()));
    s.run(1.0);
    assert!(s.body().feet.x < 29.0, "walked off to the left: {:?}", s.body().feet);
}

#[test]
fn obstacle_collision_with_a_knee_high_obstacle_is_the_foot_collide() {
    use crate::player::collide::CollideKind;
    // the 0.6 m box at (9, 0), 1.5 m square (face at z 0.75): too high to step onto (0.35 m)
    let mut s = Sim::new(Vec3::new(9.0, 0.0, 1.17), 0.0);
    s.run(0.2);
    assert!(send_obstacle_collision(&mut s), "no obstacle ahead: {:?}", s.body().feet);
    let c = s.ground().collide.unwrap();
    assert_eq!(c.kind, CollideKind::Foot);
    assert!((c.h - 0.5).abs() < 0.02, "0.6 m → halfway between the 50 and 70 cm clips: {}", c.h);
}

#[test]
fn standing_at_a_roof_edge_looks_down_toward_it() {
    // roof A (x -3..3, top 3), standing 0.3 m from its +X edge facing -Z: the edge is on the right
    let mut s = Sim::new(Vec3::new(2.7, 3.0, 12.0), 0.0);
    s.run(0.3);
    let ld = s.ground().look_down.expect("look-down");
    let w = ld.action.weights();
    assert!(w[2] > 0.95 && w[1] == 0.0, "right look-down: {w:?}");
    s.pad(Vec3::NEG_X, 1.0, false, false);
    s.run(0.1);
    assert!(s.ground().look_down.is_none(), "the stick ends it");
}

// ---------------------------------------------------------------- pass-over (RE/04 §4.1.13)

#[test]
fn passover_weights_follow_the_game() {
    use crate::player::jump_blend::{passover_flight_weights, passover_takeoff_weights};
    let mut f = [0.0f32; 5];
    passover_flight_weights(&mut f, 0.4, 0.5, 0);
    assert_eq!(f, [0.3, 0.2, 0.0, 0.3, 0.2]);
    let mut t = [0.0f32; 40];
    passover_takeoff_weights(&mut t, &f, 0);
    assert_eq!((t[0], t[1], t[6], t[7]), (0.3, 0.2, 0.3, 0.2));
    let mut f = [0.0f32; 5];
    passover_flight_weights(&mut f, 0.5, 0.2, 1);
    assert!((f[1] - 0.4).abs() < 1e-6 && (f[2] - 0.4).abs() < 1e-6 && (f[4] - 0.2).abs() < 1e-6);
}

#[test]
fn running_jump_at_a_railing_vaults_it() {
    use crate::player::passover::{PassOverPhase, FLIGHT_PASSOVER, VAULT};
    // the 1 m railing at z 4 (x 38..42, z 3.85..4.15): run along -Z, jump
    let mut s = Sim::new(Vec3::new(40.0, 0.0, 8.5), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(0.4);
    s.press_legs();
    assert!(s.run_until(0.5, |s| s.loco().current == ActorContextId::InAir), "no jump");
    let t = s.data().air.target.expect("a target");
    assert_eq!(t.type_flags, 2, "pass-over target");
    assert!(FLIGHT_PASSOVER.contains(&s.data().air.flight.unwrap().id));
    assert!(s.run_until(2.0, |s| s.data().ledge.pass_over.is_some_and(|p| p.phase == PassOverPhase::Vault)), "no vault: {:?} {:?}", s.loco().current, s.body().feet);
    let p = s.data().ledge.pass_over.unwrap();
    assert!(VAULT.contains(&p.action.id));
    assert!((p.w - 0.3).abs() < 0.02, "0.3 m thick → w 0.3: {}", p.w);
    assert!((s.body().feet.y - 1.0).abs() < 0.01, "on the top: {:?}", s.body().feet);
    // over and down on the far side
    assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::Ground && s.body().feet.y < 0.01), "never landed beyond: {:?} {:?}", s.loco().current, s.body().feet);
    assert!(s.body().feet.z < 3.85, "on the far side: {:?}", s.body().feet);
}

// ---------------------------------------------------------------- swing bars (RE/03 §7.10)

#[test]
fn swing_from_bar_to_bar() {
    use crate::player::swing::{SwingPhase, SWING_CYCLE, SWING_LANDING, TAKEOFF_SWING};
    let mut s = Sim::new(Vec3::new(60.0, 1.2, 85.0), std::f32::consts::PI);
    s.pad(Vec3::Z, 1.0, true, false);
    s.run(0.35);
    s.press_legs();
    assert!(s.run_until(0.5, |s| s.loco().current == ActorContextId::InAir), "no jump");
    assert_eq!(s.data().air.target.unwrap().type_flags, crate::player::targets::TARGET_LEDGE_FREE);
    assert!(s.run_until(3.0, |s| s.data().ledge.swing.is_some()), "never swung: {:?} {:?}", s.loco().current, s.body().feet);
    assert_eq!(s.data().ledge.swing.unwrap().action.id, SWING_LANDING);
    // a bar ahead → the swing cycle
    assert!(s.run_until(2.0, |s| s.data().ledge.swing.is_some_and(|w| matches!(w.phase, SwingPhase::Cycle(_)))), "no cycle: {:?}", s.data().ledge.swing.map(|w| w.phase));
    assert_eq!(s.data().ledge.swing.unwrap().action.id, SWING_CYCLE);
    for z in [93.5f32, 97.0] {
        s.press_legs();
        assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir), "no swing jump toward {z}");
        assert_eq!(s.data().air.takeoff.unwrap().id, TAKEOFF_SWING);
        assert!(s.run_until(3.0, |s| s.data().ledge.swing.is_some() && s.loco().current == ActorContextId::Ledge), "never reached bar {z}: {:?} {:?}", s.loco().current, s.body().feet);
        let mid = (s.data().ledge.hand_l + s.data().ledge.hand_r) * 0.5;
        assert!((mid.z - (z - 0.1)).abs() < 0.05, "hands on bar {z}: {mid:?}");
        assert!(s.run_until(2.0, |s| s.data().ledge.swing.is_some_and(|w| matches!(w.phase, SwingPhase::Cycle(_)))), "no cycle on {z}");
    }
    // and onto the far platform
    s.press_legs();
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir));
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground), "never landed: {:?} {:?}", s.loco().current, s.body().feet);
    assert!(s.body().feet.z > 100.0 && (s.body().feet.y - 1.2).abs() < 0.05, "on the far platform: {:?}", s.body().feet);
}

#[test]
fn landing_on_a_bar_with_nothing_ahead_settles_into_the_hang() {
    use crate::player::swing::{SwingPhase, IMPACT_ELBOW};
    // from the end platform back toward bar 3 (z 97): nothing to jump to beyond bar 2 ... use bar 1 from the start
    // platform facing -Z instead: the start platform side has no bar behind
    let mut s = Sim::new(Vec3::new(60.0, 1.2, 100.5), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(0.2);
    s.press_legs();
    assert!(s.run_until(3.0, |s| s.data().ledge.swing.is_some()), "never swung");
    s.pad(Vec3::ZERO, 0.0, true, false);
    assert!(s.run_until(2.0, |s| s.data().ledge.swing.is_some_and(|w| !matches!(w.phase, SwingPhase::Landing))));
    let w = s.data().ledge.swing.unwrap();
    // bar 2 lies ahead (3.5 m): a target, so it swings; released stick on a down phase -> stop -> hang
    if matches!(w.phase, SwingPhase::Settle(id, _) if id == IMPACT_ELBOW) {
        assert!(s.run_until(3.0, |s| s.data().ledge.swing.is_none()));
    } else {
        assert!(s.run_until(3.0, |s| s.data().ledge.swing.is_none()), "the released stick stops the swing");
    }
    assert_eq!(s.loco().current, ActorContextId::Ledge, "hanging");
}

// ---------------------------------------------------------------- ladders (RE/05 §4.1)

#[test]
fn native_masyaf_roof_stands_and_walks_on_source_collision() {
    let Some((c, g, spawn)) = crate::native_map::simulation_fixture() else { return; };
    let mut s = Sim::new_raw(spawn, 0.0);
    s.app.insert_resource(c).insert_resource(g);
    s.app.update();
    s.run(1.0);
    assert_eq!(s.loco().current, ActorContextId::Ground);
    assert!((s.body().feet.y - spawn.y).abs() < 0.05);
    s.pad(Vec3::X, 0.5, false, false);
    s.run(0.5);
    assert!(s.body().feet.x > spawn.x + 0.1, "{:?}", s.body().feet);
    assert_eq!(s.loco().current, ActorContextId::Ground);
}

#[test]
fn native_freerun_culling_preserves_queries_targets_and_motion() {
    let Some((mut c,g,spawn)) = crate::native_map::village_simulation_fixture() else { return; };
    let rotation = Quat::from_rotation_y(0.63);
    let axes = [rotation * Vec3::X,Vec3::Y,rotation * Vec3::Z];
    for offset in [Vec3::ZERO,Vec3::new(-10.0,0.0,5.0),Vec3::new(10.0,4.0,-5.0),Vec3::new(-20.0,8.0,10.0)] {
        let p = spawn + offset;
        let mut baseline = None;
        for enabled in [false,true] {
            c.native_query_culling = enabled;
            let queries = (c.floor_height_below(p+Vec3::Y,10.0),c.point_inside(p),c.capsule_fits(p),
                c.capsule_cast_free(p,1.8,0.35,Vec3::new(1.0,0.4,-1.0)),
                c.obb_free(p+Vec3::Y,axes,Vec3::new(0.3,0.5,0.2)),
                c.obb_lowest(p+Vec3::Y,axes,Vec3::new(0.3,0.5,0.2),1),
                c.sphere_free_distance(p+Vec3::Y,Vec3::NEG_X,0.1,2.0),
                c.camera_distance(p+Vec3::Y,Vec3::Z,6.0));
            if let Some(expected) = baseline { assert_eq!(queries,expected,"query at {p:?}"); }
            else { baseline = Some(queries); }
        }
        for direction in [Vec3::NEG_X,Vec3::Z] {
            c.native_query_culling = false;
            let baseline = crate::player::targets::find_jump_target(p,direction,&g,&c).map(|t|format!("{t:?}"));
            c.native_query_culling = true;
            let result = crate::player::targets::find_jump_target(p,direction,&g,&c).map(|t|format!("{t:?}"));
            assert_eq!(result,baseline,"jump target at {p:?}, {direction:?}");
        }
    }
    // The same walk → fresh Legs press → freerun sequence that reproduces the hitch.
    let mut baseline = Vec::new();
    for enabled in [false,true] {
        let Some((mut c,g,spawn)) = crate::native_map::village_simulation_fixture() else { return; };
        c.native_query_culling = enabled;
        let mut s = Sim::new_raw(spawn,crate::player::heading_of(Vec3::NEG_X));
        s.app.insert_resource(c).insert_resource(g);s.app.update();
        for frame in 0..240 {
            s.pad(Vec3::NEG_X,1.0,frame>=120,frame>=120);
            if frame==120 { s.press_legs(); }
            s.run(1.0/60.0);
            let state = (s.body().feet,s.loco().current);
            assert!(state.0.is_finite());
            if enabled { assert_eq!(state,baseline[frame],"freerun frame {frame}"); }
            else { baseline.push(state); }
        }
        assert!(s.body().feet.x<spawn.x-10.0,"freerun did not advance: {:?}",s.body().feet);
    }
}

#[test]
#[ignore = "manual native-map CPU profile; requires the local game install"]
fn profile_native_freerun_transition() {
    let Some((c,g,spawn)) = crate::native_map::village_simulation_fixture() else { return; };
    let mut s = Sim::new_raw(spawn,crate::player::heading_of(Vec3::NEG_X));
    s.app.insert_resource(c).insert_resource(g);s.app.update();
    for (name, high, legs) in [("walk",false,false),("freerun",true,true)] {
        s.pad(Vec3::NEG_X,1.0,high,legs);
        if legs { s.press_legs(); }
        let mut times = Vec::new();
        for _ in 0..120 {
            let start = std::time::Instant::now();s.run(1.0/60.0);
            let elapsed = start.elapsed().as_secs_f64()*1000.0;
            if elapsed > 10.0 { eprintln!("slow {name} frame {}: {elapsed:.3} ms", times.len()); }
            times.push(elapsed);
        }
        times.sort_by(f64::total_cmp);
        eprintln!("{name}: CPU frame median {:.3} ms, p95 {:.3} ms, max {:.3} ms; {:?} {:?}",times[60],times[114],times[119],s.loco().current,s.body().feet);
    }
    let c = s.app.world().resource::<crate::collision::CollisionWorld>();
    let p = spawn + Vec3::Y;
    let g = s.app.world().resource::<crate::guidance::GuidanceWorld>();
    let start = std::time::Instant::now();
    std::hint::black_box(crate::player::targets::find_jump_target(spawn,Vec3::NEG_X,g,c));
    eprintln!("jump target: {:.3} ms",start.elapsed().as_secs_f64()*1000.0);
    let start = std::time::Instant::now();
    std::hint::black_box(crate::player::walling::wall_ahead(spawn,Vec3::NEG_X,c));
    eprintln!("wall ahead: {:.3} ms",start.elapsed().as_secs_f64()*1000.0);
    for name in ["capsule_fits","obb_free","obb_lowest","capsule_cast"] {
        let start = std::time::Instant::now();
        for _ in 0..100 { match name {
            "capsule_fits" => { std::hint::black_box(c.capsule_fits(p)); },
            "obb_free" => { std::hint::black_box(c.obb_free(p,[Vec3::X,Vec3::Y,Vec3::Z],Vec3::splat(0.3))); },
            "obb_lowest" => { std::hint::black_box(c.obb_lowest(p,[Vec3::X,Vec3::Y,Vec3::Z],Vec3::splat(0.3),1)); },
            _ => { std::hint::black_box(c.capsule_cast_free(p,1.8,0.4,Vec3::NEG_X)); },
        } }
        eprintln!("{name}: {:.3} ms/query",start.elapsed().as_secs_f64()*10.0);
    }
}

#[test]
fn native_masyaf_village_walks_along_a_source_street() {
    let Some((c,g,spawn)) = crate::native_map::village_simulation_fixture() else { return; };
    let mut s = Sim::new_raw(spawn,crate::player::heading_of(Vec3::NEG_X));
    s.app.insert_resource(c).insert_resource(g);
    s.app.update();
    s.run(1.0);
    assert_eq!(s.loco().current,ActorContextId::Ground);
    s.pad(Vec3::NEG_X,0.6,false,false);
    s.run(4.0);
    assert!(s.body().feet.x<spawn.x-1.5,"{:?}",s.body().feet);
    assert_eq!(s.loco().current,ActorContextId::Ground);
    assert!((s.body().feet.y-spawn.y).abs()<1.0,"street support: {:?}",s.body().feet);
}

#[test]
fn native_masyaf_village_ladder_connects_street_to_roof() {
    let Some((c,g,_)) = crate::native_map::village_simulation_fixture() else { return; };
    let target = Vec3::new(8.22,34.64,41.62);
    let edge = g.edges.iter().filter(|e|e.subtype==crate::guidance::GuidanceSubType::Ladder)
        .min_by(|a,b|a.p0.min(a.p1).distance_squared(target).total_cmp(&b.p0.min(b.p1).distance_squared(target))).unwrap();
    let (base,top) = if edge.p0.y<edge.p1.y {(edge.p0,edge.p1)} else {(edge.p1,edge.p0)};
    let n = Vec3::new(edge.n1.x,0.0,edge.n1.z).normalize();
    let mut feet = base+n*0.8;
    feet.y = c.floor_height_below(feet+Vec3::Y*0.2,0.6).expect("native ground at ladder base");
    let expected_roof = c.floor_height_below(top-n*0.5+Vec3::Y*0.2,2.0).expect("native roof above the ladder");
    let mut s = Sim::new_raw(feet,crate::player::heading_of(-n));
    s.app.insert_resource(c).insert_resource(g);
    s.app.update();s.pad(-n,1.0,false,false);
    assert!(s.run_until(3.0,|s|s.loco().current==ActorContextId::Ladder),"no native ladder entry");
    assert!(s.run_until(30.0,|s|s.loco().current==ActorContextId::Ground),"no exit: {:?} {:?}",s.data().ladder.phase,s.body().feet);
    s.pad(-n,0.0,false,false);s.run(2.0);
    assert_eq!(s.loco().current,ActorContextId::Ground);
    assert!((s.body().feet.y-expected_roof).abs()<0.1,"source roof {expected_roof}, feet {:?}",s.body().feet);
}

#[test]
fn native_masyaf_authored_ladder_mounts_and_climbs() {
    let Some((c, g, _)) = crate::native_map::simulation_fixture() else { return; };
    let edge = g.edges.iter().find(|e| e.subtype == crate::guidance::GuidanceSubType::Ladder).unwrap();
    let base = if edge.p0.y < edge.p1.y { edge.p0 } else { edge.p1 };
    let n = Vec3::new(edge.n1.x, 0.0, edge.n1.z).normalize();
    let mut feet = base + n * 0.8;
    feet.y = c.floor_height_below(feet + Vec3::Y * 0.2, 0.6).expect("source ladder bottom must have roof support");
    let mut s = Sim::new_raw(feet, crate::player::heading_of(-n));
    s.app.insert_resource(c).insert_resource(g);
    s.app.update();
    s.pad(-n, 1.0, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ladder), "never mounted source ladder: {:?} {:?}", s.loco().current, s.body().feet);
    assert!(s.run_until(8.0, |s| s.data().ladder.height > 1.0), "did not climb source ladder");
    assert!(s.body().feet.is_finite());
}

#[test]
fn native_masyaf_beam_guidance_supports_narrow_movement() {
    use crate::player::narrow::{BeamEntry, BeamEntryMode};
    let Some((collision, guidance, _)) = crate::native_map::village_simulation_fixture() else { return; };
    let count = guidance.edges.iter().filter(|e| e.subtype == crate::guidance::GuidanceSubType::Beam).count();
    let Some(edge) = guidance.edges.iter().find(|e| e.subtype == crate::guidance::GuidanceSubType::Beam
        && e.p0.distance(e.p1) > 2.0 && collision.capsule_fits(e.p0.lerp(e.p1,0.5))) else {
        eprintln!("native beam exercise skipped: {count} beam edges in the village slice, none with a clear standing midpoint");
        return;
    };
    let (p0,p1) = (edge.p0,edge.p1);
    let point = p0.lerp(p1,0.5);
    let facing = (p1-p0).with_y(0.0).normalize();
    let mut s = Sim::new_raw(point,crate::player::heading_of(facing));
    s.app.insert_resource(collision).insert_resource(guidance);
    s.app.update();
    force(&mut s, crate::player::TransitionSetup::ToBeam(BeamEntry { p0,p1,point,from:point,toward_p1:true,
        mode:BeamEntryMode::Straight,foot:0,action:None,facing }));
    s.pad(facing,1.0,false,false); s.run(0.4);
    assert_eq!(s.loco().current,ActorContextId::NarrowObject);
    assert!(s.body().feet.is_finite() && (s.body().feet-point).dot(facing)>0.01);
}

#[test]
fn walk_to_a_ladder_climb_it_and_step_onto_the_top() {
    use crate::player::ladder::{LadderPhase, CLIMB_UP, ENTER_GROUND, EXIT_TOP};
    // the ladder at x 50 on the 5 m wall face z 63.5 (normal -Z); walk at it along +Z
    let mut s = Sim::new(Vec3::new(50.0, 0.0, 60.5), std::f32::consts::PI);
    s.pad(Vec3::Z, 1.0, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ladder), "never mounted: {:?} {:?}", s.loco().current, s.body().feet);
    assert_eq!(s.data().ladder.action.unwrap().id, ENTER_GROUND[0][s.data().ladder.foot]);
    assert!(s.run_until(2.0, |s| s.data().ladder.phase == Some(LadderPhase::ClimbUp)), "no climb: {:?}", s.data().ladder.phase);
    assert_eq!(s.data().ladder.action.unwrap().id, CLIMB_UP[0], "low-profile climb");
    let f = s.body().feet;
    assert!((f.z - 63.0).abs() < 0.02 && (f.x - 50.0).abs() < 0.02, "0.5 m out from the ladder: {f:?}");
    // keeps climbing in 0.5 m steps, then exits to the top
    assert!(s.run_until(10.0, |s| s.data().ladder.phase == Some(LadderPhase::ExitTop(0))), "no exit at the top: {:?} {:?}", s.data().ladder.phase, s.data().ladder.height);
    assert!(EXIT_TOP[0].contains(&s.data().ladder.action.unwrap().id));
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ground), "never stood on top");
    let f = s.body().feet;
    assert!((f.y - 5.0).abs() < 0.05 && f.z > 63.6, "on the wall top: {f:?}");
}

#[test]
fn enter_a_ladder_from_the_top_and_climb_down() {
    use crate::player::ladder::{LadderPhase, ENTER_TOP, EXIT_GROUND};
    // on the wall top (z 63.5..64.5, top 5) at the ladder, facing out (-Z)
    let mut s = Sim::new(Vec3::new(50.0, 5.0, 63.9), 0.0);
    s.run(0.1);
    s.press_legs();
    assert!(s.run_until(0.5, |s| s.loco().current == ActorContextId::Ladder), "no entry from the top: {:?}", s.loco().current);
    assert!(ENTER_TOP[0].contains(&s.data().ladder.action.unwrap().id));
    assert!(s.run_until(3.0, |s| s.data().ladder.phase == Some(LadderPhase::Wait)), "never settled: {:?}", s.data().ladder.phase);
    assert!(s.body().forward().dot(Vec3::Z) > 0.99, "facing the ladder");
    // down: the stick away from the ladder
    s.pad(Vec3::NEG_Z, 1.0, false, false);
    assert!(s.run_until(12.0, |s| s.data().ladder.phase == Some(LadderPhase::ExitGround)), "no exit at the bottom: {:?} {:?}", s.data().ladder.phase, s.data().ladder.height);
    assert!(EXIT_GROUND[0].contains(&s.data().ladder.action.unwrap().id));
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ground));
    assert!(s.body().feet.y.abs() < 0.05, "on the floor: {:?}", s.body().feet);
}

/// Walk to the ladder at x 50 (front z 63.5), climb to 2 m and wait there.
fn on_the_ladder() -> Sim {
    use crate::player::ladder::LadderPhase;
    let mut s = Sim::new(Vec3::new(50.0, 0.0, 60.5), std::f32::consts::PI);
    s.pad(Vec3::Z, 1.0, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ladder));
    assert!(s.run_until(4.0, |s| s.data().ladder.height >= 2.0));
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(1.0, |s| s.data().ladder.phase == Some(LadderPhase::Wait)));
    s
}

#[test]
fn letting_go_of_a_ladder_falls() {
    use crate::player::ladder::RELEASE;
    // QuickDrop (event 0): the empty-hand button (0xEEB570 → IHumanLadder slots 6 / 7)
    let mut s = on_the_ladder();
    s.press_hand();
    assert!(s.run_until(0.5, |s| s.loco().current == ActorContextId::InAir), "no release");
    assert!(s.data().air.fall_action.is_some_and(|a| RELEASE[0].contains(&a.id)));
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground));
}

#[test]
fn legs_alone_does_not_drop_off_a_ladder() {
    // without high profile Legs does nothing on a ladder (the jump needs high profile; the drop is the empty hand)
    let mut s = on_the_ladder();
    s.press_legs();
    s.run(0.5);
    assert_eq!(s.loco().current, ActorContextId::Ladder);
}

#[test]
fn high_profile_legs_jumps_off_a_ladder_straight_back() {
    use crate::player::ladder::LadderPhase;
    // event 1 (slots 4 / 5): high profile + Legs, no stick → straight back from the ladder (+n = -Z)
    let mut s = on_the_ladder();
    s.pad(Vec3::ZERO, 0.0, true, true);
    s.press_legs();
    assert!(s.run_until(0.2, |s| s.data().ladder.phase == Some(LadderPhase::Jump)), "no jump");
    assert!(s.data().ladder.jump_dir.distance(Vec3::NEG_Z) < 1e-3, "straight back: {:?}", s.data().ladder.jump_dir);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::InAir), "never left");
}

#[test]
fn a_jump_off_a_ladder_into_it_is_refused_and_a_side_push_is_turned() {
    use crate::player::ladder::LadderPhase;
    // more than 135° from straight back (the stick into the ladder): no jump
    let mut s = on_the_ladder();
    s.pad(Vec3::Z, 1.0, true, true);
    s.press_legs();
    s.run(0.2);
    assert_ne!(s.data().ladder.phase, Some(LadderPhase::Jump), "jumped into the ladder");
    // 90°–135° (sideways and a little in): turned to 89° from straight back
    let mut s = on_the_ladder();
    s.pad((Vec3::X + Vec3::Z * 0.4).normalize(), 1.0, true, true);
    s.press_legs();
    assert!(s.run_until(0.2, |s| s.data().ladder.phase == Some(LadderPhase::Jump)), "no jump");
    let d = s.data().ladder.jump_dir;
    assert!((d.angle_between(Vec3::NEG_Z).to_degrees() - 89.0).abs() < 0.1 && d.x > 0.0, "turned to 89°: {d:?}");
}

#[test]
fn falling_past_a_ladder_with_legs_held_catches_it() {
    use crate::player::ladder::{LadderPhase, CATCH_AIR, WAIT};
    // FindLadderCatch 0xE04100 (grab held): the catch height rounded down to 0.5 m, the catch action, then the low wait
    let mut s = Sim::new(Vec3::new(50.0, 3.2, 62.9), FACE_PZ);
    s.pad(Vec3::ZERO, 0.0, false, true);
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Ladder), "no catch: {:?} {:?}", s.loco().current, s.body().feet);
    let l = &s.data().ladder;
    assert_eq!(l.phase, Some(LadderPhase::Entry));
    assert_eq!(l.action.map(|a| a.id), Some(CATCH_AIR));
    assert!((l.height * 2.0).fract().abs() < 1e-4 && l.height >= 0.5 && l.height < 5.0 - 1.95, "on a rung: {}", l.height);
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().ladder.phase == Some(LadderPhase::Wait)), "never settled");
    assert_eq!(s.data().ladder.action.map(|a| a.id), Some(WAIT[0][0]), "the low wait, foot l");
    let f = s.body().feet;
    assert!((f.x - 50.0).abs() < 0.05 && (f.z - 63.0).abs() < 0.05, "0.5 m out from the ladder: {f:?}");
}

#[test]
fn falling_past_a_ladder_without_legs_does_not_catch_it() {
    let mut s = Sim::new(Vec3::new(50.0, 3.2, 62.9), FACE_PZ);
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ground), "never landed");
    assert!(s.saw_air);
}

#[test]
fn a_jump_at_a_ladder_lands_on_it() {
    use crate::player::ladder::{LadderPhase, ARRIVE_TARGET};
    use crate::player::targets::TARGET_LADDER;
    // a ladder jump target (type 0x1000): the arrival (0xE07D00) plays `ARRIVE_TARGET`, Ladder EntryType 1
    let mut s = Sim::new(Vec3::new(50.0, 0.0, 59.5), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, false);
    s.press_legs();
    assert!(s.run_until(0.5, |s| s.loco().current == ActorContextId::InAir), "no jump: {:?}", s.loco().current);
    assert_eq!(s.data().air.target.map(|t| t.type_flags), Some(TARGET_LADDER));
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ladder), "never arrived: {:?} {:?}", s.loco().current, s.body().feet);
    assert_eq!(s.data().ladder.action.map(|a| a.id), Some(ARRIVE_TARGET));
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().ladder.phase == Some(LadderPhase::Wait)), "never settled");
    assert!((s.data().ladder.height - 1.0).abs() < 1e-3, "1 m up the ladder: {}", s.data().ladder.height);
}

#[test]
fn a_wall_run_beside_a_ladder_steps_onto_it() {
    use crate::player::ladder::{LadderPhase, CLIMB_UP};
    use crate::player::walling::WallingSubState;
    // the interpreter's walling state (0xEE05E0): a ladder within 0.6 m at the end of a Vertical step, the stick toward
    // it → Ladder EntryType 3 (the high climb up's right-foot item)
    let mut s = Sim::new(Vec3::new(50.2, 0.0, 62.0), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    s.press_legs();
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Walling), "no wall run: {:?}", s.loco().current);
    assert!(s.run_until(3.0, |s| s.loco().current != ActorContextId::Walling), "never left the wall");
    assert_eq!(s.loco().current, ActorContextId::Ladder, "the ladder (last walling state {:?})", s.data().walling.sub_state);
    let l = &s.data().ladder;
    assert_eq!(l.phase, Some(LadderPhase::Entry));
    assert_eq!(l.action.map(|a| (a.id, a.item)), Some((CLIMB_UP[1], 1)));
    assert!(l.high);
    let _ = WallingSubState::Vertical;
    s.pad(Vec3::ZERO, 0.0, true, false);
    assert!(s.run_until(2.0, |s| s.data().ladder.phase == Some(LadderPhase::Wait)), "never settled");
}

#[test]
fn a_wall_run_beside_a_ladder_without_the_stick_stays_on_the_wall() {
    // no stick: the ladder search (sub_EDFBC0(1)) needs the stick
    let mut s = Sim::new(Vec3::new(50.2, 0.0, 62.0), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    s.press_legs();
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Walling), "no wall run");
    s.pad(Vec3::ZERO, 0.0, true, true);
    assert!(s.run_until(3.0, |s| s.loco().current != ActorContextId::Walling), "never left the wall");
    assert_ne!(s.loco().current, ActorContextId::Ladder);
}

#[test]
fn the_climb_steps_onto_a_ladder_at_the_side() {
    use crate::player::ladder::{LadderPhase, FROM_CLIMB_SIDE, WAIT};
    // TryLadder 0xDF10C0 (ChooseMove's first test): the ladder at x -129.4 is 0.65 m left of wall X's last holds
    let mut s = climb_then_left(Vec3::new(-131.0, 0.0, -11.4), |s| s.loco().current == ActorContextId::Ladder);
    assert_eq!(s.data().climb.last_action, "onto a ladder");
    let l = &s.data().ladder;
    assert_eq!(l.phase, Some(LadderPhase::Entry));
    assert_eq!(l.action.map(|a| a.id), Some(FROM_CLIMB_SIDE[0]), "dir 4 (left)");
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().ladder.phase == Some(LadderPhase::Wait)), "never settled");
    assert_eq!(s.data().ladder.action.map(|a| a.id), Some(WAIT[0][1]), "the low wait, foot r after a left move");
    let f = s.body().feet;
    assert!((f.x + 129.4).abs() < 0.05 && (f.z + 11.0).abs() < 0.05, "0.5 m out from the ladder: {f:?}");
}

#[test]
fn a_ladder_with_a_blocked_top_stops_the_climb() {
    use crate::player::ladder::LadderPhase;
    // sub_E240B0: a slab over the top of the ladder at x -129.4 → ReachedTop stops the climb (no exit to the top)
    let mut s = Sim::new(Vec3::new(-129.4, 0.0, -12.5), FACE_PZ);
    s.pad(Vec3::Z, 1.0, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ladder), "never mounted");
    assert!(s.run_until(30.0, |s| s.data().ladder.height + 1.63 >= 7.0), "never reached the top: {}", s.data().ladder.height);
    s.run(3.0);
    assert_eq!(s.loco().current, ActorContextId::Ladder, "climbed off a blocked top");
    assert_eq!(s.data().ladder.phase, Some(LadderPhase::Wait));
    assert!(s.data().ladder.height <= 7.0 - 1.0, "stopped below the top: {}", s.data().ladder.height);
}

#[test]
fn a_low_profile_climb_with_a_light_push_is_slower() {
    use crate::player::ladder::LadderPhase;
    // 0xE24540: low profile, input value 0.5 (the stick in the walk band) → the climb plays at half rate
    let time_up = |stick: f32| {
        let mut s = Sim::new(Vec3::new(50.0, 0.0, 60.5), std::f32::consts::PI);
        s.pad(Vec3::Z, 1.0, false, false);
        assert!(s.run_until(3.0, |s| s.data().ladder.phase == Some(LadderPhase::Wait) || s.data().ladder.phase == Some(LadderPhase::ClimbUp)));
        s.pad(Vec3::Z, stick, false, false);
        let h0 = s.data().ladder.height;
        s.run(2.0);
        s.data().ladder.height - h0
    };
    let (slow, fast) = (time_up(0.45), time_up(1.0));
    assert!(slow > 0.0 && slow < fast * 0.75, "slow {slow} fast {fast}");
}

// ---------------------------------------------------------------- per-frame traces (diagnostics, run by hand)

fn trace_frames(s: &mut Sim, label: &str, seconds: f32, mut each: impl FnMut(&mut Sim, usize)) {
    println!("== {label}");
    let mut prev = s.body().feet;
    let mut prev_h = 0.0f32;
    for i in 0..(seconds * 60.0) as usize {
        each(s, i);
        s.run(1.0 / 60.0 + 1e-4);
        let f = s.body().feet;
        let v = (f - prev) * 60.0;
        let h = Vec2::new(v.x, v.z).length();
        let mode = match s.data().air.mode {
            air::AirMode::Jump { t, .. } => format!("jump t={t:.2}"),
            air::AirMode::Fall { .. } => "fall".into(),
            air::AirMode::Idle => "idle".into(),
        };
        let os = s.ground().oneshot.map(|o| format!("os {:08x} {:.2}/{:.2}", o.blend.id, o.t, o.duration)).unwrap_or_default();
        let flag = if (h - prev_h).abs() > 1.5 { " <<< dh" } else { "" };
        println!(
            "{i:3} {:?} {mode:12} feet ({:6.2} {:5.2} {:6.2}) h {h:5.2} vy {:6.2} hd {:5.2} sp {:.2} {os}{flag}",
            s.loco().current, f.x, f.y, f.z, v.y, s.body().heading, s.ground().speed_param
        );
        prev = f;
        prev_h = h;
    }
}

#[test]
#[ignore]
fn trace_run_off_a_drop() {
    let mut s = Sim::new(Vec3::new(-12.5, 6.0, 4.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    trace_frames(&mut s, "sprint off the 6 m block", 2.5, |_, _| {});
}

#[test]
#[ignore]
fn trace_drop_grab() {
    // the dropgrab scene: walk off the 6 m block's +X edge at 65 deg, hold grab and the stick to -Z once in the air
    let mut s = Sim::new(Vec3::new(-11.0, 6.0, 2.0), -std::f32::consts::FRAC_PI_2);
    let off = Vec3::new(0.4226, 0.0, 0.9063);
    s.pad(off, 0.5, true, false);
    trace_frames(&mut s, "walk off the 6 m block, grab in the air", 2.5, |s, _| {
        if s.loco().current == ActorContextId::InAir {
            s.pad(Vec3::NEG_Z, 0.5, true, true);
        }
    });
}

#[test]
#[ignore]
fn trace_roof_gap_jump() {
    let mut s = Sim::new(Vec3::new(6.0, 3.5, 12.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    trace_frames(&mut s, "free-run roof A to B", 3.0, |s, _| {
        if s.saw_air && s.loco().current == ActorContextId::Ground {
            s.pad(Vec3::X, 0.0, false, false);
        }
    });
}

#[test]
#[ignore]
fn trace_step_up_jump() {
    // stepped roofs: 3.2 -> 4.4 (1.2 m up, 2 m gap)
    let mut s = Sim::new(Vec3::new(5.5, 3.2, 24.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    trace_frames(&mut s, "free-run up a step", 3.0, |_, _| {});
}

#[test]
#[ignore]
fn trace_diagonal_run() {
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    let d = Vec3::new(1.0, 0.0, -0.4);
    s.pad(d, 1.0, true, false);
    trace_frames(&mut s, "run at an angle", 2.0, |_, _| {});
}

#[test]
#[ignore]
fn trace_wall_at_an_angle() {
    // wall x -7..7, z -8.3..-7.7, h 2.4: run into it at 30 degrees
    let mut s = Sim::new(Vec3::new(-3.0, 0.0, -4.0), 0.0);
    s.pad(Vec3::new(0.5, 0.0, -0.866), 1.0, true, false);
    trace_frames(&mut s, "run into the wall at 30 deg", 2.5, |_, _| {});
}

// ---------------------------------------------------------------- user bug round 2026-10-03 (regressions)

/// Horizontal speed of the feet over the last frame.
fn hspeed(prev: Vec3, now: Vec3) -> f32 {
    Vec2::new(now.x - prev.x, now.z - prev.z).length() * 60.0
}

#[test]
fn landing_with_the_stick_released_plays_no_run_stop() {
    use crate::player::jump_blend::{RUN_STOP, RUN_STOP_TO_WAIT};
    let mut s = Sim::new(Vec3::new(7.0, 3.5, 12.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    assert!(s.run_until(4.0, |s| s.saw_air && s.loco().current == ActorContextId::Ground), "never landed");
    s.pad(Vec3::X, 0.0, false, false);
    let landed_x = s.body().feet.x;
    for _ in 0..120 {
        s.run(1.0 / 60.0 + 1e-4);
        let os = s.ground().oneshot.map(|o| o.blend.id);
        assert!(!os.is_some_and(|id| RUN_STOP.contains(&id) || RUN_STOP_TO_WAIT.contains(&id)), "a run stop followed the landing");
    }
    assert_eq!(s.ground().speed_param, 0.0);
    // only the reception's own root motion moved the feet
    assert!(s.body().feet.x - landed_x < 0.8, "slid on after the landing: {} -> {}", landed_x, s.body().feet.x);
}

#[test]
fn moving_again_during_a_run_stop_waits_for_the_locked_stop_then_leaves_its_settle() {
    use crate::player::anim_gate;
    use crate::player::jump_blend::{RUN_STOP, RUN_STOP_TO_WAIT};
    // the stop item is locked (word 0x3a / 0x35: 0x20); its settle into the wait allows leaving for moving
    // (0xf8a / 0xf85: 0x200 / 0x800), `HumanGround__AnimAllowsModeExit` 0xD80010
    let mut s = Sim::new(Vec3::new(0.0, 0.0, -30.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, false);
    s.run(2.0);
    s.pad(Vec3::X, 0.0, true, false);
    s.run(1.0 / 60.0 + 1e-4);
    let stop = s.ground().oneshot.expect("run stop");
    assert!(RUN_STOP.contains(&stop.blend.id) && anim_gate::locked(&stop.blend));
    s.pad(Vec3::X, 1.0, true, false);
    // the stop plays out (no cut)
    for _ in 0..6 {
        s.run(1.0 / 60.0 + 1e-4);
        assert!(s.ground().oneshot.is_some_and(|o| RUN_STOP.contains(&o.blend.id)), "the locked stop was cut");
    }
    // then the locomotion takes over at once instead of the settle
    assert!(s.run_until(1.5, |s| s.ground().oneshot.is_none() && s.ground().speed_param > 0.0), "never moved again");
    let _ = RUN_STOP_TO_WAIT;
}

#[test]
fn pushing_the_stick_after_a_pull_up_leaves_its_end_clip_at_once() {
    use crate::player::anim_gate;
    // the pull-up ends with `hangknee_foot*_tr_freestep_entry` (0x248E9730 / 31, word 0xfc4 / 0xfc8): every mode exit
    // allowed, so the stick leaves it for the locomotion straight away (this wall top is narrow: the walk itself then
    // halts at its far edge, 0xEE7BDA)
    let mut s = hang_on_jump_up_wall();
    s.pad(Vec3::Z, 1.0, false, false);
    assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::Ground), "never pulled up");
    let os = s.ground().oneshot;
    if let Some(o) = os {
        assert!(anim_gate::allows_mode_exit(&o.blend, true, false), "{:#x}", o.blend.id);
    }
    s.run(2.0 / 60.0 + 1e-4);
    assert!(s.ground().oneshot.is_none(), "still in the pull-up's end clip: {:?}", s.ground().oneshot.map(|o| o.blend.id));
}

#[test]
fn pull_up_ends_standing_still() {
    let mut s = hang_on_jump_up_wall();
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()));
    s.pad(Vec3::Z, 1.0, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground));
    assert_eq!(s.ground().speed_param, 0.0, "the speed from before the climb must not carry over");
    s.pad(Vec3::Z, 0.0, false, false);
    let f = s.body().feet;
    s.run(1.0);
    assert!((s.body().feet - f).length() < 0.05, "moved after the pull-up: {f:?} -> {:?}", s.body().feet);
}

#[test]
fn a_jump_that_clips_a_higher_roof_lip_steps_onto_it() {
    // stepped roofs 3.2 -> 4.4 (2 m gap): the jump's foot catches the lip
    let mut s = Sim::new(Vec3::new(5.5, 3.2, 24.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir), "no jump");
    let mut prev = s.body().feet;
    for _ in 0..90 {
        s.run(1.0 / 60.0 + 1e-4);
        let f = s.body().feet;
        assert!((f.y - prev.y) * 60.0 < 7.0, "flung up: {prev:?} -> {f:?}");
        assert!(!matches!(s.data().air.mode, air::AirMode::Fall { .. }), "the jump aborted into a fall at {f:?}");
        prev = f;
        if s.loco().current == ActorContextId::Ground {
            break;
        }
    }
    assert_eq!(s.loco().current, ActorContextId::Ground);
    assert!((s.body().feet.y - 4.4).abs() < 0.05 && s.body().feet.x > 11.5, "on the upper roof: {:?}", s.body().feet);
}

#[test]
fn a_free_jump_keeps_its_speed_and_falls_on_smoothly() {
    // sprint off the 6 m block with nothing to land on
    let mut s = Sim::new(Vec3::new(-12.5, 6.0, 4.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir));
    let mut prev = s.body().feet;
    let mut last_h: Option<f32> = None;
    while s.loco().current == ActorContextId::InAir {
        s.run(1.0 / 60.0 + 1e-4);
        let h = hspeed(prev, s.body().feet);
        if let Some(l) = last_h {
            assert!((h - l).abs() < 0.6, "horizontal speed jumped {l:.2} -> {h:.2} at {:?} ({:?})", s.body().feet, s.data().air.mode);
        }
        last_h = Some(h);
        prev = s.body().feet;
    }
}

#[test]
fn obstacle_collision_never_snaps_back() {
    // against the 2.4 m wall (z -8.3..-7.7, face at -7.7) facing 30 degrees off it: the lean warps onto the wall once
    let mut s = Sim::new(Vec3::new(-3.0, 0.0, -7.3), crate::player::heading_of(Vec3::new(0.5, 0.0, -0.866)));
    s.run(0.2);
    assert!(send_obstacle_collision(&mut s), "no obstacle ahead: {:?}", s.body().feet);
    s.pad(Vec3::new(0.5, 0.0, -0.866), 1.0, true, false);
    let mut prev = s.body().feet;
    let mut prev_h = s.body().heading;
    for _ in 0..90 {
        s.run(1.0 / 60.0 + 1e-4);
        let f = s.body().feet;
        assert!((f - prev).length() < 0.03, "lean snapped: {prev:?} -> {f:?}");
        assert!((s.body().heading - prev_h).abs() < 0.2, "turned in one frame");
        prev = f;
        prev_h = s.body().heading;
    }
}

#[test]
fn the_hands_let_go_of_the_bar_on_a_swing_jump() {
    let mut s = Sim::new(Vec3::new(60.0, 1.2, 85.0), std::f32::consts::PI);
    s.pad(Vec3::Z, 1.0, true, false);
    s.run(0.35);
    s.press_legs();
    assert!(s.run_until(4.0, |s| s.data().ledge.swing.is_some_and(|w| matches!(w.phase, crate::player::swing::SwingPhase::Cycle(_)))));
    assert!(s.app.world().get::<crate::player::LimbTargets>(s.player).unwrap().hands.is_some(), "hands on the bar while swinging");
    s.press_legs();
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir), "no swing jump");
    s.run(1.0 / 60.0 + 1e-4);
    assert!(s.app.world().get::<crate::player::LimbTargets>(s.player).unwrap().hands.is_none(), "hands still pinned to the bar in the air");
}

#[test]
fn grabbing_near_the_end_of_a_ledge_keeps_both_hands_on_it() {
    // the jump-up wall's edge starts at x 8: stand at its very end and jump at it
    let mut s = Sim::new(Vec3::new(7.85, 0.0, 40.6), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ledge), "never hung: {:?}", s.body().feet);
    let d = s.data().ledge.hand_l.x.min(s.data().ledge.hand_r.x);
    assert!(d >= 8.0 + 0.04, "a hand past the end of the edge: {:?} {:?}", s.data().ledge.hand_l, s.data().ledge.hand_r);
}

#[test]
fn ledge_jumps_do_not_reach_across_the_map() {
    // a 2.6 m ledge 5 m away (the root rises more than 1 m): too far to jump at
    let (c, g) = level::geometry();
    let feet = Vec3::new(12.0, 0.0, 36.5);
    let t = crate::player::targets::find_jump_target(feet, Vec3::Z, &g, &c);
    assert!(t.is_none_or(|t| t.hang.is_none()), "jumped at a ledge from {:.1} m: {:?}", (t.unwrap().position - feet).length(), t);
    // within reach it still does
    let near = Vec3::new(12.0, 0.0, 39.8);
    assert!(crate::player::targets::find_jump_target(near, Vec3::Z, &g, &c).is_some_and(|t| t.hang.is_some()));
}

#[test]
fn reversing_the_stick_turns_one_way() {
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(1.0);
    // exactly opposite, with a little noise either side
    let mut last = 0.0f32;
    for i in 0..20 {
        let e = if i % 2 == 0 { 0.01 } else { -0.01 };
        s.pad(Vec3::new(e, 0.0, 1.0), 1.0, true, false);
        let h0 = s.body().heading;
        s.run(1.0 / 60.0 + 1e-4);
        let mut d = s.body().heading - h0;
        if d > std::f32::consts::PI { d -= std::f32::consts::TAU } else if d < -std::f32::consts::PI { d += std::f32::consts::TAU }
        // (the turn itself, not the 0.01 rad of stick noise it follows once it faces the stick)
        if d.abs() > 0.05 {
            assert!(last == 0.0 || d.signum() == last, "turn flipped direction at frame {i}");
            last = d.signum();
        }
    }
}

#[test]
fn running_off_a_roof_falls_once_off_the_rim() {
    // 3.5 m block x -14.5..-9.5: Movement only falls by the fall-off rule (0xDB46B0 → 0xB23CB0), once the capsule
    // (r 0.4) has no contact flatter than 45°, i.e. its centre ≈ 0.28 m past the edge. The edge-line ground loss
    // (0xD87720) is for the fight's grabbed reaction only.
    let mut s = Sim::new(Vec3::new(-11.5, 3.5, 24.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir), "never fell");
    let x = s.data().air.start.x;
    assert!((-9.5 + 0.2..-9.5 + 0.45).contains(&x), "fell at x {x} (edge at -9.5)");
}

#[test]
fn reversing_from_standing_pivots_with_the_turn_clip() {
    use crate::player::ground::PIVOT;
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0); // facing -Z
    s.run(0.2);
    s.pad(Vec3::Z, 1.0, true, false); // straight behind
    s.run(1.0 / 60.0 + 1e-4);
    let os = s.ground().oneshot.expect("no pivot");
    assert!(PIVOT.iter().flatten().flatten().flatten().any(|&id| id == os.blend.id), "not a pivot action: {:#x}", os.blend.id);
    assert!((os.blend.weights()[1] - 1.0).abs() < 0.05, "180 deg -> the 180 clip: {:?}", os.blend.weights());
    assert!(s.run_until(1.0, |s| s.ground().oneshot.is_none()), "pivot never ended");
    let f = s.body().forward();
    assert!(f.dot(Vec3::Z) > 0.99, "turned by the clip's root yaw: {f:?}");
    let p = s.body().feet;
    s.run(1.0);
    assert!(s.body().feet.z > p.z + 1.0, "walks off the other way");
}

#[test]
fn reversing_at_a_run_skids_then_pivots() {
    use crate::player::ground::PIVOT_ACTIONS;
    use crate::player::jump_blend::RUN_STOP;
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(1.5);
    s.pad(Vec3::Z, 1.0, true, false);
    s.run(1.0 / 60.0 + 1e-4);
    assert!(s.ground().oneshot.is_some_and(|o| RUN_STOP.contains(&o.blend.id)), "no skid (run stop) on pulling back");
    assert!(s.run_until(1.5, |s| s.ground().oneshot.is_some_and(|o| PIVOT_ACTIONS.contains(&o.blend.id))), "no pivot after the skid");
    assert!(s.run_until(1.0, |s| s.ground().oneshot.is_none_or(|o| !PIVOT_ACTIONS.contains(&o.blend.id))));
    assert!(s.body().forward().dot(Vec3::Z) > 0.95, "faces the stick: {:?}", s.body().forward());
}

#[test]
fn reversing_a_low_profile_walk_pivots() {
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, false, false);
    s.run(1.0);
    s.pad(Vec3::new(0.3, 0.0, 1.0), 1.0, false, false);
    s.run(1.0 / 60.0 + 1e-4);
    assert!(s.ground().oneshot.is_some_and(|o| crate::player::ground::PIVOT_ACTIONS.contains(&o.blend.id)), "no walk pivot");
}

#[test]
fn starting_from_standing_plays_the_start_item() {
    use crate::player::ground::START_MOVE;
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.run(0.2);
    s.pad(Vec3::NEG_Z, 1.0, false, false);
    s.run(1.0 / 60.0 + 1e-4);
    let os = s.ground().oneshot.expect("no start item");
    assert!(START_MOVE[0].contains(&os.blend.id), "low-profile start: {:#x}", os.blend.id);
    assert!((s.ground().speed_param - 0.25).abs() < 0.02, "walk band at once: {}", s.ground().speed_param);
    // steering stays live during it
    s.pad(Vec3::new(-0.5, 0.0, -1.0), 1.0, false, false);
    let h = s.body().heading;
    s.run(0.1);
    assert!((s.body().heading - h).abs() > 0.1, "turned while starting");
    // releasing the stick ends it
    s.pad(Vec3::ZERO, 0.0, false, false);
    s.run(1.0 / 60.0 + 1e-4);
    assert!(s.ground().oneshot.is_none_or(|o| !START_MOVE.iter().flatten().any(|&i| i == o.blend.id)));
}

#[test]
fn running_off_a_roof_is_not_the_fight_drop() {
    // the off-support fall (0xD8ADB0) is InAir kind 3 with no drop report: no HumanGround_Hurt drop entry
    let mut s = Sim::new(Vec3::new(-11.5, 3.5, 24.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir));
    assert_eq!(s.data().air.drop, None);
    // the fight drop's fall types (0xD8C380), kept for the grabbed reaction
    assert_eq!(air::fall_type(0.7, 1.0), 0);
    assert_eq!(air::fall_type(0.7, 3.0), 1);
    assert_eq!(air::fall_type(3.0, 1.0), 4);
}

#[test]
fn running_off_at_an_angle_keeps_the_facing() {
    // the drop steer (0xE04B60) belongs to the drop sub-state only; an off-support fall keeps its facing
    let mut s = Sim::new(Vec3::new(-11.0, 3.5, 22.5), -std::f32::consts::FRAC_PI_2);
    let d = Vec3::new(0.5, 0.0, 0.866);
    s.pad(d, 1.0, true, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir));
    let a0 = s.body().forward().dot(Vec3::X).acos().to_degrees();
    s.run(0.35);
    let a1 = s.body().forward().dot(Vec3::X).acos().to_degrees();
    assert!((a0 - a1).abs() < 1.0, "facing vs the edge normal: {a0:.1} -> {a1:.1} deg");
}

#[test]
fn grabbing_while_walking_off_does_not_stick_to_the_rim() {
    // walk off the 6 m block at 65 deg to the edge, then hold grab with the stick along the edge: the fall used to
    // start at the edge line with the capsule still on the rim, land on it after 2 cm and loop Ground <-> InAir
    let mut s = Sim::new(Vec3::new(-11.0, 6.0, 2.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::new(0.4226, 0.0, 0.9063), 0.5, true, false);
    assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::InAir), "never fell");
    s.pad(Vec3::NEG_Z, 0.5, true, true);
    assert!(s.run_until(3.0, |s| s.body().feet.y < 1.0 || s.loco().current == ActorContextId::Ledge), "stuck at {:?}", s.body().feet);
}

#[test]
#[ignore]
fn trace_low_profile_edge_halt() {
    let mut s = Sim::new(Vec3::new(9.0, 3.5, 12.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, false, false);
    trace_frames(&mut s, "low profile into roof A's edge", 1.6, |_, _| {});
}

#[test]
fn walking_into_a_lower_edge_halts_without_the_ledge_stop() {
    // roof A (3.5 m, drop 2-5 m) in low profile: the interpreter's edge halt (0xEE7BDA) stops the walk 0.16 m from the
    // edge, no ledge-stop clip, no fall, and no restart while the stick still pushes into it
    let mut s = Sim::new(Vec3::new(9.0, 3.5, 12.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, false, false);
    s.run(3.0);
    assert_eq!(s.loco().current, ActorContextId::Ground, "fell: {:?}", s.body().feet);
    assert!(s.ground().ledge_stop.is_none(), "no ledge stop below 5 m");
    let x = s.body().feet.x;
    assert!((11.0 - x - 0.16).abs() < 0.05, "halted 0.16 m from the edge: {x}");
}

#[test]
fn a_wall_hang_jumps_up_to_climb_holds_above() {
    use crate::player::climb::ClimbEntryType;
    use crate::player::ledge_moves::{MoveKind, LEDGE_JUMP_TABLE};
    // wall G (2.6 m) under tower G2: holds at 4.1 m (hands) and 2.9 m (feet), none within a hand step
    let mut s = hang_at(Vec3::new(22.0, 2.6, -20.3), Vec3::NEG_Z);
    s.pad(Vec3::Z, 1.0, false, false); // up
    let jumped = s.run_until(2.0, |s| s.data().ledge.mv.is_some_and(|m| m.kind == MoveKind::JumpUpToClimb));
    assert!(jumped, "no jump up to climb: {}", s.data().ledge.last_action);
    let [start, lp, end] = LEDGE_JUMP_TABLE[0][0].1;
    let mv = s.data().ledge.mv.unwrap();
    assert_eq!(mv.seq[0].map(|a| a.id), Some(start));
    assert_eq!(mv.seq[1].map(|a| a.id), Some(lp));
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Climb), "never reached the climb: {}", s.data().ledge.last_action);
    let c = &s.data().climb;
    assert_eq!(c.entry_type, ClimbEntryType::FromLedgeParallelJump);
    assert_eq!(c.move_action, Some(end), "the end item plays in the climb");
    assert!((c.hand_l.y - 4.1).abs() < 0.05 && (c.foot_l.y - 2.9).abs() < 0.05, "hands {:?} feet {:?}", c.hand_l, c.foot_l);
    let want = crate::player::climb::climb_root(c.foot_l, c.foot_r, c.normal);
    // the root reaches the climb pose
    assert!(s.run_until(2.0, |s| s.data().climb.moving.is_none()));
    assert!((s.body().feet - want).length() < 0.05, "root {:?} want {want:?}", s.body().feet);
}

#[test]
fn a_free_hang_drops_onto_climb_holds_below() {
    use crate::player::climb::FREE_DROP_TO_CLIMB;
    use crate::player::ledge_moves::MoveKind;
    // the climb tower's overhang slab (-Z edge z 26.0, 3.5 m): the tower bands 0.9 m ahead at 2.4 / 1.2 m
    let mut s = hang_at(Vec3::new(-22.0, 3.5, 26.0), Vec3::NEG_Z);
    s.run_until(1.0, |s| s.data().ledge.mv.is_none() && s.data().ledge.hang_set);
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Free);
    s.pad(Vec3::NEG_Z, 1.0, false, false); // down
    let dropped = s.run_until(2.0, |s| s.data().ledge.mv.is_some_and(|m| m.kind == MoveKind::FreeHangDrop { to_climb: true }));
    assert!(dropped, "no drop: {}", s.data().ledge.last_action);
    let mv = s.data().ledge.mv.unwrap();
    assert_eq!(mv.seq[0].map(|a| a.id), Some(FREE_DROP_TO_CLIMB[0]));
    assert_eq!(mv.seq[1].map(|a| a.id), Some(FREE_DROP_TO_CLIMB[1]));
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::Climb), "never reached the climb: {}", s.data().ledge.last_action);
    let c = &s.data().climb;
    assert!((c.hand_l.y - 2.4).abs() < 0.05 && (c.foot_l.y - 1.2).abs() < 0.05, "hands {:?} feet {:?}", c.hand_l, c.foot_l);
    let want = crate::player::climb::climb_root(c.foot_l, c.foot_r, c.normal);
    assert!((s.body().feet - want).length() < 0.05, "root {:?} want {want:?}", s.body().feet);
}

#[test]
fn a_free_hang_drops_into_a_wall_hang_on_the_ledge_below() {
    use crate::player::climb::FREE_DROP_TO_HANG;
    use crate::player::ledge_moves::MoveKind;
    // overhang H (-Z edge z -20.0, 3.6 m), wall H's 2.4 m ledge 0.9 m ahead with no climb holds below it
    let mut s = hang_at(Vec3::new(28.0, 3.6, -20.0), Vec3::NEG_Z);
    s.run_until(1.0, |s| s.data().ledge.mv.is_none() && s.data().ledge.hang_set);
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Free);
    s.pad(Vec3::NEG_Z, 1.0, false, false); // down
    let dropped = s.run_until(2.0, |s| s.data().ledge.mv.is_some_and(|m| m.kind == MoveKind::FreeHangDrop { to_climb: false }));
    assert!(dropped, "no drop: {}", s.data().ledge.last_action);
    assert_eq!(s.data().ledge.mv.unwrap().seq[0].map(|a| a.id), Some(FREE_DROP_TO_HANG[0]));
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(4.0, |s| s.data().ledge.mv.is_none()));
    let l = &s.data().ledge;
    assert_eq!(s.loco().current, ActorContextId::Ledge);
    assert_eq!(l.hang_type, ledge::LedgeHangType::Wall);
    assert!((l.hand_l.y - 2.4).abs() < 0.05 && (l.hand_l.z + 19.1).abs() < 0.05, "hands on wall H: {:?}", l.hand_l);
}

/// Replays every recording in `port/bugs/*/input.txt` through the gameplay systems (no rendering, animation or IK)
/// and checks each frame's context, root and heading against the `state.txt` captured in the game. Run with
/// `cargo test --release replay_bug_recordings -- --ignored --nocapture`; set `AC_BUG=<folder name>` for one bug.
#[test]
#[ignore]
fn replay_bug_recordings() {
    use crate::recorder::{bugs_dir, gameplay_key, Recording};
    let only = std::env::var("AC_BUG").ok();
    let Ok(entries) = std::fs::read_dir(bugs_dir()) else {
        eprintln!("no bugs folder at {}", bugs_dir().display());
        return;
    };
    let mut failures = Vec::new();
    for e in entries.flatten() {
        let dir = e.path();
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        if only.as_ref().is_some_and(|o| *o != name) {
            continue;
        }
        let Some(rec) = Recording::load(&dir) else { continue };
        let state = std::fs::read_to_string(dir.join("state.txt")).unwrap_or_default();
        let want: std::collections::HashMap<u64, String> = state
            .lines()
            .filter_map(|l| {
                let key = l.split(" | ").next()?.to_string();
                Some((key.split_whitespace().next()?.parse().ok()?, key))
            })
            .collect();
        let mut s = Sim::new_raw(rec.start_feet, rec.start_heading);
        let mut first_diff = None;
        let mut compared = 0;
        for (i, f) in rec.frames.iter().enumerate() {
            s.app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_nanos(f.dt_nanos)));
            f.apply(&mut s.app.world_mut().resource_mut::<PadInput>());
            s.app.update();
            if let Some(w) = want.get(&(i as u64)) {
                compared += 1;
                let got = gameplay_key(i as u64, s.loco(), s.body());
                if first_diff.is_none() && got != *w {
                    first_diff = Some(format!("frame {i}: game `{w}` replay `{got}`"));
                }
            }
        }
        match first_diff {
            None => eprintln!("{name}: {} frames replayed, {compared} compared, identical", rec.frames.len()),
            Some(d) => {
                eprintln!("{name}: DIFFERS at {d}");
                failures.push(name);
            }
        }
    }
    assert!(failures.is_empty(), "replays differ: {failures:?}");
}

#[test]
fn a_hang_beside_a_ladder_moves_sideways_onto_it() {
    use crate::player::ladder::{LadderPhase, FROM_LEDGE_SIDE};
    use crate::player::ledge_moves::MoveKind;
    // the 5 m ladder wall (x 48..52, face z 63.5), ladder at x 50: hang on its top edge 0.6 m to the left
    let mut s = hang_at(Vec3::new(49.4, 5.0, 63.5), Vec3::NEG_Z);
    s.run_until(1.0, |s| s.data().ledge.mv.is_none() && s.data().ledge.hang_set);
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Wall);
    s.pad(Vec3::X, 1.0, false, false); // toward the ladder: +X is the character's left (facing +Z)
    let moved = s.run_until(2.0, |s| s.data().ledge.mv.is_some_and(|m| m.kind == MoveKind::SideToLadder));
    assert!(moved, "no side move to the ladder: {}", s.data().ledge.last_action);
    assert_eq!(s.data().ledge.mv.unwrap().seq[0].map(|a| a.id), Some(FROM_LEDGE_SIDE[0][0]));
    s.pad(Vec3::X, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ladder), "never reached the ladder: {}", s.data().ledge.last_action);
    let l = &s.data().ladder;
    assert_eq!(l.phase, Some(LadderPhase::Wait));
    assert_eq!(l.foot, 1, "`hangwall_tr_l_ladder_wait_r_left` ends on the right foot");
    let want = l.root_at(l.height);
    assert!((s.body().feet - want).length() < 0.05 && (want.x - 50.0).abs() < 1e-3, "root {:?} want {want:?}", s.body().feet);
    assert!((l.height - (5.0 - 1.1 - 0.5)).abs() < 0.1, "height {}", l.height);
}

#[test]
fn a_climb_hold_that_disappears_drops_the_climber() {
    use crate::guidance::GuidanceWorld;
    use crate::player::air::FallOrigin;
    // start climbing the tower, then delete the band under the hands (the game: the hold's object is destroyed or
    // unloaded, sub_E55A10 fails)
    let mut s = Sim::new(Vec3::new(-20.0, 0.0, 26.5), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Climb));
    s.pad(Vec3::Z, 0.0, true, false);
    assert!(s.run_until(2.0, |s| s.data().climb.moving.is_none()));
    s.run(0.2);
    assert_eq!(s.loco().current, ActorContextId::Climb, "holding on while the holds exist");
    let hand_y = s.data().climb.hand_l.y;
    s.app.world_mut().resource_mut::<GuidanceWorld>().edges.retain(|e| (e.p0.y - hand_y).abs() > 0.05);
    s.run(2.0 / 60.0);
    assert_eq!(s.loco().current, ActorContextId::InAir);
    assert_eq!(s.data().air.fall_origin, FallOrigin::Climb);
    assert_eq!(s.data().climb.last_action, "lost grip");
}

#[test]
fn climbing_up_to_a_top_edge_climbs_out_onto_the_roof() {
    use crate::player::ledge_moves::{MoveKind, ACT_KNEE_TO_FREESTEP, CLIMB_OUT};
    // climb the tower's middle column to the top (9.6 m): TryReachLedgeAbove 0xDF1730, hangknee, then the free step
    let mut s = Sim::new(Vec3::new(-20.5, 0.0, 26.5), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Climb));
    s.pad(Vec3::Z, 1.0, true, false);
    let out = s.run_until(30.0, |s| s.data().ledge.mv.is_some_and(|m| m.kind == MoveKind::Pullup) && s.loco().current == ActorContextId::Ledge);
    assert!(out, "never climbed out: climb {:?} pose {}", s.data().climb.last_action, s.data().climb.pose);
    let pose = s.data().climb.pose;
    let mv = s.data().ledge.mv.unwrap();
    assert_eq!(mv.seq[0].map(|a| a.id), Some(CLIMB_OUT[(pose == 3) as usize]), "1m / 2m by the pose ({pose})");
    assert_eq!(mv.seq[1].map(|a| a.id), Some(CLIMB_OUT[(pose == 3) as usize]), "both items of the action");
    assert!((mv.to.y - 9.6).abs() < 0.05 && (mv.to.z - 27.1).abs() < 0.05, "knee on the edge, 0.1 m in: {:?}", mv.to);
    assert!(s.run_until(4.0, |s| s.data().ledge.mv.is_some_and(|m| m.seq[0].map(|a| a.id) == Some(ACT_KNEE_TO_FREESTEP[0]))));
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground));
    assert!(s.body().feet.y > 9.5 && s.body().feet.z > 27.0, "standing on the roof: {:?}", s.body().feet);
}

#[test]
fn climbing_down_to_the_bottom_steps_off_onto_the_ground() {
    // climb the tower a few rows, then hold down: at the lowest holds TryDropToLedgeBelow 0xDF1D20 finds the ground
    // under the root and the climber stands up (climb_{1m,2m}_tr_h_wait_a, then Ground with the _b action)
    let mut s = Sim::new(Vec3::new(-20.5, 0.0, 26.5), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Climb));
    s.pad(Vec3::Z, 1.0, true, false);
    assert!(s.run_until(10.0, |s| s.data().climb.foot_l.y.min(s.data().climb.foot_r.y) > 2.0), "never climbed up");
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    let down = s.run_until(15.0, |s| s.data().climb.to_ground);
    assert!(down, "never climbed down to the ground: {} pose {}", s.data().climb.last_action, s.data().climb.pose);
    let pose = s.data().climb.pose;
    assert_eq!(s.data().climb.move_action, Some(climb::CLIMB_TO_GROUND[(pose == 3) as usize]));
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ground));
    assert!(s.body().feet.y.abs() < 0.05, "standing on the ground: {:?}", s.body().feet);
    let one = s.ground().oneshot.expect("the _b transition plays in Ground");
    assert_eq!(one.blend.id, climb::CLIMB_TO_GROUND_B[(pose == 3) as usize]);
}

#[test]
fn pulling_up_at_the_end_of_a_wall_uses_one_hand() {
    use crate::player::ledge_moves::{ACT_KNEE_ONEHAND_TO_FREESTEP, ACT_PULLUP_WALL, ACT_PULLUP_WALL_ONEHAND, MoveKind};
    // wall C (x 17..23, top 2.6, face z 49.7): in the middle both hands, 0.3 m from its -X end the top drops away
    // within 0.35 m to the side (HumanLedge__PullupNeedsOneHand 0xDD1120) → the one-hand pull-up
    for (x, one) in [(20.0f32, false), (17.3, true)] {
        let mut s = hang_at(Vec3::new(x, 2.6, 49.7), Vec3::NEG_Z);
        assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none() && s.data().ledge.hang_set));
        assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Wall);
        s.pad(Vec3::Z, 1.0, false, false);
        assert!(s.run_until(2.0, |s| s.data().ledge.mv.is_some_and(|m| m.kind == MoveKind::Pullup)), "no pull-up at x {x}");
        let mv = s.data().ledge.mv.unwrap();
        let want = if one { ACT_PULLUP_WALL_ONEHAND } else { ACT_PULLUP_WALL };
        assert_eq!(mv.seq[0].map(|a| a.id), Some(want), "x {x}");
        if one {
            assert_eq!(mv.seq[2].map(|a| a.id), Some(ACT_KNEE_ONEHAND_TO_FREESTEP));
        }
        s.pad(Vec3::Z, 0.0, false, false);
        assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::Ground), "never stood up at x {x}");
    }
}

#[test]
fn a_long_side_jump_into_a_free_hang_catches_one_handed_then_reaches_with_the_other() {
    use crate::player::ledge_moves::{MoveKind, SECOND_HAND_LOOPS};
    // wall D (x 24..27) → slab I (x 28.6..29.6, free hang): the long jump's loop item is a SecondHandGrab trigger
    let mut s = hang_at(Vec3::new(26.6, 2.6, 49.7), Vec3::NEG_Z);
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none() && s.data().ledge.hang_set));
    s.pad(Vec3::X, 1.0, false, false);
    let jumped = s.run_until(4.0, |s| s.data().ledge.mv.is_some_and(|m| matches!(m.kind, MoveKind::SideJump { long: true })));
    assert!(jumped, "no long side jump: {}", s.data().ledge.last_action);
    let mv = s.data().ledge.mv.unwrap();
    let lp = mv.seq[1].map(|a| a.id).unwrap();
    assert!(SECOND_HAND_LOOPS.contains(&lp), "loop {lp:#x} ends on one hand");
    assert_eq!(mv.hand_l, mv.hand_r, "caught with one hand");
    let tail = s.data().ledge.queue.first().copied().expect("the second hand's reach");
    assert_eq!(tail.seq[0].map(|a| a.id), Some(lp + 2), "`_3_d`");
    s.pad(Vec3::X, 0.0, false, false);
    assert!(s.run_until(4.0, |s| s.data().ledge.mv.is_none() && s.data().ledge.queue.is_empty()));
    let l = &s.data().ledge;
    assert_eq!(l.hang_type, ledge::LedgeHangType::Free);
    let gap = (l.hand_l - l.hand_r).length();
    assert!((gap - 0.25).abs() < 0.03, "second hand 0.25 m from the first: {gap}");
    assert!(l.hand_l.x > 28.55 && l.hand_r.x > 28.55, "both hands on slab I: {:?} {:?}", l.hand_l, l.hand_r);
}

#[test]
fn a_long_reach_from_the_climb_into_a_free_hang_catches_one_handed() {
    use crate::player::ledge_moves::{MoveKind, CLIMB_REACH_TABLE};
    // TryReachOtherSurface 0xDF9B30: wall P's holds end at x -56; slab Q (x -54.2..-53.0, top 3.6, nothing below) is a
    // free-hang edge at hand height past the gap. Pushing left (+X) at the end of the holds plays
    // `climb1m_tr_hangfree_left_3` (start in the climb, loop into the Ledge context), caught on one hand (SecondHandGrab)
    let mut s = Sim::new(Vec3::new(-56.8, 0.0, -11.4), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Climb), "never climbed: {:?}", s.body().feet);
    s.pad(Vec3::Z, 0.45, true, false);
    let level = |s: &Sim| {
        let c = &s.data().climb;
        c.moving.is_none() && (c.foot_l.y - 2.4).abs() < 0.01 && (c.foot_r.y - 2.4).abs() < 0.01
    };
    assert!(s.run_until(10.0, level), "feet never reached 2.4 m: {:?} {:?}", s.data().climb.foot_l, s.data().climb.foot_r);
    s.pad(Vec3::X, 0.45, true, false);
    let reached = s.run_until(6.0, |s| s.loco().current == ActorContextId::Ledge);
    assert!(reached, "no reach: {} feet {:?} {:?}", s.data().climb.last_action, s.data().climb.foot_l, s.data().climb.foot_r);
    assert_eq!(s.data().climb.last_action, "reach to a hang");
    let mv = s.data().ledge.mv.expect("the reach");
    assert!(matches!(mv.kind, MoveKind::ClimbReach { long: true }), "a long reach: {:?}", mv.kind);
    let ids = CLIMB_REACH_TABLE[2][1][0];
    assert_eq!(mv.seq.map(|a| a.map(|a| a.id)), [Some(ids[0]), Some(ids[1]), Some(ids[2]), None], "climb1m_tr_hangfree_left_3 a / b / c");
    assert_eq!(mv.hand_l, mv.hand_r, "caught with one hand");
    let tail = s.data().ledge.queue.first().copied().expect("the second hand's reach");
    assert_eq!(tail.seq[0].map(|a| a.id), Some(ids[1] + 2), "`_3_d`");
    s.pad(Vec3::X, 0.0, false, false);
    assert!(s.run_until(5.0, |s| s.data().ledge.mv.is_none() && s.data().ledge.queue.is_empty()), "never settled");
    let l = &s.data().ledge;
    assert_eq!(l.hang_type, ledge::LedgeHangType::Free);
    assert!(l.hand_l.x > -54.25 && l.hand_r.x > -54.25 && (l.hand_l.y - 3.6).abs() < 0.05, "both hands on slab Q: {:?} {:?}", l.hand_l, l.hand_r);
}

/// Grab the wall in front (facing +Z), climb with a light push until both feet are on the 2.4 m band, then push left
/// (+X) lightly until `done`.
fn climb_then_left(start: Vec3, done: impl Fn(&Sim) -> bool) -> Sim {
    let mut s = Sim::new(start, FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Climb), "never climbed: {:?}", s.body().feet);
    s.pad(Vec3::Z, 0.45, true, false);
    let level = |s: &Sim| {
        let c = &s.data().climb;
        c.moving.is_none() && (c.foot_l.y - 2.4).abs() < 0.01 && (c.foot_r.y - 2.4).abs() < 0.01
    };
    assert!(s.run_until(10.0, level), "feet never reached 2.4 m: {:?} {:?}", s.data().climb.foot_l, s.data().climb.foot_r);
    s.pad(Vec3::X, 0.45, true, false);
    let ok = s.run_until(8.0, &done);
    assert!(ok, "never got there: {} feet {:?} {:?}", s.data().climb.last_action, s.data().climb.foot_l, s.data().climb.foot_r);
    s
}

#[test]
fn the_climb_reaches_across_a_gap_onto_climb_holds() {
    use crate::player::ledge_moves::CLIMB_REACH_TABLE;
    // ReachOtherSurfaceSide 0xDF7850 type 0: wall R1's holds end at x -70; wall R2's start 2 m further left. The reach
    // (`climb1m_tr_climb1m_left_3`: start in place, loop to the new holds, end in the wait) keeps the climb.
    let mut s = climb_then_left(Vec3::new(-71.0, 0.0, -11.4), |s| s.data().climb.reach.is_some());
    let r = s.data().climb.reach.unwrap();
    assert_eq!(s.data().climb.last_action, "reach to climb holds");
    assert_eq!(r.ids, CLIMB_REACH_TABLE[0][1][0], "the long left reach");
    s.pad(Vec3::X, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().climb.reach.is_none() && s.data().climb.moving.is_none()), "never arrived");
    let c = &s.data().climb;
    assert_eq!(s.loco().current, ActorContextId::Climb);
    assert!(c.settle.is_some_and(|(id, _)| id == r.ids[2]), "the end plays in the wait: {:?}", c.settle);
    assert!(c.hand_l.x > -68.0 && c.foot_l.x > -68.0 && (c.foot_l.y - 2.4).abs() < 0.05 && (c.hand_l.y - 3.6).abs() < 0.05, "on wall R2: {:?} {:?}", c.hand_l, c.foot_l);
    assert!((s.body().feet - c.frame.root).length() < 1e-3, "the root on the new pose");
}

#[test]
fn the_climb_turns_into_an_inside_corner() {
    use crate::player::climb::CORNER_CLIMB;
    // TrySideJump 0xDF29E0 near: wall T stands square to wall S on its left, with holds on its -X face
    let mut s = climb_then_left(Vec3::new(-81.0, 0.0, -11.4), |s| s.data().climb.last_action == "corner climb in");
    assert_eq!(s.data().climb.move_action, Some(CORNER_CLIMB[0][0]), "`corner_left_090_in`");
    s.pad(Vec3::X, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().climb.moving.is_none()), "never arrived");
    let c = &s.data().climb;
    assert!(c.normal.x < -0.99, "facing +X into wall T: {:?}", c.normal);
    assert!((c.foot_l.x + 80.08).abs() < 0.01 && (c.hand_l.y - c.foot_l.y - 1.2).abs() < 0.01, "on wall T's holds: {:?} {:?}", c.foot_l, c.hand_l);
    assert!((s.body().feet.x + 80.58).abs() < 0.05, "root 0.5 m out from wall T: {:?}", s.body().feet);
}

#[test]
fn the_climb_goes_round_an_outside_corner() {
    use crate::player::climb::CORNER_CLIMB;
    // TrySideJump 0xDF29E0 far: wall U ends at x -90, its end face carries holds
    let mut s = climb_then_left(Vec3::new(-91.0, 0.0, -11.4), |s| s.data().climb.last_action == "corner climb out");
    assert_eq!(s.data().climb.move_action, Some(CORNER_CLIMB[1][0]), "`corner_left_090_out`");
    s.pad(Vec3::X, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().climb.moving.is_none()), "never arrived");
    let c = &s.data().climb;
    assert!(c.normal.x > 0.99, "facing -X into the end face: {:?}", c.normal);
    assert!((c.foot_l.x + 89.92).abs() < 0.01, "on the end face's holds: {:?}", c.foot_l);
    assert!(s.body().feet.x > -89.6, "root out from the end face: {:?}", s.body().feet);
}

#[test]
fn the_climb_reaches_onto_a_ladder_at_the_side() {
    use crate::player::ladder::LadderPhase;
    // SideReachToLadder 0xDEF3A0: the ladder at x -100 is 2 m left of wall W's last holds
    let mut s = climb_then_left(Vec3::new(-102.8, 0.0, -11.4), |s| s.data().climb.reach.is_some_and(|r| r.ladder.is_some()));
    s.pad(Vec3::X, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ladder), "never reached the ladder: {}", s.data().climb.last_action);
    assert_eq!(s.data().ladder.phase, Some(LadderPhase::Entry), "the reach's end `…_tr_ladder_wait_r`");
    assert!(s.run_until(3.0, |s| s.data().ladder.phase == Some(LadderPhase::Wait)), "never settled");
    let f = s.body().feet;
    assert!((f.x + 100.0).abs() < 0.05 && (f.z + 11.0).abs() < 0.05, "0.5 m out from the ladder: {f:?}");
    assert_eq!(s.data().ladder.foot, 1, "right foot ahead after a left reach");
}

#[test]
fn the_climb_reaches_up_past_missing_holds() {
    use crate::player::climb::{reach_ids, CLIMB_REACH_VERTICAL};
    // ReachOtherSurfaceVertical 0xDF0050: wall V has no band at 3.0 m. With the hands on 2.4 m a short step up finds no
    // hold (a hard push would take the LONG grid move); the reach goes to feet 2.4 / hands 3.6.
    let mut s = Sim::new(Vec3::new(-116.0, 0.0, -11.4), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Climb), "never climbed");
    s.pad(Vec3::Z, 0.45, true, false);
    assert!(s.run_until(10.0, |s| s.data().climb.reach.is_some()), "no reach: {} {:?}", s.data().climb.last_action, s.data().climb.foot_l);
    let r = s.data().climb.reach.unwrap();
    assert_eq!(s.data().climb.last_action, "reach up / down");
    assert!(r.ids == reach_ids(CLIMB_REACH_VERTICAL[0][0]) || r.ids == reach_ids(CLIMB_REACH_VERTICAL[1][0]), "a short reach up: {:x?}", r.ids);
    assert!((r.holds[2].y - 2.4).abs() < 0.05 && (r.holds[0].y - 3.6).abs() < 0.05, "feet 2.4 / hands 3.6: {:?}", r.holds);
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().climb.reach.is_none() && s.data().climb.moving.is_none()), "never arrived");
    assert!((s.data().climb.foot_l.y - 2.4).abs() < 0.05);
}
