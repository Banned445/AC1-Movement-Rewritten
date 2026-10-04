//! HumanWalling (context 11): the run up a wall (RE/05 §1).
//!
//! Entry: Movement event 49 (guard 0xDA54F0 → 0xDA2B10 → wall test 0xE18390), start 0xDA2C30. Sub-states
//! EntryA → EntryB → Vertical → VerticalEnd (`UpdateWallingSubState` 0xE37590); ledge probes A–D hand over to
//! the Ledge context with the game's `wallingfront_*_tr_*` exit clips; the rebound jump (0xE365C0) leaves to
//! InAir. Only the front (vertical) wall run exists in the game's data (no `wallingside` clips).

use bevy::prelude::*;

use super::air::{FallOrigin, InAirEntry};
use super::jump_blend::{self, ActionBlend};
use super::ledge::{hang_root, hang_root_at, LedgeEntry, LedgeHangType, LedgeSubState};
use super::ledge_moves::{self, LedgeMove, MoveKind};
use super::targets::JumpTarget;
use super::{right_of, switch_context, ActorContextId, Body, HumanDataBundle, Locomotion, Player, TransitionSetup};
use crate::collision::CollisionWorld;
use crate::guidance::{GuidanceSubType, GuidanceWorld};
use crate::input::PadInput;
use crate::tuning::PULLUP_IN;

/// HumanWalling block actions (matched by clip name; the exe requests some through runtime ids 68 / 69).
pub const ENTRY_A: u32 = 0x00D8_2C5C; // xx_h_wallingfront_entry_footl_a (id 68, 0xDA2C30)
pub const ENTRY_B: u32 = 0x0100_4122; // xx_h_wallingfront_entry_footl_b
pub const STEP1: u32 = 0x00D8_2C9F; // xx_h_wallingfront_step1_footr (id 69, SubState 3 Vertical)
pub const STEP1_TO_FALL: u32 = 0x00D8_2CE2;
pub const VERTICAL_END: u32 = 0x0121_847A; // step1_footr_tr_rebound a, b (SubState 6)
pub const ENTRY_TO_FALL: u32 = 0x00FF_7CA1;
pub const ENTRY_TO_KNEE: u32 = 0x0100_759B; // entry_footl_tr_hangknee_{131,200}cm (probe A pull-up)
pub const ENTRY_TO_PASSOVER: u32 = 0x0109_B6CF;
pub const ENTRY_TO_HANGFREE: u32 = 0x0564_BB1D; // 90487581, probe B (4-way)
pub const STEP1_TO_KNEE: u32 = 0x0106_CFC2; // probe D pull-up, 201 / 250 cm
pub const STEP1_TO_HANGWALL: u32 = 0x0106_CFC5; // probe D hang, 251 / 430 cm × 0 / 50 cm
pub const STEP1_TO_HANGFREE: u32 = 0x0564_BB1E; // 90487582, probe C
pub const STEP1_TO_PASSOVER: u32 = 0x0109_BB50;
pub const ENTRY_TO_REBOUND: u32 = 0x0115_7E35;
pub const STEP1_TO_REBOUND: u32 = 0x0115_7E36;

pub const DUMPED_ACTIONS: &[u32] = &[
    ENTRY_A, ENTRY_B, STEP1, STEP1_TO_FALL, VERTICAL_END, ENTRY_TO_FALL, ENTRY_TO_KNEE, ENTRY_TO_PASSOVER, ENTRY_TO_HANGFREE,
    STEP1_TO_KNEE, STEP1_TO_HANGWALL, STEP1_TO_HANGFREE, STEP1_TO_PASSOVER, ENTRY_TO_REBOUND, STEP1_TO_REBOUND,
];

/// entity+0x7C: the reference length of every walling band. 1.0 for Altaïr: the entry clip lifts the root
/// exactly 1.0 m, which is the warp target's height (wall hit at 1.3·h minus 0.3·h, 0xE18390).
pub const H: f32 = 1.0;

/// HumanWallingData::SubState (RE/05 §1.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WallingSubState {
    #[default]
    EntryA = 1,
    EntryB = 2,
    Vertical = 3,
    VerticalEnd = 6,
}

#[derive(Clone, Copy, Debug)]
pub struct WallingEntry {
    /// Warp target of the entry (0xE18390 output): 0.5·h out from the wall, h above the feet.
    pub contact: Vec3,
    /// Outward wall normal.
    pub normal: Vec3,
    pub from: Vec3,
}

