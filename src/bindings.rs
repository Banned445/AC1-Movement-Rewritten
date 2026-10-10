//! The game's input bindings, read at runtime from the user's install like the game does (RE/21 §1).
//!
//! `DefaultBindings.map` (next to the exe) is an INI of profiles. PadProxyPC (0x90B470) registers the keyboard devices
//! in order KeyboardMouse2 (vendor / product 1), KeyboardMouse5 (2), Keyboard (3), KeyboardAlt (4); the profile is
//! `[Input] SelectedInput` of the user's `Assassin.ini` (0x402FB0), else the first device, KeyboardMouse2.
//! Each profile entry maps one pad slot (`sub_4025D0`: Button1–4 = pad 0–3 = A / X / Y / B, PadDown / Left / Up / Right
//! = 4–7, Select / Start = 8 / 9, ShoulderLeft1 / 2 = LT / LB = 10 / 11, ShoulderRight1 / 2 = RT / RB = 12 / 13,
//! StickLeft / Right = L3 / R3 = 14 / 15, then the stick directions). Keyboard values (`sub_402450`): < 256 a DirectInput
//! scan code, 256–263 mouse button 0–7, −1 unbound. `[Input] HighProfileToggle` makes the high-profile button a toggle
//! on a tap shorter than 0.5 s (0x90B080, dword_192CD28).

use bevy::prelude::*;
use std::path::Path;

/// One binding: a key, a mouse button or nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Binding {
    #[default]
    None,
    Key(KeyCode),
    Mouse(MouseButton),
}

/// The pad slots of a profile in the game's order (`sub_4025D0`'s struct, 133 dwords each).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Slot {
    /// Button1 = pad 0 (A): Legs.
    Legs = 0,
    /// Button2 = pad 1 (X): the weapon hand.
    WeaponHand = 1,
    /// Button3 = pad 2 (Y): the head.
    Head = 2,
    /// Button4 = pad 3 (B): the empty hand.
    EmptyHand = 3,
    PadDown = 4,
    PadLeft = 5,
    PadUp = 6,
    PadRight = 7,
    Select = 8,
    Start = 9,
    /// ShoulderLeft1 = pad 10 (LT).
    LeftTrigger = 10,
    /// ShoulderLeft2 = pad 11 (LB).
    LeftShoulder = 11,
    /// ShoulderRight1 = pad 12 (RT): high profile.
    HighProfile = 12,
    /// ShoulderRight2 = pad 13 (RB).
    RightShoulder = 13,
    StickLeft = 14,
    StickRight = 15,
    LeftStickUp = 16,
    LeftStickDown = 17,
    LeftStickLeft = 18,
    LeftStickRight = 19,
    RightStickUp = 20,
    RightStickDown = 21,
    RightStickLeft = 22,
    RightStickRight = 23,
}

/// The 16 pad buttons in order (slot i = pad button i).
pub const PAD_SLOTS: [Slot; 16] = [
    Slot::Legs, Slot::WeaponHand, Slot::Head, Slot::EmptyHand, Slot::PadDown, Slot::PadLeft, Slot::PadUp, Slot::PadRight,
    Slot::Select, Slot::Start, Slot::LeftTrigger, Slot::LeftShoulder, Slot::HighProfile, Slot::RightShoulder,
    Slot::StickLeft, Slot::StickRight,
];

const KEYS: [&str; 24] = [
    "Button1", "Button2", "Button3", "Button4", "PadDown", "PadLeft", "PadUp", "PadRight", "Select", "Start",
    "ShoulderLeft1", "ShoulderLeft2", "ShoulderRight1", "ShoulderRight2", "StickLeft", "StickRight",
    "LeftStickUp", "LeftStickDown", "LeftStickLeft", "LeftStickRight", "RightStickUp", "RightStickDown",
    "RightStickLeft", "RightStickRight",
];

/// The game's default PC profile, `[KeyboardMouse2]` of the shipped DefaultBindings.map (used when the file is missing).
const KEYBOARD_MOUSE2: [i32; 24] = [57, 256, 18, 42, 5, 2, 3, 4, 15, 1, 33, 16, 257, -1, -1, 46, 17, 31, 30, 32, 200, 208, 203, 205];

#[derive(Resource, Clone, Debug)]
pub struct Bindings {
    pub profile: String,
    pub slots: [Binding; 24],
    /// `CameraActivator` (read, not consumed by the movement code).
    pub camera_activator: Binding,
    /// `[Input] HighProfileToggle`.
    pub high_profile_toggle: bool,
    /// Where the profile came from.
    pub source: String,
}

impl Default for Bindings {
    fn default() -> Self {
        Self {
            profile: "KeyboardMouse2".into(),
            slots: KEYBOARD_MOUSE2.map(code_to_binding),
            camera_activator: code_to_binding(29),
            high_profile_toggle: false,
            source: "built-in KeyboardMouse2".into(),
        }
    }
}

