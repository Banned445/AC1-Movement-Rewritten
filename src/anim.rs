//! Animation playback on Altaïr's rig.
//!
//! Clips are decoded from the user's own install (RE/10): per-bone local rotation/translation keys
//! (keyed by BoneID), the root DISPLACEMENT track (root motion) and the ACUATORCONTACTS track (limb
//! contact tags). Clips are chosen through the game's **animation graph** (RE/13): the contexts name an
//! *action* by the id the exe uses; an action is a sequence of *items*, each a weighted blend of clips
//! with its own blend time and displacement mode. While a clip plays the rig root converts animation
//! space (Z-up, facing +Y, feet at 0) to Bevy space.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::assets::ac_actions::{ActBlend, ActionGraph, DisplacementMode};
use crate::assets::ac_anim::{AnimEvent, EventKind, CONTACT_HUMAN};
use crate::assets::anims::load_locomotion;
use crate::assets::game_dir;
use crate::model::Rig;
use crate::player::move_blend::ACT_GROUND_LOCOMOTION;
use crate::player::{ground::speed_band, ground::SpeedBand, ActorContextId, HumanDataBundle, Locomotion};

/// A decoded animation clip (Bevy types).
#[derive(Clone, Debug, Default)]
pub struct AnimClip {
    pub name: String,
    pub duration: f32,
    pub rotations: HashMap<u32, Vec<(f32, Quat)>>,
    pub translations: HashMap<u32, Vec<(f32, Vec3)>>,
    /// Root-motion speed (m/s) from the DISPLACEMENT track.
    pub root_speed: f32,
    /// ACUATORCONTACTS keys (bitmask held until the next key; bits = `AcuatorsContactsTypes`).
    pub contacts: Vec<(f32, u8)>,
    /// Event track keys in time order (footsteps, landings, sounds; RE/10 §2.1).
    pub events: Vec<AnimEvent>,
}

/// An event key the playing clip passed this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct FiredEvent {
    pub clip: String,
    pub time: f32,
    pub kind: EventKind,
}

/// The event keys passed when a clip's time goes from `from` to `to`: (from, to], or, when the time went backwards
/// (the loop wrapped), (from, end] and then [0, to]. `from = None`: the clip just started, [0, to].
/// PORT (hypothesis): the game's firing rule (inclusive ends, keys skipped by a jump in time) is not traced.
fn passed_events(events: &[AnimEvent], from: Option<f32>, to: f32) -> impl Iterator<Item = &AnimEvent> {
    let (a, wrap) = match from {
        Some(f) if to >= f => (f, false),
        Some(f) => (f, true),
        None => (-1.0, false),
    };
    let (end, head) = if wrap { (f32::INFINITY, to) } else { (to, -1.0) };
    events.iter().filter(move |e| e.time > a && e.time <= end).chain(events.iter().filter(move |e| e.time <= head))
}

/// A readable name for an event (the ContactEventTypeHuman name for contacts).
pub fn event_label(k: &EventKind) -> String {
    match k {
        EventKind::Contact { ty, .. } => CONTACT_HUMAN.get(*ty as usize).map_or_else(|| format!("contact {ty}"), |s| s.to_string()),
        EventKind::Audio { sound } => format!("audio {sound:08x}"),
        EventKind::Other(c) => format!("event {c:08x}"),
    }
}

fn find_span<T>(keys: &[(f32, T)], t: f32) -> (usize, usize, f32) {
    if keys.len() == 1 || t <= keys[0].0 {
        return (0, 0, 0.0);
    }
    let last = keys.len() - 1;
    if t >= keys[last].0 {
        return (last, last, 0.0);
    }
    let i = keys.partition_point(|k| k.0 <= t) - 1;
    let (t0, t1) = (keys[i].0, keys[i + 1].0);
    (i, i + 1, if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 })
}

/// Quaternion interpolation as the game does it (0x4916F0): shortest arc; nlerp when the keys are
/// close (dot ≥ 0.98936), slerp otherwise.
fn qinterp(a: Quat, b: Quat, s: f32) -> Quat {
    let b = if a.dot(b) < 0.0 { -b } else { b };
    if a.dot(b) >= 0.989_355_7 { a.lerp(b, s).normalize() } else { a.slerp(b, s) }
}

pub fn sample_rot(keys: &[(f32, Quat)], t: f32) -> Quat {
    let (a, b, s) = find_span(keys, t);
    qinterp(keys[a].1, keys[b].1, s)
}

pub fn sample_vec(keys: &[(f32, Vec3)], t: f32) -> Vec3 {
    let (a, b, s) = find_span(keys, t);
    keys[a].1.lerp(keys[b].1, s)
}

/// Contact bits at time `t` (keys are held: ByteLowest).
pub fn contact_bits(keys: &[(f32, u8)], t: f32) -> u8 {
    if keys.is_empty() {
        return 0;
    }
    let i = keys.partition_point(|k| k.0 <= t + 1e-4);
    keys[i.saturating_sub(1)].1
}

#[derive(Resource, Default)]
pub struct AnimLibrary {
    pub clips: HashMap<String, AnimClip>,
    pub graph: ActionGraph,
    /// Body-part channels and the bones of each part (from the install).
    pub body: crate::assets::body_parts::BodyParts,
    /// Animation resource id → resource (clip) name.
    pub anim_names: HashMap<u32, String>,
    pub status: String,
}

/// One playing item: clips with weights, the blend time into it, and whether the clip's root motion
/// shapes the path (`ACTDisplacementMode::FromAnim`).
#[derive(Clone, Debug, Default)]
pub struct ItemPlay {
    pub layers: Vec<(String, f32)>,
    /// The item's own blend: how it takes over from the item before it in its action (mostly NONE, a cut).
    pub blend: ActBlend,
    pub root_motion: bool,
    /// The graph action the item belongs to (0: a named clip outside the graph).
    pub action: u32,
    /// The item's outgoing transitions (the transition lookup `sub_725950` reads the playing item's).
    pub transitions: Vec<crate::assets::ac_actions::ActTransition>,
}

impl AnimLibrary {
    /// The items of a graph action with its authored default weights (layers whose clip is not loaded
    /// are dropped).
    pub fn action_items(&self, id: u32) -> Option<Vec<ItemPlay>> {
        let a = self.graph.actions.get(&id)?;
        let items: Vec<ItemPlay> = a
            .items
            .iter()
            .map(|it| {
                let mut layers = Vec::new();
                for (k, anim) in it.animations.iter().enumerate() {
                    let w = it.weights.get(k).copied().unwrap_or(if k == 0 { 1.0 } else { 0.0 });
                    if let Some(n) = self.anim_names.get(anim).filter(|n| self.clips.contains_key(*n)) {
                        layers.push((n.clone(), w));
                    }
                }
                if layers.iter().all(|l| l.1 <= 0.0) {
                    if let Some(l) = layers.first_mut() {
                        l.1 = 1.0;
                    }
                }
                ItemPlay { layers, blend: it.blend, root_motion: it.displacement == DisplacementMode::FromAnim, action: id, transitions: it.transitions.clone() }
            })
            .filter(|i| !i.layers.is_empty())
            .collect();
        (!items.is_empty()).then_some(items)
    }

    /// First action of `block` whose first item plays clip `clip` (for actions whose id is not yet
    /// traced to the exe; RE/13 lists which ones).
    pub fn action_with_clip(&self, block: &str, clip: &str) -> Option<u32> {
        self.graph
            .actions
            .values()
            .filter(|a| a.block == block)
            .find(|a| a.items.first().is_some_and(|it| it.animations.first().and_then(|id| self.anim_names.get(id)).is_some_and(|n| n == clip)))
            .map(|a| a.id)
    }

    fn item_duration(&self, item: &ItemPlay) -> f32 {
        let (mut d, mut w) = (0.0, 0.0);
        for (n, wt) in &item.layers {
            if let Some(c) = self.clips.get(n) {
                d += c.duration * wt;
                w += wt;
            }
        }
        if w > 0.0 { d / w } else { 0.0 }
    }

    fn dominant<'a>(&'a self, item: &ItemPlay) -> Option<&'a AnimClip> {
        item.layers.iter().max_by(|a, b| a.1.total_cmp(&b.1)).and_then(|(n, _)| self.clips.get(n))
    }
}

/// Contact state of the playing item (dominant clip), for the limb IK (RE/11 §2, RE/13 §5).
#[derive(Clone, Copy, Debug, Default)]
pub struct ContactState {
    /// The clip has a contact track.
    pub tagged: bool,
    pub bits: u8,
    /// Seconds into the dominant clip.
    pub t: f32,
    /// Per bit: time the bit last switched off (≤ t), if it is off.
    pub off_since: [Option<f32>; 8],
    /// Per bit: next time (> t) the bit switches on, if it is off.
    pub next_on: [Option<f32>; 8],
}

