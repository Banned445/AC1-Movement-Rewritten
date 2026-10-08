//! Analysis probes over the user's install (ignored tests; run with `cargo test probe_ -- --ignored --nocapture`).
//! They print data used to derive port constants from the game's own animations.

use std::collections::HashMap;

use bevy::prelude::*;

use super::ac_anim::{bone_tracks, decode, TRACK_DISPLACEMENT};
use super::altair::load_altair;
use super::forge::Forge;
use super::game_dir;
use crate::anim::{sample_rot, sample_vec};

const CLASS_ANIMATION: u32 = 0x0FA3_067F;

fn game_fix() -> Vec<super::forge::Resource> {
    let mut f = Forge::open(&game_dir().join("DataPC.forge")).unwrap();
    let e = f.find("Game Fix").cloned().unwrap();
    f.resources(&e).unwrap()
}

#[test]
#[ignore]
fn probe_list_clips() {
    let pat: Vec<String> = std::env::var("PROBE_PAT").unwrap_or("hang|climb|fall|grab|catch|pull|ledge".into()).split('|').map(String::from).collect();
    let mut names: Vec<(String, f32)> = game_fix()
        .iter()
        .filter(|r| r.class_hash == CLASS_ANIMATION && pat.iter().any(|p| r.name.contains(p.as_str())))
        .map(|r| (r.name.clone(), decode(&r.payload).map(|a| a.duration).unwrap_or(-1.0)))
        .collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    for (n, d) in &names {
        println!("{n} {d:.3}");
    }
    println!("{} clips", names.len());
}

