//! Animation playback on Altaïr's rig.
//!
//! Clips are decoded from the user's own install (RE/10): per-bone local rotation/translation keys
//! (keyed by BoneID), the root DISPLACEMENT track (root motion) and the ACUATORCONTACTS track (limb
//! contact tags). Clips are chosen through the game's **animation graph** (RE/13): the contexts name an
//! *action* by the id the exe uses; an action is a sequence of *items*, each a weighted blend of clips
//! with its own blend time and displacement mode. While a clip plays the rig root converts animation
//! space (Z-up, facing +Y, feet at 0) to Bevy space.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;

use crate::assets::ac_actions::{ActBlend, ActionGraph, DisplacementMode};
use crate::assets::ac_anim::{AnimEvent, EventKind, CONTACT_HUMAN};
use crate::assets::anims::load_locomotion;
use crate::assets::game_dir;
use crate::bone_pose::{BonePose, PoseSkeleton};
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

/// Contact bits at time `t` (keys are held: ByteLowest, `sub_491040` returns the key with time[k] ≤ frame < time[k+1],
/// frame = (int)(t·60 + 0.5), so the game switches half a tick early; RE/24 ANIM-14).
pub fn contact_bits(keys: &[(f32, u8)], t: f32) -> u8 {
    if keys.is_empty() {
        return 0;
    }
    let i = keys.partition_point(|k| k.0 <= t + 1e-4);
    keys[i.saturating_sub(1)].1
}

/// The clips and the action graph from the install. The tables are shared (`Arc`): the headless runs clone one load
/// per process (`shared_library`).
#[derive(Resource, Default, Clone)]
pub struct AnimLibrary {
    pub clips: Arc<HashMap<String, AnimClip>>,
    pub graph: Arc<ActionGraph>,
    /// Body-part channels and the bones of each part (from the install).
    pub body: crate::assets::body_parts::BodyParts,
    /// Animation resource id → resource (clip) name.
    pub anim_names: Arc<HashMap<u32, String>>,
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
    /// The item word (+60, RE/13 §7.6): 0x20 = locked.
    pub word: u16,
    /// ActionItem +24: the item plays this many times in a row (0 is read as 1; RE/24 ANIM-6).
    pub repeat: u32,
}

impl ItemPlay {
    /// The clips' own phase at the item's phase: a repeated item runs its clips `repeat` times (`ItemEvaluator__Advance`
    /// 0x778010 restarts them while rep + 1 < item +24, 0x7780D2).
    pub fn clip_phase(&self, phase: f32) -> f32 {
        let r = self.repeat.max(1) as f32;
        if r <= 1.0 || phase >= 1.0 {
            phase
        } else {
            (phase * r).fract()
        }
    }
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
                // the authored weights through the evaluator's setup (0x777AE0, RE/24 ANIM-1)
                let mut ws: Vec<f32> = (0..it.animations.len()).map(|k| it.weights.get(k).copied().unwrap_or(if k == 0 { 1.0 } else { 0.0 })).collect();
                crate::player::jump_blend::normalize_item_weights(&mut ws);
                for (k, anim) in it.animations.iter().enumerate() {
                    let w = ws[k];
                    if let Some(n) = self.anim_names.get(anim).filter(|n| self.clips.contains_key(*n)) {
                        layers.push((n.clone(), w));
                    }
                }
                if layers.iter().all(|l| l.1 <= 0.0) {
                    if let Some(l) = layers.first_mut() {
                        l.1 = 1.0;
                    }
                }
                ItemPlay { layers, blend: it.blend, root_motion: it.displacement == DisplacementMode::FromAnim, action: id, transitions: it.transitions.clone(), word: it.flags, repeat: it.repeat }
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
        // a repeated item's runtime length (`sub_7778A0` 0x777A3D: duration × item +24; RE/24 ANIM-6)
        if w > 0.0 { d / w * item.repeat.max(1) as f32 } else { 0.0 }
    }

    /// The item's dominant clip: the first of equal maxima (`sub_777AE0` keeps it with a strict `w > max`, 0x777BF9;
    /// RE/24 ANIM-9).
    fn dominant<'a>(&'a self, item: &ItemPlay) -> Option<&'a AnimClip> {
        item.layers.iter().fold(None::<&(String, f32)>, |best, l| if best.is_none_or(|b| l.1 > b.1) { Some(l) } else { best }).and_then(|(n, _)| self.clips.get(n))
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

/// The next contact of a released limb is searched through the rest of the playing action, each later item in its
/// dominant clip (`LimbIK__GetNextContactTime` 0xE56220 → `sub_5085C0` 0x50864E–0x50872D, forward to the action's last
/// item, no wrap), not only in the current clip (RE/24 ANIM-7). `later`: the dominant clips of the action's following
/// items, in order. Times stay in the current clip's seconds: the rest of this item, then each later clip's length.
fn extend_next_on<'a>(s: &mut ContactState, cur: &AnimClip, later: impl Iterator<Item = &'a AnimClip>) {
    if !s.tagged {
        return;
    }
    let mut offset = cur.duration;
    for c in later {
        for b in 0..8 {
            if s.bits >> b & 1 != 0 || s.next_on[b].is_some() {
                continue;
            }
            if let Some(kt) = c.contacts.iter().find(|(_, v)| v >> b & 1 != 0).map(|k| k.0) {
                s.next_on[b] = Some(offset + kt);
            }
        }
        offset += c.duration;
    }
}