#[derive(Debug, Default)]
pub struct HumanWallingData {
    pub sub_state: WallingSubState,
    pub normal: Vec3,
    pub action: Option<ActionBlend>,
    /// Second item of a two-item action (VerticalEnd a → b).
    pub action_next: Option<ActionBlend>,
    pub t: f32,
    pub from: Vec3,
    /// Where the root must be when the action ends (motion warp, `sub_711130`).
    pub to: Vec3,
    pub seq: u32,
    /// HumanWallingData+0x14.
    pub start_height: f32,
}

fn single(id: u32, item: usize) -> Option<ActionBlend> {
    let n = jump_blend::action_items(id)?.get(item)?.len();
    let mut w = vec![0.0; n.max(1)];
    w[0] = 1.0;
    Some(ActionBlend::new(id, item, &w))
}

fn blended(id: u32, item: usize, w: &[f32]) -> Option<ActionBlend> {
    let n = jump_blend::action_items(id)?.get(item)?.len();
    let mut v = vec![0.0; n];
    for (k, x) in w.iter().enumerate().take(n) {
        v[k] = *x;
    }
    Some(ActionBlend::new(id, item, &v))
}

impl HumanWallingData {
    /// `HumanGround` 0xDA2C30: SubState EntryA, WallingType Vertical, action 68 with the root warped to the
    /// contact over the action's length.
    pub fn enter(&mut self, e: WallingEntry) {
        self.normal = e.normal;
        self.start_height = e.from.y;
        self.play(WallingSubState::EntryA, single(ENTRY_A, 0), None, e.from, Some(e.contact));
    }

    fn play(&mut self, sub: WallingSubState, a: Option<ActionBlend>, next: Option<ActionBlend>, from: Vec3, to: Option<Vec3>) {
        self.sub_state = sub;
        self.action = a;
        self.action_next = next;
        self.t = 0.0;
        self.from = from;
        let facing = self.facing();
        let disp = |a: &Option<ActionBlend>| a.map(|a| a.disp(1.0)).unwrap_or([0.0; 3]);
        let (da, db) = (disp(&a), disp(&next));
        let d = [da[0] + db[0], da[1] + db[1], da[2] + db[2]];
        self.to = to.unwrap_or(from + right_of(facing) * d[0] + facing * d[1] + Vec3::Y * d[2]);
        self.seq = self.seq.wrapping_add(1);
    }

    pub fn facing(&self) -> Vec3 {
        -Vec3::new(self.normal.x, 0.0, self.normal.z).normalize_or_zero()
    }

    fn duration(&self) -> f32 {
        self.action.map(|a| a.duration()).unwrap_or(0.1) + self.action_next.map(|a| a.duration()).unwrap_or(0.0)
    }

    /// Root at time t: the actions' displacement plus a linear correction onto `to` (FROMANIM + warp).
    fn root_at(&self, t: f32) -> Vec3 {
        let f = self.facing();
        let w = |d: [f32; 3]| right_of(f) * d[0] + f * d[1] + Vec3::Y * d[2];
        let d1 = self.action.map(|a| a.duration()).unwrap_or(0.1);
        let total = self.duration();
        let disp_at = |t: f32| -> Vec3 {
            let a = self.action.map(|a| w(a.disp((t / d1).min(1.0)))).unwrap_or(Vec3::ZERO);
            let b = match self.action_next {
                Some(n) if t > d1 => w(n.disp(((t - d1) / (total - d1).max(1e-4)).min(1.0))),
                _ => Vec3::ZERO,
            };
            a + b
        };
        let end = self.from + disp_at(total);
        self.from + disp_at(t) + (self.to - end) * (t / total).min(1.0)
    }

    /// The action playing now and its phase (for the animator).
    pub fn current(&self) -> Option<(ActionBlend, f32)> {
        let d1 = self.action.map(|a| a.duration()).unwrap_or(0.1);
        match self.action_next {
            Some(n) if self.t > d1 => Some((n, ((self.t - d1) / n.duration().max(1e-4)).min(1.0))),
            _ => self.action.map(|a| (a, (self.t / d1.max(1e-4)).min(1.0))),
        }
    }
}

// ---------------------------------------------------------------- the wall ahead (0xE18390)