fn contact_state(c: &AnimClip, t: f32) -> ContactState {
    let mut s = ContactState { tagged: !c.contacts.is_empty(), t, ..default() };
    if !s.tagged {
        return s;
    }
    s.bits = contact_bits(&c.contacts, t);
    for b in 0..8 {
        if s.bits >> b & 1 != 0 {
            continue;
        }
        let mut off = 0.0;
        for (kt, v) in &c.contacts {
            if *kt > t + 1e-4 {
                break;
            }
            if v >> b & 1 == 0 && (off == 0.0 || contact_bits(&c.contacts, kt - 1e-3) >> b & 1 != 0) {
                off = *kt;
            }
        }
        s.off_since[b] = Some(off);
        s.next_on[b] = c.contacts.iter().find(|(kt, v)| *kt > t + 1e-4 && v >> b & 1 != 0).map(|k| k.0);
    }
    s
}

/// Playback state on the player.
#[derive(Component, Default)]
pub struct AnimPlayer {
    /// Identity of what is playing (clip name or `act_xxxxxxxx`).
    pub clip: Option<String>,
    pub items: Vec<ItemPlay>,
    pub item: usize,
    pub phase: f32,
    /// The pose on screen when the current transition started (per joint), faded out over `fade_time`.
    /// PORT: the game's blend tree keeps the outgoing item running; the port freezes the displayed pose, which
    /// also covers a transition that starts while another is still fading (no snap).
    pub prev: Option<Vec<(Quat, Vec3)>>,
    /// Root-joint translation on screen when the transition started (root-motion offset of a FromAnim clip).
    prev_root: Vec3,
    /// The last displayed pose and root translation (before IK).
    last_pose: Vec<(Quat, Vec3)>,
    last_root: Vec3,
    /// The last clip key was a locomotion cycle (keeps the phase between gaits).
    last_cyclic: bool,
    pub fade: f32,
    /// Crossfade length for the current transition (s).
    pub fade_time: f32,
    /// The running blend (`ActionBlend` of the transition or item seam): its type decides whether A (outgoing) and B
    /// (incoming) keep playing, its flags the weight curve, its modes where root motion and contacts come from.
    pub blend: ActBlend,
    /// A still playing while the blend runs (AROLLB* types): its items, item and phase.
    a_roll: Option<(Vec<ItemPlay>, usize, f32, bool)>,
    /// A's contact tags when the blend started (ACTBlendAcuatorMode FROMA).
    prev_contacts: ContactState,
    /// The action sequence loops back to this item (the items before it are a transition action).
    loop_from: usize,
    /// The simulation's phase applies from this item on (the items before it are a transition action playing on its
    /// own clock).
    sim_from: usize,
    /// Looping, or one-shot (the last item holds its last frame).
    pub looping: bool,
    /// One-shot stretched to this many seconds (e.g. a jump fitted to the jump's duration).
    pub fit: Option<f32>,
    /// Restart key: a new token restarts the action even if it is the same one.
    pub token: u64,
    /// A one-shot that must finish before normal selection resumes in this context.
    pub hold: Option<ActorContextId>,
    /// Contact tags of the current frame (written by `apply_clip`).
    pub contacts: ContactState,
    /// Event keys the dominant clip passed this frame (written by `apply_clip`; footstep / landing sounds and FX
    /// read these).
    pub events: Vec<FiredEvent>,
    /// The dominant clip and its time when events were last collected.
    event_cursor: Option<(String, f32)>,
    seen_landing: u32,
    /// InAir entry the fall was started for.
    seen_fall: Option<u32>,
    /// The fall's reception phase (0xE00EF0): the full-body item the fall left is running to its end.
    fall_entry: bool,
    /// Smoothed grasp direction while falling with grab held (HumanInAir+48, 7/s).
    grasp_dir: Vec3,
    /// Ledge grab whose reception has been played.
    caught: Option<u64>,
    /// Phase set by the simulation (the ground step cycle, `MoveBlend`): the player shows it instead of
    /// advancing its own clock.
    sim_phase: Option<f32>,
    /// A partial-body action playing over the full-body one (a channel of slot group 1).
    pub overlay: Option<Overlay>,
}

/// A partial-body slot (`Anim__PlayActionOnSlots` 0x4FE730: an action on a channel of slot group 1 plays on the
/// slots whose mask meets the channel's, over the full-body slot). PORT (hypothesis on the slot model): one overlay
/// slot with the channel's bones, faded in and out like the default transition (0.2 s); while it shows, its bones
/// follow its action instead of the full-body one.
#[derive(Clone, Debug, Default)]
pub struct Overlay {
    pub action: u32,
    items: Vec<ItemPlay>,
    item: usize,
    phase: f32,
    /// 0 → 1 over 0.2 s, back to 0 when the action ends.
    pub weight: f32,
    ending: bool,
    looping: bool,
    /// The channel's bones, and per joint whether the overlay animates it (resolved against the rig on first use).
    bones: std::collections::HashSet<u32>,
    mask: Vec<bool>,
}

/// Fade time of the overlay slot (PORT: the default transition's 0.2 s).
const OVERLAY_FADE: f32 = 0.2;

impl AnimPlayer {
    /// Update the simulation-driven item in place (its weights change every frame), behind a transition action
    /// that may still be playing.
    fn set_sim_item(&mut self, it: ItemPlay) {
        if self.items.is_empty() {
            self.items = vec![it];
            self.item = 0;
            return;
        }
        let k = self.sim_from.min(self.items.len() - 1);
        self.items[k] = it;
        self.items.truncate(k + 1);
        self.item = self.item.min(k);
    }

    /// Play `action` on the partial-body slot when its channel is in slot group 1; returns false for a full-body
    /// action (play those through the normal selection).
    pub fn play_overlay(&mut self, lib: &AnimLibrary, action: u32) -> bool {
        if self.overlay.as_ref().is_some_and(|o| o.action == action && !o.ending) {
            return true;
        }
        let Some(a) = lib.graph.actions.get(&action) else { return false };
        let Some(ch) = lib.body.channels.get(&a.channel) else { return false };
        if ch.group == 0 {
            return false;
        }
        let Some(items) = lib.action_items(action) else { return false };
        let bones = lib.body.bones(ch.mask);
        let weight = self.overlay.as_ref().map_or(0.0, |o| o.weight);
        self.overlay = Some(Overlay { action, items, item: 0, phase: 0.0, weight, ending: false, looping: a.repeat == 0, bones, mask: Vec::new() });
        true
    }

    /// Set the layer weights of the overlay's current item (a blended action such as the fall-grasp blend).
    pub fn set_overlay_weights(&mut self, w: &[f32]) {
        if let Some(it) = self.overlay.as_mut().and_then(|o| o.items.get_mut(o.item)) {
            if it.layers.len() == w.len() {
                for (l, w) in it.layers.iter_mut().zip(w) {
                    l.1 = *w;
                }
            }
        }
    }

    /// Stop the partial-body action (it fades out).
    pub fn stop_overlay(&mut self) {
        if let Some(o) = self.overlay.as_mut() {
            o.ending = true;
        }
    }
}

/// Blend the overlay's pose over the full-body pose on its bones; advances it by `dt`.
fn apply_overlay(o: &mut Overlay, lib: &AnimLibrary, rig: &Rig, pose: &mut [(Quat, Vec3)], dt: f32) {
    if o.mask.len() != rig.bone_ids.len() {
        o.mask = rig.bone_ids.iter().map(|b| o.bones.contains(b)).collect();
    }
    let Some(it) = o.items.get(o.item).cloned() else { return };
    let duration = lib.item_duration(&it).max(1e-3);
    o.phase += dt / duration;
    if o.phase >= 1.0 {
        if o.item + 1 < o.items.len() {
            o.item += 1;
            o.phase = 0.0;
        } else if o.looping {
            o.item = 0;
            o.phase = o.phase.fract();
        } else {
            o.phase = 1.0;
            o.ending = true;
        }
    }
    o.weight = if o.ending { (o.weight - dt / OVERLAY_FADE).max(0.0) } else { (o.weight + dt / OVERLAY_FADE).min(1.0) };
    let mut ov = Vec::new();
    sample_layers(lib, rig, &o.items[o.item].layers, o.phase, &mut ov);
    blend_masked(pose, &ov, &o.mask, o.weight);
}

/// `pose[i]` → `over[i]` by `w` where `mask[i]`.
pub fn blend_masked(pose: &mut [(Quat, Vec3)], over: &[(Quat, Vec3)], mask: &[bool], w: f32) {
    for (i, p) in pose.iter_mut().enumerate() {
        if let (Some(true), Some(&(r, t))) = (mask.get(i).copied(), over.get(i)) {
            *p = (qinterp(p.0, r, w), p.1.lerp(t, w));
        }
    }
}

/// What the selector wants playing.
struct Request {
    key: String,
    items: Vec<ItemPlay>,
    /// The simulation drives the phase (no transition action may be put in front).
    sim: bool,
    looping: bool,
    fit: Option<f32>,
    token: u64,
    fade: f32,
    hold: bool,
}

const CROSSFADE: f32 = 0.2;

pub struct AnimPlugin;

