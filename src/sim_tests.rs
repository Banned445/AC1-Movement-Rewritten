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
    /// Scripted tests give world directions; the camera then stays behind the character so the interpreter's
    /// actor-frame stick (ledge / climb / ladder, RE/24 INPUT-4) means the same direction. Replays set the recorded yaw.
    cam_follow: bool,
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
            .add_systems(Update, (crate::player::interpreter::update_interpreter, crate::player::social::update_social, crate::player::crowd::update_crowd, ground::update_ground, air::update_air, ledge::update_ledge, climb::update_climb, crate::player::hay::update_hay, crate::player::kiosk::update_kiosk, crate::player::walling::update_walling, crate::player::narrow::update_narrow, crate::player::ladder::update_ladder, crate::player::release_limbs).chain());
        let (c, g) = level::geometry();
        app.insert_resource(c).insert_resource(g);
        let player = app.world_mut().spawn(player_components(feet, heading)).id();
        Sim { app, player, saw_air: false, cam_follow: true }
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
            if self.cam_follow {
                let h = self.body().heading;
                self.app.world_mut().resource_mut::<CameraRig>().yaw = h;
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
    /// Hold (or release) the empty-hand button: its state byte, which `Pad__IsPressed` 0x93F170 reads.
    fn hold_hand(&mut self, held: bool) {
        self.app.world_mut().resource_mut::<PadInput>().hand_held = held;
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
fn a_context_switched_in_last_frame_runs_its_update_at_once() {
    // AIActor__UpdateContexts 0x55F6A9 clears justSwitched right after the update that switched, so the new context's
    // next update is the next frame's; the port used to skip one more frame (RE/24 INPUT-1). The ground's update grows
    // the turn band's k (UpdateHeading 0xD95290) from the 0 its entry sets.
    let mut s = hang_at(Vec3::new(12.0, 2.6, 41.7), Vec3::NEG_Z);
    force(&mut s, crate::player::TransitionSetup::ToMovement { landing: None });
    assert_eq!(s.ground().turn_k, 0.0);
    s.run(1.0 / 60.0 + 1e-4);
    assert!(s.ground().turn_k > 0.0, "the ground skipped its first frame");
}

#[test]
fn a_flick_back_raises_the_quick_reversal_for_0_3_s() {
    // ProcessGroundMovement 0xEE68C3: the stick entering a band more than 120° from the facing sets +0x112C → HG+1780
    // for 0.3 s (RE/24 INPUT-7); a stick that only swings round within its band does not
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(1.5);
    assert!(!s.ground().quick_turn);
    s.pad(Vec3::Z, 1.0, true, false); // swung round without passing the centre: no band entry
    s.run(1.0 / 60.0 + 1e-4);
    assert!(!s.ground().quick_turn, "no band entry, no flag");
    s.pad(Vec3::ZERO, 0.0, true, false);
    s.run(0.2);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(1.0);
    s.pad(Vec3::ZERO, 0.0, true, false);
    s.run(1.0 / 60.0 + 1e-4);
    s.pad(Vec3::Z, 1.0, true, false); // the flick: out of the centre straight into band 2, behind
    s.run(1.0 / 60.0 + 1e-4);
    assert!(s.ground().quick_turn, "the flick back sets HG+1780");
    s.pad(Vec3::ZERO, 0.0, true, false);
    s.run(0.4);
    assert!(!s.ground().quick_turn, "cleared once its 0.3 s ran out");
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
fn eight_metre_drop_lands_at_the_band_edge() {
    // tower at (-12, 14), 5x5, h 8: run off its +X edge at 65 deg (a drop > 5 m straight ahead would ledge-stop,
    // 0xEE8899; at 65 deg it is not a front edge)
    let mut s = Sim::new(Vec3::new(-11.0, 8.0, 12.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::new(0.4226, 0.0, 0.9063), 1.0, true, false);
    let landed = s.run_until(4.0, |s| s.saw_air && s.ground().last_landing.is_some());
    assert!(landed && s.saw_air);
    s.pad(Vec3::X, 0.0, false, false);
    s.run(0.2);
    let l = s.ground().last_landing.expect("landed");
    // the step off the rim (event 68) is a small free jump whose takeoff (word 0x1000) already descends: the apex is
    // recorded only after the takeoff (0xE0DF99, RE/24 AIR-31), so the fall stays under the 8 m band edge (0xED6280:
    // 8 m and more is SmallDamage). LIVE (RE/20 #3): the game's fall height after a step off (RE/18 §9)
    assert!((7.45..7.75).contains(&l.fall_height), "{l:?}");
    assert!(l.damage <= 10, "{l:?}");
    assert!(s.body().feet.x < -5.0, "unexpected respawn: {:?}", s.body().feet);
}

#[test]
fn six_metre_drop_rolls_without_heavy_damage() {
    // Player fixed table (0xED6280 / 0xE00FE0): 6 m is Safe; total drop >3 selects a roll.
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
    // past the step on the ground (further on, the 0.6 m box at x 9 is hopped onto by the free-run target jump)
    assert!(s.run_until(2.0, |s| s.body().feet.x > 7.0 && s.loco().current == ActorContextId::Ground), "blocked by 0.3 m step: {:?}", s.body().feet);
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
    walk_to_wall(&mut s, Vec3::Z, 2.0);
    s.pad(Vec3::Z, 1.0, true, false);
    s.press_hand(); // climbing from the ground: the empty-hand press (event 50, RE/02 4.7)
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Climb), "did not start climbing");
    assert_eq!(s.data().climb.entry_type, climb::ClimbEntryType::FromGround);
    let reached = s.run_until(30.0, |s| s.loco().current == ActorContextId::Ground && s.body().feet.y > 9.5);
    let f = s.body().feet;
    assert!(reached, "never stood on the roof: ctx {:?} feet {f:?} climb {:?} ledge {:?}", s.loco().current, s.data().climb.last_action, s.data().ledge.last_action);
    assert!(f.z > 27.0, "should be on the roof, feet {f:?}");
}

/// RE/24 CLIMB-2: the player always tries the LONG table first (ClimbData+0x30 = 1.0 every frame, 0xEE60E6), so a light
/// stick push (0.45, under the old 0.5 threshold) still climbs with LONG moves.
#[test]
fn a_light_stick_push_still_climbs_with_long_moves() {
    use crate::player::climb::LONG_ACTIONS;
    let mut s = Sim::new(Vec3::new(-20.5, 0.0, 26.5), FACE_PZ);
    walk_to_wall(&mut s, Vec3::Z, 2.0);
    s.pad(Vec3::Z, 1.0, true, false);
    s.press_hand();
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Climb), "did not start climbing");
    s.pad(Vec3::Z, 0.45, true, false);
    let long: Vec<u32> = LONG_ACTIONS.iter().flatten().copied().filter(|&a| a != 0 && a != u32::MAX).collect();
    let used_long = s.run_until(6.0, |s| s.data().climb.move_action.is_some_and(|a| long.contains(&a)));
    assert!(used_long, "no LONG move with a light push: {:?}", s.data().climb.move_action);
}

/// Start climbing the tower from the ground and let the entry finish.
fn climbing_the_tower() -> Sim {
    let mut s = Sim::new(Vec3::new(-20.5, 0.0, 26.5), FACE_PZ);
    walk_to_wall(&mut s, Vec3::Z, 2.0);
    s.pad(Vec3::Z, 1.0, true, false);
    s.press_hand();
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Climb), "did not start climbing");
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().climb.moving.is_none()), "the entry never ended");
    s
}

/// RE/24 CLIMB-17: a grid move's root interpolation advances once in the frame it starts (StartMove 0xDFA7A2).
#[test]
fn a_climb_move_moves_the_root_in_the_frame_it_starts() {
    let mut s = climbing_the_tower();
    s.pad(Vec3::Z, 1.0, true, false);
    for _ in 0..300 {
        let before = s.body().feet;
        let seq = s.data().climb.move_seq;
        s.run(1.0 / 60.0 + 1e-4);
        let c = &s.data().climb;
        // the frame a grid move with a root change starts
        if c.move_seq != seq && c.grid_move && c.moving.is_some_and(|m| m.from.distance(m.to) > 0.01) {
            let m = c.moving.unwrap();
            assert!(m.t > 0.0, "advanced in the starting frame: {m:?}");
            assert!((s.body().feet - before).length() > 1e-4, "the root moved in the starting frame: {before} -> {}", s.body().feet);
            return;
        }
    }
    panic!("no grid move: {:?}", s.data().climb.last_action);
}

/// RE/24 CLIMB-7: the jump off the climb plays the rebound (0x011FFC1A) in place first, then jumps facing away.
#[test]
fn the_jump_off_the_climb_plays_the_rebound_first() {
    use crate::player::ledge_moves::REBOUND;
    let mut s = climbing_the_tower();
    let at = s.body().feet;
    s.pad(Vec3::ZERO, 0.0, true, false);
    s.press_legs();
    assert!(s.run_until(0.2, |s| s.data().climb.rebound.is_some()), "no rebound");
    assert_eq!(s.data().climb.move_action, Some(REBOUND[0]));
    s.run(0.05);
    assert_eq!(s.loco().current, ActorContextId::Climb, "still on the wall during the rebound");
    assert!((s.body().feet - at).length() < 1e-3, "the root holds during the rebound");
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::InAir), "never jumped");
    assert!(s.body().forward().z < -0.9, "facing away from the wall: {:?}", s.body().forward());
}

/// RE/24 CLIMB-6: the empty hand releases with the lost-grip fall (0x1120E17B, the hold the root + 1.2 m up), also
/// mid-move during a grid move (+4620).
#[test]
fn the_climb_release_is_the_lost_grip_fall_and_works_mid_move() {
    use crate::player::climb::LOST_GRIP;
    let mut s = climbing_the_tower();
    s.pad(Vec3::Z, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().climb.moving.is_some() && s.data().climb.grid_move), "no grid move");
    let root = s.body().feet;
    s.press_hand();
    s.run(1.0 / 60.0 + 1e-4);
    assert_eq!(s.loco().current, ActorContextId::InAir, "released mid-move");
    assert!(s.data().air.fall_action.is_some_and(|a| a.id == LOST_GRIP), "the lost-grip fall action");
    let hold = s.data().air.release_hold.expect("a released hold");
    assert!((hold.y - (root.y + 1.2)).abs() < 0.1, "the hold 1.2 m above the root: {hold:?} root {root:?}");
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
        s.pad(Vec3::Z, 1.0, true, false);
        s.press_hand();
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
    walk_to_wall(&mut s, Vec3::Z, 2.0);
    s.pad(Vec3::Z, 1.0, true, false);
    s.press_hand(); // climbing from the ground: the empty-hand press (event 50, RE/02 4.7)
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
    walk_to_wall(&mut s, Vec3::Z, 2.0);
    s.pad(Vec3::Z, 1.0, true, false);
    s.press_hand(); // climbing from the ground: the empty-hand press (event 50, RE/02 4.7)
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Climb));
    s.pad(Vec3::Z, 1.0, true, false);
    assert!(s.run_until(10.0, |s| s.loco().current == ActorContextId::Ledge), "never jumped: {:?} {:?}", s.data().climb.last_action, s.body().feet);
    assert_eq!(s.data().climb.last_action, "jump up");
    let mv = s.data().ledge.mv.expect("the jump");
    // each hand keeps its own hit, ∓0.2 m in the search frame (+3280 / +3360, RE/24 CLIMB-9)
    assert!((mv.hand_l - mv.hand_r).length() > 0.3, "the two hits, not one point: {:?} {:?}", mv.hand_l, mv.hand_r);
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
fn climbing_under_wall_k_bay_never_puts_the_body_inside_it() {
    // bug 1791500965: wall K (face z -10.5) with the bay 0.9 m out above 3.3 m. Climbing straight up, the hands reached
    // the bay's bands while a foot stayed on the wall, and the body ended 0.24 m inside the bay. The game checks the
    // body at every grid move's new pose (IsGridMoveValid 0xDECD70 -> Human__ClimbBodyBoxesFree 0xB2DF90), so that
    // move is refused; with the holds below run out the climb jumps up to the bay (TryBackEject 0xDF2F50).
    let (collision, _) = crate::level::geometry();
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -11.4), FACE_PZ);
    walk_to_wall(&mut s, Vec3::Z, 2.0);
    s.pad(Vec3::Z, 1.0, true, false);
    s.press_hand(); // climbing from the ground: the empty-hand press (event 50, RE/02 4.7)
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Climb), "never climbed: {:?}", s.body().feet);
    // a light push (<= 0.5): SHORT moves only, one side at a time
    s.pad(Vec3::Z, 0.45, true, false);
    let worst = std::cell::Cell::new(None);
    let left = s.run_until(15.0, |s| {
        let d = &s.data().climb;
        if s.loco().current == ActorContextId::Climb && d.moving.is_none() && !climb::body_boxes_free(&collision, &d.frame, 0) {
            if worst.get().is_none() { worst.set(Some((d.last_action, d.frame.root))); }
        }
        s.loco().current == ActorContextId::Ledge
    });
    assert_eq!(worst.get(), None, "a climb pose with the body inside the geometry");
    assert!(left, "never left wall K: {:?} {:?}", s.data().climb.last_action, s.body().feet);
    assert!(["jump up", "overhang"].contains(&s.data().climb.last_action), "{}", s.data().climb.last_action);
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(4.0, |s| s.data().ledge.mv.is_none()), "move never ended");
    let l = &s.data().ledge;
    assert!(l.hand_l.z < -11.3 && l.hand_r.z < -11.3, "hands on the bay: {:?} {:?}", l.hand_l, l.hand_r);
    assert_eq!(l.hang_type, ledge::LedgeHangType::Free);
}

/// Climb wall M from the ground at `x` and push up (light, SHORT moves) until both feet stand on the 2.4 m band.
/// On the climb wall (face z −10.58, normal −Z) at `x`, the feet on the band at `feet_y`, the hands 1.2 m above
/// (pose 0). The tests start there directly: with the LONG table always first (RE/24 CLIMB-2) a climber on evenly
/// banded walls settles level at 1.8 / 3.0 m, not 2.4 m.
fn climb_at_feet(x: f32, feet_y: f32) -> Sim {
    climb_at(x, feet_y, -10.58)
}

/// As `climb_at_feet` on a wall whose holds lie at `face_z` (normal −Z).
fn climb_at(x: f32, feet_y: f32, face_z: f32) -> Sim {
    use crate::player::climb::{ClimbEntry, ClimbEntryType};
    use crate::player::{switch_context, TransitionSetup};
    let from = Vec3::new(x, feet_y - 0.1, face_z - 0.42);
    let mut s = Sim::new(from, FACE_PZ);
    {
        let player = s.player;
        let w = s.app.world_mut();
        let mut q = w.query::<(&mut Locomotion, &mut HumanDataBundle)>();
        let (mut loco, mut data) = q.get_mut(w, player).unwrap();
        let (hands, feet) = (Vec3::new(x, feet_y + 1.2, face_z), Vec3::new(x, feet_y, face_z));
        let e = ClimbEntry { entry_type: ClimbEntryType::Default, pose_2m: None, hand_l: hands, hand_r: hands, foot_l: feet, foot_r: feet, normal: Vec3::NEG_Z, from_feet: from, foot_right: false, action: None };
        switch_context(&mut loco, &mut data, TransitionSetup::ToClimb(e));
    }
    s.run(0.5);
    assert_eq!(s.loco().current, ActorContextId::Climb, "not climbing at {x}");
    s
}

fn climb_wall_m_to_feet_at(x: f32) -> Sim {
    let mut s = climb_at_feet(x, 2.4);
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
    // the standing straight jump: walk up to the wall, then high profile + Legs, still, facing it
    let mut s = Sim::new(Vec3::new(12.0, 0.0, 40.6), FACE_PZ);
    walk_to_wall(&mut s, Vec3::Z, 2.0);
    s.pad(Vec3::Z, 0.0, true, true);
    s.run(0.35);
    s.pad(Vec3::Z, 0.0, true, false);
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

#[test]
fn shimmy_reaches_out_then_brings_the_hands_together() {
    // ProbeLateral 0xDD9640 row 3: the reach takes the farthest point of the edge within w = min(d − 0.4, 1 − |hands|)
    // beyond 0.1 m (the piece trimmed 0.1 m); the closing step's box ends 0.05 m short of the lead hand and the trimmed
    // piece 0.15 m short, so the hands come together. On a long edge they alternate 1.0 / 0.15 m apart.
    let mut s = hang_on_jump_up_wall();
    s.pad(Vec3::NEG_X, 1.0, false, false); // away from the pillar
    let spacing = |s: &Sim| (s.data().ledge.hand_l - s.data().ledge.hand_r).length();
    assert!(s.run_until(3.0, |s| spacing(s) > 0.95), "the reach opens the hands to 1 m: {}", spacing(&s));
    assert!(s.run_until(2.0, |s| spacing(s) < 0.3), "the closing step brings them together: {}", spacing(&s));
    assert!((spacing(&s) - 0.15).abs() < 0.02, "{}", spacing(&s));
    assert!(s.run_until(2.0, |s| spacing(s) > 0.95), "and the next reach: {}", spacing(&s));
}

/// Free-hang balcony slab: top 2.9 m, x -1..5, front edge z 35.4 (normal -Z), nothing below.
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
    assert!((f.y - 0.5).abs() < 0.05, "free hang root 2.4 m below the hands: {f:?}");
    // the player's right (facing +Z) is -X; the slab ends at x = -1
    s.pad(Vec3::NEG_X, 1.0, false, false);
    s.run(10.0);
    let min_x = s.data().ledge.hand_l.x.min(s.data().ledge.hand_r.x);
    assert_eq!(s.loco().current, ActorContextId::Ledge);
    assert!(min_x >= -1.01 && min_x < 0.2, "hands should stop at the slab end: {min_x}");
    // at the end the free hang turns the outer corner onto the slab's -X side (0xDD3BB0, `corner_090_out`)
    assert!(s.data().ledge.normal.dot(Vec3::NEG_X) > 0.99, "no outer corner: normal {:?}", s.data().ledge.normal);
    // the corner keeps the hang type (0xDD3BB0 never writes +104, RE/24 LEDGE-11)
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Free);
}

#[test]
fn let_go_from_a_hang_falls_and_lands() {
    // the empty-hand button lets go (interpreter ledge state 0xEEBFD0: +0x1129 buffer -> event 4)
    let mut s = hang_on_balcony();
    s.press_hand();
    assert!(s.run_until(0.5, |s| s.loco().current == ActorContextId::InAir), "did not let go");
    assert_eq!(s.data().air.fall_origin, air::FallOrigin::HangFree);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground));
    assert_eq!(s.ground().last_landing.map(|l| l.kind), Some(LandingType::Safe));
}

#[test]
fn the_empty_hand_buffer_lasts_one_frame() {
    // +0x1129 is set on the press and cleared the next frame unless Legs was pressed within 0.3 s (0xEEE0F8 / 0xEEE125,
    // RE/24 INPUT-2): a press a few frames old no longer lets go
    let mut s = hang_on_balcony();
    s.app.world_mut().resource_mut::<PadInput>().hand_pressed_ago = 0.05;
    s.run(0.3);
    assert_eq!(s.loco().current, ActorContextId::Ledge, "a stale press let go");
}

#[test]
fn the_empty_hand_is_ignored_just_after_grabbing() {
    // entering the Ledge state clears the buffer and blocks it for 0.2 s (`sub_ED8610` 0xED86D6, ReadInput 0xEEE149;
    // RE/24 INPUT-3)
    use crate::player::ledge::{LedgeEntry, LedgeSubState};
    let (mid, n) = (Vec3::new(12.0, 2.6, 41.7), Vec3::NEG_Z);
    let feet = mid - Vec3::Y * 1.1 + n * 0.5;
    let mut s = Sim::new(feet, crate::player::heading_of(-n));
    force(&mut s, crate::player::TransitionSetup::ToLedge(LedgeEntry::at(mid, n, feet, LedgeSubState::Movement)));
    s.press_hand();
    s.run(1.0 / 60.0 + 1e-4);
    assert!(!s.app.world().resource::<PadInput>().hand_buffered(), "the press during the block is dropped");
    s.run(0.15);
    s.press_hand();
    s.run(1.0 / 60.0 + 1e-4);
    assert!(!s.app.world().resource::<PadInput>().hand_buffered(), "still blocked at 0.17 s");
    s.run(1.0);
    assert_eq!(s.loco().current, ActorContextId::Ledge);
    s.press_hand();
    assert!(s.run_until(0.3, |s| s.loco().current == ActorContextId::InAir), "a press after the block lets go");
}

#[test]
fn the_ledge_reads_the_stick_in_the_characters_frame() {
    // GoAssassinActionInterpreter__LedgeState 0xEEC356 reads the latched raw stick (+0x1310) against stick-up, in the
    // character's frame (RE/24 INPUT-4): with the camera off to the side, pushing the stick away from the camera is
    // still "up" → the pull-up (camera-relative it would be a shimmy)
    let mut s = hang_on_jump_up_wall();
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()), "reception never ended");
    s.cam_follow = false;
    let yaw = s.body().heading + std::f32::consts::FRAC_PI_2;
    s.app.world_mut().resource_mut::<CameraRig>().yaw = yaw;
    let away_from_camera = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    s.pad(away_from_camera, 1.0, false, false);
    let on_top = s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground);
    assert!(on_top && (s.body().feet.y - 2.6).abs() < 0.05, "no pull-up: {:?} ledge {}", s.body().feet, s.data().ledge.last_action);
}

