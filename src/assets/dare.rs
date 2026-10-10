//! DARE sound data, the sound engine inside the exe: the SoundBao objects in DataPC.forge ("Game Fix" and "Game
//! Bootstrap Settings"), the event graph that decides what plays, the random containers and the IMA-ADPCM decoder.
//!
//! A SoundBao payload is a BAO file: version 0x00011B01, a 36-byte header (type at +0x20: 0x10000000 event,
//! 0x20000000 sound, 0x30000000 audio resource), then the object. Offsets below are from the object start (BAO +0x24).
//! The engine reads an object from its +4 (`_SND_tdstBlockEvent`: +0 id, +4 type), so a code offset k is object
//! offset k + 4.
//! - Event: +8 type (`SND_fn_bInitBinEvent` 0x7E4310). 1 = play a sound (+0xC sound id, +0x10 random range, 16.16);
//!   11 = SwitchEvent (+0x10 switch id, +0x14 default event, +0x1C case count, cases at +0x68: event, -, value;
//!   0x7DD4B0); 12 = MultiEvent (+0x10 count, events at +0x68, all played at once; 0x7DF640).
//! - Sound: +8 type. 1 = one resource (+0x20 resource id, +0x48 channels, +0x4C rate, +0x50 bytes/s, +0x54 frames,
//!   +0x58 resource size); 4 = random container (+0x20 count, entries at +0x84: sound id, weight 16.16, repeat flag,
//!   -; picked by 0x80D170).
//! - Audio resource: +4 the TImaAdpcm header (28 bytes, 0x839150), then the first frames as PCM, then the nibbles.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::forge::Forge;

