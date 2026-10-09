//! Whole-world import with the game's streaming grid (RE/17): Damascus. Reads the user's install at runtime.
pub mod archive;
pub mod decode;
pub mod fakes;
pub mod grid;
pub mod material;
pub mod shapes;
pub mod stream;
#[cfg(test)]
mod survey;
#[cfg(test)]
mod stream_tests;

use std::{path::Path, sync::{Arc, Mutex}};

use bevy::prelude::*;

use crate::assets::forge::{crc32, Resource};
use decode::CityData;

/// A city world: archive, the stored file holding its World, and the archives its cells link into (RE/15 §3).
pub struct CityWorld {
    pub archive: &'static str,
    pub world: &'static str,
}

pub const DAMASCUS: CityWorld = CityWorld { archive: "DataPC_Damascus.forge", world: "Damascus" };

pub struct OpenedWorld {
    pub data: Arc<CityData>,
    /// The World file's resources (FakeEntities, spawners, fake meshes …).
    pub globals: Vec<Arc<Resource>>,
}

pub fn open(game: &Path, world: &CityWorld, bc: bool) -> Result<OpenedWorld, String> {
    let archives = archive::shared(game, &[world.archive, "DataPC_Common.forge", "DataPC.forge"])?;
    let entry = archives.find(0, world.world).ok_or_else(|| format!("{} has no {} file", world.archive, world.world))?;
    let file = archives.file(0, entry)?;
    let mut globals: Vec<Arc<Resource>> = file.values().cloned().collect();
    globals.sort_by_key(|r| r.id);
    let w = globals.iter().find(|r| r.class_hash == crc32("World")).ok_or("no World resource")?;
    let (size, origin, width, radius) = grid::parse_layout(&w.payload)?;
    let partition = globals.iter().find(|r| r.class_hash == crc32("GridPartition")).ok_or("no GridPartition")?;
    let grid = grid::parse_partition(&partition.payload, size, origin, width, radius)?;
    let data = CityData { archives, grid, meshes: Default::default(), materials: Default::default(), bc, resident: Mutex::default() };
    Ok(OpenedWorld { data: Arc::new(data), globals })
}

/// A named player spawner in the World file (`PlayerSpawner_*`): Bevy-space feet and heading.
pub fn spawner(globals: &[Arc<Resource>], name: &str) -> Option<(Vec3, f32)> {
    let r = globals.iter().find(|r| r.class_hash == crc32("Entity") && r.name == name)?;
    let m = crate::assets::world::entity_transform(&r.payload).ok()?;
    let p = decode::to_bevy(m.w_axis.truncate());
    // The entity's forward axis (game +Y) as a Bevy heading (the port's heading 0 faces +Z).
    let f = decode::to_bevy(m.y_axis.truncate());
    Some((p, f.x.atan2(f.z)))
}

/// A decoded city ready to spawn: the world, the cells around the spawn (decoded before the current map is torn
/// down, so a failure keeps it), the distant fakes and the spawn point.
pub struct Prepared {
    pub world: OpenedWorld,
    pub cells: Vec<decode::DecodedCell>,
    pub fakes: Vec<fakes::FakeBlock>,
    pub spawn: Vec3,
    pub heading: f32,
}

/// PORT: debug start points. The game enters Damascus at `PlayerSpawner_Kingdom` (the arrival from the Kingdom);
/// `AC_CITY_SPAWN=Bureau|Academy|Palace|Souk` picks the World's `PlayerSpawner_DEBUG_*` instead.
pub fn spawn_name() -> String {
    match std::env::var("AC_CITY_SPAWN").ok().as_deref() {
        Some(n @ ("Bureau" | "Academy" | "Palace" | "Souk")) => format!("PlayerSpawner_DEBUG_{n}"),
        _ => "PlayerSpawner_Kingdom".into(),
    }
}

pub fn prepare(game: &Path, world: &CityWorld, bc: bool, spawn: &str) -> Result<Prepared, String> {
    let opened = open(game, world, bc)?;
    let (feet, heading) = spawner(&opened.globals, spawn).ok_or_else(|| format!("{} has no {spawn}", world.world))?;
    let fakes = fakes::parse(&opened.data, &opened.globals)?;
    // every cell within the load radius, decoded on all cores
    let wanted = stream::wanted_cells(&opened.data, &stream::content_mask(&opened.data), feet, None);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).max(2);
    let data = &opened.data;
    let chunks: Vec<Vec<usize>> = (0..threads).map(|t| wanted.iter().copied().skip(t).step_by(threads).collect()).collect();
    let cells = std::thread::scope(|s| {
        let handles: Vec<_> = chunks.iter().map(|c| s.spawn(move || c.iter().map(|&i| decode::decode_cell(data, i)).collect::<Vec<_>>())).collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect::<Result<Vec<_>, String>>()
    })?;
    Ok(Prepared { world: opened, cells, fakes, spawn: feet, heading })
}