impl Plugin for AnimPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnimLibrary>()
            .add_systems(Startup, load_library)
            .add_systems(Update, (choose_clip, apply_clip).chain().after(crate::player::PlayerSet));
    }
}

/// Named one-shots whose DISPLACEMENT track shapes the body's path (the graph's FromAnim mode is used for
/// actions).
pub fn uses_root_motion(name: &str) -> bool {
    name.starts_with("shimmy_") || name.starts_with("pullup_") || name.starts_with("catch_")
}

fn single(name: &str) -> Vec<ItemPlay> {
    vec![ItemPlay { layers: vec![(name.to_string(), 1.0)], blend: named_blend(CROSSFADE), root_motion: uses_root_motion(name), action: 0, transitions: Vec::new() }]
}

fn looped(clip: &str, fade: f32) -> Request {
    Request { key: clip.into(), items: single(clip), sim: false, looping: true, fit: None, token: 0, fade, hold: false }
}

fn once(clip: String, token: u64, fit: Option<f32>, fade: f32) -> Request {
    Request { items: single(&clip), key: clip, sim: false, looping: false, fit, token, fade, hold: false }
}

/// A graph action (by the id the exe uses). Falls back to `fallback` (a clip name) if the action or its
/// clips are missing. The fade is the first item's authored blend time unless `fade` is given.
fn action(lib: &AnimLibrary, ids: &[u32], looping: bool, token: u64, fade: Option<f32>, fallback: Option<&str>) -> Option<Request> {
    let mut items = Vec::new();
    for id in ids {
        items.extend(lib.action_items(*id)?);
    }
    if items.is_empty() {
        return fallback.map(|f| Request { looping, token, ..once(f.to_string(), token, None, fade.unwrap_or(CROSSFADE)) });
    }
    let fade = fade.unwrap_or(items[0].blend.time.max(0.05));
    let key = ids.iter().map(|i| format!("act_{i:08x}")).collect::<Vec<_>>().join("+");
    Some(Request { key, items, sim: false, looping, fit: None, token, fade, hold: false })
}

/// PORT: a named clip (outside the graph) enters as the default transition does (A frozen, B playing, linear)
/// over `fade`.
fn named_blend(fade: f32) -> ActBlend {
    ActBlend { time: fade, ..ActBlend::DEFAULT }
}

/// The game's transition into `new` from the playing item (`sub_725950`, RE/13 §7.1), in this order:
/// 1. the playing item's transitions whose destination is `new` (`sub_5B98B0`);
/// 2. the playing action's out-transition (Action +20);
/// 3. `new`'s in-transition (Action +16) (the exe first checks the body-part channel priorities; the port's actions
///    share one channel);
/// 4. the default transition (ASTOPBROLL 0.2 s, `sub_46A460`), unless `new` has flag bit 2 (hypothesis: the exe's
///    last test, two node lookups compared, is taken as true).
/// `None` means a cut.
pub fn game_transition(lib: &AnimLibrary, cur: Option<&ItemPlay>, new: u32) -> Option<crate::assets::ac_actions::ActTransition> {
    use crate::assets::ac_actions::ActTransition;
    let new_action = lib.graph.actions.get(&new)?;
    if let Some(item) = cur {
        if let Some(t) = item.transitions.iter().find(|t| t.action_b == new) {
            return Some(*t);
        }
        if let Some(t) = lib.graph.actions.get(&item.action).and_then(|a| a.out_transition) {
            return Some(t);
        }
    }
    if let Some(t) = new_action.in_transition {
        return Some(t);
    }
    (new_action.flags & 4 == 0).then_some(ActTransition { blend_a: ActBlend::DEFAULT, ..Default::default() })
}

/// The blend weight at linear progress `t` (`sub_773C30`): linear, or eased in (2t² − t³), out (t + t² − t³) or
/// both (3t² − 2t³).
pub fn blend_weight(b: &ActBlend, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match (b.ease_in, b.ease_out) {
        (true, true) => 3.0 * t * t - 2.0 * t * t * t,
        (true, false) => 2.0 * t * t - t * t * t,
        (false, true) => t + t * t - t * t * t,
        (false, false) => t,
    }
}

/// Fall-grasp weights (HumanInAir__CheckAirCatch 0xE0BF2B–0xE0C2E9): with grab held the reach direction
/// (stick, else facing) is smoothed at 7/s; `w` = its length (≤ 1); the static fall pose gets 1 − w and
/// the rest goes to front / left / right / back-left / back-right by the signed angle to the facing, in
/// 90° sectors. Clip order in action 0x1F0C22C2: falling01, front, left, right, backleft, backright.
pub fn fall_grasp_weights(smoothed: Vec3, facing: Vec3) -> [f32; 6] {
    let mut w6 = [0.0; 6];
    let flat = Vec3::new(smoothed.x, 0.0, smoothed.z);
    let w = flat.length().clamp(0.0, 1.0);
    w6[0] = 1.0 - w;
    if w < 5e-4 {
        return w6;
    }
    let d = flat.normalize();
    // signed angle from facing; positive = to the right (game: left/right from sub_55E570)
    let right = crate::player::right_of(facing);
    let a = d.dot(right).atan2(d.dot(facing));
    let q = std::f32::consts::FRAC_PI_2;
    if a >= 0.0 {
        if a <= q {
            w6[1] = (1.0 - a / q) * w;
            w6[3] = a / q * w;
        } else {
            let b = (a - q) / q;
            w6[3] = (1.0 - b) * w;
            w6[5] = b * w;
        }
    } else {
        let a = -a;
        if a <= q {
            w6[1] = (1.0 - a / q) * w;
            w6[2] = a / q * w;
        } else {
            let b = (a - q) / q;
            w6[2] = (1.0 - b) * w;
            w6[4] = b * w;
        }
    }
    w6
}

// Action ids used by the exe (RE/13 §4). Ledge tables 0x1A2C490.. (shimmy), 0x1A2C4C0.. (vertical steps).
const ACT_HANG_WALL: u32 = 0x0106_F2E8;
const ACT_HANG_FREE: u32 = 0x0127_19F1;
const ACT_HANG_WALLFREE: u32 = 0x0106_F2E9;
const ACT_PULLUP_WALL: u32 = 0x0106_D2C5;
const ACT_PULLUP_FREE: u32 = 0x0127_19F2;
/// Pull-up outcome "stand": hangknee → free-step entry (0xDE2EE0 → 0xDD2A50).
const ACT_HANGKNEE_TO_WAIT: u32 = crate::player::ledge_moves::ACT_KNEE_TO_FREESTEP[0];
/// [wall, free] × [left open, left close, right open, right close].
const ACT_SHIMMY: [[u32; 4]; 2] = [[0x01B7_0B35, 0x01B7_0B36, 0x01B7_0B37, 0x01B7_0B38], [0x01A2_490A, 0x01A2_490B, 0x01A2_490C, 0x01A2_490D]];
/// [wall, free] × [up: 1m_u_1lu, 1m_u_1ru, 1lu_u_1m, 1ru_u_1m, down: 1m_d_1lu, 1m_d_1ru, 1lu_d_1m, 1ru_d_1m].
const ACT_VSTEP: [[u32; 8]; 2] = [
    [0x01B7_0FAA, 0x01B7_0FAB, 0x01B7_0FAC, 0x01B7_0FAD, 0x01B7_0FAE, 0x01B7_0FAF, 0x01B7_0FB0, 0x01B7_0FB1],
    [0x01A2_79B9, 0x01A2_79BA, 0x01A2_79BB, 0x01A2_79BC, 0x01A2_79BD, 0x01A2_79BE, 0x01A2_79BF, 0x01A2_79C0],
];
/// Catches (CheckAirCatch): ledge with wall below (3-way angle blend), free hang; [< 3 m, ≥ 3 m].
const ACT_CATCH_WALL: [u32; 2] = [0x1F0C_0C23, 0x1F0C_0C2D];
const ACT_CATCH_FREE: [u32; 2] = [0x1F0C_2EB8, 0x1F0C_2EB9];
/// Falling (6-way grasp blend).
const ACT_FALL: u32 = 0x1F0C_22C2;
/// PORT (stand-in until the ground state tree with MoveBlend's transition path, RE/12 §1): the game also plays an
/// authored transition action in front of the ground locomotion, and MoveBlend's start / transition blend layouts
/// (HG+0x724, 0xDA08C0) keep speed and foot phase in step with it. The port has no such path yet, so these
/// transitions are left out and the switch uses the default transition. Set to `true` once that path is ported;
/// nothing else depends on this switch.
const GROUND_LOCOMOTION_TRANSITIONS: bool = false;

/// The ground waits (HumanGround), [low, high profile] × [left, right foot ahead]: `xx_{l,h}_wait_hipm_foot{l,r}`.
const ACT_WAIT: [[u32; 2]; 2] = [[0x00D8_243F, 0x00D8_24C5], [0x00D8_2508, 0x00D8_258E]];

