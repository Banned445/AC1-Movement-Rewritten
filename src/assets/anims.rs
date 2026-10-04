//! Loads Altaïr's locomotion clips from the user's install (DataPC.forge → "Game Fix", RE/10).
//! Each gait is stored as two half cycles (left foot / right foot); they are joined into one loop.

use std::collections::HashMap;
use std::path::Path;

use super::ac_anim::{bone_tracks, decode, TRACK_DISPLACEMENT};
use super::ac_actions::{ActionGraph, CLASS_ACTION_BLOCK};
use super::forge::Forge;

pub struct RawClip {
    pub name: String,
    pub duration: f32,
    pub rotations: HashMap<u32, Vec<(f32, [f32; 4])>>,
    pub translations: HashMap<u32, Vec<(f32, [f32; 3])>>,
    /// ACUATORCONTACTS track (fixed track 2): contact bitmask per key, held until the next key.
    /// Bits (`AcuatorsContactsTypes`): 0 L heel, 1 R heel, 2 L toes, 3 R toes, 4 L hand, 5 R hand, 6 no look-at.
    pub contacts: Vec<(f32, u8)>,
}

/// Fixed track id of the contact bitmask (`AnimTrack::FixedTrackIds` 2 = ACUATORCONTACTS).
pub const TRACK_ACUATOR_CONTACTS: u32 = 2;

/// ActionBlocks whose clips are loaded (the movement contexts).
pub const MOVEMENT_BLOCKS: &[&str] = &["HumanClimb", "HumanClimb_Jumps", "HumanLedge", "HumanInAir"];
/// Single actions of other blocks whose clips are loaded too: the ground locomotion blend (MoveBlend 0xDA0810).
pub const EXTRA_ACTIONS: &[u32] = &[
    crate::player::move_blend::ACT_GROUND_LOCOMOTION,
    // jump takeoff / flight / reception / landings (RE/04 §6)
    0x0A4C_8C0E, 0x0A4C_8C0F, 0x010D_DAFA, 0x010D_F0D8, 0x010D_E1FE, 0x010D_F150,
    0x6D20_0160, 0x6D20_0161, 0x6E9C_754A, 0x6E9C_754C, 0x6E9C_702A, 0x6E9C_7557, 0x010D_D707, 0x010D_D70B,
];

fn contact_keys(a: &super::ac_anim::AnimData) -> Vec<(f32, u8)> {
    a.tracks
        .iter()
        .find(|t| t.key == TRACK_ACUATOR_CONTACTS)
        .and_then(|t| match &t.values {
            super::ac_anim::TrackValues::Byte(v) => Some(t.times.iter().copied().zip(v.iter().copied()).collect()),
            _ => None,
        })
        .unwrap_or_default()
}

