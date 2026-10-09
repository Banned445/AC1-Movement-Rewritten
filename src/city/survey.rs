//! Manual data survey of a world archive (ignored test; prints statistics used by RE/17).
//! `AC_SURVEY_HASHES=<file of "hash name" lines> cargo test --release city_survey -- --ignored --nocapture`

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::assets::forge::crc32;

#[test]
#[ignore]
fn city_survey() {
    let Some(game) = crate::assets::find_game_dir() else { return };
    let archive = std::env::var("AC_SURVEY_ARCHIVE").unwrap_or("DataPC_Damascus.forge".into());
    let names: HashMap<u32, String> = std::env::var("AC_SURVEY_HASHES").ok().and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.lines().filter_map(|l| { let (h, n) = l.split_once(' ')?; Some((u32::from_str_radix(h, 16).ok()?, n.to_string())) }).collect())
        .unwrap_or_default();
    let name = |h: u32| names.get(&h).cloned().unwrap_or(format!("{h:08x}"));
    let t = std::time::Instant::now();
    let archives = super::archive::Archives::open(&game, &[&archive, "DataPC_Common.forge", "DataPC.forge"]).unwrap();
    println!("index built in {:?}", t.elapsed());
    let forge = &archives.forges[0];
    let mut top = BTreeMap::<String, usize>::new();
    let mut component = BTreeMap::<String, usize>::new();
    let mut samples = BTreeMap::<String, Vec<String>>::new();
    let mut members_total = 0;
    let mut missing = HashMap::<String, usize>::new();
    let mut prefixes = BTreeMap::<String, usize>::new();
    let entity = crc32("Entity").to_le_bytes();
    let t = std::time::Instant::now();
    let mut raw = 0usize;
    for e in forge.entries.iter().filter(|e| e.name.starts_with("Cell")) {
        let res = forge.resources_shared(e).unwrap();
        raw += res.iter().map(|r| r.payload.len()).sum::<usize>();
        for r in &res {
            *top.entry(name(r.class_hash)).or_default() += 1;
            if r.class_hash != crc32("Entity") && r.class_hash != crc32("EntityGroup") { continue; }
            // split inline members: [u32 id][u32 Entity] (RE/15 §3)
            let mut starts = vec![0usize];
            if r.class_hash == crc32("EntityGroup") {
                for p in 12..r.payload.len().saturating_sub(4) {
                    if r.payload[p..p + 4] == entity { starts.push(p - 4); }
                }
                members_total += starts.len() - 1;
            }
            for (k, &s) in starts.iter().enumerate() {
                let end = starts.get(k + 1).copied().unwrap_or(r.payload.len());
                let body = &r.payload[s..end];
                let mut seen = HashSet::new();
                for p in 8..body.len().saturating_sub(3) {
                    let h = u32::from_le_bytes(body[p..p + 4].try_into().unwrap());
                    if names.contains_key(&h) && seen.insert(h) {
                        let n = name(h);
                        *component.entry(n.clone()).or_default() += 1;
                        let list = samples.entry(n).or_default();
                        if list.len() < 6 { list.push(r.name.clone()); }
                    }
                    // referenced resource ids that no archive holds
                }
                let prefix: String = r.name.split(['_', ' ']).next().unwrap_or("").to_string();
                *prefixes.entry(prefix).or_default() += 1;
            }
            // references: words at the Visual marker + 5
            let vis = crc32("Visual").to_le_bytes();
            for p in r.payload.windows(4).enumerate().filter(|(_, w)| *w == vis).map(|(p, _)| p) {
                if let Some(b) = r.payload.get(p + 5..p + 9) {
                    let id = u32::from_le_bytes(b.try_into().unwrap());
                    if id != 0 && !archives.contains(id) { *missing.entry("visual drawable".into()).or_default() += 1; }
                }
            }
        }
    }
    println!("cells read in {:?}, {} MB inflated", t.elapsed(), raw >> 20);
    println!("== top-level classes\n{top:#?}");
    println!("== components (entities containing the class hash)");
    for (k, v) in &component { println!("{v:7} {k:40} {:?}", samples.get(k).unwrap()); }
    println!("== group members {members_total}; missing refs {missing:?}");
    let mut p: Vec<_> = prefixes.into_iter().collect();
    p.sort_by_key(|x| std::cmp::Reverse(x.1));
    println!("== name prefixes {:?}", &p[..p.len().min(120)]);
}