/// One item of a graph action with the sim's weights, if all its clips are loaded. Root motion is off:
/// the sim already moves the body along it.
fn sim_item(lib: &AnimLibrary, b: &crate::player::jump_blend::ActionBlend) -> Option<ItemPlay> {
    let items = lib.action_items(b.id)?;
    let mut it = items.get(b.item)?.clone();
    if it.layers.len() != b.n {
        return None;
    }
    for (k, l) in it.layers.iter_mut().enumerate() {
        l.1 = b.w[k];
    }
    it.root_motion = false;
    Some(it)
}

/// Play `b` (phase from the sim): update the weights in place if it is already playing.
fn sim_request(p: &mut AnimPlayer, lib: &AnimLibrary, b: &crate::player::jump_blend::ActionBlend, token: u64, fade: f32) -> Option<Request> {
    let it = sim_item(lib, b)?;
    let key = format!("act_{:08x}", b.id);
    if p.clip.as_deref() == Some(key.as_str()) && p.token == token {
        p.set_sim_item(it);
        return None;
    }
    Some(Request { key, items: vec![it], sim: true, looping: false, fit: None, token, fade, hold: false })
}

/// Both items (footl, footr) of the ground locomotion action with all 17 clips loaded.
fn ground_items(lib: &AnimLibrary) -> Option<Vec<ItemPlay>> {
    lib.action_items(ACT_GROUND_LOCOMOTION).filter(|items| items.len() == 2 && items.iter().all(|i| i.layers.len() == 17))
}

