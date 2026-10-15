//! Debug draw of the jump targeting (F8): what the player would jump at and what a jump in the air is aiming for.
//!
//! The jump target searches (`targets::find_target`, `swing_jump_target`, `haystack_target`) report each run here
//! while the draw is on: the query's origin and direction, every candidate the query found (`jump_candidates::query`,
//! 0xE18970) and the one the scorer (0xE96BF0) chose, as the port's `JumpTarget`. The draw keeps the last search for
//! a moment, so a free run past a post shows the post lighting up before the jump. In the air it draws the target the
//! jump is flying to (InAirData +0x290 / the aim). PORT: debug only; the game has no such draw.
//!
//! Colours: candidates by the room on top (`Candidate::ty`): green a roof (≥ 1 m), yellow 0.3–1 m, orange a low lip
//! (< 0.3 m), magenta a pole, brown a beam, purple a ladder; the chosen one a white cube with a line from the feet; the
//! flight's target a cyan cube. `AC_TARGET_DEBUG=0` starts it off.

use bevy::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use super::jump_candidates::Candidate;
use super::{ActorContextId, Body, HumanDataBundle, Locomotion, Player};

/// Whether the searches report (the draw is on); set by `collect` every frame.
static RECORD: AtomicBool = AtomicBool::new(false);
/// The searches reported since the last `collect`.
static SEARCHES: Mutex<Vec<Search>> = Mutex::new(Vec::new());

/// One run of a jump target search.
#[derive(Clone, Debug, Default)]
pub struct Search {
    /// Which search ran (`"jump (query kind 0)"`, `"swing jump"`, `"leap of faith"`).
    pub kind: String,
    /// The jumper's feet and the wanted direction.
    pub from: Vec3,
    pub dir: Vec3,
    /// Every candidate the query found: position, room on top (`Candidate::ty`), jump types (`Candidate::flags`).
    pub cands: Vec<(Vec3, u32, u32)>,
    /// The chosen target: its position and type flags.
    pub chosen: Option<(Vec3, u32)>,
}

/// Report a search (no-op while the draw is off).
pub fn record(kind: impl FnOnce() -> String, from: Vec3, dir: Vec3, cands: &[Candidate], chosen: Option<&super::targets::JumpTarget>) {
    if !RECORD.load(Ordering::Relaxed) {
        return;
    }
    let s = Search { kind: kind(), from, dir, cands: cands.iter().map(|c| (c.pos, c.ty, c.flags)).collect(), chosen: chosen.map(|t| (t.position, t.type_flags)) };
    if let Ok(mut v) = SEARCHES.lock() {
        v.push(s);
    }
}

/// A jump target type's name (InAirData +0x290, the scorer's types).
pub fn type_name(t: u32) -> &'static str {
    match t {
        0x1 => "free step (roof / edge)",
        0x2 => "pass-over (vault)",
        0x40 => "wall hang",
        0x80 => "free hang (swing)",
        0x200 => "horse",
        0x400 => "kiosk / swing bar",
        0x800 => "haystack",
        0x1000 => "ladder",
        0x2000 => "pole",
        0x8000 => "air assassination",
        0x10000 => "beam / far edge",
        _ => "other",
    }
}

fn room_name(ty: u32) -> &'static str {
    match ty {
        2 => "roof",
        8 => "0.3-1 m",
        16 => "low lip",
        32 => "pole",
        4 => "beam",
        1 => "ladder",
        _ => "?",
    }
}

#[derive(Resource)]
pub struct TargetDebug {
    pub show: bool,
    /// The last search worth showing and when it ran (a search that chose a target wins over later empty ones for
    /// `HOLD` seconds).
    pub last: Option<(Search, f32)>,
}

impl Default for TargetDebug {
    fn default() -> Self {
        Self { show: std::env::var("AC_TARGET_DEBUG").as_deref() != Ok("0"), last: None }
    }
}

/// How long a search with a chosen target stays drawn (s).
const HOLD: f32 = 0.6;

impl TargetDebug {
    /// The HUD line: the flight's target in the air, else the last search.
    pub fn summary(&self, flying: Option<(u32, f32)>) -> String {
        if let Some((t, d)) = flying {
            return format!("target: jumping at {} ({t:#x}), {d:.1} m away", type_name(t));
        }
        match &self.last {
            Some((s, _)) => match s.chosen {
                Some((p, t)) => format!(
                    "target: {} -> {} ({t:#x}) {:.1} m away, {} candidates",
                    s.kind,
                    type_name(t),
                    (p - s.from).length(),
                    s.cands.len()
                ),
                None => format!("target: {} -> none ({} candidates)", s.kind, s.cands.len()),
            },
            None => "target: - (high profile + stick toward something to jump at; F8 hides)".into(),
        }
    }
}