/// Layout checks for the entity decoder: marker multiplicity per body, referenced classes, field offsets.
#[test]
#[ignore]
fn city_survey_layouts() {
    let Some(game) = crate::assets::find_game_dir() else { return };
    let archive = std::env::var("AC_SURVEY_ARCHIVE").unwrap_or("DataPC_Damascus.forge".into());
    let archives = super::archive::Archives::open(&game, &[&archive, "DataPC_Common.forge", "DataPC.forge"]).unwrap();
    let cls = |id: u32| archives.get(id).map(|r| r.class_hash).unwrap_or(0);
    let hn = |h: u32| ["Mesh","LODSelector","DynamicMesh","MeshShape","BoxShape","BarrelShape","CapsuleShape","ListShape","SphereShape","TransformShape","ConvexVerticesShape","HeightFieldShape","ReferenceListShape","WrappedShape","SubMeshShape","Entity","GuidanceSystem"]
        .iter().find(|n| crc32(n) == h).map(|s| s.to_string()).unwrap_or(format!("{h:08x}"));
    let mut hist = BTreeMap::<String, usize>::new();
    let mut add = |k: String| *hist.entry(k).or_default() += 1;
    let mut dumps = BTreeMap::<String, usize>::new();
    let entity = crc32("Entity").to_le_bytes();
    let forge = &archives.forges[0];
    let find = |b: &[u8], n: &str| { let h = crc32(n).to_le_bytes(); b.windows(4).enumerate().filter(|(_, w)| *w == h).map(|(p, _)| p).collect::<Vec<_>>() };
    for e in forge.entries.iter().filter(|e| e.name.starts_with("Cell")) {
        for r in forge.resources_shared(e).unwrap() {
            if r.class_hash != crc32("Entity") && r.class_hash != crc32("EntityGroup") { continue; }
            let mut starts = vec![0usize];
            if r.class_hash == crc32("EntityGroup") {
                for p in 12..r.payload.len().saturating_sub(4) { if r.payload[p..p + 4] == entity { starts.push(p - 4); } }
            }
            for (k, &s) in starts.iter().enumerate() {
                let end = starts.get(k + 1).copied().unwrap_or(r.payload.len());
                let b = &r.payload[s..end];
                let v = find(b, "Visual"); let mi = find(b, "MeshInstanceData"); let li = find(b, "LODSelectorInstance");
                let ic = find(b, "InertComponent"); let rb = find(b, "RigidBody"); let rbc = find(b, "RigidBodyComponent"); let gs = find(b, "GuidanceSystem");
                add(format!("count visual={} lodinst={} meshinst={}", v.len(), li.len(), mi.len()));
                add(format!("count inert={} rigidbody={} rbcomp={} guidance={}", ic.len(), rb.len(), rbc.len(), gs.len()));
                for &p in &v {
                    let active = b.get(p + 4).copied().unwrap_or(9);
                    let id = b.get(p + 5..p + 9).map(|x| u32::from_le_bytes(x.try_into().unwrap())).unwrap_or(0);
                    add(format!("visual active={active} drawable={}", if id == 0 { "null".into() } else { hn(cls(id)) }));
                    let key = format!("visual bytes {}", hn(cls(id)));
                    if *dumps.entry(key.clone()).or_default() < 3 { *dumps.get_mut(&key).unwrap() += 1;
                        println!("{key} {}: {}", r.name, b[p..(p + 60).min(b.len())].iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")); }
                }
                for &p in &rb {
                    let filt = b.get(p + 8..p + 12).map(|x| u32::from_le_bytes(x.try_into().unwrap())).unwrap_or(0);
                    let shape = b.get(p + 20..p + 24).map(|x| u32::from_le_bytes(x.try_into().unwrap())).unwrap_or(0);
                    add(format!("rigidbody +8 filter={} shape={}", filt == crc32("CollisionFilterInfo"), if shape == 0 { "null".into() } else { hn(cls(shape)) }));
                    let layer = b.get(p + 12).copied().unwrap_or(255) & 0x3f;
                    add(format!("rigidbody layer {layer:02}"));
                }
                for &p in &ic {
                    let rel: Vec<_> = rb.iter().map(|&q| q as i64 - p as i64).collect();
                    add(format!("inert->rigidbody offsets {rel:?}"));
                    let g = b.get(p + 126..p + 130).map(|x| u32::from_le_bytes(x.try_into().unwrap())).unwrap_or(0);
                    let gid = gs.first().and_then(|&q| b.get(q - 4..q)).map(|x| u32::from_le_bytes(x.try_into().unwrap()));
                    add(format!("inert+126 guidance handle {}", if g == 0 { "null" } else if Some(g) == gid { "matches" } else { "other" }));
                }
                for &p in &mi {
                    let id = b.get(p + 5..p + 9).map(|x| u32::from_le_bytes(x.try_into().unwrap())).unwrap_or(0);
                    add(format!("meshinstance +5 = {}", hn(cls(id))));
                }
            }
        }
    }
    for (k, v) in hist { println!("{v:8} {k}"); }
}

#[test]
#[ignore]
fn city_survey_lods() {
    let Some(game) = crate::assets::find_game_dir() else { return };
    let archives = super::archive::Archives::open(&game, &["DataPC_Damascus.forge", "DataPC_Common.forge", "DataPC.forge"]).unwrap();
    let mut hist = BTreeMap::<String, usize>::new();
    let mut seen = HashSet::new();
    for (a, f) in archives.forges.iter().enumerate().take(2) {
        for e in &f.entries {
            let Ok(res) = archives.file(a, e.index) else { continue };
            for r in res.values().filter(|r| r.class_hash == crc32("LODSelector")) {
                if !seen.insert(r.id) { continue; }
                let d = &r.payload;
                let mut desc = Vec::new();
                for k in 0..5 {
                    let p = 12 + 21 * k;
                    let g = u32::from_le_bytes(d[p + 8..p + 12].try_into().unwrap());
                    let dist = f32::from_le_bytes(d[p + 12..p + 16].try_into().unwrap());
                    let f2 = f32::from_le_bytes(d[p + 16..p + 20].try_into().unwrap());
                    let flag = d[p + 20];
                    let name = if g == 0 { "-".to_string() } else { archives.get(g).map(|m| m.name.rsplit('_').take(2).collect::<Vec<_>>().join("<")).unwrap_or("?".into()) };
                    desc.push(format!("{name}@{dist}/{f2}/{flag}"));
                }
                *hist.entry(desc.iter().map(|s| s.split('@').nth(1).unwrap().to_string()).collect::<Vec<_>>().join(" ")
                    + "  |  " + &desc.iter().map(|s| if s.starts_with('-') { "-" } else { "M" }).collect::<String>() + &format!(" tail {:?}", &d[12 + 105..])).or_default() += 1;
            }
        }
    }
    let mut v: Vec<_> = hist.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    for (k, n) in v.iter().take(40) { println!("{n:6} {k}"); }
}

#[test]
#[ignore]
fn city_survey_dump_class() {
    let Some(game) = crate::assets::find_game_dir() else { return };
    let class = std::env::var("AC_SURVEY_CLASS").unwrap_or("CapsuleShape".into());
    let n: usize = std::env::var("AC_SURVEY_N").ok().and_then(|v| v.parse().ok()).unwrap_or(2);
    let archives = super::archive::Archives::open(&game, &["DataPC_Damascus.forge", "DataPC_Common.forge", "DataPC.forge"]).unwrap();
    let mut shown = 0;
    'outer: for e in &archives.forges[0].entries {
        for r in archives.forges[0].resources_shared(e).unwrap() {
            if r.class_hash == crc32(&class) || r.name.contains(&class) {
                println!("== {} {} {} bytes", e.name, r.name, r.payload.len());
                for (i, c) in r.payload.chunks(16).enumerate().take(40) {
                    let f: Vec<String> = c.chunks(4).filter(|x| x.len() == 4).map(|x| format!("{:10.4}", f32::from_le_bytes(x.try_into().unwrap()))).collect();
                    println!("{:05x} {:48} {}", i * 16, c.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "), f.join(" "));
                }
                shown += 1;
                if shown >= n { break 'outer; }
            }
        }
    }
}

#[test]
#[ignore]
fn city_survey_names() {
    let Some(game) = crate::assets::find_game_dir() else { return };
    let pat: Vec<String> = std::env::var("AC_SURVEY_PAT").unwrap_or("Start".into()).split(',').map(|s| s.to_lowercase()).collect();
    let archives = super::archive::Archives::open(&game, &["DataPC_Damascus.forge", "DataPC_Common.forge", "DataPC.forge"]).unwrap();
    for e in &archives.forges[0].entries {
        for r in archives.forges[0].resources_shared(e).unwrap() {
            let l = r.name.to_lowercase();
            if pat.iter().any(|p| l.contains(p.as_str())) {
                let pos = if r.class_hash == crc32("Entity") || r.class_hash == crc32("EntityGroup") {
                    let m: Vec<f32> = (0..16).map(|i| f32::from_le_bytes(r.payload[8 + 4 * i..12 + 4 * i].try_into().unwrap())).collect();
                    format!("({:.1}, {:.1}, {:.1})", m[12], m[13], m[14])
                } else { String::new() };
                println!("{} {:08x} {} {}", e.name, r.class_hash, r.name, pos);
            }
        }
    }
}

#[test]
#[ignore]
fn city_survey_misc() {
    let Some(game) = crate::assets::find_game_dir() else { return };
    let archives = super::archive::Archives::open(&game, &["DataPC_Damascus.forge", "DataPC_Common.forge", "DataPC.forge"]).unwrap();
    let mut shown = 0;
    let mut mats = HashSet::new();
    for e in &archives.forges[0].entries {
        for r in archives.forges[0].resources_shared(e).unwrap() {
            if r.class_hash == crc32("Material") && r.name.contains("Water") && mats.insert(r.name.clone()) {
                let w = |p: usize| u32::from_le_bytes(r.payload[p..p + 4].try_into().unwrap());
                let t = archives.get(w(8)).map(|x| x.name.clone()).unwrap_or_default();
                let set = archives.get(w(12)).unwrap();
                let specs: Vec<String> = (8..set.payload.len().saturating_sub(3)).step_by(4).filter_map(|p| {
                    let id = u32::from_le_bytes(set.payload[p..p + 4].try_into().unwrap());
                    archives.get(id).ok().filter(|s| s.class_hash == crc32("TextureMapSpec")).map(|s| s.name.clone())
                }).collect();
                println!("material {} template {t} specs {specs:?}", r.name);
            }
            if r.class_hash != crc32("Entity") && r.class_hash != crc32("EntityGroup") { continue; }
            for b in super::decode::bodies(&r) {
                if crate::assets::world::entity_transform(b).is_err() && shown < 12 {
                    shown += 1;
                    let m: Vec<f32> = (0..16).map(|i| f32::from_le_bytes(b[8 + 4 * i..12 + 4 * i].try_into().unwrap())).collect();
                    println!("non-affine {} {:?}", r.name, m);
                }
            }
        }
    }
}

#[test]
#[ignore]
fn city_survey_mesh() {
    let Some(game) = crate::assets::find_game_dir() else { return };
    let want = std::env::var("AC_SURVEY_PAT").unwrap_or("Hay_Bale_01".into());
    let archives = super::archive::Archives::open(&game, &["DataPC_Damascus.forge", "DataPC_Common.forge", "DataPC.forge"]).unwrap();
    for (a, f) in archives.forges.iter().enumerate() {
        for e in &f.entries {
            let Ok(res) = archives.file(a, e.index) else { continue };
            for r in res.values().filter(|r| r.name == want) {
                let d = &r.payload;
                let w = |p: usize| d.get(p..p + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap())).unwrap_or(0);
                println!("== {} class {:08x} len {} words {:?}", r.name, r.class_hash, d.len(), (0..12).map(|k| format!("{:08x}", w(4 * k))).collect::<Vec<_>>());
                if let Some(cm) = (8..d.len().saturating_sub(4)).find(|&o| w(o) == crate::assets::ac_formats::CLASS_COMPILED_MESH) {
                    println!("compiled at {cm}: {:?}", (0..10).map(|k| w(cm + 4 + 4 * k)).collect::<Vec<_>>());
                }
                println!("parse_mesh ok: {}", crate::assets::ac_formats::parse_mesh(d).is_some());
                if r.class_hash == crc32("TextureMap") { println!("tex words {:?}", (0..16).map(|k| w(4 * k)).collect::<Vec<_>>()); }
            }
        }
    }
}

#[test]
#[ignore]
fn city_survey_fake_vertices() {
    let Some(game) = crate::assets::find_game_dir() else { return };
    let w = super::open(&game, &super::DAMASCUS, true).unwrap();
    let r = w.globals.iter().find(|r| r.name == std::env::var("AC_SURVEY_PAT").unwrap_or("Damascus_FakeMesh_26".into())).unwrap();
    let d = &r.payload;
    let word = |p: usize| u32::from_le_bytes(d[p..p + 4].try_into().unwrap());
    println!("{} header {:?}", r.name, (0..12).map(|k| format!("{:08x}", word(4 * k))).collect::<Vec<_>>());
    let blob = &d[34..];
    let (vb, ib) = (u32::from_le_bytes(blob[8..12].try_into().unwrap()) as usize, u32::from_le_bytes(blob[12..16].try_into().unwrap()) as usize);
    println!("blob words {:?}", (0..9).map(|k| u32::from_le_bytes(blob[4 * k..4 * k + 4].try_into().unwrap())).collect::<Vec<_>>());
    let mut ws = BTreeMap::<i16, usize>::new();
    for (i, v) in blob[36..36 + vb].chunks_exact(24).enumerate() {
        let s = |p: usize| i16::from_le_bytes([v[p], v[p + 1]]);
        *ws.entry(s(6)).or_default() += 1;
        if i < 12 || i % 500 == 0 { println!("v{i}: {:?} uv {:?} bytes {:02x?}", [s(0), s(2), s(4), s(6)], [s(20), s(22)], &v[8..20]); }
    }
    println!("w histogram (first 20) {:?}", ws.iter().take(20).collect::<Vec<_>>());
    println!("vb {vb} ib {ib} vertices {}", vb / 24);
}

#[test]
#[ignore]
fn city_survey_draw_records() {
    let Some(game) = crate::assets::find_game_dir() else { return };
    let w = super::open(&game, &super::DAMASCUS, true).unwrap();
    let a = &w.data.archives;
    for name in ["Damascus_FakeMesh_26", "Damascus_FakeMesh_9"] {
        let r = w.globals.iter().find(|r| r.name == name).unwrap();
        dump(&r.payload, name);
    }
    for e in &a.forges[0].entries {
        if e.name != "Cell00500_DataBlock" { continue; }
        for r in a.forges[0].resources_shared(e).unwrap() {
            if r.class_hash == crc32("Mesh") && (r.name.starts_with("Corner_M_B") || r.name.starts_with("House_P")) { dump(&r.payload, &r.name); }
        }
    }
    fn dump(d: &[u8], name: &str) {
        let blob = &d[34..];
        let u = |p: usize| u32::from_le_bytes(blob[p..p + 4].try_into().unwrap());
        let (vb, ib, count, groups) = (u(8) as usize, u(12) as usize, u(24) as usize, u(28) as usize);
        let idx: Vec<u16> = blob[36 + vb..36 + vb + ib].chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
        println!("== {name}: {} vertices, {} indices, {count} draws, {groups} groups, max index {}", vb / 24, idx.len(), idx.iter().max().unwrap());
        let t = 36 + vb + ib;
        for k in 0..count + groups {
            let p = t + 20 * k;
            let rec: Vec<u32> = (0..5).map(|j| u(p + 4 * j)).collect();
            let (first, tris) = (rec[3] as usize, rec[4] as usize);
            let sl = idx.get(first..first + 3 * tris).unwrap_or(&[]);
            println!("  rec {k}: {rec:?} index range {:?}", (sl.iter().min(), sl.iter().max()));
        }
    }
}