/// Outward normal of the axis-aligned greybox face at `p` (the game takes the collision hit's normal).
fn face_normal(p: Vec3, toward: Vec3, collision: &CollisionWorld) -> Vec3 {
    let mut best = -toward;
    let mut score = f32::MIN;
    for n in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
        if collision.point_inside(p - n * 0.03) && !collision.point_inside(p + n * 0.03) {
            let s = n.dot(-toward);
            if s > score {
                score = s;
                best = n;
            }
        }
    }
    best
}

/// `HumanWalling` entry test (event 49 guard 0xDA2B10 → 0xE18390): a ray from 1.3·h above the feet along the
/// facing, 1.5·h long; the wall must face back within 45° and extend about 0.5 m to both sides (0xE149E0); its
/// slope (0xE14590 / 0xE14FB0) must be within 45°. Returns the warp target (hit + 0.5·h out, feet + h)
/// and the outward normal.
pub fn wall_ahead(feet: Vec3, forward: Vec3, collision: &CollisionWorld) -> Option<(Vec3, Vec3)> {
    let f = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let o = feet + Vec3::Y * (1.3 * H);
    let d = collision.sphere_free_distance(o, f, 0.01, 1.5 * H);
    if d >= 1.5 * H - 1e-3 {
        return None;
    }
    let hit = o + f * d;
    let n = face_normal(hit, f, collision);
    if n.dot(-f) < 45f32.to_radians().cos() {
        return None;
    }
    let side = right_of(-n);
    let solid = |p: Vec3| collision.point_inside(p - n * 0.05);
    if !(solid(hit + side * 0.5) && solid(hit - side * 0.5)) {
        return None;
    }
    // 0xE14590 probes 1 m higher only to measure the wall's slope (a vertical greybox wall: 0°); it does not
    // reject a wall that ends lower.
    let contact = Vec3::new(hit.x, feet.y + H, hit.z) + n * (0.5 * H);
    Some((contact, n))
}

// ---------------------------------------------------------------- ledge probes (0xE36BD0)

struct LedgeCandidate {
    point: Vec3,
    normal: Vec3,
    edge_dir: Vec3,
    /// Height above the root.
    height: f32,
    /// Lateral offset from the probe origin.
    lateral: f32,
}

