//! PORT: deterministic Ground-extra acceptance fixtures, never native gameplay rules.
//! `AC_GROUND_CHECK=static|static-target|crouch|crouch-ceiling|slow|ledge-stop|slope|avoid`.
//! The F9 header retains the fixture so AC_REPLAY restores geometry and external social requests.
use crate::{
    collision::{Aabb3, CollisionWorld},
    guidance::GuidanceWorld,
    input::PadInput,
    player::{
        crowd::{CrowdActor, PushReceiver},
        social::SocialHelper,
        Body, Player,
    },
};
use bevy::prelude::*;

#[derive(Resource)]
pub(crate) struct GroundCheck {
    pub name: String,
    replay: bool,
}
impl GroundCheck {
    pub fn from_env() -> Option<Self> {
        let replay = std::env::var_os("AC_REPLAY");
        let name = if let Some(dir) = replay.as_ref() {
            std::fs::read_to_string(std::path::Path::new(dir).join("input.txt"))
                .ok()?
                .lines()
                .find_map(|l| l.strip_prefix("# ground_check ").map(str::to_owned))?
        } else {
            std::env::var("AC_GROUND_CHECK").ok()?
        };
        matches!(
            name.as_str(),
            "static"
                | "static-target"
                | "crouch"
                | "crouch-ceiling"
                | "slow"
                | "ledge-stop"
                | "slope"
                | "avoid"
        )
        .then_some(Self {
            name,
            replay: replay.is_some(),
        })
    }
    pub fn start(&self) -> (Vec3, f32) {
        match self.name.as_str() {
            "ledge-stop" => (Vec3::new(-12.0, 6.0, 4.0), -std::f32::consts::FRAC_PI_2),
            "static-target" => (Vec3::new(12.0, 0.0, 40.6), std::f32::consts::PI),
            "slope" => (Vec3::new(-30.0, 9.0, -30.0), 0.0),
            _ => (Vec3::new(-30.0, 0.0, -30.0), 0.0),
        }
    }
}
/// Shared geometry for the rendered check and headless integration test.
pub(crate) fn slope_fixture(world: &mut CollisionWorld) {
    let origin = Vec3::new(-30.0, 0.0, -30.0);
    let point = |x: f32, z: f32| origin + Vec3::new(x, 6.0 + 3f32.sqrt() * x, z);
    let [a, b, c, d] = [
        point(-3.0, -4.0),
        point(-3.0, 4.0),
        point(3.0, 4.0),
        point(3.0, -4.0),
    ];
    for vertices in [[a, b, c], [a, c, d]] {
        world
            .triangles
            .push(crate::triangles::Triangle::new(vertices, crate::layers::STATIC).unwrap());
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn place(
    check: Res<GroundCheck>,
    mut commands: Commands,
    mut players: Query<&mut Body, With<Player>>,
    mut rig: ResMut<crate::camera::CameraRig>,
    mut collision: ResMut<CollisionWorld>,
    mut guidance: ResMut<GuidanceWorld>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let (start, heading) = check.start();
    if !check.replay {
        for mut b in &mut players {
            b.feet = start;
            b.heading = heading;
        }
    }
    rig.yaw = -0.8;
    rig.pitch = -0.25;
    rig.distance = 6.0;
    let material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.65, 0.37, 0.15),
        ..default()
    });
    if check.name == "slope" {
        let first = collision.triangles.len();
        slope_fixture(&mut collision);
        // Render each fixture face from the same vertices used by the proxy.
        for triangle in &collision.triangles[first..] {
            let mesh = Mesh::new(
                bevy::mesh::PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::default(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_POSITION,
                triangle.vertices.map(|p| p.to_array()).to_vec(),
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![triangle.normal.to_array(); 3]);
            commands.spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material.clone()),
                Transform::default(),
            ));
        }
        *guidance = GuidanceWorld::default();
        rig.yaw = -std::f32::consts::FRAC_PI_2;
        rig.distance = 9.0;
    } else if check.name == "crouch-ceiling" {
        let b = Aabb3 {
            min: start + Vec3::new(-2.0, 1.2, -2.0),
            max: start + Vec3::new(2.0, 1.5, 2.0),
        };
        collision.boxes.push(b);
        let material = materials.add(StandardMaterial {
            base_color: Color::srgba(0.65, 0.37, 0.15, 0.25),
            alpha_mode: AlphaMode::Blend,
            ..default()
        });
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::from_size(b.max - b.min))),
            MeshMaterial3d(material.clone()),
            Transform::from_translation((b.min + b.max) * 0.5),
        ));
    } else if check.name == "avoid" {
        // PORT: a registered test recipient, not an invented native NPC importer or AI.
        let feet = start + Vec3::new(0.6, 0.0, -1.0);
        commands.spawn((
            Body {
                feet,
                heading: std::f32::consts::PI,
                ..default()
            },
            CrowdActor {
                proximity_eligible: true,
                receiver: PushReceiver {
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
                },
            },
            Mesh3d(meshes.add(Capsule3d::new(0.35, 1.1))),
            MeshMaterial3d(material),
            Transform::from_translation(feet + Vec3::Y * 0.9),
        ));
    }
}
pub(crate) fn drive(
    check: Res<GroundCheck>,
    time: Res<Time>,
    mut pad: ResMut<PadInput>,
    mut helpers: Query<&mut SocialHelper, With<Player>>,
) {
    let t = time.elapsed_secs();
    if check.name.starts_with("crouch") {
        // PORT: explicit status-producer fixture; the native bit-5 producer is still under audit.
        for mut helper in &mut helpers {
            helper.allowed = true;
            if t < 3.0 {
                helper.set(5);
            } else {
                helper.clear(5);
            }
        }
    }
    if check.replay {
        return;
    } // Replay owns all recorded input, fixtures own only external state.
    pad.dir = Vec3::ZERO;
    pad.magnitude = 0.0;
    pad.speed01 = 0.0;
    pad.high_profile = false;
    pad.legs_held = false;
    pad.hand_held = false;
    match check.name.as_str() {
        "static" | "static-target" if (0.5..3.5).contains(&t) => {
            pad.high_profile = true;
            pad.legs_held = t < 1.0;
        }
        "crouch" if (0.8..2.2).contains(&t) => {
            pad.dir = Vec3::NEG_Z;
            pad.magnitude = 1.0;
            pad.speed01 = 1.0;
        }
        "slow" | "avoid" if (0.5..3.5).contains(&t) => {
            pad.dir = Vec3::NEG_Z;
            pad.magnitude = 0.6;
            pad.speed01 = (0.6 - 0.35) / 0.65;
        }
        "ledge-stop" if (0.5..4.5).contains(&t) => {
            pad.dir = Vec3::X;
            pad.magnitude = 1.0;
            pad.speed01 = 1.0;
        }
        _ => {}
    }
}