/// Limb positions in animation space (x right, y forward, z up; metres) at several times.
#[test]
#[ignore]
fn probe_clip_contacts() {
    let model = load_altair(&game_dir()).unwrap();
    let res = game_fix();
    let by_name: HashMap<&str, &[u8]> = res.iter().map(|r| (r.name.as_str(), r.payload.as_slice())).collect();
    let want: Vec<String> = std::env::var("PROBE_CLIPS")
        .unwrap_or("xx_h_hangwall_wait,xx_h_hangfree_wait,xx_climb_wait_1m,xx_climb_wait_2m".into())
        .split(',')
        .map(String::from)
        .collect();
    let named = [
        (0xb675_f36c_u32, "LHand"),
        (0x75f9_4d30, "RHand"),
        (0x5898_8870, "LFoot"),
        (0x9b14_362c, "RFoot"),
        (0xded1_0611, "Hips"),
        (0x07c1_59a2, "Head"),
        (0x2c52_cbb0, "Ref"),
        (0x7f11_dbe3, "LMid3"),
        (0xf069_fffc, "LIdx3"),
        (0xab4a_365e, "RMid3"),
    ];
    for name in &want {
        let Some(p) = by_name.get(name.as_str()) else {
            println!("{name}: not found");
            continue;
        };
        let a = decode(p).unwrap();
        let (rot, pos) = bone_tracks(&a);
        let rot: HashMap<u32, Vec<(f32, Quat)>> =
            rot.into_iter().map(|(k, v)| (k, v.into_iter().map(|(t, q)| (t, Quat::from_array(q).normalize())).collect())).collect();
        let pos: HashMap<u32, Vec<(f32, Vec3)>> = pos.into_iter().map(|(k, v)| (k, v.into_iter().map(|(t, p)| (t, Vec3::from_array(p))).collect())).collect();
        println!("== {name}  duration {:.3}", a.duration);
        let steps = std::env::var("PROBE_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
        for s in 0..=steps {
            let t = a.duration * s as f32 / steps as f32;
            let mut g: Vec<(Quat, Vec3)> = Vec::new();
            for b in &model.skeleton {
                let r = rot.get(&b.bone_id).map(|k| sample_rot(k, t)).unwrap_or(Quat::from_array(b.local_rot).normalize());
                let tr = pos.get(&b.bone_id).map(|k| sample_vec(k, t)).unwrap_or(Vec3::from_array(b.local_pos));
                let (pr, pp) = b.parent.map(|i| g[i]).unwrap_or((Quat::IDENTITY, Vec3::ZERO));
                g.push((pr * r, pp + pr * tr));
            }
            let disp = pos.get(&TRACK_DISPLACEMENT).map(|k| sample_vec(k, t)).unwrap_or(Vec3::ZERO);
            let mut line = format!("t={t:5.2} disp=({:5.2},{:5.2},{:5.2})", disp.x, disp.y, disp.z);
            for (id, label) in named {
                if let Some(i) = model.skeleton.iter().position(|b| b.bone_id == id) {
                    let p = g[i].1;
                    line += &format!(" {label}=({:5.2},{:5.2},{:5.2})", p.x, p.y, p.z);
                }
            }
            println!("{line}");
        }
    }
}

/// Which skeleton bones each clip animates (rotation tracks), to check finger coverage.
#[test]
#[ignore]
fn probe_track_coverage() {
    let model = load_altair(&game_dir()).unwrap();
    let names: HashMap<u32, String> = serde_free_names();
    let res = game_fix();
    let clips: Vec<String> = std::env::var("PROBE_CLIPS").unwrap_or("xx_h_hangwall_wait".into()).split(',').map(String::from).collect();
    for c in clips {
        let Some(r) = res.iter().find(|r| r.name == c) else { continue };
        let a = decode(&r.payload).unwrap();
        let (rot, pos) = bone_tracks(&a);
        let missing: Vec<String> = model
            .skeleton
            .iter()
            .filter(|b| !rot.contains_key(&b.bone_id))
            .map(|b| names.get(&b.bone_id).cloned().unwrap_or(format!("{:08x}", b.bone_id)))
            .collect();
        println!("{c}: {} rot tracks, {} pos tracks; not animated: {}", rot.len(), pos.len(), missing.join(" "));
    }
}

fn serde_free_names() -> HashMap<u32, String> {
    let txt = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../RE/data/altair_bone_names.json")).unwrap();
    txt.lines()
        .filter_map(|l| {
            let l = l.trim().trim_end_matches(',');
            let (k, v) = l.split_once(':')?;
            let k = u32::from_str_radix(k.trim().trim_matches('"'), 16).ok()?;
            Some((k, v.trim().trim_matches('"').to_string()))
        })
        .collect()
}

/// Generate `src/player/item_flags.rs`: the item words (+60) of every movement action.
/// `cargo test probe_dump_item_flags -- --ignored`
#[test]
#[ignore]
fn probe_dump_item_flags() {
    use super::ac_actions::{ActionGraph, CLASS_ACTION_BLOCK};
    // the movement blocks (no fight, facial or NPC reaction blocks)
    const BLOCKS: &[&str] = &[
        "HumanClimb", "HumanClimb_Jumps", "HumanGround", "HumanGround_Actions", "HumanGround_Hurt", "HumanGround_TurnOnSpot",
        "HumanHayStack", "HumanInAir", "HumanKiosk", "HumanLadder", "HumanLedge", "HumanNarrowObject", "HumanOrientedMove",
        "HumanWalling", "Human_Environment",
    ];
    let res = game_fix();
    let mut graph = ActionGraph::default();
    for r in res.iter().filter(|r| r.class_hash == CLASS_ACTION_BLOCK) {
        graph.add_block(&r.name, &r.payload).unwrap();
    }
    let mut ids: Vec<u32> = graph.actions.values().filter(|a| BLOCKS.contains(&a.block.as_str())).map(|a| a.id).collect();
    ids.sort();
    let mut out = String::new();
    out += "//! GENERATED by `assets::probe::probe_dump_item_flags` from the user's install (DataPC.forge, Game Fix): each
";
    out += "//! movement action's item words at +60 (`ActionItem::Serialize` 0x5BADF0; flag meanings in `anim_gate`, RE/13 §7.6).
";
    out += "//! Derived data only. Regenerate with `cargo test probe_dump_item_flags -- --ignored`.

";
    out += "/// (action id, item words in slot order), sorted by id.
pub const ITEM_FLAGS: &[(u32, &[u16])] = &[
";
    for id in ids {
        let a = &graph.actions[&id];
        let w: Vec<String> = a.items.iter().map(|it| format!("{:#06x}", it.flags)).collect();
        out += &format!("    ({id:#010x}, &[{}]),
", w.join(", "));
    }
    out += "];
";
    std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/src/player/item_flags.rs"), out).unwrap();
}

/// Generate `src/player/jump_clips.rs`: for every clip of the jump / reception / landing actions, its
/// duration and its DISPLACEMENT track sampled at 9 evenly spaced times (animation space: x right,
/// y forward, z up; metres). Derived numbers only — no clip data is copied.
/// `cargo test probe_dump_jump_clips -- --ignored --nocapture`
#[test]
#[ignore]
fn probe_dump_jump_clips() {
    use super::ac_actions::{ActionGraph, CLASS_ACTION_BLOCK};
    let res = game_fix();
    let mut graph = ActionGraph::default();
    for r in res.iter().filter(|r| r.class_hash == CLASS_ACTION_BLOCK) {
        graph.add_block(&r.name, &r.payload).unwrap();
    }
    let names: HashMap<u32, &str> = res.iter().filter(|r| r.class_hash == CLASS_ANIMATION).map(|r| (r.id, r.name.as_str())).collect();
    let by_name: HashMap<&str, &[u8]> = res.iter().map(|r| (r.name.as_str(), r.payload.as_slice())).collect();
    let mut out = String::new();
    out += "//! GENERATED by `assets::probe::probe_dump_jump_clips` from the user's install (DataPC.forge, Game Fix):\n";
    out += "//! the clips of the jump takeoff / flight / reception / landing actions (RE/04 §4.1) and of the ledge moves
//! (corners, ledge jumps, hop-up; RE/03 §7.6) with their duration and\n";
    out += "//! DISPLACEMENT track sampled at 9 evenly spaced times (animation space: x right, y forward, z up; m) and its\n//! rotation as a yaw about z (radians, + = turning left, unwrapped, relative to the start).\n";
    out += "//! Derived measurements only. Regenerate with `cargo test probe_dump_jump_clips -- --ignored`.\n\n";
    out += "pub struct ClipRoot {\n    pub name: &'static str,\n    pub duration: f32,\n    pub disp: [[f32; 3]; 9],\n    pub yaw: [f32; 9],\n}\n\n";
    out += "/// (action id, items: clip names per item in slot order).\npub const ACTIONS: &[(u32, &[&[&str]])] = &[\n";
    let mut clips: Vec<String> = Vec::new();
    let mut ids: Vec<u32> = crate::player::jump_blend::DUMPED_ACTIONS.iter().chain(crate::player::ledge_moves::DUMPED_ACTIONS.iter()).chain(crate::player::walling::DUMPED_ACTIONS.iter()).chain(crate::player::narrow::DUMPED_ACTIONS.iter()).chain(crate::player::collide::DUMPED_ACTIONS.iter()).chain(crate::player::passover::DUMPED_ACTIONS.iter()).chain(crate::player::swing::DUMPED_ACTIONS.iter()).chain(crate::player::ladder::DUMPED_ACTIONS.iter()).chain(crate::player::climb::DUMPED_ACTIONS.iter()).chain(crate::player::ground::LOOK_DOWN.iter()).chain(crate::player::ground::PIVOT_ACTIONS.iter()).chain(crate::player::ground_tree::DUMPED_ACTIONS.iter()).chain(crate::player::ledge::DUMPED_ACTIONS.iter()).copied().collect();
    // The authored transitions into the ground locomotion and the waits (item transitions whose destination is one of
    // them, `sub_5B98B0`): the simulation runs MoveBlend's transition path on their transition actions (RE/02 §4.6), so
    // those actions are dumped too (closure: a transition action can itself lead on).
    let mut transitions: Vec<(u32, usize, u32, u32, u32, u32)> = Vec::new();
    let mut k = 0;
    while k < ids.len() {
        let src = graph.actions.get(&ids[k]).unwrap_or_else(|| panic!("action {:#x} missing", ids[k])).id;
        let items: Vec<_> = graph.actions[&ids[k]].items.iter().map(|it| it.transitions.clone()).collect();
        for (i, trs) in items.iter().enumerate() {
            for t in trs.iter().filter(|t| crate::player::ground_tree::GROUND_DESTINATIONS.contains(&t.action_b) && t.action_a != 0) {
                let Some(ta) = graph.actions.get(&t.action_a) else { continue };
                transitions.push((src, i, ta.id, t.u_a, t.action_b, t.u_b));
                if !ids.contains(&ta.id) {
                    ids.push(ta.id);
                }
            }
        }
        k += 1;
    }
    for id in ids.iter() {
        let a = graph.actions.get(id).unwrap_or_else(|| panic!("action {id:#x} missing"));
        out += &format!("    ({id:#010x}, &[\n");
        for it in &a.items {
            let ns: Vec<String> = it.animations.iter().map(|aid| names.get(aid).map(|s| s.to_string()).unwrap_or_default()).collect();
            out += &format!("        &[{}],\n", ns.iter().map(|n| format!("{n:?}")).collect::<Vec<_>>().join(", "));
            clips.extend(ns);
        }
        out += "    ]),\n";
    }
    out += "];\n\n/// Item transitions into the ground locomotion / waits: (action, item, transition action, its item u_a,\n/// destination action, destination item u_b) (ActionTransition +4/+8/+12/+16, `AnimSlot__SetAction` 0x727F70).\npub const GROUND_TRANSITIONS: &[(u32, usize, u32, u32, u32, u32)] = &[\n";
    for (src, i, ta, ua, tb, ub) in &transitions {
        out += &format!("    ({src:#010x}, {i}, {ta:#010x}, {ua}, {tb:#010x}, {ub}),\n");
    }
    out += "];\n\npub const CLIPS: &[ClipRoot] = &[\n";
    clips.sort();
    clips.dedup();
    for n in clips.iter().filter(|n| !n.is_empty()) {
        let Some(p) = by_name.get(n.as_str()) else { panic!("clip {n} missing") };
        let a = decode(p).unwrap();
        let (rot, pos) = bone_tracks(&a);
        let rkeys: Vec<(f32, Quat)> = rot.get(&TRACK_DISPLACEMENT).map(|k| k.iter().map(|(t, q)| (*t, Quat::from_array(*q).normalize())).collect()).unwrap_or_default();
        let yaw_at = |t: f32| -> f32 {
            if rkeys.is_empty() {
                return 0.0;
            }
            let f = sample_rot(&rkeys, t) * Vec3::Y;
            -f.x.atan2(f.y)
        };
        // unwrap through fine steps so a 180 degree turn is not read as -180
        let mut yaws = Vec::new();
        let (mut acc, mut prev) = (0.0f32, yaw_at(0.0));
        for i in 0..=64 {
            let y = yaw_at(a.duration * i as f32 / 64.0);
            let mut d = y - prev;
            while d > std::f32::consts::PI {
                d -= std::f32::consts::TAU;
            }
            while d < -std::f32::consts::PI {
                d += std::f32::consts::TAU;
            }
            acc += d;
            prev = y;
            if i % 8 == 0 {
                yaws.push(acc);
            }
        }
        // a half turn keyed as two rotations 180 deg apart has no sign of its own: take the side from the clip name, so a
        // blend of a back and a side clip of the same exit turns one way
        let last = *yaws.last().unwrap_or(&0.0);
        if last.abs() > 2.5 && ((n.contains("right") && last > 0.0) || (n.contains("left") && !n.contains("right") && last < 0.0)) {
            for y in yaws.iter_mut() {
                *y = -*y;
            }
        }
        let yaws: Vec<String> = yaws.iter().map(|y| format!("{y:.4}")).collect();
        let keys: Vec<(f32, Vec3)> = pos.get(&TRACK_DISPLACEMENT).map(|k| k.iter().map(|(t, v)| (*t, Vec3::from_array(*v))).collect()).unwrap_or_default();
        let start = keys.first().map(|k| k.1).unwrap_or(Vec3::ZERO);
        let samples: Vec<String> = (0..9)
            .map(|i| {
                let v = if keys.is_empty() { Vec3::ZERO } else { sample_vec(&keys, a.duration * i as f32 / 8.0) - start };
                format!("[{:.4}, {:.4}, {:.4}]", v.x, v.y, v.z)
            })
            .collect();
        out += &format!("    ClipRoot {{ name: {n:?}, duration: {:.4}, disp: [{}], yaw: [{}] }},\n", a.duration, samples.join(", "), yaws.join(", "));
    }
    out += "];\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/player/jump_clips.rs");
    std::fs::write(&path, out).unwrap();
    println!("wrote {} ({} clips)", path.display(), clips.len());
}

/// Model sanity: per part submeshes (ranges, materials), skin joints that fell back to the root / attach
/// bone, and triangles that duplicate another triangle (overlapping LODs / double-sided copies).
#[test]
#[ignore]
fn probe_model_parts() {
    use super::altair::load_altair;
    let m = load_altair(&game_dir()).unwrap();
    println!("skeleton {} bones", m.skeleton.len());
    for p in &m.parts {
        let root_only = p.joints.iter().zip(p.weights.iter()).filter(|(j, w)| j[0] == 0 && w[0] > 0.99).count();
        let mut tri_keys = std::collections::HashMap::new();
        let mut dup = 0;
        let mut total = 0;
        for (idx, _) in &p.sections {
            for t in idx.chunks_exact(3) {
                total += 1;
                let q = |i: u32| { let v = p.positions[i as usize]; [(v[0] * 1000.0) as i32, (v[1] * 1000.0) as i32, (v[2] * 1000.0) as i32] };
                let mut k = [q(t[0]), q(t[1]), q(t[2])];
                k.sort();
                *tri_keys.entry(k).or_insert(0) += 1;
            }
        }
        for c in tri_keys.values() {
            if *c > 1 { dup += c - 1; }
        }
        println!("{}: {} verts, {} tris, {} sections, root-only verts {}, duplicate tris {}", p.name, p.positions.len(), total, p.sections.len(), root_only, dup);
        for (i, (idx, tex)) in p.sections.iter().enumerate() {
            let lo = idx.iter().min().copied().unwrap_or(0);
            let hi = idx.iter().max().copied().unwrap_or(0);
            let (mut agree, mut n) = (0, 0);
            for t in idx.chunks_exact(3) {
                let v = |i: u32| Vec3::from_array(p.positions[i as usize]);
                let f = (v(t[1]) - v(t[0])).cross(v(t[2]) - v(t[0]));
                let vn = Vec3::from_array(p.normals[t[0] as usize]) + Vec3::from_array(p.normals[t[1] as usize]) + Vec3::from_array(p.normals[t[2] as usize]);
                if f.length() < 1e-9 { continue; }
                n += 1;
                if f.dot(vn) >= 0.0 { agree += 1; }
            }
            println!("   section {i}: tris {} verts {lo}..{hi} tex {:?} winding agrees {agree}/{n}", idx.len() / 3, tex);
        }
    }
}

/// Writes Altaïr's decoded diffuse textures (mip 0) as BMPs into PROBE_OUT (local inspection only).
#[test]
#[ignore]
fn probe_dump_textures() {
    let m = load_altair(&game_dir()).unwrap();
    let out = std::path::PathBuf::from(std::env::var("PROBE_OUT").expect("PROBE_OUT"));
    for (id, t) in &m.textures {
        let (w, h) = (t.width as usize, t.height as usize);
        let px = &t.mips[0];
        let mut f = Vec::new();
        let size = 54 + w * h * 4;
        f.extend_from_slice(b"BM");
        f.extend_from_slice(&(size as u32).to_le_bytes());
        f.extend_from_slice(&[0; 4]);
        f.extend_from_slice(&54u32.to_le_bytes());
        f.extend_from_slice(&40u32.to_le_bytes());
        f.extend_from_slice(&(w as i32).to_le_bytes());
        f.extend_from_slice(&(-(h as i32)).to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&32u16.to_le_bytes());
        f.extend_from_slice(&[0; 24]);
        let mut transparent = 0;
        for p in px.chunks_exact(4).take(w * h) {
            if p[3] < 128 { transparent += 1; }
            f.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
        std::fs::write(out.join(format!("{id}.bmp")), f).unwrap();
        println!("{id}: {w}x{h}, {transparent} transparent px");
    }
}

/// Which joints the top of the head / hood is skinned to, per part (vertices above `PROBE_Y`, default 1.65 m).
#[test]
#[ignore]
fn probe_hood_skin() {
    let m = load_altair(&game_dir()).unwrap();
    let names = serde_free_names();
    let y: f32 = std::env::var("PROBE_Y").ok().and_then(|v| v.parse().ok()).unwrap_or(1.65);
    for p in &m.parts {
        let mut count: HashMap<u16, f32> = HashMap::new();
        let mut n = 0;
        for (i, pos) in p.positions.iter().enumerate() {
            if pos[1] < y {
                continue;
            }
            n += 1;
            for k in 0..4 {
                *count.entry(p.joints[i][k]).or_default() += p.weights[i][k];
            }
        }
        if n == 0 {
            continue;
        }
        let mut v: Vec<_> = count.into_iter().filter(|c| c.1 > 0.01).collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let s: Vec<String> = v.iter().take(8).map(|(j, w)| {
            let id = m.skeleton[*j as usize].bone_id;
            format!("{}({:08x})={:.1}", names.get(&id).cloned().unwrap_or("?".into()), id, w)
        }).collect();
        println!("{}: {} verts above {y}: {}", p.name, n, s.join(", "));
    }
}

/// Mesh bones missing from Altaïr's 90-bone skeleton, per part, with their skin weight and bind position.
#[test]
#[ignore]
fn probe_missing_bones() {
    use super::ac_formats::{parse_mesh, parse_skeleton};
    use super::forge::{crc32, Forge};
    let names = serde_free_names();
    let path = game_dir().join("DataPC.forge");
    let mut forge = Forge::open(&path).unwrap();
    let entry = forge.find("Rank 9").cloned().unwrap();
    let res = forge.resources(&entry).unwrap();
    let skel = res.iter().find(|r| r.name == "UCMA_Altair" && r.class_hash == crc32("Skeleton")).map(|r| parse_skeleton(&r.payload)).unwrap();
    let have: std::collections::HashSet<u32> = skel.iter().map(|b| b.bone_id).collect();
    for &name in super::altair::PARTS {
        let Some(r) = res.iter().find(|r| r.name == name && r.class_hash == crc32("Mesh")) else { continue };
        let Some(m) = parse_mesh(&r.payload) else { continue };
        let mut w: HashMap<u32, f32> = HashMap::new();
        for s in &m.submeshes {
            for v in s.vstart as usize..(s.vstart + s.vcount) as usize {
                for k in 0..4 {
                    let local = m.bone_idx[v][k] as usize;
                    if let Some(b) = s.palette.get(local).and_then(|&mb| m.bones.get(mb as usize)) {
                        if !have.contains(&b.bone_id) {
                            *w.entry(b.bone_id).or_default() += m.bone_w[v][k] as f32 / 255.0;
                        }
                    }
                }
            }
        }
        let mut v: Vec<_> = w.into_iter().collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let s: Vec<String> = v.iter().map(|(id, x)| format!("{}({id:08x})={x:.0}", names.get(id).cloned().unwrap_or("?".into()))).collect();
        println!("{name}: {} mesh bones, missing: {}", m.bones.len(), s.join(", "));
    }
}

/// Yaw over time of the DISPLACEMENT track's rotation (if any), the Reference bone and the hips (degrees, about the
/// animation's Z axis; 0 = facing +Y). `PROBE_CLIPS` = comma list.
#[test]
#[ignore]
fn probe_clip_yaw() {
    let model = load_altair(&game_dir()).unwrap();
    let res = game_fix();
    let by_name: HashMap<&str, &[u8]> = res.iter().map(|r| (r.name.as_str(), r.payload.as_slice())).collect();
    let want: Vec<String> = std::env::var("PROBE_CLIPS").unwrap_or("xx_h_wait_hipm_footl_to_h_waitturn_left_180_footl".into()).split(',').map(String::from).collect();
    let yaw = |q: Quat| {
        let f = q * Vec3::Y;
        f.x.atan2(f.y).to_degrees() * -1.0
    };
    for name in &want {
        let Some(p) = by_name.get(name.as_str()) else {
            println!("{name}: not found");
            continue;
        };
        let a = decode(p).unwrap();
        let (rot, pos) = bone_tracks(&a);
        let rot: HashMap<u32, Vec<(f32, Quat)>> = rot.into_iter().map(|(k, v)| (k, v.into_iter().map(|(t, q)| (t, Quat::from_array(q).normalize())).collect())).collect();
        let pos: HashMap<u32, Vec<(f32, Vec3)>> = pos.into_iter().map(|(k, v)| (k, v.into_iter().map(|(t, p)| (t, Vec3::from_array(p))).collect())).collect();
        println!("== {name}  duration {:.3}  disp rot track: {}", a.duration, rot.contains_key(&TRACK_DISPLACEMENT));
        for s in 0..=8 {
            let t = a.duration * s as f32 / 8.0;
            let mut g: Vec<(Quat, Vec3)> = Vec::new();
            for b in &model.skeleton {
                let r = rot.get(&b.bone_id).map(|k| sample_rot(k, t)).unwrap_or(Quat::from_array(b.local_rot).normalize());
                let tr = pos.get(&b.bone_id).map(|k| sample_vec(k, t)).unwrap_or(Vec3::from_array(b.local_pos));
                let (pr, pp) = b.parent.map(|i| g[i]).unwrap_or((Quat::IDENTITY, Vec3::ZERO));
                g.push((pr * r, pp + pr * tr));
            }
            let dr = rot.get(&TRACK_DISPLACEMENT).map(|k| yaw(sample_rot(k, t)));
            let dp = pos.get(&TRACK_DISPLACEMENT).map(|k| sample_vec(k, t)).unwrap_or(Vec3::ZERO);
            let bone = |id: u32| model.skeleton.iter().position(|b| b.bone_id == id).map(|i| yaw(g[i].0));
            println!("t={t:5.2} disp=({:5.2},{:5.2}) disp_yaw={:?} ref_yaw={:?} hips_yaw={:?}", dp.x, dp.y, dr, bone(0x2c52_cbb0), bone(0xded1_0611));
        }
    }
}

/// Resolve action ids (resource ids or Action+8 request ids, `PROBE_IDS` hex or `PROBE_DEC` decimal) and print
/// their items, clip durations, item words and every transition with its two u32 fields.
#[test]
#[ignore]
fn probe_resolve_actions() {
    use super::ac_actions::{ActionGraph, CLASS_ACTION_BLOCK};
    let res = game_fix();
    let mut graph = ActionGraph::default();
    for r in res.iter().filter(|r| r.class_hash == CLASS_ACTION_BLOCK) {
        graph.add_block(&r.name, &r.payload).unwrap();
    }
    let names: HashMap<u32, &str> = res.iter().filter(|r| r.class_hash == CLASS_ANIMATION).map(|r| (r.id, r.name.as_str())).collect();
    let dur: HashMap<u32, f32> = res.iter().filter(|r| r.class_hash == CLASS_ANIMATION).filter_map(|r| decode(&r.payload).ok().map(|a| (r.id, a.duration))).collect();
    let mut ids: Vec<u32> = std::env::var("PROBE_IDS").unwrap_or_default().split(',').filter_map(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()).collect();
    ids.extend(std::env::var("PROBE_DEC").unwrap_or_default().split(',').filter_map(|s| s.trim().parse::<u32>().ok()));
    for id in ids {
        let Some(a) = graph.actions.get(&id) else { println!("{id} ({id:#x}): not found"); continue };
        println!("{id} ({id:#x}) -> action {:#010x} request {} [{}] repeat {} flags {:#x} channel {:#x}", a.id, a.request_id, a.block, a.repeat, a.flags, a.channel);
        if let Some(t) = a.in_transition { println!("    in  {t:x?}"); }
        if let Some(t) = a.out_transition { println!("    out {t:x?}"); }
        for (k, it) in a.items.iter().enumerate() {
            let clips: Vec<String> = it.animations.iter().map(|x| format!("{}({:.3})", names.get(x).copied().unwrap_or("?"), dur.get(x).copied().unwrap_or(-1.0))).collect();
            println!("  item {k} {:#010x} word {:#06x} w {:?} {:?}", it.id, it.flags, it.weights, clips);
            for t in &it.transitions {
                println!("      tr a {:#010x} u_a {} -> b {:#010x} u_b {} blend_a {} {:.3} blend_b {} {:.3}", t.action_a, t.u_a, t.action_b, t.u_b, t.blend_a.kind, t.blend_a.time, t.blend_b.kind, t.blend_b.time);
            }
        }
    }
}

/// Find action / item ids (`PROBE_IDS`, comma list of hex) in every ActionBlock: block, items, clips, transitions.
#[test]
#[ignore]
fn probe_find_actions() {
    use super::ac_actions::{ActionGraph, CLASS_ACTION_BLOCK};
    let res = game_fix();
    let mut graph = ActionGraph::default();
    for r in res.iter().filter(|r| r.class_hash == CLASS_ACTION_BLOCK) {
        graph.add_block(&r.name, &r.payload).unwrap();
    }
    let names: HashMap<u32, &str> = res.iter().filter(|r| r.class_hash == CLASS_ANIMATION).map(|r| (r.id, r.name.as_str())).collect();
    let ids: Vec<u32> = std::env::var("PROBE_IDS").unwrap_or_default().split(',').filter_map(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()).collect();
    for id in ids {
        let mut found = false;
        for a in graph.actions.values() {
            let hit_item = a.items.iter().any(|it| it.id == id);
            let hit_tr = a.items.iter().any(|it| it.transitions.iter().any(|t| t.action_a == id || t.action_b == id));
            if a.id == id || hit_item || hit_tr {
                found = true;
                let what = if a.id == id { "action" } else if hit_item { "item of" } else { "transition in" };
                println!("{id:#010x}: {what} {:#010x} [{}]", a.id, a.block);
                for it in &a.items {
                    let clips: Vec<&str> = it.animations.iter().map(|x| names.get(x).copied().unwrap_or("?")).collect();
                    println!("    item {:#010x} {:?} w {:?} tr {:x?}", it.id, clips, it.weights, it.transitions);
                }
            }
        }
        if !found {
            println!("{id:#010x}: not found");
        }
    }
}
