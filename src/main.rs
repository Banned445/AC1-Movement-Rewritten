//! Assassin's Creed (2008) movement — clean-room recreation in Bevy.
//!
//! Stage 1: player locomotion-context framework, input, ground locomotion, jumps/falls/landing on a
//! greybox level. Behaviour is specified by the reverse-engineering notes in ../RE/*.md; no game
//! code or assets are included. Game assets will be loaded at runtime from the user's own install.

mod anim;
mod assets;
mod camera;
mod collision;
mod debug_capture;
mod guidance;
mod hud;
mod ik;
mod input;
mod layers;
mod level;
mod model;
mod cloth;
mod player;
mod proxy;
mod recorder;
#[cfg(test)]
mod sim_tests;
mod tuning;

use bevy::prelude::*;

fn main() {
    // `--game-dir <path>`: use (and remember) the Assassin's Creed install folder
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--game-dir") {
        let Some(dir) = args.get(i + 1) else {
            eprintln!("usage: ac_port --game-dir \"<your Assassin's Creed folder>\"");
            std::process::exit(2);
        };
        match assets::set_game_dir(std::path::Path::new(dir)) {
            Ok(file) => println!("game folder saved to {}", file.display()),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        }
    }
    match assets::find_game_dir() {
        Some(d) => println!("game folder: {}", d.display()),
        None => eprintln!(
            "Assassin's Creed not found: run with --game-dir \"<your Assassin's Creed folder>\" (or set AC_GAME_DIR). \
             Without it the port runs with a capsule and no animations."
        ),
    }
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "AC1 Movement Rewritten".into(),
                resolution: (1600, 900).into(),
                ..default()
            }),
            ..default()
        }))
        .init_resource::<collision::CollisionWorld>()
        .add_plugins((
            level::LevelPlugin,
            guidance::GuidancePlugin,
            input::InputPlugin,
            camera::CameraPlugin,
            player::PlayerPlugin,
            hud::HudPlugin,
            model::ModelPlugin,
            anim::AnimPlugin,
            ik::IkPlugin,
            debug_capture::DebugCapturePlugin,
            debug_capture::ShotsPlugin,
            recorder::RecorderPlugin,
        ))
        .run();
}