#[allow(clippy::too_many_arguments)]
pub fn spawn(In(p): In<Prepared>, mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>, mut collision: ResMut<crate::collision::CollisionWorld>, mut guidance: ResMut<crate::guidance::GuidanceWorld>,
    mut spawn_point: ResMut<crate::player::SpawnPoint>, mut camera: ResMut<crate::camera::CameraRig>, mut city: ResMut<stream::City>,
    mut players: Query<(&mut crate::player::Body, &mut Transform), With<crate::player::Player>>,
    cams: Query<Entity, With<crate::camera::MainCamera>>) {
    *collision = crate::collision::CollisionWorld::default();
    collision.enable_index(8.0);
    *guidance = crate::guidance::GuidanceWorld { jump_pilotis: Some(Vec::new()), ..default() };
    let mut state = stream::CityState::new(p.world.data.clone());
    let mut ctx = stream::Ctx { commands: &mut commands, meshes: &mut meshes, materials: &mut materials, images: &mut images,
        collision: &mut collision, guidance: &mut guidance };
    let n = p.cells.len();
    for d in p.cells { state.integrate(d, &mut ctx); }
    state.spawn_fakes(p.fakes, &mut ctx);
    let eye = p.spawn + Vec3::Y * 3.0;
    state.prime(p.spawn, eye, &mut ctx);
    info!("Damascus: {n} cells around the spawn, {:?}", state.stats);
    city.0 = Some(Box::new(state));
    spawn_point.0 = p.spawn;
    *camera = default();
    camera.yaw = p.heading + std::f32::consts::PI;
    if let Ok((mut body, mut transform)) = players.single_mut() {
        body.feet = p.spawn; body.heading = p.heading; body.grounded = true; body.velocity = Vec3::ZERO; body.proxy.manifold.clear();
        transform.translation = p.spawn;
    }
    // PORT: a fixed sun, sky colour and haze stand in for WorldAmbiance / LayeredSky (not decoded).
    commands.spawn((crate::map_menu::MapEntity, DirectionalLight { illuminance: 11_000.0, shadow_maps_enabled: true, ..default() },
        bevy::light::CascadeShadowConfigBuilder { num_cascades: 3, first_cascade_far_bound: 12.0, maximum_distance: 120.0, ..default() }.build(),
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.9, -0.85, 0.0))));
    for cam in &cams {
        commands.entity(cam).insert(DistanceFog { color: Color::srgba(0.80, 0.78, 0.70, 1.0),
            falloff: FogFalloff::Linear { start: 220.0, end: 1100.0 }, ..default() });
    }
}

/// Leaving the city: stop its loaders and take the haze off the camera.
pub fn leave(world: &mut World) {
    if world.get_resource_mut::<stream::City>().is_some_and(|mut c| c.0.take().is_some()) {
        let cams: Vec<Entity> = world.query_filtered::<Entity, With<crate::camera::MainCamera>>().iter(world).collect();
        for c in cams { world.entity_mut(c).remove::<DistanceFog>(); }
    }
}

pub struct CityPlugin;

impl Plugin for CityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<stream::City>()
            .add_systems(Startup, spawn_overlay)
            .add_systems(Update, (update_city.before(crate::player::PlayerSet), hold_player.after(crate::player::PlayerSet), update_overlay));
    }
}

#[allow(clippy::too_many_arguments)]
/// Below the lowest Damascus terrain (about −70 m in the countryside ring).
const VOID_Y: f32 = -150.0;

pub(crate) fn update_city(mut city: ResMut<stream::City>, time: Res<Time>, spawn: Res<crate::player::SpawnPoint>, mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>, mut images: ResMut<Assets<Image>>, mut collision: ResMut<crate::collision::CollisionWorld>,
    mut guidance: ResMut<crate::guidance::GuidanceWorld>, mut players: Query<&mut crate::player::Body, With<crate::player::Player>>,
    cams: Query<&GlobalTransform, With<crate::camera::MainCamera>>) {
    let Some(state) = city.0.as_mut() else { return };
    let Ok(mut body) = players.single_mut() else { return };
    // PORT: falling out of the world (past the grid's ground) puts the player back at the spawn; the game's
    // OutOfBounds / desynchronisation handling is not ported.
    if body.feet.y < VOID_Y { body.feet = spawn.0; body.velocity = Vec3::ZERO; body.proxy.manifold.clear(); }
    // AC_CITY_TOUR=<m/s>: debug fly-through, 25 m above the spawn, straight across the grid (streaming / LOD stress)
    if let Some(speed) = std::env::var("AC_CITY_TOUR").ok().and_then(|v| v.parse::<f32>().ok()) {
        let height = std::env::var("AC_CITY_TOUR_HEIGHT").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(25.0);
        let start = *state.tour.get_or_insert(body.feet + Vec3::Y * height);
        let t = time.elapsed_secs();
        let dir = Vec3::new(-1.0, 0.0, 0.35).normalize();
        body.feet = start + dir * (speed * t);
        body.velocity = Vec3::ZERO;
    }
    let (eye, view) = cams.iter().next().map_or((body.feet, Vec3::NEG_Z), |t| (t.translation(), t.forward().as_vec3()));
    let mut ctx = stream::Ctx { commands: &mut commands, meshes: &mut meshes, materials: &mut materials, images: &mut images,
        collision: &mut collision, guidance: &mut guidance };
    state.update(body.feet, eye, view, time.delta_secs(), &mut ctx);
    // AC_CITY_LOG=<seconds>: log the streaming counters and frame times at that interval
    if let Some(every) = std::env::var("AC_CITY_LOG").ok().and_then(|v| v.parse::<f32>().ok()) {
        let log = &mut state.log;
        log.0 += time.delta_secs(); log.1 += 1; log.2 = log.2.max(time.delta_secs());
        if log.0 >= every {
            info!("city log: {:.2} ms avg frame, {:.2} ms worst, at {:?}: {:?}", log.0 * 1000.0 / log.1 as f32, log.2 * 1000.0, body.feet, state.stats);
            *log = (0.0, 0, 0.0);
        }
    }
}