impl Bindings {
    pub fn get(&self, s: Slot) -> Binding {
        self.slots[s as usize]
    }
    pub fn pressed(&self, s: Slot, keys: &ButtonInput<KeyCode>, mouse: &ButtonInput<MouseButton>) -> bool {
        match self.get(s) {
            Binding::Key(k) => keys.pressed(k),
            Binding::Mouse(m) => mouse.pressed(m),
            Binding::None => false,
        }
    }
    pub fn just_pressed(&self, s: Slot, keys: &ButtonInput<KeyCode>, mouse: &ButtonInput<MouseButton>) -> bool {
        match self.get(s) {
            Binding::Key(k) => keys.just_pressed(k),
            Binding::Mouse(m) => mouse.just_pressed(m),
            Binding::None => false,
        }
    }
    pub fn just_released(&self, s: Slot, keys: &ButtonInput<KeyCode>, mouse: &ButtonInput<MouseButton>) -> bool {
        match self.get(s) {
            Binding::Key(k) => keys.just_released(k),
            Binding::Mouse(m) => mouse.just_released(m),
            Binding::None => false,
        }
    }
    /// A short name for the HUD.
    pub fn label(&self, s: Slot) -> String {
        match self.get(s) {
            Binding::Key(k) => format!("{k:?}").trim_start_matches("Key").to_string(),
            Binding::Mouse(MouseButton::Left) => "LMB".into(),
            Binding::Mouse(MouseButton::Right) => "RMB".into(),
            Binding::Mouse(MouseButton::Middle) => "MMB".into(),
            Binding::Mouse(m) => format!("{m:?}"),
            Binding::None => "-".into(),
        }
    }

    /// Load the user's profile from the install's DefaultBindings.map and the game's Assassin.ini (%APPDATA%\Ubisoft\
    /// Assassin's Creed); fall back to the built-in KeyboardMouse2.
    pub fn load(game_dir: Option<&Path>) -> Self {
        let ini = std::env::var_os("APPDATA").map(|a| Path::new(&a).join("Ubisoft").join("Assassin's Creed").join("Assassin.ini"));
        let ini_text = ini.and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let selected = ini_value(&ini_text, "Input", "SelectedInput").filter(|s| s.starts_with("Keyboard"));
        let toggle = ini_value(&ini_text, "Input", "HighProfileToggle").and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(0) != 0;
        let map = game_dir.and_then(|d| std::fs::read_to_string(d.join("DefaultBindings.map")).ok());
        let mut b = match map {
            Some(text) => Self::from_map(&text, selected.as_deref().unwrap_or("KeyboardMouse2")).unwrap_or_default(),
            None => Self::default(),
        };
        b.high_profile_toggle = toggle;
        b
    }

    /// Parse one keyboard profile out of a DefaultBindings.map text.
    pub fn from_map(text: &str, profile: &str) -> Option<Self> {
        let mut slots = [Binding::None; 24];
        let mut found = false;
        for (i, key) in KEYS.iter().enumerate() {
            if let Some(v) = ini_value(text, profile, key) {
                found = true;
                slots[i] = v.trim().parse::<i32>().map(code_to_binding).unwrap_or_default();
            }
        }
        found.then(|| Self {
            profile: profile.to_string(),
            slots,
            camera_activator: ini_value(text, profile, "CameraActivator").and_then(|v| v.trim().parse::<i32>().ok()).map(code_to_binding).unwrap_or_default(),
            high_profile_toggle: false,
            source: format!("DefaultBindings.map [{profile}]"),
        })
    }
}

