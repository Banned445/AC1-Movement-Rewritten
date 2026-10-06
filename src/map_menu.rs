//! PORT: F2 debug map menu and scene lifecycle; the original game's menu is not recreated.
use bevy::{ecs::system::RunSystemOnce, prelude::*, window::{CursorGrabMode, CursorOptions, PrimaryWindow}};
use crate::player::{Player, SpawnPoint};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Map { #[default] Greybox, MasyafRoofs, MasyafVillage }
impl Map {
    pub fn label(self) -> &'static str {
        match self { Self::Greybox => "Greybox test level", Self::MasyafRoofs => "Masyaf roofs (experimental)", Self::MasyafVillage => "Masyaf village (experimental)" }
    }
}

#[derive(Component)]
pub(crate) struct MapEntity;
#[derive(Component)]
struct MenuRoot;
#[derive(Component)]
struct MenuStatus;
#[derive(Component)]
struct MapButton(Map);

#[derive(Resource, Default)]
pub struct MapMenu {
    pub active: Map,
    pub open: bool,
    pending: Option<Map>,
    error: Option<String>,
    was_paused: bool,
    cursor_grab: Option<CursorGrabMode>,
    cursor_visible: bool,
}

pub struct MapMenuPlugin;
impl Plugin for MapMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapMenu>()
            .add_systems(Startup, create_menu)
            .add_systems(PostStartup, initialize)
            .add_systems(PreUpdate, (controls, apply_pending).chain().before(crate::input::read_pad))
            .add_systems(Update, display)
            .configure_sets(Update, crate::player::PlayerSet.run_if(|menu: Res<MapMenu>| !menu.open));
    }
}

fn create_menu(mut commands: Commands) {
    commands.spawn((MenuRoot, Visibility::Hidden, GlobalZIndex(100),
        Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), align_items: AlignItems::Center, justify_content: JustifyContent::Center, ..default() },
        BackgroundColor(Color::srgba(0.02,0.03,0.05,0.75))))
        .with_children(|root| {
            root.spawn((Node { width: px(480), padding: UiRect::all(px(24)), flex_direction: FlexDirection::Column, row_gap: px(16), ..default() }, BackgroundColor(Color::srgb(0.10,0.13,0.18))))
                .with_children(|panel| {
                    panel.spawn((Text::new("Debug maps"), TextFont { font_size: FontSize::Px(26.0), ..default() }));
                    panel.spawn((MenuStatus, Text::new(""), TextFont { font_size: FontSize::Px(16.0), ..default() }));
                    for (key, map) in [("1",Map::Greybox),("2",Map::MasyafRoofs),("3",Map::MasyafVillage)] {
                        panel.spawn((Button, MapButton(map), Node { padding: UiRect::all(px(14)), ..default() }, BackgroundColor(Color::srgb(0.20,0.27,0.36))))
                            .with_children(|button| { button.spawn((Text::new(format!("{key}  {}",map.label())), TextFont { font_size: FontSize::Px(18.0), ..default() })); });
                    }
                    panel.spawn((Text::new("Click a map or press 1 / 2 / 3.\nF2 or Esc: close. Switching resets your position.\nMasyaf requires your installed game files."), TextFont { font_size: FontSize::Px(15.0), ..default() }));
                });
        });
}

pub(crate) fn initialize(world: &mut World) {
    // PORT: optional startup-open switch for debug UI inspection.
    world.resource_mut::<MapMenu>().open = std::env::var_os("AC_DEBUG_MAP_MENU").is_some();
    // Environment selection remains supported for automated captures and existing workflows.
    let initial = if std::env::var(crate::native_map::NATIVE_MAP_IMPORT).is_ok_and(|v|v=="masyaf-village") { Map::MasyafVillage }
        else if crate::native_map::enabled() { Map::MasyafRoofs } else { Map::Greybox };
    if let Err(error) = switch(world, initial) {
        warn!("map import failed: {error}");
        switch(world, Map::Greybox).expect("greybox must load");
        let mut menu = world.resource_mut::<MapMenu>();
        menu.error = Some(error); menu.open = true;
    }
}

