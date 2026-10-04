//! Body-part channels: which bones an action animates.
//!
//! - `BodyPartTemplate_Human` (DataPC.forge "Game Fix", BodyPartTemplate::Serialize 0x6D42D0) lists the channels an
//!   action can name (`BodyPartChannel` 0x5B7DB0: index, slot group, part mask) and the acuator → part mapping.
//! - `Human_BodyPartMapping` ("Rank 9", BodyPartMapping 0x6D50F0 / BodyPart 0x6D5810) gives each part its bones:
//!   0 legs + hips + Reference, 1 torso, 2 head / neck, 3 / 4 single bones, 5 left arm, 6 right arm, 7 / 8 the left /
//!   right fingers, 9 every bone.
//! - `Anim__PlayActionOnSlots` 0x4FE730 plays an action on every slot of its channel's group whose mask meets the
//!   channel's mask. Every movement action is on channel 0 (group 0, all parts: the full body).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::forge::Forge;

/// One `BodyPartChannel`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Channel {
    pub index: u32,
    /// The slot group (0: the full-body slot; 1: the partial slots).
    pub group: u32,
    /// Bit i = part i.
    pub mask: u32,
}

#[derive(Clone, Debug, Default)]
pub struct BodyParts {
    /// Channel object id → channel.
    pub channels: HashMap<u32, Channel>,
    /// Part index → bone ids.
    pub parts: Vec<Vec<u32>>,
}

impl BodyParts {
    /// The bones a part mask covers.
    pub fn bones(&self, mask: u32) -> HashSet<u32> {
        self.parts.iter().enumerate().filter(|(i, _)| *i < 32 && mask >> i & 1 != 0).flat_map(|(_, b)| b.iter().copied()).collect()
    }
}

struct R<'a> {
    b: &'a [u8],
    o: usize,
}

impl R<'_> {
    fn u8(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.o).ok_or("body parts: truncated")?;
        self.o += 1;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.b.get(self.o..self.o + 4).ok_or("body parts: truncated")?;
        self.o += 4;
        Ok(u32::from_le_bytes(s.try_into().unwrap()))
    }
}

/// `BodyPartTemplate`: u8; u32 n × handle object (`BodyPartChannel`, inline or a reference); u32 m × embedded
/// (id, class, u32 acuator, u8 part).
pub fn parse_template(payload: &[u8]) -> Result<HashMap<u32, Channel>, String> {
    let mut r = R { b: payload, o: 0 };
    let (_id, _cls) = (r.u32()?, r.u32()?);
    r.u8()?;
    let n = r.u32()?;
    let mut channels = HashMap::new();
    for _ in 0..n {
        match r.u8()? {
            0 => {
                let (id, _cls) = (r.u32()?, r.u32()?);
                let c = Channel { index: r.u32()?, group: r.u32()?, mask: r.u32()? };
                channels.insert(id, c);
            }
            1 | 2 => {
                r.u32()?;
            }
            _ => {}
        }
    }
    let m = r.u32()?;
    for _ in 0..m {
        (r.u32()?, r.u32()?, r.u32()?, r.u8()?);
    }
    (r.o == payload.len()).then_some(channels).ok_or_else(|| format!("body part template: {} of {} bytes", r.o, payload.len()))
}

/// `BodyPartMapping`: u32 n × object (`BodyPart`: u32 k × u32 bone id); a skeleton reference.
pub fn parse_mapping(payload: &[u8]) -> Result<Vec<Vec<u32>>, String> {
    let mut r = R { b: payload, o: 0 };
    let (_id, _cls) = (r.u32()?, r.u32()?);
    let n = r.u32()?;
    let mut parts = Vec::new();
    for _ in 0..n {
        let mut bones = Vec::new();
        if r.u8()? == 0 {
            let (_id, _cls) = (r.u32()?, r.u32()?);
            let k = r.u32()?;
            for _ in 0..k {
                bones.push(r.u32()?);
            }
        } else {
            r.u32()?;
        }
        parts.push(bones);
    }
    r.u32()?;
    (r.o == payload.len()).then_some(parts).ok_or_else(|| format!("body part mapping: {} of {} bytes", r.o, payload.len()))
}

/// Load the human template and Altaïr's mapping from the install.
pub fn load_body_parts(game_dir: &Path) -> Result<BodyParts, String> {
    let path = game_dir.join("DataPC.forge");
    let mut forge = Forge::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let find = |forge: &mut Forge, file: &str, name: &str| -> Result<Vec<u8>, String> {
        let entry = forge.find(file).cloned().ok_or(format!("file '{file}' not found"))?;
        let res = forge.resources(&entry).map_err(|e| e.to_string())?;
        res.into_iter().find(|r| r.name == name).map(|r| r.payload).ok_or(format!("{name} not found"))
    };
    let channels = parse_template(&find(&mut forge, "Game Fix", "BodyPartTemplate_Human")?)?;
    let parts = parse_mapping(&find(&mut forge, "Rank 9", "Human_BodyPartMapping")?)?;
    Ok(BodyParts { channels, parts })
}