/// (clip name, resource names joined in order). All from DataPC.forge → "Game Fix".
pub const LOCOMOTION: &[(&str, &[&str])] = &[
    // ground
    // the game's wait actions (HumanGround 0xD8243F / 0xD824C5 low, 0xD82508 / 0xD8258E high), by the leading
    // foot; the `_footm` waits are 1 s parallel-feet poses, not the idle
    ("idle_low", &["xx_l_wait_hipm_footl"]),
    ("idle_low_r", &["xx_l_wait_hipm_footr"]),
    ("idle_high", &["xx_h_wait_hipm_footl"]),
    ("idle_high_r", &["xx_h_wait_hipm_footr"]),
    ("walk", &["xx_l_walk_hipm_footl", "xx_l_walk_hipm_footr"]),
    ("jog", &["xx_h_jog_hipm_footl", "xx_h_jog_hipm_footr"]),
    ("run", &["xx_h_run_hipm_footl", "xx_h_run_hipm_footr"]),
    ("sprint", &["xx_h_sprint_hipm_footl", "xx_h_sprint_hipm_footr"]),
    // air and landing
    ("jump", &["xx_h_jumpstraight_clear_footl"]),
    ("fall", &["xx_h_jumpfalling01"]),
    ("roll", &["xx_roll_hipm"]),
    ("land_soft", &["xx_h_landing_forward_soft_footl_tr_h_jog_footr_a"]),
    ("land_hard", &["xx_h_landing_damage_footl"]),
    // ledge (hang + shimmy: open = lead hand reaches, close = trailing hand catches up)
    ("hang_wall", &["xx_h_hangwall_wait"]),
    ("hang_free", &["xx_h_hangfree_wait"]),
    ("shimmy_wall_left_open", &["xx_h_hangwall_strafe_left_050cm_open"]),
    ("shimmy_wall_left_close", &["xx_h_hangwall_strafe_left_050cm_close"]),
    ("shimmy_wall_right_open", &["xx_h_hangwall_strafe_right_050cm_open"]),
    ("shimmy_wall_right_close", &["xx_h_hangwall_strafe_right_050cm_close"]),
    ("shimmy_free_left_open", &["xx_h_hangfree_strafe_left_050cm_open"]),
    ("shimmy_free_left_close", &["xx_h_hangfree_strafe_left_050cm_close"]),
    ("shimmy_free_right_open", &["xx_h_hangfree_strafe_right_050cm_open"]),
    ("shimmy_free_right_close", &["xx_h_hangfree_strafe_right_050cm_close"]),
    ("hang_up", &["xx_h_hangwall_u_climb_1m"]),
    ("hang_down", &["xx_h_hangwall_d_climb_1m"]),
    // pull-up: onto a knee on the edge, then stand up (the game's hangknee transition chain)
    ("pullup_wall", &["xx_h_hangwall_tr_hangknee_footl_a", "xx_h_hangwall_tr_hangknee_footl_b", "xx_h_hangknee_footl_tr_h_wait_footr_a", "xx_h_hangknee_footl_tr_h_wait_footr_b"]),
    ("pullup_free", &["xx_h_hangfree_tr_hangwaist_a", "xx_h_hangfree_tr_hangwaist_b", "xx_h_hangwaist_tr_hangknee_footl", "xx_h_hangknee_footl_tr_h_wait_footr_a", "xx_h_hangknee_footl_tr_h_wait_footr_b"]),
    // catching a ledge from the air (reception into the hang)
    ("catch_wall", &["xx_fall_tr_hangwall_straight_min_a", "xx_fall_tr_hangwall_straight_min_b"]),
    ("catch_free", &["xx_fall_tr_hangfree_min_a", "xx_fall_tr_hangfree_min_b"]),
    // jumping at a ledge: flight into the hang, then the matching reception
    ("jump_hangwall", &["xx_h_jumpstraight_footl_to_hangwall_250cm"]),
    ("jump_hangfree", &["xx_h_jumpstraight_footl_to_hangfree_300cm"]),
    ("catch_jump_wall", &["xx_h_jumpstraight_footl_to_hangwall_250cm_tr_hangwall_a", "xx_h_jumpstraight_footl_to_hangwall_250cm_tr_hangwall_b"]),
    ("catch_jump_free", &["xx_h_jumpstraight_footl_to_hangfree_300cm_tr_hangfree_a", "xx_h_jumpstraight_footl_to_hangfree_300cm_tr_hangfree_b"]),
    // entering the fall pose (the fall loop itself is a single held pose in the game data)
    ("jump_to_fall", &["xx_h_jumpstraight_clear_footall_tr_fall"]),
    ("walk_to_fall", &["xx_l_walklowfall_footl_tr_fall"]),
    ("run_to_fall", &["xx_h_runlowfall_footl_tr_fall"]),
    ("hangwall_to_fall", &["xx_h_hangwall_tr_fall_a", "xx_h_hangwall_tr_fall_b"]),
    ("hangfree_to_fall", &["xx_h_hangfree_tr_fall_a", "xx_h_hangfree_tr_fall_b"]),
    // climb: one wait clip per limb pose (pose table 0x1A2CD70)
    ("climb_pose0", &["xx_climb_wait_1m"]),
    ("climb_pose1", &["xx_climb_wait_1lu"]),
    ("climb_pose2", &["xx_climb_wait_1ru"]),
    ("climb_pose3", &["xx_climb_wait_2m"]),
    ("climb_pose4", &["xx_climb_wait_2lu"]),
    ("climb_pose5", &["xx_climb_wait_2ru"]),
];

fn append<T: Copy>(dst: &mut Vec<(f32, T)>, src: &[(f32, T)], offset: f32, map: impl Fn(T) -> T) {
    for &(t, v) in src {
        let t = t + offset;
        if dst.last().is_some_and(|l| (l.0 - t).abs() < 1e-4) {
            dst.pop(); // the join key appears at the end of A and the start of B
        }
        dst.push((t, map(v)));
    }
}

const CLASS_ANIMATION: u32 = 0x0FA3_067F;

