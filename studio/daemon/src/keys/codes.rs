//! Windows key identities and names, matching the original keyboard service.

use super::macros::MacroEvent;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Key {
    pub scan: u16,
    pub extended: bool,
    pub vk: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Output {
    Physical(Key),
    Virtual(u16),
}

pub const KEYS: &[(&str, Key)] = &[
    (
        "KEY_ESC",
        Key {
            scan: 0x01,
            extended: false,
            vk: 0x1b,
        },
    ),
    (
        "KEY_1",
        Key {
            scan: 0x02,
            extended: false,
            vk: 0x31,
        },
    ),
    (
        "KEY_2",
        Key {
            scan: 0x03,
            extended: false,
            vk: 0x32,
        },
    ),
    (
        "KEY_3",
        Key {
            scan: 0x04,
            extended: false,
            vk: 0x33,
        },
    ),
    (
        "KEY_4",
        Key {
            scan: 0x05,
            extended: false,
            vk: 0x34,
        },
    ),
    (
        "KEY_5",
        Key {
            scan: 0x06,
            extended: false,
            vk: 0x35,
        },
    ),
    (
        "KEY_6",
        Key {
            scan: 0x07,
            extended: false,
            vk: 0x36,
        },
    ),
    (
        "KEY_7",
        Key {
            scan: 0x08,
            extended: false,
            vk: 0x37,
        },
    ),
    (
        "KEY_8",
        Key {
            scan: 0x09,
            extended: false,
            vk: 0x38,
        },
    ),
    (
        "KEY_9",
        Key {
            scan: 0x0a,
            extended: false,
            vk: 0x39,
        },
    ),
    (
        "KEY_0",
        Key {
            scan: 0x0b,
            extended: false,
            vk: 0x30,
        },
    ),
    (
        "KEY_MINUS",
        Key {
            scan: 0x0c,
            extended: false,
            vk: 0xbd,
        },
    ),
    (
        "KEY_EQUAL",
        Key {
            scan: 0x0d,
            extended: false,
            vk: 0xbb,
        },
    ),
    (
        "KEY_BACKSPACE",
        Key {
            scan: 0x0e,
            extended: false,
            vk: 0x08,
        },
    ),
    (
        "KEY_TAB",
        Key {
            scan: 0x0f,
            extended: false,
            vk: 0x09,
        },
    ),
    (
        "KEY_Q",
        Key {
            scan: 0x10,
            extended: false,
            vk: 0x51,
        },
    ),
    (
        "KEY_W",
        Key {
            scan: 0x11,
            extended: false,
            vk: 0x57,
        },
    ),
    (
        "KEY_E",
        Key {
            scan: 0x12,
            extended: false,
            vk: 0x45,
        },
    ),
    (
        "KEY_R",
        Key {
            scan: 0x13,
            extended: false,
            vk: 0x52,
        },
    ),
    (
        "KEY_T",
        Key {
            scan: 0x14,
            extended: false,
            vk: 0x54,
        },
    ),
    (
        "KEY_Y",
        Key {
            scan: 0x15,
            extended: false,
            vk: 0x59,
        },
    ),
    (
        "KEY_U",
        Key {
            scan: 0x16,
            extended: false,
            vk: 0x55,
        },
    ),
    (
        "KEY_I",
        Key {
            scan: 0x17,
            extended: false,
            vk: 0x49,
        },
    ),
    (
        "KEY_O",
        Key {
            scan: 0x18,
            extended: false,
            vk: 0x4f,
        },
    ),
    (
        "KEY_P",
        Key {
            scan: 0x19,
            extended: false,
            vk: 0x50,
        },
    ),
    (
        "KEY_LEFTBRACE",
        Key {
            scan: 0x1a,
            extended: false,
            vk: 0xdb,
        },
    ),
    (
        "KEY_RIGHTBRACE",
        Key {
            scan: 0x1b,
            extended: false,
            vk: 0xdd,
        },
    ),
    (
        "KEY_ENTER",
        Key {
            scan: 0x1c,
            extended: false,
            vk: 0x0d,
        },
    ),
    (
        "KEY_LEFTCTRL",
        Key {
            scan: 0x1d,
            extended: false,
            vk: 0xa2,
        },
    ),
    (
        "KEY_A",
        Key {
            scan: 0x1e,
            extended: false,
            vk: 0x41,
        },
    ),
    (
        "KEY_S",
        Key {
            scan: 0x1f,
            extended: false,
            vk: 0x53,
        },
    ),
    (
        "KEY_D",
        Key {
            scan: 0x20,
            extended: false,
            vk: 0x44,
        },
    ),
    (
        "KEY_F",
        Key {
            scan: 0x21,
            extended: false,
            vk: 0x46,
        },
    ),
    (
        "KEY_G",
        Key {
            scan: 0x22,
            extended: false,
            vk: 0x47,
        },
    ),
    (
        "KEY_H",
        Key {
            scan: 0x23,
            extended: false,
            vk: 0x48,
        },
    ),
    (
        "KEY_J",
        Key {
            scan: 0x24,
            extended: false,
            vk: 0x4a,
        },
    ),
    (
        "KEY_K",
        Key {
            scan: 0x25,
            extended: false,
            vk: 0x4b,
        },
    ),
    (
        "KEY_L",
        Key {
            scan: 0x26,
            extended: false,
            vk: 0x4c,
        },
    ),
    (
        "KEY_SEMICOLON",
        Key {
            scan: 0x27,
            extended: false,
            vk: 0xba,
        },
    ),
    (
        "KEY_APOSTROPHE",
        Key {
            scan: 0x28,
            extended: false,
            vk: 0xde,
        },
    ),
    (
        "KEY_GRAVE",
        Key {
            scan: 0x29,
            extended: false,
            vk: 0xc0,
        },
    ),
    (
        "KEY_LEFTSHIFT",
        Key {
            scan: 0x2a,
            extended: false,
            vk: 0xa0,
        },
    ),
    (
        "KEY_BACKSLASH",
        Key {
            scan: 0x2b,
            extended: false,
            vk: 0xdc,
        },
    ),
    (
        "KEY_Z",
        Key {
            scan: 0x2c,
            extended: false,
            vk: 0x5a,
        },
    ),
    (
        "KEY_X",
        Key {
            scan: 0x2d,
            extended: false,
            vk: 0x58,
        },
    ),
    (
        "KEY_C",
        Key {
            scan: 0x2e,
            extended: false,
            vk: 0x43,
        },
    ),
    (
        "KEY_V",
        Key {
            scan: 0x2f,
            extended: false,
            vk: 0x56,
        },
    ),
    (
        "KEY_B",
        Key {
            scan: 0x30,
            extended: false,
            vk: 0x42,
        },
    ),
    (
        "KEY_N",
        Key {
            scan: 0x31,
            extended: false,
            vk: 0x4e,
        },
    ),
    (
        "KEY_M",
        Key {
            scan: 0x32,
            extended: false,
            vk: 0x4d,
        },
    ),
    (
        "KEY_COMMA",
        Key {
            scan: 0x33,
            extended: false,
            vk: 0xbc,
        },
    ),
    (
        "KEY_DOT",
        Key {
            scan: 0x34,
            extended: false,
            vk: 0xbe,
        },
    ),
    (
        "KEY_SLASH",
        Key {
            scan: 0x35,
            extended: false,
            vk: 0xbf,
        },
    ),
    (
        "KEY_RIGHTSHIFT",
        Key {
            scan: 0x36,
            extended: false,
            vk: 0xa1,
        },
    ),
    (
        "KEY_KPASTERISK",
        Key {
            scan: 0x37,
            extended: false,
            vk: 0x6a,
        },
    ),
    (
        "KEY_LEFTALT",
        Key {
            scan: 0x38,
            extended: false,
            vk: 0xa4,
        },
    ),
    (
        "KEY_SPACE",
        Key {
            scan: 0x39,
            extended: false,
            vk: 0x20,
        },
    ),
    (
        "KEY_CAPSLOCK",
        Key {
            scan: 0x3a,
            extended: false,
            vk: 0x14,
        },
    ),
    (
        "KEY_NUMLOCK",
        Key {
            scan: 0x45,
            extended: true,
            vk: 0x90,
        },
    ),
    (
        "KEY_SCROLLLOCK",
        Key {
            scan: 0x46,
            extended: false,
            vk: 0x91,
        },
    ),
    (
        "KEY_KPENTER",
        Key {
            scan: 0x1c,
            extended: true,
            vk: 0x0d,
        },
    ),
    (
        "KEY_RIGHTCTRL",
        Key {
            scan: 0x1d,
            extended: true,
            vk: 0xa3,
        },
    ),
    (
        "KEY_RIGHTALT",
        Key {
            scan: 0x38,
            extended: true,
            vk: 0xa5,
        },
    ),
    (
        "KEY_SYSRQ",
        Key {
            scan: 0x37,
            extended: true,
            vk: 0x2c,
        },
    ),
    (
        "KEY_HOME",
        Key {
            scan: 0x47,
            extended: true,
            vk: 0x24,
        },
    ),
    (
        "KEY_UP",
        Key {
            scan: 0x48,
            extended: true,
            vk: 0x26,
        },
    ),
    (
        "KEY_PAGEUP",
        Key {
            scan: 0x49,
            extended: true,
            vk: 0x21,
        },
    ),
    (
        "KEY_LEFT",
        Key {
            scan: 0x4b,
            extended: true,
            vk: 0x25,
        },
    ),
    (
        "KEY_RIGHT",
        Key {
            scan: 0x4d,
            extended: true,
            vk: 0x27,
        },
    ),
    (
        "KEY_END",
        Key {
            scan: 0x4f,
            extended: true,
            vk: 0x23,
        },
    ),
    (
        "KEY_DOWN",
        Key {
            scan: 0x50,
            extended: true,
            vk: 0x28,
        },
    ),
    (
        "KEY_PAGEDOWN",
        Key {
            scan: 0x51,
            extended: true,
            vk: 0x22,
        },
    ),
    (
        "KEY_INSERT",
        Key {
            scan: 0x52,
            extended: true,
            vk: 0x2d,
        },
    ),
    (
        "KEY_DELETE",
        Key {
            scan: 0x53,
            extended: true,
            vk: 0x2e,
        },
    ),
    (
        "KEY_PAUSE",
        Key {
            scan: 0x45,
            extended: false,
            vk: 0x13,
        },
    ),
    (
        "KEY_F1",
        Key {
            scan: 0x3b,
            extended: false,
            vk: 0x70,
        },
    ),
    (
        "KEY_F2",
        Key {
            scan: 0x3c,
            extended: false,
            vk: 0x71,
        },
    ),
    (
        "KEY_F3",
        Key {
            scan: 0x3d,
            extended: false,
            vk: 0x72,
        },
    ),
    (
        "KEY_F4",
        Key {
            scan: 0x3e,
            extended: false,
            vk: 0x73,
        },
    ),
    (
        "KEY_F5",
        Key {
            scan: 0x3f,
            extended: false,
            vk: 0x74,
        },
    ),
    (
        "KEY_F6",
        Key {
            scan: 0x40,
            extended: false,
            vk: 0x75,
        },
    ),
    (
        "KEY_F7",
        Key {
            scan: 0x41,
            extended: false,
            vk: 0x76,
        },
    ),
    (
        "KEY_F8",
        Key {
            scan: 0x42,
            extended: false,
            vk: 0x77,
        },
    ),
    (
        "KEY_F9",
        Key {
            scan: 0x43,
            extended: false,
            vk: 0x78,
        },
    ),
    (
        "KEY_F10",
        Key {
            scan: 0x44,
            extended: false,
            vk: 0x79,
        },
    ),
    (
        "KEY_F11",
        Key {
            scan: 0x57,
            extended: false,
            vk: 0x7a,
        },
    ),
    (
        "KEY_F12",
        Key {
            scan: 0x58,
            extended: false,
            vk: 0x7b,
        },
    ),
    (
        "KEY_F13",
        Key {
            scan: 0x64,
            extended: false,
            vk: 0x7c,
        },
    ),
    (
        "KEY_F14",
        Key {
            scan: 0x65,
            extended: false,
            vk: 0x7d,
        },
    ),
    (
        "KEY_F15",
        Key {
            scan: 0x66,
            extended: false,
            vk: 0x7e,
        },
    ),
    (
        "KEY_F16",
        Key {
            scan: 0x67,
            extended: false,
            vk: 0x7f,
        },
    ),
    (
        "KEY_F17",
        Key {
            scan: 0x68,
            extended: false,
            vk: 0x80,
        },
    ),
    (
        "KEY_F18",
        Key {
            scan: 0x69,
            extended: false,
            vk: 0x81,
        },
    ),
    (
        "KEY_F19",
        Key {
            scan: 0x6a,
            extended: false,
            vk: 0x82,
        },
    ),
    (
        "KEY_F20",
        Key {
            scan: 0x6b,
            extended: false,
            vk: 0x83,
        },
    ),
    (
        "KEY_F21",
        Key {
            scan: 0x6c,
            extended: false,
            vk: 0x84,
        },
    ),
    (
        "KEY_F22",
        Key {
            scan: 0x6d,
            extended: false,
            vk: 0x85,
        },
    ),
    (
        "KEY_F23",
        Key {
            scan: 0x6e,
            extended: false,
            vk: 0x86,
        },
    ),
    (
        "KEY_F24",
        Key {
            scan: 0x76,
            extended: false,
            vk: 0x87,
        },
    ),
    (
        "KEY_LEFTMETA",
        Key {
            scan: 0x5b,
            extended: true,
            vk: 0x5b,
        },
    ),
    (
        "KEY_RIGHTMETA",
        Key {
            scan: 0x5c,
            extended: true,
            vk: 0x5c,
        },
    ),
];

pub const VIRTUAL_KEYS: &[(&str, u16)] = &[
    ("KEY_MUTE", 0xad),
    ("KEY_VOLUMEDOWN", 0xae),
    ("KEY_VOLUMEUP", 0xaf),
    ("KEY_NEXTSONG", 0xb0),
    ("KEY_PREVIOUSSONG", 0xb1),
    ("KEY_STOPCD", 0xb2),
    ("KEY_PLAYPAUSE", 0xb3),
    ("KEY_SLEEP", 0x5f),
    ("KEY_CALC", 0xb7),
];

const ALIASES: &[(&str, &str)] = &[
    ("ctrl", "KEY_LEFTCTRL"),
    ("control", "KEY_LEFTCTRL"),
    ("lctrl", "KEY_LEFTCTRL"),
    ("rctrl", "KEY_RIGHTCTRL"),
    ("shift", "KEY_LEFTSHIFT"),
    ("lshift", "KEY_LEFTSHIFT"),
    ("rshift", "KEY_RIGHTSHIFT"),
    ("alt", "KEY_LEFTALT"),
    ("lalt", "KEY_LEFTALT"),
    ("ralt", "KEY_RIGHTALT"),
    ("altgr", "KEY_RIGHTALT"),
    ("win", "KEY_LEFTMETA"),
    ("super", "KEY_LEFTMETA"),
    ("cmd", "KEY_LEFTMETA"),
    ("esc", "KEY_ESC"),
    ("escape", "KEY_ESC"),
    ("enter", "KEY_ENTER"),
    ("return", "KEY_ENTER"),
    ("tab", "KEY_TAB"),
    ("space", "KEY_SPACE"),
    ("backspace", "KEY_BACKSPACE"),
    ("del", "KEY_DELETE"),
    ("delete", "KEY_DELETE"),
    ("ins", "KEY_INSERT"),
    ("insert", "KEY_INSERT"),
    ("home", "KEY_HOME"),
    ("end", "KEY_END"),
    ("pgup", "KEY_PAGEUP"),
    ("pageup", "KEY_PAGEUP"),
    ("pgdn", "KEY_PAGEDOWN"),
    ("pagedown", "KEY_PAGEDOWN"),
    ("up", "KEY_UP"),
    ("down", "KEY_DOWN"),
    ("left", "KEY_LEFT"),
    ("right", "KEY_RIGHT"),
    ("printscreen", "KEY_SYSRQ"),
    ("prtsc", "KEY_SYSRQ"),
    ("pause", "KEY_PAUSE"),
    ("capslock", "KEY_CAPSLOCK"),
    ("numlock", "KEY_NUMLOCK"),
    ("scrolllock", "KEY_SCROLLLOCK"),
    ("minus", "KEY_MINUS"),
    ("equal", "KEY_EQUAL"),
    ("mute", "KEY_MUTE"),
    ("volup", "KEY_VOLUMEUP"),
    ("voldown", "KEY_VOLUMEDOWN"),
    ("play", "KEY_PLAYPAUSE"),
    ("next", "KEY_NEXTSONG"),
    ("prev", "KEY_PREVIOUSSONG"),
];

pub const ACTIONS: &[(&str, &str)] = &[
    ("RAZER_MACRO_KEY", "macro_record"),
    ("RAZER_GAME_KEY", "game_mode"),
    ("RAZER_BRIGHTNESS_DOWN", "brightness_down"),
    ("RAZER_BRIGHTNESS_UP", "brightness_up"),
    ("KEY_KBDILLUMDOWN", "brightness_down"),
    ("KEY_KBDILLUMUP", "brightness_up"),
];

pub const MACRO_TARGETS: &[(&str, &str)] = &[
    ("KEY_MACRO1", "M1"),
    ("KEY_MACRO2", "M2"),
    ("KEY_MACRO3", "M3"),
    ("KEY_MACRO4", "M4"),
];

pub const MACRO_KEYS: &[(u8, &str)] = &[
    (0x20, "M1"),
    (0x21, "M2"),
    (0x22, "M3"),
    (0x23, "M4"),
    (0x24, "M5"),
    (0x25, "M6"),
    (0x60, "DIAL"),
    (0x63, "SIDE1"),
    (0x64, "SIDE2"),
    (0x65, "SIDE3"),
];

pub fn source_name(scan: u32, extended: bool, vk: u32) -> Option<&'static str> {
    if vk == 0x13 {
        return Some("KEY_PAUSE");
    }
    KEYS.iter()
        .find(|(name, key)| {
            // The original source map predates the sequence-only Windows keys.
            !matches!(*name, "KEY_PAUSE" | "KEY_LEFTMETA" | "KEY_RIGHTMETA")
                && u32::from(key.scan) == scan
                && key.extended == extended
        })
        .map(|(name, _)| *name)
}