fn controls(keys: Res<ButtonInput<KeyCode>>, mut menu: ResMut<MapMenu>, mut time: ResMut<Time<Virtual>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>, buttons: Query<(&Interaction,&MapButton), Changed<Interaction>>) {
    if keys.just_pressed(KeyCode::F2) || (menu.open && keys.just_pressed(KeyCode::Escape)) {
        menu.open = !menu.open;
    }
    // Reconcile pause/cursor every frame, including a startup import failure opening the panel.
    if menu.open && menu.cursor_grab.is_none() {
        menu.was_paused = time.is_paused(); time.pause();
        if let Ok(mut c) = cursor.single_mut() {
            menu.cursor_grab = Some(c.grab_mode); menu.cursor_visible = c.visible;
            c.grab_mode = CursorGrabMode::None; c.visible = true;
        } else { menu.cursor_grab = Some(CursorGrabMode::None); }
    } else if !menu.open {
        if let Some(grab) = menu.cursor_grab.take() {
            if !menu.was_paused { time.unpause(); }
            if let Ok(mut c) = cursor.single_mut() { c.grab_mode = grab; c.visible = menu.cursor_visible; }
        }
    }
    if !menu.open { return; }
    if keys.just_pressed(KeyCode::Digit1) { menu.pending = Some(Map::Greybox); }
    if keys.just_pressed(KeyCode::Digit2) { menu.pending = Some(Map::MasyafRoofs); }
    if keys.just_pressed(KeyCode::Digit3) { menu.pending = Some(Map::MasyafVillage); }
    for (interaction, button) in &buttons {
        if *interaction == Interaction::Pressed { menu.pending = Some(button.0); }
    }
}

fn display(menu: Res<MapMenu>, mut roots: Query<&mut Visibility, With<MenuRoot>>,
    mut status: Query<&mut Text, With<MenuStatus>>, mut buttons: Query<(&Interaction,&MapButton,&mut BackgroundColor)>) {
    for mut visibility in &mut roots { *visibility = if menu.open { Visibility::Inherited } else { Visibility::Hidden }; }
    for mut text in &mut status {
        text.0 = format!("Current: {}{}", menu.active.label(), menu.error.as_ref().map(|e| format!("\nCould not load map: {e}")).unwrap_or_default());
    }
    for (interaction, button, mut color) in &mut buttons {
        color.0 = if *interaction == Interaction::Hovered { Color::srgb(0.30,0.40,0.53) }
            else if button.0 == menu.active { Color::srgb(0.16,0.38,0.32) } else { Color::srgb(0.20,0.27,0.36) };
    }
}

fn apply_pending(world: &mut World) {
    let pending = world.resource_mut::<MapMenu>().pending.take();
    if let Some(map) = pending {
        if world.resource::<MapMenu>().active == map { return; }
        if let Err(error) = switch(world, map) { world.resource_mut::<MapMenu>().error = Some(error); }
    }
}

fn switch(world: &mut World, map: Map) -> Result<(), String> {
    switch_from(world, map, &crate::assets::game_dir())
}

fn switch_from(world: &mut World, map: Map, game: &std::path::Path) -> Result<(), String> {
    // Decode before removing the current scene: failures retain its render/collision/player state.
    let imported = match map { Map::MasyafRoofs=>Some(crate::native_map::load(game)?),Map::MasyafVillage=>Some(crate::native_map::load_village(game)?),Map::Greybox=>None };
    let entities: Vec<_> = world.query_filtered::<Entity,With<MapEntity>>().iter(world).collect();
    for entity in entities { world.despawn(entity); }
    if let Some(imported) = imported {
        world.run_system_once_with(crate::native_map::spawn, imported).map_err(|e| e.to_string())?;
    } else {
        world.run_system_once(crate::level::build_level).map_err(|e| e.to_string())?;
        world.resource_mut::<SpawnPoint>().0 = Vec3::new(0.0,0.0,4.0);
        *world.resource_mut::<crate::camera::CameraRig>() = default();
    }
    reset_player(world);
    let mut menu = world.resource_mut::<MapMenu>();
    menu.active = map; menu.error = None;
    Ok(())
}

