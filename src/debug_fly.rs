//! Debug fly (noclip): F5 toggles a free flight that ignores collision, for moving around a map quickly.
//! PORT: a port-only debug tool, not a game state. While it is on the player contexts do not run (`PlayerSet` is
//! skipped); the body is moved directly and plays the Ground idle.
//!
//! Keyboard: WASD = move along the camera's view (looking down + W descends), Space = up, Left Ctrl = down,
//! Left Shift = fast, Left Alt = slow. Gamepad: left stick = move, A = up, LT = down, RT = fast.
//! Leaving (F5 again) puts the feet on the floor straight below and enters Ground; with no floor below, InAir falls.

use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::prelude::*;

use crate::camera::CameraRig;
use crate::collision::CollisionWorld;
use crate::input::PadInput;
use crate::player::air::{FallOrigin, InAirEntry};
use crate::player::{switch_context, Body, HumanDataBundle, LimbTargets, Locomotion, Player, PlayerSet, TransitionSetup};

const FLY_SPEED: f32 = 10.0;
const FLY_FAST: f32 = 4.0;
const FLY_SLOW: f32 = 0.25;
/// How far below the exit point a floor is looked for.
const EXIT_FLOOR_REACH: f32 = 1000.0;

#[derive(Resource, Default)]
pub struct DebugFly {
    pub active: bool,
}

pub struct DebugFlyPlugin;

impl Plugin for DebugFlyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugFly>()
            .configure_sets(Update, PlayerSet.run_if(|fly: Res<DebugFly>| !fly.active))
            .add_systems(
                Update,
                (toggle_fly, fly)
                    .chain()
                    .before(PlayerSet)
                    .run_if(|menu: Res<crate::map_menu::MapMenu>| !menu.open),
            );
    }
}

fn toggle_fly(
    keys: Res<ButtonInput<KeyCode>>,
    collision: Res<CollisionWorld>,
    mut fly: ResMut<DebugFly>,
    mut pad: ResMut<PadInput>,
    mut q: Query<(&mut Locomotion, &mut HumanDataBundle, &mut Body, &mut LimbTargets, Option<&mut crate::ik::LimbIk>), With<Player>>,
) {
    if !keys.just_pressed(KeyCode::F5) {
        return;
    }
    let Ok((mut loco, mut data, mut body, mut limbs, ik)) = q.single_mut() else { return };
    fly.active = !fly.active;
    body.velocity = Vec3::ZERO;
    body.tilt = Quat::IDENTITY;
    body.stick_residual = 0.0;
    body.stick_normal_y = None;
    *limbs = LimbTargets::default();
    if fly.active {
        body.grounded = false;
        switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: None });
        return;
    }
    // the manifold, foot probes and pelvis drop belong to where the flight started
    body.proxy = default();
    if let Some(mut ik) = ik {
        *ik = default();
    }
    // Space / A (up) also fed the jump buffer while flying
    pad.consume_jump();
    pad.consume_hand();
    match collision.floor_height_below(body.feet, EXIT_FLOOR_REACH) {
        Some(y) => {
            body.feet.y = y;
            body.grounded = true;
            switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: None });
        }
        None => {
            body.grounded = false;
            let entry = InAirEntry::Fall { from: body.feet, velocity: Vec3::ZERO, origin: FallOrigin::Ground, speed_param: 0.0 };
            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
        }
    }
}

