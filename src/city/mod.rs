//! Whole-world import with the game's streaming grid (RE/17): Damascus. Reads the user's install at runtime.
pub mod ambiance;
pub mod archive;
pub mod decode;
pub mod fakes;
pub mod grid;
pub mod ground;
pub mod material;
pub mod presentation;
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
    open_with(game, world, bc, &std::env::var("AC_CITY_DATABLOCKS").unwrap_or_default())
}

/// `open` with the mission DataBlocks named in `datablocks` (`mission_datablocks`).
pub fn open_with(game: &Path, world: &CityWorld, bc: bool, datablocks: &str) -> Result<OpenedWorld, String> {
    let archives = archive::shared(game, &[world.archive, "DataPC_Common.forge", "DataPC.forge"])?;
    let entry = archives.find(0, world.world).ok_or_else(|| format!("{} has no {} file", world.archive, world.world))?;
    let file = archives.file(0, entry)?;
    let mut globals: Vec<Arc<Resource>> = file.values().cloned().collect();
    globals.sort_by_key(|r| r.id);
    let w = globals.iter().find(|r| r.class_hash == crc32("World")).ok_or("no World resource")?;
    let (size, origin, width, radius) = grid::parse_layout(&w.payload)?;
    let partition = globals.iter().find(|r| r.class_hash == crc32("GridPartition")).ok_or("no GridPartition")?;
    let mut grid = grid::parse_partition(&partition.payload, size, origin, width, radius)?;
    grid.add_datablocks(&mission_datablocks(&archives, world, datablocks)?);
    let data = CityData { archives, grid, meshes: Default::default(), materials: Default::default(), bc, resident: Mutex::default() };
    Ok(OpenedWorld { data: Arc::new(data), globals })
}

/// The mission / memory-block DataBlocks to load with the grid (PORT: the game picks them by story progress, which the
/// port has no state for): `AC_CITY_DATABLOCKS`, a comma-separated list of the archive's DataBlock names (with or
/// without `<World>_` and `_DataBlock`), or `none` (the default: the grid alone). Their header resource ids.
pub fn mission_datablocks(archives: &archive::Archives, world: &CityWorld, list: &str) -> Result<Vec<u32>, String> {
    let mut ids = Vec::new();
    for item in list.split(',').map(str::trim).filter(|s| !s.is_empty() && *s != "none") {
        let mut name = item.to_string();
        if !name.starts_with(&format!("{}_", world.world)) { name = format!("{}_{name}", world.world); }
        if !name.ends_with("_DataBlock") { name.push_str("_DataBlock"); }
        let entry = archives.find(0, &name).ok_or_else(|| format!("{}: no {name}", world.archive))?;
        let file = archives.file(0, entry)?;
        let header = file.values().find(|r| r.name == name).ok_or_else(|| format!("{name}: no DataBlock header"))?;
        ids.push(header.id);
    }
    Ok(ids)
}

