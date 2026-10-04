//! Animation graph decoder: `ActionBlock` resources (DataPC.forge "Game Fix"), RE/13.
//!
//! Port of RE/tools/ac_actions.py. Every read mirrors a serializer in the exe:
//! ActionBlock 0x6E9BF0, Action 0x507EE0, ActionItem 0x5BADF0, ActionTransition 0x564DB0,
//! ActionBlend 0x5BCDE0, AssociatedActionGroup 0x6D8D20. Stream helpers: object field 0x931780
//! (u8 flag: 0 inline {id, class, fields} / 2 ref id / 3 null), handle field 0x9311A0 (also 1 = ref),
//! typed reference 0x931410 (u32 id), embedded object 0x438EF0 ({id, class, fields}), handle 0x433D80
//! (u32 id), pod array 0x930B10 (u32 count + elements).

use std::collections::HashMap;

pub const CLASS_ACTION_BLOCK: u32 = 0xEF82_FCE4;
const CLASS_ACTION: u32 = 0x4060_89A4;
const CLASS_ACTION_ITEM: u32 = 0x80E5_0E4A;
const CLASS_TRANSITION: u32 = 0x46ED_6DF7;
const CLASS_BLEND: u32 = 0xC704_1AEE;
const CLASS_ASSOC_GROUP: u32 = 0x1E6D_DBE2;

/// `ACTDisplacementMode`: who moves the root while the item plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DisplacementMode {
    #[default]
    FromAnim,
    FromPhysics,
    FromAi,
}

/// `ActionBlend` (Serialize 0x5BCDE0; in memory +0 time, +4 / +8 two floats, +16 bits, +18 byte). RE/13 §2.1.
#[allow(dead_code)] // fields kept as decoded
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActBlend {
    /// `ACTBlendType`: 0 NONE (a cut), 1 AROLLBROLL, 2 ASTOPBROLL, 3 AROLLBSTOP, 4 ASTOPBSTOP, 5 ASTOPBSTOPCUBICQUAT.
    pub kind: u8,
    /// Bit 3: the blend lasts at most as long as what is left of A (0x773F90).
    pub clamp: bool,
    /// Bits 4 / 5: ease in / ease out of the weight (0x773C30).
    pub ease_in: bool,
    pub ease_out: bool,
    /// `ACTBlendBPosMode` (bits 6–7): 0 FROMSTART, 1 PROGRESSIVE, 2 INVPROGRESSIVE, 3 FROMTARGETTIME.
    pub b_pos: u8,
    /// `ACTBlendDispSrcMode` (bits 8–9): 0 BLENDAB, 1 FROMAONLY, 2 FROMBONLY.
    pub disp_src: u8,
    /// `ACTBlendAcuatorMode` (bits 10–11): 0 FROMA, 1 FROMB.
    pub acuator: u8,
    /// Blend duration (s).
    pub time: f32,
    /// +4: the overlap an item blend takes off the action's length (0x72FC30).
    pub overlap: f32,
    /// +8: B's start time for FROMTARGETTIME.
    pub target_time: f32,
}

impl ActBlend {
    /// The default transition's blend (`sub_46A460`: dword_1A11C68, type ASTOPBROLL, 0.2 s; the rest as the
    /// `ActionBlend` ctor 0x5BD220: FROMSTART, BLENDAB, acuator FROMB).
    pub const DEFAULT: ActBlend = ActBlend {
        kind: 2,
        clamp: false,
        ease_in: false,
        ease_out: false,
        b_pos: 0,
        disp_src: 0,
        acuator: 1,
        time: 0.2,
        overlap: 0.0,
        target_time: 0.0,
    };
}

/// `ActionTransition` (Serialize 0x564DB0; in memory +4 action A, +12 action B, +20 blend A, +40 blend B): play
/// `action_a` (if any) blended in with `blend_a`, then `action_b` blended in with `blend_b`. An action's own
/// in / out transitions usually carry only blend A.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActTransition {
    pub action_a: u32,
    pub blend_a: ActBlend,
    pub u_a: u32,
    pub action_b: u32,
    pub blend_b: ActBlend,
    pub u_b: u32,
}