fn fly(
    time: Res<Time>,
    fly: Res<DebugFly>,
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    rig: Res<CameraRig>,
    mut q: Query<(&mut Body, &mut Transform), With<Player>>,
) {
    if !fly.active {
        return;
    }
    let Ok((mut body, mut tr)) = q.single_mut() else { return };
    let key = |k: KeyCode| keys.pressed(k) as i32 as f32;
    let mut stick = Vec2::new(key(KeyCode::KeyD) - key(KeyCode::KeyA), key(KeyCode::KeyW) - key(KeyCode::KeyS));
    let mut lift = key(KeyCode::Space) - key(KeyCode::ControlLeft);
    let mut scale = if keys.pressed(KeyCode::ShiftLeft) { FLY_FAST } else if keys.pressed(KeyCode::AltLeft) { FLY_SLOW } else { 1.0 };
    for gp in &gamepads {
        let s = gp.left_stick();
        if s.length() > stick.length() {
            stick = s;
        }
        lift += gp.pressed(GamepadButton::South) as i32 as f32 - gp.pressed(GamepadButton::LeftTrigger2) as i32 as f32;
        if gp.pressed(GamepadButton::RightTrigger2) {
            scale = FLY_FAST;
        }
    }
    let stick = stick.clamp_length_max(1.0);
    // the camera sits at focus + rot * Z, looking back at the focus
    let view = -(Quat::from_euler(EulerRot::YXZ, rig.yaw, rig.pitch, 0.0) * Vec3::Z);
    let flat = Vec3::new(-rig.yaw.sin(), 0.0, -rig.yaw.cos());
    let right = crate::player::right_of(flat);
    let wish = view * stick.y + right * stick.x + Vec3::Y * lift.clamp(-1.0, 1.0);
    body.feet += wish.clamp_length_max(1.0) * FLY_SPEED * scale * time.delta_secs();
    body.heading = crate::player::heading_of(flat);
    body.velocity = Vec3::ZERO;
    body.grounded = false;
    // `sync_visuals` is in the skipped PlayerSet
    tr.translation = body.feet;
    tr.rotation = Quat::from_rotation_y(body.heading);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::ActorContextId;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    #[derive(Resource, Default)]
    struct ContextRuns(u32);

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(100)))
            .insert_resource(ButtonInput::<KeyCode>::default())
            .insert_resource(crate::camera::CameraRig { pitch: 0.0, ..default() })
            .insert_resource(crate::map_menu::neutral_pad())
            .insert_resource(crate::map_menu::MapMenu::default())
            .init_resource::<ContextRuns>()
            .add_plugins(DebugFlyPlugin)
            .add_systems(Update, (|mut runs: ResMut<ContextRuns>| runs.0 += 1).in_set(PlayerSet));
        let mut collision = CollisionWorld::default();
        collision.boxes.push(crate::collision::Aabb3 { min: Vec3::new(-50.0, -1.0, -50.0), max: Vec3::new(50.0, 2.0, 50.0) });
        app.insert_resource(collision);
        app.world_mut().spawn((crate::player::player_components(Vec3::new(0.0, 2.0, 0.0), 0.0), Transform::default()));
        app.update(); // the first update has no delta
        app
    }

    fn step(app: &mut App, keys: &[KeyCode]) {
        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        input.release_all();
        input.clear();
        for k in keys {
            input.press(*k);
        }
        app.update();
    }

    fn player<'a>(app: &'a mut App) -> (&'a Locomotion, &'a Body) {
        let world = app.world_mut();
        let e = world.query_filtered::<Entity, With<Player>>().single(world).unwrap();
        let entity = world.entity(e);
        (entity.get::<Locomotion>().unwrap(), entity.get::<Body>().unwrap())
    }

    #[test]
    fn f5_flies_through_the_world_without_the_contexts_and_lands_on_the_floor_below() {
        let mut app = app();
        let runs = app.world().resource::<ContextRuns>().0;
        step(&mut app, &[KeyCode::F5]);
        assert!(app.world().resource::<DebugFly>().active);
        assert_eq!(app.world().resource::<ContextRuns>().0, runs, "PlayerSet is skipped while flying");
        // up 10 frames (1 s), then forward (camera yaw PI looks along +Z) into the solid box: noclip
        for _ in 0..10 {
            step(&mut app, &[KeyCode::Space]);
        }
        let y = player(&mut app).1.feet.y;
        assert!((y - (2.0 + FLY_SPEED)).abs() < 1e-3, "{y}");
        for _ in 0..10 {
            step(&mut app, &[KeyCode::KeyW, KeyCode::ControlLeft]);
        }
        let feet = player(&mut app).1.feet;
        assert!(feet.z > 5.0 && feet.y < y, "{feet}");
        step(&mut app, &[KeyCode::F5]);
        assert!(!app.world().resource::<DebugFly>().active);
        let (loco, body) = player(&mut app);
        assert_eq!(loco.current, ActorContextId::Ground);
        assert!(body.grounded && (body.feet.y - 2.0).abs() < 1e-4, "{}", body.feet);
        assert!(app.world().resource::<PadInput>().legs_pressed_ago.is_infinite(), "flying up must not leave a buffered jump");
        step(&mut app, &[]);
        assert!(app.world().resource::<ContextRuns>().0 > runs);
    }

    #[test]
    fn leaving_the_flight_with_no_floor_below_falls() {
        let mut app = app();
        step(&mut app, &[KeyCode::F5]);
        app.world_mut().resource_mut::<CollisionWorld>().boxes.clear();
        step(&mut app, &[KeyCode::F5]);
        let (loco, body) = player(&mut app);
        assert_eq!(loco.current, ActorContextId::InAir);
        assert!(!body.grounded);
    }
}