/// Pick what plays for the current context.
fn choose_clip(
    time: Res<Time>,
    lib: Res<AnimLibrary>,
    pad: Res<crate::input::PadInput>,
    collision: Res<crate::collision::CollisionWorld>,
    mut q: Query<(&Locomotion, &HumanDataBundle, &crate::player::Body, &mut AnimPlayer)>,
) {
    use crate::player::air::AirMode;
    use crate::player::ledge::{LedgeHangType, LedgeSubState};
    let dt = time.delta_secs();
    for (loco, data, body, mut p) in &mut q {
        let g = &data.ground;
        p.sim_phase = None;
        // the falling blend belongs to InAir: leaving it (a landing, a catch) fades the partial slot out
        if loco.current != ActorContextId::InAir && p.overlay.as_ref().is_some_and(|o| o.action == ACT_FALL) {
            p.stop_overlay();
        }
        if let Some(ctx) = p.hold {
            if ctx == loco.current && !(p.phase >= 1.0 && p.item + 1 >= p.items.len()) {
                continue;
            }
            p.hold = None;
        }
        let req = match loco.current {
            // landing / free-step reception action (0xE05940 / 0xE07D00), shown at the sim's phase
            ActorContextId::Ground if g.oneshot.is_some_and(|os| sim_item(&lib, &os.blend).is_some()) => {
                let os = g.oneshot.unwrap();
                p.seen_landing = g.landing_seq;
                p.sim_phase = Some((os.t / os.duration.max(1e-4)).min(1.0));
                sim_request(&mut p, &lib, &os.blend, 1_000_000 + g.landing_seq as u64, 0.1)
            }
            // obstacle collision / lean (0xD9CB90 / 0xD9DA20): its action at the sim's phase
            ActorContextId::Ground if g.collide.is_some_and(|c| sim_item(&lib, &c.action).is_some()) => {
                let c = g.collide.unwrap();
                let (b, ph) = c.current();
                p.sim_phase = Some(ph);
                sim_request(&mut p, &lib, &b, 1_500_000 + c.seq as u64, 0.1)
            }
            // look-down at an edge (0xD9FC80)
            ActorContextId::Ground if g.look_down.is_some_and(|l| sim_item(&lib, &l.action).is_some()) => {
                let l = g.look_down.unwrap();
                p.sim_phase = Some((l.t / l.action.duration().max(1e-4)).fract());
                sim_request(&mut p, &lib, &l.action, 1_600_000 + g.pose_seq as u64, 0.3)
            }
            // (a landing without its action, e.g. the clips are missing: nothing extra plays; the one-shot branch above
            // shows the game's landing action)
            ActorContextId::Ground if g.landing_seq != p.seen_landing => {
                p.seen_landing = g.landing_seq;
                continue;
            }
            // after a pull-up the action ends standing: let it finish before idling
            ActorContextId::Ground if p.clip.as_deref().is_some_and(|c| c.starts_with(&format!("act_{ACT_PULLUP_WALL:08x}")) || c.starts_with(&format!("act_{ACT_PULLUP_FREE:08x}"))) && !(p.phase >= 1.0 && p.item + 1 >= p.items.len()) => continue,
            // moving: the game's locomotion action 0x05923BDB, item of the leading foot, MoveBlend's 17 weights
            ActorContextId::Ground if g.speed_param > 0.0 && ground_items(&lib).is_some() => {
                let mut it = ground_items(&lib).unwrap()[g.blend.foot].clone();
                for (k, l) in it.layers.iter_mut().enumerate() {
                    l.1 = g.blend.weights[k];
                }
                p.sim_phase = Some(g.blend.phase);
                let key = format!("act_{ACT_GROUND_LOCOMOTION:08x}");
                if p.clip.as_deref() == Some(key.as_str()) {
                    p.set_sim_item(it);
                    continue;
                }
                Some(Request { key, items: vec![it], sim: true, looping: true, fit: None, token: 0, fade: CROSSFADE, hold: false })
            }
            // standing: the game's wait action of the leading foot (MoveBlend foot: 0 = left ahead), entered through the
            // graph's transition like every action (RE/13 §7)
            ActorContextId::Ground if speed_band(g.speed_param) == SpeedBand::None => {
                let id = ACT_WAIT[g.high_profile as usize][(g.blend.foot != 0) as usize];
                let name = match (g.high_profile, g.blend.foot == 0) {
                    (true, true) => "idle_high",
                    (true, false) => "idle_high_r",
                    (false, true) => "idle_low",
                    (false, false) => "idle_low_r",
                };
                action(&lib, &[id], true, 0, None, Some(name))
            }
            // moving without the locomotion action's clips loaded (fallback): the named cycles
            ActorContextId::Ground => Some(looped(
                match speed_band(g.speed_param) {
                    SpeedBand::Walk => "walk",
                    SpeedBand::Jog => "jog",
                    SpeedBand::Run => "run",
                    _ => "sprint",
                },
                CROSSFADE,
            )),
            ActorContextId::InAir => match data.air.mode {
                // the game's takeoff then flight item (0xB20200), at the sim's time
                AirMode::Jump { real: true, t, t_takeoff, duration, .. } if data.air.flight.is_some_and(|b| sim_item(&lib, &b).is_some()) => {
                    let (b, ph) = if t < t_takeoff && data.air.takeoff.is_some() {
                        (data.air.takeoff.unwrap(), t / t_takeoff.max(1e-4))
                    } else {
                        (data.air.flight.unwrap(), (t - t_takeoff) / (duration - t_takeoff).max(1e-4))
                    };
                    p.sim_phase = Some(ph.min(1.0));
                    sim_request(&mut p, &lib, &b, 2_000_000 + data.air.seq as u64, 0.05)
                }
                // a jump whose actions are not loaded: the pose carries on
                AirMode::Jump { .. } => continue,
                // Leap of Faith free-fall tail: `faith_jump_fall` (0xB1EC40 third action)
                AirMode::Fall { .. } if data.air.flight.is_some_and(|f| f.id == crate::player::jump_blend::FLIGHT_FAITH) => {
                    let b = crate::player::jump_blend::ActionBlend::new(crate::player::jump_blend::FALL_FAITH, 0, &[1.0]);
                    p.sim_phase = Some(0.5);
                    sim_request(&mut p, &lib, &b, 2_500_000 + data.air.seq as u64, 0.2)
                }
                // the jump's own fall action (InAir +416, e.g. `beam_jumpstraight_clear_tr_fall`)
                AirMode::Fall { .. } if data.air.fall_action.is_some_and(|b| sim_item(&lib, &b).is_some()) => {
                    let b = data.air.fall_action.unwrap();
                    p.sim_phase = Some((data.air.fall_t / b.duration().max(1e-4)).min(1.0));
                    sim_request(&mut p, &lib, &b, 2_600_000 + data.air.seq as u64, 0.1)
                }
                // ground loss: the drop sub-state's entry, then the hurt-fall loop, for the drop phase (0xE064C0)
                AirMode::Fall { .. }
                    if data.air.drop.is_some_and(|(ty, _, _)| {
                        let entry = crate::player::air::DROP_ENTRY[ty][0];
                        entry != 0 && lib.action_items(entry).is_some() && data.air.fall_t < crate::player::air::drop_phase(ty).unwrap_or_else(|| lib.action_items(entry).map(|it| it.iter().map(|i| lib.item_duration(i)).sum::<f32>()).unwrap_or(0.5))
                    }) =>
                {
                    let (ty, side, _) = data.air.drop.unwrap();
                    p.seen_fall = Some(data.air.seq);
                    action(&lib, &[crate::player::air::DROP_ENTRY[ty][side], crate::player::air::DROP_LOOP[side]], false, 4_200_000 + data.air.seq as u64, Some(0.2), None)
                }
                _ => {
                    if p.seen_fall != Some(data.air.seq) {
                        p.seen_fall = Some(data.air.seq);
                        p.grasp_dir = Vec3::ZERO;
                        // InAir sub-state 3 (`HumanInAir__EnterReceptionState` 0xE00EF0), entered by walking / running off
                        // an edge (0xD8ADB0) and by letting go of a hang (0xDD08F0), both TransitionSetupDataToInAir
                        // +132 = 3: no entry clip. The playing item is left through its own exit (sub_5045F0 → 0x726F40;
                        // the locomotion and hang waits author none) and the sub-state lasts its remaining length
                        // (sub_502570); then the main fall plays the falling blend. The port lets the full-body item
                        // run to its end and hold its last frame.
                        p.fall_entry = p.clip.is_some() && !p.items.is_empty();
                        if p.fall_entry {
                            p.looping = false;
                            continue;
                        }
                        None
                    } else if p.fall_entry && p.overlay.is_none() && !(p.phase >= 1.0 && p.item + 1 >= p.items.len()) {
                        continue;
                    } else if p.fall_entry && p.play_overlay(&lib, ACT_FALL) {
                        // the falling blend is an upper-body action (channel 1, BodyPartTemplate_Human): it plays on
                        // the partial slot while the full-body slot holds the entry's last frame; grasp weights only
                        // while the grab input (Legs) is held
                        let facing = body.forward();
                        let want = if pad.legs_held { if pad.speed01 > 0.0 { pad.dir } else { facing } } else { Vec3::ZERO };
                        let k = (dt * 7.0).min(1.0);
                        let gd = p.grasp_dir;
                        p.grasp_dir = gd + (want - gd) * k;
                        let w = fall_grasp_weights(p.grasp_dir, facing);
                        p.set_overlay_weights(&w);
                        continue;
                    } else {
                        // no body-part data (or no entry clip): the falling blend on the full body
                        let facing = body.forward();
                        let want = if pad.legs_held { if pad.speed01 > 0.0 { pad.dir } else { facing } } else { Vec3::ZERO };
                        let k = (dt * 7.0).min(1.0);
                        let gd = p.grasp_dir;
                        p.grasp_dir = gd + (want - gd) * k;
                        let w = fall_grasp_weights(p.grasp_dir, facing);
                        let r = action(&lib, &[ACT_FALL], true, 0, Some(0.15), Some("fall"));
                        if let Some(mut r) = r {
                            if let Some(it) = r.items.first_mut() {
                                if it.layers.len() == 6 {
                                    for (k, l) in it.layers.iter_mut().enumerate() {
                                        l.1 = w[k];
                                    }
                                }
                            }
                            // same action: just update the weights
                            if p.clip.as_deref() == Some(r.key.as_str()) {
                                p.items = r.items;
                                continue;
                            }
                            Some(r)
                        } else {
                            None
                        }
                    }
                }
            },
            // beam (HumanNarrowObjectBeam): the state's action at the sim's phase
            ActorContextId::NarrowObject if data.narrow.current().is_some_and(|(b, _)| sim_item(&lib, &b).is_some()) => {
                let (b, ph) = data.narrow.current().unwrap();
                p.sim_phase = Some(ph);
                sim_request(&mut p, &lib, &b, 9_000_000 + data.narrow.seq as u64, 0.12)
            }
            // wall run (0xE37590): the sub-state's action at the sim's phase
            ActorContextId::Walling if data.walling.current().is_some_and(|(b, _)| sim_item(&lib, &b).is_some()) => {
                let (b, ph) = data.walling.current().unwrap();
                p.sim_phase = Some(ph);
                sim_request(&mut p, &lib, &b, 8_000_000 + data.walling.seq as u64 * 2 + (b.id == crate::player::walling::VERTICAL_END && b.item == 1) as u64, 0.08)
            }
            // haystack (0xE43140): entry action, then the wait, at the sim's time
            ActorContextId::HayStack if data.hay.action.is_some_and(|b| sim_item(&lib, &b).is_some()) => {
                let b = data.hay.action.unwrap();
                let ph = data.hay.t / b.duration().max(1e-4);
                p.sim_phase = Some(if data.hay.phase == crate::player::hay::HayPhase::Waiting { ph.fract() } else { ph.min(1.0) });
                sim_request(&mut p, &lib, &b, 5_000_000 + data.hay.seq as u64, 0.2)
            }
            // corner turn / ledge jump / hop up (`ledge_moves`): its current action at the sim's phase
            // ladder (0xE27D30): the table's action for the state, at the sim's phase
            ActorContextId::Ladder if data.ladder.current().is_some_and(|(b, _)| sim_item(&lib, &b).is_some()) => {
                let (b, ph) = data.ladder.current().unwrap();
                p.sim_phase = Some(ph);
                sim_request(&mut p, &lib, &b, 7_700_000 + data.ladder.seq as u64, 0.1)
            }
            // swinging on a bar (0xDD24F0): landing, swing cycle, stops / impacts
            ActorContextId::Ledge if data.ledge.swing.is_some_and(|s| sim_item(&lib, &s.action).is_some()) => {
                let s = data.ledge.swing.unwrap();
                let (b, ph) = s.current();
                p.sim_phase = Some(ph);
                sim_request(&mut p, &lib, &b, 7_600_000 + s.seq as u64, 0.08)
            }
            // pass-over (0xE07D00 case 2 / 0xDDB800): the reception, then the vault, at the sim's phase
            ActorContextId::Ledge if data.ledge.pass_over.is_some_and(|p| sim_item(&lib, &p.action).is_some()) => {
                let po = data.ledge.pass_over.unwrap();
                p.sim_phase = Some((po.t / po.action.duration().max(1e-4)).min(1.0));
                sim_request(&mut p, &lib, &po.action, 7_500_000 + po.seq as u64, 0.06)
            }
            ActorContextId::Ledge if data.ledge.mv.and_then(|m| m.current()).is_some_and(|(b, _)| sim_item(&lib, &b).is_some()) => {
                let (b, ph) = data.ledge.mv.unwrap().current().unwrap();
                p.sim_phase = Some(ph);
                sim_request(&mut p, &lib, &b, 7_000_000 + data.ledge.step_seq as u64 * 4 + b.item as u64, 0.08)
            }
            ActorContextId::Ledge => {
                let l = &data.ledge;
                let wall = l.hang_type == LedgeHangType::Wall;
                let wi = if wall { 0 } else { 1 };
                let moving = matches!(l.sub_state, LedgeSubState::HandPlacement | LedgeSubState::Pullup);
                let token = 3_000_000 + l.step_seq as u64;
                match l.last_action {
                    // caught from the air (CheckAirCatch): the game's reception action
                    // (one reception per grab: once it has played the hang idle follows)
                    "grab" if loco.previous == ActorContextId::InAir && l.moving() && p.caught != Some(token) => {
                        p.caught = Some(token);
                        let hi = data.air.long_catch as usize;
                        let mut r = action(&lib, &[if wall { ACT_CATCH_WALL[hi] } else { ACT_CATCH_FREE[hi] }], false, token, None, Some(if wall { "catch_wall" } else { "catch_free" }));
                        // CheckAirCatch snaps the 3-way wall catch (straight / 30° out / 45° in) to the dominant angle
                        // class; the port's ledges are straight
                        if let Some(r) = r.as_mut().filter(|_| wall) {
                            for it in &mut r.items {
                                for (k, l) in it.layers.iter_mut().enumerate() {
                                    l.1 = if k == 0 { 1.0 } else { 0.0 };
                                }
                            }
                        }
                        r.map(|r| Request { hold: true, ..r })
                    }
                    a if moving && a.starts_with("shimmy ") => {
                        // alt flag set after the step = the lead hand reached out (open)
                        let k = match (a.ends_with("left"), l.alt_flag) {
                            (true, true) => 0,
                            (true, false) => 1,
                            (false, true) => 2,
                            (false, false) => 3,
                        };
                        action(&lib, &[ACT_SHIMMY[wi][k]], false, token, Some(0.08), None)
                    }
                    "hand step up" | "hand step down" if moving => {
                        let (first_left, second) = l.vstep.unwrap_or((true, false));
                        let down = l.last_action == "hand step down";
                        let k = (down as usize) * 4 + (second as usize) * 2 + (!first_left) as usize;
                        action(&lib, &[ACT_VSTEP[wi][k]], false, token, Some(0.1), None)
                    }
                    "pull-up" if moving => {
                        let ids: Vec<u32> = if wall {
                            vec![ACT_PULLUP_WALL, ACT_HANGKNEE_TO_WAIT]
                        } else {
                            let waist = lib.action_with_clip("HumanLedge", "xx_h_hangwaist_tr_hangknee_footl");
                            [Some(ACT_PULLUP_FREE), waist, Some(ACT_HANGKNEE_TO_WAIT)].into_iter().flatten().collect()
                        };
                        action(&lib, &ids, false, token, Some(0.1), None)
                    }
                    // free hang with a wall under it = LedgeHangType WallFree: `xx_h_hangwallfree_wait` (0x0106F2E9,
                    // played after the straight jump's wall-free reception, 0xE07D00 → 0xE09055; the wait update
                    // 0xDE1FE0 plays it for type 2), legs held clear of the wall
                    _ if !wall && crate::player::ledge::wall_below_hands((l.hand_l + l.hand_r) * 0.5, l.normal, &collision) => {
                        action(&lib, &[ACT_HANG_WALLFREE], true, 0, Some(0.15), Some("hang_free"))
                    }
                    _ => action(&lib, &[if wall { ACT_HANG_WALL } else { ACT_HANG_FREE }], true, 0, Some(0.15), Some(if wall { "hang_wall" } else { "hang_free" })),
                }
            }
            ActorContextId::Climb => {
                let c = &data.climb;
                match (c.moving, c.move_action) {
                    // a grid move plays the action of its SHORT/LONG table entry
                    (Some(_), Some(id)) => action(&lib, &[id], false, 6_000_000 + c.move_seq as u64, Some(0.1), None),
                    // a reach's end action, played when the root arrives (0xDE9B50)
                    (None, _) if c.settle.is_some() => {
                        let (id, _) = c.settle.unwrap();
                        action(&lib, &[id], false, 6_000_000 + c.move_seq as u64, Some(0.1), None)
                    }
                    // no move for the stick: look around toward it (0xDF4410)
                    (None, _) if c.look.is_some() => {
                        let id = c.look.unwrap();
                        action(&lib, &[id], false, 6_500_000 + (id & 0xFFFF) as u64, Some(0.2), None)
                    }
                    // waiting (or entering): the pose's wait action (pose table animStateId)
                    _ => action(&lib, &[crate::player::climb::POSE_ACTIONS[c.pose]], true, 0, Some(crate::tuning::CLIMB_MOVE_TIME), None),
                }
            }
            _ => Some(looped("idle_high", CROSSFADE)),
        };
        let Some(req) = req else { continue };
        if req.items.iter().any(|it| it.layers.iter().any(|(n, _)| !lib.clips.contains_key(n))) {
            continue;
        }
        let same = p.clip.as_deref() == Some(req.key.as_str()) && p.token == req.token;
        if same {
            continue;
        }
        // the game's transition into the requested action (sub_725950); named clips keep the port's fade
        let mut items = req.items;
        let new_action = items.first().map(|i| i.action).unwrap_or(0);
        let cur_item = p.items.get(p.item.min(p.items.len().max(1) - 1)).filter(|_| p.clip.is_some()).cloned();
        let mut loop_from = 0;
        let mut sim_from = 0;
        let blend = if new_action != 0 {
            match game_transition(&lib, cur_item.as_ref(), new_action) {
                Some(t) => {
                    // a transition action plays first, blended in with blend A; the requested action follows it with
                    // blend B (the slot queues both, SetAction 0x727F70). Also in front of a standing loop the simulation
                    // times (the beam / ladder / pilotis waits, Action +28 = 0): the transition plays on its own clock
                    // without root motion, then the simulation's phase takes over.
                    // Not in front of the ground locomotion: while a transition plays the game's MoveBlend runs its own
                    // path (the start / transition blend layouts, HG+0x724, 0xDA08C0, not ported) that keeps the speed
                    // and the foot phase in step with it; without it a walk transition ran at run speed and the cycle
                    // then jumped to another foot. Nor in front of the simulation's one-shot moves, whose timing the body
                    // follows.
                    let standing_loop = req.sim && !req.looping && lib.graph.actions.get(&new_action).is_some_and(|a| a.repeat == 0);
                    let ground_locomotion = req.sim && req.looping && GROUND_LOCOMOTION_TRANSITIONS;
                    let queued = !req.sim || standing_loop || ground_locomotion;
                    let skipped = t.action_a != 0 && !queued;
                    if t.action_a != 0 && queued {
                        if let Some(mut ti) = lib.action_items(t.action_a) {
                            if req.sim {
                                for it in &mut ti {
                                    it.root_motion = false;
                                }
                                sim_from = ti.len();
                            }
                            items[0].blend = t.blend_b;
                            loop_from = ti.len();
                            ti.extend(items);
                            items = ti;
                        }
                    }
                    if skipped {
                        // PORT: the transition action is skipped, so its blend A (often a cut into that clip) would cut
                        // into the destination: the default transition instead (ASTOPBROLL 0.2 s)
                        ActBlend::DEFAULT
                    } else {
                        t.blend_a
                    }
                }
                None => ActBlend::default(),
            }
        } else {
            named_blend(req.fade)
        };
        let a_state = (std::mem::take(&mut p.items), p.item, p.phase, p.looping);
        let had = p.clip.take().is_some() && !p.last_pose.is_empty();
        start_blend(&mut p, blend, had, Some(a_state), &lib);
        // locomotion cycles all start on the left foot: keep the phase between gaits
        let cyclic = |n: &str| matches!(n, "walk" | "jog" | "run" | "sprint");
        let prev_cyclic = p.last_cyclic;
        p.last_cyclic = cyclic(&req.key);
        if !(cyclic(&req.key) && prev_cyclic) {
            p.phase = b_start_phase(&p, &lib, &items);
        }
        if std::env::var_os("AC_ANIM_LOG").is_some() {
            let layers: Vec<String> = items.iter().map(|i| i.layers.iter().map(|(n, w)| format!("{n}*{w:.2}")).collect::<Vec<_>>().join("+")).collect();
            info!("anim t={:.2} {:?} -> {} [{}] blend {} {:.2}s", time.elapsed_secs(), loco.current, req.key, layers.join(" | "), p.blend.kind, if p.prev.is_some() { p.fade_time } else { 0.0 });
        }
        p.clip = Some(req.key);
        p.items = items;
        p.item = 0;
        p.loop_from = loop_from;
        p.sim_from = sim_from;
        p.looping = req.looping;
        p.fit = req.fit;
        p.token = req.token;
        p.hold = req.hold.then_some(loco.current);
    }
}