const BAO_MAGIC: [u8; 4] = [0x01, 0x1B, 0x01, 0x00];
/// The object starts after the 36-byte header.
const OBJECT: usize = 0x24;
/// CRC32("SoundBao").
pub const CLASS_SOUND_BAO: u32 = 0xD829_5DCB;
/// CRC32("ContactSound"): one sound of a ContactTable (an AudioEvent and a ContactEventTypeHuman).
const CLASS_CONTACT_SOUND: u32 = 0x54A4_C59C;
/// The class of ContactTable_AlTair's groups (one per surface material: +0 u32 material, +4 u32 15, the ContactSounds).
const CLASS_CONTACT_GROUP: u32 = 0xE1CB_F60F;
/// Nested-call guard for a broken graph.
const MAX_DEPTH: u32 = 8;

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn i16_at(b: &[u8], o: usize) -> Option<i16> {
    Some(i16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn fixed(v: u32) -> f32 {
    v as i32 as f32 / 65536.0
}

/// The object bytes of a SoundBao payload. The wrapper in front of the BAO varies (a list of dependency ids on some);
/// the BAO is the magic preceded by a u32 holding the remaining length.
pub fn bao_object(payload: &[u8]) -> Option<&[u8]> {
    let m = (4..payload.len().saturating_sub(4))
        .find(|&m| payload[m..m + 4] == BAO_MAGIC && u32_at(payload, m - 4) == Some((payload.len() - m) as u32))?;
    payload.get(m + OBJECT..)
}

/// The BAO id in a SoundBao resource name (`BAO_0x1032061a`).
pub fn bao_id(name: &str) -> Option<u32> {
    u32::from_str_radix(name.strip_prefix("BAO_0x")?, 16).ok()
}

/// One sound to start.
#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    /// The audio resource (BAO id).
    pub res: u32,
    pub channels: u16,
    pub rate: u32,
    pub frames: u32,
    /// Sum of the sound objects' +0x18 on the way down (16.16).
    /// PORT (hypothesis): read as a gain in dB (containers carry -2 .. -20, single sounds mostly 0); not traced.
    pub gain_db: f32,
    /// The play event's random factor (0x7DC930: 1 - r + rand * 2r).
    /// PORT (hypothesis): applied as playback speed (pitch); the voice field it fills (+36) is not traced further.
    pub speed: f32,
}

/// The random source and each container's last pick (0x80D170 keeps it at +44 of the container).
pub struct Picker {
    state: u32,
    last: HashMap<u32, usize>,
}

impl Picker {
    pub fn new(seed: u32) -> Self {
        Picker { state: seed.max(1), last: HashMap::new() }
    }
    /// `sub_7F7B20(0x10000)`: rand() / 0x7FFF scaled to 16.16, so 0 ..= 65536.
    /// PORT: an xorshift stands in for the CRT rand().
    fn rand16(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        ((x >> 17) & 0x7FFF) * 65536 / 0x7FFF
    }
}

/// The sound objects of the install, by BAO id, and the ContactTable's events.
#[derive(Default)]
pub struct SoundBank {
    objects: HashMap<u32, Arc<[u8]>>,
    /// SoundBao forge resource id -> BAO id (anim AudioEvents reference the resource).
    by_resource: HashMap<u32, u32>,
    /// ContactEventTypeHuman -> event BAO id (ContactTable_AlTair, material group 0: every group lists the same
    /// events; the material is the events' switch 0).
    pub contact: [Option<u32>; 15],
}

impl SoundBank {
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn insert(&mut self, resource: u32, name: &str, payload: &[u8]) {
        if let (Some(id), Some(obj)) = (bao_id(name), bao_object(payload)) {
            self.objects.insert(id, obj.into());
            self.by_resource.insert(resource, id);
        }
    }

    /// The event an anim AudioEvent points at (its sound reference is the SoundBao resource).
    pub fn event_for_resource(&self, resource: u32) -> Option<u32> {
        self.by_resource.get(&resource).copied()
    }

    pub fn object(&self, id: u32) -> Option<&[u8]> {
        self.objects.get(&id).map(|o| &o[..])
    }

    /// Every voice an event starts. `switch` gives a switch's current value (the game asks the sound object's type
    /// callback, `dword_1A16DD8 + 188 * type + 16`, called from 0x7DF4A0).
    pub fn resolve(&self, event: u32, switch: &dyn Fn(u32) -> Option<u32>, pick: &mut Picker) -> Vec<Voice> {
        let mut out = Vec::new();
        self.event(event, switch, pick, 0, &mut out);
        out
    }

    fn event(&self, id: u32, switch: &dyn Fn(u32) -> Option<u32>, pick: &mut Picker, depth: u32, out: &mut Vec<Voice>) {
        let Some(o) = self.object(id) else { return };
        if depth > MAX_DEPTH || id >> 28 != 1 {
            return;
        }
        match u32_at(o, 8) {
            Some(1) => {
                let (Some(sound), Some(r)) = (u32_at(o, 0xC), u32_at(o, 0x10)) else { return };
                let r = fixed(r);
                let speed = 1.0 - r + pick.rand16() as f32 / 65536.0 * 2.0 * r;
                self.sound(sound, 0.0, speed, pick, depth + 1, out);
            }
            Some(11) => {
                let (Some(var), Some(default), Some(n)) = (u32_at(o, 0x10), u32_at(o, 0x14), u32_at(o, 0x1C)) else { return };
                let cases: Vec<(u32, u32)> =
                    (0..n as usize).filter_map(|j| Some((u32_at(o, 0x68 + 12 * j)?, u32_at(o, 0x68 + 12 * j + 8)?))).collect();
                // PORT: the port has no switch values yet (surface material etc.); without one the default plays,
                // or the first case when there is no default.
                let chosen = switch(var)
                    .and_then(|v| cases.iter().find(|c| c.1 == v).map(|c| c.0))
                    .or_else(|| self.objects.contains_key(&default).then_some(default))
                    .or_else(|| cases.first().map(|c| c.0));
                if let Some(e) = chosen {
                    self.event(e, switch, pick, depth + 1, out);
                }
            }
            Some(12) => {
                let Some(n) = u32_at(o, 0x10) else { return };
                for j in 0..n as usize {
                    if let Some(e) = u32_at(o, 0x68 + 4 * j) {
                        self.event(e, switch, pick, depth + 1, out);
                    }
                }
            }
            // stop / volume / other control events: nothing to start
            _ => {}
        }
    }

    fn sound(&self, id: u32, gain_db: f32, speed: f32, pick: &mut Picker, depth: u32, out: &mut Vec<Voice>) {
        let Some(o) = self.object(id) else { return };
        if depth > MAX_DEPTH || id >> 28 != 2 {
            return;
        }
        let gain_db = gain_db + u32_at(o, 0x18).map_or(0.0, fixed);
        match u32_at(o, 8) {
            Some(1) => {
                let (Some(res), Some(ch), Some(rate), Some(frames)) = (u32_at(o, 0x20), u32_at(o, 0x48), u32_at(o, 0x4C), u32_at(o, 0x54))
                else {
                    return;
                };
                if self.objects.contains_key(&res) && (1..=2).contains(&ch) && rate > 0 {
                    out.push(Voice { res, channels: ch as u16, rate, frames, gain_db, speed });
                }
            }
            Some(4) => {
                let Some(n) = u32_at(o, 0x20) else { return };
                let entries: Vec<(u32, u32, u32)> = (0..n as usize)
                    .filter_map(|j| Some((u32_at(o, 0x84 + 16 * j)?, u32_at(o, 0x84 + 16 * j + 4)?, u32_at(o, 0x84 + 16 * j + 8)?)))
                    .collect();
                if let Some(i) = pick_random(&entries, pick.last.get(&id).copied(), pick.rand16()) {
                    pick.last.insert(id, i);
                    self.sound(entries[i].0, gain_db, speed, pick, depth + 1, out);
                }
            }
            // PORT: the other sound types (multilayer, sequence, ...) are not used by the player's events
            _ => {}
        }
    }
}

/// 0x80D170 (weighted mode): walk the weights with `r` (0 ..= 65536); the last pick is skipped unless its repeat flag
/// (+8) is set, its weight still counted; nothing found -> entry 0.
/// PORT: the container's silence chance and its shuffle mode (0x80D080) are fields not located in the file layout;
/// the weighted mode without silence is used.
fn pick_random(entries: &[(u32, u32, u32)], last: Option<usize>, r: u32) -> Option<usize> {
    if entries.is_empty() {
        return None;
    }
    let skip = last.filter(|&l| entries.get(l).is_some_and(|e| e.2 == 0));
    let mut v = r as i64;
    for (i, e) in entries.iter().enumerate() {
        if v <= e.1 as i64 && skip != Some(i) {
            return Some(i);
        }
        v -= e.1 as i64;
    }
    Some(0)
}

const IMA_INDEX: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];
const IMA_STEP: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118,
    130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060,
    1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132,
    7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

