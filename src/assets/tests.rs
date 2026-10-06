//! Asset decoder tests. The integration tests read the user's own install and are skipped when it
//! is not present (set AC_GAME_DIR to point at it).

use super::altair::load_altair;
use super::forge::{adler32_init0, crc32, Forge};
use super::game_dir;

#[test]
fn crc32_matches_known_engine_hashes() {
    // verified pairs from RE/07 and RE/08
    assert_eq!(crc32("Enter"), 0x78B1_EF6A);
    assert_eq!(crc32("World"), 0xFBB6_3E47);
    assert_eq!(crc32("Entity"), 0x0984_415E);
    assert_eq!(crc32("Mesh"), 0x415D_9568);
    assert_eq!(crc32("Bone"), 0x9574_1049);
}

#[test]
fn adler32_init0_is_stock_adler_minus_one() {
    // stock Adler-32 of "abc" is 0x024D0127; with initial a = 0 instead of 1:
    // a' = a - 1, b' = b - len
    assert_eq!(adler32_init0(b"abc"), 0x024D_0127 - (3 << 16) - 1);
}

fn install_present() -> bool {
    game_dir().join("DataPC.forge").exists()
}

#[test]
fn forge_index_reads_from_install() {
    if !install_present() {
        eprintln!("skipped: game install not found");
        return;
    }
    let f = Forge::open(&game_dir().join("DataPC_Map_Menu.forge")).unwrap();
    assert_eq!(f.entries.len(), 2);
    assert_eq!(f.entries[1].name, "Map_Menu");
    let mut f = f;
    let e = f.entries[1].clone();
    let res = f.resources(&e).unwrap();
    assert_eq!(res.len(), 184, "same count as the Python reader");
}

#[test]
fn altair_model_loads_from_install() {
    if !install_present() {
        eprintln!("skipped: game install not found");
        return;
    }
    let m = load_altair(&game_dir()).expect("load Altaïr");
    let body = m.parts.iter().find(|p| p.name == "UCMA_Altair_Body_C").expect("body part");
    assert_eq!(body.positions.len(), 3128);
    let tris: usize = body.sections.iter().map(|s| s.0.len() / 3).sum();
    assert_eq!(tris, 3887);
    let all_y = m.parts.iter().flat_map(|p| p.positions.iter().map(|v| v[1]));
    let (lo, hi) = all_y.fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(y), b.max(y)));
    assert!(lo.abs() < 1e-4, "feet at y = 0, got {lo}");
    assert!((1.75..2.0).contains(&hi), "Altaïr (with hood) should be ~1.8–1.9 m tall, got {hi}");
    let head = m.parts.iter().find(|p| p.name == "UCMA_Altair_Head").expect("head part");
    let head_min = head.positions.iter().map(|v| v[1]).fold(f32::MAX, f32::min);
    assert!(head_min > 1.4, "head should sit on the shoulders after attaching, min y {head_min}");
    assert!(!m.textures.is_empty(), "diffuse textures decoded");
    for t in m.textures.values() {
        assert_eq!(t.mips[0].len(), (t.width * t.height * 4) as usize);
    }
}

#[test]
fn altair_atlas_and_normal_maps_from_install() {
    if !install_present() {
        eprintln!("skipped: game install not found");
        return;
    }
    let m = load_altair(&game_dir()).expect("load Altaïr");
    let body = m.parts.iter().find(|p| p.name == "UCMA_Altair_Body_C").unwrap();
    // The robe intentionally uses a second atlas tile. Clamping it produces red/black strips.
    assert!(body.uvs.iter().any(|uv| uv[0] > 1.9), "body must retain its tiled UVs");
    let boots = m.parts.iter().find(|p| p.name == "UCMA_Altair_Boots_B").unwrap();
    assert!(boots.uvs.iter().any(|uv| uv[1] > 0.99), "boots must use the bottom of the accessories atlas");
    assert!(!m.normal_textures.is_empty(), "normal maps loaded separately from sRGB diffuse maps");
    for part in &m.parts {
        assert_eq!(part.sections.len(), part.normal_maps.len(), "{}", part.name);
        assert_eq!(part.positions.len(), part.tangents.len(), "{}", part.name);
        for t in &part.tangents {
            assert!(t.iter().all(|x| x.is_finite()), "{}: invalid tangent", part.name);
            assert!((t[0] * t[0] + t[1] * t[1] + t[2] * t[2] - 1.0).abs() < 1e-3);
            assert!(t[3] == 1.0 || t[3] == -1.0);
        }
        for id in part.normal_maps.iter().flatten() {
            let tex = &m.normal_textures[id];
            assert_eq!(tex.mips[0].len(), (tex.width * tex.height * 4) as usize);
        }
    }
}

