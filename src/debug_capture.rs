//! Debug helpers for automated verification (no effect unless the env vars are set):
//! - `AC_AUTOPILOT=roofs|climb|ledge` (or `1` = roofs): scripted input for a scenario.
//!   roofs: sprint across the rooftops · climb: climb the tower · ledge: jump to the balcony and shimmy.
//! - `AC_SCREENSHOT=path.png` (+ optional `AC_SCREENSHOT_AT=seconds`, default 3): save a screenshot
//!   of the primary window at that time, then exit.

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use crate::input::PadInput;
use crate::player::{Body, Player};

pub struct DebugCapturePlugin;

#[derive(Resource, Clone, Copy, PartialEq, Eq)]
enum Scenario {
    /// The wall run's start at a wall picked by `AC_KICK_X` (70: the pull-up from the entry, 76: the vertical step
    /// and hang, 82: no ledge), stick held into the wall.
    KickUp,
    /// Run in high profile and tap Legs every 1.6 s: running jumps without a target and their landings.
    Jumps,
    /// Walk from the loaded map spawn, then press Legs in high profile (native-map hitch reproduction).
    NativeFreerun,
    Roofs,
    Climb,
    Ledge,
    /// Stand still; camera in front (model check).
    Pose,
    /// Stand still; camera behind at an angle.
    Back,
    /// Locomotion on flat ground toward +X, camera from the side.
    Walk,
    Run,
    Sprint,
    /// Jump up to the 2.6 m wall ledge (wall hang), hold, then pull up.
    WallHang,
    /// Grab wall C, side-jump to D, shimmy to D's end and turn its outer corner (RE/03 §7.6).
    LedgeMoves,
    /// Jump at wall F (free arrival), shimmy left: free → wall, then wall → free at the overhang (0xDE1060).
    HangSwitch,
    /// Stand at roof A's +X edge, press Legs in low profile: pull-down into a wall hang (0xDDE4D0).
    PullDown,
    /// Walk (low profile) into roof A's edge: ledge stop, step back.
    LedgeStop,
    /// Walk across the beam between the two 4 m platforms.
    Beam,
    /// Wall run at the 3.8 m wall: entry, vertical step, hang from the top edge.
    WallRun,
    /// Run off the high block's +X edge into the haystack (Leap of Faith), wait, hop out.
    Faith,
    /// Run across the kiosk roof (x 120, z 20) and tap-jump at the kiosk frame: entry, monkey bar, drop (RE/18 §4).
    Kiosk,
    /// Walk into the haystack: the dive from the ground (event 122, RE/18 §3).
    HayDive,
    /// Free-run onto the first pilotis, hop along the posts, land on the far platform.
    Pilotis,
    /// Running jump onto the free beam, walk under the slab, impulsion, jump at its ledge.
    BeamJump,
    /// Walk into the 1.1 m wall and keep pushing (blocked, no lean: event 42 is unreachable, RE/02 §4.5), then walk
    /// off to the left.
    Lean,
    /// Stand at roof A's +X edge facing along it: the look-down to the side.
    LookDown,
    /// Stand, then push the stick straight behind: the pivot (Movement state 25).
    Pivot,
    /// Run (high profile, no Legs) off the 6 m block at an angle: ground loss into the drop sub-state.
    RunOff,
    /// Run, then pull the stick back: the run stop (skid), then the pivot.
    Skid,
    /// Run at the 1 m railing and jump: pass-over vault.
    PassOver,
    /// Jump at the first swing bar and swing from bar to bar onto the far platform.
    Swing,
    /// Walk to the ladder and climb it onto the wall top.
    Ladder,
    /// Walk (high profile, slow stick) off the 6 m block at an angle and fall.
    Drop,
    /// As Drop, holding grab (Legs) and the stick to the left while falling (the game's fall-grasp blend).
    DropGrab,
    /// Run at the 0.6 m box with high profile + Legs: the < 0.7 m straight-jump band steps onto it (0xB21DA0).
    StepUp,
    /// Stand still on the edge of the 0.3 m box, one foot over the edge (standing foot IK).
    FootIk,
    /// Climb the tower's left column into the gap: the climb's jump up to the overhang (TryBackEject 0xDF2F50).
    ClimbJump,
    Overhang,
    /// Climb wall M, then step right onto block N (`corner`) or left onto block O (`corner90`).
    Corner(bool),
    /// Climb wall P, then push left past its last holds: the long reach into a free hang on slab Q (TryReachOtherSurface
    /// 0xDF9B30) and the second hand's grab.
    ClimbReach,
    /// Climb → climb (index into `CLIMB_CLIMB`): climb to the 2.4 m band, then push left: 0 the reach across the gap
    /// (wall R1 → R2), 1 into the inside corner (S → T), 2 round the outside corner (wall U), 3 the reach onto the ladder
    /// (wall W); 4 climb wall V straight up, past its missing band (the reach up).
    ClimbClimb(usize),
}

/// The start x (facing +Z at z -11.4) of each `ClimbClimb` scenario.
const CLIMB_CLIMB: [f32; 5] = [-71.0, -81.0, -91.0, -102.8, -116.0];