/// One IMA nibble (0x841BA0 / 0x841C90: the standard tables at 0x16A9BD0 / 0x16A9C10).
fn ima_step(n: u8, pred: &mut i32, index: &mut i32) -> i16 {
    let step = IMA_STEP[*index as usize];
    let diff = (step * (2 * (n & 7) as i32 + 1)) >> 3;
    *pred = if n & 8 != 0 { *pred - diff } else { *pred + diff }.clamp(-32768, 32767);
    *index = (*index + IMA_INDEX[n as usize]).clamp(0, 88);
    *pred as i16
}

/// Decode an audio resource object to interleaved PCM (`TImaAdpcm`, 0x839150). Header at +4: u8 version (5), +12 u8
/// stereo, +14 u16 PCM frames in front, +16 i16 / +18 u8 predictor and step index (left), +20 / +22 (right), +24 u8
/// a trailing sample follows, +26 i16 that sample. Then the PCM frames, then one byte per two mono samples (high
/// nibble first) or per stereo frame (high nibble left).
pub fn decode_ima(obj: &[u8], channels: u16, frames: u32) -> Option<Vec<i16>> {
    let h = obj.get(4..32)?;
    if h[0] != 5 || (h[12] == 1) != (channels == 2) {
        return None;
    }
    let ch = channels as usize;
    let raw = i16_at(h, 14)? as u16 as usize;
    let trailing = (h[24] == 1 && ch == 1).then(|| i16_at(h, 26)).flatten();
    let mut pred = [i16_at(h, 16)? as i32, i16_at(h, 20)? as i32];
    let mut index = [(h[18] as i32).clamp(0, 88), (h[22] as i32).clamp(0, 88)];
    let total = frames as usize * ch;
    let mut out = Vec::with_capacity(total);
    let mut p = 32;
    for _ in 0..(raw * ch).min(total) {
        out.push(i16_at(obj, p)?);
        p += 2;
    }
    let end = total - trailing.is_some() as usize;
    while out.len() < end {
        let Some(&b) = obj.get(p) else { break };
        p += 1;
        if ch == 1 {
            out.push(ima_step(b >> 4, &mut pred[0], &mut index[0]));
            if out.len() < end {
                out.push(ima_step(b & 15, &mut pred[0], &mut index[0]));
            }
        } else {
            out.push(ima_step(b >> 4, &mut pred[0], &mut index[0]));
            out.push(ima_step(b & 15, &mut pred[1], &mut index[1]));
        }
    }
    out.extend(trailing);
    Some(out)
}