/// `FindLedgeCandidates` 0xE36BD0 (reduced): LedgeGrab edges facing the character within 45°, whose closest
/// point to the probe origin lies within ±`width` sideways, 0..`height` above it and within `range` ahead.
/// Nearest first.
fn find_ledge(g: &GuidanceWorld, origin: Vec3, root: Vec3, facing: Vec3, width: f32, height: f32, range: f32) -> Vec<LedgeCandidate> {
    let r = right_of(facing);
    let mut out: Vec<(f32, LedgeCandidate)> = Vec::new();
    for e in &g.edges {
        if e.subtype != GuidanceSubType::LedgeGrab || (-e.n1).dot(facing) < 45f32.to_radians().cos() {
            continue;
        }
        let q = e.closest_point(origin);
        let d = q - origin;
        let up = d.y;
        let ahead = d.dot(facing);
        let lat = d.dot(r);
        if up < 0.0 || up > height || lat.abs() > width || !(-0.1..=range).contains(&ahead) {
            continue;
        }
        out.push((ahead.abs() + up * 0.01, LedgeCandidate { point: q, normal: e.n1, edge_dir: (e.p1 - e.p0).normalize_or_zero(), height: q.y - root.y, lateral: lat }));
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    out.into_iter().map(|x| x.1).collect()
}

/// `ClassifyLedgeCandidate` 0xE341D0 (reduced): room to stand on top (≥ 0.4 m clear above, class 1).
fn room_on_top(c: &LedgeCandidate, collision: &CollisionWorld) -> bool {
    let top = c.point - c.normal * PULLUP_IN;
    collision.ground_height(top + Vec3::Y * 0.05, 0.3).is_some() && collision.capsule_fits(Vec3::new(top.x, c.point.y, top.z))
}

fn hands(c: &LedgeCandidate) -> (Vec3, Vec3) {
    let along = if c.edge_dir == Vec3::ZERO { right_of(-c.normal) } else { c.edge_dir };
    (c.point - along * 0.2, c.point + along * 0.2)
}

fn exit(seq: [Option<ActionBlend>; 4], from: Vec3, to: Vec3, c: &LedgeCandidate, end: (bool, bool, bool)) -> LedgeEntry {
    let (hl, hr) = hands(c);
    let facing = -Vec3::new(c.normal.x, 0.0, c.normal.z).normalize_or_zero();
    let durations = seq.map(|a| a.map(|a| a.duration()).unwrap_or(0.0));
    let mv = LedgeMove {
        kind: MoveKind::Arrival,
        seq,
        durations,
        t: 0.0,
        from,
        to,
        facing_from: facing,
        facing_to: facing,
        follow_disp: durations.iter().sum::<f32>() > 0.0,
        lead: 0.0,
        end_free: end.0,
        end_wall: end.1,
        end_stand: end.2,
        hand_l: hl,
        hand_r: hr,
        normal: c.normal,
    };
    let mut e = LedgeEntry::at(c.point, c.normal, from, LedgeSubState::HangWallReception);
    e.hand_l = hl;
    e.hand_r = hr;
    e.entry_move = Some(mv);
    e
}

/// Pull-up from the wall run: `…_tr_hangknee_{low,high}` blended by `b`, then hangknee → free-step entry (the
/// Ledge pull-up state's stand-up, 0xDE2EE0 lists both walling → hangknee actions), ending on top. The entry ends
/// on footl, step1 on footr.
fn pullup_exit(id: u32, b: f32, root: Vec3, c: &LedgeCandidate) -> LedgeEntry {
    let top = c.point - c.normal * PULLUP_IN;
    let stand = ledge_moves::ACT_KNEE_TO_FREESTEP[(id == STEP1_TO_KNEE) as usize];
    let seq = [blended(id, 0, &[1.0 - b, b]), single(stand, 0), None, None];
    exit(seq, root, Vec3::new(top.x, c.point.y, top.z), c, (false, false, true))
}

/// Probes after EntryB (UpdateWallingSubState 0xE37590, command 0): A = pull-up, B = free hang.
fn probe_entry(g: &GuidanceWorld, root: Vec3, facing: Vec3, collision: &CollisionWorld) -> Option<LedgeEntry> {
    // A: FindLedge(pos, 0.3h, 1.05h+0.05, h, 0.8): 0.25h ≤ height ≤ h, room on top
    for c in find_ledge(g, root, root, facing, 0.3 * H, 1.05 * H + 0.05, 0.8) {
        if (0.25 * H..=H).contains(&c.height) && room_on_top(&c, collision) {
            let b = ((c.height.max(0.3) - 0.3 * H) / (0.7 * H)).clamp(0.0, 1.0);
            return Some(pullup_exit(ENTRY_TO_KNEE, b, root, &c));
        }
    }
    // B: FindLedge(pos + 1.5·up − 0.6·fwd, 0.3h, h+0.05, …, 1.2): free hang, 4-way blend
    let o = root + Vec3::Y * 1.5 - facing * 0.6;
    if let Some(c) = find_ledge(g, o, root, facing, 0.3 * H, H + 0.05, 1.2).into_iter().next() {
        let hb = (c.height - 1.5).clamp(0.0, 1.0);
        let lat = (1.0 - (c.lateral.abs() - 0.1) / 0.7).clamp(0.0, 1.0);
        // clips: 250 min, 350 min, 250 max, 350 max (swing back)
        let w = [(1.0 - hb) * lat, hb * lat, (1.0 - hb) * (1.0 - lat), hb * (1.0 - lat)];
        let (hl, hr) = hands(&c);
        let to = hang_root_at(hl, hr, c.normal, LedgeHangType::Free, collision);
        return Some(exit([blended(ENTRY_TO_HANGFREE, 0, &w), None, None, None], root, to, &c, (true, false, false)));
    }
    None
}

/// Probes after the Vertical step: C = free hang (2h ≤ height < 2.7h+0.1), D = pull-up (0.5h–h) or wall hang
/// (h–2.7h+0.1).
fn probe_vertical(g: &GuidanceWorld, root: Vec3, facing: Vec3, collision: &CollisionWorld) -> Option<LedgeEntry> {
    let oc = root - facing * 0.6 + Vec3::Y;
    for c in find_ledge(g, oc, root, facing, 0.3 * H, 1.7 * H + 0.1, 1.2) {
        if c.lateral.abs() < 0.25 * H + 0.1 && (2.0 * H..2.7 * H + 0.1).contains(&c.height) {
            let hb = ((c.height - 2.0) / 0.7).clamp(0.0, 1.0);
            let (hl, hr) = hands(&c);
            let to = hang_root_at(hl, hr, c.normal, LedgeHangType::Free, collision);
            return Some(exit([blended(STEP1_TO_HANGFREE, 0, &[1.0 - hb, hb, 0.0, 0.0]), None, None, None], root, to, &c, (true, false, false)));
        }
    }
    let od = root - facing * 0.6;
    for c in find_ledge(g, od, root, facing, 0.8 * H, 2.7 * H + 0.1, 1.2) {
        if (0.5 * H..H).contains(&c.height) && room_on_top(&c, collision) {
            return Some(pullup_exit(STEP1_TO_KNEE, (c.height - 0.5 * H) / (0.5 * H), root, &c));
        }
        if (H..2.7 * H + 0.1).contains(&c.height) {
            // clips 251 / 430 cm (above the ground at the step's start, root + 1.0 … + 2.8) × 0 / 50 cm
            let hb = ((c.height - H) / (1.8 * H)).clamp(0.0, 1.0);
            let w = [1.0 - hb, hb, 0.0, 0.0];
            let (hl, hr) = hands(&c);
            let to = hang_root(hl, hr, c.normal, LedgeHangType::Wall);
            return Some(exit([blended(STEP1_TO_HANGWALL, 0, &w), blended(STEP1_TO_HANGWALL, 1, &w), None, None], root, to, &c, (false, true, false)));
        }
    }
    None
}

// ---------------------------------------------------------------- update (UpdateWallingSubState 0xE37590)

pub fn update_walling(
    time: Res<Time>,
    pad: Res<PadInput>,
    collision: Res<CollisionWorld>,
    guidance: Res<GuidanceWorld>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (mut loco, mut body, mut data) in &mut q {
        if loco.current != ActorContextId::Walling {
            continue;
        }
        if loco.just_switched {
            loco.just_switched = false;
            continue;
        }
        let w = &mut data.walling;
        w.t = (w.t + dt).min(w.duration());
        body.feet = w.root_at(w.t);
        body.heading = super::heading_of(w.facing());
        body.velocity = Vec3::ZERO;
        body.grounded = false;
        let facing = w.facing();
        let normal = w.normal;
        let sub = w.sub_state;
        let done = w.t >= w.duration() - 1e-4;

        // Rebound (ReboundJump 0xE365C0): push-off along the stick, within ±89° of the wall normal; the landing
        // query is not ported: the fallback target pos + 7·dir − 3·up. PORT trigger: the stick pushed away from
        // the wall (the game's stick vector +0x20 comes from the untraced pad controller).
        if sub != WallingSubState::EntryA && pad.speed01 > 0.0 && pad.dir.dot(normal) > 0.5 {
            let dir = Vec3::new(pad.dir.x, 0.0, pad.dir.z).normalize_or(normal);
            let target = JumpTarget { position: body.feet + dir * 7.0 - Vec3::Y * 3.0, type_flags: jump_blend::TARGET_FREESTEP, hang: None, straight: None, pass: None };
            body.heading = super::heading_of(dir);
            let from = body.feet;
            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::JumpToTarget { from, target, speed_param: 0.5, foot_left: true }));
            continue;
        }
        if !done {
            continue;
        }
        // command 0 = keep going up / try the ledges (+0x38). PORT: Legs held.
        let go_on = pad.legs_held;
        let feet = body.feet;
        let fall = InAirEntry::Fall { from: feet, velocity: Vec3::ZERO, origin: FallOrigin::Ground, speed_param: 0.0 };
        match sub {
            WallingSubState::EntryA => {
                let a = single(ENTRY_B, 0);
                data.walling.play(WallingSubState::EntryB, a, None, feet, None);
            }
            WallingSubState::EntryB => {
                if !go_on {
                    // command ≠ 0: entry_footl_tr_fall (0xFF7CA1) and leave → falling
                    switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(fall));
                    continue;
                }
                if let Some(e) = probe_entry(&guidance, feet, facing, &collision) {
                    switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(e));
                    continue;
                }
                let a = single(STEP1, 0);
                data.walling.play(WallingSubState::Vertical, a, None, feet, None);
            }
            WallingSubState::Vertical => {
                if let Some(e) = probe_vertical(&guidance, feet, facing, &collision) {
                    switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(e));
                    continue;
                }
                let (a, b) = (single(VERTICAL_END, 0), single(VERTICAL_END, 1));
                data.walling.play(WallingSubState::VerticalEnd, a, b, feet, None);
            }
            WallingSubState::VerticalEnd => {
                // anim done → leave: falling back off the wall
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(fall));
            }
        }
    }
}