/// A named player spawner in the World file (`PlayerSpawner_*`): Bevy-space feet and heading.
pub fn spawner(globals: &[Arc<Resource>], name: &str) -> Option<(Vec3, f32)> {
    let r = globals.iter().find(|r| r.class_hash == crc32("Entity") && r.name == name)?;
    let m = crate::assets::world::entity_transform(&r.payload).ok()?;
    let p = decode::to_bevy(m.w_axis.truncate());
    // The entity's forward axis (game +Y) as a port heading (`Body::forward` is (-sin, 0, -cos)).
    let f = decode::to_bevy(m.y_axis.truncate());
    Some((p, (-f.x).atan2(-f.z)))
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

/// PORT: `AC_CITY_AT="x y z yaw"`, a start in game coordinates (yaw in degrees, the lab's `tp`: atan2 of the game
/// forward's y and x), to compare a spot with the running game. Bevy feet and heading.
fn debug_start() -> Option<(Vec3, f32)> {
    let v: Vec<f32> = std::env::var("AC_CITY_AT").ok()?.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    if v.len() != 4 { return None; }
    let yaw = v[3].to_radians();
    let f = decode::to_bevy(Vec3::new(yaw.cos(), yaw.sin(), 0.0));
    Some((decode::to_bevy(Vec3::new(v[0], v[1], v[2])), (-f.x).atan2(-f.z)))
}

pub fn prepare(game: &Path, world: &CityWorld, bc: bool, spawn: &str) -> Result<Prepared, String> {
    let opened = open(game, world, bc)?;
    let (feet, heading) = match debug_start() {
        Some(at) => at,
        None => spawner(&opened.globals, spawn).ok_or_else(|| format!("{} has no {spawn}", world.world))?,
    };
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
    mut grounds: ResMut<Assets<ground::GroundSurface>>,
    mut images: ResMut<Assets<Image>>, mut collision: ResMut<crate::collision::CollisionWorld>, mut guidance: ResMut<crate::guidance::GuidanceWorld>,
    mut spawn_point: ResMut<crate::player::SpawnPoint>, mut camera: ResMut<crate::camera::CameraRig>, mut city: ResMut<stream::City>,
    mut players: Query<(&mut crate::player::Body, &mut Transform), With<crate::player::Player>>,
    cams: Query<Entity, With<crate::camera::MainCamera>>) {
    *collision = crate::collision::CollisionWorld::default();
    collision.enable_index(8.0);
    *guidance = crate::guidance::GuidanceWorld::default();
    let mut state = stream::CityState::new(p.world.data.clone());
    let mut ctx = stream::Ctx { commands: &mut commands, meshes: &mut meshes, materials: &mut materials, grounds: &mut grounds, images: &mut images,
        collision: &mut collision, guidance: &mut guidance };
    let n = p.cells.len();
    for d in p.cells { state.integrate(d, &mut ctx); }
    state.spawn_fakes(p.fakes, &mut ctx);
    let eye = p.spawn + Vec3::Y * 3.0;
    state.prime(p.spawn, eye, &mut ctx);
    let g = Vec3::new(p.spawn.x, -p.spawn.z, p.spawn.y);
    info!("Damascus: {n} cells around the spawn at game ({:.2} {:.2} {:.2}), {:?}", g.x, g.y, g.z, state.stats);
    city.0 = Some(Box::new(state));
    spawn_point.0 = p.spawn;
    *camera = default();
    camera.yaw = p.heading;
    if let Ok((mut body, mut transform)) = players.single_mut() {
        body.feet = p.spawn; body.heading = p.heading; body.grounded = true; body.velocity = Vec3::ZERO; body.proxy.manifold.clear();
        transform.translation = p.spawn;
    }
    // the World's ambiance: the sun, the ambient cube, the fog, the post effects' grading and the sky (RE/17 §11)
    let cams: Vec<Entity> = cams.iter().collect();
    match ambiance::parse(&p.world.globals) {
        Ok(a) => ambiance::spawn_scene(&a, &p.world.data, &mut commands, &mut meshes, &mut materials, &mut images, &cams),
        Err(e) => {
            warn!("Damascus ambiance: {e}; a fixed sun and haze instead");
            commands.spawn((crate::map_menu::MapEntity, DirectionalLight { illuminance: 11_000.0, shadow_maps_enabled: true, ..default() },
                bevy::light::CascadeShadowConfigBuilder { num_cascades: 3, first_cascade_far_bound: 12.0, maximum_distance: 120.0, ..default() }.build(),
                Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.9, -0.85, 0.0))));
            for &cam in &cams {
                commands.entity(cam).insert(DistanceFog { color: Color::srgba(0.80, 0.78, 0.70, 1.0),
                    falloff: FogFalloff::Linear { start: 220.0, end: 1100.0 }, ..default() });
            }
        }
    }
}

