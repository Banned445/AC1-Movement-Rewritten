//! Decoder for AC1 `Animation` payloads (class 0x0FA3067F) — port of RE/tools/ac_anim.py,
//! spec RE/10_animation_format.md. Quaternions are (x, y, z, w); times in seconds.

use std::collections::HashMap;

const CLASS_ANIM_TRACK_DATA: u32 = 0x0181_EFE8;
const CLASS_TRACK_MAPPING: u32 = 0x653C_AA76;
/// Key time unit: 1/60 s (f32 @0x1912CA0).
const TIME_SCALE: f32 = 60.0;
const INV_SQRT2: f32 = 0.707_106_77;

/// Track key ids: skeleton BoneIDs, or fixed ids (0 DISPLACEMENT, 1 PIVOT, 2 ACUATORCONTACTS, …).
pub const TRACK_DISPLACEMENT: u32 = 0;

#[derive(Clone, Debug)]
pub enum TrackValues {
    Quat(Vec<[f32; 4]>),
    Vec3(Vec<[f32; 3]>),
    Float(Vec<f32>),
    Byte(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct Track {
    pub key: u32,
    pub times: Vec<f32>,
    pub values: TrackValues,
}

#[derive(Clone, Debug)]
pub struct AnimData {
    pub duration: f32,
    pub tracks: Vec<Track>,
    /// The clip's event tracks, all keys in time order (empty if a track holds an event class the decoder doesn't
    /// know: FXEvent and one assassination event, 23 of 12,347 clips).
    pub events: Vec<AnimEvent>,
}

/// Event key time unit: 1/120 s (`AnimTrack__GetLastKeyTime` 0x58FFB0 divides by the double 120.0 at 0x168E128).
const EVENT_TIME_SCALE: f32 = 120.0;

const CLASS_ANIM_TRACK_EVENT: u32 = 0x4168_2213;
const CLASS_CONTACT_EVENT: u32 = 0x9784_0CBF;
const CLASS_AUDIO_EVENT: u32 = 0xA7A6_4A34;
const CLASS_EVENT_SEED_LINK: u32 = 0x6CE0_4D52;

/// One key of an event track (`AnimTrackEvent`: a list of EventSeeds and one key time per seed).
#[derive(Clone, Debug, PartialEq)]
pub struct AnimEvent {
    /// Seconds into the clip.
    pub time: f32,
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EventKind {
    /// `ContactEvent`: a foot / body contact for the sound and FX of the surface. `ty` = ContactEventTypeHuman
    /// (`CONTACT_HUMAN`), `bits` = the two flag bits at +12 (both set on almost every key; meaning unknown).
    Contact { ty: u32, bits: u8 },
    /// `AudioEvent`: a sound; `sound` = the id its sound reference points at.
    Audio { sound: u32 },
    /// Any other event (speech, sound sets, AI / sync events, blood): its class hash.
    Other(u32),
}

/// ContactEventTypeHuman names (enum desc 0x18E185C).
pub const CONTACT_HUMAN: [&str; 15] = [
    "ft_body_roll", "ft_jump_start", "ft_land_hands", "ft_land_heavy", "ft_land_light", "ft_land_medium", "ft_pivot",
    "ft_run_cycle", "ft_slide", "ft_sneak_cycle", "ft_step_foot", "ft_step_toe", "ft_walk_cycle", "ft_walk_end",
    "ft_walk_start",
];

/// Fields after the Event base handle, per event class (each class's serializer, vtable slot 2): u = u32, b = u8,
/// E = an embedded sound reference (u32 objId, u32 class, u32, u32 typed ref to the sound; 0x438EF0 / 0x524FD0).
fn event_fields(class: u32) -> Option<&'static str> {
    Some(match class {
        CLASS_CONTACT_EVENT => "ubb", // 0x5B1E20
        0xDF26_0DA5 => "uE",          // AudioBaseEvent 0x446120
        CLASS_AUDIO_EVENT => "uEb",   // 0x446210
        0x90D9_EC94 => "uE",          // SpeechEvent 0x446570
        0x0EFF_FE90 => "uu",          // SoundSetEvent 0x445CE0
        0xD84E_267F => "uuuu",        // BloodSplatterEvent 0xB7EC90
        0x28C6_E4EE => "u",           // AnimAssassinationEvent 0xB36770
        // EntityEvent 0x6AD6D0, DropObjectEvent, AnimSyncFall / Die, AIAnimationEvent: the handle only
        0x691A_6344 | 0x46D8_B31E | 0x8ED2_8AE9 | 0x1B73_5FBF | 0xF9BF_CB7C => "",
        _ => return None,
    })
}

/// An object pointer (RE/09 §2): u8 0 = inline object (returns its class), 2 = a reference, 3 = null.
fn object_ptr(r: &mut R) -> Result<Option<u32>, String> {
    match r.u8()? {
        0 => {
            r.u32()?;
            Ok(Some(r.u32()?))
        }
        2 => {
            r.u32()?;
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// The event tracks (payload 0x12: count, then AnimTrackEvent objects; `AnimTrackEvent__Serialize` 0x6DF4E0):
/// u32 kind, u32 n, n × EventSeed, u32, then the AnimTrack key times (0x58D1C0: u16 k, k × u16).
/// EventSeed (`EventSeed__Serialize` 0x525880): Event pointer, f32, u8, EventSeedLinkData pointer (u32, f32, u8).
fn read_events(r: &mut R) -> Result<Vec<AnimEvent>, String> {
    let n = r.u32()?;
    let mut out = Vec::new();
    for _ in 0..n {
        if object_ptr(r)? != Some(CLASS_ANIM_TRACK_EVENT) {
            return Err("not an AnimTrackEvent".into());
        }
        let _kind = r.u32()?;
        let ns = r.u32()? as usize;
        let mut kinds = Vec::with_capacity(ns);
        for _ in 0..ns {
            object_ptr(r)?; // the seed's class (ContactEventSeed, AudioEventSeed, …): EventSeed's fields
            let mut kind = None;
            if let Some(class) = object_ptr(r)? {
                let fields = event_fields(class).ok_or_else(|| format!("event class {class:08x}"))?;
                r.u32()?; // Event: handle
                let mut vals = Vec::new();
                for f in fields.bytes() {
                    match f {
                        b'u' => vals.push(r.u32()?),
                        b'b' => vals.push(r.u8()? as u32),
                        _ => {
                            r.raw(12)?;
                            vals.push(r.u32()?);
                        }
                    }
                }
                kind = Some(match class {
                    CLASS_CONTACT_EVENT => EventKind::Contact { ty: vals[0], bits: (vals[1] & 1 | (vals[2] & 1) << 1) as u8 },
                    CLASS_AUDIO_EVENT => EventKind::Audio { sound: vals[1] },
                    c => EventKind::Other(c),
                });
            }
            r.f32()?;
            r.u8()?;
            if let Some(c) = object_ptr(r)? {
                if c != CLASS_EVENT_SEED_LINK {
                    return Err("bad EventSeedLinkData".into());
                }
                r.raw(9)?;
            }
            kinds.push(kind);
        }
        r.u32()?;
        let k = r.u16()? as usize;
        let mut times = Vec::with_capacity(k);
        for _ in 0..k {
            times.push(r.u16()? as f32 / EVENT_TIME_SCALE);
        }
        out.extend(times.into_iter().zip(kinds).filter_map(|(time, k)| Some(AnimEvent { time, kind: k? })));
    }
    out.sort_by(|a, b| a.time.total_cmp(&b.time));
    Ok(out)
}

struct R<'a> {
    b: &'a [u8],
    o: usize,
}

impl<'a> R<'a> {
    fn need(&self, n: usize) -> Result<(), String> {
        if self.o + n > self.b.len() { Err(format!("overrun at {}", self.o)) } else { Ok(()) }
    }
    fn u8(&mut self) -> Result<u8, String> {
        self.need(1)?;
        self.o += 1;
        Ok(self.b[self.o - 1])
    }
    fn u16(&mut self) -> Result<u16, String> {
        self.need(2)?;
        self.o += 2;
        Ok(u16::from_le_bytes([self.b[self.o - 2], self.b[self.o - 1]]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        self.need(4)?;
        self.o += 4;
        Ok(u32::from_le_bytes(self.b[self.o - 4..self.o].try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn raw(&mut self, n: usize) -> Result<&'a [u8], String> {
        self.need(n)?;
        self.o += n;
        Ok(&self.b[self.o - n..self.o])
    }
}

fn smallest3(c: [f32; 3], idx: usize, neg: bool) -> [f32; 4] {
    let s = c[0] * c[0] + c[1] * c[1] + c[2] * c[2];
    let mut m = (1.0 - s).max(0.0).sqrt();
    if neg {
        m = -m;
    }
    let mut q = [0f32; 4];
    let mut k = 0;
    for (i, slot) in q.iter_mut().enumerate() {
        if i == idx {
            *slot = m;
        } else {
            *slot = c[k];
            k += 1;
        }
    }
    q
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn sx(v: u32, bits: u32) -> i32 {
    let v = v & ((1 << bits) - 1);
    if v & (1 << (bits - 1)) != 0 { v as i32 - (1 << bits) } else { v as i32 }
}

/// Scale constants read from the exe (RE/10): sqrt(2)/15, /127, /1024, /32767, /1048575.
const Q16: f32 = f32::from_bits(0x3DC1_1659);
const Q24: f32 = f32::from_bits(0x3C36_71D7);
const Q32: f32 = f32::from_bits(0x3AB5_04F3);
const Q48: f32 = f32::from_bits(0x3835_065D);
const Q64: f32 = f32::from_bits(0x35B5_04FF);

fn decode_quat(comp: u8, b: &[u8], o: usize) -> [f32; 4] {
    let f = |x: u32, s: f32| x as f32 * s - INV_SQRT2;
    match comp {
        0 => [0, 1, 2, 3].map(|k| f32::from_bits(le32(b, o + 4 * k))),
        1 => {
            let v = le16(b, o) as u32;
            smallest3([f((v >> 8) & 15, Q16), f((v >> 4) & 15, Q16), f(v & 15, Q16)], (v >> 14) as usize, v & 0x2000 != 0)
        }
        2 => {
            let (b0, b1, b2) = (b[o] as u32, b[o + 1] as u32, b[o + 2] as u32);
            smallest3([f(b0 & 0x7F, Q24), f(b1 & 0x7F, Q24), f(b2 & 0x7F, Q24)], ((b0 >> 7) | ((b1 >> 7) << 1)) as usize, false)
        }
        3 => {
            let v = le32(b, o);
            smallest3([f((v >> 20) & 0x3FF, Q32), f((v >> 10) & 0x3FF, Q32), f(v & 0x3FF, Q32)], (v >> 30) as usize, false)
        }
        4 => {
            let s = [le16(b, o) as u32, le16(b, o + 2) as u32, le16(b, o + 4) as u32];
            smallest3([f(s[0] & 0x7FFF, Q48), f(s[1] & 0x7FFF, Q48), f(s[2] & 0x7FFF, Q48)], ((s[0] >> 15) | ((s[1] >> 15) << 1)) as usize, false)
        }
        5 => {
            let q = u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
            let a = ((q >> 8) & 0xFFFFF) as u32;
            let bb = (((q >> 52) & 0xFFF) | ((q & 0xFF) << 12)) as u32;
            let cc = ((q >> 32) & 0xFFFFF) as u32;
            smallest3([f(a, Q64), f(bb, Q64), f(cc, Q64)], ((q >> 30) & 3) as usize, false)
        }
        _ => {
            let u = [le32(b, o), le32(b, o + 4), le32(b, o + 8)];
            let idx = ((u[0] & 1) | ((u[1] & 1) << 1)) as usize;
            smallest3(u.map(f32::from_bits), idx, false)
        }
    }
}

/// Descriptor groups (index = desc >> 2): (kind, compression, value size). Table 0x1A11E90.
const GROUPS: [(u8, u8, usize); 16] = [
    (0, 0, 16), (0, 1, 2), (0, 2, 3), (0, 3, 4), (0, 4, 6), (0, 5, 8), (0, 6, 12), // quats
    (1, 0, 12), (1, 1, 4), (1, 2, 6),                                            // vec3 None/32/48
    (2, 0, 4), (2, 1, 1), (2, 2, 2),                                             // float None/8/16
    (3, 0, 1), (3, 1, 1), (3, 2, 1),                                             // byte
];

fn read_track(r: &mut R) -> Result<(Vec<f32>, TrackValues), String> {
    let desc = r.u8()?;
    let (kind, comp, vsize) = *GROUPS.get((desc >> 2) as usize).ok_or("bad descriptor")?;
    let t16 = desc & 1 != 0;
    let _alloc = r.u32()?;
    let n = r.u32()? as usize;
    if n > 100_000 {
        return Err("absurd key count".into());
    }
    let mut times = vec![0f32];
    for _ in 1..n.max(1) {
        let t = if t16 { r.u16()? as f32 } else { r.u8()? as f32 };
        times.push(t / TIME_SCALE);
    }
    times.truncate(n);
    let raw = r.raw(n * vsize)?;
    let values = match kind {
        0 => TrackValues::Quat((0..n).map(|i| decode_quat(comp, raw, i * vsize)).collect()),
        1 => TrackValues::Vec3(
            (0..n)
                .map(|i| {
                    let o = i * vsize;
                    match comp {
                        0 => [0, 1, 2].map(|k| f32::from_bits(le32(raw, o + 4 * k))),
                        1 => {
                            let v = le32(raw, o);
                            [sx(v >> 21, 11) as f32 * 0.001, sx(v >> 10, 11) as f32 * 0.001, sx(v, 10) as f32 * 0.001]
                        }
                        _ => [0, 1, 2].map(|k| le16(raw, o + 2 * k) as i16 as f32 * 0.001),
                    }
                })
                .collect(),
        ),
        2 => TrackValues::Float(
            (0..n)
                .map(|i| {
                    let o = i * vsize;
                    match comp {
                        0 => f32::from_bits(le32(raw, o)),
                        1 => raw[o] as i8 as f32 * 0.008,
                        _ => le16(raw, o) as i16 as f32 * 0.008,
                    }
                })
                .collect(),
        ),
        _ => TrackValues::Byte(raw.to_vec()),
    };
    Ok((times, values))
}

/// Only the event tracks of an `Animation` payload.
pub fn decode_events(payload: &[u8]) -> Result<Vec<AnimEvent>, String> {
    read_events(&mut R { b: payload, o: 0x12 })
}

pub fn decode(payload: &[u8]) -> Result<AnimData, String> {
    let mut r = R { b: payload, o: 8 };
    let duration = r.f32()?;
    let events = decode_events(payload).unwrap_or_default();
    // skip header hash, flags and the reflected event tracks: find the AnimTrackData object
    let start = r.o;
    let pat = CLASS_ANIM_TRACK_DATA.to_le_bytes();
    let i = payload[start..].windows(4).position(|w| w == pat).ok_or("AnimTrackData not found")? + start;
    r.o = i + 4;
    let _hash = r.u32()?;
    let nmap = r.u32()? as usize;
    let mut keys = Vec::with_capacity(nmap);
    for _ in 0..nmap {
        let _oid = r.u32()?;
        if r.u32()? != CLASS_TRACK_MAPPING {
            return Err("bad track mapping".into());
        }
        keys.push(r.u32()?);
    }
    let _u16 = r.u16()?;
    let ntr = r.u32()? as usize;
    let mut tracks = Vec::with_capacity(ntr);
    for k in 0..ntr {
        let (times, values) = read_track(&mut r)?;
        tracks.push(Track { key: *keys.get(k).unwrap_or(&u32::MAX), times, values });
    }
    Ok(AnimData { duration, tracks, events })
}

/// Bone tracks of a decoded clip: (rotation keys, translation keys) by key id.
pub type BoneTracks = (HashMap<u32, Vec<(f32, [f32; 4])>>, HashMap<u32, Vec<(f32, [f32; 3])>>);

pub fn bone_tracks(a: &AnimData) -> BoneTracks {
    let mut rot = HashMap::new();
    let mut pos = HashMap::new();
    for t in &a.tracks {
        match &t.values {
            TrackValues::Quat(v) => {
                rot.insert(t.key, t.times.iter().copied().zip(v.iter().copied()).collect());
            }
            TrackValues::Vec3(v) => {
                pos.insert(t.key, t.times.iter().copied().zip(v.iter().copied()).collect());
            }
            _ => {}
        }
    }
    (rot, pos)
}