/// Start blend `b` from what is on screen (A) into what follows (B) (`sub_773F90`). NONE, or nothing shown before,
/// is a cut. A keeps playing for the AROLLB* types and is frozen otherwise; the length is the blend time, at most
/// what is left of A with the clamp flag.
fn start_blend(p: &mut AnimPlayer, b: ActBlend, had: bool, a_state: Option<(Vec<ItemPlay>, usize, f32, bool)>, lib: &AnimLibrary) {
    p.blend = b;
    p.a_roll = None;
    if b.kind == 0 || b.time <= 0.0 || !had {
        p.prev = None;
        p.fade = 1.0;
        p.fade_time = 0.01;
        return;
    }
    p.prev = Some(p.last_pose.clone());
    p.prev_root = p.last_root;
    p.prev_contacts = p.contacts;
    p.fade = 0.0;
    let mut time = b.time;
    if let Some((items, item, phase, _)) = a_state.as_ref() {
        if b.clamp {
            if let Some(it) = items.get(*item) {
                time = time.min((1.0 - phase) * lib.item_duration(it));
            }
        }
    }
    p.fade_time = time.max(0.01);
    if matches!(b.kind, 1 | 3) {
        p.a_roll = a_state.filter(|(items, _, _, _)| !items.is_empty());
    }
}