#[test]
fn the_ledge_stick_is_mirrored_when_the_camera_faces_the_character() {
    // sub_EDB450: with the camera in front (dot < −0.1736) the stick's x is negated, so "stick right" still moves to the
    // screen's right: toward the camera's right, which is the character's left
    let mut s = hang_on_jump_up_wall();
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()), "reception never ended");
    s.cam_follow = false;
    let yaw = s.body().heading + std::f32::consts::PI;
    s.app.world_mut().resource_mut::<CameraRig>().yaw = yaw;
    let cam_right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
    let x0 = s.data().ledge.hand_l.dot(cam_right) + s.data().ledge.hand_r.dot(cam_right);
    s.pad(cam_right, 1.0, false, false);
    s.run(2.0);
    assert_eq!(s.loco().current, ActorContextId::Ledge);
    let x1 = s.data().ledge.hand_l.dot(cam_right) + s.data().ledge.hand_r.dot(cam_right);
    assert!(x1 > x0 + 0.3, "should shimmy toward the camera's right: {x0} → {x1}");
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
    let AirMode::Jump { duration, t_takeoff, .. } = air.mode else { panic!("not a jump: {:?}", air.mode) };
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

/// RE/24 CENSUS-2 / AIR-23: letting go of the high block's edge above a haystack (IHuman vt84) dives: the let-go plays,
/// then the faith dive steers into the haystack, through the HollywoodMode layer.
#[test]
fn letting_go_above_a_haystack_dives_into_it() {
    let mut s = hang_at(Vec3::new(33.0, 9.5, 23.6), Vec3::X);
    s.press_hand();
    assert!(s.run_until(0.3, |s| s.loco().current == ActorContextId::InAir), "no let-go");
    assert_eq!(s.data().air.target.map(|t| t.type_flags), Some(crate::player::jump_blend::TARGET_HAYSTACK), "the let-go record's haystack");
    assert!(s.data().air.dive_wait.is_some());
    assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::HayStack), "never dove in: {:?} {:?}", s.loco().current, s.body().feet);
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
    // both hand targets are the corner point (0xDD3C17..0xDD3CE1, RE/24 LEDGE-11)
    assert!((l.hand_l - l.hand_r).length() < 0.01, "both hands on the corner point: {:?} {:?}", l.hand_l, l.hand_r);
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
fn swing_back_catches_the_beam_behind_into_a_free_hang() {
    use crate::player::ledge_moves::{MoveKind, SWING_BACK};
    // `HumanLedge__TryWallJumpUp` 0xDD62A0: hanging on wall E's -Z face, stick up → the box 0.75 m out from the wall
    // (root + 2.25 m) finds the far side edge of the beam behind (z 48.8, top 3.2 m), facing the way the wall does
    let mut s = hang_at(Vec3::new(42.0, 2.6, 49.7), Vec3::NEG_Z);
    s.pad(Vec3::Z, 1.0, false, false); // up
    let hopped = s.run_until(2.0, |s| s.data().ledge.mv.is_some_and(|m| m.kind == MoveKind::SwingBack));
    assert!(hopped, "no swing back: {}", s.data().ledge.last_action);
    let mv = s.data().ledge.mv.unwrap();
    assert_eq!(mv.seq[0].map(|a| a.id), Some(SWING_BACK));
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()));
    let l = &s.data().ledge;
    assert!((l.hand_l.y - 3.2).abs() < 0.05 && (l.hand_l.z - 48.8).abs() < 0.05, "hands on the beam's far edge: {:?}", l.hand_l);
    assert!(l.normal.dot(Vec3::NEG_Z) > 0.99, "hanging facing the wall again: {:?}", l.normal);
    assert_eq!(l.hang_type, ledge::LedgeHangType::Free);
    assert!((s.body().feet.y - (3.2 - 2.4)).abs() < 0.05, "free-hang root: {:?}", s.body().feet);
}

/// Walk into a wall (facing +Z) with high profile + Legs from `feet`; return once the straight jump started.

/// Walk (high profile, no Legs) straight along `dir` until the body stops against something (or `max_s` runs out).
/// The grab probes reach 0.75 m, so the wall-start tests first walk up to the wall, as a player does.
fn walk_to_wall(s: &mut Sim, dir: Vec3, max_s: f32) {
    walk_to_wall_in(s, dir, max_s, true);
}

/// `high` = high profile, which hops onto obstacles under 1.3 m by itself (the free-run target jump 0xEE7F7A).
fn walk_to_wall_in(s: &mut Sim, dir: Vec3, max_s: f32, high: bool) {
    s.pad(dir, 0.6, high, false);
    let mut prev = s.body().feet;
    let mut still = 0;
    for _ in 0..(max_s * 60.0) as usize {
        s.run(1.0 / 60.0 + 1e-4);
        let f = s.body().feet;
        still = if (f - prev).with_y(0.0).length() < 0.002 { still + 1 } else { 0 };
        prev = f;
        if still >= 6 {
            break;
        }
    }
}

fn straight_jump_at(feet: Vec3) -> Sim {
    // walk up to the wall, then high profile + Legs standing still (walking in with Legs held starts the wall run,
    // 0xEE7E50)
    let mut s = Sim::new(feet, FACE_PZ);
    walk_to_wall_in(&mut s, Vec3::Z, 2.0, false);
    // standing still: hold Legs (the stationary free run, event 43), then let go: the release jumps at the hand
    // target (0xEE84CC)
    s.pad(Vec3::Z, 0.0, true, true);
    s.run(0.35);
    s.pad(Vec3::Z, 0.0, true, false);
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
    let j = s.data().air.target.and_then(|t| t.straight).unwrap_or_else(|| panic!("straight jump band: {:?} feet {:?}", s.data().air.target, s.body().feet));
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
    // both hand targets are the lead hand's target (0xDE1892 / 0xDE18B8, RE/24 LEDGE-9)
    let mv = s.data().ledge.mv.unwrap();
    assert!((mv.hand_l - mv.hand_r).length() < 1e-4, "both hands on one point: {:?} {:?}", mv.hand_l, mv.hand_r);
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
    // the empty hand at the edge (interpreter 0xEE8A33: pressed within 0.2 s)
    s.press_hand();
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
    // the stick let go during the start action: no pull-down, the end action steps back
    s.pad(Vec3::X, 0.0, false, false);
    assert!(s.run_until(1.0, |s| s.ground().ledge_stop.is_some_and(|l| l.ending)));
    assert_eq!(s.ground().oneshot.map(|o| o.blend.id), Some(LEDGE_STOP_END));
    let at_end = s.body().feet;
    assert!(at_end.x < edge - 0.03, "feet stayed behind the edge: {at_end:?}");
    assert!(s.run_until(1.5, |s| s.ground().ledge_stop.is_none()));
    // the end action steps back (~0.5 m), still on the roof, no fall
    assert!(s.body().feet.x < at_end.x - 0.3, "stepped back: {:?}", s.body().feet);
    assert_eq!(s.loco().current, ActorContextId::Ground);
    // walking in again stops again; holding on into the edge then hangs from it
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.ground().ledge_stop.is_some()), "stops again");
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Ledge), "held into the edge: the pull-down");
}

#[test]
fn pull_down_from_the_ledge_stop_uses_the_edge_stop_orientation() {
    use crate::player::ledge_moves::{MoveKind, PULLDOWN_ORIENT};
    let mut s = Sim::new(Vec3::new(-11.0, 6.0, 4.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(3.0, |s| s.ground().ledge_stop.is_some()));
    // still pushing into the edge when the start action is done (0xEE8977 / guard 0xD9D580): the pull-down
    assert!(s.run_until(0.5, |s| s.loco().current == ActorContextId::Ledge), "no pull-down: {:?}", s.loco().current);
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
    // the stick released: the low wait (event 1, RE/24 NARROW-19)
    assert_eq!(s.data().hay.action.map(|a| a.id), Some(crate::player::hay::HAYSTACK_LOW_WAIT));
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
fn wall_run_at_a_3_m_edge_ends_in_a_wall_hang() {
    // bug 1791478709: the free-hang probe B caught the wall's own 3 m top, so the grab and the pull-up played as a
    // free hang 0.5 m off the wall. The game's probe B box only reaches overhanging edges (0xE37A07); a 3 m wall top
    // is probe D's wall hang, as read live (root 0.5 m out, 1.1 m under the hands).
    let mut s = Sim::new(Vec3::new(1.56, 0.0, 5.5), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, false);
    assert!(s.run_until(2.0, |s| s.body().feet.z > 7.5));
    s.pad(Vec3::Z, 1.0, true, true);
    s.press_legs();
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Walling), "no wall run: {:?} at {:?}", s.loco().current, s.body().feet);
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ledge), "never hung: {:?}", s.loco().current);
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()), "grab never ended");
    let l = &s.data().ledge;
    assert_eq!(l.hang_type, ledge::LedgeHangType::Wall);
    assert!((l.hand_l.y - 3.0).abs() < 0.05 && l.hand_l.x > l.hand_r.x, "hands on the edge, left on the left: {:?} {:?}", l.hand_l, l.hand_r);
    let f = s.body().feet;
    assert!((f.y - 1.9).abs() < 0.05 && (l.hand_l.z - f.z - 0.5).abs() < 0.05, "wall-hang root: {f:?}");
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

/// RE/24 WALL-10 / AIR-5: the push-off turns away from the wall and takes off with the rebound takeoff (jump kind 2);
/// pushed off to the side, its side groups turn the character toward the push during the takeoff (no snap at takeoff).
#[test]
fn the_wall_rebound_takes_off_with_the_rebound_clips_and_turns_with_them() {
    use crate::player::walling::WallingSubState;
    let mut s = wall_run_at(82.0);
    assert!(s.run_until(1.0, |s| s.data().walling.sub_state == WallingSubState::Vertical), "no vertical step");
    // facing the wall (+Z); push off back and to the right of the facing-away direction (-Z), 45° toward -X
    let push = (Vec3::NEG_Z + Vec3::NEG_X).normalize();
    s.pad(push, 1.0, true, true);
    s.press_legs();
    assert!(s.run_until(0.1, |s| s.loco().current == ActorContextId::InAir), "no rebound");
    assert!(crate::player::jump_blend::TAKEOFF_REBOUND.contains(&s.data().air.takeoff.unwrap().id), "takeoff {:#x}", s.data().air.takeoff.unwrap().id);
    // the first frame faces away from the wall, not the push
    let away = crate::player::heading_of(Vec3::NEG_Z);
    let d = |h: f32| crate::player::ground::heading_delta(h, away).abs();
    assert!(d(s.body().heading) < 0.3, "turned away from the wall at takeoff: {}", s.body().heading);
    let toward = |h: f32| crate::player::ground::heading_delta(h, crate::player::heading_of(push)).abs();
    let h0 = s.body().heading;
    s.run(0.3);
    assert!(d(s.body().heading) > 0.4 && toward(s.body().heading) < toward(h0) - 0.3, "the takeoff turned toward the push: {} -> {}", h0, s.body().heading);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground));
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

/// RE/24 WALL-13: the push direction reads the stick's angle to the facing as last written while the stick was past 0.35
/// (+0x1244). Released after pointing sideways (90°), Legs starts no rebound (the zero stick); released after pointing at
/// the wall, it rebounds straight back.
#[test]
fn a_released_stick_rebounds_only_if_it_last_pointed_at_the_wall() {
    use crate::player::walling::WallingSubState;
    let mut s = wall_run_at(82.0);
    assert!(s.run_until(1.0, |s| s.data().walling.sub_state == WallingSubState::Vertical), "no vertical step");
    s.pad(Vec3::X, 1.0, true, true);
    s.run(0.05);
    s.pad(Vec3::ZERO, 0.0, true, true);
    s.press_legs();
    s.run(0.1);
    assert_eq!(s.loco().current, ActorContextId::Walling, "the stale sideways angle starts no rebound");
    let mut s = wall_run_at(82.0);
    assert!(s.run_until(1.0, |s| s.data().walling.sub_state == WallingSubState::Vertical), "no vertical step");
    s.pad(Vec3::Z, 1.0, true, true);
    s.run(0.05);
    s.pad(Vec3::ZERO, 0.0, true, true);
    s.press_legs();
    assert!(s.run_until(0.1, |s| s.loco().current == ActorContextId::InAir), "the stale angle at the wall rebounds straight back");
}

/// RE/24 NARROW-1: the beam walk's speed ramps toward the profile's band at 2 m/s² (the jog weight follows it); let go
/// RE/24 NARROW-3: event 72 has no axis angle test: walking at the beam's start 45° off its axis still mounts it, at the
/// closest point + 0.5 m along the beam.
#[test]
fn walking_at_a_beam_at_45_degrees_mounts_it() {
    let d = Vec3::new(1.0, 0.0, 1.0).normalize();
    let mut s = Sim::new(Vec3::new(70.6, 4.0, 68.8), crate::player::heading_of(d));
    s.pad(d, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::NarrowObject), "never mounted: {:?} {:?}", s.loco().current, s.body().feet);
    let n = &s.data().narrow;
    assert!(n.toward_p1 && (n.p0.z - 70.0).abs() < 0.01, "on the beam, heading along it: {:?}", n.p0);
}

/// while jogging it plays the jog stop, while walking it settles into the wait.
#[test]
fn the_beam_walk_ramps_its_speed_and_stops_by_it() {
    use crate::player::narrow::{beam_bands, BeamState};
    let b = beam_bands();
    assert!(b[0] > 1.0 && b[4] > b[0] + 1.0, "bands from the walk clips: {b:?}");
    // on in low profile (the mount seeds band 0, band 4 when running), then high profile ramps up
    let mut s = Sim::new(Vec3::new(71.0, 4.0, 70.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::Walk), "never walked: {:?}", s.data().narrow.state);
    s.pad(Vec3::X, 1.0, true, false);
    s.run(1.0 / 60.0 + 1e-4);
    let v0 = s.data().narrow.speed;
    s.run(0.25);
    let v1 = s.data().narrow.speed;
    assert!(v1 > v0 && v1 - v0 <= 2.0 * 0.26 + 1e-3, "ramps up at 2 m/s²: {v0} -> {v1}");
    assert!(s.run_until(2.0, |s| s.data().narrow.speed >= b[2] - 1e-3), "reaches the jog band");
    s.pad(Vec3::X, 0.0, true, false);
    s.run(1.0 / 60.0 + 1e-4);
    s.run(1.0 / 60.0 + 1e-4);
    assert_eq!(s.data().narrow.state, BeamState::Stop, "let go at jog speed: the jog stop");

    let mut s = Sim::new(Vec3::new(71.0, 4.0, 70.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::Walk), "never walked");
    s.run(0.5);
    assert!((s.data().narrow.speed - b[1]).abs() < 1e-3, "low profile holds the walk band: {}", s.data().narrow.speed);
    s.pad(Vec3::X, 0.0, false, false);
    s.run(1.0 / 60.0 + 1e-4);
    s.run(1.0 / 60.0 + 1e-4);
    assert_eq!(s.data().narrow.state, BeamState::Wait, "let go at walk speed: the wait");
}

#[test]
fn walk_across_a_beam_onto_the_far_platform() {
    use crate::player::narrow::{BeamState, BEAM_WALK};
    // platform A x 68..72 (top 4 m), beam x 72..78 at 4 m, platform B x 78..82
    let mut s = Sim::new(Vec3::new(71.0, 4.0, 70.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::NarrowObject), "never mounted: {:?} {:?}", s.loco().current, s.body().feet);
    // the ground mount (mode 1) plays the beam walk from its first frame (RE/24 NARROW-2)
    assert_eq!(s.data().narrow.state, BeamState::Entry);
    assert_eq!(s.data().narrow.action.map(|a| a.id), Some(BEAM_WALK), "mode 1 plays the beam walk");
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

/// RE/24 AIR-4: a jump at a target 9.5 m below goes through the over-drop: the flight aims 5 m down, then the tail
/// steers onto the true target (the game's m / frame integration) and the landing ends it there.
#[test]
fn the_over_drop_tail_steers_onto_the_true_target() {
    use crate::player::air::InAirEntry;
    use crate::player::targets::JumpTarget;
    let mut s = Sim::new(Vec3::new(30.0, 9.5, 23.5), 0.0);
    s.run(0.2);
    let target = JumpTarget { position: Vec3::new(30.0, 0.0, 17.0), type_flags: 1, hang: None, straight: None, pass: None, ladder: None };
    let from = s.body().feet;
    force(&mut s, crate::player::TransitionSetup::ToInAir(InAirEntry::JumpToTarget { from, target, speed_param: 0.5, foot_left: true }));
    assert!(s.run_until(1.5, |s| matches!(s.data().air.mode, crate::player::air::AirMode::Fall { steer_to: Some(_) })), "no tail: {:?}", s.data().air.mode);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground), "never landed: {:?}", s.body().feet);
    let f = s.body().feet;
    assert!(f.y.abs() < 0.05 && Vec2::new(f.x - 30.0, f.z - 17.0).length() < 0.6, "landed on the target: {f:?}");
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
    // on its edge: the free-step arrival ends on the candidate (RE/24 AIR-2)
    assert!(f.x > 81.95 && (f.y - 3.0).abs() < 0.05, "on platform Q: {f:?}");
}

