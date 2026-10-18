//! Debug HUD: current locomotion context, sub-state, speed band, last landing. Toggle with F1.

use bevy::prelude::*;

use crate::input::PadInput;
use crate::player::air::AirMode;
use crate::player::ground::speed_band;
use crate::player::{ActorContextId, Body, HumanDataBundle, Locomotion, Player};

#[derive(Component)]
struct HudText;

/// The frame-rate readout in the top-right corner.
#[derive(Component)]
struct FpsText;

/// Frames and real time since the readout last changed (it updates four times a second).
#[derive(Default)]
struct FpsWindow {
    frames: u32,
    elapsed: f32,
}

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud).add_systems(Update, (update_hud, update_fps, log_frame_times));
    }
}

fn spawn_hud(mut commands: Commands) {
    commands.spawn((
        HudText,
        Text::new(""),
        TextFont { font_size: FontSize::Px(15.0), ..default() },
        TextColor(Color::srgb(0.05, 0.05, 0.08)),
        Node { position_type: PositionType::Absolute, top: Val::Px(10.0), left: Val::Px(12.0), ..default() },
    ));
    commands.spawn((
        FpsText,
        Text::new(""),
        TextFont { font_size: FontSize::Px(18.0), ..default() },
        TextColor(Color::srgb(0.95, 0.95, 0.95)),
        // a dark backing drawn over the HUD, which can run under it in a narrow window
        BackgroundColor(Color::srgba(0.05, 0.05, 0.08, 0.92)),
        GlobalZIndex(10),
        Node { position_type: PositionType::Absolute, top: Val::Px(8.0), right: Val::Px(8.0), padding: UiRect::axes(Val::Px(8.0), Val::Px(3.0)), ..default() },
    ));
}

/// The frame rate over the last quarter second of real time (not the simulation clock, which screenshot runs fix),
/// with the frame time; hidden with the HUD under `AC_NO_HUD`.
fn update_fps(time: Res<Time<bevy::time::Real>>, mut window: Local<FpsWindow>, mut q: Query<(&mut Text, &mut Visibility), With<FpsText>>) {
    let Ok((mut text, mut vis)) = q.single_mut() else { return };
    if std::env::var_os("AC_NO_HUD").is_some() {
        *vis = Visibility::Hidden;
        return;
    }
    window.frames += 1;
    window.elapsed += time.delta_secs();
    if window.elapsed >= 0.25 {
        let fps = window.frames as f32 / window.elapsed;
        text.0 = format!("{fps:.0} FPS  ({:.1} ms)", 1000.0 / fps.max(1e-3));
        *window = FpsWindow::default();
    }
}

