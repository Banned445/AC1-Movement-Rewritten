//! `AC_RENDER_PROFILE=<file>`: the render world's time per render stage, for every render frame that takes longer
//! than 10 ms (the frame-time spikes `AC_FPS_LOG` finds where the main world's streaming work is small). A debug tool,
//! not part of the game.

use std::{io::Write, time::Instant};

use bevy::{prelude::*, render::{Render, RenderApp, RenderSystems as S}};

/// The stage boundaries: a mark runs after the stage before it and before the next.
const STAGES: [&str; 10] = ["extract_cmds", "prepare_assets", "prepare_meshes", "create_views", "specialize", "prepare_views", "queue", "sort", "prepare", "render"];

#[derive(Resource)]
struct Marks {
    file: std::fs::File,
    t: [Option<Instant>; 11],
    start: Instant,
    frame: u64,
    /// (seconds, frames, summed time per stage) over the current half second
    window: (Option<Instant>, u32, [f32; 10]),
}

/// The main world's Update-to-Last time (AC_RENDER_PROFILE too): (start of First, summed ms, frames, window start).
#[derive(Resource, Default)]
struct MainMarks { first: Option<Instant>, sum: f32, frames: u32, window: Option<Instant>, file: Option<std::fs::File>, passes: u32, assets: [u32; 3] }

/// Every fourth window: the render passes' GPU / CPU times (`RenderDiagnosticsPlugin`), smoothed.
fn log_passes(mut m: ResMut<MainMarks>, store: Res<bevy::diagnostic::DiagnosticsStore>, mut last: Local<u32>) {
    if m.passes == *last || m.passes % 4 != 0 { return; }
    *last = m.passes;
    let mut items: Vec<(String, f64)> = store.iter().filter(|d| d.path().as_str().starts_with("render/"))
        .filter_map(|d| Some((d.path().as_str().trim_start_matches("render/").to_string(), d.smoothed()?))).filter(|(_, v)| *v > 0.05).collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let line = items.iter().map(|(k, v)| format!("{k} {v:.2}")).collect::<Vec<_>>().join(" | ");
    if let Some(f) = m.file.as_mut() { let _ = writeln!(f, "passes: {line}"); }
}

/// A line whenever the pipeline cache's pipeline count changes (new shader variants compile on first use).
fn log_pipelines(mut m: ResMut<Marks>, cache: Res<bevy::render::render_resource::PipelineCache>, mut last: Local<(usize, usize)>) {
    let now = (cache.pipelines().count(), cache.waiting_pipelines().count());
    if now != *last && m.start.elapsed().as_secs_f32() > 2.0 {
        let line = format!("{:.2} pipelines {} waiting {}", m.start.elapsed().as_secs_f32(), now.0, now.1);
        let _ = writeln!(m.file, "{line}");
    }
    *last = now;
}

fn log_support(mut m: ResMut<Marks>, support: Option<Res<bevy::render::batching::gpu_preprocessing::GpuPreprocessingSupport>>,
    adapter: Option<Res<bevy::render::renderer::RenderAdapterInfo>>, mut done: Local<bool>) {
    if *done { return; }
    *done = true;
    let line = format!("adapter {:?} gpu preprocessing {:?}", adapter.map(|a| format!("{} {:?}", a.name, a.backend)), support.map(|s| s.is_available()));
    let _ = writeln!(m.file, "{line}");
}

fn main_first(mut m: ResMut<MainMarks>) { m.first = Some(Instant::now()); }

/// Asset changes per frame (each is uploaded again by the render world): meshes, standard materials, images.
fn count_assets(mut m: ResMut<MainMarks>, mut meshes: MessageReader<AssetEvent<Mesh>>, mut mats: MessageReader<AssetEvent<StandardMaterial>>,
    mut images: MessageReader<AssetEvent<Image>>) {
    fn changed<A: Asset>(e: &AssetEvent<A>) -> bool { matches!(e, AssetEvent::Modified { .. } | AssetEvent::Added { .. }) }
    m.assets[0] += meshes.read().filter(|e| changed(e)).count() as u32;
    m.assets[1] += mats.read().filter(|e| changed(e)).count() as u32;
    m.assets[2] += images.read().filter(|e| changed(e)).count() as u32;
}