#[test]
fn pilotis_impulsion_and_jump_on_the_spot() {
    use crate::player::narrow::{BeamState, PilotisEntry, PilotisEntryType};
    let top = Vec3::new(74.5, 3.0, 80.0);
    let mut s = Sim::new(top, -std::f32::consts::FRAC_PI_2);
    force(&mut s, crate::player::TransitionSetup::ToPilotis(PilotisEntry { top, from: top, facing: Vec3::X, kind: PilotisEntryType::FromInAir, foot: 0 }));
    assert!(s.run_until(1.5, |s| s.data().narrow.state == BeamState::PilotisWait));
    // hold Legs: the impulsion; let go after 0.2 s: the jump on the spot (RE/24 NARROW-10)
    s.pad(Vec3::ZERO, 0.0, true, true);
    s.press_legs();
    assert!(s.run_until(0.3, |s| s.data().narrow.state == BeamState::ImpulseIn), "no impulsion: {:?}", s.data().narrow.state);
    assert!(s.run_until(1.0, |s| s.data().narrow.state == BeamState::ImpulseWait));
    s.pad(Vec3::ZERO, 0.0, true, false);
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
fn running_jump_onto_a_beam_targets_it_and_mounts_on_arrival() {
    use crate::player::narrow::{BeamEntryMode, BeamState, NarrowKind};
    // platform B (x 78..82, top 4) → the free beam x 84.5..90.5 at z 70
    let mut s = Sim::new(Vec3::new(79.0, 4.0, 70.0), -std::f32::consts::FRAC_PI_2);
    // standing with the right foot ahead: the start (0xD98990) hands over to the left-foot locomotion item, and the
    // edge jump takes off from the left foot, whose flight meets the beam before the target ray clears
    s.app.world_mut().get_mut::<HumanDataBundle>(s.player).unwrap().ground.blend.foot = 1;
    s.pad(Vec3::X, 1.0, true, true);
    assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::NarrowObject), "never reached the beam: {:?} {:?}", s.loco().current, s.body().feet);
    let n = &s.data().narrow;
    assert_eq!(n.kind, NarrowKind::Beam);
    // the query offers the beam as a type 4 / sub-type 1 candidate and the scorer jumps at it as 0x10000 (RE/18
    // §1.4–1.6); the target ray (0xE044D0, 0.0005 m slack for 0x10000) clears, so the arrival mounts it
    assert_eq!(s.data().air.target_flags, 0x10000);
    assert_eq!(n.entry_mode, BeamEntryMode::Straight);
    assert!(n.toward_p1);
    // with the stick held, mode 2 goes straight into the walk (RE/24 NARROW-6)
    assert_eq!(n.state, BeamState::Walk, "a moving straight arrival walks on");
    s.run(0.25); // 0xE0BB70: reception starts before the 0.2 s RootInterp finishes.
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
    s.pad(Vec3::ZERO, 0.0, true, true);
    s.press_legs();
    assert!(s.run_until(1.0, |s| s.data().narrow.state == BeamState::ImpulseWait), "no impulsion: {:?}", s.data().narrow.state);
    s.pad(Vec3::ZERO, 0.0, true, false);
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
/// RE/24 NARROW-5: a side free-step arrival (mode 3) plays `freestep_entry_*_tr_beam_air_90` facing across the beam and
/// settles into the 90° wait; with the stick along the beam it walks off that way instead.
#[test]
fn a_side_arrival_faces_across_the_beam_then_waits_or_walks_off() {
    use crate::player::narrow::{free_step_beam_entry, BeamEntryMode, BeamState, BEAM_AIR90_WALK, BEAM_SIDE_AIR90};
    for walk in [false, true] {
        let point = Vec3::new(87.0, 4.0, 70.0);
        // arriving across the beam (facing +Z): mode 3
        let mut s = Sim::new(point, FACE_PZ);
        let e = {
            let g = s.app.world().resource::<crate::guidance::GuidanceWorld>();
            let c = s.app.world().resource::<crate::collision::CollisionWorld>();
            free_step_beam_entry(point, Vec3::Z, None, 0, None, false, g, c).expect("the free beam")
        };
        assert_eq!(e.mode, BeamEntryMode::Side);
        assert_eq!(e.action.map(|a| a.id), Some(BEAM_SIDE_AIR90[0]));
        force(&mut s, crate::player::TransitionSetup::ToBeam(e));
        assert!(s.data().narrow.across.is_some_and(|a| a.dot(Vec3::Z) > 0.99), "facing across, toward the arrival's forward");
        if walk {
            s.pad(Vec3::X, 1.0, false, false);
            assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::Walk), "never walked off: {:?}", s.data().narrow.state);
            assert_eq!(s.data().narrow.action.map(|a| a.id), Some(BEAM_AIR90_WALK[0]), "+X is to the left of the across facing (+Z)");
            assert!(s.data().narrow.toward_p1);
        } else {
            assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::Wait90), "never settled: {:?}", s.data().narrow.state);
        }
    }
}

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
    // the empty hand held (RE/24 NARROW-23)
    s.hold_hand(true);
    assert!(s.run_until(0.2, |s| s.loco().current == ActorContextId::Ledge), "no pull-down: {:?} {:?}", s.loco().current, s.data().narrow.state);
    s.hold_hand(false);
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
    s.cam_follow = false;
    s.app.world_mut().resource_mut::<CameraRig>().yaw = std::f32::consts::FRAC_PI_2;
    force(&mut s, crate::player::TransitionSetup::ToPilotis(PilotisEntry { top, from: top, facing: Vec3::X, kind: PilotisEntryType::FromInAir, foot: 0 }));
    assert!(s.run_until(1.5, |s| s.data().narrow.state == BeamState::PilotisWait));
    // the empty hand held (RE/24 NARROW-23)
    s.hold_hand(true);
    assert!(s.run_until(0.2, |s| s.loco().current == ActorContextId::Ledge), "no pull-down: {:?}", s.loco().current);
    s.hold_hand(false);
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
    // Legs held (`Pad__IsPressed`(0) 0xEEA765, RE/24 NARROW-23)
    s.pad(Vec3::X, 1.0, true, true);
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
        .retain(|e| e.subtype != crate::guidance::GuidanceSubType::Beam
            && !(e.p0.min(e.p1).x < 90.6 && e.p0.max(e.p1).x > 84.4
                && (e.p0.y-4.0).abs()<0.01 && (e.p0.z-70.0).abs()<0.3));
    // Remove the source ledge contacts too: the runtime detector now derives beams from these.
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
    // sub_F7DB30: the obstacle's top edge 1.2 m above the beam lies in the report box (0.3 … 1.4 m up, 0.7 m ahead)
    let mut s = on_free_beam(87.5);
    s.app.world_mut().resource_mut::<crate::collision::CollisionWorld>().boxes.push(crate::collision::Aabb3 {
        min:Vec3::new(88.0,0.0,69.5),max:Vec3::new(89.0,5.2,70.5) });
    s.app.world_mut().resource_mut::<crate::guidance::GuidanceWorld>().edges.push(crate::guidance::GuidanceEdge {
        p0:Vec3::new(88.0,5.2,69.5),p1:Vec3::new(88.0,5.2,70.5),n0:Vec3::Y,n1:Vec3::NEG_X,
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
    // Legs held: letting go would jump on the spot (RE/24 NARROW-10)
    s.pad(Vec3::Z, 1.0, true, true); s.run(0.3);
    assert_eq!(s.data().narrow.state, BeamState::ImpulseWait);
    s.pad(Vec3::NEG_X, 1.0, true, true); s.run(0.3);
    assert_eq!(s.data().narrow.state, BeamState::ImpulseWait);
    s.pad(Vec3::X, 1.0, true, true);
    assert!(s.run_until(0.2, |s| s.data().narrow.state == BeamState::Walk));
}

#[test]
fn beam_impulsion_resumes_after_timer_before_entry_clip_end() {
    use crate::player::narrow::BeamState;
    let mut s = on_free_beam(87.0);
    s.pad(Vec3::ZERO, 0.0, true, true); s.press_legs();
    assert!(s.run_until(0.3, |s| s.data().narrow.state == BeamState::ImpulseIn));
    let duration = s.data().narrow.action.unwrap().duration();
    assert!(duration > 0.25, "fixture must distinguish the timer from clip completion");
    s.pad(Vec3::X, 1.0, true, true);
    s.run(0.18);
    assert_eq!(s.data().narrow.state, BeamState::ImpulseIn, "forward stays gated before 0.2 s");
    assert!(s.run_until(0.05, |s| s.data().narrow.state == BeamState::Walk), "forward remains blocked after the deadline");
    let x = s.body().feet.x;
    s.run(0.1);
    assert!(s.body().feet.x > x, "resumed walk must move the root");
}

#[test]
fn beam_impulsion_jump_obeys_timer_during_entry_clip() {
    use crate::player::narrow::BeamState;
    let mut s = on_free_beam(87.0);
    s.pad(Vec3::ZERO, 0.0, true, true); s.press_legs();
    assert!(s.run_until(0.3, |s| s.data().narrow.state == BeamState::ImpulseIn));
    // let go at 0.1 s: the jump on the spot waits for the 0.2 s deadline (RE/24 NARROW-10)
    s.run(0.1); s.pad(Vec3::ZERO, 0.0, true, false); s.run(0.08);
    assert_eq!(s.data().narrow.state, BeamState::ImpulseIn, "jump stays gated before 0.2 s");
    assert!(s.run_until(0.05, |s| s.data().narrow.state == BeamState::JumpOnPlace), "the release must jump on the timer, before clip end");
}

#[test]
fn beam_impulsion_deadline_resets_on_reentry() {
    use crate::player::narrow::BeamState;
    let mut s = on_free_beam(87.0);
    for _ in 0..2 {
        s.pad(Vec3::ZERO, 0.0, true, true); s.press_legs();
        assert!(s.run_until(0.3, |s| s.data().narrow.state == BeamState::ImpulseIn));
        s.pad(Vec3::X, 1.0, true, true); s.run(0.18);
        assert_eq!(s.data().narrow.state, BeamState::ImpulseIn);
        assert!(s.run_until(0.05, |s| s.data().narrow.state == BeamState::Walk));
        s.pad(Vec3::ZERO, 0.0, false, false);
        assert!(s.run_until(1.0, |s| s.data().narrow.state == BeamState::Wait));
    }
}

#[test]
fn beam_jump_release_waits_until_the_item_has_positive_leftover_time() {
    use crate::player::narrow::BeamState;
    let mut s = on_free_beam(87.0);
    s.pad(Vec3::ZERO, 0.0, true, true); s.press_legs();
    assert!(s.run_until(1.0, |s| s.data().narrow.state == BeamState::ImpulseWait));
    s.pad(Vec3::ZERO, 0.0, true, false);
    assert!(s.run_until(0.1, |s| s.data().narrow.state == BeamState::JumpOnPlace));
    let duration = s.data().narrow.action.unwrap().duration();
    let dt = s.app.world().resource::<Time>().delta_secs();
    s.app.world_mut().get_mut::<HumanDataBundle>(s.player).unwrap().narrow.t = duration - dt;
    s.run(1.0 / 60.0 + 1e-4);
    assert_eq!(s.data().narrow.t, duration, "fixture lands exactly on the completion boundary");
    assert_eq!(s.data().narrow.state, BeamState::JumpOnPlace);
    assert_eq!(s.loco().current, ActorContextId::NarrowObject);
    s.run(1.0 / 60.0 + 1e-4);
    assert_eq!(s.loco().current, ActorContextId::InAir, "release on positive leftover time");
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

/// RE/24 NARROW-13: standing at a beam end with room beyond, the stick steps off at once (Idle → step-off state), no
/// walk start first.
#[test]
fn standing_at_a_beam_end_steps_off_without_walking() {
    use crate::player::narrow::{BeamEntry, BeamEntryMode, BeamState};
    let point = Vec3::new(77.86, 4.0, 70.0);
    let mut s = Sim::new(point, crate::player::heading_of(Vec3::X));
    force(&mut s, crate::player::TransitionSetup::ToBeam(BeamEntry { p0: Vec3::new(72.0,4.0,70.0), p1: Vec3::new(78.0,4.0,70.0),
        point, from: point, toward_p1: true, mode: BeamEntryMode::Straight, foot: 0, action: None, facing: Vec3::X }));
    assert!(s.run_until(2.0, |s| s.data().narrow.state == BeamState::Wait), "never waited: {:?}", s.data().narrow.state);
    s.pad(Vec3::X, 1.0, false, false);
    let mut walked = false;
    for _ in 0..60 {
        s.run(1.0 / 60.0 + 1e-4);
        walked |= matches!(s.data().narrow.state, BeamState::Start | BeamState::Walk);
        if s.data().narrow.state == BeamState::StepOff {
            assert!(!walked, "no walk start before the step off");
            return;
        }
    }
    panic!("never stepped off: {:?}", s.data().narrow.state);
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

/// The lean (ObstacleCollision) starts in high profile with the stick pushed into a wall at least 0.7 m high (owner's
/// live check 2026-10-10, bug-1791665811; PORT sender); low profile and a 0.6 m box only block the run.
#[test]
fn pushing_into_a_wall_in_high_profile_puts_the_hands_on_it() {
    use crate::player::collide::CollideKind;
    let leans = |from: Vec3, dir: Vec3, high: bool| {
        let mut s = Sim::new(from, 0.0);
        s.pad(dir, 1.0, high, false);
        let leaned = s.run_until(3.0, |s| s.ground().collide.is_some());
        assert_eq!(s.loco().current, ActorContextId::Ground);
        leaned.then(|| s.ground().collide.unwrap().kind)
    };
    // the 2.4 m wall (face z -7.7), straight on and at 30 deg: the hands (the 1.1 m wall is a free-run hop-up target
    // in high profile, 0xEE7F7A, so it is climbed, not leaned on)
    assert_eq!(leans(Vec3::new(-3.0, 0.0, -4.0), Vec3::NEG_Z, true), Some(CollideKind::Hand));
    assert_eq!(leans(Vec3::new(-3.0, 0.0, -4.0), Vec3::new(0.5, 0.0, -0.866), true), Some(CollideKind::Hand));
    // the 0.6 m box: no lean (the foot type is not observed); low profile: no lean
    assert_eq!(leans(Vec3::new(9.0, 0.0, 3.0), Vec3::NEG_Z, true), None);
    assert_eq!(leans(Vec3::new(-3.0, 0.0, -4.0), Vec3::NEG_Z, false), None);
}

/// bug-1791667261: holding the stick into the wall and tapping sideways wiggles on it: W + D pushes away to the right
/// (the slid stick runs along the wall), W alone goes back onto the wall (event 42 again, along the stick, during the
/// push-away's unlocked transition), W + A pushes away to the left, and so on, without getting stuck.
#[test]
fn holding_into_the_wall_and_tapping_sideways_wiggles_on_it() {
    use crate::player::collide::{LEAN_EXIT, CollidePhase};
    let mut s = Sim::new(Vec3::new(7.46, 0.0, 26.9), 0.0);
    let into = Vec3::NEG_Z;
    s.pad(into, 1.0, true, false);
    assert!(s.run_until(2.0, |s| s.ground().collide.is_some_and(|c| c.phase != CollidePhase::Entry(0))), "no lean");
    for (k, side) in [Vec3::X, Vec3::NEG_X, Vec3::X, Vec3::NEG_X].into_iter().enumerate() {
        s.pad(into + side, 1.0, true, false);
        assert!(s.run_until(1.0, |s| s.ground().collide.is_none()), "round {k}: never pushed away");
        let exit = s.ground().oneshot.map(|o| o.blend.id);
        assert!(exit.is_some_and(|id| LEAN_EXIT.contains(&id)), "round {k}: not a push-away: {exit:#x?}");
        s.run(0.15);
        s.pad(into, 1.0, true, false);
        assert!(s.run_until(2.0, |s| s.ground().collide.is_some()), "round {k}: stuck off the wall at {:?} heading {} owner {:?} oneshot {:#x?}", s.body().feet, s.body().heading, s.ground().owner, s.ground().oneshot.map(|o| o.blend.id));
        assert!(s.run_until(1.0, |s| s.body().forward().dot(into) > 0.95), "round {k}: not facing the wall");
        assert!(s.body().feet.z > 26.7 && s.body().feet.z < 27.1, "round {k}: at the wall: {:?}", s.body().feet);
    }
    // the report's case: pushed away, then standing still along the wall; pushing into it puts the hands back on it
    // (toward the middle of the 5 m wall, x 4.5..9.5)
    let side = if s.body().feet.x > 7.0 { Vec3::NEG_X } else { Vec3::X };
    s.pad(into + side, 1.0, true, false);
    assert!(s.run_until(1.0, |s| s.ground().collide.is_none()));
    s.pad(Vec3::ZERO, 0.0, true, false);
    s.run(2.0);
    assert!(s.body().forward().dot(into).abs() < 0.5, "stands along the wall: {:?}", s.body().forward());
    s.pad(into, 1.0, true, false);
    assert!(s.run_until(1.0, |s| s.ground().collide.is_some()), "stuck standing along the wall: {:?}", s.body().feet);
}

/// bug-1791665811: snug against the 3.2 m block (back face z 26.5), high profile with the stick into it, Legs starts
/// the wall run: the game's input test reads the stick, not the wanted speed the wall slide zeroed (0xEE7E73).
#[test]
fn snug_against_a_wall_legs_starts_the_wall_climb() {
    let mut s = Sim::new(Vec3::new(7.46, 0.0, 26.9), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(1.0);
    assert_eq!(s.loco().current, ActorContextId::Ground);
    // the hands are on the wall (the lean), and the wall run starts from it (event 49 in the lean, RE/24 WALL-19)
    assert!(s.ground().collide.is_some_and(|c| c.phase == crate::player::collide::CollidePhase::Wait), "not leaning: {:?}", s.ground().collide.map(|c| c.phase));
    s.press_legs();
    assert!(s.run_until(0.5, |s| s.loco().current == ActorContextId::Walling), "no wall climb: {:?} collide {:?}", s.loco().current, s.ground().collide.map(|c| c.phase));
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
fn walking_into_a_roof_edge_halts_then_looks_down() {
    // roof A (x -3..3, top 3: a 3 m drop, under the ledge stop's 5 m): walking into its +X edge in low profile halts
    // at it (0xEE7C31); held there for more than 0.25 s the interpreter sends event 119 (0xEE87DE–0xEE8844)
    let mut s = Sim::new(Vec3::new(-1.0, 3.0, 12.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(5.0, |s| s.ground().edge_halt_t > 0.0), "never halted: {:?}", s.body().feet);
    s.run(0.15);
    assert!(s.ground().look_down.is_none(), "not before 0.25 s");
    assert!(s.run_until(0.3, |s| s.ground().look_down.is_some()), "no look-down");
    let ld = s.ground().look_down.unwrap();
    let w = ld.action.weights();
    assert!(w[0] > 0.9, "front look-down: {w:?}");
    assert!(s.body().feet.x < 3.0 && s.loco().current == ActorContextId::Ground, "on the roof: {:?}", s.body().feet);
    // letting go keeps it (the player sends no end event); turning the stick off the edge walks away
    s.pad(Vec3::X, 0.0, false, false);
    s.run(0.5);
    assert!(s.ground().look_down.is_some(), "kept after letting go");
    s.pad(Vec3::NEG_X, 1.0, false, false);
    assert!(s.run_until(0.5, |s| s.ground().look_down.is_none()), "walking away ends it");
}

#[test]
fn the_empty_hand_does_not_pull_down_away_from_an_edge_and_legs_never_does() {
    // the Wait pull-down's sender is the empty hand (0xEE8A33); Legs at the edge is not a pull-down (the old stand-in)
    let mut s = Sim::new(Vec3::new(10.6, 3.5, 12.0), -std::f32::consts::FRAC_PI_2);
    s.run(0.2);
    s.press_legs();
    s.run(0.5);
    assert_eq!(s.loco().current, ActorContextId::Ground, "Legs: no pull-down");
    let mut s = Sim::new(Vec3::new(8.0, 3.5, 12.0), -std::f32::consts::FRAC_PI_2);
    s.run(0.2);
    s.press_hand();
    s.run(0.5);
    assert_eq!(s.loco().current, ActorContextId::Ground, "no edge in reach: no pull-down");
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

/// bug-1791664621: standing with the back against the 3 m wall (z 9) and the stick pushed toward it, the wall slide
/// zeroes the speed (0xEDD7C0 result 2) but the direction into the wall still reaches DestHeading (0xEE6FB0), so Idle
/// pivots round to face the wall (`Idle_CanPivot` 0xD84B10 has no move-request test).
#[test]
fn the_stick_into_a_wall_behind_turns_the_character_round() {
    let mut s = Sim::new(Vec3::new(0.0, 0.0, 8.6), 0.0);
    s.run(0.5);
    assert!(s.body().forward().z < -0.9, "starts facing away from the wall: {:?}", s.body().forward());
    s.pad(Vec3::Z, 1.0, false, false);
    assert!(s.run_until(3.0, |s| s.body().forward().z > 0.9), "never turned to the wall: heading {} owner {:?}", s.body().heading, s.data().ground.owner);
    assert!(s.body().feet.z > 8.4 && s.body().feet.z < 8.7, "stays at the wall: {:?}", s.body().feet);
}

/// The vertical hand step's second hand waits for the stick (RE/24 LEDGE-4, `ContinuePendingVerticalStep` 0xDCF3C0):
/// on part B's stone ledge (2.6 m, face x 35.92) with the second ledge 0.6 m above, the stick up moves one hand; released,
/// the hands stay split; pushed again (even sideways), the other hand follows.
#[test]
fn a_vertical_hand_step_waits_for_the_stick_for_the_second_hand() {
    let mut s = hang_at(Vec3::new(35.92, 2.6, 45.5), Vec3::NEG_X);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().ledge.last_action == "hand step up"), "no hand step: {}", s.data().ledge.last_action);
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().ledge.pending_vstep.is_some()), "first half never ended");
    s.run(1.0);
    let l = &s.data().ledge;
    assert!(l.pending_vstep.is_some() && (l.hand_l.y - l.hand_r.y).abs() > 0.5, "hands split while waiting: {:?} {:?}", l.hand_l, l.hand_r);
    // RE/24 LEDGE-5: the first vertical step after an entry moves the right hand (+1844 = 0), and clears +133
    assert!(l.hand_r.y > l.hand_l.y + 0.5 && l.last_hand == 1 && !l.alt_flag, "the right hand first: {:?} {:?}", l.hand_l, l.hand_r);
    // a lateral stick is taken as the pending direction
    s.pad(Vec3::NEG_Z, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().ledge.pending_vstep.is_none()), "second hand never moved");
    assert_eq!(s.data().ledge.last_action, "hand step up");
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().ledge.sub_state == crate::player::ledge::LedgeSubState::Movement), "second half never ended");
    let l = &s.data().ledge;
    assert!((l.hand_l.y - 3.2).abs() < 0.05 && (l.hand_r.y - 3.2).abs() < 0.05, "both hands on the upper ledge: {:?} {:?}", l.hand_l, l.hand_r);
    // each onto the row point nearest the hands' midpoint line at its turn (x_ref 0): they end nearly together (the second
    // search runs from the half-step's midpoint), the left one moved last
    assert!((l.hand_l - l.hand_r).length() < 0.15 && l.last_hand == 0, "hands nearly together: {:?} {:?}", l.hand_l, l.hand_r);
}

/// RE/24 LEDGE-26: the empty hand lets go in the middle of a shimmy step (event 4's guard 0xDCCDD0 refuses only a locked
/// item, and the step items are not locked); the port used to wait for the step's end.
#[test]
fn letting_go_works_in_the_middle_of_a_shimmy() {
    let mut s = hang_at(Vec3::new(35.92, 2.6, 45.5), Vec3::NEG_X);
    s.pad(Vec3::Z, 1.0, false, false); // sideways along the ledge
    assert!(s.run_until(2.0, |s| s.data().ledge.last_action.starts_with("shimmy ") && s.data().ledge.moving()), "no shimmy: {}", s.data().ledge.last_action);
    s.run(0.15);
    assert!(s.data().ledge.moving(), "still mid-step");
    s.press_hand();
    assert!(s.run_until(0.1, |s| s.loco().current == ActorContextId::InAir), "let go mid-step: {:?}", s.loco().current);
}

/// bug-1791711547: a free hang's vertical hand step lifts the body the whole step with the first hand (the root hangs
/// from the higher hand target, `ComputeRootFromHandTargets` 0xDD6730: max − 2.4), so the reaching hand gets to its
/// hold; the halfway root left it 0.4 m short. Wall K's bars (0.6 m apart from 3.6 m) under the bay.
#[test]
fn a_free_hang_hand_step_lifts_the_body_with_the_first_hand() {
    use crate::player::ledge::LedgeHangType;
    let mut s = hang_at(Vec3::new(-30.1, 3.6, -11.4), Vec3::NEG_Z);
    assert_eq!(s.data().ledge.hang_type, LedgeHangType::Free, "a free hang under the bay");
    let y0 = s.body().feet.y;
    // how far out from its hands' line the root hangs (the hands themselves move onto the bars' edge)
    let out = |s: &Sim| crate::player::ledge::root_out(s.body().feet, s.data().ledge.hand_l, s.data().ledge.hand_r, Vec3::NEG_Z);
    let out0 = out(&s);
    s.pad(Vec3::Z, 1.0, false, false); // up
    assert!(s.run_until(2.0, |s| s.data().ledge.last_action == "hand step up"), "no hand step: {}", s.data().ledge.last_action);
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().ledge.pending_vstep.is_some()), "first half never ended");
    s.run(0.5);
    let l = &s.data().ledge;
    let top = l.hand_l.y.max(l.hand_r.y);
    assert!((top - y0 - 2.4 - 0.6).abs() < 0.05, "one hand on the bar above: {:?} {:?}", l.hand_l, l.hand_r);
    assert!((s.body().feet.y - (top - 2.4)).abs() < 0.02, "the root hangs from the higher hand: {} vs {}", s.body().feet.y, top - 2.4);
    // bug-1791714696: the bar above has wall below it, but a hand step keeps the hang's distance from the wall (only a
    // hang-type switch changes it); it moved 0.5 m out and both hands fell short
    assert!((out(&s) - out0).abs() < 0.02, "stays as far out from the hands: {} vs {out0}", out(&s));
    s.pad(Vec3::Z, 1.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().ledge.pending_vstep.is_none()), "second hand never moved");
    s.pad(Vec3::ZERO, 0.0, false, false);
    s.run(1.0);
    assert!((out(&s) - out0).abs() < 0.02, "still as far out after the second hand: {} vs {out0}", out(&s));
}

// ---------------------------------------------------------------- swing bars (RE/03 §7.10)