/// Everything the animator needs from the install: the named locomotion clips, every clip referenced by
/// the movement ActionBlocks (by resource name), and the decoded action graph.
pub fn load_locomotion(game_dir: &Path) -> Result<(Vec<RawClip>, ActionGraph, HashMap<u32, String>), String> {
    let path = game_dir.join("DataPC.forge");
    let mut forge = Forge::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let entry = forge.find("Game Fix").cloned().ok_or("file 'Game Fix' not found")?;
    let res = forge.resources(&entry).map_err(|e| e.to_string())?;
    let by_name: HashMap<&str, &[u8]> = res.iter().map(|r| (r.name.as_str(), r.payload.as_slice())).collect();
    let anim_names: HashMap<u32, String> =
        res.iter().filter(|r| r.class_hash == CLASS_ANIMATION).map(|r| (r.id, r.name.clone())).collect();

    // the animation graph (all ActionBlocks; every byte must parse)
    let mut graph = ActionGraph::default();
    for r in res.iter().filter(|r| r.class_hash == CLASS_ACTION_BLOCK) {
        graph.add_block(&r.name, &r.payload)?;
    }

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // named (joined) clips
    let named = LOCOMOTION.iter().map(|(c, p)| (c.to_string(), p.iter().map(|s| s.to_string()).collect::<Vec<_>>()));
    // every clip used by the movement blocks, under its resource name
    let mut graph_clips: Vec<String> = graph
        .actions
        .values()
        .filter(|a| {
            MOVEMENT_BLOCKS.contains(&a.block.as_str())
                || EXTRA_ACTIONS.contains(&a.id)
                || crate::player::ledge_moves::DUMPED_ACTIONS.contains(&a.id)
                || crate::player::jump_blend::DUMPED_ACTIONS.contains(&a.id)
                || crate::player::walling::DUMPED_ACTIONS.contains(&a.id)
                || crate::player::narrow::DUMPED_ACTIONS.contains(&a.id)
                || crate::player::collide::DUMPED_ACTIONS.contains(&a.id)
                || crate::player::passover::DUMPED_ACTIONS.contains(&a.id)
                || crate::player::swing::DUMPED_ACTIONS.contains(&a.id)
                || crate::player::ladder::DUMPED_ACTIONS.contains(&a.id)
                || crate::player::climb::DUMPED_ACTIONS.contains(&a.id)
                || crate::player::ground::LOOK_DOWN.contains(&a.id)
                || crate::player::ground::PIVOT_ACTIONS.contains(&a.id)
                || crate::player::air::DROP_ENTRY.iter().flatten().any(|&d| d == a.id)
                || crate::player::air::DROP_LOOP.contains(&a.id)
        })
        .flat_map(|a| a.items.iter().flat_map(|it| it.animations.iter()))
        .filter_map(|id| anim_names.get(id).cloned())
        .collect();
    graph_clips.sort();
    graph_clips.dedup();
    let from_graph = graph_clips.into_iter().map(|n| (n.clone(), vec![n]));
    for (clip, parts) in named.chain(from_graph) {
        if !seen.insert(clip.clone()) {
            continue;
        }
        let mut c = RawClip { name: clip.to_string(), duration: 0.0, rotations: HashMap::new(), translations: HashMap::new(), contacts: Vec::new() };
        let mut disp_end = [0f32; 3];
        for part in &parts {
            let payload = by_name.get(part.as_str()).ok_or(format!("animation {part} not found"))?;
            let a = decode(payload).map_err(|e| format!("{part}: {e}"))?;
            let (rot, pos) = bone_tracks(&a);
            let off = c.duration;
            for (k, keys) in rot {
                append(c.rotations.entry(k).or_default(), &keys, off, |v| v);
            }
            for (k, keys) in pos {
                // root motion accumulates across the halves; bone translations do not
                let base = if k == TRACK_DISPLACEMENT { disp_end } else { [0.0; 3] };
                append(c.translations.entry(k).or_default(), &keys, off, |v| [v[0] + base[0], v[1] + base[1], v[2] + base[2]]);
            }
            append(&mut c.contacts, &contact_keys(&a), off, |v| v);
            if let Some(d) = c.translations.get(&TRACK_DISPLACEMENT).and_then(|k| k.last()) {
                disp_end = d.1;
            }
            c.duration += a.duration;
        }
        out.push(c);
    }
    Ok((out, graph, anim_names))
}
