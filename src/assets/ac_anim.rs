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

pub fn decode(payload: &[u8]) -> Result<AnimData, String> {
    let mut r = R { b: payload, o: 8 };
    let duration = r.f32()?;
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
    Ok(AnimData { duration, tracks })
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