/// The swing's held-button rules (RE/24 LEDGE-17 / CENSUS-3, the interpreter 0xEECCF9–0xEED11F): on a bar (a Pole edge),
/// releasing high profile + Legs stops the swing (event 6) into the free hang; holding them again from the still hang
/// starts it (event 5, 0xDD3750 → 0xDD2940).
#[test]
fn the_swing_stops_and_starts_with_high_profile_and_legs_held() {
    use crate::player::swing::SwingPhase;
    let mut s = Sim::new(Vec3::new(60.0, 1.2, 85.0), std::f32::consts::PI);
    s.pad(Vec3::Z, 1.0, true, false);
    s.run(0.35);
    s.press_legs();
    assert!(s.run_until(3.0, |s| s.data().ledge.swing.is_some()), "never swung");
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(2.0, |s| s.data().ledge.swing.is_some_and(|w| matches!(w.phase, SwingPhase::Cycle(_)))), "no cycle");
    // release: the stop on a down item, then the free hang
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(4.0, |s| s.data().ledge.swing.is_none() && s.loco().current == ActorContextId::Ledge), "never stopped: {:?}", s.data().ledge.swing.map(|w| w.phase));
    s.run(0.5);
    assert!(s.data().ledge.swing.is_none(), "stays still with the buttons released");
    // hold them again: swinging starts from the hang
    s.pad(Vec3::ZERO, 0.0, true, true);
    assert!(s.run_until(1.0, |s| s.data().ledge.swing.is_some_and(|w| matches!(w.phase, SwingPhase::Cycle(_)))), "event 5 never started the swing");
}

/// bug-1791661604: the far jump from the 3.5 m roof (x 5..11, z 9..15) at the 3.2 m block's -Z face (z 21.5) is a
/// free-hang jump (0x80: the edge lies in the third zone, which clears 0x40, 0xE1A81F), but the block is solid below
/// the edge, so the arrival finds foot holds (`sub_B15C30`) and plays the wall reception, not the swing (0xE093FC).
#[test]
fn a_free_hang_jump_at_a_wall_lands_feet_on_the_wall() {
    use crate::player::ledge::LedgeHangType;
    // the free run off the roof edge (high profile, Legs held), as recorded
    let mut s = Sim::new(Vec3::new(8.2, 3.5, 11.0), std::f32::consts::PI);
    s.pad(Vec3::Z, 1.0, true, true);
    s.press_legs();
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir), "no jump: {:?}", s.body().feet);
    assert_eq!(s.data().air.target.expect("a target").type_flags, crate::player::targets::TARGET_LEDGE_FREE);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ledge), "never caught: {:?} {:?}", s.loco().current, s.body().feet);
    assert!(s.data().ledge.swing.is_none(), "swings in front of a wall");
    let m = s.data().ledge.mv.expect("the wall reception");
    assert_eq!(m.seq[0].map(|a| a.id), Some(crate::player::ledge_moves::RECEPTION_WALL_OTHER));
    // bug-1791711526: the root goes onto the hang in 0.1666 s (0xE0949A -> 0xE02BA0), then holds while the reception
    // plays on; it no longer slides there over the whole action
    assert_eq!(m.root_time, Some(0.1666));
    s.run(0.2);
    let held = s.body().feet;
    assert!((held - m.to).length() < 1e-3, "on the hang after 0.17 s: {:?} vs {:?}", held, m.to);
    assert!(s.data().ledge.mv.is_some(), "the reception still plays");
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()), "reception never ended");
    assert_eq!(s.data().ledge.hang_type, LedgeHangType::Wall, "feet on the wall");
}

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
    // both hands on a pole (the bars are Pole edges) → the swing cycle; high profile and Legs held keep it going and
    // jump with the stick toward the facing (RE/24 LEDGE-17)
    assert!(s.data().ledge.swing.unwrap().on_pole, "the bar is a pole");
    s.pad(Vec3::Z, 1.0, true, true);
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
    // on its edge: the free-step arrival ends on the candidate (RE/24 AIR-2)
    assert!(s.body().feet.z > 99.95 && (s.body().feet.y - 1.2).abs() < 0.05, "on the far platform: {:?}", s.body().feet);
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
        // the stop waits for the down item to end, then plays its four items (RE/24 NARROW-16)
        assert!(s.run_until(6.0, |s| s.data().ledge.swing.is_none()), "the released stick stops the swing");
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
    // full stick: a light one starts into the slow walk (layout 7), which barely moves in 0.5 s
    s.pad(Vec3::X, 1.0, false, false);
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
    s.pad(Vec3::NEG_X,1.0,false,false);
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

/// RE/24 LADDER-4: a blocked attach box on the approach side turns onto the far side: the turn's two items, then the
/// wait facing the ladder from the other side.
#[test]
fn a_blocked_approach_side_turns_onto_the_far_side() {
    use crate::player::ladder::{attach_blocked, LadderEntry, LadderPhase, ATTACH_OUT, TURN};
    let (base, top) = (Vec3::new(50.0, 0.0, 63.5), Vec3::new(50.0, 5.0, 63.5));
    // a crate on the approach side (-Z) at the attach box's height
    let mut c = crate::collision::CollisionWorld::default();
    c.boxes.push(crate::collision::Aabb3 { min: Vec3::new(49.7, 0.8, 62.8), max: Vec3::new(50.3, 1.2, 63.2) });
    assert!(attach_blocked(base + Vec3::NEG_Z * ATTACH_OUT, Vec3::Z, &c));
    assert!(!attach_blocked(base + Vec3::NEG_Z * ATTACH_OUT, Vec3::Z, &crate::collision::CollisionWorld::default()));
    let from = Vec3::new(50.0, 0.0, 62.6);
    let mut s = Sim::new(from, FACE_PZ);
    force(&mut s, crate::player::TransitionSetup::ToLadder(LadderEntry { base, top, n: Vec3::Z, from, facing: Vec3::Z, back_turn: Some(true), ..Default::default() }));
    assert_eq!(s.data().ladder.action.map(|a| (a.id, a.item)), Some((TURN[0], 0)));
    assert!(s.run_until(3.0, |s| s.data().ladder.phase == Some(LadderPhase::Wait)), "never settled: {:?}", s.data().ladder.phase);
    assert!(s.body().forward().dot(Vec3::NEG_Z) > 0.99, "facing the ladder from the far side");
}

/// RE/24 LADDER-5: the ground entry attaches at the nearest rung to the root's height, 0.5 m higher in high profile.
#[test]
fn a_ground_entry_attaches_at_the_rung_and_higher_in_high_profile() {
    use crate::player::ladder::LadderPhase;
    for high in [false, true] {
        let mut s = Sim::new(Vec3::new(50.0, 0.0, 62.0), FACE_PZ);
        s.pad(Vec3::Z, 1.0, high, false);
        assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ladder), "no entry");
        assert_eq!(s.data().ladder.phase, Some(LadderPhase::EnterGround));
        let want = if high { 0.5 } else { 0.0 };
        assert!((s.data().ladder.height - want).abs() < 1e-4, "high {high}: {}", s.data().ladder.height);
    }
}

/// RE/24 LADDER-8 / LADDER-9: the ladder up the 1 m wall (a wall top behind its top: +908 & 0x20) ends with the low exit
/// even in high profile, taken as soon as ReachedTop holds.
#[test]
fn a_high_profile_ladder_onto_a_wall_top_takes_the_low_exit() {
    use crate::player::ladder::{LadderPhase, EXIT_TOP};
    let mut s = Sim::new(Vec3::new(50.0, 0.0, 62.0), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, false);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ladder), "no entry");
    assert!(s.run_until(10.0, |s| matches!(s.data().ladder.phase, Some(LadderPhase::ExitTop(_)))), "no exit: {:?}", s.data().ladder.phase);
    assert_eq!(s.data().ladder.thick_top, Some(true), "the wall top behind the ladder");
    assert!(EXIT_TOP[0].contains(&s.data().ladder.action.unwrap().id), "the low exit");
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground), "never stood on top");
}

/// RE/24 LADDER-7: after a ground entry the wait's column is the entry item's feet after (LEFTAHEAD → column l), not
/// the foot the entry started on.
#[test]
fn the_wait_after_a_ground_entry_follows_the_entry_items_feet() {
    use crate::player::ladder::LadderPhase;
    for foot in [0usize, 1] {
        let mut s = Sim::new(Vec3::new(50.0, 0.0, 62.0), FACE_PZ);
        s.app.world_mut().get_mut::<HumanDataBundle>(s.player).unwrap().ground.blend.foot = foot;
        s.pad(Vec3::Z, 1.0, false, false);
        assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ladder), "no entry");
        let entry = s.data().ladder.action.expect("the entry action");
        let after = crate::player::anim_gate::word(&entry).map(|w| ((w & 0xC) != 4) as usize).expect("the entry item's word");
        s.pad(Vec3::ZERO, 0.0, false, false);
        assert!(s.run_until(3.0, |s| s.data().ladder.phase == Some(LadderPhase::Wait)), "never waited");
        assert_eq!(s.data().ladder.foot, after, "entry {:#x} item {}", entry.id, entry.item);
    }
}

/// RE/24 LADDER-1 / LADDER-2: the ground entry is automatic: walking at the ladder 55° off its front mounts it once
/// within 0.6 m (no 45° front test); free running (high profile with Legs held) never takes it.
#[test]
fn walking_at_a_ladder_mounts_it_automatically_unless_free_running() {
    let d = Vec3::new(55f32.to_radians().sin(), 0.0, 55f32.to_radians().cos());
    for free_run in [false, true] {
        let mut s = Sim::new(Vec3::new(49.2, 0.0, 62.4), crate::player::heading_of(d));
        s.pad(d, 1.0, free_run, free_run);
        let mounted = s.run_until(3.0, |s| s.loco().current == ActorContextId::Ladder);
        assert_eq!(mounted, !free_run, "free run {free_run}: {:?} {:?}", s.loco().current, s.body().feet);
        if mounted {
            // the entry starts in the approach facing and turns onto the ladder by half its clip (RE/24 LADDER-6)
            let face = crate::player::heading_of(Vec3::Z);
            let off = |h: f32| crate::player::ground::heading_delta(h, face).abs();
            assert!(off(s.body().heading) > 0.3, "still the approach facing at the start: {}", s.body().heading);
            s.pad(Vec3::ZERO, 0.0, false, false);
            assert!(s.run_until(3.0, |s| s.data().ladder.phase == Some(crate::player::ladder::LadderPhase::Wait)));
            assert!(off(s.body().heading) < 1e-3, "facing the ladder: {}", s.body().heading);
        }
    }
}

#[test]
fn enter_a_ladder_from_the_top_and_climb_down() {
    use crate::player::ladder::{LadderPhase, ENTER_TOP, EXIT_GROUND};
    // on the wall top (z 63.5..64.5, top 5) at the ladder, facing out (-Z): the stick toward the ladder enters it once
    // within 0.6 m, no button (RE/24 LADDER-1)
    let mut s = Sim::new(Vec3::new(50.0, 5.0, 63.9), 0.0);
    s.run(0.1);
    s.pad(Vec3::NEG_Z, 0.5, false, false);
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Ladder), "no entry from the top: {:?} {:?}", s.loco().current, s.body().feet);
    s.pad(Vec3::ZERO, 0.0, false, false);
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
    // the target was searched at the press and the jump is the rebound (kind 2), facing away (RE/24 LADDER-12)
    assert!(s.data().ladder.jump_target.is_some(), "searched at the press");
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::InAir), "never left");
    assert!(crate::player::jump_blend::TAKEOFF_REBOUND.contains(&s.data().air.takeoff.unwrap().id), "the rebound takeoff");
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
fn falling_past_a_ladder_with_empty_hand_held_catches_it() {
    use crate::player::ladder::{LadderPhase, CATCH_AIR, WAIT};
    // FindLadderCatch 0xE04100 (grab held): the catch height rounded down to 0.5 m, the catch action, then the low wait
    let mut s = Sim::new(Vec3::new(50.0, 3.2, 62.9), FACE_PZ);
    s.pad(Vec3::ZERO, 0.0, false, false);
    s.app.world_mut().resource_mut::<PadInput>().hand_held = true;
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
fn falling_past_a_ladder_with_only_legs_does_not_catch_it() {
    let mut s = Sim::new(Vec3::new(50.0, 3.2, 62.9), FACE_PZ);
    s.pad(Vec3::ZERO, 0.0, false, true);
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
    // the ladder candidate: nearest to the hands' height on the ladder shortened to [bottom + 2.5, top - 2.0] (RE/18 §1.4)
    assert!((s.data().ladder.height - 2.5).abs() < 1e-3, "2.5 m up the ladder: {}", s.data().ladder.height);
}

#[test]
fn a_wall_run_beside_a_ladder_steps_onto_it() {
    use crate::player::ladder::{LadderPhase, WALL_RUN_TO_LADDER};
    use crate::player::walling::WallingSubState;
    // the interpreter's walling state (0xEE05E0): a ladder within 0.6 m at the end of a Vertical step, the stick toward
    // it → Ladder EntryType 3: the wall run's own transition clip first (RE/24 LADDER-13)
    let mut s = Sim::new(Vec3::new(50.2, 0.0, 62.0), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    s.press_legs();
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Walling), "no wall run: {:?}", s.loco().current);
    assert!(s.run_until(3.0, |s| s.loco().current != ActorContextId::Walling), "never left the wall");
    assert_eq!(s.loco().current, ActorContextId::Ladder, "the ladder (last walling state {:?})", s.data().walling.sub_state);
    let l = &s.data().ladder;
    assert_eq!(l.phase, Some(LadderPhase::Entry));
    assert_eq!(l.action.map(|a| (a.id, a.item)), Some((WALL_RUN_TO_LADDER, 0)));
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
            air::AirMode::Arrive { .. } => "arrive".into(),
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
    // the free-step reception (0.5 m) and its authored transition into the wait (0.3 m, `freestep_entry_tr_l_wait`)
    assert!(s.body().feet.x - landed_x < 0.95, "slid on after the landing: {} -> {}", landed_x, s.body().feet.x);
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
fn the_ledge_suspends_the_limb_ik_through_the_pull_up() {
    use crate::player::ledge::{ledge_ik_runs, LedgeSubState as S};
    // sub_DCD910: the limb IK runs in SubStates 0, 1, 3, 4, 6, 8, 11, 13 only (RE/24 ANIM-10)
    for (st, runs) in [(S::Movement, true), (S::HandPlacement, true), (S::HangWallReception, true), (S::PullDown, true), (S::Grasp, true),
                       (S::Pullup, false), (S::HangFreeReception, false), (S::TurnCorner, false), (S::TransitionInFromClimb, false), (S::ParallelJump, false)] {
        assert_eq!(ledge_ik_runs(st, None), runs, "{st:?}");
    }
    // the action exceptions: Entry only for the swing back / hop `_b` / step1_to_hangwall, the pull-up only for
    // hangfree_tr_hangwall, never for the wall run's swing into the wall-free hang
    assert!(ledge_ik_runs(S::Entry, Some(crate::player::ledge_moves::SWING_BACK_B)) && !ledge_ik_runs(S::Entry, None));
    assert!(ledge_ik_runs(S::Pullup, Some(0x01B7_02FD)));
    assert!(!ledge_ik_runs(S::HandPlacement, Some(0x0106_F36F)));
    let mut s = hang_at(Vec3::new(12.0, 2.6, 41.7), Vec3::NEG_Z);
    let limbs = |s: &Sim| *s.app.world().get::<crate::player::LimbTargets>(s.player).unwrap();
    assert!(!limbs(&s).suspended && limbs(&s).hands.is_some(), "the hang pins the hands");
    s.pad(Vec3::Z, 1.0, false, false);
    assert!(s.run_until(1.0, |s| s.data().ledge.sub_state == S::Pullup), "no pull-up");
    assert!(limbs(&s).suspended, "the pull-up runs no limb IK, from its first frame");
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground));
    assert!(!limbs(&s).suspended);
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
    // the free-step arrival puts the feet on the candidate, the roof edge (RE/24 AIR-2)
    assert!((s.body().feet.y - 4.4).abs() < 0.05 && s.body().feet.x > 11.45, "on the upper roof: {:?}", s.body().feet);
}

#[test]
fn a_free_jump_enters_the_native_five_metre_drift_cap() {
    // sprint off the 6 m block with nothing to land on: the free jump runs down to its target 3 m below
    // (0xB1E7F0), then falls; the fall never speeds up and stays under the 5 m/s drift cap
    let mut s = Sim::new(Vec3::new(-12.5, 6.0, 4.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir));
    let mut prev = s.body().feet;
    let mut last_h: Option<f32> = None;
    let mut fell = false;
    while s.loco().current == ActorContextId::InAir {
        s.run(1.0 / 60.0 + 1e-4);
        let h = hspeed(prev, s.body().feet);
        prev = s.body().feet;
        if !matches!(s.data().air.mode, crate::player::air::AirMode::Fall { .. }) {
            continue;
        }
        fell = true;
        if let Some(l) = last_h {
            // 0xE0F643 clamps excess drift immediately; no eased seam is present.
            if h > 0.1 { assert!(h <= l + 0.6, "unexpected acceleration {l:.2} -> {h:.2}"); }
        }
        assert!(h <= 5.0 + 0.05, "fall drift {h:.2} over the cap");
        last_h = Some(h);
    }
    assert!(fell, "the jump never turned into a fall");
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
    s.pad(Vec3::Z, 1.0, true, true);
    s.press_legs();
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir), "no swing jump");
    s.run(1.0 / 60.0 + 1e-4);
    assert!(s.app.world().get::<crate::player::LimbTargets>(s.player).unwrap().hands.is_none(), "hands still pinned to the bar in the air");
}