impl Plugin for DebugCapturePlugin {
    fn build(&self, app: &mut App) {
        if let Ok(v) = std::env::var("AC_AUTOPILOT") {
            let sc = match v.as_str() {
                "climb" => Scenario::Climb,
                "native-freerun" => Scenario::NativeFreerun,
                "ledge" => Scenario::Ledge,
                "pose" => Scenario::Pose,
                "run" => Scenario::Run,
                "kickup" => Scenario::KickUp,
                "jumps" => Scenario::Jumps,
                "sprint" => Scenario::Sprint,
                "walk" => Scenario::Walk,
                "back" => Scenario::Back,
                "wallhang" => Scenario::WallHang,
                "ledgemoves" => Scenario::LedgeMoves,
                "hangswitch" => Scenario::HangSwitch,
                "pulldown" => Scenario::PullDown,
                "ledgestop" => Scenario::LedgeStop,
                "faith" => Scenario::Faith,
                "kiosk" => Scenario::Kiosk,
                "haydive" => Scenario::HayDive,
                "wallrun" => Scenario::WallRun,
                "beam" => Scenario::Beam,
                "pilotis" => Scenario::Pilotis,
                "lean" => Scenario::Lean,
                "passover" => Scenario::PassOver,
                "swing" => Scenario::Swing,
                "ladder" => Scenario::Ladder,
                "lookdown" => Scenario::LookDown,
                "pivot" => Scenario::Pivot,
                "runoff" => Scenario::RunOff,
                "skid" => Scenario::Skid,
                "beamjump" => Scenario::BeamJump,
                "drop" => Scenario::Drop,
                "dropgrab" => Scenario::DropGrab,
                "stepup" => Scenario::StepUp,
                "footik" => Scenario::FootIk,
                "climbjump" => Scenario::ClimbJump,
                "overhang" => Scenario::Overhang,
                "corner" => Scenario::Corner(false),
                "corner90" => Scenario::Corner(true),
                "climbreach" => Scenario::ClimbReach,
                "climbgap" => Scenario::ClimbClimb(0),
                "cornerin" => Scenario::ClimbClimb(1),
                "cornerout" => Scenario::ClimbClimb(2),
                "climbladder" => Scenario::ClimbClimb(3),
                "climbup" => Scenario::ClimbClimb(4),
                _ => Scenario::Roofs,
            };
            app.insert_resource(sc)
                // Map initialization resets the player; scripted placement must run afterward.
                .add_systems(PostStartup, place.after(crate::map_menu::initialize))
                .add_systems(PreUpdate, autopilot.after(crate::input::read_pad));
        }
        if std::env::var("AC_TESTPOSE").is_ok() {
            app.add_systems(Update, test_pose);
        }
        if let Ok(path) = std::env::var("AC_SCREENSHOT") {
            let at: f32 = std::env::var("AC_SCREENSHOT_AT").ok().and_then(|s| s.parse().ok()).unwrap_or(3.0);
            app.insert_resource(Capture { path, at, taken: None }).add_systems(Update, capture);
        }
    }
}

#[derive(Resource)]
struct Capture {
    path: String,
    at: f32,
    taken: Option<f32>,
}

