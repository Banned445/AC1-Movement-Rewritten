//! Test areas: F4 / Shift+F4 step through named spots on the greybox map, one per movement feature, so testers can
//! go straight to what they want to try. PORT tooling (no counterpart in the game).

use bevy::prelude::*;

use crate::player::{Player, PlayerSet};

/// A named spot: the feet and the facing (+Z = π, −Z = 0, +X = −π/2, −X = π/2). A drop spot hovers in Ghost mode:
/// F5 leaves it as a fall from rest (the air catches' starts in the sim tests).
pub struct Area {
    pub name: &'static str,
    pub feet: Vec3,
    pub heading: f32,
    pub drop: bool,
}

const fn at(name: &'static str, feet: Vec3, heading: f32) -> Area {
    Area { name, feet, heading, drop: false }
}

const fn drop(name: &'static str, feet: Vec3, heading: f32) -> Area {
    Area { name, feet, heading, drop: true }
}

const PI: f32 = std::f32::consts::PI;
const EAST: f32 = -std::f32::consts::FRAC_PI_2;
const NORTH: f32 = PI;
const SOUTH: f32 = 0.0;

/// The greybox areas, grouped by system.
pub const AREAS: &[Area] = &[
    // ground and jumps
    at("Open ground (walk, run, sprint, pivots)", Vec3::new(-60.0, 0.0, -40.0), EAST),
    at("Roofs and jumps", Vec3::new(7.0, 3.5, 12.0), EAST),
    at("Step-up box (free-run hop)", Vec3::new(9.0, 0.0, -3.0), NORTH),
    at("Pass-over railing", Vec3::new(40.0, 0.0, 8.4), SOUTH),
    at("Look-down and pull-down edge", Vec3::new(10.6, 3.5, 12.0), EAST),
    at("Falls and ledge stop (6 m block)", Vec3::new(-11.0, 6.0, 2.0), EAST),
    at("Walk / run off an edge (2 m and 3.2 m blocks)", Vec3::new(0.0, 2.0, 24.0), SOUTH),
    // haystacks and kiosks
    at("Haystack (walk into it)", Vec3::new(37.5, 0.0, 21.5), NORTH),
    at("Haystack in a raised bed (free run onto the rim)", Vec3::new(-104.0, 0.0, -126.0), NORTH),
    at("Leap of Faith (9.5 m block)", Vec3::new(30.5, 9.5, 26.0), EAST),
    at("Kiosk (market-stall bar)", Vec3::new(120.0, 4.5, 19.0), NORTH),
    // walls, ledges and climbing
    at("Wall run", Vec3::new(76.0, 0.0, 57.9), NORTH),
    at("Ledges", Vec3::new(2.0, 0.0, 33.0), NORTH),
    at("Ledge moves (corners, side jumps)", Vec3::new(22.0, 0.0, 48.6), NORTH),
    at("Hang-type switch (wall / free hang)", Vec3::new(61.5, 0.0, 48.6), NORTH),
    at("Hang → ladder side jump", Vec3::new(142.2, 0.0, 9.0), NORTH),
    at("Hang → climb holds side jump", Vec3::new(-137.8, 0.0, -136.0), NORTH),
    at("Climb tower", Vec3::new(-20.5, 0.0, 26.5), NORTH),
    at("Climb corners onto the ground", Vec3::new(-41.0, 0.0, -11.4), NORTH),
    at("Climb reach to a hang", Vec3::new(-56.8, 0.0, -11.4), NORTH),
    at("Climb across a gap", Vec3::new(-71.0, 0.0, -11.4), NORTH),
    at("Climb inside / outside corners", Vec3::new(-81.0, 0.0, -11.4), NORTH),
    at("Climb onto a ladder", Vec3::new(-102.8, 0.0, -11.4), NORTH),
    at("Climb up past missing holds", Vec3::new(-116.0, 0.0, -11.4), NORTH),
    at("Climb onto an overhang", Vec3::new(-30.0, 0.0, -11.4), NORTH),
    at("Climb side ledge grab (wall Y)", Vec3::new(-116.9, 0.0, -136.4), NORTH),
    // air catches and edge landings (hovering: F5 drops)
    drop("Air catch: climb holds (F5 to drop, hold E)", Vec3::new(-20.0, 6.0, 26.45), NORTH),
    drop("Air catch: one-handed side catch (F5 to drop, hold E and D)", Vec3::new(1.0, 5.2, 34.9), EAST),
    drop("Air catch: knee catch onto the roof (F5 to drop)", Vec3::new(41.0, 4.0, 8.62), NORTH),
    drop("Edge landing: step off the roof's lip, back (F5 to drop)", Vec3::new(41.0, 5.0, 9.1), SOUTH),
    drop("Edge landing: step off the roof's lip, front (F5 to drop)", Vec3::new(41.0, 5.0, 9.1), NORTH),
    // beams, posts, bars, ladders
    at("Beam", Vec3::new(71.0, 4.0, 70.0), EAST),
    at("Beam jumps", Vec3::new(79.0, 4.0, 70.0), EAST),
    at("Posts (pilotis)", Vec3::new(69.0, 3.0, 80.0), EAST),
    at("Swing bars", Vec3::new(60.0, 1.2, 85.0), NORTH),
    at("Ladder", Vec3::new(50.0, 0.0, 61.0), NORTH),
];