#[test]
fn grabbing_near_the_end_of_a_ledge_keeps_both_hands_on_it() {
    // the jump-up wall's edge starts at x 8: stand at its very end and jump at it
    // (a tap jump: Legs pressed and let go; free running in with Legs held would start the wall run, 0xEE7E50)
    let mut s = Sim::new(Vec3::new(7.85, 0.0, 40.6), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, false);
    s.press_legs();
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
fn running_off_a_roof_steps_off_at_the_rim() {
    // 3.5 m block x -14.5..-9.5: once the feet are 1 cm past the edge (the foot-level edge report, IHuman vt132
    // 0xB1BC50, +52 < -0.01) with a drop of 0.53 m or more, the interpreter's event 68 (guard 0xD84190) steps off:
    // a free jump 1 + 2·speed m ahead in high profile, 3 m down (0xD9D1F0), not a fall from the rim (RE/18 §1.7)
    let mut s = Sim::new(Vec3::new(-11.5, 3.5, 24.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir), "never left the roof");
    let x = s.data().air.start.x;
    assert!((-9.5 + 0.01..-9.5 + 0.15).contains(&x), "stepped off at x {x} (edge at -9.5)");
    assert!(s.data().air.free_target, "the step off is a free jump");
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
fn reversing_at_a_run_skids_into_the_run_turn() {
    use crate::player::ground::TreeState;
    use crate::player::ground_tree::{RUN_TURN, RUN_TURN_EXIT};
    use crate::player::jump_blend::RUN_STOP;
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(1.5);
    s.pad(Vec3::Z, 1.0, true, false);
    s.run(1.0 / 60.0 + 1e-4);
    assert!(s.ground().oneshot.is_some_and(|o| RUN_STOP.contains(&o.blend.id)), "no skid (run stop) on pulling back");
    // the stop ends with the stick still more than 120 deg away: the skid turn (state 24, legacy 94 / 95), not a pivot
    assert!(s.run_until(1.0, |s| s.ground().oneshot.is_some_and(|o| RUN_TURN.contains(&o.blend.id))), "no run turn after the skid");
    assert_eq!(s.ground().owner, TreeState::RunTurn);
    // completed while still turned: the heading turns round and the turn's own exit (legacy 96 / 97) leads into the run
    assert!(s.run_until(1.0, |s| s.ground().tr.is_some_and(|t| RUN_TURN_EXIT.contains(&t.action.id))), "no run-turn exit");
    assert!(s.body().forward().dot(Vec3::Z) > 0.95, "faces the stick: {:?}", s.body().forward());
    let z = s.body().feet.z;
    s.run(1.0);
    assert!(s.body().feet.z > z + 1.5, "runs off the other way: {} -> {}", z, s.body().feet.z);
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
    // `HumanGround__PlayStartMove` 0xD98990: the start transition in front of the locomotion, layout 7
    let t = s.ground().tr.expect("no start transition");
    assert!(START_MOVE[0].contains(&t.action.id), "low-profile start: {:#x}", t.action.id);
    assert_eq!(t.layout, 7);
    assert!((s.ground().speed_param - 0.25).abs() < 0.02, "walk band at once: {}", s.ground().speed_param);
    // steering stays live during it
    s.pad(Vec3::new(-0.5, 0.0, -1.0), 1.0, false, false);
    let h = s.body().heading;
    s.run(0.1);
    assert!((s.body().heading - h).abs() > 0.1, "turned while starting");
    // it hands over to the locomotion item of the other foot when it ends
    assert!(s.run_until(1.0, |s| s.ground().tr.is_none()), "start never ended");
    assert!(s.ground().speed_param > 0.0);
    // releasing the stick during a start ends it (Move_CanStop: the playing action is not the locomotion)
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.run(0.2);
    s.pad(Vec3::NEG_Z, 1.0, false, false);
    s.run(0.1);
    s.pad(Vec3::ZERO, 0.0, false, false);
    s.run(1.0 / 60.0 + 1e-4);
    assert!(s.ground().tr.is_none() && s.ground().speed_param == 0.0);
}

#[test]
fn a_light_stick_starts_into_the_slow_walk() {
    // the player (EntityDescriptorType Main) gets no [0.25, 0.5] clamp on the transition path: layout 7 steers the
    // speed parameter to the light stick's target and weights the slow-walk start (0xDA09B3)
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.run(0.2);
    s.pad(Vec3::NEG_Z, 0.5, false, false);
    s.run(0.5);
    let t = s.ground().tr.expect("start still playing");
    assert!(t.action.weights()[0] > 0.5, "slow-walk start dominates: {:?}", t.action.weights());
    assert!(s.ground().speed_param < 0.15, "{}", s.ground().speed_param);
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
    let [start, lp, _end] = LEDGE_JUMP_TABLE[0][0].1;
    let mv = s.data().ledge.mv.unwrap();
    // the start plays in the Ledge context on its own displacement, the loop in the Climb context with the linear
    // interpolation to the climb pose (0xDDCE40 / 0xDD8290, RE/24 LEDGE-14)
    assert_eq!(mv.seq[0].map(|a| a.id), Some(start));
    assert!(mv.seq[1].is_none() && mv.table == Some((1, 0)));
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Climb), "never reached the climb: {}", s.data().ledge.last_action);
    let c = &s.data().climb;
    assert_eq!(c.entry_type, ClimbEntryType::FromLedgeParallelJump);
    assert_eq!(c.move_action, Some(lp), "the loop plays in the climb");
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

/// Replays every recording in `bugs/*/input.txt` through the gameplay systems (no rendering, animation or IK) and
/// compares each frame's context, root and heading with the capture. Run with
/// `cargo test --release replay_bug_recordings -- --ignored --nocapture`; `AC_BUG=<folder name>` for one bug.
///
/// Two kinds of capture (RE/16 M1 / M2):
/// - a **report** carries a written note (beyond the template line): the bug as it happened. It is replayed and
///   listed, never asserted: a difference usually means the behaviour changed since;
/// - a **fixture** (the template note only: scripted `AC_BUG_AT` captures) is the regression baseline. It is compared
///   with its `baseline.txt` when it has one, else with the `state.txt` it was captured with.
///
/// `AC_REBASELINE=1` writes each fixture's `baseline.txt` from this replay (the capture's own `state.txt` is kept).
/// One frame of simulation detail for `AC_DUMP` (the context, the body, and the context's own state).
fn dump_line(i: usize, s: &Sim) -> String {
    let d = s.data();
    let b = s.body();
    let ctx = s.loco().current;
    let detail = match ctx {
        ActorContextId::InAir => format!(
            "air={:?} takeoff={:?} flight={:?} target={:?}/{:#x}",
            d.air.mode,
            d.air.takeoff.map(|a| (format!("{:#010x}", a.id), a.item)),
            d.air.flight.map(|a| (format!("{:#010x}", a.id), a.item)),
            d.air.target,
            d.air.target_flags
        ),
        ActorContextId::Walling => format!("walling={:?}", d.walling.sub_state),
        ActorContextId::Ledge => format!(
            "ledge={:?}/{:?} \"{}\" hands={:?} {:?} mv={:?}",
            d.ledge.sub_state, d.ledge.hang_type, d.ledge.last_action, d.ledge.hand_l, d.ledge.hand_r, d.ledge.mv.map(|m| (m.kind, m.t))
        ),
        ActorContextId::Climb => format!("climb=pose{} \"{}\"", d.climb.pose, d.climb.last_action),
        _ => String::new(),
    };
    format!("{i} {ctx:?} feet={:?} heading={:.3} vel={:?} {detail}", b.feet, b.heading, b.velocity)
}

#[test]
#[ignore]
fn replay_bug_recordings() {
    use crate::recorder::{bugs_dir, gameplay_key, Recording};
    let only = std::env::var("AC_BUG").ok();
    let rebaseline = std::env::var_os("AC_REBASELINE").is_some();
    // `AC_DUMP=<from>..<to>`: print the simulation's per-frame detail for those frames (debugging a capture)
    let dump = std::env::var("AC_DUMP").ok().and_then(|d| {
        let (a, b) = d.split_once("..")?;
        Some(a.parse::<usize>().ok()?..=b.parse::<usize>().ok()?)
    });
    let Ok(entries) = std::fs::read_dir(bugs_dir()) else {
        eprintln!("no bugs folder at {}", bugs_dir().display());
        return;
    };
    let keys = |text: &str| -> std::collections::BTreeMap<u64, String> {
        text.lines()
            .filter_map(|l| {
                let key = l.split(" | ").next()?.trim().to_string();
                Some((key.split_whitespace().next()?.parse().ok()?, key))
            })
            .collect()
    };
    let mut failures = Vec::new();
    let (mut fixtures, mut reports, mut written) = (0, 0, 0);
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let dir = e.path();
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        if only.as_ref().is_some_and(|o| *o != name) {
            continue;
        }
        let Some(rec) = Recording::load(&dir) else { continue };
        let note = std::fs::read_to_string(dir.join("note.txt")).unwrap_or_default();
        let report = note.lines().skip(1).any(|l| !l.trim().is_empty());
        let state = keys(&std::fs::read_to_string(dir.join("state.txt")).unwrap_or_default());
        let baseline = std::fs::read_to_string(dir.join("baseline.txt")).ok().map(|t| keys(&t));
        let want = if report { &state } else { baseline.as_ref().unwrap_or(&state) };
        let mut s = Sim::new_raw(rec.start_feet, rec.start_heading);
        s.cam_follow = false;
        let mut first_diff = None;
        let mut compared = 0;
        let mut replayed = String::new();
        for (i, f) in rec.frames.iter().enumerate() {
            s.app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_nanos(f.dt_nanos)));
            f.apply(&mut s.app.world_mut().resource_mut::<PadInput>());
            {
                let mut rig = s.app.world_mut().resource_mut::<CameraRig>();
                rig.yaw = f.cam_yaw;
                rig.pitch = f.cam_pitch;
            }
            s.app.update();
            if dump.as_ref().is_some_and(|r| r.contains(&i)) {
                eprintln!("{}", dump_line(i, &s));
            }
            if state.contains_key(&(i as u64)) {
                let got = gameplay_key(i as u64, s.loco(), s.body());
                if let Some(w) = want.get(&(i as u64)) {
                    compared += 1;
                    if first_diff.is_none() && got != *w {
                        first_diff = Some(format!("frame {i}: capture `{w}` replay `{got}`"));
                    }
                }
                replayed.push_str(&got);
                replayed.push('\n');
            }
        }
        if report {
            reports += 1;
            match &first_diff {
                None => eprintln!("{name} (report): {compared} frames compared, identical"),
                Some(d) => eprintln!("{name} (report, not asserted): differs at {d}"),
            }
            continue;
        }
        fixtures += 1;
        if rebaseline {
            std::fs::write(dir.join("baseline.txt"), &replayed).expect("baseline.txt");
            written += 1;
            eprintln!("{name}: baseline written ({compared} frames)");
            continue;
        }
        let against = if baseline.is_some() { "baseline" } else { "state" };
        match first_diff {
            None => eprintln!("{name}: {} frames replayed, {compared} compared with the {against}, identical", rec.frames.len()),
            Some(d) => {
                eprintln!("{name}: DIFFERS from the {against} at {d}");
                failures.push(name);
            }
        }
    }
    eprintln!("{fixtures} fixtures, {reports} reports{}", if rebaseline { format!(", {written} baselines written") } else { String::new() });
    assert!(failures.is_empty(), "fixtures differ: {failures:?} (AC_REBASELINE=1 rewrites their baselines)");
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
    // the wall run's probe D takes the highest band in reach (`SortCandidates`, RE/24 WALL-6), then the hang climbs on
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Climb));
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
    // the wall run's probe D takes the highest band in reach (`SortCandidates`, RE/24 WALL-6), then the hang climbs on
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Climb));
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
    // the wall run's probe D takes the highest band in reach (`SortCandidates`, RE/24 WALL-6), then the hang climbs on
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Climb));
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
    let mut s = climb_at_feet(-56.8, 2.4);
    s.pad(Vec3::X, 0.45, true, false);
    let reached = s.run_until(6.0, |s| s.loco().current == ActorContextId::Ledge);
    assert!(reached, "no reach: {} feet {:?} {:?}", s.data().climb.last_action, s.data().climb.foot_l, s.data().climb.foot_r);
    assert_eq!(s.data().climb.last_action, "reach to a hang");
    let mv = s.data().ledge.mv.expect("the reach");
    assert!(matches!(mv.kind, MoveKind::ClimbReach { long: true }), "a long reach: {:?}", mv.kind);
    let ids = CLIMB_REACH_TABLE[2][1][0];
    assert_eq!(mv.seq.map(|a| a.map(|a| a.id)), [Some(ids[0]), Some(ids[1]), Some(ids[2]), None], "climb1m_tr_hangfree_left_3 a / b / c");
    assert_eq!(mv.hand_l, mv.hand_r, "caught with one hand");
    // CLIMB-11: the root holds still through the start (0xDF6810), goes linearly onto the hang over the loop
    // (0xDEF2F9), and holds there through the end
    let mut m = mv;
    m.t = 0.0;
    let (d0, d1) = (m.durations[0], m.durations[1]);
    assert!(d0 > 0.0 && d1 > 0.0);
    while m.t < m.duration() {
        let (p, _) = m.advance(1.0 / 60.0);
        let t = m.t;
        if t <= d0 {
            assert!(p.distance(m.from) < 1e-4, "still through the start at {t}: {p:?}");
        } else if t < d0 + d1 {
            let want = m.from.lerp(m.to, (t - d0) / d1);
            assert!(p.distance(want) < 1e-3, "linear over the loop at {t}: {p:?} vs {want:?}");
        } else {
            assert!(p.distance(m.to) < 1e-4, "held on the hang through the end at {t}: {p:?}");
        }
    }
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
    let mut s = climb_at_feet(start.x, 2.4);
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
    walk_to_wall(&mut s, Vec3::Z, 2.0);
    s.pad(Vec3::Z, 1.0, true, false);
    s.press_hand(); // climbing from the ground: the empty-hand press (event 50, RE/02 4.7)
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Climb), "never climbed");
    s.pad(Vec3::Z, 0.45, true, false);
    assert!(s.run_until(10.0, |s| s.data().climb.reach.is_some()), "no reach: {} {:?}", s.data().climb.last_action, s.data().climb.foot_l);
    let r = s.data().climb.reach.unwrap();
    assert_eq!(s.data().climb.last_action, "reach up / down");
    assert!(r.ids == reach_ids(CLIMB_REACH_VERTICAL[0][0]) || r.ids == reach_ids(CLIMB_REACH_VERTICAL[1][0]), "a short reach up: {:x?}", r.ids);
    assert!((r.holds[2].y - 2.4).abs() < 0.05 && (r.holds[0].y - 3.6).abs() < 0.05, "feet 2.4 / hands 3.6: {:?}", r.holds);
    // RE/24 CLIMB-12: the limbs take the target holds as the start begins (0xDE86C0 at 0xDF68BA), the root still in place
    let c = &s.data().climb;
    assert_eq!(r.stage, 0);
    assert_eq!([c.hand_l, c.hand_r, c.foot_l, c.foot_r], r.holds, "the limbs on the target holds during the start");
    s.pad(Vec3::Z, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().climb.reach.is_none() && s.data().climb.moving.is_none()), "never arrived");
    assert!((s.data().climb.foot_l.y - 2.4).abs() < 0.05);
}

#[test]
fn review_fatal_landing_is_not_replayed_on_non_air_ground_entry() {
    use crate::player::TransitionSetup;
    let mut s = Sim::new(Vec3::new(-30.0, 21.0, -30.0), 0.0);
    assert!(s.run_until(5.0, |s| s.ground().last_landing.is_some()));
    assert_eq!(s.ground().last_landing.unwrap().kind, LandingType::Fatal);
    s.run(0.1);
    assert!((s.body().feet - SPAWN).length() < 1.0);

    // Model a traversal exit without another landing. The historical fatal fall is still observable.
    let destination = Vec3::new(-30.0, 0.0, -30.0);
    s.app.world_mut().get_mut::<Body>(s.player).unwrap().feet = destination;
    s.app.world_mut().get_mut::<Body>(s.player).unwrap().velocity = Vec3::ZERO;
    s.app.world_mut().get_mut::<Locomotion>(s.player).unwrap().current = ActorContextId::Climb;
    s.app.world_mut().resource_mut::<CameraRig>().shake = 0.0;
    force(&mut s, TransitionSetup::ToMovement { landing: None });
    s.run(1.0 / 60.0 + 1e-4);
    assert!((s.body().feet - destination).length() < 0.01, "stale fatal landing teleported to {:?}", s.body().feet);
    assert_eq!(s.app.world().resource::<CameraRig>().shake, 0.0, "stale landing shook the camera");
    assert_eq!(s.ground().last_landing.unwrap().kind, LandingType::Fatal);
}

#[test]
fn review_nonfatal_landing_shake_is_consumed_once() {
    use crate::player::TransitionSetup;
    let mut s = Sim::new(Vec3::new(-30.0, 5.0, -30.0), 0.0);
    assert!(s.run_until(5.0, |s| s.ground().last_landing.is_some()));
    s.run(1.0 / 60.0 + 1e-4);
    assert!(s.app.world().resource::<CameraRig>().shake > 0.0);
    s.app.world_mut().resource_mut::<CameraRig>().shake = 0.0;
    s.app.world_mut().get_mut::<Locomotion>(s.player).unwrap().current = ActorContextId::Ladder;
    force(&mut s, TransitionSetup::ToMovement { landing: None });
    s.run(1.0 / 60.0 + 1e-4);
    assert_eq!(s.app.world().resource::<CameraRig>().shake, 0.0, "stale nonfatal landing shook the camera");
    assert!(s.ground().last_landing.is_some(), "keep landing history available to diagnostics");
}

#[test]
fn flat_fall_height_scenarios_reach_the_expected_player_bands() {
    // Deliberately away from edges: >=9 m rag-fall is an edge/reception rule, not a flat-fall height rule.
    for (height,kind,damage) in [(1.0,LandingType::Safe,0),(3.0,LandingType::Safe,0),
        (5.0,LandingType::Safe,0),(9.0,LandingType::SmallDamage,10),(13.0,LandingType::HeavyDamage,40),
        (21.0,LandingType::Fatal,200)] {
        let mut s = Sim::new(Vec3::new(-30.0,height,-30.0),0.0);
        assert!(s.run_until(5.0, |s| s.ground().last_landing.is_some()), "no landing at {height}");
        let l=s.ground().last_landing.unwrap();
        assert_eq!((l.kind,l.damage),(kind,damage),"height {height}: {l:?}");
        if kind==LandingType::Fatal {
            // PORT: immediate respawn remains until articulated fatal landing/desynchronisation exists.
            s.run(0.1);
            assert!((s.body().feet-SPAWN).length()<1.0);
        }
    }
}

#[test]
fn an_ordinary_ballistic_fall_enters_hay_without_ground_damage() {
    let mut s=Sim::new(Vec3::new(37.5,13.0,26.0),0.0);
    assert!(s.run_until(4.0, |s| s.loco().current==ActorContextId::HayStack));
    // a drop straight onto the top: contact normal more than 0.9 up → entry Top (0xE05490 / 0xE00E30)
    assert_eq!(s.data().hay.action.map(|a|a.id),Some(crate::player::hay::HAYSTACK_FAITH_LANDING));
    assert!(s.ground().last_landing.is_none());
    assert!(s.run_until(3.0, |s| s.data().hay.phase==crate::player::hay::HayPhase::Waiting));
}


#[test]
fn falling_ladder_catch_requires_empty_hand_and_finishes_the_full_action() {
    for hand in [false,true] {
        let from=Vec3::new(50.0,3.2,62.9);
        let mut s=Sim::new(from,std::f32::consts::PI);
        force(&mut s,crate::player::TransitionSetup::ToInAir(air::InAirEntry::Fall{from,velocity:Vec3::ZERO,origin:air::FallOrigin::Ground,speed_param:0.0}));
        s.pad(Vec3::ZERO,0.0,false,!hand);
        s.app.world_mut().resource_mut::<PadInput>().hand_held=hand;
        assert!(s.run_until(2.0,|s|s.loco().current!=ActorContextId::InAir));
        if hand {
            assert_eq!(s.loco().current,ActorContextId::Ladder);
            assert_eq!(s.data().ladder.action.unwrap().id,crate::player::ladder::CATCH_AIR);
            assert!(s.run_until(2.0,|s|s.data().ladder.phase==Some(crate::player::ladder::LadderPhase::Wait)));
            assert!(!s.data().ladder.high && s.data().ladder.foot==0);
            assert!((s.body().feet.y-s.data().ladder.height).abs()<0.001);
        } else {assert_ne!(s.loco().current,ActorContextId::Ladder);}
    }
}

#[test]
fn air_catch_ladder_uses_raw_reach_and_clearance_before_ledge_priority() {
    use crate::player::ladder::find_ladder_catch;
    let (mut collision,guidance)=level::geometry();
    let from=Vec3::new(50.15,2.7,62.9);
    let reach=Vec3::new(-0.15,0.0,0.6).normalize();
    let e=find_ladder_catch(from,reach,true,&guidance,&collision).expect("diagonal ladder catch");
    // the out direction is the ladder's normal on the climber's side, not the diagonal to the root (RE/24 LADDER-14)
    let out=e.catch_out.unwrap();
    assert!(out.x.abs()<1e-3 && out.z< -0.99,"the ladder's normal toward the root: {out:?}");
    let to=e.base+Vec3::Y*e.height.unwrap()+e.catch_out.unwrap()*0.6+Vec3::Y;
    collision.boxes.push(crate::collision::Aabb3{min:to-Vec3::splat(0.1),max:to+Vec3::splat(0.1)});
    assert!(find_ladder_catch(from,reach,true,&guidance,&collision).is_none());
}

#[test]
fn falling_beam_catch_uses_runtime_contacts_without_authored_beam_lines() {
    let from=Vec3::new(86.0,5.5,70.05);
    let mut s=Sim::new(from,-std::f32::consts::FRAC_PI_2);
    s.app.world_mut().resource_mut::<crate::guidance::GuidanceWorld>().edges.retain(|e|e.subtype!=crate::guidance::GuidanceSubType::Beam);
    s.app.world_mut().resource_mut::<crate::collision::CollisionWorld>().native_query_culling=true;
    force(&mut s,crate::player::TransitionSetup::ToInAir(air::InAirEntry::Fall{from,velocity:Vec3::ZERO,origin:air::FallOrigin::Ground,speed_param:0.0}));
    assert!(s.run_until(2.0,|s|s.loco().current!=ActorContextId::InAir));
    assert_eq!(s.loco().current,ActorContextId::NarrowObject);
    assert_eq!(s.data().narrow.kind,crate::player::narrow::NarrowKind::Beam);
    s.run(1.5);assert!((s.body().feet.y-4.0).abs()<0.01);
    assert_eq!(s.loco().current,ActorContextId::NarrowObject,"runtime support persists after reception");
}

#[test]
fn native_masyaf_runtime_beam_contacts_are_derived_from_ledge_edges() {
    let Some((collision,guidance,_))=crate::native_map::village_simulation_fixture() else {return;};
    let report=crate::guidance::beam_contacts::detect(&guidance,&collision,Vec3::ZERO,Vec3::splat(10000.0));
    eprintln!("native runtime beam contacts: {}",report.len());
    assert!(!report.is_empty(),"native ledge geometry must produce runtime beam contacts");
    assert!(report.iter().all(|b|b.widths.iter().all(|w|*w<=0.5005)));
    let clear=report.iter().filter(|b|crate::player::narrow::beam_catch_clear(b.p0.lerp(b.p1,0.5),(b.p1-b.p0).with_y(0.0).normalize_or_zero(),b.width(b.p0.lerp(b.p1,0.5)),&collision)).count();
    eprintln!("native runtime contacts with clear reception: {clear}");
    let beam=report.iter().filter(|b|b.p0.distance(b.p1)>0.8).filter(|b| {
        let point=b.p0.lerp(b.p1,0.5);let axis=(b.p1-b.p0).with_y(0.0).normalize_or_zero();
        crate::player::narrow::beam_catch_clear(point,axis,b.width(point),&collision) && collision.capsule_fits(point+Vec3::Y*1.5)
    }).max_by(|a,b|a.p0.distance_squared(a.p1).total_cmp(&b.p0.distance_squared(b.p1))).expect("clear native reception fixture");
    let from=beam.p0.lerp(beam.p1,0.5)+Vec3::Y*1.5;
    let facing=(beam.p1-beam.p0).with_y(0.0).normalize();
    let mut s=Sim::new_raw(from,crate::player::heading_of(facing));
    s.app.insert_resource(collision).insert_resource(guidance);s.app.update();
    assert!(s.run_until(3.0,|s|s.loco().current==ActorContextId::NarrowObject),"native fall missed beam: {:?} {:?}",s.loco().current,s.body().feet);
    assert_eq!(s.data().narrow.entry_mode,crate::player::narrow::BeamEntryMode::Reception);
    s.run(1.5);
    assert_eq!(s.loco().current,ActorContextId::NarrowObject,"source beam support persists after reception");
    assert_eq!(s.data().narrow.state,crate::player::narrow::BeamState::Wait);
}

#[test]
fn ground_extras_static_jump_requires_high_profile_then_returns_to_ground() {
    let start=Vec3::new(-30.0,0.0,-30.0);
    let mut s=Sim::new(start,0.0);
    s.pad(Vec3::ZERO,0.0,false,true);s.run(0.5);assert!(!s.saw_air);
    s.pad(Vec3::ZERO,0.0,true,true);s.press_legs();
    s.run(0.4);assert!(!s.saw_air,"holding Legs must remain in FreeRun");
    assert!(s.ground().extra.is_some());
    s.pad(Vec3::ZERO,0.0,true,false);
    let mut top=0.0f32;
    for _ in 0..120 {s.run(1.0/60.0+0.0001);top=top.max(s.body().feet.y);if s.saw_air {s.pad(Vec3::ZERO,0.0,false,false);}}
    assert!(s.saw_air,"static jump did not leave Ground");assert!(top>0.2,"static clip did not lift: {top}");
    assert_eq!(s.loco().current,ActorContextId::Ground);assert!(s.body().feet.y.abs()<0.02);
}