/// Playback state on the player. Its pose goes to the `BonePose` beside it (`animate_pose`).
#[derive(Component, Default)]
#[require(crate::bone_pose::BonePose)]
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
    /// The running blend is a transition between two actions (slot 0's selected evaluator is then the transition's
    /// ActionControllerEvaluator, 0x726F40 / 0x727560, so `AnimSlotInstance__GetSelectedAction` 0x723D10 returns no
    /// action), not a seam between two items of one action.
    pub action_switch: bool,
    /// A still playing while the blend runs (AROLLB* types): its items, item and phase.
    a_roll: Option<(Vec<ItemPlay>, usize, f32, bool)>,
    /// The action A belonged to when the running blend started (0 for a named clip): the root displacement shares
    /// (`root_shares_for`) apply only across a switch between two actions.
    blend_from: u32,
    /// A's contact tags when the blend started (ACTBlendAcuatorMode FROMA).
    prev_contacts: ContactState,
    /// The action sequence loops back to this item (the items before it are a transition action).
    loop_from: usize,
    /// Action +32 of the requested action: the item its loop starts again at, after `loop_from` (RE/24 ANIM-5).
    loop_item: usize,
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
    /// The destination of the ground transition action playing and the blend to hand over with (its blend B);
    /// destination 0 = the wait.
    pending_b: Option<(u32, ActBlend)>,
    /// A partial-body action playing over the full-body one (a channel of slot group 1).
    pub overlay: Option<Overlay>,
    /// Action +48 flags of the playing item's action (bit 3 lets the standing foot IK run, `IKGroundBiped__Update`
    /// 0x432C8A); 8 for named clips outside the graph.
    pub action_flags: u8,
    /// The walking fidget (RE/24 GROUND-18): the ground's last fidget request taken, the group picks played so far and
    /// the picker's random state.
    fidget_seq: u32,
    fidget_played: Vec<u32>,
    fidget_rng: u32,
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
    /// 0 → 1 over `fade`, back to 0 when the action ends.
    pub weight: f32,
    /// Its blend in and out (s).
    fade: f32,
    ending: bool,
    looping: bool,
    /// Action +32: the item a looping action starts again at (RE/24 ANIM-5).
    loop_item: usize,
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
        self.play_overlay_fade(lib, action, OVERLAY_FADE)
    }

    /// `play_overlay` with its own blend time.
    pub fn play_overlay_fade(&mut self, lib: &AnimLibrary, action: u32, fade: f32) -> bool {
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
        let loop_item = (a.loop_item as usize).min(items.len() - 1);
        self.overlay = Some(Overlay { action, items, item: 0, phase: 0.0, weight, fade: fade.max(1e-3), ending: false, looping: a.repeat == 0, loop_item, bones, mask: Vec::new() });
        true
    }

    /// Set the layer weights of the overlay's current item (a blended action such as the fall-grasp blend).
    pub fn set_overlay_weights(&mut self, w: &[f32]) {
        let mut w = w.to_vec();
        crate::player::jump_blend::normalize_item_weights(&mut w);
        if let Some(it) = self.overlay.as_mut().and_then(|o| o.items.get_mut(o.item)) {
            if it.layers.len() == w.len() {
                for (l, w) in it.layers.iter_mut().zip(&w) {
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

/// The action an action with no items plays from its `AssociatedActionGroup` (`Anim__PlayActionPtr` 0x4FF5B0 →
/// `sub_72AD60` → `sub_72A9D0`): a weighted random pick (the game's LCG) among the entries not played yet, each entry's
/// weight its share; once every entry has played, all are open again (the group's flag bit 0; PORT: the game's history
/// of picks is kept per animation component and may expire, **hypothesis**). Used for the walking fidget (RE/24
/// GROUND-18).
pub fn pick_associated(lib: &AnimLibrary, action: u32, played: &mut Vec<u32>, rng: &mut u32) -> Option<u32> {
    let a = lib.graph.actions.get(&action)?;
    let entries: Vec<(u32, f32)> = a.assoc.iter().map(|e| (e[2], f32::from_bits(e[3]))).collect();
    let mut open: Vec<(u32, f32)> = entries.iter().filter(|e| !played.contains(&e.0)).copied().collect();
    if open.is_empty() {
        played.retain(|id| !entries.iter().any(|e| e.0 == *id));
        open = entries;
    }
    let total: f32 = open.iter().map(|e| e.1).sum();
    *rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    let mut x = total * (*rng & 0x0FFF_FFFF) as f32 / 0x1000_0000 as f32;
    let pick = open.iter().find(|e| {
        let hit = e.1 >= x;
        x -= e.1;
        hit
    }).or(open.last())?.0;
    played.push(pick);
    Some(pick)
}

/// Blend the overlay's pose over the full-body pose on its bones; advances it by `dt`.
fn apply_overlay(o: &mut Overlay, lib: &AnimLibrary, skel: &PoseSkeleton, pose: &mut [(Quat, Vec3)], dt: f32) {
    if o.mask.len() != skel.bone_ids.len() {
        o.mask = skel.bone_ids.iter().map(|b| o.bones.contains(b)).collect();
    }
    let Some(it) = o.items.get(o.item).cloned() else { return };
    let duration = lib.item_duration(&it).max(1e-3);
    o.phase += dt / duration;
    if o.phase >= 1.0 {
        if o.item + 1 < o.items.len() {
            o.item += 1;
            o.phase = 0.0;
        } else if o.looping {
            o.item = o.loop_item;
            o.phase = o.phase.fract();
        } else {
            o.phase = 1.0;
            o.ending = true;
        }
    }
    o.weight = if o.ending { (o.weight - dt / o.fade).max(0.0) } else { (o.weight + dt / o.fade).min(1.0) };
    let mut ov = Vec::new();
    sample_layers(lib, skel, &o.items[o.item].layers, o.items[o.item].clip_phase(o.phase), &mut ov);
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
    /// PORT: fallback only (named clips, `named_blend`); graph actions take the authored blends (RE/24 ANIM-18).
    fade: f32,
    hold: bool,
    /// The blend into the requested action when the simulation knows it (a ground transition action: the authored
    /// item transition's blend A, or the code's explicit one); `None`: the graph's lookup (`sub_725950`).
    blend: Option<ActBlend>,
    /// The two weight sets the code applies right after the request (0xDE1FE0's swing into the free idle): the playing
    /// item's (`sub_502E30`) and then `AnimGraph__SetItemBlendWeights` 0x503040's. When the graph queues a transition
    /// action in front, the first set goes to that action's first item, which is the one playing, and the second to the
    /// requested action queued behind it (hypothesis: live, the swing's length matches the first set). Without a queued
    /// action the request's own weights stand.
    playing_w: Option<(Vec<f32>, Vec<f32>)>,
}

const CROSSFADE: f32 = 0.2;

pub struct AnimPlugin;

impl Plugin for AnimPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnimLibrary>()
            .init_resource::<crate::bone_pose::PoseSkeleton>()
            .add_systems(Startup, load_library)
            .add_systems(Update, (choose_clip, animate_pose, apply_clip).chain().after(crate::player::PlayerSet));
    }
}

/// Named one-shots whose DISPLACEMENT track shapes the body's path (the graph's FromAnim mode is used for
/// actions).
pub fn uses_root_motion(name: &str) -> bool {
    name.starts_with("shimmy_") || name.starts_with("pullup_") || name.starts_with("catch_")
}

fn single(name: &str) -> Vec<ItemPlay> {
    vec![ItemPlay { layers: vec![(name.to_string(), 1.0)], blend: named_blend(CROSSFADE), root_motion: uses_root_motion(name), action: 0, transitions: Vec::new(), word: 0, repeat: 1 }]
}

fn looped(clip: &str, fade: f32) -> Request {
    Request { key: clip.into(), items: single(clip), sim: false, looping: true, fit: None, token: 0, fade, hold: false, blend: None, playing_w: None }
}

fn once(clip: String, token: u64, fit: Option<f32>, fade: f32) -> Request {
    Request { items: single(&clip), key: clip, sim: false, looping: false, fit, token, fade, hold: false, blend: None, playing_w: None }
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
    Some(Request { key, items, sim: false, looping, fit: None, token, fade, hold: false, blend: None, playing_w: None })
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
/// 4. the default transition (ASTOPBROLL 0.2 s, `sub_46A460`), unless `new` has flag bit 2, and only when the playing
///    item and `new`'s item 0 differ in the player's animation remap (0x725A3E: `AnimItem__DominantAnimEntry` 0x5BC790
///    of each, the remap entry of its heaviest remapped clip above 0.0005, or none; RE/24 ANIM-3). Altaïr's one set
///    remaps only the twelve `xx_turn_on_spot_*` clips, so for him the default applies only into or out of those.
/// `None` means a cut (SetAction 0x727F70 pushes the action with a default-constructed ActionBlend: type NONE).
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
    let remapped = |it: &ItemPlay| it.layers.iter().any(|(n, w)| *w > 0.0005 && lib.graph.remap.iter().any(|r| r.2 == *n));
    // the new action's item 0 with its authored weights (`sub_507CE0`)
    let new_remapped = new_action.items.first().is_some_and(|it| {
        it.animations.iter().enumerate().any(|(k, a)| it.weights.get(k).copied().unwrap_or(if k == 0 { 1.0 } else { 0.0 }) > 0.0005 && lib.graph.remap.iter().any(|r| r.0 == *a))
    });
    (new_action.flags & 4 == 0 && cur.is_some_and(|it| remapped(it)) != new_remapped)
        .then_some(ActTransition { blend_a: ActBlend::DEFAULT, ..Default::default() })
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

/// The shares of the root displacement while a blend runs (RE/24 ANIM-16, `sub_774350`): (A's, B's). A's displacement
/// is gathered only while A plays on (AROLLB*, kinds 1 / 3); then ACTBlendDispSrcMode: BLENDAB lerps A's and B's by the
/// blend weight `w`, FROMAONLY takes A's, FROMBONLY B's. So under the default ASTOPBROLL the new action's root motion
/// ramps in with the weight. PORT: B is taken to play on in every type (the simulation's clock does not stop for the
/// ASTOPBSTOP / AROLLBSTOP types, so their displacement is not held back).
pub fn root_shares(b: &ActBlend, w: f32) -> (f32, f32) {
    let a = if matches!(b.kind, 1 | 3) { 1.0 } else { 0.0 };
    match b.disp_src {
        1 => (a, 0.0),
        2 => (0.0, 1.0),
        _ => (a * (1.0 - w), w),
    }
}

impl AnimPlayer {
    /// `root_shares` for the simulation's `action`: while the running blend switches from another action into it;
    /// (0, 1) otherwise (no blend, an item seam within one action, or the animation not yet on that action). One frame
    /// late: the animation runs after the simulation.
    pub fn root_shares_for(&self, action: u32) -> (f32, f32) {
        let blending = self.fade < 1.0 && self.prev.is_some() && self.action_switch;
        let on_it = self.items.get(self.item).is_some_and(|i| i.action == action && action != 0);
        if !blending || !on_it || self.blend_from == action {
            return (0.0, 1.0);
        }
        root_shares(&self.blend, blend_weight(&self.blend, self.fade))
    }
}

/// The weight of `action`'s own displacement this frame (`root_shares_for`, A's share counted as the same motion:
/// PORT, the simulation runs one action, so a rolling A's displacement stands for itself). 1 without an animation.
pub fn root_weight(anim: Option<&AnimPlayer>, action: u32) -> f32 {
    anim.map_or(1.0, |p| {
        let (a, b) = p.root_shares_for(action);
        a + b
    })
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
/// `PlayIdleOrBlocked` 0xDD3A10's blocked actions (wall hang): [up, down, left, right].
const ACT_LEDGE_BLOCKED: [u32; 4] = [0x5823_575D, 0x5823_575A, 0x5823_575B, 0x5823_575C];
const ACT_HANG_FREE: u32 = 0x0127_19F1;
const ACT_HANG_WALLFREE: u32 = 0x0106_F2E9;
const ACT_PULLUP_WALL: u32 = 0x0106_D2C5;
const ACT_PULLUP_FREE: u32 = 0x0127_19F2;
/// Pull-up outcome "stand": hangknee → free-step entry (0xDE2EE0 → 0xDD2A50).
const ACT_HANGKNEE_TO_WAIT: u32 = crate::player::ledge_moves::ACT_KNEE_TO_FREESTEP[0];
use crate::player::ledge::{shimmy_index, vstep_index, ACT_SHIMMY, ACT_VSTEP};
/// Catches (CheckAirCatch): ledge with wall below (3-way angle blend), free hang; [< 3 m, ≥ 3 m].
const ACT_CATCH_WALL: [u32; 2] = crate::player::ledge::ACT_CATCH_WALL;
const ACT_CATCH_FREE: [u32; 2] = crate::player::ledge::ACT_CATCH_FREE;
/// Falling (6-way grasp blend).
const ACT_FALL: u32 = 0x1F0C_22C2;

/// The ground waits (HumanGround), [low, high profile] × [left, right foot ahead]: `xx_{l,h}_wait_hipm_foot{l,r}`.
const ACT_WAIT: [[u32; 2]; 2] = crate::player::ground_tree::WAITS;

/// The item after `it` in its looping action, with `it`'s layer weights (None: a one-item action, or a named clip).
fn next_loop_item(lib: &AnimLibrary, it: &ItemPlay) -> Option<ItemPlay> {
    let all = lib.action_items(it.action).filter(|a| a.len() > 1)?;
    let k = all.iter().position(|x| x.layers.first().map(|l| &l.0) == it.layers.first().map(|l| &l.0))?;
    let mut next = all[(k + 1) % all.len()].clone();
    if next.layers.len() == it.layers.len() {
        for (l, w) in next.layers.iter_mut().zip(it.layers.iter()) {
            l.1 = w.1;
        }
    }
    next.root_motion = it.root_motion;
    Some(next)
}

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
    Some(Request { key, items: vec![it], sim: true, looping: false, fit: None, token, fade, hold: false, blend: None, playing_w: None })
}

/// The seam from a queued transition action's last item into the requested action's first item (`loop_from`, on the
/// first pass only: a loop back to `loop_from` comes from the action's own last item).
fn queued_seam(loop_from: usize, from: usize, to: usize) -> bool {
    loop_from > 0 && from + 1 == loop_from && to == loop_from
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
    fly: Option<Res<crate::debug_fly::DebugFly>>,
) {
    use crate::player::air::AirMode;
    let flying = fly.is_some_and(|f| f.active);
    use crate::player::ledge::{LedgeHangType, LedgeSubState};
    let dt = time.delta_secs();
    for (loco, data, body, mut p) in &mut q {
        let g = &data.ground;
        p.sim_phase = None;
        // the falling blend belongs to InAir: leaving it (a landing, a catch) fades the partial slot out
        if loco.current != ActorContextId::InAir && p.overlay.as_ref().is_some_and(|o| o.action == ACT_FALL) {
            p.stop_overlay();
        }
        // the walking fidget on the upper-body slot (RE/24 GROUND-18): a new request picks a cycle breaker from the
        // player's group (variant 0); a run, a locked upper body or leaving the ground ends it
        {
            use crate::player::ground_tree::{CYCLE_BREAKER, CYCLE_BREAKER_BLEND};
            let is_fidget = |lib: &AnimLibrary, id: u32| CYCLE_BREAKER.iter().any(|c| lib.graph.actions.get(c).is_some_and(|a| a.assoc.iter().any(|e| e[2] == id)));
            let f = g.fidget;
            if loco.current == ActorContextId::Ground && f.seq != p.fidget_seq {
                p.fidget_seq = f.seq;
                let (mut played, mut rng) = (std::mem::take(&mut p.fidget_played), p.fidget_rng);
                if let Some(a) = pick_associated(&lib, CYCLE_BREAKER[0], &mut played, &mut rng) {
                    p.play_overlay_fade(&lib, a, CYCLE_BREAKER_BLEND);
                }
                (p.fidget_played, p.fidget_rng) = (played, rng);
            }
            let stop = loco.current != ActorContextId::Ground || f.stop;
            if stop && p.overlay.as_ref().is_some_and(|o| !o.ending && is_fidget(&lib, o.action)) {
                p.stop_overlay();
            }
        }
        if let Some(ctx) = p.hold {
            if ctx == loco.current && !(p.phase >= 1.0 && p.item + 1 >= p.items.len()) {
                continue;
            }
            p.hold = None;
        }
        let req = match loco.current {
            // the debug flight is the game's Debug context (3, 0xE46190): action request 7, Ghost mode
            _ if flying => action(&lib, &[crate::debug_fly::GHOST_MODE], true, 1_900_000, Some(0.2), None),
            ActorContextId::Ground if g.crouch.is_some() => {
                let c=g.crouch.unwrap();p.sim_phase=Some(c.phase());
                sim_request(&mut p,&lib,&c.action,1_800_000+c.sequence as u64,c.fade)
            }
            ActorContextId::Ground if g.extra.is_some() => {
                let extra=g.extra.unwrap();
                p.sim_phase=Some(extra.phase());
                sim_request(&mut p,&lib,&extra.action,1_700_000+extra.sequence as u64,0.2)
            }
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
            // a transition action in front of the locomotion or the wait (`ground_tree`, MoveBlend's transition path):
            // the sim's weights and phase; entered with the authored item transition's blend A (or the code's explicit
            // blend); its blend B is kept for the hand-over to the destination
            ActorContextId::Ground if g.tr.is_some_and(|t| sim_item(&lib, &t.action).is_some()) => {
                let t = g.tr.unwrap();
                p.sim_phase = Some(t.phase);
                let token = 1_100_000 + t.seq as u64;
                let key = format!("act_{:08x}", t.action.id);
                let it = sim_item(&lib, &t.action).unwrap();
                if p.clip.as_deref() == Some(key.as_str()) && p.token == token {
                    p.set_sim_item(it);
                    continue;
                }
                // the same transition action can lead to several destinations (e.g. the skid turn's exit into the
                // locomotion and into a fight action): the one into the locomotion, or into a wait
                let wanted = |b: u32| if t.to_wait { ACT_WAIT.iter().flatten().any(|w| *w == b) || crate::player::ground_tree::WAITS_PARALLEL.contains(&b) } else { b == ACT_GROUND_LOCOMOTION };
                let authored = t.from.and_then(|(a, i)| lib.graph.actions.get(&a).and_then(|a| a.items.get(i)))
                    .and_then(|it| it.transitions.iter().find(|tr| tr.action_a == t.action.id && wanted(tr.action_b)).copied());
                let cur = p.items.get(p.item.min(p.items.len().max(1) - 1)).filter(|_| p.clip.is_some()).cloned();
                let blend_a = t.blend_in.or(authored.map(|a| a.blend_a)).unwrap_or_else(|| game_transition(&lib, cur.as_ref(), t.action.id).map(|tr| tr.blend_a).unwrap_or_default());
                // PORT (hypothesis): a transition the code passes explicitly (start, run stop / skid-turn exits) hands over
                // with a cut, as the authored ones do (their clips are made to meet the destination's first frame)
                let dest = authored.map(|a| a.action_b).unwrap_or(if t.to_wait { 0 } else { ACT_GROUND_LOCOMOTION });
                let blend_b = authored.map(|a| a.blend_b).unwrap_or_default();
                p.pending_b = Some((dest, blend_b));
                Some(Request { key, items: vec![it], sim: true, looping: false, fit: None, token, fade: 0.0, hold: false, blend: Some(blend_a), playing_w: None })
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
                Some(Request { key, items: vec![it], sim: true, looping: true, fit: None, token: 0, fade: CROSSFADE, hold: false, blend: None, playing_w: None })
            }
            // standing: the game's wait action of the leading foot (MoveBlend foot: 0 = left ahead), entered through the
            // graph's transition like every action (RE/13 §7)
            ActorContextId::Ground if speed_band(g.speed_param) == SpeedBand::None => {
                // the wait's own profile and the stance-change wait (HG+1500 / HG+1524, RE/24 GROUND-14)
                let id = if crate::tuning::GAME_GROUND_TREE { crate::player::ground_tree::idle_wait(g.wait_high, g.wait_foot, g.stance_change) } else { ACT_WAIT[g.high_profile as usize][(g.blend.foot != 0) as usize] };
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
                AirMode::Jump { t, t_takeoff, duration, .. } if data.air.flight.is_some_and(|b| sim_item(&lib, &b).is_some()) => {
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
                // Leap of Faith free-fall tail: `faith_jump_fall` (0xB1EC40 third action), played when the flight ends with
                // a 0.5 s blend (0xE0F3D4, RE/24 AIR-22); the let-go's dive keeps its own blend
                AirMode::Fall { .. } if data.air.flight.is_some_and(|f| f.id == crate::player::jump_blend::FLIGHT_FAITH) => {
                    let b = crate::player::jump_blend::ActionBlend::new(crate::player::jump_blend::FALL_FAITH, 0, &[1.0]);
                    p.sim_phase = Some(0.5);
                    sim_request(&mut p, &lib, &b, 2_500_000 + data.air.seq as u64, if data.air.hollywood { 0.2 } else { 0.5 })
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
                    // a jump set up by 0xB20200 whose flight ended without an arrival: the JumpFallAction (+432, request
                    // 33 = `xx_h_jump_falling01` 0x00D81513) plays at once with a 0.5 s blend (0xE0F3D4–0xE0F45B, RE/24
                    // AIR-22). PORT: the falling blend 0x1F0C22C2 stands in for it (its layer 0 is falling01; the grasp
                    // layers carry the empty-hand grasp the game plays on the upper body)
                    let after_flight = data.air.takeoff.is_some() && data.air.flight.is_some();
                    if p.seen_fall != Some(data.air.seq) {
                        p.seen_fall = Some(data.air.seq);
                        p.grasp_dir = Vec3::ZERO;
                        // InAir sub-state 3 (`HumanInAir__EnterReceptionState` 0xE00EF0), entered by walking / running off
                        // an edge (0xD8ADB0) and by letting go of a hang (0xDD08F0), both TransitionSetupDataToInAir
                        // +132 = 3: no entry clip. The playing item is left through its own exit (sub_5045F0 → 0x726F40;
                        // the locomotion and hang waits author none) and the sub-state lasts its remaining length
                        // (sub_502570); then the main fall plays the falling blend. The port lets the full-body item
                        // run to its end and hold its last frame.
                        p.fall_entry = !after_flight && p.clip.is_some() && !p.items.is_empty();
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
                        // while the grab input (empty hand, pad button 3) is held.
                        // PORT: native timer/ability gates and overlay exit scheduling (0xE0C301)
                        // remain simplified; zero weights reset grasp immediately on release.
                        let facing = body.forward();
                        let want = if (if crate::tuning::GAME_FALLS { pad.hand_held } else { pad.legs_held }) && data.air.apex_reached && data.air.apex_y - body.feet.y > 0.3 { if pad.magnitude > 0.35 { pad.dir } else { facing } } else { Vec3::ZERO };
                        let k = (dt * 7.0).min(1.0);
                        let gd = p.grasp_dir;
                        p.grasp_dir = if crate::tuning::GAME_FALLS && want == Vec3::ZERO { Vec3::ZERO } else { gd + (want - gd) * k };
                        let w = fall_grasp_weights(p.grasp_dir, facing);
                        p.set_overlay_weights(&w);
                        continue;
                    } else {
                        // no body-part data (or no entry clip): the falling blend on the full body
                        let facing = body.forward();
                        let want = if (if crate::tuning::GAME_FALLS { pad.hand_held } else { pad.legs_held }) && data.air.apex_reached && data.air.apex_y - body.feet.y > 0.3 { if pad.magnitude > 0.35 { pad.dir } else { facing } } else { Vec3::ZERO };
                        let k = (dt * 7.0).min(1.0);
                        let gd = p.grasp_dir;
                        p.grasp_dir = if crate::tuning::GAME_FALLS && want == Vec3::ZERO { Vec3::ZERO } else { gd + (want - gd) * k };
                        let w = fall_grasp_weights(p.grasp_dir, facing);
                        // the free-step abort's fall blends in over 0.3 s (`sub_E009F0`, RE/24 AIR-10)
                        let fade = if data.air.free_step_abort { 0.3 } else if after_flight { 0.5 } else { 0.15 };
                        let r = action(&lib, &[ACT_FALL], true, 0, Some(fade), Some("fall"));
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
            // kiosk (0xE3E790 / 0xE3DD40): the table's action for the stage, at the sim's time
            ActorContextId::Kiosk if data.kiosk.action.is_some_and(|b| sim_item(&lib, &b).is_some()) => {
                let b = data.kiosk.action.unwrap();
                p.sim_phase = Some((data.kiosk.t / b.duration().max(1e-4)).min(1.0));
                sim_request(&mut p, &lib, &b, 5_500_000 + data.kiosk.seq as u64, 0.13)
            }
            // haystack (0xE43140): entry action, then the wait, at the sim's time
            ActorContextId::HayStack if data.hay.action.is_some_and(|b| sim_item(&lib, &b).is_some()) => {
                let b = data.hay.action.unwrap();
                let ph = data.hay.t / b.duration().max(1e-4);
                p.sim_phase = Some(if data.hay.phase == crate::player::hay::HayPhase::Waiting { ph.fract() } else { ph.min(1.0) });
                sim_request(&mut p, &lib, &b, 5_000_000 + data.hay.seq as u64, 0.2)
            }
            // corner turn / ledge jump / swing back (`ledge_moves`): its current action at the sim's phase
            // ladder (0xE27D30): the table's action for the state, at the sim's phase
            ActorContextId::Ladder if data.ladder.current().is_some_and(|(b, _)| sim_item(&lib, &b).is_some()) => {
                let (b, ph) = data.ladder.current().unwrap();
                p.sim_phase = Some(ph);
                sim_request(&mut p, &lib, &b, 7_700_000 + data.ladder.seq as u64, data.ladder.blend_time)
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
                        let id = l.catch_action.unwrap_or(if wall { ACT_CATCH_WALL[hi] } else { ACT_CATCH_FREE[hi] });
                        let mut r = action(&lib, &[id], false, token, None, Some(if wall { "catch_wall" } else { "catch_free" }));
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
                        let k = shimmy_index(!a.ends_with("left"), l.alt_flag);
                        action(&lib, &[ACT_SHIMMY[wi][k]], false, token, Some(0.08), None)
                    }
                    "hand step up" | "hand step down" if moving || l.pending_vstep.is_some() => {
                        let (first_left, second) = l.vstep.unwrap_or((true, false));
                        let down = l.last_action == "hand step down";
                        let k = vstep_index(down, second, first_left);
                        // a pending second hand holds the first half's last pose (0xDD3A2B plays nothing meanwhile)
                        let r = action(&lib, &[ACT_VSTEP[wi][k]], false, token, Some(0.1), None);
                        if l.pending_vstep.is_some() { r.map(|r| Request { hold: true, ..r }) } else { r }
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
                    // the free idle after the swing back, a walling free hang or the climb's jump up (Entry end 0xDE1FE0,
                    // HangFreeReceptionType 3), until the next move. The graph plays its transition action first, the swing
                    // `ACT_HANGFREE_SWINGBACK` (min / max), and the weights go [1 − SwingStrength, SwingStrength] to its
                    // playing first item (`sub_502E30`, the length the simulation holds, `ledge::swing_lock_time`) and
                    // [1, 0] to the idle behind it (0x503040; RE/24 LEDGE-24). With no transition action the idle takes
                    // the first set itself.
                    _ if !wall && l.swing_idle().is_some() => {
                        let s = l.swing_idle().unwrap_or(0.0).clamp(0.0, 1.0);
                        action(&lib, &[ACT_HANG_FREE], true, 0, Some(0.15), Some("hang_free")).map(|mut r| {
                            if let Some(it) = r.items.first_mut().filter(|it| it.layers.len() == 2) {
                                let mut w = [1.0 - s, s];
                                crate::player::jump_blend::normalize_item_weights(&mut w);
                                it.layers[0].1 = w[0];
                                it.layers[1].1 = w[1];
                            }
                            r.playing_w = Some((vec![1.0 - s, s], vec![1.0, 0.0]));
                            r
                        })
                    }
                    // free hang with a wall under it = LedgeHangType WallFree: `xx_h_hangwallfree_wait` (0x0106F2E9,
                    // played after the straight jump's wall-free reception, 0xE07D00 → 0xE09055; the wait update
                    // 0xDE1FE0 plays it for type 2), legs held clear of the wall
                    _ if !wall && crate::player::ledge::wall_below_hands((l.hand_l + l.hand_r) * 0.5, l.normal, &collision) => {
                        action(&lib, &[ACT_HANG_WALLFREE], true, 0, Some(0.15), Some("hang_free"))
                    }
                    // `PlayIdleOrBlocked` 0xDD3A10 (wall hang, no pending step): the stick held with no move possible plays
                    // the blocked action of its direction (RE/24 LEDGE-20); the free hang plays nothing for it
                    a if wall && matches!(a, "blocked up" | "blocked down" | "end of ledge" | "shimmy blocked (obstacle / inner corner)") => {
                        let id = match a {
                            "blocked up" => ACT_LEDGE_BLOCKED[0],
                            "blocked down" => ACT_LEDGE_BLOCKED[1],
                            _ if l.blocked_right => ACT_LEDGE_BLOCKED[3],
                            _ => ACT_LEDGE_BLOCKED[2],
                        };
                        action(&lib, &[id], true, 0, Some(0.15), None)
                    }
                    // the wall hang's idle blends in over 0.5 s (0xDD3A10: 0x0106F2E8, 0x3F000000)
                    _ => action(&lib, &[if wall { ACT_HANG_WALL } else { ACT_HANG_FREE }], true, 0, Some(if wall { 0.5 } else { 0.15 }), Some(if wall { "hang_wall" } else { "hang_free" })),
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
        // debug: the game never leaves a locked item (word 0x20) for a new action before it ends (RE/13 §7.6)
        if std::env::var_os("AC_ANIM_LOG").is_some() {
            if let Some(it) = p.items.get(p.item).filter(|it| it.word & 0x20 != 0 && p.clip.is_some()) {
                if p.phase < 0.98 {
                    info!("anim left locked {:#010x} at phase {:.2} for {} ({:?})", it.action, p.phase, req.key, loco.current);
                }
            }
        }
        // the game's transition into the requested action (sub_725950); named clips keep the port's fade
        let mut items = req.items;
        let new_action = items.first().map(|i| i.action).unwrap_or(0);
        let cur_item = p.items.get(p.item.min(p.items.len().max(1) - 1)).filter(|_| p.clip.is_some()).cloned();
        let mut loop_from = 0;
        let mut sim_from = 0;
        // the hand-over from a ground transition action (`ground_tree`) to its destination uses the transition's blend B
        // (dest 0 = whichever wait); any other request drops it
        let pending = if req.blend.is_some() {
            None
        } else {
            p.pending_b.take().filter(|(d, _)| *d == new_action || (*d == 0 && (ACT_WAIT.iter().flatten().any(|w| *w == new_action) || crate::player::ground_tree::WAITS_PARALLEL.contains(&new_action))))
        };
        let blend = if let Some(b) = req.blend {
            b
        } else if let Some((_, b)) = pending {
            b
        } else if new_action != 0 {
            match game_transition(&lib, cur_item.as_ref(), new_action) {
                Some(t) => {
                    // a transition action plays first, blended in with blend A; the requested action follows it with
                    // blend B (the slot queues both, SetAction 0x727F70). Also in front of a standing loop the simulation
                    // times (the beam / ladder / pilotis waits, Action +28 = 0): the transition plays on its own clock
                    // without root motion, then the simulation's phase takes over.
                    // Not in front of the ground locomotion: there the simulation plays the transition action itself
                    // (`ground_tree`, MoveBlend's transition path keeps the speed and the foot in step with it) and hands
                    // over with the transition's blend B above; one it does not model is replaced by the default
                    // transition (PORT, below). Nor in front of the simulation's one-shot moves, whose timing the body
                    // follows.
                    let standing_loop = req.sim && !req.looping && lib.graph.actions.get(&new_action).is_some_and(|a| a.repeat == 0);
                    let queued = !req.sim || standing_loop;
                    let skipped = t.action_a != 0 && !queued;
                    if t.action_a != 0 && queued {
                        if let Some(mut ti) = lib.action_items(t.action_a) {
                            if let Some((playing, queued)) = req.playing_w.as_ref() {
                                let set = |it: &mut ItemPlay, w: &[f32]| {
                                    if it.layers.len() == w.len() {
                                        let mut w = w.to_vec();
                                        crate::player::jump_blend::normalize_item_weights(&mut w);
                                        for (l, w) in it.layers.iter_mut().zip(w) {
                                            l.1 = w;
                                        }
                                    }
                                };
                                if let Some(it) = ti.first_mut() {
                                    set(it, playing);
                                }
                                if let Some(it) = items.first_mut() {
                                    set(it, queued);
                                }
                            }
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
                    if skipped && std::env::var_os("AC_ANIM_LOG").is_some() {
                        info!("anim skipped transition {:#010x} in front of {:#010x} ({:?})", t.action_a, new_action, loco.current);
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
        p.blend_from = cur_item.as_ref().map_or(0, |i| i.action);
        start_blend(&mut p, blend, had, Some(a_state), &lib);
        p.action_switch = true;
        // locomotion cycles all start on the left foot: keep the phase between gaits
        let cyclic = |n: &str| matches!(n, "walk" | "jog" | "run" | "sprint");
        let prev_cyclic = p.last_cyclic;
        p.last_cyclic = cyclic(&req.key);
        if !(cyclic(&req.key) && prev_cyclic) {
            p.phase = b_start_phase(&p, &lib, &items);
        }
        if std::env::var_os("AC_ANIM_LOG").is_some() {
            let layers: Vec<String> = items.iter().map(|i| i.layers.iter().map(|(n, w)| format!("{n}*{w:.2}")).collect::<Vec<_>>().join("+")).collect();
            info!("anim t={:.2} {:?} -> {} [{}] blend {} {:.2}s pending {:x?}", time.elapsed_secs(), loco.current, req.key, layers.join(" | "), p.blend.kind, if p.prev.is_some() { p.fade_time } else { 0.0 }, p.pending_b.map(|x| x.0));
        }
        p.clip = Some(req.key);
        p.items = items;
        p.item = 0;
        p.loop_from = loop_from;
        p.loop_item = lib.graph.actions.get(&new_action).map_or(0, |a| a.loop_item as usize);
        p.sim_from = sim_from;
        p.looping = req.looping;
        p.fit = req.fit;
        p.token = req.token;
        p.hold = req.hold.then_some(loco.current);
    }
}

impl AnimPlayer {
    /// The action slot 0 reports (`AnimSlotInstance__GetSelectedAction` 0x723D10): none while a transition between
    /// two actions runs. (hypothesis): every action-to-action blend inserts the ActionControllerEvaluator.
    pub fn selected_action(&self) -> Option<u32> {
        if self.action_switch && self.prev.is_some() && self.fade < 1.0 {
            return None;
        }
        self.items.get(self.item).map(|i| i.action)
    }
}

/// Start blend `b` from what is on screen (A) into what follows (B) (`sub_773F90`). NONE, or nothing shown before,
/// is a cut. A keeps playing for the AROLLB* types and is frozen otherwise; the length is the blend time, at most
/// what is left of A with the clamp flag.
fn start_blend(p: &mut AnimPlayer, b: ActBlend, had: bool, a_state: Option<(Vec<ItemPlay>, usize, f32, bool)>, lib: &AnimLibrary) {
    // a blend still running when this one starts (its own A and B are on screen together)
    let was_fading = p.fade < 1.0 && p.prev.is_some();
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
            // what is left of the whole action A, its later items included (an ActionEvaluator's vfunc 14 `sub_7723D0`:
            // the action's duration less its time, 0x774032 / 0x774054; RE/24 ANIM-9)
            if let Some(it) = items.get(*item) {
                let rest: f32 = items.iter().skip(*item + 1).map(|it| lib.item_duration(it)).sum();
                time = time.min((1.0 - phase) * lib.item_duration(it) + rest);
            }
        }
    }
    p.fade_time = time.max(0.01);
    // A keeps playing for the AROLLB* types. PORT: when the switch comes while another blend is still fading, the game's
    // A is that whole blend node (both its children keep running); the port has one level, so it freezes the pose on
    // screen instead of rolling only the newest item (which snapped the half-blended pose onto that item)
    if matches!(b.kind, 1 | 3) && !was_fading {
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

/// Weighted pose of a layer set at normalised phase `phase`: per bone (rotation, translation). The item evaluator
/// (`sub_777AE0` 0x777DD3–0x777EF8) blends the active clips pairwise in index order, clip i in with the factor
/// wᵢ / Σⱼ≤ᵢ wⱼ (rotations by `qinterp`); clips of weight ≤ 0.0005 are skipped (0x777B5C) (RE/24 ANIM-15 / ANIM-9).
fn sample_layers(lib: &AnimLibrary, skel: &PoseSkeleton, layers: &[(String, f32)], phase: f32, out: &mut Vec<(Quat, Vec3)>) {
    out.clear();
    out.extend(skel.rest.iter().map(|r| (r.rotation, r.translation)));
    let mut total = 0.0f32;
    for (name, w) in layers {
        if *w <= 0.0005 {
            continue;
        }
        let Some(c) = lib.clips.get(name) else { continue };
        let t = phase * c.duration;
        total += *w;
        let f = *w / total;
        for (i, bone) in skel.bone_ids.iter().enumerate() {
            let r = c.rotations.get(bone).map(|k| sample_rot(k, t)).unwrap_or(skel.rest[i].rotation);
            let p = c.translations.get(bone).map(|k| sample_vec(k, t)).unwrap_or(skel.rest[i].translation);
            let acc = &mut out[i];
            if f >= 1.0 {
                *acc = (r, p);
            } else {
                *acc = (qinterp(acc.0, r, f), acc.1.lerp(p, f));
            }
        }
    }
}

/// The animation step: advances each player's actions and samples the pose into its `BonePose` (the renderer copies
/// it in `apply_clip`; the simulation reads it the next frame, `bone_pose`).
pub fn animate_pose(
    time: Res<Time>,
    lib: Res<AnimLibrary>,
    skel: Res<PoseSkeleton>,
    mut q: Query<(&mut AnimPlayer, &crate::player::Body, &mut BonePose)>,
    mut cur: Local<Vec<(Quat, Vec3)>>,
) {
    let dt = time.delta_secs();
    if skel.is_empty() {
        return;
    }
    let skel = &*skel;
    for (mut p, body, mut bone) in &mut q {
        if p.clip.is_none() || p.items.is_empty() {
            continue;
        }
        let item = p.items[p.item.min(p.items.len() - 1)].clone();
        p.action_flags = if item.action == 0 { 8 } else { lib.graph.actions.get(&item.action).map_or(8, |a| a.flags) };
        let duration = lib.item_duration(&item).max(1e-3);
        let root_speed = item.layers.first().and_then(|(n, _)| lib.clips.get(n)).map(|c| c.root_speed).unwrap_or(0.0);
        // playback rate (PORT: the named fallback cycles only, RE/24 ANIM-18): locomotion cycles match their root motion to the actual speed (no foot
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
            let from = p.item;
            // a looping action starts again at its Action +32 item (`ActionEvaluator__Advance` 0x730E50 at 0x730F3F; RE/24
            // ANIM-5), counted from the requested action's first item
            p.item = if p.item + 1 < p.items.len() { p.item + 1 } else { (p.loop_from + p.loop_item).min(p.items.len() - 1) };
            let nxt = &p.items[p.item];
            let nxt_switch = nxt.action != cur.action && nxt.action != 0;
            // the requested action queued behind a transition action enters with the transition's blend B, stored on
            // its first item (`AnimSlot__SetAction` 0x727F70 pushes the destination with blend B, 0x7286CA; it is not
            // looked up again; RE/24 ANIM-2)
            let queued_seam = queued_seam(p.loop_from, from, p.item);
            let b = if nxt_switch && !queued_seam {
                game_transition(&lib, Some(&cur), nxt.action).map(|t| t.blend_a).unwrap_or_default()
            } else {
                nxt.blend
            };
            p.blend_from = cur.action;
            start_blend(&mut p, b, true, Some(a_state), &lib);
            p.action_switch = nxt_switch;
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
            // a looping A goes on to its next item. The simulation feeds a slot only its current item (the ground
            // locomotion's foot cycle): the action's next item follows with the same weights (they live on the slot,
            // `sub_725B30`), instead of the item restarting on itself (footr's end is footl's start)
            if *alooping && n >= 1.0 {
                if items.len() > 1 {
                    *ai = (*ai + 1) % items.len();
                } else if let Some(next) = items.first().and_then(|it| next_loop_item(&lib, it)) {
                    items[0] = next;
                }
            }
        }
        p.fade = (p.fade + dt / p.fade_time.max(0.01)).min(1.0);
        let weight = blend_weight(&p.blend, p.fade);
        let item = p.items[p.item].clone();

        // contact tags of the dominant clip (for the limb IK); ACTBlendAcuatorMode FROMA keeps A's while blending
        p.contacts = lib
            .dominant(&item)
            .map(|c| {
                let mut s = contact_state(c, p.phase * c.duration);
                let later = p.items.iter().skip(p.item + 1).take_while(|it| item.action != 0 && it.action == item.action).filter_map(|it| lib.dominant(it));
                extend_next_on(&mut s, c, later);
                s
            })
            .unwrap_or_default();

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
                    sample_layers(&lib, skel, &it.layers, it.clip_phase(*aph), &mut a_pose);
                    p.prev = Some(a_pose);
                }
            }
        }
        {
            let root = &mut bone.root;
            *root = anim_root();
            if !p.looping && item.root_motion {
                let mut off = Vec3::ZERO;
                let mut wsum = 0.0;
                for (n, w) in &item.layers {
                    if let Some(c) = lib.clips.get(n) {
                        off += root_motion_offset(c, item.clip_phase(p.phase)) * *w;
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
        sample_layers(&lib, skel, &item.layers, item.clip_phase(p.phase), &mut cur);
        let prev = if fading { p.prev.take() } else { None };
        let empty = Vec::new();
        let prev_pose = prev.as_ref().filter(|v| v.len() == cur.len()).unwrap_or(&empty);
        let mut shown = std::mem::take(&mut p.last_pose);
        let mut base: Vec<(Quat, Vec3)> = (0..skel.bone_ids.len())
            .map(|i| {
                let (rot, pos) = cur[i];
                match prev_pose.get(i) {
                    Some(&(r0, p0)) => (qinterp(r0, rot, weight), p0.lerp(pos, weight)),
                    None => (rot, pos),
                }
            })
            .collect();
        if let Some(o) = p.overlay.as_mut() {
            apply_overlay(o, &lib, skel, &mut base, dt);
        }
        if p.overlay.as_ref().is_some_and(|o| o.ending && o.weight <= 0.0) {
            p.overlay = None;
        }
        // debug: `AC_POP_LOG=<degrees>` reports a frame where a joint turns more than that (a pop at a switch)
        if let Some(th) = std::env::var("AC_POP_LOG").ok().and_then(|v| v.parse::<f32>().ok()) {
            let worst = shown.iter().zip(&base).enumerate().map(|(i, (a, b))| (i, a.0.angle_between(b.0).to_degrees())).fold((0, 0.0f32), |m, x| if x.1 > m.1 { x } else { m });
            if shown.len() == base.len() && worst.1 > th {
                info!("pop {:.1} deg joint {} (bone {:08x}) clip {:?} item {} phase {:.3} fade {:.2}", worst.1, worst.0, skel.bone_ids[worst.0], p.clip, p.item, p.phase, p.fade);
            }
        }
        shown.clear();
        for (i, &(rot, pos)) in base.iter().enumerate() {
            shown.push((rot, pos));
            if !(rot.is_finite() && pos.is_finite()) && std::env::var_os("AC_NAN_LOG").is_some() {
                warn!("non-finite joint {i} (bone {:08x}) clip {:?} item {} phase {:.3} fade {:.3} layers {:?}", skel.bone_ids[i], p.clip, p.item, p.phase, p.fade, item.layers);
            }
        }
        bone.locals.clear();
        bone.locals.extend_from_slice(&base);
        bone.compute_model(skel);
        bone.seq += 1;
        p.last_pose = shown;
        p.prev = if p.fade >= 1.0 { None } else { prev };
    }
}

/// The render step: copies the animation step's pose onto the rig's joints (IK, the bone modifiers and the sounds run
/// after it).
pub fn apply_clip(q: Query<(&Rig, &BonePose)>, mut joints: Query<&mut Transform>) {
    for (rig, bone) in &q {
        if bone.seq == 0 {
            continue;
        }
        if let Ok(mut root) = joints.get_mut(rig.root) {
            *root = bone.root;
        }
        for (e, &(rot, pos)) in rig.joints.iter().zip(&bone.locals) {
            if let Ok(mut tr) = joints.get_mut(*e) {
                tr.rotation = rot;
                tr.translation = pos;
            }
        }
    }
}

fn load_library(mut lib: ResMut<AnimLibrary>) {
    if std::env::var_os("AC_NO_MODEL").is_some() {
        return;
    }
    match library_from_install() {
        Ok(l) => {
            *lib = l;
            info!("{}", lib.status);
        }
        Err(e) => {
            lib.status = format!("anims: not loaded ({e})");
            warn!("{}", lib.status);
        }
    }
}

/// The clips, the graph and the body parts from the install.
fn library_from_install() -> Result<AnimLibrary, String> {
    let mut lib = AnimLibrary::default();
    match load_locomotion(&game_dir()) {
        Ok((raw, graph, names)) => {
            let mut clips = HashMap::new();
            for c in raw {
                let rotations = c.rotations.into_iter().map(|(k, v)| (k, v.into_iter().map(|(t, q)| (t, Quat::from_array(q).normalize())).collect())).collect();
                let translations: HashMap<u32, Vec<(f32, Vec3)>> =
                    c.translations.into_iter().map(|(k, v)| (k, v.into_iter().map(|(t, p)| (t, Vec3::from_array(p))).collect())).collect();
                let root_speed = translations
                    .get(&crate::assets::ac_anim::TRACK_DISPLACEMENT)
                    .and_then(|k| Some((k.first()?.1, k.last()?.1)))
                    .map(|(a, b)| (b - a).length() / c.duration.max(1e-3))
                    .unwrap_or(0.0);
                clips.insert(c.name.clone(), AnimClip { name: c.name, duration: c.duration, rotations, translations, root_speed, contacts: c.contacts, events: c.events });
            }
            lib.status = format!("anims: {} clips, {} graph actions from your install", clips.len(), graph.actions.len());
            lib.clips = Arc::new(clips);
            lib.graph = Arc::new(graph);
            lib.anim_names = Arc::new(names);
            match crate::assets::body_parts::load_body_parts(&game_dir()) {
                Ok(b) => lib.body = b,
                Err(e) => warn!("body parts not loaded: {e}"),
            }
            Ok(lib)
        }
        Err(e) => Err(e),
    }
}

/// One library per process for the runs without the renderer (tests, replays): loaded on first use, then cloned
/// (its tables are shared). None without the install.
#[cfg_attr(not(test), allow(dead_code))] // the headless runs (tests, replays) only, so far
pub fn shared_library() -> Option<AnimLibrary> {
    static LIB: std::sync::OnceLock<Option<AnimLibrary>> = std::sync::OnceLock::new();
    LIB.get_or_init(|| library_from_install().ok()).clone()
}

/// The clip choice and the animation step, for a schedule without the renderer: the player's `BonePose` follows its
/// actions as in the game (the rig writer `apply_clip` and IK are the renderer's).
#[cfg_attr(not(test), allow(dead_code))] // the headless runs (tests, replays) only, so far
pub fn headless_animation_systems() -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (choose_clip, animate_pose).chain().into_configs()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RE/24 LEDGE-6: the game's vertical step tables (0xDCC3E5..0xDCC403, free hang 0x01A279B9..C0). Down: left first
    /// plays FAF / 79BE (`1m_d_1ru`), right first FAE / 79BD; the second hand after a left-first step FB1 / 79C0, after a
    /// right-first step FB0 / 79BF. Up: left first FAA, right first FAB; the second hand after a left-first step FAC
    /// (`1lu_u_1m`), after a right-first step FAD.
    #[test]
    fn a_repeated_item_runs_its_clips_that_many_times() {
        // RE/24 ANIM-6: a repeated item lasts its clips' length times its repeat count and runs its clips that many times
        let mut lib = AnimLibrary::default();
        Arc::make_mut(&mut lib.clips).insert("c".into(), AnimClip { name: "c".into(), duration: 0.5, ..Default::default() });
        let it = ItemPlay { layers: vec![("c".into(), 1.0)], repeat: 4, ..Default::default() };
        assert!((lib.item_duration(&it) - 2.0).abs() < 1e-6);
        assert!((it.clip_phase(0.3) - 0.2).abs() < 1e-5, "the second run, 0.2 in");
        assert_eq!(it.clip_phase(1.0), 1.0);
        let once = ItemPlay { repeat: 0, ..it.clone() };
        assert!((lib.item_duration(&once) - 0.5).abs() < 1e-6 && once.clip_phase(0.3) == 0.3);
    }

    #[test]
    fn vertical_hand_steps_pick_the_games_clips() {
        let free = |down, second, first_left| ACT_VSTEP[1][vstep_index(down, second, first_left)];
        assert_eq!(free(true, false, true), 0x01A2_79BE);
        assert_eq!(free(true, false, false), 0x01A2_79BD);
        assert_eq!(free(true, true, true), 0x01A2_79C0);
        assert_eq!(free(true, true, false), 0x01A2_79BF);
        assert_eq!(free(false, false, true), 0x01A2_79B9);
        assert_eq!(free(false, false, false), 0x01A2_79BA);
        assert_eq!(free(false, true, true), 0x01A2_79BB);
        assert_eq!(free(false, true, false), 0x01A2_79BC);
    }

    #[test]
    fn selected_action_is_none_through_an_action_transition_only() {
        let mut p = AnimPlayer { items: vec![ItemPlay { action: 7, ..default() }], ..default() };
        assert_eq!(p.selected_action(), Some(7));
        p.prev = Some(Vec::new());
        p.fade = 0.5;
        // an item seam inside one action keeps the action evaluator selected
        assert_eq!(p.selected_action(), Some(7));
        p.action_switch = true;
        assert_eq!(p.selected_action(), None);
        p.fade = 1.0;
        assert_eq!(p.selected_action(), Some(7));
    }

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
        let lib = AnimLibrary { graph: Arc::new(graph), ..Default::default() };
        let item = |action: u32| {
            let it = &lib.graph.actions[&action].items[0];
            ItemPlay { layers: Vec::new(), blend: it.blend, root_motion: false, action, transitions: it.transitions.clone(), word: it.flags, repeat: it.repeat }
        };
        // 1. the playing item's transition to the requested action: high wait (0x00D8258E) → fight wait (0x00FD645B)
        //    goes through `xx_h_wait_hipm_footr_tr_xx_h_light_wait_footr` (0x1D504306)
        let t = game_transition(&lib, Some(&item(0x00D8_258E)), 0x00FD_645B).expect("item transition");
        assert_eq!(t.action_a, 0x1D50_4306);
        // 3. the run stop's own in-transition (0x00D837AE): A and B both play for 0.2 s, root motion from B only
        let t = game_transition(&lib, Some(&item(0x00D8_258E)), 0x00D8_37AE).expect("in-transition");
        assert_eq!((t.action_a, t.blend_a.kind, t.blend_a.disp_src), (0, 1, 2));
        assert!((t.blend_a.time - 0.2).abs() < 1e-6);
        // 4. nothing authored: a cut, unless one side plays a clip of Altaïr's remap set (RE/24 ANIM-3): the twelve
        //    `xx_turn_on_spot_*` clips, swapped for his `amaa_` ones
        assert_eq!(lib.graph.remap.len(), 12);
        assert!(lib.graph.remap.iter().all(|r| r.2.starts_with("xx_turn_on_spot_")));
        assert!(game_transition(&lib, Some(&item(0x00D8_258E)), 0x00D8_243F).is_none(), "a cut");
        let turning = ItemPlay { layers: vec![(lib.graph.remap[0].2.clone(), 1.0)], action: 0x00D8_258E, ..Default::default() };
        let t = game_transition(&lib, Some(&turning), 0x00D8_243F).expect("default");
        assert_eq!(t.blend_a, ActBlend::DEFAULT);
        // the repeat count: idles loop, the pull-up plays once
        assert_eq!(lib.graph.actions[&0x0106_F2E8].repeat, 0);
        assert_eq!(lib.graph.actions[&0x0106_D2C5].repeat, 1);
        // Action +32 (RE/24 ANIM-5): the impulsion wait loops from its item b; ActionItem +24 (ANIM-6): the debug flight's
        // Ghost items repeat 4 and 3 times, every other item once
        assert_eq!((lib.graph.actions[&0x0DF0_3385].repeat, lib.graph.actions[&0x0DF0_3385].loop_item), (0, 1));
        assert_eq!(lib.graph.actions[&0x0106_F2E8].loop_item, 0);
        let ghost: Vec<u32> = lib.graph.actions[&0x0006_F88B].items.iter().map(|it| it.repeat).collect();
        assert_eq!(ghost, [4, 3]);
        assert!(lib.graph.actions.values().filter(|a| a.id != 0x0006_F88B).flat_map(|a| &a.items).all(|it| it.repeat == 1));
        // the wall hang's blocked actions (0xDD3A10, RE/24 LEDGE-20) are in the graph
        for id in ACT_LEDGE_BLOCKED {
            assert!(lib.graph.actions.contains_key(&id), "blocked action {id:#x}");
        }
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
    fn the_queued_destination_keeps_the_transitions_blend_b() {
        // items [transition a, transition b, dest 0, dest 1], loop_from 2 (RE/24 ANIM-2)
        assert!(queued_seam(2, 1, 2), "transition → destination: blend B");
        assert!(!queued_seam(2, 0, 1), "inside the transition");
        assert!(!queued_seam(2, 3, 2), "the destination looping back");
        assert!(!queued_seam(0, 0, 1), "no queued transition");
    }

    #[test]
    fn the_next_contact_is_found_in_the_actions_later_items() {
        // a hand lets go in item a and grabs again 0.1 s into item b: the limb travels instead of releasing
        let a = AnimClip { duration: 0.5, contacts: vec![(0.0, 0b110000), (0.3, 0b100000)], ..default() };
        let b = AnimClip { duration: 0.6, contacts: vec![(0.0, 0b100000), (0.1, 0b110000)], ..default() };
        let mut s = contact_state(&a, 0.4);
        assert_eq!(s.next_on[4], None);
        extend_next_on(&mut s, &a, [&b].into_iter());
        assert!((s.next_on[4].unwrap() - 0.6).abs() < 1e-5, "{:?}", s.next_on[4]);
        // a bit that comes back in the current clip keeps that time
        assert_eq!(s.next_on[5], None, "bit 5 is on");
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

#[cfg(test)]
#[path = "anim_pose_tests.rs"]
mod pose_tests;