fn reset_player(world: &mut World) {
    let spawn = world.resource::<SpawnPoint>().0;
    let players: Vec<_> = world.query_filtered::<Entity,With<Player>>().iter(world).collect();
    for entity in players {
        world.entity_mut(entity).insert((crate::player::player_components(spawn,std::f32::consts::PI), Transform::from_translation(spawn)));
        if world.entity(entity).contains::<crate::anim::AnimPlayer>() {
            world.entity_mut(entity).insert(crate::anim::AnimPlayer::default());
        }
    }
    *world.resource_mut::<crate::input::PadInput>() = neutral_pad();
    let mut rig = world.resource_mut::<crate::camera::CameraRig>();
    rig.shake = 0.0;
    let focus = spawn + Vec3::Y * 1.5;
    let direction = Quat::from_euler(EulerRot::YXZ,rig.yaw,rig.pitch,0.0) * Vec3::Z;
    let distance = rig.distance;
    let collision = world.resource::<crate::collision::CollisionWorld>();
    let distance = if collision.triangles.is_empty() { distance }
        else { (collision.camera_distance(focus,direction,distance)-0.20).clamp(0.25,distance) };
    let eye = focus + direction * distance;
    for mut camera in world.query_filtered::<&mut Transform,With<crate::camera::MainCamera>>().iter_mut(world) {
        *camera = Transform::from_translation(eye).looking_at(focus,Vec3::Y);
    }
}