pub fn output_for(name: &str) -> Option<Output> {
    let mapped;
    let name = if let Some(suffix) = name.strip_prefix("USB_HID_KEY_MEDIA_") {
        if !matches!(
            suffix,
            "MUTE"
                | "VOLUMEDOWN"
                | "VOLUMEUP"
                | "NEXTSONG"
                | "PREVIOUSSONG"
                | "PLAYPAUSE"
                | "SLEEP"
        ) {
            return None;
        }
        mapped = format!("KEY_{suffix}");
        mapped.as_str()
    } else if let Some(suffix) = name.strip_prefix("USB_HID_KEY_F") {
        if !suffix
            .parse::<u8>()
            .ok()
            .is_some_and(|n| (13..=24).contains(&n))
        {
            return None;
        }
        mapped = format!("KEY_F{suffix}");
        mapped.as_str()
    } else {
        name
    };
    KEYS.iter()
        .find(|(n, _)| *n == name)
        .map(|(_, key)| Output::Physical(*key))
        .or_else(|| {
            VIRTUAL_KEYS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, vk)| Output::Virtual(*vk))
        })
}

pub fn key_named(name: &str) -> Result<Output> {
    let lowered = name.trim().to_lowercase();
    let full = ALIASES
        .iter()
        .find(|(alias, _)| *alias == lowered)
        .map(|(_, full)| (*full).to_owned())
        .unwrap_or_else(|| format!("KEY_{}", lowered.to_uppercase()));
    output_for(&full).ok_or_else(|| Error::Invalid(format!("unknown key {name:?}")))
}