pub struct TargetDebugPlugin;

impl Plugin for TargetDebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TargetDebug>().add_systems(PostUpdate, (collect, draw).chain());
    }
}

fn collect(keys: Res<ButtonInput<KeyCode>>, time: Res<Time>, mut debug: ResMut<TargetDebug>) {
    if keys.just_pressed(KeyCode::F8) {
        debug.show = !debug.show;
    }
    RECORD.store(debug.show, Ordering::Relaxed);
    let now = time.elapsed_secs();
    let found = SEARCHES.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default();
    if !debug.show {
        debug.last = None;
        return;
    }
    // this frame's most useful search: one that chose a target, else the last one
    let pick = found.iter().rev().find(|s| s.chosen.is_some()).or(found.last()).cloned();
    if let Some(s) = pick {
        let keep_old = s.chosen.is_none() && debug.last.as_ref().is_some_and(|(o, at)| o.chosen.is_some() && now - at < HOLD);
        if !keep_old {
            debug.last = Some((s, now));
        }
    } else if debug.last.as_ref().is_some_and(|(_, at)| now - at > HOLD) {
        debug.last = None;
    }
}

fn draw(debug: Res<TargetDebug>, q: Query<(&Locomotion, &Body, &HumanDataBundle), With<Player>>, mut gizmos: Gizmos) {
    if !debug.show {
        return;
    }
    let white = Color::srgb(1.0, 1.0, 1.0);
    if let Some((s, _)) = &debug.last {
        for &(p, ty, _) in &s.cands {
            let c = match room_name(ty) {
                "roof" => Color::srgb(0.2, 1.0, 0.3),
                "0.3-1 m" => Color::srgb(1.0, 0.9, 0.1),
                "low lip" => Color::srgb(1.0, 0.5, 0.1),
                "pole" => Color::srgb(1.0, 0.2, 1.0),
                "beam" => Color::srgb(0.75, 0.5, 0.25),
                "ladder" => Color::srgb(0.6, 0.3, 1.0),
                _ => Color::srgb(0.6, 0.6, 0.6),
            };
            gizmos.sphere(Isometry3d::from_translation(p), 0.08, c);
        }
        let dir = Vec3::new(s.dir.x, 0.0, s.dir.z).normalize_or_zero();
        gizmos.arrow(s.from + Vec3::Y * 0.1, s.from + Vec3::Y * 0.1 + dir * 1.0, Color::srgb(0.6, 0.6, 0.6));
        if let Some((p, _)) = s.chosen {
            gizmos.cube(Transform::from_translation(p + Vec3::Y * 0.15).with_scale(Vec3::splat(0.3)), white);
            gizmos.line(s.from + Vec3::Y * 0.1, p + Vec3::Y * 0.15, white);
        }
    }
    // in the air: the target the jump is flying to
    for (loco, body, data) in &q {
        if loco.current != ActorContextId::InAir {
            continue;
        }
        if let Some(t) = data.air.target {
            let cyan = Color::srgb(0.1, 0.95, 1.0);
            let p = t.hang.map_or(t.position, |(mid, _)| mid);
            gizmos.cube(Transform::from_translation(p + Vec3::Y * 0.15).with_scale(Vec3::splat(0.35)), cyan);
            gizmos.line(body.feet + Vec3::Y * 0.9, p + Vec3::Y * 0.15, cyan);
        }
    }
}

/// The flight's target for the HUD: its type and distance, while a jump with a target is in the air.
pub fn flying_target(loco: &Locomotion, body: &Body, data: &HumanDataBundle) -> Option<(u32, f32)> {
    (loco.current == ActorContextId::InAir).then_some(())?;
    let t = data.air.target?;
    let p = t.hang.map_or(t.position, |(mid, _)| mid);
    Some((t.type_flags, (p - body.feet).length()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_names_cover_the_scorer_types() {
        assert_eq!(type_name(0x2000), "pole");
        assert_eq!(type_name(0x40), "wall hang");
        assert_eq!(type_name(0x10000), "beam / far edge");
    }
}