#[allow(dead_code)] // decoded for the remaining graph work (RE/13)
#[derive(Clone, Debug, Default)]
pub struct ActItem {
    pub id: u32,
    /// Animation resource ids, blended with `weights` (the authored default weights).
    pub animations: Vec<u32>,
    pub weights: Vec<f32>,
    pub displacement: DisplacementMode,
    pub blend: ActBlend,
    /// Outgoing transitions (`sub_5B98B0` matches `action_b` with the requested action).
    pub transitions: Vec<ActTransition>,
}

#[derive(Clone, Debug, Default)]
pub struct Action {
    pub id: u32,
    pub block: String,
    pub items: Vec<ActItem>,
    /// +16: the transition used when entering this action; +20: the one used when leaving it (`sub_725950`).
    pub in_transition: Option<ActTransition>,
    pub out_transition: Option<ActTransition>,
    /// +48 bits 0–4 (bit 2: no default transition into it, `sub_725950`).
    pub flags: u8,
    /// +28: how many times the action plays; 0 = it loops (0x72FC30).
    pub repeat: u32,
    /// +12: the `BodyPartChannel` (object id in `BodyPartTemplate_Human`; `crate::assets::body_parts`).
    pub channel: u32,
}

/// All decoded actions, by action id (the ids the exe passes to the animation graph).
#[derive(Default, Debug)]
pub struct ActionGraph {
    pub actions: HashMap<u32, Action>,
}

struct R<'a> {
    b: &'a [u8],
    o: usize,
}

impl R<'_> {
    fn u8(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.o).ok_or("overrun")?;
        self.o += 1;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.b.get(self.o..self.o + 4).ok_or("overrun")?;
        self.o += 4;
        Ok(u32::from_le_bytes(s.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }
}

#[allow(dead_code)]
enum Obj {
    None,
    Ref(u32),
    Action(Action),
    Item(ActItem),
    Transition(ActTransition),
    Other,
}

struct Ctx {
    block: String,
    items: HashMap<u32, ActItem>,
    actions: Vec<Action>,
}

fn read_obj(r: &mut R, c: &mut Ctx, handle: bool) -> Result<Obj, String> {
    match r.u8()? {
        3 => Ok(Obj::None),
        2 => Ok(Obj::Ref(r.u32()?)),
        1 if handle => Ok(Obj::Ref(r.u32()?)),
        0 => {
            let id = r.u32()?;
            let cls = r.u32()?;
            read_body(r, c, id, cls)
        }
        f => Err(format!("bad object flag {f} at {:#x}", r.o - 1)),
    }
}

fn read_blend(r: &mut R) -> Result<ActBlend, String> {
    let kind = (r.u32()? & 7) as u8;
    let clamp = r.u8()? & 1 != 0;
    let ease_in = r.u8()? & 1 != 0;
    let ease_out = r.u8()? & 1 != 0;
    let b_pos = (r.u32()? & 3) as u8;
    let disp_src = (r.u32()? & 3) as u8;
    let acuator = (r.u32()? & 3) as u8;
    r.u8()?;
    let time = r.f32()?;
    let overlap = r.f32()?;
    let target_time = r.f32()?;
    r.u8()?;
    // optional ActionBlendFrankenstein: never present in the shipped data (RE/13)
    match r.u8()? {
        3 => {}
        2 => {
            r.u32()?;
        }
        f => return Err(format!("unsupported ActionBlendFrankenstein (flag {f})")),
    }
    Ok(ActBlend { kind, clamp, ease_in, ease_out, b_pos, disp_src, acuator, time, overlap, target_time })
}