/// Leaving the city: stop its loaders and take the haze off the camera.
pub fn leave(world: &mut World) {
    if world.get_resource_mut::<stream::City>().is_some_and(|mut c| c.0.take().is_some()) {
        let cams: Vec<Entity> = world.query_filtered::<Entity, With<crate::camera::MainCamera>>().iter(world).collect();
        for c in cams {
            world.entity_mut(c).remove::<(DistanceFog, bevy::light::EnvironmentMapLight, ambiance::PostGrade)>()
                .insert((bevy::camera::Exposure::default(), bevy::core_pipeline::tonemapping::Tonemapping::default()));
        }
        world.insert_resource(ClearColor(Color::srgb(0.62, 0.72, 0.82)));
    }
}

pub struct CityPlugin;

impl Plugin for CityPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "grade.wgsl");
        ground::install(app);
        app.add_plugins(bevy::core_pipeline::fullscreen_material::FullscreenMaterialPlugin::<ambiance::PostGrade>::default());
        app.init_resource::<stream::City>()
            .init_resource::<presentation::CityEffects>()
            .init_resource::<presentation::CityAmbience>()
            .add_systems(Startup, spawn_overlay)
            .add_systems(Update, (update_city.before(crate::player::PlayerSet), hold_player.after(crate::player::PlayerSet), update_overlay))
            .add_systems(PostUpdate, (ambiance::follow_sky, presentation::update_effects, presentation::update_ambience, presentation::log_audio).chain().after(bevy::transform::TransformSystems::Propagate));
    }
}

#[allow(clippy::too_many_arguments)]
/// Below the lowest Damascus terrain (about −70 m in the countryside ring).
const VOID_Y: f32 = -150.0;

pub(crate) fn update_city(mut city: ResMut<stream::City>, time: Res<Time>, spawn: Res<crate::player::SpawnPoint>, mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>, mut grounds: ResMut<Assets<ground::GroundSurface>>, mut images: ResMut<Assets<Image>>, mut collision: ResMut<crate::collision::CollisionWorld>,
    mut guidance: ResMut<crate::guidance::GuidanceWorld>, mut players: Query<(&mut crate::player::Body, &mut crate::player::Locomotion, &mut crate::player::HumanDataBundle), With<crate::player::Player>>,
    cams: Query<(&GlobalTransform, Option<&Projection>), With<crate::camera::MainCamera>>) {
    let Some(state) = city.0.as_mut() else { return };
    let Ok((mut body, mut loco, mut data)) = players.single_mut() else { return };
    // PORT: falling out of the world (past the grid's ground) puts the player back at the spawn through the game's
    // teleport (re-entering Ground, RE/24 CENSUS-6); the game's OutOfBounds / desynchronisation handling is not ported.
    if body.feet.y < VOID_Y { crate::player::teleport(&mut loco, &mut data, &mut body, spawn.0); }
    // AC_CITY_TOUR=<m/s>: debug fly-through, 25 m above the spawn, straight across the grid (streaming / LOD stress)
    if let Some(speed) = std::env::var("AC_CITY_TOUR").ok().and_then(|v| v.parse::<f32>().ok()) {
        let height = std::env::var("AC_CITY_TOUR_HEIGHT").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(25.0);
        let start = *state.tour.get_or_insert(body.feet + Vec3::Y * height);
        let t = time.elapsed_secs();
        let dir = Vec3::new(-1.0, 0.0, 0.35).normalize();
        body.feet = start + dir * (speed * t);
        body.velocity = Vec3::ZERO;
    }
    let (eye, view) = cams.iter().next().map_or((body.feet, Vec3::NEG_Z), |(t, _)| (t.translation(), t.forward().as_vec3()));
    // 0xA2F890: renderer +684 = tan(fov_y / 2) · 2.414213, the LOD distance scale (1 at a 45° vertical field of view)
    let fov = cams.iter().next().and_then(|(_, p)| match p { Some(Projection::Perspective(p)) => Some(p.fov), _ => None }).unwrap_or(std::f32::consts::FRAC_PI_4);
    let lod_scale = (fov * 0.5).tan() * 2.414_213;
    let mut ctx = stream::Ctx { commands: &mut commands, meshes: &mut meshes, materials: &mut materials, grounds: &mut grounds, images: &mut images,
        collision: &mut collision, guidance: &mut guidance };
    state.update(body.feet, eye, view, lod_scale, time.delta_secs(), &mut ctx);
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
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
        // AC_CITY_OVERLAY=1: shown from the start (screenshots)
        if std::env::var("AC_CITY_OVERLAY").is_ok_and(|v| v == "1") { Visibility::Inherited } else { Visibility::Hidden },
        Node { position_type: PositionType::Absolute, bottom: Val::Px(10.0), right: Val::Px(12.0), padding: UiRect::all(Val::Px(6.0)), ..default() }));
}