#[test]
fn ground_extras_crouch_walk_and_clearance_exit_use_real_ground_system() {
    let start=Vec3::new(-30.0,0.0,-30.0);let mut s=Sim::new(start,0.0);
    {
        let mut helper=s.app.world_mut().get_mut::<crate::player::social::SocialHelper>(s.player).unwrap();
        helper.allowed=true;helper.set(5); // PORT validation: native status-5 producer remains unknown.
    }
    s.run(0.2);assert!(s.ground().crouch.is_some());assert_eq!(s.body().proxy.height,Some(1.0));
    s.pad(Vec3::NEG_Z,1.0,false,false);s.run(2.0);
    assert!(s.body().feet.z<start.z-0.2,"crouch root motion missing: {:?}",s.body().feet);
    s.pad(Vec3::ZERO,0.0,false,false);s.run(0.2);
    s.app.world_mut().get_mut::<crate::player::social::SocialHelper>(s.player).unwrap().clear(5);
    s.run(1.0);assert!(s.ground().crouch.is_none());assert_eq!(s.body().proxy.height,None);
}

#[test]
fn ground_extras_steep_surface_slides_in_air_then_lands_on_flat() {
    let mut s=Sim::new_raw(Vec3::new(-30.0,9.0,-30.0),0.0);
    crate::recorder::ground_checks::slope_fixture(&mut s.app.world_mut().resource_mut::<crate::collision::CollisionWorld>());
    s.app.insert_resource(crate::guidance::GuidanceWorld::default());
    s.app.update();
    assert!(s.run_until(2.0,|s|s.data().air.slope_slide.start_y.is_some()),"no steep contact: {:?}",s.body().feet);
    assert_eq!(s.loco().current,ActorContextId::InAir);
    let contact=s.body().feet;
    assert!(s.run_until(4.0,|s|s.loco().current==ActorContextId::Ground),"stuck: {:?} {:?}",s.body().feet,s.body().velocity);
    assert!(s.body().feet.x<contact.x-2.0,"did not slide down: {:?}",s.body().feet);
    assert!(!s.data().air.slope_slide.ragfall_required);
}

#[test]
fn ground_extras_social_request_keeps_crouch_under_a_low_ceiling() {
    let start=Vec3::new(-30.0,0.0,-30.0);let mut s=Sim::new_raw(start,0.0);
    s.app.world_mut().resource_mut::<crate::collision::CollisionWorld>().boxes.push(crate::collision::Aabb3{
        min:start+Vec3::new(-2.0,1.2,-2.0),max:start+Vec3::new(2.0,1.5,2.0)});
    {let mut helper=s.app.world_mut().get_mut::<crate::player::social::SocialHelper>(s.player).unwrap();helper.allowed=true;helper.set(5);}
    s.app.update();s.run(1.0);assert!(s.ground().crouch.is_some());assert_eq!(s.body().proxy.height,Some(1.0));
    s.app.world_mut().get_mut::<crate::player::social::SocialHelper>(s.player).unwrap().clear(5);
    s.run(1.0);assert!(s.ground().crouch.is_some(),"standing must wait for clearance");
    s.app.world_mut().resource_mut::<crate::collision::CollisionWorld>().boxes.pop();
    assert!(s.run_until(1.0,|s|s.ground().crouch.is_none()));assert_eq!(s.body().proxy.height,None);
}

#[test]
fn ground_extras_registered_crowd_actor_drives_and_releases_lean() {
    use crate::player::crowd::{CrowdActor,PushReceiver};
    let start=Vec3::new(-30.0,0.0,-30.0);let mut s=Sim::new(start,0.0);
    let actor=s.app.world_mut().spawn((Body{feet:start+Vec3::new(0.6,0.0,-1.0),..default()},CrowdActor{
        proximity_eligible:true,receiver:PushReceiver{valid:true,relationship_blocks:false,state_63_87_81:false,state_77:false,
        protected:false,context_blocks:false,move_mode:0,special_gentle:false,event_allowed:true,minimum_reaction:0}})).id();
    s.pad(Vec3::NEG_Z,0.6,false,false);s.run(0.5);
    assert!(s.ground().blend.lean.abs()>0.05,"crowd actor did not alter locomotion lean");
    assert!(s.ground().crowd_avoid.is_none(),"native vector consumed once by the blend");
    s.app.world_mut().despawn(actor);s.run(2.0);
    assert!(s.ground().blend.lean.abs()<0.001,"lean did not release: {}",s.ground().blend.lean);
}


// ---------------------------------------------------------------- ground tree re-entry (RE/02 §4.6)

#[test]
fn a_landing_hands_over_through_its_authored_transition() {
    use crate::player::move_blend::ACT_GROUND_LOCOMOTION;
    let mut s = Sim::new(Vec3::new(7.0, 3.5, 12.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    assert!(s.run_until(4.0, |s| s.saw_air && s.loco().current == ActorContextId::Ground), "never landed");
    let landing = s.ground().oneshot.expect("landing action").blend;
    // the landing completes (state 21 has no early exit), then its item's transition action plays in front of the
    // locomotion, steered by MoveBlend's transition path
    assert!(s.run_until(1.0, |s| s.ground().tr.is_some()), "no transition after the landing");
    let t = s.ground().tr.unwrap();
    let authored = crate::player::ground_tree::authored_transition(landing.id, landing.item, ACT_GROUND_LOCOMOTION);
    assert_eq!(authored.map(|a| a.0), Some(t.action.id), "not the landing's authored transition");
    assert!(s.run_until(1.5, |s| s.ground().tr.is_none()), "transition never ended");
    assert_eq!(s.ground().blend.foot, t.dest_item, "hands over to the transition's destination item");
    assert!(s.ground().speed_param > 0.25);
}

#[test]
fn a_pivot_hands_over_through_the_turn_start() {
    // `HumanGround__PlayTurnStartMove` 0xD99710 decides by the profile the pivot started in (HG+1500, RE/24 GROUND-12)
    // a pivot started in high profile → layout 2, s 0.5
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.pad(Vec3::ZERO, 0.0, true, false);
    s.run(0.2);
    s.pad(Vec3::Z, 1.0, true, false);
    s.run(1.0 / 60.0 + 1e-4);
    assert_eq!(s.ground().owner, crate::player::ground::TreeState::Pivot);
    assert!(s.run_until(1.0, |s| s.ground().tr.is_some()), "no turn start");
    let t = s.ground().tr.unwrap();
    assert_eq!(t.layout, 2);
    assert!(!t.fast_start);
    assert!((s.ground().speed_param - 0.5).abs() < 0.02, "{}", s.ground().speed_param);
    // a 180 deg turn weights the 180 clip pair
    assert!(t.action.weights()[3] > 0.9, "{:?}", t.action.weights());
    // started low with the high profile pressed with the stick: layout 3 at s 0.5 (slots 4 / 5), the fast start
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.run(0.2);
    s.pad(Vec3::Z, 1.0, true, false);
    s.run(1.0 / 60.0 + 1e-4);
    assert_eq!(s.ground().owner, crate::player::ground::TreeState::Pivot);
    assert!(s.run_until(1.0, |s| s.ground().tr.is_some()), "no turn start");
    let t = s.ground().tr.unwrap();
    assert_eq!(t.layout, 3);
    assert!(t.fast_start);
    assert!((s.ground().speed_param - 0.5).abs() < 0.02, "{}", s.ground().speed_param);
    assert!(t.action.weights()[5] > 0.9, "{:?}", t.action.weights());
}

#[test]
fn the_run_stop_plays_the_foot_of_the_locomotion_items_word() {
    // RunStop_Enter 0xD98E30: PickLocomotionFoot over item words [0x0FC9, 0x0FC6] → item 0 plays legacy 87
    // (RUN_STOP[1]), item 1 legacy 86 (RUN_STOP[0]) (RE/24 GROUND-7)
    use crate::player::jump_blend::RUN_STOP;
    for want_item in [0usize, 1] {
        let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
        s.pad(Vec3::NEG_Z, 1.0, true, false);
        s.run(1.5);
        // early in the item, so the foot does not flip on the release frame
        assert!(s.run_until(2.0, |s| s.ground().tr.is_none() && s.ground().blend.foot == want_item && s.ground().blend.phase < 0.5), "never on item {want_item}");
        s.pad(Vec3::ZERO, 0.0, true, false);
        s.run(1.0 / 60.0 + 1e-4);
        assert_eq!(s.ground().owner, crate::player::ground::TreeState::RunStop);
        let id = s.ground().oneshot.map(|o| o.blend.id);
        assert_eq!(id, Some(RUN_STOP[1 - want_item]), "item {want_item}");
    }
}

#[test]
fn moving_again_after_a_run_stop_plays_its_exit() {
    use crate::player::ground_tree::RUN_STOP_EXIT;
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(1.5);
    s.pad(Vec3::ZERO, 0.0, true, false);
    s.run(1.0 / 60.0 + 1e-4);
    assert_eq!(s.ground().owner, crate::player::ground::TreeState::RunStop);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    // `HumanGround__PlayRunStopExitMove` 0xD8B3F0: legacy 92 / 93 into the locomotion, s := 0.5
    assert!(s.run_until(1.0, |s| s.ground().tr.is_some_and(|t| RUN_STOP_EXIT.contains(&t.action.id))), "no run-stop exit");
    assert!((s.ground().speed_param - 0.5).abs() < 0.05);
}

// ---------------------------------------------------------------- turning (RE/02 §4.3)

#[test]
fn the_player_turns_at_the_interpreters_rate() {
    // walking, then the stick 60 deg to the side: 4.0 rad/s (0xEE6D5D), 2.6 rad/s with Legs held
    for (legs, rate) in [(false, 4.0f32), (true, 2.6)] {
        let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
        s.pad(Vec3::NEG_Z, 1.0, false, legs);
        s.run(1.5);
        let h0 = s.body().heading;
        s.pad(Vec3::new(-0.866, 0.0, -0.5), 1.0, false, legs);
        s.run(0.1);
        let turned = crate::player::ground::heading_delta(s.body().heading, h0).abs();
        assert!((turned - rate * 0.1).abs() < 0.05, "legs {legs}: turned {turned} rad in 0.1 s, want {}", rate * 0.1);
    }
}

#[test]
fn running_at_a_wall_slides_along_it_and_head_on_stops() {
    // wall x -7..7, z -8.3..-7.7, h 2.4 (the trace_wall_at_an_angle wall)
    let mut s = Sim::new(Vec3::new(-5.0, 0.0, -6.0), 0.0);
    s.pad(Vec3::new(0.866, 0.0, -0.5), 1.0, true, false);
    s.run(2.0);
    // 60 deg off head-on: the stick is turned along the wall (0xEDD7C0), so the body faces along it and keeps going
    // (within 45 deg of head-on it would stop)
    let f = s.body().forward();
    assert!(f.x > 0.9, "faces along the wall: {f:?}");
    assert!(s.ground().speed_param > 0.4, "keeps running: {}", s.ground().speed_param);
    // straight into it: the wanted speed is 0
    let mut s = Sim::new(Vec3::new(0.0, 0.0, -4.0), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(2.0);
    assert!(s.ground().speed_param == 0.0, "stops at the wall: {}", s.ground().speed_param);
    assert!(s.body().forward().z < -0.95);
}


#[test]
fn an_air_catch_holds_the_hang_for_its_locked_catch_action() {
    // both catch items are locked (0x0FE0); the ledge starts no move until the catch action has played
    use crate::player::ledge::catch_time;
    for (wall, long, t) in [(true, false, 0.667f32), (true, true, 0.934), (false, false, 0.867), (false, true, 0.667)] {
        assert!((catch_time(wall, long) - t).abs() < 0.002, "wall {wall} long {long}: {}", catch_time(wall, long));
    }
}

#[test]
fn a_wall_free_hang_swings_onto_the_wall_before_it_pulls_up() {
    // HumanLedge__Pullup_Start 0xDDBE80: the wall-free hang (free with a wall below) does not pull up; it plays
    // `hangwallfree_tr_hangwall` into the wall hang, and the next push up is the wall pull-up. The free pull-up from
    // there held the body a metre off the wall, legs in the air.
    let mut s = hang_on_jump_up_wall();
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()));
    assert_eq!(s.data().ledge.hang_type, ledge::LedgeHangType::Free);
    let face_z = 41.7;
    s.pad(Vec3::Z, 1.0, false, false);
    assert!(s.run_until(1.0, |s| s.data().ledge.mv.is_some()));
    assert_eq!(s.data().ledge.mv.unwrap().seq[0].map(|a| a.id), Some(crate::player::ledge_moves::WALLFREE_TO_WALL));
    let mut worst = 0.0f32;
    let mut was_wall = false;
    for _ in 0..240 {
        s.run(1.0 / 60.0 + 1e-4);
        worst = worst.max(face_z - s.body().feet.z);
        was_wall |= s.data().ledge.hang_type == ledge::LedgeHangType::Wall;
        if s.loco().current == ActorContextId::Ground {
            break;
        }
    }
    assert!(was_wall, "never switched to the wall hang");
    assert!(worst <= 0.55, "the root left the wall by {worst} m");
    assert!(s.loco().current == ActorContextId::Ground && (s.body().feet.y - 2.6).abs() < 0.05, "pull-up failed: {:?}", s.body().feet);
}

#[test]
fn holding_legs_free_runs_and_a_tap_jumps_when_let_go() {
    // GoAssassinActionInterpreter__ReadInput 0xEEDF80 fills the Legs buffer on the press; the running jump (0xEE817E)
    // needs the buffer with Legs no longer held
    let mut s = Sim::new(Vec3::new(-60.0, 0.0, -40.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, false);
    s.run(1.0);
    s.pad(Vec3::X, 1.0, true, true);
    s.press_legs();
    s.run(0.6);
    assert_eq!(s.loco().current, ActorContextId::Ground, "a held press must not jump");
    s.pad(Vec3::X, 1.0, true, false);
    s.run(0.6);
    s.press_legs();
    assert!(s.run_until(0.3, |s| s.loco().current == ActorContextId::InAir), "a tap jumps once let go");
}

#[test]
fn a_running_jump_without_a_target_matches_the_live_game() {
    // Human__BuildFreeJumpTarget 0xB1E7F0: 8 m ahead, 3 m down; 0xB145B0 swaps to the beam_air flight; the ground
    // contact ends it. Live (CE, 2026-10-08): 0.656 s in the air, 4.81 m, apex 0.57 m, flight 0x118DA41C/D.
    let mut s = Sim::new(Vec3::new(-60.0, 0.0, -40.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, false);
    s.run(1.5);
    s.press_legs();
    assert!(s.run_until(0.3, |s| s.loco().current == ActorContextId::InAir));
    let flight = s.data().air.flight.map(|a| a.id).unwrap();
    assert!(crate::player::jump_blend::FLIGHT_FREESTEP_DOWN.contains(&flight), "flight {flight:#x}");
    let (start, mut apex, mut t) = (s.body().feet, 0.0f32, 0.0f32);
    while s.loco().current == ActorContextId::InAir && t < 3.0 {
        s.run(1.0 / 60.0 + 1e-4);
        t += 1.0 / 60.0;
        apex = apex.max(s.body().feet.y - start.y);
    }
    let d = (s.body().feet - start).with_y(0.0).length();
    assert_eq!(s.loco().current, ActorContextId::Ground);
    assert!((0.6..0.72).contains(&t), "air time {t}");
    assert!((0.5..0.65).contains(&apex), "apex {apex}");
    assert!((4.4..5.4).contains(&d), "distance {d}");
}

#[test]
fn legs_in_high_profile_jumps_straight_off_a_ledge_and_low_profile_legs_does_nothing() {
    use crate::player::ledge::LedgeSubState;
    use crate::player::ledge_moves::{MoveKind, REBOUND};
    // GoAssassinActionInterpreter__LedgeState 0xEEC89F (event 2): high profile + the Legs buffer; with the stick in the
    // dead zone the jump goes straight away from the wall. Without high profile Legs does nothing (verified live).
    // Event 2's guard refuses a free hang (0xDCCE10): the balcony slab's free hang stays (RE/24 LEDGE-16).
    let mut s = hang_on_balcony();
    s.pad(Vec3::ZERO, 0.0, true, false);
    s.press_legs();
    s.run(0.5);
    assert_eq!(s.loco().current, ActorContextId::Ledge, "a free hang jumped off");
    // wall C (2.6 m, face z 49.7): a wall hang
    let mut s = hang_at(Vec3::new(20.0, 2.6, 49.7), Vec3::NEG_Z);
    s.pad(Vec3::ZERO, 0.0, false, false);
    s.press_legs();
    s.run(0.5);
    assert_eq!(s.loco().current, ActorContextId::Ledge, "low-profile Legs let go");
    let n = s.data().ledge.normal;
    let start = s.body().feet;
    s.pad(Vec3::ZERO, 0.0, true, false);
    s.press_legs();
    // the rebound turn plays in the Ledge context first (0xDD2CA0, SubState 5), straight back = `_footr`
    assert!(s.run_until(0.2, |s| s.data().ledge.mv.is_some_and(|m| m.kind == MoveKind::Rebound)), "no rebound");
    assert_eq!(s.data().ledge.sub_state, LedgeSubState::ReboundTransition);
    assert_eq!(s.data().ledge.mv.unwrap().seq[0].map(|a| a.id), Some(REBOUND[1]));
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::InAir), "high profile + Legs did not jump");
    assert!(s.body().forward().dot(Vec3::new(n.x, 0.0, n.z).normalize()) > 0.9, "facing away from the wall");
    s.run(0.3);
    let moved = (s.body().feet - start).with_y(0.0);
    assert!(moved.normalize_or_zero().dot(Vec3::new(n.x, 0.0, n.z).normalize()) > 0.9, "not away from the wall: {moved:?} n {n:?}");
}

#[test]
fn the_jump_off_a_wall_follows_the_stick_and_refuses_into_the_wall() {
    use crate::player::targets::jump_off_wall_dir;
    let away = Vec3::Z;
    assert!((jump_off_wall_dir(None, away).unwrap() - Vec3::Z).length() < 1e-5);
    // 60 degrees round: kept
    let d60 = Vec3::new(60f32.to_radians().sin(), 0.0, 60f32.to_radians().cos());
    assert!((jump_off_wall_dir(Some(d60), away).unwrap() - d60).length() < 1e-4);
    // 120 degrees round: turned to 89 degrees
    let d120 = Vec3::new(120f32.to_radians().sin(), 0.0, 120f32.to_radians().cos());
    let j = jump_off_wall_dir(Some(d120), away).unwrap();
    assert!((away.angle_between(j).to_degrees() - 89.0).abs() < 0.1, "{j:?}");
    assert!(j.x > 0.0, "turned to the wrong side");
    // into the wall: refused
    assert!(jump_off_wall_dir(Some(-Vec3::Z), away).is_none());
}

#[test]
fn free_running_into_a_wall_starts_the_wall_run_without_a_press() {
    // 0xEE7E50 (verified live): high profile, Legs held, the stick past the dead zone and within 60 degrees of the
    // facing -> the walling guard and start; no fresh press needed
    let mut s = Sim::new(Vec3::new(76.0, 0.0, 57.9), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Walling), "no wall run: {:?} {:?}", s.loco().current, s.body().feet);
}

/// RE/24 WALL-15: standing in the FreeRun sub-state (high profile, Legs held, the stick released) in front of a wall,
/// the last strong stick direction aims the wall run; it starts 0.2 s after the FreeRun start, not before. Without a
/// strong push ever made, the stored direction is zero and nothing starts.
#[test]
fn the_free_run_stance_runs_up_a_wall_along_the_last_strong_stick() {
    let mut s = Sim::new(Vec3::new(76.0, 0.0, 57.8), FACE_PZ);
    s.pad(Vec3::Z, 1.0, false, false);
    s.run(0.3);
    s.pad(Vec3::Z, 0.0, false, false);
    s.run(0.5);
    assert_eq!(s.loco().current, ActorContextId::Ground);
    s.pad(Vec3::Z, 0.0, true, true);
    s.run(0.1);
    assert_eq!(s.loco().current, ActorContextId::Ground, "the wall run came before the 0.2 s gate");
    assert!(s.run_until(0.5, |s| s.loco().current == ActorContextId::Walling), "no wall run from the free-run stance: {:?} {:?}", s.loco().current, s.body().feet);

    let mut s = Sim::new(Vec3::new(76.0, 0.0, 58.2), FACE_PZ);
    s.pad(Vec3::Z, 0.0, true, true);
    s.run(1.0);
    assert_eq!(s.loco().current, ActorContextId::Ground, "a wall run with no stored direction");
}

#[test]
fn the_empty_hand_press_starts_a_climb_and_holding_legs_does_not() {
    // climbing from the ground is the empty-hand press (event 50, 0xEE7255); Legs held at a standstill does not climb
    let mut s = Sim::new(Vec3::new(-20.5, 0.0, 26.5), FACE_PZ);
    s.pad(Vec3::Z, 0.0, false, true);
    s.run(0.5);
    assert_eq!(s.loco().current, ActorContextId::Ground, "low-profile Legs started something");
    s.press_hand();
    assert!(s.run_until(0.2, |s| s.loco().current == ActorContextId::Climb), "the empty hand did not climb");
}

#[test]
fn the_free_step_exit_runs_at_the_free_run_band() {
    // the free-step reception's exit into the locomotion (`freestep_entry_tr_{l_walk, h_jog, h_sprint_impultion}`):
    // live 2026-10-08 the high-profile run played the jog clip (797 ms) and free run the sprint impulsion (516 / 532 ms)
    let exit = [0x0D99_71FB, 0x0D99_71FC];
    for free_run in [true, false] {
        let mut s = Sim::new(Vec3::new(7.0, 3.5, 12.0), -std::f32::consts::FRAC_PI_2);
        s.pad(Vec3::X, 1.0, true, true);
        assert!(s.run_until(4.0, |s| s.saw_air), "never jumped");
        s.pad(Vec3::X, 1.0, true, free_run);
        assert!(
            s.run_until(4.0, |s| s.ground().tr.is_some_and(|t| exit.contains(&t.action.id))),
            "no free-step exit (free run {free_run})"
        );
        let w = s.ground().tr.unwrap().action.weights().to_vec();
        if free_run {
            assert!(w[2] > 0.99, "free run did not play the sprint impulsion: {w:?}");
        } else {
            assert!(w[1] > 0.99, "the run did not play the jog: {w:?}");
        }
    }
}