/// `GetPrivateProfileString` for an INI text (case-insensitive section and key, CRLF-tolerant).
fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') && l.ends_with(']') {
            in_section = l[1..l.len() - 1].eq_ignore_ascii_case(section);
            continue;
        }
        if in_section {
            if let Some((k, v)) = l.split_once('=') {
                if k.trim().eq_ignore_ascii_case(key) {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// A keyboard profile's value (`sub_402450`): < 256 a DirectInput scan code, 256–263 a mouse button, else unbound.
fn code_to_binding(code: i32) -> Binding {
    match code {
        0..=255 => dik_to_key(code as u8).map(Binding::Key).unwrap_or_default(),
        256 => Binding::Mouse(MouseButton::Left),
        257 => Binding::Mouse(MouseButton::Right),
        258 => Binding::Mouse(MouseButton::Middle),
        259 => Binding::Mouse(MouseButton::Back),
        260 => Binding::Mouse(MouseButton::Forward),
        261..=263 => Binding::Mouse(MouseButton::Other((code - 256) as u16)),
        _ => Binding::None,
    }
}

/// DirectInput DIK_* scan codes → Bevy key codes.
fn dik_to_key(c: u8) -> Option<KeyCode> {
    use KeyCode::*;
    Some(match c {
        0x01 => Escape,
        0x02 => Digit1, 0x03 => Digit2, 0x04 => Digit3, 0x05 => Digit4, 0x06 => Digit5,
        0x07 => Digit6, 0x08 => Digit7, 0x09 => Digit8, 0x0A => Digit9, 0x0B => Digit0,
        0x0C => Minus, 0x0D => Equal, 0x0E => Backspace, 0x0F => Tab,
        0x10 => KeyQ, 0x11 => KeyW, 0x12 => KeyE, 0x13 => KeyR, 0x14 => KeyT, 0x15 => KeyY, 0x16 => KeyU,
        0x17 => KeyI, 0x18 => KeyO, 0x19 => KeyP, 0x1A => BracketLeft, 0x1B => BracketRight, 0x1C => Enter,
        0x1D => ControlLeft,
        0x1E => KeyA, 0x1F => KeyS, 0x20 => KeyD, 0x21 => KeyF, 0x22 => KeyG, 0x23 => KeyH, 0x24 => KeyJ,
        0x25 => KeyK, 0x26 => KeyL, 0x27 => Semicolon, 0x28 => Quote, 0x29 => Backquote, 0x2A => ShiftLeft,
        0x2B => Backslash,
        0x2C => KeyZ, 0x2D => KeyX, 0x2E => KeyC, 0x2F => KeyV, 0x30 => KeyB, 0x31 => KeyN, 0x32 => KeyM,
        0x33 => Comma, 0x34 => Period, 0x35 => Slash, 0x36 => ShiftRight, 0x37 => NumpadMultiply, 0x38 => AltLeft,
        0x39 => Space, 0x3A => CapsLock,
        0x3B => F1, 0x3C => F2, 0x3D => F3, 0x3E => F4, 0x3F => F5, 0x40 => F6, 0x41 => F7, 0x42 => F8,
        0x43 => F9, 0x44 => F10, 0x45 => NumLock, 0x46 => ScrollLock,
        0x47 => Numpad7, 0x48 => Numpad8, 0x49 => Numpad9, 0x4A => NumpadSubtract, 0x4B => Numpad4,
        0x4C => Numpad5, 0x4D => Numpad6, 0x4E => NumpadAdd, 0x4F => Numpad1, 0x50 => Numpad2, 0x51 => Numpad3,
        0x52 => Numpad0, 0x53 => NumpadDecimal, 0x57 => F11, 0x58 => F12,
        0x9C => NumpadEnter, 0x9D => ControlRight, 0xB5 => NumpadDivide, 0xB8 => AltRight,
        0xC7 => Home, 0xC8 => ArrowUp, 0xC9 => PageUp, 0xCB => ArrowLeft, 0xCD => ArrowRight, 0xCF => End,
        0xD0 => ArrowDown, 0xD1 => PageDown, 0xD2 => Insert, 0xD3 => Delete,
        _ => return None,
    })
}

pub struct BindingsPlugin;

impl Plugin for BindingsPlugin {
    fn build(&self, app: &mut App) {
        let b = Bindings::load(crate::assets::find_game_dir().as_deref());
        info!("input bindings: {}", b.source);
        app.insert_resource(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_keyboard_mouse_profile_parses_to_the_games_default() {
        let text = "[KeyboardMouse2]\r\nVendorID=1\r\nButton1=57\r\nButton2=256\r\nButton3=18\r\nButton4=42\r\nPadUp=3\r\nPadRight=4\r\nPadDown=5\r\nPadLeft=2\r\nSelect=15\r\nStart=1\r\nShoulderLeft1=33\r\nShoulderLeft2=16\r\nShoulderRight1=257\r\nShoulderRight2=-1\r\nStickLeft=-1\r\nStickRight=46\r\nLeftStickUp=17\r\nLeftStickDown=31\r\nLeftStickLeft=30\r\nLeftStickRight=32\r\nRightStickUp=200\r\nRightStickDown=208\r\nRightStickLeft=203\r\nRightStickRight=205\r\nCameraActivator=29\r\n[KeyboardMouse5]\r\nButton1=259\r\n";
        let b = Bindings::from_map(text, "KeyboardMouse2").unwrap();
        assert_eq!(b.slots, Bindings::default().slots, "the built-in fallback equals the shipped profile");
        assert_eq!(b.get(Slot::Legs), Binding::Key(KeyCode::Space));
        assert_eq!(b.get(Slot::EmptyHand), Binding::Key(KeyCode::ShiftLeft));
        assert_eq!(b.get(Slot::Head), Binding::Key(KeyCode::KeyE));
        assert_eq!(b.get(Slot::WeaponHand), Binding::Mouse(MouseButton::Left));
        assert_eq!(b.get(Slot::HighProfile), Binding::Mouse(MouseButton::Right));
        assert_eq!(b.get(Slot::RightShoulder), Binding::None);
        assert_eq!(b.camera_activator, Binding::Key(KeyCode::ControlLeft));
        assert_eq!(Bindings::from_map(text, "KeyboardMouse5").unwrap().get(Slot::Legs), Binding::Mouse(MouseButton::Back));
    }

    #[test]
    fn the_installs_bindings_load_when_present() {
        let Some(dir) = crate::assets::find_game_dir() else { return };
        let b = Bindings::load(Some(&dir));
        assert!(b.source.starts_with("DefaultBindings.map"), "{}", b.source);
    }
}