fn update_hud(
    keys: Res<ButtonInput<KeyCode>>,
    pad: Res<PadInput>,
    model: Res<crate::model::ModelStatus>,
    map: Res<crate::map_menu::MapMenu>,
    fly: Option<Res<crate::debug_fly::DebugFly>>,
    areas: Option<Res<crate::test_areas::TestAreas>>,
    bindings: Option<Res<crate::bindings::Bindings>>,
    catch_debug: Option<Res<crate::player::air_catches::CatchDebug>>,
    target_debug: Option<Res<crate::player::target_debug::TargetDebug>>,
    time: Res<Time>,
    q: Query<(&Locomotion, &Body, &HumanDataBundle), With<Player>>,
    mut hud: Query<(&mut Text, &mut Visibility), With<HudText>>,
) {
    let Ok((mut text, mut vis)) = hud.single_mut() else { return };
    // `AC_NO_HUD`: hidden (demo recordings)
    if std::env::var_os("AC_NO_HUD").is_some() {
        if let Ok((_, mut vis)) = hud.single_mut() {
            *vis = Visibility::Hidden;
        }
        return;
    }
    if keys.just_pressed(KeyCode::F1) {
        *vis = if *vis == Visibility::Hidden { Visibility::Inherited } else { Visibility::Hidden };
    }
    let Ok((loco, body, data)) = q.single() else { return };
    let (g, air) = (&data.ground, &data.air);
    let context_detail = match loco.current {
        ActorContextId::Ledge => format!(
            "ledge: {:?} / {:?} hang / alt-hand {} / {}",
            data.ledge.sub_state, data.ledge.hang_type, data.ledge.alt_flag as u8, data.ledge.last_action
        ),
        ActorContextId::Climb => format!(
            "climb: pose {} / entry {:?} / {}",
            data.climb.pose, data.climb.entry_type, data.climb.last_action
        ),
        _ => format!("fall origin {:?}", air.fall_origin),
    };
    let air_mode = match air.mode {
        AirMode::Jump { .. } => "Jump (target-warped clip)",
        AirMode::Fall { steer_to: Some(_) } => "Fall (steered to target)",
        AirMode::Fall { steer_to: None } => "Fall",
        AirMode::Idle => "-",
    };
    let landing = g
        .last_landing
        .map(|l| format!("{:?}  fall {:.2} m  drop {:.2} m{}", l.kind, l.fall_height, l.total_drop, if l.roll { "  (roll)" } else { "" }))
        .unwrap_or_else(|| "-".into());
    // the air catch's state (F7 draws its boxes, `air_catches::CatchDebug`)
    let grab_line = match catch_debug.as_deref() {
        Some(d) if d.show => d
            .current(time.elapsed_secs())
            .map(|t| t.summary())
            .unwrap_or_else(|| "grab: - (hold the empty hand while falling; F7 hides the boxes)".into()),
        Some(_) => "grab: debug draw off (F7)".into(),
        None => String::new(),
    };
    // the jump targeting (F8 draws it, `target_debug::TargetDebug`)
    let target_line = match target_debug.as_deref() {
        Some(d) if d.show => d.summary(crate::player::target_debug::flying_target(loco, body, data)),
        Some(_) => "target: debug draw off (F8)".into(),
        None => String::new(),
    };
    use crate::bindings::Slot;
    let default_bindings = crate::bindings::Bindings::default();
    let b = bindings.as_deref().unwrap_or(&default_bindings);
    let (hp, legs, hand, head) = (b.label(Slot::HighProfile), b.label(Slot::Legs), b.label(Slot::EmptyHand), b.label(Slot::Head));
    let help = format!(
        "bindings: {} | {}{}{}{} move | {hp} high profile | {legs} legs (hold with {hp} = sprint/free-run; into a wall = wall run; {hand} at a wall = climb;\n\
         while hanging or climbing: {hand} = let go, {hp}+{legs} = jump off (stick steers); up at a top edge = pull up) | {head} head | Alt slow (port)\n\
         {hand} empty hand (at a roof edge: pull down to a hang; on a ladder: drop; {hp}+{legs} = jump off; on a wall run: {legs} = push off)\n\
         LMB capture mouse | Esc release | G guidance edges | F5 / pad Start Ghost mode (noclip; Space/Ctrl up/down, {hp} x3, Alt x10, {} strafe, F6 camera, {hand}+F5 keep speed) | F4 test areas | F7 air-catch boxes | F8 jump targets | F9 save a bug report | F1 hide | F2 maps",
        b.profile, b.label(Slot::LeftStickUp), b.label(Slot::LeftStickLeft), b.label(Slot::LeftStickDown), b.label(Slot::LeftStickRight), b.label(Slot::LeftShoulder)
    );
    text.0 = format!(
        "AC1 movement port - {}   {}{}{}\n\
         context: {:?} ({})   previous: {:?}\n\
         {}\n\
         ground sub-state: {:?}   speed param {:.2} -> {:?}   turn atten {:.2}\n\
         profile: {}   legs: {}   stick {:.2} (speed01 {:.2})\n\
         air: {}   height {:.2} m\n\
         {}\n\
         {}\n\
         last landing: {}\n\n\
         {}",
        map.active.label(),
        model.0,
        if fly.is_some_and(|f| f.active) { "   [GHOST MODE - F5 to drop out]" } else { "" },
        areas.and_then(|a| a.name()).map(|n| format!("   test area: {n} (F4 next, Shift+F4 back)")).unwrap_or_default(),
        loco.current,
        loco.current as u8,
        loco.previous,
        context_detail,
        g.sub_state,
        g.speed_param,
        speed_band(g.speed_param),
        g.turn_atten,
        if pad.high_profile { "HIGH" } else { "low" },
        if pad.legs_held { "held" } else { "-" },
        pad.magnitude,
        pad.speed01,
        air_mode,
        body.feet.y,
        grab_line,
        target_line,
        landing,
        help
    );
}

/// `AC_FPS_LOG=<file>`: every half second of real time, the frame count, the mean and worst frame time (ms) and the
/// player's feet (game axes), appended to the file: where the frame rate drops along a route.
#[derive(Default)]
struct FrameLog {
    file: Option<std::fs::File>,
    t: f32,
    frames: u32,
    elapsed: f32,
    worst: f32,
    last: Option<std::time::Instant>,
}

/// Wall-clock frame times (a replay fixes the real-time clock's step, so `Time<Real>` would read 1/60 s).
fn log_frame_times(mut log: Local<FrameLog>, q: Query<&Body, With<Player>>) {
    use std::io::Write;
    if log.file.is_none() {
        let Some(path) = std::env::var_os("AC_FPS_LOG") else { return };
        log.file = std::fs::File::create(path).ok();
        if let Some(f) = log.file.as_mut() {
            let _ = writeln!(f, "t frames mean_ms worst_ms x y z");
        }
    }
    let now = std::time::Instant::now();
    let dt = log.last.map_or(0.0, |l| (now - l).as_secs_f32());
    log.last = Some(now);
    log.t += dt;
    log.frames += 1;
    log.elapsed += dt;
    log.worst = log.worst.max(dt);
    if log.elapsed >= 0.5 {
        let p = q.iter().next().map(|b| b.feet).unwrap_or(Vec3::ZERO);
        let line = format!("{:.2} {} {:.2} {:.2} {:.2} {:.2} {:.2}", log.t, log.frames, 1000.0 * log.elapsed / log.frames as f32, 1000.0 * log.worst, p.x, -p.z, p.y);
        if let Some(f) = log.file.as_mut() {
            let _ = writeln!(f, "{line}");
        }
        log.frames = 0;
        log.elapsed = 0.0;
        log.worst = 0.0;
    }
}
