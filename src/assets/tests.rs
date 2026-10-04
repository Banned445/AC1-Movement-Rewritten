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