/// `GridStreamer__Update` 0x55D810 holds the player while a cell under the half-cell box around them is still
/// loading. PORT: the port keeps the position (and drops the velocity) instead of the game's loading state.
pub(crate) fn hold_player(city: Res<stream::City>, mut players: Query<&mut crate::player::Body, With<crate::player::Player>>) {
    let Some(hold) = city.0.as_ref().and_then(|c| c.hold) else { return };
    for mut body in &mut players { body.feet = hold; body.velocity = Vec3::ZERO; }
}

#[derive(Component)]
struct Overlay;

fn spawn_overlay(mut commands: Commands) {
    commands.spawn((Overlay, Text::new(""), TextFont { font_size: FontSize::Px(14.0), ..default() }, TextColor(Color::srgb(1.0, 1.0, 0.85)),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)), Visibility::Hidden,
        Node { position_type: PositionType::Absolute, bottom: Val::Px(10.0), right: Val::Px(12.0), padding: UiRect::all(Val::Px(6.0)), ..default() }));
}

/// F3: streaming counters.
fn update_overlay(keys: Res<ButtonInput<KeyCode>>, city: Res<stream::City>, time: Res<Time>, mut q: Query<(&mut Text, &mut Visibility), With<Overlay>>) {
    let Ok((mut text, mut vis)) = q.single_mut() else { return };
    if keys.just_pressed(KeyCode::F3) { *vis = if *vis == Visibility::Hidden { Visibility::Inherited } else { Visibility::Hidden }; }
    let Some(s) = city.0.as_ref().map(|c| &c.stats) else { text.0 = "no streamed world".into(); return };
    text.0 = format!("city: {} cells loaded, {} loading | {} objects, {} drawn\nphysics: {} objects, {} triangles, {} edges\nGPU: {} materials, {} textures | {:.1} ms frame{}",
        s.loaded, s.loading, s.objects, s.drawn, s.physics_objects, s.physics_triangles, s.physics_edges, s.materials, s.textures,
        time.delta_secs() * 1000.0, if s.errors > 0 { format!(" | {} errors", s.errors) } else { String::new() });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn damascus() -> Option<OpenedWorld> {
        let game = crate::assets::find_game_dir()?;
        if !game.join(DAMASCUS.archive).is_file() { return None; }
        Some(open(&game, &DAMASCUS, true).expect("Damascus must open when the archive is present"))
    }

    #[test]
    fn damascus_grid_matches_the_partition() {
        let Some(w) = damascus() else { return };
        let g = &w.data.grid;
        assert_eq!((g.cell_size, g.origin, g.width, g.levels), (32.0, Vec2::new(-480.0, -512.0), 32, 6));
        assert_eq!(g.cells.len(), 1365);
        assert_eq!(g.cells[1364].level, 5);
        assert!(spawner(&w.globals, "PlayerSpawner_Kingdom").is_some());
    }

    /// Every cell decodes; entity placements lie in (or near: large objects overhang) their cell's rectangle.
    #[test]
    #[ignore]
    fn damascus_every_cell_decodes() {
        let Some(w) = damascus() else { return };
        let t = std::time::Instant::now();
        let mut totals = decode::CellStats::default();
        let mut far = 0;
        for cell in 0..w.data.grid.cells.len() {
            let d = decode::decode_cell(&w.data, cell).unwrap_or_else(|e| panic!("cell {cell}: {e}"));
            let (min, max) = w.data.grid.cell_rect(&w.data.grid.cells[cell]);
            let margin = w.data.grid.level_size(w.data.grid.cells[cell].level);
            for o in &d.objects {
                let p = o.transform.w_axis.truncate();
                let g = Vec2::new(p.x, -p.z);
                if g.x < min.x - margin || g.y < min.y - margin || g.x > max.x + margin || g.y > max.y + margin { far += 1; }
            }
            totals.bodies += d.stats.bodies; totals.objects += d.stats.objects;
            totals.triangles += d.stats.triangles; totals.edges += d.stats.edges;
            for (k, v) in d.stats.skipped { *totals.skipped.entry(k).or_default() += v; }
        }
        println!("decoded all cells in {:?}: {totals:#?}; {far} placements more than a cell outside their cell", t.elapsed());
        assert!(totals.objects > 15000 && totals.edges > 50000);
        assert!(far < totals.objects / 100);
    }
}