#[derive(Resource, Default)]
pub struct TestAreas {
    /// The area last jumped to.
    pub current: Option<usize>,
}

impl TestAreas {
    pub fn name(&self) -> Option<&'static str> {
        self.current.map(|i| AREAS[i].name)
    }
}

pub struct TestAreasPlugin;

impl Plugin for TestAreasPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TestAreas>().add_systems(Update, step_areas.before(PlayerSet));
    }
}

fn step_areas(world: &mut World) {
    let keys = world.resource::<ButtonInput<KeyCode>>();
    if !keys.just_pressed(KeyCode::F4) {
        return;
    }
    let back = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    if world.resource::<crate::map_menu::MapMenu>().active != crate::map_menu::Map::Greybox {
        return;
    }
    let n = AREAS.len();
    let i = match world.resource::<TestAreas>().current {
        None => if back { n - 1 } else { 0 },
        Some(i) => if back { (i + n - 1) % n } else { (i + 1) % n },
    };
    world.resource_mut::<TestAreas>().current = Some(i);
    jump_to(world, &AREAS[i]);
}

/// Put the player at an area: a fresh Ground start there, the camera behind, Ghost mode off.
pub fn jump_to(world: &mut World, area: &Area) {
    if let Some(mut fly) = world.get_resource_mut::<crate::debug_fly::DebugFly>() {
        fly.active = false;
        fly.free_camera = false;
    }
    let players: Vec<_> = world.query_filtered::<Entity, With<Player>>().iter(world).collect();
    for entity in players {
        world.entity_mut(entity).insert((crate::player::player_components(area.feet, area.heading), Transform::from_translation(area.feet)));
        if world.entity(entity).contains::<crate::anim::AnimPlayer>() {
            world.entity_mut(entity).insert(crate::anim::AnimPlayer::default());
        }
        if world.entity(entity).contains::<crate::ik::LimbIk>() {
            world.entity_mut(entity).insert(crate::ik::LimbIk::default());
        }
    }
    *world.resource_mut::<crate::input::PadInput>() = crate::map_menu::neutral_pad();
    if area.drop {
        // hover in Ghost mode (0xE46190): F5 leaves it as a fall from rest
        if let Some(mut fly) = world.get_resource_mut::<crate::debug_fly::DebugFly>() {
            fly.active = true;
            fly.velocity = Vec3::ZERO;
        }
        let mut q = world.query_filtered::<(&mut crate::player::Locomotion, &mut crate::player::HumanDataBundle, &mut crate::player::Body), With<Player>>();
        for (mut loco, mut data, mut body) in q.iter_mut(world) {
            body.grounded = false;
            body.velocity = Vec3::ZERO;
            crate::player::switch_context(&mut loco, &mut data, crate::player::TransitionSetup::ToDebug);
        }
    }
    let mut rig = world.resource_mut::<crate::camera::CameraRig>();
    // the rig's yaw equals the heading for a camera behind the player
    rig.yaw = area.heading;
    rig.pitch = -0.25;
    rig.shake = 0.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_test_area_starts_on_the_floor_or_a_roof_and_clear_of_walls() {
        let (collision, _) = crate::level::geometry();
        for a in AREAS {
            assert!(collision.capsule_fits(a.feet + Vec3::Y * 0.05), "{}: the capsule does not fit at {:?}", a.name, a.feet);
            if a.drop {
                continue;
            }
            let floor = collision.floor_height_below(a.feet + Vec3::Y * 0.05, 0.5);
            assert!(floor.is_some_and(|y| (y - a.feet.y).abs() < 0.06), "{}: no floor under {:?} ({floor:?})", a.name, a.feet);
        }
    }
}