fn place(sc: Res<Scenario>, mut q: Query<&mut Body, With<Player>>, mut rig: ResMut<crate::camera::CameraRig>) {
    for mut b in &mut q {
        match *sc {
            Scenario::NativeFreerun => {
                b.heading = crate::player::heading_of(Vec3::NEG_X);
            }
            Scenario::Roofs => {
                b.feet = Vec3::new(7.0, 3.5, 12.0);
                b.heading = -std::f32::consts::FRAC_PI_2;
                rig.yaw = -0.9;
                rig.distance = 9.0;
                rig.pitch = -0.25;
            }
            Scenario::Climb => {
                b.feet = Vec3::new(-20.5, 0.0, 26.5);
                b.heading = std::f32::consts::PI;
                rig.yaw = std::f32::consts::PI + 0.6;
                rig.distance = 7.0;
                rig.pitch = 0.15;
            }
            Scenario::Ledge => {
                b.feet = Vec3::new(2.0, 0.0, 33.0);
                b.heading = std::f32::consts::PI;
                rig.yaw = std::f32::consts::PI - 0.5;
                rig.distance = 6.0;
                rig.pitch = 0.05;
            }
            Scenario::PullDown => {
                b.feet = Vec3::new(10.6, 3.5, 12.0);
                b.heading = -std::f32::consts::FRAC_PI_2; // facing +X, over the drop
                rig.yaw = 0.0;
                rig.distance = 6.0;
            }
            Scenario::Beam => {
                b.feet = Vec3::new(71.0, 4.0, 70.0);
                b.heading = -std::f32::consts::FRAC_PI_2; // facing +X, toward the beam
                rig.yaw = 0.0;
                rig.distance = 6.0;
            }
            Scenario::Ladder => {
                b.feet = Vec3::new(50.0, 0.0, 61.0);
                b.heading = std::f32::consts::PI;
                rig.yaw = std::f32::consts::FRAC_PI_2 + 0.5;
                rig.distance = 7.0;
            }
            Scenario::Swing => {
                b.feet = Vec3::new(60.0, 1.2, 85.0);
                b.heading = std::f32::consts::PI;
                rig.yaw = std::f32::consts::FRAC_PI_2;
                rig.distance = 8.0;
            }
            Scenario::PassOver => {
                b.feet = Vec3::new(40.0, 0.0, 9.5);
                b.heading = 0.0;
                rig.yaw = -1.2;
                rig.distance = 6.0;
            }
            Scenario::Lean => {
                b.feet = Vec3::new(30.0, 0.0, -1.0);
                b.heading = 0.0;
                rig.yaw = 0.0;
                rig.distance = 6.0;
            }
            Scenario::Skid => {
                b.feet = Vec3::new(-30.0, 0.0, -20.0);
                b.heading = 0.0;
                rig.yaw = -1.2;
                rig.distance = 6.0;
            }
            Scenario::RunOff | Scenario::Drop | Scenario::DropGrab => {
                // the 6 m block (x -14.5..-9.5, z 1.5..6.5): leave its +X edge at 65 deg to the normal (`OFF_EDGE`).
                // Straight at it, a drop of more than 5 m is a front edge and gets the ledge stop (0xEE8899), and in
                // low profile any drop over 2 m halts the walk (0xEE7C1A), so the falls go out at an angle in high profile.
                b.feet = Vec3::new(-11.0, 6.0, 2.0);
                b.heading = -std::f32::consts::FRAC_PI_2;
                rig.yaw = 0.0;
                rig.distance = 7.0;
            }
            Scenario::ClimbJump => {
                b.feet = Vec3::new(-22.0, 0.0, 26.5);
                b.heading = std::f32::consts::PI; // facing +Z, the tower face
                rig.yaw = std::f32::consts::PI + 1.2;
                rig.distance = 7.0;
                rig.pitch = 0.1;
            }
            Scenario::Corner(ninety) => {
                b.feet = Vec3::new(if ninety { -38.9 } else { -41.0 }, 0.0, -11.4);
                b.heading = std::f32::consts::PI; // facing +Z, wall M
                rig.yaw = std::f32::consts::PI + if ninety { -1.0 } else { 1.0 };
                rig.distance = 7.0;
                rig.pitch = 0.2;
            }
            Scenario::ClimbReach => {
                b.feet = Vec3::new(-56.8, 0.0, -11.4);
                b.heading = std::f32::consts::PI; // facing +Z, wall P
                rig.yaw = std::f32::consts::PI - 0.6;
                rig.distance = 7.0;
                rig.pitch = 0.2;
            }
            Scenario::ClimbClimb(i) => {
                b.feet = Vec3::new(CLIMB_CLIMB[i], 0.0, -11.4);
                b.heading = std::f32::consts::PI; // facing +Z
                rig.yaw = std::f32::consts::PI - 0.6;
                rig.distance = 7.0;
                rig.pitch = 0.2;
            }
            Scenario::Overhang => {
                b.feet = Vec3::new(-30.0, 0.0, -11.4);
                b.heading = std::f32::consts::PI; // facing +Z, wall K
                rig.yaw = std::f32::consts::PI + 1.3;
                rig.distance = 7.0;
                rig.pitch = 0.15;
            }
            Scenario::FootIk => {
                b.feet = Vec3::new(5.3, 0.3, 0.0);
                b.heading = 0.0; // facing -Z along the box's west edge: the left foot is over it
                rig.yaw = 0.0;
                rig.distance = 3.0;
                rig.pitch = 0.1;
            }
            Scenario::StepUp => {
                b.feet = Vec3::new(9.0, 0.0, -3.0);
                b.heading = std::f32::consts::PI; // facing +Z, toward the box
                rig.yaw = std::f32::consts::FRAC_PI_2;
                rig.distance = 5.0;
            }
            Scenario::Pivot => {
                b.feet = Vec3::new(-30.0, 0.0, -30.0);
                b.heading = 0.0;
                rig.yaw = -1.2;
                rig.distance = 5.0;
            }
            Scenario::LookDown => {
                b.feet = Vec3::new(2.7, 3.0, 12.0);
                b.heading = 0.0;
                rig.yaw = -0.6;
                rig.distance = 6.0;
            }
            Scenario::Pilotis => {
                b.feet = Vec3::new(69.0, 3.0, 80.0);
                b.heading = -std::f32::consts::FRAC_PI_2;
                rig.yaw = 0.0;
                rig.distance = 7.0;
            }
            Scenario::BeamJump => {
                b.feet = Vec3::new(79.0, 4.0, 70.0);
                b.heading = -std::f32::consts::FRAC_PI_2;
                rig.yaw = 0.0;
                rig.distance = 7.0;
            }
            Scenario::KickUp => {
                let x = std::env::var("AC_KICK_X").ok().and_then(|v| v.parse().ok()).unwrap_or(70.0);
                b.feet = Vec3::new(x, 0.0, 57.9);
                b.heading = std::f32::consts::PI;
                rig.yaw = std::f32::consts::FRAC_PI_2;
                rig.distance = 6.0;
            }
            Scenario::WallRun => {
                b.feet = Vec3::new(76.0, 0.0, 57.9);
                b.heading = std::f32::consts::PI; // facing +Z, at the wall
                rig.yaw = std::f32::consts::FRAC_PI_2;
                rig.distance = 6.0;
            }
            Scenario::Kiosk => {
                b.feet = Vec3::new(120.0, 4.5, 19.0);
                b.heading = std::f32::consts::PI;
                rig.yaw = std::f32::consts::FRAC_PI_2;
                rig.distance = 7.0;
            }
            Scenario::HayDive => {
                b.feet = Vec3::new(37.5, 0.0, 21.5);
                b.heading = std::f32::consts::PI;
                rig.yaw = std::f32::consts::FRAC_PI_2;
                rig.distance = 6.0;
            }
            Scenario::Faith => {
                b.feet = Vec3::new(30.5, 9.5, 26.0);
                b.heading = -std::f32::consts::FRAC_PI_2;
                rig.yaw = 0.0;
                rig.distance = 7.0;
            }
            Scenario::LedgeStop => {
                // the 6 m block: a drop of more than 5 m gets the ledge stop (0xEE8899); roof A (3.5 m) only halts
                b.feet = Vec3::new(-12.0, 6.0, 4.0);
                b.heading = -std::f32::consts::FRAC_PI_2;
                rig.yaw = 0.0;
                rig.distance = 6.0;
            }
            Scenario::HangSwitch => {
                b.feet = Vec3::new(61.5, 0.0, 48.6);
                b.heading = std::f32::consts::PI;
                rig.yaw = std::f32::consts::PI;
                rig.distance = 6.0;
            }
            Scenario::LedgeMoves => {
                b.feet = Vec3::new(22.0, 0.0, 48.6);
                b.heading = std::f32::consts::PI; // facing +Z, into wall C
                rig.yaw = std::f32::consts::PI;
                rig.distance = 6.0;
            }
            Scenario::WallHang => {
                b.feet = Vec3::new(12.0, 0.0, 40.6);
                b.heading = std::f32::consts::PI; // facing +Z, into the wall
            }
            Scenario::Walk | Scenario::Run | Scenario::Sprint | Scenario::Jumps => {
                b.feet = Vec3::new(-60.0, 0.0, -40.0);
                b.heading = -std::f32::consts::FRAC_PI_2; // facing +X
                rig.yaw = 0.0; // camera on the +Z side → side view
                rig.distance = 4.0;
                rig.pitch = -0.05;
            }
            Scenario::Pose | Scenario::Back => {
                b.feet = Vec3::new(-30.0, 0.0, -20.0);
                b.heading = std::f32::consts::PI; // facing +Z
                rig.yaw = if *sc == Scenario::Pose { 0.35 } else { std::f32::consts::PI + 0.6 };
                rig.distance = 3.2;
                rig.pitch = -0.08;
            }
        }
    }
}

