//! Debug HUD: current locomotion context, sub-state, speed band, last landing. Toggle with F1.

use bevy::prelude::*;

use crate::input::PadInput;
use crate::player::air::AirMode;
use crate::player::ground::speed_band;
use crate::player::{ActorContextId, Body, HumanDataBundle, Locomotion, Player};

#[derive(Component)]
struct HudText;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud).add_systems(Update, update_hud);
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
}

fn update_hud(
    keys: Res<ButtonInput<KeyCode>>,
    pad: Res<PadInput>,
    model: Res<crate::model::ModelStatus>,
    map: Res<crate::map_menu::MapMenu>,
    q: Query<(&Locomotion, &Body, &HumanDataBundle), With<Player>>,
    mut hud: Query<(&mut Text, &mut Visibility), With<HudText>>,
) {
    let Ok((mut text, mut vis)) = hud.single_mut() else { return };
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
    text.0 = format!(
        "AC1 movement port - {}   {}\n\
         context: {:?} ({})   previous: {:?}\n\
         {}\n\
         ground sub-state: {:?}   speed param {:.2} -> {:?}   turn atten {:.2}\n\
         profile: {}   legs: {}   stick {:.2} (speed01 {:.2})\n\
         air: {}   height {:.2} m\n\
         last landing: {}\n\n\
         WASD move | RMB high profile | Space legs (hold with RMB = sprint/free-run; into a wall = wall run; E at a wall = climb;\n\
         while hanging or climbing: E = let go, RMB+Space = jump off (stick steers); up at a top edge = pull up) | Alt slow\n\
         E empty hand (on a ladder: drop; RMB+Space = jump off; on a wall run: Space = push off)\n\
         LMB capture mouse | Esc release | G guidance edges | F9 save a bug report | F1 hide | F2 maps",
        map.active.label(),
        model.0,
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
        landing
    );
}