#[test]
fn free_running_into_a_knee_high_box_hops_onto_it() {
    // the 0.6 m box at (9, 0), 1.5 m square: the free-run target jump (0xEE7F7A, no press) finds its near edge in
    // the kind-4 box (0.5–2.0 m ahead, 0.45–1.3 m up) and jumps at it as a free step (type 1, RE/18 §1.5)
    let mut s = Sim::new(Vec3::new(5.5, 0.0, 0.0), -std::f32::consts::FRAC_PI_2);
    s.pad(Vec3::X, 1.0, true, true);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::InAir), "no hop: {:?}", s.body().feet);
    assert_eq!(s.data().air.target.map(|t| t.type_flags), Some(1));
    assert!(s.data().air.start.x < 8.25 - 0.4, "jumped from in front of the box: {:?}", s.data().air.start);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ground && (s.body().feet.y - 0.6).abs() < 0.05), "not on the box: {:?}", s.body().feet);
}

#[test]
fn walking_off_a_low_ledge_steps_off_it() {
    // on top of the 1.6 m knee block (x 48.5..51.5, z 49.8..50.8), walking off its -Z edge in low profile: no
    // low-profile halt (drop under 2 m); 1 cm past the edge event 68 steps off: a free jump 1 m ahead, 3 m down
    // (0xD84190 / 0xD9D1F0, RE/18 §1.7)
    let mut s = Sim::new(Vec3::new(50.0, 1.6, 50.3), std::f32::consts::PI);
    s.pad(Vec3::NEG_Z, 0.6, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir), "never left the block: {:?}", s.body().feet);
    let start = s.data().air.start;
    assert!((49.8 - 0.15..49.8 - 0.01).contains(&start.z), "stepped off at {start:?} (edge at z 49.8)");
    assert!(s.data().air.free_target);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ground), "never landed");
    assert!(s.body().feet.y.abs() < 0.05 && s.body().feet.z < 49.5, "{:?}", s.body().feet);
}

#[test]
fn walking_into_a_haystack_dives_in() {
    use crate::player::hay::{HayEntry, HayPhase, HAYSTACK_DIVE};
    // the haystack at (37.5, 26), 2.2 m square, 1.5 m high: walking into it pushes event 122 (guard 0xD8F0B0) and
    // dives in with the Ground entry (0xE42420): one of the two dive clips, the root carried to the haystack
    let mut s = Sim::new(Vec3::new(37.5, 0.0, 22.5), FACE_PZ);
    s.pad(Vec3::Z, 1.0, false, false);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::HayStack), "never dived in: {:?}", s.body().feet);
    let h = &s.data().hay;
    assert_eq!(h.kind, HayEntry::Ground);
    assert!(h.action.is_some_and(|a| HAYSTACK_DIVE.contains(&a.id)));
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().hay.phase == HayPhase::Waiting), "never settled");
    let f = s.body().feet;
    assert!((f.x - 37.5).abs() < 0.05 && (f.z - 26.0).abs() < 0.05, "inside the haystack: {f:?}");
}


#[test]
fn a_jump_at_a_kiosk_swings_through_its_frame_and_drops() {
    use crate::player::kiosk::{KioskSide, KIOSK_ACTIONS};
    // the 4.5 m roof at (120, 20) and the kiosk frame 3 m out at 3 m (KIOSKS): the tap jump's query offers the
    // kiosk piece (0x400, RE/18 §1.4), the arrival hands over to the Kiosk context (0xE07D00 → 0xE015A0); jumping
    // across its bar is a front side: entry, one monkey-bar cycle, then InAir with the `_tr_fall` exit (0xE3E390)
    let mut s = Sim::new(Vec3::new(120.0, 4.5, 19.5), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, false);
    s.run(0.6);
    s.press_legs();
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::InAir), "no jump: {:?}", s.body().feet);
    assert_eq!(s.data().air.target_flags, 0x400);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Kiosk), "never reached the kiosk: {:?} {:?}", s.loco().current, s.body().feet);
    let side = s.data().kiosk.side;
    assert!(matches!(side, KioskSide::FrontLeft | KioskSide::FrontRight), "{side:?}");
    assert_eq!(s.data().kiosk.action.map(|a| a.id), Some(KIOSK_ACTIONS[0][side as usize]));
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().kiosk.stage == 1), "no monkey bar");
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::InAir), "never left the kiosk");
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground), "never landed: {:?}", s.body().feet);
    assert!(s.body().feet.y.abs() < 0.05, "on the street: {:?}", s.body().feet);
}

/// RE/24 NARROW-18: arriving at a kiosk along its axis is a lateral side: the side entry plays and, with no ground under
/// it, ends in a plain fall (no free-step jump; that function belongs to HumanRiding).
#[test]
fn a_lateral_kiosk_side_plays_its_entry_then_falls() {
    use crate::player::kiosk::{KioskEntry, KIOSK_ACTIONS};
    let target = Vec3::new(120.0, 3.0, 25.0);
    let mut s = Sim::new(target, -std::f32::consts::FRAC_PI_2);
    force(&mut s, crate::player::TransitionSetup::ToKiosk(KioskEntry { from: target, target, axis: Vec3::X, dir: Vec3::X, facing: Vec3::X }));
    s.run(1.0 / 60.0 + 1e-4);
    let side = s.data().kiosk.side;
    assert!(side.lateral(), "{side:?}");
    assert_eq!(s.data().kiosk.action.map(|a| a.id), Some(KIOSK_ACTIONS[0][side as usize]));
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir), "never left the kiosk");
    assert!(s.data().air.target.is_none() && matches!(s.data().air.mode, crate::player::air::AirMode::Fall { .. }), "a plain fall, not a jump");
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground), "never landed");
}

#[test]
fn the_ability_stack_gates_the_jump() {
    use crate::player::abilities::{Ability, AbilitySet};
    // AssassinAbilitySet (0xEF0320): without the Jump flag the tap jump is never requested (0xEE817E checks
    // `sub_D324C0`); with it, the same tap on open ground is the free jump
    for allowed in [false, true] {
        let mut s = Sim::new(Vec3::new(0.0, 0.0, 4.0), FACE_PZ);
        let flags = if allowed { u64::MAX } else { !(1u64 << Ability::Jump as u32) };
        s.app.world_mut().insert_resource(AbilitySet { flags, max_speed: 4, all_actions: true, high_profile: true });
        s.pad(Vec3::Z, 1.0, true, false);
        s.run(0.4);
        s.press_legs();
        let jumped = s.run_until(0.6, |s| s.loco().current == ActorContextId::InAir);
        assert_eq!(jumped, allowed);
    }
    // Grasp gates the InAir empty-hand catch (0xEDBC26): the same fall past a ladder with the hand held
    for allowed in [false, true] {
        let mut s = Sim::new(Vec3::new(50.0, 3.2, 62.9), FACE_PZ);
        let flags = if allowed { u64::MAX } else { !(1u64 << Ability::Grasp as u32) };
        s.app.world_mut().insert_resource(AbilitySet { flags, max_speed: 4, all_actions: true, high_profile: true });
        s.pad(Vec3::ZERO, 0.0, false, false);
        s.app.world_mut().resource_mut::<PadInput>().hand_held = true;
        assert_eq!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Ladder), allowed);
    }
}

#[test]
fn a_landing_starts_at_speed_0_and_hands_the_profile_speed_to_move() {
    // Movement_EnterBody 0xD92C40: landings (modes 9 / 10) enter at s = 0; LandingToMove_Layout6 0xD990A0 then sets the
    // profile's speed (RE/24 GROUND-5)
    let mut s = Sim::new(Vec3::new(-30.0, 2.5, -30.0), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ground && s.ground().owner == crate::player::ground::TreeState::Landing), "no landing");
    assert_eq!(s.ground().speed_param, 0.0);
    assert!(s.run_until(2.0, |s| s.ground().owner != crate::player::ground::TreeState::Landing), "the landing never ended");
    assert!(s.ground().speed_param >= 0.5, "Move at the high profile's speed: {}", s.ground().speed_param);
}

#[test]
fn sprint_needs_the_sprint_max_speed() {
    use crate::player::abilities::AbilitySet;
    // vt144 Sprint = MaxSpeed 4 and Legs (0xEE709F, RE/24 GROUND-22): with MaxSpeed Run the high-profile run never sprints
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.app.world_mut().insert_resource(AbilitySet { flags: u64::MAX, max_speed: 3, all_actions: true, high_profile: true });
    s.pad(Vec3::NEG_Z, 1.0, true, true);
    s.run(2.0);
    assert!(!s.ground().sprint);
    assert_ne!(speed_band(s.ground().speed_param), SpeedBand::Sprint);
}

#[test]
fn walk_speed_ability_keeps_the_ground_in_low_profile() {
    use crate::player::abilities::AbilitySet;
    // IHumanGround vt12 SetHighProfile = high && MaxSpeed > Walk (0xEE70F9, RE/24 INPUT-10)
    let mut s = Sim::new(Vec3::new(-30.0, 0.0, -30.0), 0.0);
    s.app.world_mut().insert_resource(AbilitySet { flags: u64::MAX, max_speed: 1, all_actions: true, high_profile: true });
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(1.5);
    assert!(!s.ground().high_profile);
    assert_eq!(speed_band(s.ground().speed_param), SpeedBand::Walk);
}

#[test]
fn a_hang_at_the_end_of_a_ledge_jumps_sideways_onto_a_ladder() {
    // the 2.6 m wall x 140..143 (face z 9.7) and the ladder at x 144.4 beside it: shimmying left (+X, facing +Z) to the
    // edge's end, `TryJumpUpOrTurnCorner` 0xDE05E0 tries the ladder before a ledge (0xDD55F0): ledge-jump type 3, then
    // the Ladder context (RE/18 §7.1)
    let mut s = hang_at(Vec3::new(142.2, 2.6, 9.7), Vec3::NEG_Z);
    s.pad(Vec3::X, 1.0, false, false);
    assert!(s.run_until(6.0, |s| s.loco().current == ActorContextId::Ladder), "never reached the ladder: {:?} ledge {}", s.loco().current, s.data().ledge.last_action);
    assert_eq!(s.data().ledge.last_action, "side jump to ladder");
    let f = s.body().feet;
    assert!((f.x - 144.4).abs() < 0.1 && (f.z - 9.2).abs() < 0.1, "on the ladder: {f:?}");
}

/// Every ledge jump of 0xDDD490 / 0xDD8CF0 has its clips (bug-1791664645: the climb-hold and ladder rows, wall and free
/// hang, were not dumped, so those jumps slid on a timer under the hang's wait).
#[test]
fn every_ledge_jump_action_has_its_clips() {
    use crate::player::ledge_moves::LEDGE_JUMP_TABLE;
    let missing: Vec<u32> = LEDGE_JUMP_TABLE.iter().flatten().flat_map(|(_, a)| *a).filter(|&id| id != 0 && crate::player::jump_blend::action_items(id).is_none()).collect();
    assert!(missing.is_empty(), "no clips for {missing:#x?}");
}

#[test]
fn a_hang_at_the_end_of_a_ledge_jumps_sideways_onto_climb_holds() {
    use crate::player::ledge_moves::LEDGE_JUMP_TABLE;
    // the 2.6 m wall x -140..-137 (face z -135.3) and the climb wall x -135.4..-131.4 beside it: shimmying left (+X, facing +Z)
    // to the edge's end, the side jump (0xDDD490) finds the 2.4 m band with the 1.2 m band below it for the feet →
    // type 0: the start in the Ledge context, the loop in the Climb context (RE/24 LEDGE-14)
    let mut s = hang_at(Vec3::new(-137.8, 2.6, -135.3), Vec3::NEG_Z);
    s.pad(Vec3::X, 1.0, false, false);
    // the jump plays its own start and loop clips (bug-1791664645: they were not in the clip table, so the root slid
    // there on a timer under the hang's wait)
    assert!(s.run_until(6.0, |s| s.data().ledge.last_action == "side jump to climb"), "no side jump: {}", s.data().ledge.last_action);
    let mv = s.data().ledge.mv.expect("the side jump");
    let starts: Vec<u32> = (0..2).flat_map(|f| [2, 3, 6, 7].map(|k| LEDGE_JUMP_TABLE[f][k].1[0])).collect();
    // the start on its own displacement here, the loop in the Climb context (RE/24 LEDGE-14)
    assert!(mv.seq[0].is_some_and(|a| starts.contains(&a.id)) && mv.table == Some((1, 0)), "the jump's clips: {:?}", mv.seq);
    assert!(s.run_until(6.0, |s| s.loco().current == ActorContextId::Climb), "never reached the climb: {:?} ledge {}", s.loco().current, s.data().ledge.last_action);
    assert_eq!(s.data().ledge.last_action, "side jump to climb");
    s.pad(Vec3::X, 0.0, false, false);
    s.run(2.0);
    let c = &s.data().climb;
    assert_eq!(s.loco().current, ActorContextId::Climb);
    assert!(c.hand_l.x > -135.4 && (c.hand_l.y - 2.4).abs() < 0.05 && (c.foot_l.y - 1.2).abs() < 0.05, "on the climb wall: {:?} {:?}", c.hand_l, c.foot_l);
    let ends: Vec<u32> = (0..2).flat_map(|f| [LEDGE_JUMP_TABLE[f][2].1[2], LEDGE_JUMP_TABLE[f][3].1[2], LEDGE_JUMP_TABLE[f][6].1[2], LEDGE_JUMP_TABLE[f][7].1[2]]).collect();
    assert!(ends.iter().any(|&e| e != 0), "type-0 side entries exist");
}

#[test]
fn climbing_down_past_the_lowest_holds_grabs_a_hang() {
    use crate::player::climb::{ClimbEntry, ClimbEntryType, LEDGE_GRAB_DOWN};
    use crate::player::{switch_context, TransitionSetup};
    // TryLedgeGrab 0xDF0980 (down): the wall x -130..-126 has bands from 3.0 m up. With the feet on the lowest band the
    // SHORT move down finds no foot hold and nothing below to reach, so the band between the feet and the hands (row 3)
    // becomes a wall hang (`LEDGE_GRAB_DOWN`, RE/18 §7.3)
    let (hands, feet_y) = (Vec3::new(-128.0, 4.8, -135.38), 3.6);
    let mut s = Sim::new(Vec3::new(-128.0, 3.5, -135.8), FACE_PZ);
    {
        let player = s.player;
        let w = s.app.world_mut();
        let mut q = w.query::<(&mut Locomotion, &mut HumanDataBundle)>();
        let (mut loco, mut data) = q.get_mut(w, player).unwrap();
        let foot = Vec3::new(hands.x, feet_y, hands.z);
        let e = ClimbEntry {
            entry_type: ClimbEntryType::Default,
            pose_2m: None,
            hand_l: hands,
            hand_r: hands,
            foot_l: foot,
            foot_r: foot,
            normal: Vec3::NEG_Z,
            from_feet: Vec3::new(-128.0, 3.5, -135.8),
            foot_right: false,
            action: None,
        };
        switch_context(&mut loco, &mut data, TransitionSetup::ToClimb(e));
    }
    s.run(0.5);
    assert_eq!(s.loco().current, ActorContextId::Climb);
    s.pad(Vec3::NEG_Z, 0.45, true, false);
    assert!(s.run_until(10.0, |s| s.loco().current == ActorContextId::Ledge), "never hung: {} {:?}", s.data().climb.last_action, s.data().climb.foot_l);
    assert_eq!(s.data().climb.last_action, "ledge grab");
    let l = &s.data().ledge;
    assert!((l.hand_l.y - 3.6).abs() < 0.05, "hanging from the 3.6 m band: {:?}", l.hand_l);
    assert_eq!(l.mv.and_then(|m| m.seq[0]).map(|a| a.id), Some(LEDGE_GRAB_DOWN[0]), "the wall-hang grab plays");
}

#[test]
fn climbing_sideways_hangs_from_a_wall_top_at_the_hands() {
    use crate::player::climb::CLIMB_TO_HANG;
    // TrySideLedgeGrab 0xDF22C0 (near): block Z stands square to wall Y on the left (+X), its top edge (3.6 m, facing -X)
    // level with the hands and no holds for the feet: pushing left hangs from it (`xx_l_climb_1m_l_hangwall`)
    let mut s = climb_at(-116.9, 2.4, -135.58);
    s.pad(Vec3::X, 0.45, true, false);
    assert!(s.run_until(6.0, |s| s.loco().current == ActorContextId::Ledge), "never hung: {} {:?}", s.data().climb.last_action, s.data().climb.foot_l);
    assert_eq!(s.data().climb.last_action, "side ledge grab");
    let l = &s.data().ledge;
    assert!(l.normal.x < -0.99 && (l.hand_l.y - 3.6).abs() < 0.05, "on block Z's edge: {:?} {:?}", l.normal, l.hand_l);
    assert_eq!(l.mv.and_then(|m| m.seq[0]).map(|a| a.id), Some(CLIMB_TO_HANG[0][4]), "`_l_hangwall`");
}

#[test]
fn free_running_onto_a_haystack_carts_rim_drops_into_the_haystack() {
    use crate::player::hay::HayEntry;
    // live 2026-10-09: the free run hops onto the cart's 1.2 m rim, then the support search of the free-step arrival
    // (NarrowObject `CheckSupportAndFall` 0xE51190) finds the haystack and drops in as FreeStep (A2 test yard)
    let mut s = Sim::new(Vec3::new(-104.0, 0.0, -126.0), FACE_PZ);
    s.pad(Vec3::Z, 1.0, true, true);
    let on_rim = std::cell::Cell::new(false);
    let ok = s.run_until(5.0, |s| {
        if s.loco().current == ActorContextId::Ground && s.body().feet.y > 1.1 {
            on_rim.set(true);
        }
        s.loco().current == ActorContextId::HayStack
    });
    assert!(ok, "never got in: {:?} {:?}", s.loco().current, s.body().feet);
    assert!(on_rim.get(), "landed on the rim first");
    assert_eq!(s.data().hay.kind, HayEntry::FreeStep);
}

#[test]
fn falling_past_climb_holds_with_the_hand_held_catches_into_the_climb() {
    use crate::player::air_catches::CLIMB_CATCH;
    // CheckAirCatch 0xE0BB70 → FindLedgeCatch 0xE0A990 (hands at the feet + 1.4 m) with holds for the feet 1.2 m below
    // (sub_E0A810) → the climb catch, type 1: the Climb context (the tower face z 27, bands every 0.6 m)
    let from = Vec3::new(-20.0, 6.0, 26.45);
    let mut s = Sim::new(from, FACE_PZ);
    force(&mut s, crate::player::TransitionSetup::ToInAir(air::InAirEntry::Fall { from, velocity: Vec3::ZERO, origin: air::FallOrigin::Ground, speed_param: 0.0 }));
    s.pad(Vec3::ZERO, 0.0, false, false);
    s.app.world_mut().resource_mut::<PadInput>().hand_held = true;
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Climb), "no climb catch: {:?} {:?}", s.loco().current, s.body().feet);
    let c = &s.data().climb;
    assert!((c.hand_l.y - c.foot_l.y - 1.2).abs() < 0.05, "hands 1.2 m above the feet: {:?} {:?}", c.hand_l, c.foot_l);
    assert!(c.move_action.is_some_and(|a| CLIMB_CATCH.contains(&a)), "the climb catch plays: {:?}", c.move_action);
    // RE/24 AIR-15: the pose by the hands' spacing (0.3 m), the root at the pose after the catch's item 0 + 0.2 s
    let want = if (c.hand_l - c.hand_r).length() >= 0.3 { 3 } else { 0 };
    assert_eq!(c.pose, want, "pose by the hands' spacing");
    let m = c.moving.expect("the catch's root interpolation");
    let reach = crate::player::ledge::catch_root_time(c.move_action.unwrap());
    assert!((m.reach - reach.min(m.duration)).abs() < 1e-4 && m.reach < m.duration, "reach {} of {}", m.reach, m.duration);
}

#[test]
fn a_long_fall_beside_a_free_hang_edge_catches_one_handed_to_the_side() {
    use crate::player::air_catches::FREE_CATCH_SIDE;
    // sub_DFFE30 (fall of 3 m or more): one hold at the feet + 1.95 m, ahead along the reach; the balcony slab's south
    // edge (z 35.4, 2.9 m, no wall below) lies 90° to the side of the facing → the side free-hang catch, one hand
    // facing +X, the stick toward the slab (+Z): the manual grab's box lies along the stick, 0.5 m ahead
    let from = Vec3::new(1.0, 5.2, 34.9);
    let mut s = Sim::new(from, -std::f32::consts::FRAC_PI_2);
    force(&mut s, crate::player::TransitionSetup::ToInAir(air::InAirEntry::Fall { from, velocity: Vec3::ZERO, origin: air::FallOrigin::Ground, speed_param: 0.0 }));
    s.pad(Vec3::Z, 0.6, false, false);
    s.app.world_mut().resource_mut::<PadInput>().hand_held = true;
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Ledge), "no catch: {:?} {:?}", s.loco().current, s.body().feet);
    s.run(0.05);
    let l = &s.data().ledge;
    assert!(l.catch_action.is_some_and(|a| FREE_CATCH_SIDE.contains(&a)), "a side catch: {:?}", l.catch_action.map(|a| format!("{a:#x}")));
    assert!(l.catch_one_hand.is_some());
    assert_eq!(l.hang_type, crate::player::ledge::LedgeHangType::Free);
}