fn read_body(r: &mut R, c: &mut Ctx, id: u32, cls: u32) -> Result<Obj, String> {
    match cls {
        CLASS_ACTION => {
            let _action_id = r.u32()?;
            let channel = match read_obj(r, c, true)? {
                Obj::Ref(id) => id,
                _ => 0,
            };
            let tr = |o: Obj| if let Obj::Transition(t) = o { Some(t) } else { None };
            let in_transition = tr(read_obj(r, c, false)?);
            let out_transition = tr(read_obj(r, c, false)?);
            let mut flags = 0u8;
            for b in 0..5 {
                flags |= (r.u8()? & 1) << b;
            }
            r.u32()?; // ACTTorsoConstraintMode
            let repeat = r.u32()?;
            r.u32()?;
            read_obj(r, c, false)?; // AssociatedActionGroup
            let n = r.u32()?;
            let mut items = Vec::new();
            for _ in 0..n {
                match read_obj(r, c, false)? {
                    Obj::Item(it) => items.push(it),
                    Obj::Ref(rid) => {
                        if let Some(it) = c.items.get(&rid) {
                            items.push(it.clone());
                        }
                    }
                    _ => {}
                }
            }
            let a = Action { id, block: c.block.clone(), items, in_transition, out_transition, flags, repeat, channel };
            c.actions.push(a.clone());
            Ok(Obj::Action(a))
        }
        CLASS_ACTION_ITEM => {
            let n = r.u32()?;
            let animations = (0..n).map(|_| r.u32()).collect::<Result<Vec<_>, _>>()?;
            let n = r.u32()?;
            let mut transitions = Vec::new();
            for _ in 0..n {
                if let Obj::Transition(t) = read_obj(r, c, false)? {
                    transitions.push(t);
                }
            }
            let (bid, bcls) = (r.u32()?, r.u32()?);
            let _ = (bid, bcls);
            let blend = read_blend(r)?;
            let disp = r.u32()?;
            r.u32()?;
            r.u32()?;
            for _ in 0..12 {
                r.u8()?;
            }
            r.f32()?;
            r.u32()?;
            r.u8()?;
            let n = r.u32()?;
            let weights = (0..n).map(|_| r.f32()).collect::<Result<Vec<_>, _>>()?;
            let it = ActItem {
                id,
                animations,
                weights,
                displacement: match disp {
                    1 => DisplacementMode::FromPhysics,
                    2 => DisplacementMode::FromAi,
                    _ => DisplacementMode::FromAnim,
                },
                blend,
                transitions,
            };
            c.items.insert(id, it.clone());
            Ok(Obj::Item(it))
        }
        CLASS_TRANSITION => {
            let (_, _) = (r.u32()?, r.u32()?);
            let blend_a = read_blend(r)?;
            let action_a = r.u32()?;
            let u_a = r.u32()?;
            let (_, _) = (r.u32()?, r.u32()?);
            let blend_b = read_blend(r)?;
            let action_b = r.u32()?;
            let u_b = r.u32()?;
            Ok(Obj::Transition(ActTransition { action_a, blend_a, u_a, action_b, blend_b, u_b }))
        }
        CLASS_BLEND => {
            read_blend(r)?;
            Ok(Obj::Other)
        }
        CLASS_ASSOC_GROUP => {
            let n = r.u32()?;
            for _ in 0..n {
                for _ in 0..5 {
                    r.u32()?;
                }
            }
            r.f32()?;
            for _ in 0..3 {
                r.u8()?;
            }
            Ok(Obj::Other)
        }
        CLASS_ACTION_BLOCK => {
            let n = r.u32()?;
            for _ in 0..n {
                read_obj(r, c, true)?;
            }
            r.u32()?; // BodyPartTemplate reference
            read_obj(r, c, false)?;
            Ok(Obj::Other)
        }
        _ => Err(format!("no reader for class {cls:08x} at {:#x}", r.o)),
    }
}

/// Decode one ActionBlock payload; returns its actions. Errors if any byte is left over.
pub fn parse_block(name: &str, payload: &[u8]) -> Result<Vec<Action>, String> {
    let mut r = R { b: payload, o: 0 };
    let id = r.u32()?;
    let cls = r.u32()?;
    let mut c = Ctx { block: name.to_string(), items: HashMap::new(), actions: Vec::new() };
    read_body(&mut r, &mut c, id, cls)?;
    if r.o != payload.len() {
        return Err(format!("{name}: parsed {} of {} bytes", r.o, payload.len()));
    }
    Ok(c.actions)
}

impl ActionGraph {
    pub fn add_block(&mut self, name: &str, payload: &[u8]) -> Result<usize, String> {
        let acts = parse_block(name, payload)?;
        let n = acts.len();
        for a in acts {
            self.actions.insert(a.id, a);
        }
        Ok(n)
    }
}