fn main_last(mut m: ResMut<MainMarks>) {
    let Some(first) = m.first else { return };
    m.sum += first.elapsed().as_secs_f32() * 1000.0;
    m.frames += 1;
    let w = *m.window.get_or_insert(Instant::now());
    if w.elapsed().as_secs_f32() >= 0.5 {
        let n = m.frames as f32;
        let line = format!("main avg {:.2}ms over {} frames; asset changes per frame: meshes {:.1} materials {:.1} images {:.1}", m.sum / n, m.frames,
            m.assets[0] as f32 / n, m.assets[1] as f32 / n, m.assets[2] as f32 / n);
        m.assets = [0; 3];
        if let Some(f) = m.file.as_mut() { let _ = writeln!(f, "{line}"); }
        m.sum = 0.0;
        m.passes += 1;
        m.frames = 0;
        m.window = Some(Instant::now());
    }
}

fn mark<const K: usize>(mut m: ResMut<Marks>) {
    m.t[K] = Some(Instant::now());
    if K < 10 { return; }
    m.frame += 1;
    let (Some(first), Some(last)) = (m.t[0], m.t[10]) else { return };
    let total = (last - first).as_secs_f32() * 1000.0;
    let t = m.t;
    let w = *m.window.0.get_or_insert(Instant::now());
    m.window.1 += 1;
    for k in 0..10 { if let (Some(a), Some(b)) = (t[k], t[k + 1]) { m.window.2[k] += (b - a).as_secs_f32() * 1000.0; } }
    if w.elapsed().as_secs_f32() >= 0.5 {
        let n = m.window.1 as f32;
        let mut line = format!("render avg {:.2}ms:", m.window.2.iter().sum::<f32>() / n);
        for k in 0..10 { line += &format!(" {} {:.2}", STAGES[k], m.window.2[k] / n); }
        let _ = writeln!(m.file, "{line}");
        m.window = (Some(Instant::now()), 0, [0.0; 10]);
    }
    if total > 10.0 && m.start.elapsed().as_secs_f32() > 2.0 {
        let mut line = format!("{:.2} render frame {} {:.1}ms:", m.start.elapsed().as_secs_f32(), m.frame, total);
        for k in 0..10 {
            if let (Some(a), Some(b)) = (m.t[k], m.t[k + 1]) { line += &format!(" {} {:.1}", STAGES[k], (b - a).as_secs_f32() * 1000.0); }
        }
        let _ = writeln!(m.file, "{line}");
    }
    m.t = [None; 11];
}

pub struct RenderProfilePlugin;

impl Plugin for RenderProfilePlugin {
    fn build(&self, app: &mut App) {
        let Some(path) = std::env::var_os("AC_RENDER_PROFILE") else { return };
        let Ok(file) = std::fs::File::create(path) else { return };
        let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
        let main_file = file.try_clone().ok();
        render.insert_resource(Marks { file, t: [None; 11], start: Instant::now(), frame: 0, window: (None, 0, [0.0; 10]) });
        render.add_systems(Render, (
            mark::<0>.before(S::ExtractCommands),
            mark::<1>.after(S::ExtractCommands).before(S::PrepareAssets),
            mark::<2>.after(S::PrepareAssets).before(S::PrepareMeshes),
            mark::<3>.after(S::PrepareMeshes).before(S::CreateViews),
            mark::<4>.after(S::CreateViews).before(S::Specialize),
            mark::<5>.after(S::Specialize).before(S::PrepareViews),
            mark::<6>.after(S::PrepareViews).before(S::Queue),
            mark::<7>.after(S::Queue).before(S::PhaseSort),
            mark::<8>.after(S::PhaseSort).before(S::Prepare),
            mark::<9>.after(S::Prepare).before(S::Render),
            mark::<10>.after(S::Render).before(S::Cleanup),
            log_support,
            log_pipelines.after(S::Render).before(S::Cleanup),
        ));
        app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin).add_systems(Last, log_passes.after(main_last));
        app.insert_resource(MainMarks { file: main_file, ..default() }).add_systems(First, main_first).add_systems(Last, (count_assets, main_last).chain());
    }
}