#[test]
fn landing_on_a_roofs_lip_steps_off_the_edge() {
    use crate::player::air_catches::STEP_OFF;
    // InAir sub-state 3 (0xE0D684): a flat landing with the feet within 0.15 m of the roof edge (roof x 38..44, z
    // 9..15, 3 m) → the step-off action, then the fall to the street
    // facing the drop (-Z): `step_off_back` (its root steps 0.48 m forward), then the fall to the street
    let from = Vec3::new(41.0, 5.0, 9.1);
    let mut s = Sim::new(from, 0.0);
    force(&mut s, crate::player::TransitionSetup::ToInAir(air::InAirEntry::Fall { from, velocity: Vec3::ZERO, origin: air::FallOrigin::Ground, speed_param: 0.0 }));
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(1.5, |s| s.data().air.edge_landed), "no edge landing: {:?} {:?}", s.loco().current, s.body().feet);
    assert_eq!(s.data().air.flight.map(|a| a.id), Some(STEP_OFF[1]), "step_off_back");
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground && s.body().feet.y < 0.1), "never reached the street: {:?}", s.body().feet);
    // facing into the roof (+Z): `step_off_front` steps 0.48 m backward off the lip; falling in front of the face with
    // its top at the feet, the knee catch (type 0) takes the edge again and stands up
    let mut s = Sim::new(from, FACE_PZ);
    force(&mut s, crate::player::TransitionSetup::ToInAir(air::InAirEntry::Fall { from, velocity: Vec3::ZERO, origin: air::FallOrigin::Ground, speed_param: 0.0 }));
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(1.5, |s| s.data().air.edge_landed), "no edge landing facing the roof");
    assert_eq!(s.data().air.flight.map(|a| a.id), Some(STEP_OFF[0]), "step_off_front");
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground), "never stood again");
    assert!((s.body().feet.y - 3.0).abs() < 0.05, "back on the roof: {:?}", s.body().feet);
    // a 0.2 m drop onto the lip: the edge landing waits behind the catches' gate (a fall over 0.3 m or 0.4 s in the air,
    // `CheckAirCatch` LABEL_56, RE/24 AIR-16), so the body lands on the roof
    let low = Vec3::new(41.0, 3.2, 9.1);
    let mut s = Sim::new(low, 0.0);
    force(&mut s, crate::player::TransitionSetup::ToInAir(air::InAirEntry::Fall { from: low, velocity: Vec3::ZERO, origin: air::FallOrigin::Ground, speed_param: 0.0 }));
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Ground), "never landed: {:?}", s.body().feet);
    assert!(!s.data().air.edge_landed && (s.body().feet.y - 3.0).abs() < 0.05, "landed on the roof: {:?}", s.body().feet);
}


#[test]
fn edge_landing_finds_the_roof_edge_at_the_feet() {
    use crate::player::air_catches::{edge_landing, STEP_OFF};
    let (collision, guidance) = crate::level::geometry();
    // the roof x 38..44, z 9..15, 3 m: feet 0.1 m inside its south edge (normal -Z), facing +Z (away from the drop)
    let got = edge_landing(Vec3::new(41.0, 3.0, 9.1), Vec3::Z, &guidance, &collision);
    assert!(got.is_some(), "no edge at the feet");
    let (id, face) = got.unwrap();
    assert_eq!(id, STEP_OFF[0], "facing away from the drop: step_off_front");
    assert!(face.z > 0.9, "keeps facing away from the drop: {face:?}");
    // well inside the roof: nothing
    assert!(edge_landing(Vec3::new(41.0, 3.0, 12.0), Vec3::Z, &guidance, &collision).is_none());
}

#[test]
fn falling_in_front_of_a_knee_high_top_catches_it_and_stands_up() {
    use crate::player::air_catches::KNEE_CATCH;
    // CheckAirCatch type 0 (sub_E04630 / sub_B2E240): falling just in front of the 3 m roof's south face (x 38..44, z
    // 9..15) with its top at the feet → the knee catch, then the stand-up onto the roof
    let from = Vec3::new(41.0, 4.0, 8.62);
    let mut s = Sim::new(from, FACE_PZ);
    force(&mut s, crate::player::TransitionSetup::ToInAir(air::InAirEntry::Fall { from, velocity: Vec3::ZERO, origin: air::FallOrigin::Ground, speed_param: 0.0 }));
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::Ledge), "no knee catch: {:?} {:?}", s.loco().current, s.body().feet);
    assert!(s.data().ledge.mv.and_then(|m| m.seq[0]).is_some_and(|a| KNEE_CATCH.contains(&a.id)), "the knee catch plays");
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground), "never stood up: {:?}", s.loco().current);
    let f = s.body().feet;
    assert!((f.y - 3.0).abs() < 0.05 && f.z > 9.0, "standing on the roof: {f:?}");
}


#[test]
fn walking_and_running_off_an_edge_land_in_the_live_times() {
    // live 2026-10-09 (RE/18 §1): event 68's free jump down (0xDA7A10) lands a 1.80 m drop in 0.44–0.49 s and a 3.16 m
    // drop in 0.58–0.68 s, rising 0.01–0.05 m first. The port's low-profile walk-off is a little slower (0.52 s) and
    // rises 0.12 m (PORT: the takeoff clip's root; which clip the game plays there is not recorded).
    for (from, h, stick, high, (lo, hi)) in [
        (Vec3::new(70.0, 1.8, 60.5), 1.8, 1.0, true, (0.42, 0.50)),
        (Vec3::new(70.0, 1.8, 60.5), 1.8, 0.6, false, (0.42, 0.55)),
        (Vec3::new(7.0, 3.2, 23.0), 3.2, 1.0, true, (0.56, 0.70)),
    ] {
        let mut s = Sim::new(from, 0.0);
        s.pad(Vec3::NEG_Z, stick, high, false);
        assert!(s.run_until(5.0, |s| s.loco().current == ActorContextId::InAir), "never left the edge ({h} m)");
        let mut t = 0.0;
        let mut top = f32::MIN;
        while s.loco().current == ActorContextId::InAir && t < 3.0 {
            s.run(1.0 / 60.0 + 1e-4);
            t += 1.0 / 60.0;
            top = top.max(s.body().feet.y);
        }
        assert_eq!(s.loco().current, ActorContextId::Ground, "{h} m: landed");
        assert!(s.body().feet.y.abs() < 0.05, "{h} m: on the street {:?}", s.body().feet);
        assert!(t > lo && t < hi, "{h} m (stick {stick}, high {high}): {t:.3} s in the air");
        assert!(top - h < 0.15, "{h} m: rose {:.3} m", top - h);
    }
}


#[test]
fn the_pull_down_reception_blends_by_the_walls_lean() {
    use crate::collision::{Aabb3, CollisionWorld};
    use crate::player::ledge_moves::pulldown_wall_weights;
    // 0xDDF0AA: the line from the foot support up to the hands, from vertical. A plain wall: straight.
    let mut c = CollisionWorld::default();
    c.boxes.push(Aabb3 { min: Vec3::new(-2.0, 0.0, -1.0), max: Vec3::new(2.0, 3.0, 0.0) });
    assert_eq!(pulldown_wall_weights(Vec3::new(0.0, 3.0, 0.0), Vec3::Z, &c), [1.0, 0.0, 0.0]);
    // the wall 0.3 m further in at the feet (the hands 0.3 m out, 1 m up: 16.7°): toward 30_out
    let mut c = CollisionWorld::default();
    c.boxes.push(Aabb3 { min: Vec3::new(-2.0, 0.0, -1.0), max: Vec3::new(2.0, 2.5, -0.3) });
    c.boxes.push(Aabb3 { min: Vec3::new(-2.0, 2.5, -1.0), max: Vec3::new(2.0, 3.0, 0.0) });
    let w = pulldown_wall_weights(Vec3::new(0.0, 3.0, 0.0), Vec3::Z, &c);
    assert!((w[1] - 0.3f32.atan2(1.0) / 30f32.to_radians()).abs() < 0.02 && w[2] == 0.0, "{w:?}");
    // the wall 0.4 m further out at the feet (a plinth): toward 45_in
    let mut c = CollisionWorld::default();
    c.boxes.push(Aabb3 { min: Vec3::new(-2.0, 0.0, -1.0), max: Vec3::new(2.0, 2.5, 0.4) });
    c.boxes.push(Aabb3 { min: Vec3::new(-2.0, 2.5, -1.0), max: Vec3::new(2.0, 3.0, 0.0) });
    let w = pulldown_wall_weights(Vec3::new(0.0, 3.0, 0.0), Vec3::Z, &c);
    assert!((w[2] - 0.4f32.atan2(1.0) / 45f32.to_radians()).abs() < 0.02 && w[1] == 0.0, "{w:?}");
}

#[test]
fn a_pull_down_with_no_hands_on_the_edge_releases_into_a_fall() {
    use crate::player::ledge_moves::MoveKind;
    // PullDownSubState 4 (0xDDFCF0 → 0xDDA100): no edge for the hands at the descent → the orientation, then the
    // let-go into InAir (origin HangWall)
    let (p, n) = (Vec3::new(11.0, 3.5, 12.0), Vec3::X);
    let empty = crate::guidance::GuidanceWorld::default();
    let (c, _) = crate::level::geometry();
    let feet = Vec3::new(10.6, 3.5, 12.0);
    let orient = crate::player::ledge_moves::pulldown(p, n, feet, true, &empty, &c).expect_err("no hands");
    assert_eq!(orient.kind, MoveKind::PullDown { stage: 4 });
    let mut s = Sim::new(feet, -std::f32::consts::FRAC_PI_2);
    force(&mut s, crate::player::TransitionSetup::ToLedge(crate::player::ground::pulldown_release_entry(orient, n, feet)));
    assert!(s.run_until(1.5, |s| s.loco().current == ActorContextId::InAir), "never let go: {:?}", s.loco().current);
    assert_eq!(s.data().air.fall_origin, air::FallOrigin::HangWall);
    assert!(s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground && s.body().feet.y < 0.1), "fell to the street");
}

#[test]
fn vaulting_a_roofs_parapet_pulls_down_to_hang_on_the_far_side() {
    use crate::player::ledge_moves::MoveKind;
    use crate::player::passover::{PassOverPhase, PULLDOWN_ORIENT};
    // A2 yard: a 4 m roof with a 1 m parapet (z -118.3..-118.0, top 5 m) over a 5 m drop. The vault finds the far
    // edge, room at it and 3 m or more beyond (0xDD2BD0) → pull-down type 4 (0xDE0220), hanging on the far side
    let mut s = Sim::new(Vec3::new(-120.0, 4.0, -113.4), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, false);
    s.run(0.4);
    s.press_legs();
    assert!(s.run_until(0.6, |s| s.loco().current == ActorContextId::InAir), "no jump: {:?}", s.body().feet);
    assert_eq!(s.data().air.target.map(|t| t.type_flags), Some(2), "pass-over target");
    assert!(s.run_until(2.0, |s| s.data().ledge.pass_over.is_some_and(|p| p.phase == PassOverPhase::Vault)), "no vault: {:?} {:?}", s.loco().current, s.body().feet);
    assert!(s.run_until(2.0, |s| s.data().ledge.mv.is_some_and(|m| m.kind == MoveKind::PullDown { stage: 1 })), "no pull-down: {:?} {:?}", s.loco().current, s.body().feet);
    let m = s.data().ledge.mv.unwrap();
    assert!(m.seq[0].is_some_and(|a| PULLDOWN_ORIENT.contains(&a.id)), "the pass-over orientation");
    assert!(s.run_until(4.0, |s| s.data().ledge.mv.is_none() && s.data().ledge.queue.is_empty()), "never settled");
    let l = &s.data().ledge;
    assert_eq!(s.loco().current, ActorContextId::Ledge);
    assert!(l.normal.dot(Vec3::NEG_Z) > 0.99 && (l.hand_l.y - 5.0).abs() < 0.05, "hanging on the parapet's far face: {:?} {:?}", l.normal, l.hand_l);
    assert!(s.body().feet.z < -118.3, "on the far side: {:?}", s.body().feet);
}

#[test]
fn a_static_jump_at_a_wall_top_hangs_from_it() {
    // the jump-up wall (x 8..16, face z 41.7, top 2.6 m): standing at it in high profile, the static request finds
    // the top through 0xE165D0 (type 128: no wall above, a wall below) and jumps up into a wall hang
    let mut s = Sim::new(Vec3::new(12.0, 0.0, 41.1), FACE_PZ);
    s.pad(Vec3::ZERO, 0.0, true, true);
    s.press_legs();
    s.run(0.4);
    s.pad(Vec3::ZERO, 0.0, true, false);
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ledge), "no hang: {:?} {:?}", s.loco().current, s.body().feet);
    assert!(s.run_until(2.0, |s| s.data().ledge.mv.is_none()), "never settled");
    let l = &s.data().ledge;
    assert!((l.hand_l.y - 2.6).abs() < 0.05 && l.normal.dot(Vec3::NEG_Z) > 0.99, "hanging on the wall top: {:?} {:?}", l.hand_l, l.normal);
}

#[test]
fn falling_against_a_haystacks_side_enters_it_from_the_air() {
    use crate::player::hay::{HayEntry, HAYSTACK_FROM_AIR};
    // 0xE05490: a contact normal at most 0.9 up → SideJump (`xx_h_air_to_haystack`, 0xE42700). The haystack at
    // (37.5, 26), x 36.4..38.6, z 24.9..27.1, top 1.5 m: fall toward its -Z side from 1.2 m
    let from = Vec3::new(37.5, 1.2, 23.9);
    let mut s = Sim::new(from, FACE_PZ);
    force(&mut s, crate::player::TransitionSetup::ToInAir(air::InAirEntry::Fall { from, velocity: Vec3::new(0.0, 0.0, 4.0), origin: air::FallOrigin::Ground, speed_param: 0.0 }));
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::HayStack), "no entry: {:?} {:?}", s.loco().current, s.body().feet);
    assert_eq!(s.data().hay.kind, HayEntry::SideJump);
    assert_eq!(s.data().hay.action.map(|a| a.id), Some(HAYSTACK_FROM_AIR));
}

#[test]
fn free_running_onto_a_narrow_rim_drops_into_the_haystack_from_the_beam() {
    use crate::player::hay::HayEntry;
    // A2 yard: the cart's 0.3 m rim; the free run lands on it and the arrival's support search (0xE51190) drops in as
    // FreeStep during the reception, before any beam mount (RE/24 NARROW-4: the mounts wait for the reception's end;
    // the port plays that NarrowObject Movement stay in Ground)
    let mut s = Sim::new(Vec3::new(-112.0, 0.0, -119.5), 0.0);
    s.pad(Vec3::NEG_Z, 1.0, true, true);
    let on_rim = std::cell::Cell::new(false);
    let ok = s.run_until(5.0, |s| {
        if matches!(s.loco().current, ActorContextId::NarrowObject | ActorContextId::Ground) && s.body().feet.y > 1.1 {
            on_rim.set(true);
        }
        s.loco().current == ActorContextId::HayStack
    });
    assert!(ok, "never got in: {:?} {:?}", s.loco().current, s.body().feet);
    assert!(on_rim.get(), "landed on the rim first");
    assert_eq!(s.data().hay.kind, HayEntry::FreeStep);
}

fn on_ladder(base: Vec3, top: Vec3, h: f32) -> Sim {
    use crate::player::ladder::{LadderEntry, ATTACH_OUT};
    let n = Vec3::NEG_Z;
    let from = base + Vec3::Y * h + n * ATTACH_OUT;
    let mut s = Sim::new(from, FACE_PZ);
    force(&mut s, crate::player::TransitionSetup::ToLadder(LadderEntry { base, top, n, from, facing: -n, from_ledge: true, height: Some(h), ..Default::default() }));
    s.pad(Vec3::ZERO, 0.0, false, false);
    s.run(0.6);
    assert_eq!(s.loco().current, ActorContextId::Ladder);
    s
}

#[test]
fn pushing_sideways_on_a_ladder_jumps_onto_the_climb_wall_beside_it() {
    use crate::player::ladder::SIDE_JUMP;
    // event 4 (0xE27610 → sub_E228E0): the ladder at x -100 and wall W (x -105.95..-102.05, bands every 0.6 m) 2 m to
    // its right (facing +Z): the side jump onto its holds, the ladder's own table, then the Climb context
    let mut s = on_ladder(Vec3::new(-100.0, 0.0, -10.5), Vec3::new(-100.0, 7.0, -10.5), 2.0);
    s.pad(Vec3::NEG_X, 1.0, false, false);
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Climb), "no side jump: {:?}", s.loco().current);
    let starts: Vec<u32> = SIDE_JUMP[0].iter().map(|f| f[1][0]).collect();
    assert!(s.data().climb.move_action.is_some_and(|a| starts.contains(&a)), "the ladder's start (right): {:?}", s.data().climb.move_action.map(|a| format!("{a:#x}")));
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().climb.reach.is_none() && s.data().climb.moving.is_none()), "never settled");
    let c = &s.data().climb;
    assert_eq!(s.loco().current, ActorContextId::Climb);
    assert!(c.hand_l.x < -102.0 && c.hand_r.x < -102.0, "on wall W: {:?} {:?}", c.hand_l, c.hand_r);
}

#[test]
fn pushing_sideways_on_a_ladder_jumps_into_a_hang_on_the_ledge_beside_it() {
    use crate::player::ledge::LedgeHangType;
    // event 4: the ladder at x 144.4 and the 2.6 m wall (x 140..143) 1.4 m to its right: no ledge within the side move's
    // 0.8 m (sub_E1FC80), so the side jump (sub_E228E0) into a wall hang on its top edge
    let mut s = on_ladder(Vec3::new(144.4, 0.0, 9.7), Vec3::new(144.4, 6.0, 9.7), 1.5);
    s.pad(Vec3::NEG_X, 1.0, false, false);
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Ledge), "no side jump: {:?}", s.loco().current);
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none() && s.data().ledge.queue.is_empty()), "never settled");
    let l = &s.data().ledge;
    assert_eq!(s.loco().current, ActorContextId::Ledge);
    assert!((l.hand_l.y - 2.6).abs() < 0.05 && l.hand_l.x < 143.05 && l.hand_r.x < 143.05, "hanging on the wall top: {:?} {:?}", l.hand_l, l.hand_r);
    assert_eq!(l.hang_type, LedgeHangType::Wall);
}

#[test]
fn pushing_sideways_on_a_ladder_moves_onto_the_ledge_right_beside_it() {
    use crate::player::ladder::TO_HANG_SIDE;
    // event 4's first search (sub_E1FC80): the A2 yard's ladder at x -131.2 and the 3 m wall's top edge ending 0.8 m to
    // its right: `ladder_wait_tr_hangwall_right` while the root goes to the wall hang
    let mut s = on_ladder(Vec3::new(-131.2, 0.0, -116.6), Vec3::new(-131.2, 6.0, -116.6), 1.5);
    s.pad(Vec3::NEG_X, 1.0, false, false);
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::Ledge), "no side move: {:?}", s.loco().current);
    assert_eq!(s.data().ledge.mv.and_then(|m| m.seq[0]).map(|a| a.id), Some(TO_HANG_SIDE[0][1]));
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(3.0, |s| s.data().ledge.mv.is_none()), "never settled");
    let l = &s.data().ledge;
    assert!((l.hand_l.y - 3.0).abs() < 0.05 && l.hand_r.x < -131.95, "hanging on the 3 m top: {:?} {:?}", l.hand_l, l.hand_r);
}

#[test]
fn pushing_sideways_on_a_free_standing_ladder_swings_round_to_its_back() {
    use crate::player::ladder::{LadderPhase, REVOLVE};
    // event 5 (guard 0xE262B0): nothing behind the A2 yard's free-standing ladder, so the sideways stick in the wait
    // swings round it (`xx_h_ladder_turn_r`, 0xE1D730) and the climb carries on from the far side (0xE21EF0)
    let mut s = on_ladder(Vec3::new(-125.0, 0.0, -121.0), Vec3::new(-125.0, 5.0, -121.0), 1.5);
    s.pad(Vec3::NEG_X, 1.0, false, false);
    assert!(s.run_until(0.5, |s| matches!(s.data().ladder.phase, Some(LadderPhase::Revolve(_)))), "no revolve");
    assert_eq!(s.data().ladder.action.map(|a| a.id), Some(REVOLVE[1]));
    s.pad(Vec3::ZERO, 0.0, false, false);
    assert!(s.run_until(2.0, |s| s.data().ladder.phase == Some(LadderPhase::Wait)), "never settled");
    let (l, f) = (&s.data().ladder, s.body().feet);
    assert!(l.n.dot(Vec3::Z) > 0.99, "climbing from the back: {:?}", l.n);
    assert!((f.z + 120.5).abs() < 0.05 && (f.x + 125.0).abs() < 0.05, "0.5 m out on the far side: {f:?}");
    assert!(s.body().forward().dot(Vec3::NEG_Z) > 0.99, "facing the ladder: {:?}", s.body().forward());
    // climbing on from there reaches the top
    s.pad(Vec3::NEG_Z, 1.0, false, false);
    assert!(s.run_until(6.0, |s| s.data().ladder.height > 2.4), "climbs from the back side");
}

#[test]
fn a_ladder_leaning_over_the_climber_swings_him_onto_its_top_side() {
    use crate::player::ladder::{LadderPhase, REVOLVE};
    // `HumanLadder__TryTurn` 0xE26440: on the underside of a ladder leaning 18°, the climb state turns round to the
    // top side at once (room to the left: `turn_l`), then the climb goes up onto the block
    let mut s = on_ladder(Vec3::new(-137.0, 0.0, -121.0), Vec3::new(-137.0, 4.5, -122.5), 1.0);
    let saw = std::cell::Cell::new(false);
    assert!(s.run_until(2.0, |s| {
        if s.data().ladder.action.is_some_and(|a| a.id == REVOLVE[0]) {
            saw.set(true);
        }
        saw.get() && s.data().ladder.phase == Some(LadderPhase::Wait)
    }), "no turn: {:?}", s.data().ladder.phase);
    assert!(s.data().ladder.n.dot(Vec3::Z) > 0.99, "on the top side: {:?}", s.data().ladder.n);
    s.pad(Vec3::NEG_Z, 1.0, false, false);
    assert!(s.run_until(10.0, |s| s.loco().current == ActorContextId::Ground), "never reached the top: {:?} {:?}", s.data().ladder.phase, s.body().feet);
    assert!((s.body().feet.y - 4.5).abs() < 0.1 && s.body().feet.z < -122.6, "on the block: {:?}", s.body().feet);
}