#[test]
fn anim_event_tracks_decode_from_the_install() {
    use super::ac_anim::{decode_events, EventKind};
    if !install_present() {
        eprintln!("skipped: game install not found");
        return;
    }
    let mut f = Forge::open(&game_dir().join("DataPC.forge")).unwrap();
    let e = f.find("Game Fix").cloned().unwrap();
    let res = f.resources(&e).unwrap();
    let anims: Vec<_> = res.iter().filter(|r| r.class_hash == 0x0FA3_067F).collect();
    let failed: Vec<_> = anims.iter().filter(|r| decode_events(&r.payload).is_err()).map(|r| r.name.as_str()).collect();
    // RE/10 §2.1: all but the clips with an FXEvent or the one unknown assassination event
    assert!(anims.len() > 12_000 && failed.len() <= 23, "{} of {} failed: {:?}", failed.len(), anims.len(), &failed[..failed.len().min(5)]);
    // a jog stop: walk end, slide, pivot at 16, 18 and 22 / 120 s
    let stop = anims.iter().find(|r| r.name == "xx_h_jogstop_footr").unwrap();
    let ev = decode_events(&stop.payload).unwrap();
    let got: Vec<(i32, u32)> = ev
        .iter()
        .map(|e| ((e.time * 120.0).round() as i32, match e.kind { EventKind::Contact { ty, .. } => ty, _ => u32::MAX }))
        .collect();
    assert_eq!(got, vec![(16, 13), (18, 8), (22, 6)]);
    // the joined walk cycle steps once per foot (ft_walk_cycle at 0.167 s into each half)
    let (clips, _, _) = super::anims::load_locomotion(&game_dir()).expect("load clips");
    let walk = clips.iter().find(|c| c.name == "walk").unwrap();
    let steps: Vec<f32> = walk.events.iter().filter(|e| e.kind == EventKind::Contact { ty: 12, bits: 3 }).map(|e| e.time).collect();
    assert_eq!(steps.len(), 2, "{:?}", walk.events);
    assert!((steps[0] - 20.0 / 120.0).abs() < 1e-4 && (steps[1] - (0.5333 + 20.0 / 120.0)).abs() < 1e-3, "{steps:?}");
}

#[test]
fn locomotion_clips_decode_with_measured_speeds() {
    if !install_present() {
        eprintln!("skipped: game install not found");
        return;
    }
    let (clips, graph, _) = super::anims::load_locomotion(&game_dir()).expect("load clips");
    // the animation graph decodes from the install and resolves the exe's hard-coded action ids (RE/13)
    assert!(graph.actions.len() > 4000, "actions decoded: {}", graph.actions.len());
    for id in [0x012D_A39Fu32, 0x0106_D2C5, 0x019A_05F1, 0x1F0C_22C2] {
        assert!(graph.actions.contains_key(&id), "action {id:#x} missing");
    }
    // root-motion speeds measured in RE/10 (DISPLACEMENT track / duration)
    let expect = [("walk", 1.90f32), ("jog", 3.54), ("run", 5.12), ("sprint", 6.28)];
    for (name, speed) in expect {
        let c = clips.iter().find(|c| c.name == name).expect(name);
        let d = &c.translations[&super::ac_anim::TRACK_DISPLACEMENT];
        let a = d.first().unwrap().1;
        let b = d.last().unwrap().1;
        let dist = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt();
        let v = dist / c.duration;
        assert!((v - speed).abs() < 0.02, "{name}: {v} m/s, expected {speed}");
        // every decoded rotation key is a unit quaternion
        for keys in c.rotations.values() {
            for (_, q) in keys {
                let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
                assert!((n - 1.0).abs() < 1e-3, "{name}: non-unit quaternion {q:?}");
            }
        }
    }
}

#[test]
fn limb_ik_chains_exist_in_altairs_skeleton() {
    if !install_present() {
        eprintln!("skipped: game install not found");
        return;
    }
    let m = load_altair(&game_dir()).unwrap();
    for (a, b, c) in crate::ik::LIMB_CHAINS {
        let idx = |id: u32| m.skeleton.iter().position(|bone| bone.bone_id == id).unwrap_or_else(|| panic!("bone {id:08x} missing"));
        let (ia, ib, ic) = (idx(a), idx(b), idx(c));
        // each chain is a real ancestor line: upper → … → middle → … → end
        let is_ancestor = |anc: usize, mut i: usize| {
            while let Some(p) = m.skeleton[i].parent {
                if p == anc {
                    return true;
                }
                i = p;
            }
            false
        };
        assert!(is_ancestor(ia, ib) && is_ancestor(ib, ic), "chain {a:08x}/{b:08x}/{c:08x} is not a hierarchy line");
    }
}