/// F3: streaming counters.
fn update_overlay(keys: Res<ButtonInput<KeyCode>>, city: Res<stream::City>, fx: Res<presentation::CityEffects>,
    amb: Res<presentation::CityAmbience>, time: Res<Time>,
    mut q: Query<(&mut Text, &mut Visibility), With<Overlay>>) {
    let Ok((mut text, mut vis)) = q.single_mut() else { return };
    if keys.just_pressed(KeyCode::F3) { *vis = if *vis == Visibility::Hidden { Visibility::Inherited } else { Visibility::Hidden }; }
    let Some(s) = city.0.as_ref().map(|c| &c.stats) else { text.0 = "no streamed world".into(); return };
    let (effects, running) = fx.counts();
    let (sounds, playing) = amb.counts();
    let beds = amb.beds().join(", ");
    text.0 = format!("city: {} cells loaded, {} loading | {} objects, {} drawn\nphysics: {} objects, {} triangles, {} edges\n\
        scene: {} lights, {} effects ({} running), {} sound emitters ({} playing)\nambience: {}\nGPU: {} materials, {} textures | {:.1} ms frame{}",
        s.loaded, s.loading, s.objects, s.drawn, s.physics_objects, s.physics_triangles, s.physics_edges, s.lights, effects, running, sounds, playing, beds,
        s.materials, s.textures,
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

    /// The collision triangles under `LAB_POS` ("x y z", game coordinates, Z up), from 0.5 m above it to 3 m below:
    /// object, cell, layer, height and normal (live-analysis probe for surfaces the port's Damascus has and the game
    /// doesn't).
    #[test]
    #[ignore]
    fn probe_collision_under() {
        let Some(w) = damascus() else { return };
        let Some(p) = std::env::var("LAB_POS").ok().map(|s| s.split_whitespace().filter_map(|x| x.parse().ok()).collect::<Vec<f32>>()) else { return };
        let (x, z, top) = (p[0], -p[1], p[2]);
        let game = |v: Vec3| Vec3::new(v.x, -v.z, v.y);
        for cell in 0..w.data.grid.cells.len() {
            let Ok(d) = decode::decode_cell(&w.data, cell) else { continue };
            for o in &d.objects {
                for t in &o.collision {
                    let [a, b, c] = t.vertices;
                    if t.normal.y.abs() < 1e-4 { continue; }
                    // inside the triangle in the horizontal plane, then the plane's height there
                    let s = |u: Vec3, v: Vec3| (v.x - u.x) * (z - u.z) - (v.z - u.z) * (x - u.x);
                    let (s0, s1, s2) = (s(a, b), s(b, c), s(c, a));
                    if !((s0 >= 0.0 && s1 >= 0.0 && s2 >= 0.0) || (s0 <= 0.0 && s1 <= 0.0 && s2 <= 0.0)) { continue; }
                    let y = a.y - (t.normal.x * (x - a.x) + t.normal.z * (z - a.z)) / t.normal.y;
                    if y <= top + 0.5 && y >= top - 3.0 {
                        println!("{} (cell {cell}) layer {} height {y:.3} normal {:.2?} material {} at {:.2?}", o.name, t.layer, game(t.normal), t.material, game(o.transform.w_axis.truncate()));
                    }
                }
            }
        }
    }

    /// Writes the entity bodies of cell `LAB_CELL` whose name contains `LAB_OBJ` to `LAB_OUT`-<n>.bin (live-analysis
    /// probe: their component classes are then looked up against the reflected class hashes).
    #[test]
    #[ignore]
    fn probe_dump_bodies() {
        let Some(w) = damascus() else { return };
        let (Ok(cell), Ok(obj), Ok(out)) = (std::env::var("LAB_CELL"), std::env::var("LAB_OBJ"), std::env::var("LAB_OUT")) else { return };
        let cells: Vec<usize> = if cell == "all" { (0..w.data.grid.cells.len()).collect() } else { vec![cell.parse().unwrap()] };
        let mut n = 0;
        for cell in cells {
        let c = &w.data.grid.cells[cell];
        let Some((archive, file)) = w.data.archives.location(c.datablock).map(|(a, f)| (a.to_string(), f.to_string())) else { continue };
        let a = w.data.archives.names.iter().position(|n| *n == archive).unwrap();
        let Some(entry) = w.data.archives.find(a, &file) else { continue };
        let Ok(res) = w.data.archives.file(a, entry) else { continue };
        for r in res.values() {
            for body in decode::bodies(r) {
                let name = if body.as_ptr() == r.payload.as_ptr() { r.name.clone() } else { format!("{}/{:08x}", r.name, u32::from_le_bytes(body[..4].try_into().unwrap())) };
                if name.contains(&obj) {
                    std::fs::write(format!("{out}-{n}.bin"), body).unwrap();
                    println!("{name} (cell {cell}): {} bytes -> {out}-{n}.bin", body.len());
                    n += 1;
                }
            }
        }
        }
    }

    /// Every entity with collision that a Scene moves (`BlendPosClip`): the scripted movers the decoder leaves out.
    #[test]
    #[ignore]
    fn probe_scene_movers() {
        let Some(w) = damascus() else { return };
        let mut n = 0;
        for cell in 0..w.data.grid.cells.len() {
            let c = &w.data.grid.cells[cell];
            let Some((archive, file)) = w.data.archives.location(c.datablock).map(|(a, f)| (a.to_string(), f.to_string())) else { continue };
            let a = w.data.archives.names.iter().position(|n| *n == archive).unwrap();
            let Some(entry) = w.data.archives.find(a, &file) else { continue };
            let Ok(res) = w.data.archives.file(a, entry) else { continue };
            for r in res.values() {
                for body in decode::bodies(r) {
                    if decode::markers(body, "BlendPosClip").is_empty() || decode::markers(body, "RigidBody").is_empty() { continue; }
                    let name = if body.as_ptr() == r.payload.as_ptr() { r.name.clone() } else { format!("{}/{:08x}", r.name, u32::from_le_bytes(body[..4].try_into().unwrap())) };
                    println!("{name} (cell {cell})");
                    n += 1;
                }
            }
        }
        println!("{n} scene-moved entities with collision");
    }

    /// How many entities carry the procedural guidance systems (`GuidanceSystemCapsule` / `GuidanceSystemBarrel`, RE/06
    /// §2), with a few names and positions (game coordinates).
    #[test]
    #[ignore]
    fn probe_procedural_guidance() {
        let Some(w) = damascus() else { return };
        let mut counts = std::collections::BTreeMap::<&str, usize>::new();
        let mut names = std::collections::BTreeMap::<String, usize>::new();
        for cell in 0..w.data.grid.cells.len() {
            let c = &w.data.grid.cells[cell];
            let Some((archive, file)) = w.data.archives.location(c.datablock).map(|(a, f)| (a.to_string(), f.to_string())) else { continue };
            let a = w.data.archives.names.iter().position(|n| *n == archive).unwrap();
            let Some(entry) = w.data.archives.find(a, &file) else { continue };
            let Ok(res) = w.data.archives.file(a, entry) else { continue };
            for r in res.values() {
                for body in decode::bodies(r) {
                    for class in ["GuidanceSystemCapsule", "GuidanceSystemBarrel", "GuidanceSystem"] {
                        if !decode::markers(body, class).is_empty() {
                            *counts.entry(class).or_default() += 1;
                            if class != "GuidanceSystem" {
                                let base = r.name.split('_').take(3).collect::<Vec<_>>().join("_");
                                *names.entry(format!("{class} {base}")).or_default() += 1;
                            }
                        }
                    }
                }
            }
        }
        println!("{counts:?}");
        let mut v: Vec<_> = names.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        for (n, k) in v.iter().take(40) { println!("{k:5} {n}"); }
    }

    /// The guidance components whose active byte is clear: their entities, edge counts and positions (game coordinates);
    /// with `LAB_POS` ("x y", game) only those within 15 m.
    #[test]
    #[ignore]
    fn probe_inactive_guidance() {
        let Some(w) = damascus() else { return };
        let near = std::env::var("LAB_POS").ok().map(|s| { let v: Vec<f32> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect(); Vec2::new(v[0], v[1]) });
        let (mut comps, mut edges) = (0, 0);
        let mut names = std::collections::BTreeMap::<String, usize>::new();
        for cell in 0..w.data.grid.cells.len() {
            let c = &w.data.grid.cells[cell];
            let Some((archive, file)) = w.data.archives.location(c.datablock).map(|(a, f)| (a.to_string(), f.to_string())) else { continue };
            let a = w.data.archives.names.iter().position(|n| *n == archive).unwrap();
            let Some(entry) = w.data.archives.find(a, &file) else { continue };
            let Ok(res) = w.data.archives.file(a, entry) else { continue };
            for r in res.values() {
                for body in decode::bodies(r) {
                    for g in decode::markers(body, "GuidanceSystem") {
                        let Some(start) = g.checked_sub(4) else { continue };
                        let Ok(parsed) = crate::assets::world::parse_guidance(&body[start..]) else { continue };
                        if parsed.active { continue; }
                        let m = &body[8..72];
                        let f = |i: usize| f32::from_le_bytes(m[i * 4..i * 4 + 4].try_into().unwrap());
                        let pos = Vec3::new(f(12), f(13), f(14));
                        if near.is_some_and(|n| Vec2::new(pos.x, pos.y).distance(n) > 15.0) { continue; }
                        comps += 1; edges += parsed.edges.len();
                        let base = r.name.split('_').take(4).collect::<Vec<_>>().join("_");
                        *names.entry(base).or_default() += parsed.edges.len();
                        if near.is_some() { println!("inactive {} at {pos:.2}: {} edges", r.name, parsed.edges.len()); }
                    }
                }
            }
        }
        println!("{comps} inactive components, {edges} edges");
        let mut v: Vec<_> = names.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        for (n, k) in v.iter().take(25) { println!("{k:5} {n}"); }
    }

    /// The haystack objects' own collision: layers, triangle count, the hull's vertical extent against the hay volume
    /// (AIR-25 probe).
    #[test]
    #[ignore]
    fn probe_damascus_haystack_collision() {
        let Some(w) = damascus() else { return };
        let mut n = 0;
        for cell in 0..w.data.grid.cells.len() {
            let Ok(d) = decode::decode_cell(&w.data, cell) else { continue };
            for o in d.objects.iter().filter(|o| o.haystack.is_some()) {
                let mut layers = std::collections::BTreeMap::new();
                let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                for t in &o.collision {
                    *layers.entry(t.layer).or_insert(0) += 1;
                    lo = lo.min(t.bounds.min.y);
                    hi = hi.max(t.bounds.max.y);
                }
                let h = o.haystack.unwrap();
                let up = o.collision.iter().filter(|t| t.normal.y > 0.7).count();
                let down = o.collision.iter().filter(|t| t.normal.y < -0.7).count();
                println!("{}: hay {:.2}..{:.2}; collision {layers:?} y {lo:.2}..{hi:.2}, up-facing {up}, down-facing {down}", o.name, h.bounds.min.y, h.bounds.max.y);
                n += 1;
                if n >= 12 { return; }
            }
        }
    }

    /// Live-analysis courses: every player spawner, then the haystacks, ladders, beams, poles, kiosks and long
    /// ledge edges within `LAB_RADIUS` (default 40) m of `LAB_CENTER` ("x y", game coordinates, Z up; default the
    /// Bureau spawner), all printed in game coordinates.
    #[test]
    #[ignore]
    fn probe_lab_courses() {
        let Some(w) = damascus() else { return };
        let game = |p: Vec3| Vec3::new(p.x, -p.z, p.y);
        for r in w.globals.iter().filter(|r| r.name.starts_with("PlayerSpawner")) {
            if let Some((p, h)) = spawner(&w.globals, &r.name) {
                println!("spawner {} at {:.2?} heading {h:.2}", r.name, game(p));
            }
        }
        let center = std::env::var("LAB_CENTER").ok().and_then(|s| {
            let v: Vec<f32> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.len() == 2).then(|| Vec2::new(v[0], v[1]))
        }).unwrap_or_else(|| { let p = game(spawner(&w.globals, "PlayerSpawner_DEBUG_Bureau").unwrap().0); Vec2::new(p.x, p.y) });
        let radius: f32 = std::env::var("LAB_RADIUS").ok().and_then(|s| s.parse().ok()).unwrap_or(40.0);
        let near = |p: Vec3| { let g = game(p); Vec2::new(g.x, g.y).distance(center) < radius };
        for cell in 0..w.data.grid.cells.len() {
            let Ok(d) = decode::decode_cell(&w.data, cell) else { continue };
            for o in &d.objects {
                if let Some(h) = o.haystack.filter(|h| near((h.bounds.min + h.bounds.max) * 0.5)) {
                    println!("haystack {} centre {:.2?} top {:.2}", o.name, game((h.bounds.min + h.bounds.max) * 0.5), h.bounds.max.y);
                }
                for e in o.edges.iter().filter(|e| near((e.p0 + e.p1) * 0.5)) {
                    use crate::guidance::GuidanceSubType as T;
                    let len = e.p0.distance(e.p1);
                    let keep = match e.subtype { T::Ladder | T::Beam | T::Pole | T::Kiosk | T::Rope => true, T::LedgeGrab => len > std::env::var("LAB_MINLEN").ok().and_then(|v| v.parse().ok()).unwrap_or(4.0), _ => false };
                    if keep {
                        println!("{:?} {} {:.2?} -> {:.2?} len {len:.1} wall {:.2?}", e.subtype, o.name, game(e.p0), game(e.p1), game(e.n1));
                    }
                }
            }
        }
    }

    /// Live-analysis sites for the pilotis catch: tops made of LedgeGrab edges no longer than 0.7 m whose ends join
    /// into a closed loop (a post's top, `Human__FindPilotisTop` 0xB2B600 wants front–back and left–right ≤ 0.7 m),
    /// within `LAB_RADIUS` (default 60) m of `LAB_CENTER` (default the Bureau spawner); game coordinates, Z up.
    #[test]
    #[ignore]
    fn probe_lab_pilotis() {
        let Some(w) = damascus() else { return };
        let game = |p: Vec3| Vec3::new(p.x, -p.z, p.y);
        let center = std::env::var("LAB_CENTER").ok().and_then(|s| {
            let v: Vec<f32> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.len() == 2).then(|| Vec2::new(v[0], v[1]))
        }).unwrap_or_else(|| { let p = game(spawner(&w.globals, "PlayerSpawner_DEBUG_Bureau").unwrap().0); Vec2::new(p.x, p.y) });
        let radius: f32 = std::env::var("LAB_RADIUS").ok().and_then(|s| s.parse().ok()).unwrap_or(60.0);
        let mut found = 0;
        for cell in 0..w.data.grid.cells.len() {
            let Ok(d) = decode::decode_cell(&w.data, cell) else { continue };
            for o in &d.objects {
                use crate::guidance::GuidanceSubType as T;
                let short: Vec<_> = o.edges.iter().filter(|e| e.subtype == T::LedgeGrab && e.p0.distance(e.p1) <= 0.75 && (e.p0.y - e.p1.y).abs() < 0.05).collect();
                for a in &short {
                    // a loop of four: each end meets another short edge's end at the same height
                    let joins = |p: Vec3| short.iter().filter(|b| b.p0.distance(p) < 0.05 || b.p1.distance(p) < 0.05).count() >= 2;
                    if !(joins(a.p0) && joins(a.p1)) { continue; }
                    let ring: Vec<_> = short.iter().filter(|b| (b.p0.y - a.p0.y).abs() < 0.05 && b.p0.distance(a.p0) < 1.1).collect();
                    if ring.len() != 4 { continue; }
                    let centre = ring.iter().map(|b| (b.p0 + b.p1) * 0.5).sum::<Vec3>() * 0.25;
                    // print each top once, from its first edge
                    if !std::ptr::eq(*ring[0], *a) { continue; }
                    let g = game(centre);
                    if Vec2::new(g.x, g.y).distance(center) > radius { continue; }
                    let size: Vec<f32> = ring.iter().map(|b| b.p0.distance(b.p1)).collect();
                    println!("pilotis {} top {:.2?} sides {size:.2?}", o.name, g);
                    found += 1;
                    if found >= 40 { return; }
                }
            }
        }
    }

    /// The drawn objects over `LAB_POS` ("x y", game coordinates): their mesh sections' materials (template, every
    /// TextureSet channel, the parameter dictionary) and the vertex / instance colour ranges (ground-material probe).
    #[test]
    #[ignore]
    fn probe_ground_materials() {
        use crate::assets::static_mesh::word;
        let Some(w) = damascus() else { return };
        let Some(p) = std::env::var("LAB_POS").ok().map(|s| s.split_whitespace().filter_map(|x| x.parse().ok()).collect::<Vec<f32>>()) else { return };
        let (x, z) = (p[0], -p[1]);
        let a = &w.data.archives;
        let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let mut seen = std::collections::HashSet::new();
        for cell in 0..w.data.grid.cells.len() {
            let Ok(d) = decode::decode_cell(&w.data, cell) else { continue };
            for o in &d.objects {
                let b = &o.visual_bounds;
                if o.meshes.is_empty() || x < b.min.x || x > b.max.x || z < b.min.z || z > b.max.z { continue; }
                let m = &o.meshes[0];
                println!("== {} (cell {cell}) mesh {:#x} {} verts, y {:.1}..{:.1}", o.name, m.id, m.mesh.positions.len(), b.min.y, b.max.y);
                let range = |c: &mut dyn Iterator<Item = [f32; 4]>| { let (mut lo, mut hi) = ([1f32; 4], [0f32; 4]); for v in c { for k in 0..4 { lo[k] = lo[k].min(v[k]); hi[k] = hi[k].max(v[k]); } } (lo, hi) };
                println!("   vertex colour {:.2?}", range(&mut m.mesh.colors.iter().copied()));
                if let Some(c) = &m.colors { println!("   instance colour {:.2?}", range(&mut c.iter().map(|c| c.map(|b| b as f32 / 255.0)))); }
                let uv = m.mesh.uvs.iter().fold((Vec2::MAX, Vec2::MIN), |(l, h), u| (l.min(*u), h.max(*u)));
                println!("   uv {uv:.2?}");
                for s in &m.mesh.sections {
                    let Ok(r) = a.get(s.material) else { continue };
                    let t = a.get(word(&r.payload, 8).unwrap_or(0)).map(|t| t.name.clone()).unwrap_or_default();
                    println!("   section {} tris: {} [{t}]", s.indices.len() / 3, r.name);
                    if !seen.insert(s.material) { continue; }
                    println!("     material {}", hex(&r.payload));
                    let Ok(set) = a.get(word(&r.payload, 12).unwrap_or(0)) else { continue };
                    for q in (8..set.payload.len().saturating_sub(3)).step_by(4) {
                        let id = word(&set.payload, q).unwrap();
                        if id == 0 || !a.contains(id) { continue; }
                        let spec = a.get(id).unwrap();
                        let tex = word(&spec.payload, spec.payload.len().saturating_sub(4)).ok().and_then(|t| a.get(t).ok()).map(|t| t.name.clone()).unwrap_or_default();
                        println!("     set +{q}: {} -> {tex}\n       {}", spec.name, hex(&spec.payload));
                    }
                }
            }
        }
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