/// The ContactTable's event per ContactEventTypeHuman (ContactTable_AlTair in "Rank 9": 24 material groups of 15
/// ContactSounds, each an AudioEvent and the contact type).
/// PORT: the ContactSounds of the first group are found by their class hash rather than by walking the table's
/// serializer (not traced); the groups list the same events, so the group does not matter.
pub fn contact_events(table: &[u8]) -> [Option<u32>; 15] {
    let mut out = [None; 15];
    let class = CLASS_CONTACT_SOUND.to_le_bytes();
    let group = CLASS_CONTACT_GROUP.to_le_bytes();
    let mut groups = 0;
    for p in 0..table.len().saturating_sub(4) {
        if table[p..p + 4] == group {
            groups += 1;
            if groups > 1 {
                break;
            }
        }
        if table[p..p + 4] == class {
            // ContactSound: +4 u32, then the AudioEvent (class, handle, u32, sound reference: objId, class, BAO id,
            // resource id), u8, then u32 ContactEventTypeHuman
            if let (Some(bao), Some(&ty)) = (u32_at(table, p + 28), table.get(p + 37)) {
                if let Some(slot) = out.get_mut(ty as usize) {
                    slot.get_or_insert(bao);
                }
            }
        }
    }
    out
}

/// Every SoundBao in "Game Fix" and "Game Bootstrap Settings", and the player's ContactTable from "Rank 9".
pub fn load_sound_bank(game_dir: &Path) -> Result<SoundBank, String> {
    let path = game_dir.join("DataPC.forge");
    let forge = Forge::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut bank = SoundBank::default();
    for file in ["Game Fix", "Game Bootstrap Settings"] {
        let entry = forge.find(file).cloned().ok_or(format!("file '{file}' not found"))?;
        for r in forge.resources_shared(&entry).map_err(|e| e.to_string())? {
            if r.class_hash == CLASS_SOUND_BAO {
                bank.insert(r.id, &r.name, &r.payload);
            }
        }
    }
    let rank9 = forge.find("Rank 9").cloned().ok_or("file 'Rank 9' not found")?;
    if let Some(t) = forge.resources_shared(&rank9).map_err(|e| e.to_string())?.into_iter().find(|r| r.name == "ContactTable_AlTair") {
        bank.contact = contact_events(&t.payload);
    }
    Ok(bank)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ima_nibbles_follow_the_standard_tables() {
        // predictor 0, index 0: nibble 7 = +(7 * 15) >> 3 = 13, index 0 + 8; nibble 15 = the same step down
        let (mut p, mut i) = (0, 0);
        assert_eq!(ima_step(7, &mut p, &mut i), 13);
        assert_eq!(i, 8);
        assert_eq!(ima_step(15, &mut p, &mut i), 13 - ((16 * 15) >> 3));
        assert_eq!(i, 16);
        let (mut p, mut i) = (32760, 88);
        assert_eq!(ima_step(7, &mut p, &mut i), 32767, "clamped");
    }

    #[test]
    fn decode_reads_the_pcm_frames_then_the_nibbles_high_first() {
        let mut obj = vec![0u8; 4];
        let mut h = [0u8; 28];
        h[0] = 5;
        h[14] = 2; // two PCM frames
        h[16] = 10; // predictor 10, index 0
        h[24] = 1; // trailing sample
        h[26] = 99;
        obj.extend_from_slice(&h);
        obj.extend_from_slice(&5i16.to_le_bytes());
        obj.extend_from_slice(&10i16.to_le_bytes());
        obj.push(0x70); // nibble 7: +13 (index -> 8); nibble 0: +(16 * 1) >> 3 = +2
        let pcm = decode_ima(&obj, 1, 5).unwrap();
        assert_eq!(pcm, vec![5, 10, 23, 25, 99]);
        assert!(decode_ima(&obj, 2, 5).is_none(), "stereo flag must match the channel count");
    }

    #[test]
    fn random_pick_skips_the_last_entry_and_falls_back_to_the_first() {
        let e = [(1, 0x5555, 0), (2, 0x5555, 0), (3, 0x5556, 0)];
        assert_eq!(pick_random(&e, None, 0), Some(0));
        assert_eq!(pick_random(&e, None, 0x6000), Some(1));
        assert_eq!(pick_random(&e, Some(1), 0x6000), Some(2), "last pick skipped, its weight still counted");
        assert_eq!(pick_random(&e, Some(2), 0x10000), Some(0), "no entry matched");
        let repeat = [(1, 0x8000, 1), (2, 0x8000, 1)];
        assert_eq!(pick_random(&repeat, Some(0), 0), Some(0), "repeat flag set");
    }

    fn event_play(id: u32, sound: u32, range: u32) -> Vec<u8> {
        let mut o = vec![0u8; 0x68];
        o[4..8].copy_from_slice(&id.to_le_bytes());
        o[8..12].copy_from_slice(&1u32.to_le_bytes());
        o[0xC..0x10].copy_from_slice(&sound.to_le_bytes());
        o[0x10..0x14].copy_from_slice(&range.to_le_bytes());
        o
    }

    #[test]
    fn switch_multi_and_container_resolve_to_voices() {
        let mut bank = SoundBank::default();
        let mut put = |id: u32, o: Vec<u8>| {
            bank.objects.insert(id, o.into());
        };
        // resource 0x30000001; single sound 0x20000001 (mono 24 kHz, 100 frames, -6 dB)
        put(0x3000_0001, vec![0; 64]);
        let mut s = vec![0u8; 0x84];
        s[8..12].copy_from_slice(&1u32.to_le_bytes());
        s[0x18..0x1C].copy_from_slice(&((-6i32 << 16) as u32).to_le_bytes());
        s[0x20..0x24].copy_from_slice(&0x3000_0001u32.to_le_bytes());
        s[0x48..0x4C].copy_from_slice(&1u32.to_le_bytes());
        s[0x4C..0x50].copy_from_slice(&24000u32.to_le_bytes());
        s[0x54..0x58].copy_from_slice(&100u32.to_le_bytes());
        put(0x2000_0001, s);
        // container 0x20000002 of the single sound
        let mut c = vec![0u8; 0x84 + 16];
        c[8..12].copy_from_slice(&4u32.to_le_bytes());
        c[0x20..0x24].copy_from_slice(&1u32.to_le_bytes());
        c[0x84..0x88].copy_from_slice(&0x2000_0001u32.to_le_bytes());
        c[0x88..0x8C].copy_from_slice(&0x10000u32.to_le_bytes());
        put(0x2000_0002, c);
        put(0x1000_0001, event_play(0x1000_0001, 0x2000_0002, 0));
        put(0x1000_0002, event_play(0x1000_0002, 0x2000_0001, 0x1000));
        // multi of both, switch (var 3: 0 -> multi, default -> 0x10000002)
        let mut m = vec![0u8; 0x68 + 8];
        m[8..12].copy_from_slice(&12u32.to_le_bytes());
        m[0x10..0x14].copy_from_slice(&2u32.to_le_bytes());
        m[0x68..0x6C].copy_from_slice(&0x1000_0001u32.to_le_bytes());
        m[0x6C..0x70].copy_from_slice(&0x1000_0002u32.to_le_bytes());
        put(0x1000_0003, m);
        let mut w = vec![0u8; 0x68 + 12];
        w[8..12].copy_from_slice(&11u32.to_le_bytes());
        w[0x10..0x14].copy_from_slice(&3u32.to_le_bytes());
        w[0x14..0x18].copy_from_slice(&0x1000_0002u32.to_le_bytes());
        w[0x1C..0x20].copy_from_slice(&1u32.to_le_bytes());
        w[0x68..0x6C].copy_from_slice(&0x1000_0003u32.to_le_bytes());
        put(0x1000_0004, w);

        let mut pick = Picker::new(7);
        let two = bank.resolve(0x1000_0004, &|v| (v == 3).then_some(0), &mut pick);
        assert_eq!(two.len(), 2, "the switch case plays the multi event's two children");
        assert!(two.iter().all(|v| v.res == 0x3000_0001 && v.rate == 24000 && (v.gain_db + 6.0).abs() < 1e-4));
        assert_eq!(two[0].speed, 1.0, "range 0");
        assert!((two[1].speed - 1.0).abs() <= 0.0625 + 1e-4, "range 1/16");
        let one = bank.resolve(0x1000_0004, &|_| None, &mut pick);
        assert_eq!(one.len(), 1, "no switch value: the default event");
    }

    /// The install's player sounds resolve and decode.
    #[test]
    #[ignore = "reads the game install (DataPC.forge)"]
    fn player_sounds_resolve_and_decode() {
        let bank = load_sound_bank(&crate::assets::game_dir()).unwrap();
        assert!(bank.len() > 8000, "{} SoundBaos", bank.len());
        assert!(bank.contact.iter().all(|c| c.is_some()), "{:x?}", bank.contact);
        let mut pick = Picker::new(1);
        let mut voices = 0;
        for e in bank.contact.iter().flatten() {
            for v in bank.resolve(*e, &|_| None, &mut pick) {
                let pcm = decode_ima(bank.object(v.res).unwrap(), v.channels, v.frames).expect("IMA resource");
                assert_eq!(pcm.len(), v.frames as usize * v.channels as usize);
                // a wrong nibble order or state runs away into clipping
                let clipped = pcm.iter().filter(|s| s.unsigned_abs() >= 32767).count();
                assert!(clipped * 100 < pcm.len(), "{:08x}: {clipped} of {} samples clipped", v.res, pcm.len());
                voices += 1;
            }
        }
        assert!(voices >= 10, "{voices} contact voices");
        // the climb clip's AudioEvent from RE/21: 0x1032061A -> 12-way container of 16-32 kHz IMA resources
        let v = bank.resolve(0x1032_061A, &|_| None, &mut pick);
        assert!(!v.is_empty());
    }
}