#[test]
fn altair_visual_rig_and_native_skinning_from_install() {
    use bevy::prelude::*;
    if !install_present() { eprintln!("skipped: game install not found"); return; }
    let m = load_altair(&game_dir()).expect("load visual rig");
    assert_eq!(m.skeleton.len(), 90, "movement rig keeps its original joint order");
    assert!(!m.visual_bones.is_empty());
    let bones: Vec<_> = m.skeleton.iter().chain(&m.visual_bones).collect();
    let mut global = Vec::new();
    for (i, bone) in bones.iter().enumerate() {
        assert!(bone.parent.is_none_or(|p| p < i), "visual parents precede children");
        let t = Mat4::from_rotation_translation(Quat::from_array(bone.local_rot).normalize(), Vec3::from_array(bone.local_pos));
        global.push(bone.parent.map(|p| global[p]).unwrap_or(Mat4::IDENTITY) * t);
    }
    let face = m.parts.iter().find(|p| p.name == "Universal_Head_Obj_Clean").expect("eyes/mouth mesh");
    assert_eq!(face.sections.len(), 2);
    assert!(face.sections.iter().all(|(_, texture)| texture.is_some()));
    assert!(face.skin_joints.iter().all(|&j| j >= m.skeleton.len()), "face uses authored facial descendants");
    let skirt = m.parts.iter().find(|p| p.name == "UCMA_Altair_Cloth").expect("lower robe rest mesh");
    assert_eq!(skirt.positions.len(), 146);
    assert_eq!(skirt.sections.iter().map(|(idx, _)| idx.len()).sum::<usize>(), 624);
    assert!(skirt.sections.iter().all(|(_, texture)| texture.is_some()));
    for part in &m.parts {
        assert_eq!(part.skin_joints.len(), part.inverse_bindposes.len());
        assert!(part.skin_joints.iter().all(|&j| j < bones.len()));
        for matrix in &part.inverse_bindposes {
            assert!(matrix.iter().all(|v| v.is_finite()));
            assert!((Mat4::from_cols_array(matrix).determinant() - 1.0).abs() < 0.01);
        }
        for (joint, weight) in part.joints.iter().zip(&part.weights) {
            assert!(joint.iter().all(|&j| (j as usize) < part.skin_joints.len()), "{}: palette out of range", part.name);
            assert!((weight.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        }
        // Rest skinning must stay finite and within character scale, including the separate face frame.
        for (i, position) in part.positions.iter().enumerate() {
            let mut p = Vec3::ZERO;
            for k in 0..4 {
                let j = part.joints[i][k] as usize;
                let skin = global[part.skin_joints[j]] * Mat4::from_cols_array(&part.inverse_bindposes[j]);
                p += skin.transform_point3(Vec3::from_array(*position)) * part.weights[i][k];
            }
            assert!(p.is_finite() && p.length() < 4.0, "{}: misplaced rest skin", part.name);
        }
    }
}

#[test]
fn character_attachments_and_cloth_constraints_from_install() {
    if !install_present() { eprintln!("skipped: game install not found"); return; }
    let m = load_altair(&game_dir()).expect("load character attachments");
    for (name, id) in [("ARCM_Altair_Sword_D", 0x3A83_5926), ("UCMA_Altair_Dagger", 0x685E_46B6)] {
        let part = m.parts.iter().find(|p| p.name == name).expect("weapon appearance mesh");
        assert_eq!(part.skin_joints.len(), 1);
        let bones: Vec<_> = m.skeleton.iter().chain(&m.visual_bones).collect();
        assert_eq!(bones[part.skin_joints[0]].bone_id, id);
        assert!(part.sections.iter().all(|(_, texture)| texture.is_some()));
        assert!(part.weights.iter().all(|w| *w == [1.0, 0.0, 0.0, 0.0]));
        let min = part.positions.iter().fold([f32::MAX; 3], |a, p| std::array::from_fn(|k| a[k].min(p[k])));
        let max = part.positions.iter().fold([f32::MIN; 3], |a, p| std::array::from_fn(|k| a[k].max(p[k])));
        let span = (0..3).map(|k| max[k] - min[k]).fold(0.0f32, f32::max);
        assert!(span > 0.3 && span < 1.2, "{name}: weapon scale {span}");
    }
    let cloth = m.parts.iter().find(|p| p.name == "UCMA_Altair_Cloth").unwrap().cloth.as_ref().expect("cloth settings");
    assert_eq!(m.visual_rotation_copies.len(), 2);
    assert_eq!(m.visual_compressions.len(), 1);
    assert_eq!(m.visual_look_at.len(), 1);
    assert!(m.visual_look_at.iter().all(|m| m.target >= 90 && m.aim < 90));
    assert!(m.visual_compressions.iter().all(|c| c.target >= m.skeleton.len() && c.sources.iter().all(|s| *s < m.skeleton.len() + m.visual_bones.len())));
    assert!(m.visual_rotation_copies.iter().all(|(target, source)| *target >= m.skeleton.len() && *source < m.skeleton.len() + m.visual_bones.len()));
    assert_eq!(cloth.pinned.len(), 146);
    assert_eq!(cloth.pinned.iter().filter(|&&p| p).count(), 44);
    assert_eq!(cloth.iterations, 3);
    assert!((cloth.damping - 0.1).abs() < 1e-5);
    assert!((cloth.upward_damping - 0.75).abs() < 1e-5);
    assert!((cloth.gravity + 9.8).abs() < 1e-5);
    assert!(cloth.edges.len() > 200);
    assert!(cloth.edges.iter().all(|[a,b]| a != b && *a < 146 && *b < 146));
    assert!(cloth.pull.iter().all(|p| (0.0..=1.0).contains(p)));
    assert!(cloth.pull.iter().any(|&p| p > 0.5), "authored pull strength, not speed/decay fields");
    assert_eq!(cloth.colliders.len(), 6);
    assert_eq!(cloth.action_settings.len(), 20);
    assert!((cloth.pull_motion.x - 0.03).abs() < 1e-6);
    assert!((cloth.pull_motion.y - 0.015).abs() < 1e-6);
    assert!((cloth.pull_decay - 0.99).abs() < 1e-6);
    assert_eq!(cloth.upward_motion, bevy::prelude::Vec2::new(-2.0, 5.0));
    assert!(cloth.vertex_radius.iter().all(|r| (0.01..=0.07001).contains(r)));
    assert!(cloth.colliders.iter().all(|c| m.skeleton.iter().any(|b| b.bone_id == c.bone_id)
        && c.local_start.is_finite() && c.local_end.is_finite() && (0.05..0.2).contains(&c.radius)));
}

#[test]
fn skirt_rotation_references_are_resolved_and_malformed_sources_rejected() {
    if !install_present() { eprintln!("skipped: game install not found"); return; }
    let mut forge = Forge::open(&game_dir().join("DataPC.forge")).unwrap();
    let entry = forge.find("Rank 9").cloned().unwrap();
    let resources = forge.resources(&entry).unwrap();
    let payload = &resources.iter().find(|r| r.name == "UCMA_Altair_Skirt" && r.class_hash == crc32("Skeleton")).unwrap().payload;
    let copies = crate::visual_pose::decode_rotation_copies(payload).unwrap();
    assert_eq!(copies, vec![(2339535765, 743623600), (3796064396, 743623600)]);
    let class = crc32("RotationPasteModifier").to_le_bytes();
    let marker = payload.windows(4).position(|b| b == class).unwrap();
    let mut invalid = payload.clone();
    invalid[marker + 11..marker + 15].copy_from_slice(&0u32.to_le_bytes());
    assert!(crate::visual_pose::decode_rotation_copies(&invalid).is_err());
    assert!(crate::visual_pose::decode_rotation_copies(&payload[..marker + 13]).is_err());
    let compressions = crate::visual_pose::decode_compressions(payload).unwrap();
    assert_eq!(compressions.len(), 1);
    assert_eq!(compressions[0].target, 1206536969);
    assert_eq!(compressions[0].sources, [1486391408, 2601793068]);
    assert_eq!(compressions[0].position_weight, 0.5);
    assert_eq!(compressions[0].rotation_weight, 0.5);
    assert!(compressions[0].position && compressions[0].rotation);
    let marker = payload.windows(4).position(|b| b == crc32("CompressBoneModifier").to_le_bytes()).unwrap();
    let mut invalid = payload.clone();
    invalid[marker + 10..marker + 14].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(crate::visual_pose::decode_compressions(&invalid).is_err());
    invalid = payload.clone();
    invalid[marker + 28..marker + 32].copy_from_slice(&1.0f32.to_le_bytes());
    assert!(crate::visual_pose::decode_compressions(&invalid).is_err());
    assert!(crate::visual_pose::decode_compressions(&payload[..marker + 93]).is_err());
    let look_at = crate::visual_pose::decode_look_at(payload).unwrap();
    assert_eq!(look_at.len(), 1);
    assert_eq!(look_at[0].target, 1206536969);
    assert_eq!(look_at[0].aim, 3738240529);
    let marker = payload.windows(4).position(|b| b == crc32("LookAtBoneModifier").to_le_bytes()).unwrap();
    let mut invalid = payload.clone();
    invalid[marker + 36..marker + 40].copy_from_slice(&5u32.to_le_bytes());
    assert!(crate::visual_pose::decode_look_at(&invalid).is_err());
    invalid = payload.clone();
    invalid[marker + 16..marker + 20].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(crate::visual_pose::decode_look_at(&invalid).is_err());
    assert!(crate::visual_pose::decode_look_at(&payload[..marker + 39]).is_err());
}