pub(crate) fn neutral_pad() -> crate::input::PadInput {
    crate::input::PadInput { legs_pressed_ago: f32::INFINITY, hand_pressed_ago: f32::INFINITY, ..default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{collision::CollisionWorld, guidance::GuidanceWorld, player::{Body, HumanDataBundle, LimbTargets, Locomotion, ActorContextId}};

    fn scene_world() -> World {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<StandardMaterial>::default());
        world.insert_resource(Assets::<Image>::default());
        world.insert_resource(CollisionWorld::default());
        world.insert_resource(GuidanceWorld::default());
        world.insert_resource(SpawnPoint(Vec3::ZERO));
        world.insert_resource(crate::camera::CameraRig::default());
        world.insert_resource(neutral_pad());
        world.insert_resource(MapMenu::default());
        world.spawn((crate::player::player_components(Vec3::ZERO,0.0), Transform::default(), crate::anim::AnimPlayer::default()));
        world
    }

    fn map_count(world: &mut World) -> usize { world.query_filtered::<Entity,With<MapEntity>>().iter(world).count() }

    #[test]
    fn failed_import_preserves_the_current_scene_and_player() {
        let mut world = scene_world();
        switch(&mut world,Map::Greybox).unwrap();
        let count = map_count(&mut world);
        let boxes = world.resource::<CollisionWorld>().boxes.len();
        let player = world.query_filtered::<Entity,With<Player>>().single(&world).unwrap();
        world.get_mut::<Body>(player).unwrap().velocity = Vec3::X;
        let error = switch_from(&mut world,Map::MasyafRoofs,std::path::Path::new("__absent_map_install__"));
        assert!(error.is_err());
        assert_eq!(world.resource::<MapMenu>().active,Map::Greybox);
        assert_eq!(map_count(&mut world),count);
        assert_eq!(world.resource::<CollisionWorld>().boxes.len(),boxes);
        assert_eq!(world.get::<Body>(player).unwrap().velocity,Vec3::X);
    }

    #[test]
    fn repeated_map_switches_replace_geometry_and_reset_climbing_state() {
        let Some(game) = crate::assets::find_game_dir() else { return; };
        if !game.join("DataPC_Masyaf.forge").is_file() { return; }
        let mut world = scene_world();
        let unrelated = world.spawn(Name::new("preserved unrelated entity")).id();
        switch(&mut world,Map::Greybox).unwrap();
        let greybox_count = map_count(&mut world);
        let player = world.query_filtered::<Entity,With<Player>>().single(&world).unwrap();
        let mut native_count = None;
        for _ in 0..2 {
            world.get_mut::<Locomotion>(player).unwrap().current = ActorContextId::Climb;
            world.get_mut::<LimbTargets>(player).unwrap().hands = Some((Vec3::ONE,Vec3::ONE));
            world.get_mut::<HumanDataBundle>(player).unwrap().climb.pose = 4;
            world.get_mut::<Body>(player).unwrap().proxy.layer = crate::layers::MAIN_CHARACTER_NO_STATIC;
            switch_from(&mut world,Map::MasyafRoofs,&game).unwrap();
            let count = map_count(&mut world);
            assert_eq!(*native_count.get_or_insert(count),count);
            assert!(world.resource::<CollisionWorld>().boxes.is_empty());
            assert_eq!(world.resource::<CollisionWorld>().triangles.len(),3757);
            assert_eq!(world.resource::<GuidanceWorld>().edges.len(),283);
            assert_eq!(world.get::<Locomotion>(player).unwrap().current,ActorContextId::Ground);
            assert!(world.get::<LimbTargets>(player).unwrap().hands.is_none());
            assert_eq!(world.get::<Body>(player).unwrap().feet,world.resource::<SpawnPoint>().0);
            assert!(world.get::<Body>(player).unwrap().proxy.manifold.is_empty());
            switch(&mut world,Map::Greybox).unwrap();
            assert_eq!(map_count(&mut world),greybox_count);
            assert!(world.resource::<CollisionWorld>().triangles.is_empty());
            assert_eq!(world.resource::<SpawnPoint>().0,Vec3::new(0.0,0.0,4.0));
            assert!(world.get_entity(unrelated).is_ok());
            assert_eq!(world.query_filtered::<Entity,With<DirectionalLight>>().iter(&world).count(),1);
        }
    }

    #[test]
    fn menu_keyboard_pauses_restores_cursor_and_does_not_resume_an_existing_pause() {
        let mut world = World::new();
        world.insert_resource(MapMenu::default());
        world.insert_resource(Time::<Virtual>::default());
        world.insert_resource(ButtonInput::<KeyCode>::default());
        let window = world.spawn((PrimaryWindow,CursorOptions { grab_mode: CursorGrabMode::Locked, visible: false, ..default() })).id();
        for already_paused in [false,true] {
            if already_paused { world.resource_mut::<Time<Virtual>>().pause(); }
            world.resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::F2);
            world.run_system_once(controls).unwrap();
            assert!(world.resource::<MapMenu>().open);
            assert!(world.resource::<Time<Virtual>>().is_paused());
            assert_eq!(world.get::<CursorOptions>(window).unwrap().grab_mode,CursorGrabMode::None);
            world.resource_mut::<ButtonInput<KeyCode>>().reset_all();
            world.resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::Digit2);
            world.run_system_once(controls).unwrap();
            assert_eq!(world.resource::<MapMenu>().pending,Some(Map::MasyafRoofs));
            world.resource_mut::<ButtonInput<KeyCode>>().reset_all();
            world.resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::F2);
            world.run_system_once(controls).unwrap();
            assert!(!world.resource::<MapMenu>().open);
            assert_eq!(world.resource::<Time<Virtual>>().is_paused(),already_paused);
            assert_eq!(world.get::<CursorOptions>(window).unwrap().grab_mode,CursorGrabMode::Locked);
            assert!(!world.get::<CursorOptions>(window).unwrap().visible);
            world.resource_mut::<ButtonInput<KeyCode>>().reset_all();
        }
    }

    #[test]
    fn village_switch_preserves_the_player_and_returns_to_greybox_without_stale_ground() {
        let Some(game) = crate::assets::find_game_dir() else { return; };
        if !game.join("DataPC_Masyaf.forge").is_file() { return; }
        let mut world = scene_world();
        switch(&mut world,Map::Greybox).unwrap();
        let count = map_count(&mut world);
        let player = world.query_filtered::<Entity,With<Player>>().single(&world).unwrap();
        switch_from(&mut world,Map::MasyafVillage,&game).unwrap();
        assert_eq!(world.resource::<MapMenu>().active,Map::MasyafVillage);
        assert_eq!(world.resource::<crate::collision::CollisionWorld>().triangles.len(),37964);
        assert_eq!(world.resource::<crate::guidance::GuidanceWorld>().edges.len(),2174);
        assert_eq!(world.get::<Body>(player).unwrap().feet,world.resource::<SpawnPoint>().0);
        switch(&mut world,Map::Greybox).unwrap();
        assert_eq!(map_count(&mut world),count);
        assert!(world.resource::<crate::collision::CollisionWorld>().triangles.is_empty());
        assert_eq!(world.resource::<MapMenu>().active,Map::Greybox);
    }
}
