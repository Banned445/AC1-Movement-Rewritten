//! Test areas: F4 / Shift+F4 step through named spots on the greybox map, one per movement feature, so testers can
//! go straight to what they want to try. PORT tooling (no counterpart in the game).

use bevy::prelude::*;

use crate::player::{Player, PlayerSet};

/// A named spot: the feet and the facing (+Z = π, −Z = 0, +X = −π/2, −X = π/2).
pub struct Area {
    pub name: &'static str,
    pub feet: Vec3,
    pub heading: f32,
}

const PI: f32 = std::f32::consts::PI;
const EAST: f32 = -std::f32::consts::FRAC_PI_2;
const NORTH: f32 = PI;
const SOUTH: f32 = 0.0;

/// The greybox areas, grouped by system.
pub const AREAS: &[Area] = &[
    // ground and jumps
    Area { name: "Open ground (walk, run, sprint, pivots)", feet: Vec3::new(-60.0, 0.0, -40.0), heading: EAST },
    Area { name: "Roofs and jumps", feet: Vec3::new(7.0, 3.5, 12.0), heading: EAST },
    Area { name: "Step-up box (free-run hop)", feet: Vec3::new(9.0, 0.0, -3.0), heading: NORTH },
    Area { name: "Pass-over railing", feet: Vec3::new(40.0, 0.0, 8.4), heading: SOUTH },
    Area { name: "Look-down and pull-down edge", feet: Vec3::new(10.6, 3.5, 12.0), heading: EAST },
    Area { name: "Falls and ledge stop (6 m block)", feet: Vec3::new(-11.0, 6.0, 2.0), heading: EAST },
    // haystacks and kiosks
    Area { name: "Haystack (walk into it)", feet: Vec3::new(37.5, 0.0, 21.5), heading: NORTH },
    Area { name: "Haystack in a raised bed (free run onto the rim)", feet: Vec3::new(-104.0, 0.0, -126.0), heading: NORTH },
    Area { name: "Leap of Faith (9.5 m block)", feet: Vec3::new(30.5, 9.5, 26.0), heading: EAST },
    Area { name: "Kiosk (market-stall bar)", feet: Vec3::new(120.0, 4.5, 19.0), heading: NORTH },
    // walls, ledges and climbing
    Area { name: "Wall run", feet: Vec3::new(76.0, 0.0, 57.9), heading: NORTH },
    Area { name: "Ledges", feet: Vec3::new(2.0, 0.0, 33.0), heading: NORTH },
    Area { name: "Ledge moves (corners, side jumps)", feet: Vec3::new(22.0, 0.0, 48.6), heading: NORTH },
    Area { name: "Hang-type switch (wall / free hang)", feet: Vec3::new(61.5, 0.0, 48.6), heading: NORTH },
    Area { name: "Hang → ladder side jump", feet: Vec3::new(142.2, 0.0, 9.0), heading: NORTH },
    Area { name: "Hang → climb holds side jump", feet: Vec3::new(-137.8, 0.0, -136.0), heading: NORTH },
    Area { name: "Climb tower", feet: Vec3::new(-20.5, 0.0, 26.5), heading: NORTH },
    Area { name: "Climb corners onto the ground", feet: Vec3::new(-41.0, 0.0, -11.4), heading: NORTH },
    Area { name: "Climb reach to a hang", feet: Vec3::new(-56.8, 0.0, -11.4), heading: NORTH },
    Area { name: "Climb across a gap", feet: Vec3::new(-71.0, 0.0, -11.4), heading: NORTH },
    Area { name: "Climb inside / outside corners", feet: Vec3::new(-81.0, 0.0, -11.4), heading: NORTH },
    Area { name: "Climb onto a ladder", feet: Vec3::new(-102.8, 0.0, -11.4), heading: NORTH },
    Area { name: "Climb up past missing holds", feet: Vec3::new(-116.0, 0.0, -11.4), heading: NORTH },
    Area { name: "Climb onto an overhang", feet: Vec3::new(-30.0, 0.0, -11.4), heading: NORTH },
    Area { name: "Climb side ledge grab (wall Y)", feet: Vec3::new(-116.9, 0.0, -136.4), heading: NORTH },
    // beams, posts, bars, ladders
    Area { name: "Beam", feet: Vec3::new(71.0, 4.0, 70.0), heading: EAST },
    Area { name: "Beam jumps", feet: Vec3::new(79.0, 4.0, 70.0), heading: EAST },
    Area { name: "Posts (pilotis)", feet: Vec3::new(69.0, 3.0, 80.0), heading: EAST },
    Area { name: "Swing bars", feet: Vec3::new(60.0, 1.2, 85.0), heading: NORTH },
    Area { name: "Ladder", feet: Vec3::new(50.0, 0.0, 61.0), heading: NORTH },
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
            let floor = collision.floor_height_below(a.feet + Vec3::Y * 0.05, 0.5);
            assert!(floor.is_some_and(|y| (y - a.feet.y).abs() < 0.06), "{}: no floor under {:?} ({floor:?})", a.name, a.feet);
            assert!(collision.capsule_fits(a.feet + Vec3::Y * 0.05), "{}: the capsule does not fit at {:?}", a.name, a.feet);
        }
    }
}