impl Output {
    pub fn event(self, down: bool) -> MacroEvent {
        match self {
            Self::Physical(key) => MacroEvent {
                vk: key.vk,
                scan: key.scan,
                ext: key.extended,
                down,
                delay: 0,
            },
            Self::Virtual(vk) => MacroEvent {
                vk,
                scan: 0,
                ext: matches!(vk, 0xad..=0xb3 | 0xb7),
                down,
                delay: 0,
            },
        }
    }
}

pub fn parse_sequence(text: &str) -> Result<Vec<MacroEvent>> {
    let mut events = Vec::new();
    for chord in text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|c| !c.is_empty())
    {
        let keys: Vec<_> = chord
            .split('+')
            .filter(|s| !s.is_empty())
            .map(key_named)
            .collect::<Result<_>>()?;
        events.extend(keys.iter().map(|key| key.event(true)));
        events.extend(keys.iter().rev().map(|key| key.event(false)));
    }
    if events.is_empty() {
        return Err(Error::Invalid("no keys given".into()));
    }
    Ok(events)
}

pub fn describe_event(event: &MacroEvent) -> String {
    if event.scan != 0 {
        if let Some((name, _)) = KEYS
            .iter()
            .find(|(_, key)| key.scan == event.scan && key.extended == event.ext)
        {
            return name[4..].into();
        }
    }
    VIRTUAL_KEYS
        .iter()
        .find(|(_, vk)| *vk == event.vk)
        .map(|(name, _)| name[4..].to_owned())
        .unwrap_or_else(|| format!("vk{:02x}", event.vk))
}