/// The fall scenes' stick: 65 deg off the 6 m block's +X edge normal (not a front edge, `FRONT_COS` 60 deg).
const OFF_EDGE: Vec3 = Vec3::new(0.422_618_3, 0.0, 0.906_307_8);

fn autopilot(
    time: Res<Time>,
    sc: Res<Scenario>,
    mut pad: ResMut<PadInput>,
    q: Query<(&crate::player::Locomotion, &crate::player::HumanDataBundle, &Body), With<Player>>,
    mut since: Local<(u32, f32)>,
    mut level_at: Local<Option<f32>>,
    mut freerun_pressed: Local<bool>,
    mut exit: MessageWriter<AppExit>,
) {
    let t = time.elapsed_secs();
    // AC_QUIT_AT=<s>: end the run after that much game time
    if std::env::var("AC_QUIT_AT").ok().and_then(|v| v.parse::<f32>().ok()).is_some_and(|q| t >= q) {
        exit.write(AppExit::Success);
    }
    // seconds the narrow-object state has stayed the same (`since` = (state seq, time))
    let narrow = q.single().ok().map(|(l, d, b)| (l.current, d.narrow.seq, d.narrow.state, b.feet));
    let held = match narrow {
        Some((crate::player::ActorContextId::NarrowObject, seq, _, _)) => {
            if since.0 != seq {
                *since = (seq, t);
            }
            t - since.1
        }
        _ => 0.0,
    };
    pad.magnitude = 1.0;
    pad.speed01 = 1.0;
    match *sc {
        Scenario::NativeFreerun => {
            pad.dir = Vec3::NEG_X;
            pad.high_profile = t >= 2.0;
            pad.legs_held = t >= 2.0;
            if pad.legs_held && !*freerun_pressed {
                pad.legs_pressed_ago = 0.0;
                *freerun_pressed = true;
            }
        }
        Scenario::Pose | Scenario::Back | Scenario::FootIk => {
            pad.magnitude = 0.0;
            pad.speed01 = 0.0;
        }
        Scenario::Jumps => {
            pad.dir = Vec3::X;
            pad.high_profile = true;
            pad.legs_held = false;
            // the time since the last tap (a tap is let go at once)
            pad.legs_pressed_ago = if t >= 1.5 { (t - 1.5) % 1.6 } else { f32::INFINITY };
        }
        Scenario::Walk | Scenario::Run | Scenario::Sprint => {
            pad.dir = Vec3::X;
            pad.high_profile = *sc != Scenario::Walk;
            pad.legs_held = *sc == Scenario::Sprint;
        }
        Scenario::Roofs => {
            pad.dir = Vec3::X;
            pad.high_profile = true;
            pad.legs_held = true;
        }
        Scenario::Climb => {
            pad.dir = Vec3::Z;
            pad.high_profile = true;
            pad.legs_held = false;
            // climbing from the ground: the empty-hand press (event 50) until the climb has started
            if t < 1.0 && q.single().ok().is_none_or(|(l, _, _)| l.current != crate::player::ActorContextId::Climb) {
                pad.hand_pressed_ago = 0.0;
            }
        }
        Scenario::PullDown => {
            pad.magnitude = 0.0;
            pad.speed01 = 0.0;
            pad.high_profile = false;
            if (0.5..0.52).contains(&t) {
                pad.legs_pressed_ago = 0.0;
            }
        }
        Scenario::Beam => {
            pad.dir = Vec3::X;
            pad.high_profile = false;
            if t < 1.0 {
                pad.magnitude = 0.0;
                pad.speed01 = 0.0;
            }
        }
        Scenario::Ladder => {
            // up the ladder onto the wall top, then stand
            pad.high_profile = false;
            pad.dir = Vec3::Z;
            let on_top = q.single().ok().is_some_and(|(l, _, b)| l.current == crate::player::ActorContextId::Ground && b.feet.y > 4.9);
            pad.magnitude = if t > 1.0 && !on_top { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
        }
        Scenario::Swing => {
            pad.dir = Vec3::Z;
            pad.high_profile = true;
            let swinging = q.single().ok().is_some_and(|(_, d, _)| d.ledge.swing.is_some_and(|w| matches!(w.phase, crate::player::swing::SwingPhase::Cycle(_))));
            let run = t > 1.0 && t < 1.6;
            pad.magnitude = if run || swinging { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
            if (1.35..1.37).contains(&t) || swinging {
                pad.legs_pressed_ago = 0.0;
            }
        }
        Scenario::PassOver => {
            pad.dir = Vec3::NEG_Z;
            pad.high_profile = true;
            pad.magnitude = if t > 1.0 && t < 3.5 { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
            if (1.6..1.62).contains(&t) {
                pad.legs_pressed_ago = 0.0;
            }
        }
        Scenario::Lean => {
            // 1 s still, walk into the wall and keep pushing, then the stick to the left
            pad.high_profile = false;
            pad.dir = if t < 4.5 { Vec3::NEG_Z } else { Vec3::NEG_X };
            pad.magnitude = if t > 1.0 { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
        }
        Scenario::LookDown => {
            pad.magnitude = 0.0;
            pad.speed01 = 0.0;
        }
        Scenario::Skid => {
            pad.high_profile = true;
            pad.dir = if t < 2.5 { Vec3::NEG_Z } else { Vec3::Z };
            pad.magnitude = if t > 1.0 { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
        }
        Scenario::RunOff => {
            pad.high_profile = true;
            pad.legs_held = false;
            pad.dir = OFF_EDGE;
            pad.magnitude = if t > 1.0 { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
        }
        Scenario::ClimbJump => {
            // 1 s still, then grab the wall (the empty hand) and keep pushing up; let go once hanging
            let hanging = q.single().ok().is_some_and(|(l, _, _)| l.current == crate::player::ActorContextId::Ledge);
            pad.dir = Vec3::Z;
            pad.high_profile = true;
            // climbing from the ground: the empty-hand press (event 50), repeated until the climb has started
            pad.legs_held = false;
            if (1.0..2.0).contains(&t) && q.single().ok().is_none_or(|(l, _, _)| l.current != crate::player::ActorContextId::Climb) {
                pad.hand_pressed_ago = 0.0;
            }
            pad.magnitude = if t > 1.0 && !hanging { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
        }
        Scenario::Corner(ninety) => {
            // 1 s still, grab wall M, push up until both feet are on the 2.4 m band, then push sideways (light, SHORT
            // moves) until the climb ends standing on the block
            let state = q.single().ok().map(|(l, _, b)| (l.current, b.feet.y));
            let climbing = state.is_some_and(|(c, _)| c == crate::player::ActorContextId::Climb);
            let level = state.is_some_and(|(_, y)| y > 2.3);
            pad.high_profile = true;
            // climbing from the ground: the empty-hand press (event 50), repeated until the climb has started
            pad.legs_held = false;
            if (1.0..2.0).contains(&t) && q.single().ok().is_none_or(|(l, _, _)| l.current != crate::player::ActorContextId::Climb) {
                pad.hand_pressed_ago = 0.0;
            }
            if t < 1.0 || (!climbing && t > 2.0) {
                pad.magnitude = 0.0;
            } else if level {
                pad.dir = if ninety { Vec3::X } else { Vec3::NEG_X };
                pad.magnitude = 0.45;
            } else {
                pad.dir = Vec3::Z;
                pad.magnitude = if t < 1.4 { 1.0 } else { 0.45 };
            }
            pad.speed01 = pad.magnitude;
        }
        Scenario::ClimbReach => {
            // 1 s still, grab wall P, push up until both feet are on the 2.4 m band, then push left (light) until the
            // reach has hung the climber from slab Q
            let state = q.single().ok().map(|(l, _, b)| (l.current, b.feet.y));
            let climbing = state.is_some_and(|(c, _)| c == crate::player::ActorContextId::Climb);
            let level = state.is_some_and(|(_, y)| y > 2.3);
            pad.high_profile = true;
            // climbing from the ground: the empty-hand press (event 50), repeated until the climb has started
            pad.legs_held = false;
            if (1.0..2.0).contains(&t) && q.single().ok().is_none_or(|(l, _, _)| l.current != crate::player::ActorContextId::Climb) {
                pad.hand_pressed_ago = 0.0;
            }
            if t < 1.0 || (!climbing && t > 2.0) {
                pad.magnitude = 0.0;
            } else if level {
                pad.dir = Vec3::X;
                pad.magnitude = 0.45;
            } else {
                pad.dir = Vec3::Z;
                pad.magnitude = if t < 1.4 { 1.0 } else { 0.45 };
            }
            pad.speed01 = pad.magnitude;
        }
        Scenario::ClimbClimb(i) => {
            // 1 s still, grab the wall, push up until both feet are on the 2.4 m band, then push left (light) for 4 s; the
            // climb up (4) keeps pushing up for 6 s
            let state = q.single().ok().map(|(l, _, b)| (l.current, b.feet.y));
            let climbing = state.is_some_and(|(c, _)| c == crate::player::ActorContextId::Climb);
            let level = i != 4 && state.is_some_and(|(_, y)| y > 2.3);
            let since = &mut *level_at;
            if level && since.is_none() {
                *since = Some(t);
            }
            pad.high_profile = true;
            // climbing from the ground: the empty-hand press (event 50), repeated until the climb has started
            pad.legs_held = false;
            if (1.0..2.0).contains(&t) && q.single().ok().is_none_or(|(l, _, _)| l.current != crate::player::ActorContextId::Climb) {
                pad.hand_pressed_ago = 0.0;
            }
            if t < 1.0 || (!climbing && t > 2.0) {
                pad.magnitude = 0.0;
            } else if let Some(t0) = *since {
                pad.dir = Vec3::X;
                pad.magnitude = if t - t0 < 4.0 { 0.45 } else { 0.0 };
            } else {
                pad.dir = Vec3::Z;
                pad.magnitude = if t < 1.4 { 1.0 } else if t < 7.0 { 0.45 } else { 0.0 };
            }
            pad.speed01 = pad.magnitude;
        }
        Scenario::Overhang => {
            // 1 s still, grab wall K, then a light push up (SHORT moves) until the climb hangs from the bay
            let hanging = q.single().ok().is_some_and(|(l, _, _)| l.current == crate::player::ActorContextId::Ledge);
            pad.dir = Vec3::Z;
            pad.high_profile = true;
            // climbing from the ground: the empty-hand press (event 50), repeated until the climb has started
            pad.legs_held = false;
            if (1.0..2.0).contains(&t) && q.single().ok().is_none_or(|(l, _, _)| l.current != crate::player::ActorContextId::Climb) {
                pad.hand_pressed_ago = 0.0;
            }
            pad.magnitude = if t > 1.0 && !hanging { if t < 1.4 { 1.0 } else { 0.45 } } else { 0.0 };
            pad.speed01 = pad.magnitude;
        }
        Scenario::StepUp => {
            // 1 s still, then run at the box with high profile + Legs until on top
            let on_top = q.single().ok().is_some_and(|(_, _, b)| b.feet.y > 0.5);
            pad.dir = Vec3::Z;
            pad.high_profile = true;
            pad.legs_held = t > 1.0 && !on_top;
            pad.magnitude = if t > 1.0 && !on_top { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
        }
        Scenario::Pivot => {
            pad.high_profile = true;
            pad.dir = Vec3::Z;
            pad.magnitude = if t > 1.0 { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
        }
        Scenario::Pilotis => {
            use crate::player::narrow::BeamState;
            pad.dir = Vec3::X;
            pad.high_profile = true;
            let on_post = matches!(narrow, Some((crate::player::ActorContextId::NarrowObject, ..)));
            let wait = matches!(narrow, Some((_, _, BeamState::PilotisWait, _)));
            // run off P with Legs held (1 s still first); on a post: wait 0.8 s, then high profile + Legs + stick
            let go = (t > 1.0 && !on_post && narrow.is_some_and(|n| n.0 == crate::player::ActorContextId::Ground) && narrow.is_some_and(|n| n.3.x < 72.5)) || (wait && held > 0.8);
            pad.legs_held = go && !on_post;
            if wait && held > 0.8 && held < 0.82 {
                pad.legs_pressed_ago = 0.0;
            }
            pad.magnitude = if go || matches!(narrow, Some((crate::player::ActorContextId::InAir, ..))) { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
        }
        Scenario::BeamJump => {
            use crate::player::narrow::BeamState;
            pad.dir = Vec3::X;
            let ctx = narrow.map(|n| n.0);
            let x = narrow.map(|n| n.3.x).unwrap_or(0.0);
            let st = narrow.map(|n| n.2);
            let on = ctx == Some(crate::player::ActorContextId::NarrowObject);
            // run + Legs off platform B onto the beam, walk to x 87.4, then impulsion and the jump
            let run = t > 1.0 && ctx == Some(crate::player::ActorContextId::Ground) && x < 83.0;
            let walk = on && x < 87.3 && !matches!(st, Some(BeamState::ImpulseIn | BeamState::ImpulseWait | BeamState::JumpOnPlace));
            pad.high_profile = run || (on && !walk);
            pad.legs_held = run;
            pad.magnitude = if run || walk || ctx == Some(crate::player::ActorContextId::InAir) && x < 84.5 { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
            if on && !walk && ((st == Some(BeamState::Wait) && held > 0.5 && held < 0.52) || (st == Some(BeamState::ImpulseWait) && held > 0.6 && held < 0.62)) {
                pad.legs_pressed_ago = 0.0;
            }
        }
        Scenario::KickUp => {
            let t = t - 1.0;
            pad.dir = Vec3::Z;
            pad.high_profile = true;
            pad.legs_held = t > 0.2;
            // one press (a press that is consumed stays consumed)
            if t > 0.2 && !*freerun_pressed {
                pad.legs_pressed_ago = 0.0;
                *freerun_pressed = true;
            }
            if t < 0.0 {
                pad.magnitude = 0.0;
                pad.speed01 = 0.0;
            }
        }
        Scenario::WallRun => {
            // stand for 1 s first (the renderer's pipelines finish compiling), then go
            let t = t - 1.0;
            pad.dir = Vec3::Z;
            pad.high_profile = true;
            pad.legs_held = t > 0.2;
            if (0.2..0.22).contains(&t) {
                pad.legs_pressed_ago = 0.0;
            }
            if !(0.0..=0.85).contains(&t) {
                pad.magnitude = 0.0;
                pad.speed01 = 0.0;
            }
        }
        Scenario::Kiosk => {
            // run toward +Z, the Legs tap at 0.8 s (released at once: the tap jump, 0xEE817E)
            pad.dir = Vec3::Z;
            let run = t < 1.4;
            pad.magnitude = if run { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
            pad.high_profile = run;
            if (0.8..0.8 + 1.0 / 60.0).contains(&t) {
                pad.legs_pressed_ago = 0.0;
            }
        }
        Scenario::HayDive => {
            pad.dir = Vec3::Z;
            pad.magnitude = if t < 3.0 { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
        }
        Scenario::Faith => {
            pad.dir = Vec3::X;
            let run = t < 1.2;
            let hop = t > 6.0;
            pad.magnitude = if run || hop { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
            pad.high_profile = run;
            pad.legs_held = run;
        }
        Scenario::LedgeStop => {
            pad.dir = Vec3::X;
            pad.magnitude = if t < 2.5 { 1.0 } else { 0.0 };
            pad.speed01 = pad.magnitude;
            pad.high_profile = false;
        }
        Scenario::HangSwitch | Scenario::LedgeMoves => {
            // grab C (jump up), then hold the stick toward +X (the player's left): side jump to D, shimmy
            // along D, outer corner at D's end
            pad.high_profile = t < 0.6;
            pad.legs_held = (0.3..0.6).contains(&t);
            pad.dir = if t < 2.0 { Vec3::Z } else { Vec3::X };
            if (0.6..2.0).contains(&t) {
                pad.magnitude = 0.0;
                pad.speed01 = 0.0;
            }
        }
        Scenario::WallHang => {
            // into the wall with high profile, Legs at 0.4 s (jump up to the ledge), hang until 3.5 s,
            // then push up (pull-up)
            pad.dir = Vec3::Z;
            pad.high_profile = true;
            pad.legs_held = (0.3..0.6).contains(&t);
            if (0.6..3.5).contains(&t) {
                pad.magnitude = 0.0;
                pad.speed01 = 0.0;
            }
        }
        Scenario::Drop | Scenario::DropGrab => {
            // walk off (high profile, slow stick: low profile would halt at the edge); DropGrab then holds grab and
            // pushes the stick to the left once falling (the drop steer has turned the facing to +X, so -Z is left)
            let falling = q.single().ok().is_some_and(|(l, _, _)| l.current == crate::player::ActorContextId::InAir);
            let grab = *sc == Scenario::DropGrab && falling;
            pad.dir = if grab { Vec3::NEG_Z } else { OFF_EDGE };
            pad.high_profile = true;
            pad.legs_held = grab;
            pad.magnitude = if t > 1.0 { 0.5 } else { 0.0 };
            pad.speed01 = ((pad.magnitude - 0.35) / 0.65).max(0.0);
        }
        Scenario::Ledge => {
            // run at the balcony, jump (Legs press at 0.3 s), let go of the stick, then shimmy right (-X)
            pad.dir = if t < 1.0 { Vec3::Z } else { Vec3::NEG_X };
            if (1.0..1.6).contains(&t) {
                pad.magnitude = 0.0;
                pad.speed01 = 0.0;
            }
            pad.high_profile = t < 1.0;
            pad.legs_held = false;
            if (0.3..0.32).contains(&t) {
                pad.legs_pressed_ago = 0.0;
            }
        }
    }
    // `AC_STICK_OFF=a-b,c-d`: release the stick in these time windows (to hold still for inspection)
    if let Ok(v) = std::env::var("AC_STICK_OFF") {
        for w in v.split(',') {
            if let Some((a, b)) = w.split_once('-') {
                if let (Ok(a), Ok(b)) = (a.parse::<f32>(), b.parse::<f32>()) {
                    if (a..b).contains(&t) {
                        pad.magnitude = 0.0;
                        pad.speed01 = 0.0;
                    }
                }
            }
        }
    }
}

/// Skin-binding check: bend LeftArm, RightForeArm and LeftLeg away from their rest rotation.
fn test_pose(rig: Query<&crate::model::Rig>, mut joints: Query<&mut Transform>) {
    let Ok(rig) = rig.single() else { return };
    for (bone, angle) in [(0xeb83_0adau32, 1.0f32), (0x7257_a1aa, 1.2), (0x060d_f401, -1.1)] {
        if let Some(i) = rig.bone_ids.iter().position(|&b| b == bone) {
            if let Ok(mut t) = joints.get_mut(rig.joints[i]) {
                t.rotation = rig.rest[i].rotation * Quat::from_rotation_z(angle);
            }
        }
    }
}

// Real time allows captures and automatic exit while the debug map menu pauses virtual time.
fn capture(mut commands: Commands, time: Res<Time<Real>>, mut cap: ResMut<Capture>, mut exit: MessageWriter<AppExit>) {
    let t = time.elapsed_secs();
    match cap.taken {
        None if t >= cap.at => {
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(cap.path.clone()));
            cap.taken = Some(t);
        }
        Some(t0) if t - t0 > 1.0 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- multi-angle shots
// `AC_SHOTS=t1,t2,…` (+ `AC_SHOT_DIR=folder`, `AC_VIEWS=back,left,right,high,hands`): at each time the
// simulation freezes (virtual time paused), the camera visits every view (one frame each, positioned
// relative to the player's facing) and a screenshot `<dir>/<t>_<view>.png` is saved; then time resumes.
// Time advances in fixed 1/60 s steps so runs are reproducible.

#[derive(Resource)]
struct Shots {
    times: Vec<f32>,
    views: Vec<String>,
    dir: String,
    next: usize,
    view: Option<usize>,
    done_at: Option<u32>,
    frames: u32,
}

pub struct ShotsPlugin;

impl Plugin for ShotsPlugin {
    fn build(&self, app: &mut App) {
        let Ok(list) = std::env::var("AC_SHOTS") else { return };
        let mut times: Vec<f32> = list.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let views = std::env::var("AC_VIEWS").unwrap_or("back,left,right,high,hands".into()).split(',').map(String::from).collect();
        let dir = std::env::var("AC_SHOT_DIR").unwrap_or(".".into());
        let _ = std::fs::create_dir_all(&dir);
        let fps = std::env::var("AC_SHOT_FPS").ok().and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && (1.0..=240.0).contains(v)).unwrap_or(60.0);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f64(1.0 / fps)))
            .insert_resource(Shots { times, views, dir, next: 0, view: None, done_at: None, frames: 0 })
            .add_systems(Startup, fill_light)
            .add_systems(PostUpdate, shots.before(bevy::transform::TransformSystems::Propagate));
    }
}

#[allow(clippy::too_many_arguments)]
fn shots(
    mut commands: Commands,
    mut shots: ResMut<Shots>,
    mut vtime: ResMut<Time<Virtual>>,
    player: Query<(&Body, &crate::player::LimbTargets), With<Player>>,
    rig: Query<&crate::model::Rig>,
    globals: Query<&GlobalTransform>,
    mut cam: Query<&mut Transform, With<crate::camera::MainCamera>>,
    mut exit: MessageWriter<AppExit>,
) {
    shots.frames += 1;
    if let Some(f) = shots.done_at {
        if shots.frames > f + 30 {
            exit.write(AppExit::Success);
        }
        return;
    }
    let t = vtime.elapsed_secs();
    if shots.view.is_none() {
        if shots.next >= shots.times.len() {
            shots.done_at = Some(shots.frames);
            return;
        }
        if t + 1e-4 < shots.times[shots.next] {
            return;
        }
        vtime.pause();
        shots.view = Some(0);
    }
    let vi = shots.view.unwrap();
    let Ok((body, limbs)) = player.single() else { return };
    let fwd = body.forward();
    let right = crate::player::right_of(fwd);
    // focus: the hips joint (falls back to the body root)
    let hips = rig.single().ok().and_then(|r| r.bone_ids.iter().position(|b| *b == 0xded1_0611).and_then(|i| globals.get(r.joints[i]).ok()));
    let focus = hips.map(|g| g.translation()).unwrap_or(body.feet + Vec3::Y * 1.0);
    if std::env::var_os("AC_SHOT_LOG").is_some() {
        info!("shot t={t:.2} feet {:?} hips {:?}", body.feet, hips.map(|g| g.translation()));
    }
    let name = shots.views[vi].clone();
    let (target, eye) = match name.as_str() {
        "left" => (focus, focus - right * 3.0 + Vec3::Y * 0.3),
        "right" => (focus, focus + right * 3.0 + Vec3::Y * 0.3),
        "hem" => {
            let hem = body.feet + Vec3::Y * 0.4;
            (hem, hem + right * 1.2 + fwd * 0.3 + Vec3::Y * 0.1)
        }
        "high" => (focus, focus - fwd * 2.2 - right * 1.6 + Vec3::Y * 2.2),
        "overview" => (focus + fwd * 120.0 - Vec3::Y * 20.0, focus - fwd * 20.0 + Vec3::Y * 30.0),
        "front" => (focus, focus + fwd * 3.0 + Vec3::Y * 0.3),
        "fingers" | "fingers_side" => {
            let h = limbs.hands.map(|(l, r)| if l.y >= r.y { l } else { r }).unwrap_or(focus + Vec3::Y * 0.8);
            let side = if name == "fingers" { -fwd * 0.6 - right * 0.45 + Vec3::Y * 0.35 } else { -fwd * 0.3 - right * 0.7 + Vec3::Y * 0.1 };
            (h, h + side)
        }
        "hands" => {
            let h = limbs.hands.map(|(l, r)| (l + r) * 0.5).unwrap_or(focus + Vec3::Y * 0.8);
            (h, h - fwd * 1.5 - right * 0.8 + Vec3::Y * 0.6)
        }
        _ => (focus, focus - fwd * 3.2 + Vec3::Y * 0.6),
    };
    if let Ok(mut c) = cam.single_mut() {
        *c = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
    let path = format!("{}/{:05.2}_{}.png", shots.dir, shots.times[shots.next], name);
    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
    if vi + 1 >= shots.views.len() {
        shots.view = None;
        shots.next += 1;
        vtime.unpause();
    } else {
        shots.view = Some(vi + 1);
    }
}

/// Capture runs only: a shadowless fill light from the -Z side so walls facing away from the sun
/// (the climb tower face) are readable.
fn fill_light(mut commands: Commands) {
    commands.spawn((
        DirectionalLight { illuminance: 5_000.0, shadow_maps_enabled: false, ..default() },
        Transform::from_xyz(0.0, 10.0, -10.0).looking_at(Vec3::new(0.0, 0.0, 6.0), Vec3::Y),
    ));
}