/// B's start phase (`ACTBlendBPosMode`, 0x773F90): FROMSTART 0, PROGRESSIVE A's phase, INVPROGRESSIVE 1 − A's
/// phase, FROMTARGETTIME the blend's target time.
fn b_start_phase(p: &AnimPlayer, lib: &AnimLibrary, items: &[ItemPlay]) -> f32 {
    let a_phase = p.a_roll.as_ref().map(|a| a.2).unwrap_or(p.phase);
    match p.blend.b_pos {
        1 => a_phase,
        2 => 1.0 - a_phase,
        3 => items.first().map(|it| (p.blend.target_time / lib.item_duration(it).max(1e-3)).clamp(0.0, 1.0)).unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Animation space (Z-up, facing +Y, feet at origin) → player-local Bevy space (Y-up, facing -Z).
fn anim_root() -> Transform {
    let m = Mat3::from_cols(Vec3::X, Vec3::NEG_Z, Vec3::Y);
    Transform::from_rotation(Quat::from_mat3(&m))
}

/// Visual root offset of a root-motion clip at phase `phase` (animation space): the clip's displacement
/// minus the linear path the logical root follows.
pub fn root_motion_offset(clip: &AnimClip, phase: f32) -> Vec3 {
    let Some(keys) = clip.translations.get(&crate::assets::ac_anim::TRACK_DISPLACEMENT) else { return Vec3::ZERO };
    let start = keys.first().map(|k| k.1).unwrap_or(Vec3::ZERO);
    let end = sample_vec(keys, clip.duration) - start;
    (sample_vec(keys, phase * clip.duration) - start) - end * phase
}

/// Weighted pose of a layer set at normalised phase `phase`: per bone (rotation, translation).
fn sample_layers(lib: &AnimLibrary, rig: &Rig, layers: &[(String, f32)], phase: f32, out: &mut Vec<(Quat, Vec3)>) {
    out.clear();
    out.extend(rig.rest.iter().map(|_| (Quat::from_xyzw(0.0, 0.0, 0.0, 0.0), Vec3::ZERO)));
    let mut total = vec![0.0f32; rig.bone_ids.len()];
    let mut first: Vec<Option<Quat>> = vec![None; rig.bone_ids.len()];
    for (name, w) in layers {
        if *w <= 0.0 {
            continue;
        }
        let Some(c) = lib.clips.get(name) else { continue };
        let t = phase * c.duration;
        for (i, bone) in rig.bone_ids.iter().enumerate() {
            let r = c.rotations.get(bone).map(|k| sample_rot(k, t)).unwrap_or(rig.rest[i].rotation);
            let p = c.translations.get(bone).map(|k| sample_vec(k, t)).unwrap_or(rig.rest[i].translation);
            let r0 = *first[i].get_or_insert(r);
            let r = if r0.dot(r) < 0.0 { -r } else { r };
            let acc = &mut out[i];
            acc.0 = Quat::from_vec4(Vec4::from(acc.0) + Vec4::from(r) * *w);
            acc.1 += p * *w;
            total[i] += *w;
        }
    }
    for (i, acc) in out.iter_mut().enumerate() {
        if total[i] > 0.0 {
            acc.0 = acc.0.normalize();
            acc.1 /= total[i];
        } else {
            *acc = (rig.rest[i].rotation, rig.rest[i].translation);
        }
    }
}

pub fn apply_clip(
    time: Res<Time>,
    lib: Res<AnimLibrary>,
    mut q: Query<(&Rig, &mut AnimPlayer, &crate::player::Body)>,
    mut joints: Query<&mut Transform>,
    mut cur: Local<Vec<(Quat, Vec3)>>,
) {
    let dt = time.delta_secs();
    for (rig, mut p, body) in &mut q {
        if p.clip.is_none() || p.items.is_empty() {
            continue;
        }
        let item = p.items[p.item.min(p.items.len() - 1)].clone();
        let duration = lib.item_duration(&item).max(1e-3);
        let root_speed = item.layers.first().and_then(|(n, _)| lib.clips.get(n)).map(|c| c.root_speed).unwrap_or(0.0);
        // playback rate: locomotion cycles match their root motion to the actual speed (no foot
        // sliding); one-shots play at their own rate or are stretched to `fit`
        // (only the looping part: a transition action queued in front plays at its own rate)
        let rate = if p.looping && p.item >= p.loop_from && root_speed > 0.1 {
            let speed = Vec2::new(body.velocity.x, body.velocity.z).length();
            (speed / root_speed).clamp(0.3, 2.0)
        } else if let Some(fit) = p.fit {
            let total: f32 = p.items.iter().map(|i| lib.item_duration(i)).sum();
            total / fit.max(1e-3)
        } else {
            1.0
        };
        // B frozen while the blend runs (AROLLBSTOP / ASTOPBSTOP*)
        let b_stopped = p.fade < 1.0 && matches!(p.blend.kind, 3 | 4 | 5);
        let next = if b_stopped { p.phase } else { p.phase + dt * rate / duration };
        if let Some(ph) = p.sim_phase.filter(|_| p.item >= p.sim_from) {
            p.phase = ph;
        } else if next >= 1.0 && (p.item + 1 < p.items.len() || (p.looping && p.items.len() > 1)) {
            // the next item of the action's sequence (looping actions start again at their first item), entered with
            // its own blend, or the game's transition where one action ends and another begins
            let a_state = (p.items.clone(), p.item, 1.0, false);
            let cur = p.items[p.item].clone();
            p.item = if p.item + 1 < p.items.len() { p.item + 1 } else { p.loop_from.min(p.items.len() - 1) };
            let nxt = &p.items[p.item];
            let b = if nxt.action != cur.action && nxt.action != 0 {
                game_transition(&lib, Some(&cur), nxt.action).map(|t| t.blend_a).unwrap_or_default()
            } else {
                nxt.blend
            };
            start_blend(&mut p, b, true, Some(a_state), &lib);
            p.phase = 0.0;
        } else if p.looping {
            p.phase = next.fract();
        } else {
            p.phase = next.min(1.0);
        }
        // A keeps playing for the AROLLB* types
        if let Some((items, ai, aph, alooping)) = p.a_roll.as_mut() {
            let ad = items.get(*ai).map(|it| lib.item_duration(it)).unwrap_or(1.0).max(1e-3);
            let n = *aph + dt / ad;
            *aph = if *alooping { n.fract() } else { n.min(1.0) };
        }
        p.fade = (p.fade + dt / p.fade_time.max(0.01)).min(1.0);
        let weight = blend_weight(&p.blend, p.fade);
        let item = p.items[p.item].clone();

        // contact tags of the dominant clip (for the limb IK); ACTBlendAcuatorMode FROMA keeps A's while blending
        p.contacts = lib.dominant(&item).map(|c| contact_state(c, p.phase * c.duration)).unwrap_or_default();

        // event keys the dominant clip passed since the last frame
        p.events.clear();
        if let Some(c) = lib.dominant(&item) {
            let t = p.phase * c.duration;
            let from = p.event_cursor.as_ref().filter(|(n, _)| *n == c.name).map(|(_, f)| *f);
            let fired: Vec<FiredEvent> =
                passed_events(&c.events, from, t).map(|e| FiredEvent { clip: c.name.clone(), time: e.time, kind: e.kind.clone() }).collect();
            for e in &fired {
                debug!("anim event: {} t={:.3} {}", e.clip, e.time, event_label(&e.kind));
            }
            p.events = fired;
            p.event_cursor = Some((c.name.clone(), t));
        }

        let fading = p.fade < 1.0 && p.prev.is_some();
        if fading && p.blend.acuator == 0 {
            p.contacts = match p.a_roll.as_ref() {
                Some((items, ai, aph, _)) => items.get(*ai).and_then(|it| lib.dominant(it)).map(|c| contact_state(c, *aph * c.duration)).unwrap_or(p.prev_contacts),
                None => p.prev_contacts,
            };
        }
        // a rolling A is sampled again this frame
        if fading {
            if let Some((items, ai, aph, _)) = p.a_roll.as_ref() {
                if let Some(it) = items.get(*ai) {
                    let mut a_pose = Vec::new();
                    sample_layers(&lib, rig, &it.layers, *aph, &mut a_pose);
                    p.prev = Some(a_pose);
                }
            }
        }
        if let Ok(mut root) = joints.get_mut(rig.root) {
            *root = anim_root();
            if !p.looping && item.root_motion {
                let mut off = Vec3::ZERO;
                let mut wsum = 0.0;
                for (n, w) in &item.layers {
                    if let Some(c) = lib.clips.get(n) {
                        off += root_motion_offset(c, p.phase) * *w;
                        wsum += *w;
                    }
                }
                if wsum > 0.0 {
                    root.translation = root.rotation * (off / wsum);
                }
            }
            if fading {
                // ACTBlendDispSrcMode: BLENDAB, FROMAONLY, FROMBONLY
                root.translation = match p.blend.disp_src {
                    1 => p.prev_root,
                    2 => root.translation,
                    _ => p.prev_root.lerp(root.translation, weight),
                };
            }
            p.last_root = root.translation;
        }
        sample_layers(&lib, rig, &item.layers, p.phase, &mut cur);
        let prev = if fading { p.prev.take() } else { None };
        let empty = Vec::new();
        let prev_pose = prev.as_ref().filter(|v| v.len() == cur.len()).unwrap_or(&empty);
        let mut shown = std::mem::take(&mut p.last_pose);
        shown.clear();
        let mut base: Vec<(Quat, Vec3)> = (0..rig.joints.len())
            .map(|i| {
                let (rot, pos) = cur[i];
                match prev_pose.get(i) {
                    Some(&(r0, p0)) => (qinterp(r0, rot, weight), p0.lerp(pos, weight)),
                    None => (rot, pos),
                }
            })
            .collect();
        if let Some(o) = p.overlay.as_mut() {
            apply_overlay(o, &lib, rig, &mut base, dt);
        }
        if p.overlay.as_ref().is_some_and(|o| o.ending && o.weight <= 0.0) {
            p.overlay = None;
        }
        for (i, e) in rig.joints.iter().enumerate() {
            let (rot, pos) = base[i];
            shown.push((rot, pos));
            let Ok(mut tr) = joints.get_mut(*e) else { continue };
            if !(rot.is_finite() && pos.is_finite()) && std::env::var_os("AC_NAN_LOG").is_some() {
                warn!("non-finite joint {i} (bone {:08x}) clip {:?} item {} phase {:.3} fade {:.3} layers {:?}", rig.bone_ids[i], p.clip, p.item, p.phase, p.fade, item.layers);
            }
            tr.rotation = rot;
            tr.translation = pos;
        }
        p.last_pose = shown;
        p.prev = if p.fade >= 1.0 { None } else { prev };
    }
}

fn load_library(mut lib: ResMut<AnimLibrary>) {
    if std::env::var_os("AC_NO_MODEL").is_some() {
        return;
    }
    match load_locomotion(&game_dir()) {
        Ok((raw, graph, names)) => {
            for c in raw {
                let rotations = c.rotations.into_iter().map(|(k, v)| (k, v.into_iter().map(|(t, q)| (t, Quat::from_array(q).normalize())).collect())).collect();
                let translations: HashMap<u32, Vec<(f32, Vec3)>> =
                    c.translations.into_iter().map(|(k, v)| (k, v.into_iter().map(|(t, p)| (t, Vec3::from_array(p))).collect())).collect();
                let root_speed = translations
                    .get(&crate::assets::ac_anim::TRACK_DISPLACEMENT)
                    .and_then(|k| Some((k.first()?.1, k.last()?.1)))
                    .map(|(a, b)| (b - a).length() / c.duration.max(1e-3))
                    .unwrap_or(0.0);
                lib.clips.insert(c.name.clone(), AnimClip { name: c.name, duration: c.duration, rotations, translations, root_speed, contacts: c.contacts, events: c.events });
            }
            lib.status = format!("anims: {} clips, {} graph actions from your install", lib.clips.len(), graph.actions.len());
            lib.graph = graph;
            lib.anim_names = names;
            match crate::assets::body_parts::load_body_parts(&game_dir()) {
                Ok(b) => lib.body = b,
                Err(e) => warn!("body parts not loaded: {e}"),
            }
            info!("{}", lib.status);
        }
        Err(e) => {
            lib.status = format!("anims: not loaded ({e})");
            warn!("{}", lib.status);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_weights_follow_the_ease_flags() {
        // sub_773C30: linear, ease in 2t² − t³, ease out t + t² − t³, both 3t² − 2t³
        let b = |ease_in, ease_out| ActBlend { ease_in, ease_out, ..ActBlend::DEFAULT };
        for (bl, mid) in [(b(false, false), 0.5), (b(true, false), 0.375), (b(false, true), 0.625), (b(true, true), 0.5)] {
            assert_eq!(blend_weight(&bl, 0.0), 0.0);
            assert!((blend_weight(&bl, 1.0) - 1.0).abs() < 1e-6);
            assert!((blend_weight(&bl, 0.5) - mid).abs() < 1e-6, "{bl:?}");
        }
    }

    #[test]
    fn masked_blend_touches_only_the_masked_bones() {
        let mut pose = vec![(Quat::IDENTITY, Vec3::ZERO); 3];
        let over = vec![(Quat::from_rotation_x(1.0), Vec3::X); 3];
        blend_masked(&mut pose, &over, &[false, true, true], 0.5);
        assert_eq!(pose[0], (Quat::IDENTITY, Vec3::ZERO));
        assert!((pose[1].1.x - 0.5).abs() < 1e-6);
        assert!((pose[2].0.angle_between(Quat::IDENTITY) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn body_part_channels_load_from_the_install() {
        let dir = game_dir();
        if !dir.join("DataPC.forge").exists() {
            eprintln!("skipped: game install not found");
            return;
        }
        let b = crate::assets::body_parts::load_body_parts(&dir).expect("body parts");
        let full = b.channels[&0x01B9_9BC5];
        assert_eq!((full.group, full.mask), (0, 0xFFFF_FFFF), "channel 0: the full body");
        let upper = b.channels[&0x01B9_9C0A];
        assert_eq!((upper.group, upper.mask), (1, 0x1FE), "channel 1: every part but the legs");
        assert_eq!(b.parts.len(), 10);
        assert!(b.parts[5].contains(&0xeb83_0ada), "part 5 = the left arm (LeftArm)");
        assert!(!b.bones(0x1FE).contains(&0xded1_0611), "the upper body leaves the hips to the full-body slot");
        // every movement action is on the full-body channel
        let (_, graph, _) = load_locomotion(&dir).expect("graph");
        let movement = ["HumanClimb", "HumanLedge", "HumanInAir", "HumanWalling", "HumanNarrow", "HumanLadder"];
        for a in graph.actions.values().filter(|a| movement.iter().any(|m| a.block.starts_with(m))) {
            if a.id == ACT_FALL || a.id == 0xCD6F_5E10 {
                assert_eq!(b.channels.get(&a.channel).map(|c| c.group), Some(1), "the falling blend is upper-body");
            } else {
                assert_eq!(b.channels.get(&a.channel).map(|c| c.group), Some(0), "{:08x} in {}", a.id, a.block);
            }
        }
    }

    #[test]
    fn transitions_are_looked_up_as_the_game_does() {
        let dir = game_dir();
        if !dir.exists() {
            eprintln!("skipped: game install not found");
            return;
        }
        let (_, graph, _) = load_locomotion(&dir).expect("graph");
        let lib = AnimLibrary { graph, ..Default::default() };
        let item = |action: u32| {
            let it = &lib.graph.actions[&action].items[0];
            ItemPlay { layers: Vec::new(), blend: it.blend, root_motion: false, action, transitions: it.transitions.clone() }
        };
        // 1. the playing item's transition to the requested action: high wait (0x00D8258E) → fight wait (0x00FD645B)
        //    goes through `xx_h_wait_hipm_footr_tr_xx_h_light_wait_footr` (0x1D504306)
        let t = game_transition(&lib, Some(&item(0x00D8_258E)), 0x00FD_645B).expect("item transition");
        assert_eq!(t.action_a, 0x1D50_4306);
        // 3. the run stop's own in-transition (0x00D837AE): A and B both play for 0.2 s, root motion from B only
        let t = game_transition(&lib, Some(&item(0x00D8_258E)), 0x00D8_37AE).expect("in-transition");
        assert_eq!((t.action_a, t.blend_a.kind, t.blend_a.disp_src), (0, 1, 2));
        assert!((t.blend_a.time - 0.2).abs() < 1e-6);
        // 4. nothing authored: the default transition (A frozen, B playing, 0.2 s)
        let t = game_transition(&lib, Some(&item(0x00D8_258E)), 0x00D8_243F).expect("default");
        assert_eq!(t.blend_a, ActBlend::DEFAULT);
        // the repeat count: idles loop, the pull-up plays once
        assert_eq!(lib.graph.actions[&0x0106_F2E8].repeat, 0);
        assert_eq!(lib.graph.actions[&0x0106_D2C5].repeat, 1);
    }

    #[test]
    fn fall_grasp_weights_follow_check_air_catch() {
        let f = Vec3::NEG_Z;
        // no reach → the static fall pose only
        assert_eq!(fall_grasp_weights(Vec3::ZERO, f), [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        // full reach straight ahead → grasp front
        let w = fall_grasp_weights(f, f);
        assert!(w[0].abs() < 1e-5 && (w[1] - 1.0).abs() < 1e-5);
        // 45° to the right → half front, half right
        let d = (f + crate::player::right_of(f)).normalize();
        let w = fall_grasp_weights(d, f);
        assert!((w[1] - 0.5).abs() < 1e-4 && (w[3] - 0.5).abs() < 1e-4);
        // half-length reach behind-left → half static, rest back-left
        let w = fall_grasp_weights(-f * 0.5, f);
        assert!((w[0] - 0.5).abs() < 1e-4);
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn passed_events_cover_the_frame_and_the_loop_wrap() {
        let ev = |t: f32| AnimEvent { time: t, kind: EventKind::Contact { ty: 12, bits: 3 } };
        let keys = [ev(0.0), ev(0.167), ev(0.7)];
        let times = |from: Option<f32>, to: f32| passed_events(&keys, from, to).map(|e| e.time).collect::<Vec<_>>();
        // a new clip fires its key at 0
        assert_eq!(times(None, 0.01), vec![0.0]);
        // (from, to]: a key exactly at the frame's end fires once, not again next frame
        assert_eq!(times(Some(0.1), 0.167), vec![0.167]);
        assert!(times(Some(0.167), 0.2).is_empty());
        // the loop wrapped: the tail of the cycle, then its start
        assert_eq!(times(Some(0.68), 0.05), vec![0.7, 0.0]);
    }

    #[test]
    fn contact_state_finds_travel_windows() {
        // xx_l_climb_1m_u_1lu: L hand + L toe off at 0.067, L hand back at 0.333, L toe at 0.533
        let c = AnimClip { duration: 0.533, contacts: vec![(0.0, 0b111100), (0.067, 0b101000), (0.333, 0b111000), (0.533, 0b111100)], ..default() };
        let s = contact_state(&c, 0.2);
        assert_eq!(s.bits >> 4 & 1, 0);
        assert!((s.off_since[4].unwrap() - 0.067).abs() < 1e-4);
        assert!((s.next_on[4].unwrap() - 0.333).abs() < 1e-4);
        assert!((s.next_on[2].unwrap() - 0.533).abs() < 1e-4);
        assert_eq!(contact_state(&c, 0.4).bits >> 4 & 1, 1);
    }
}
