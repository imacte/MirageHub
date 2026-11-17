//! Portable mouse input used by the macro editor and native playback.
use serde::{Deserialize, Serialize};

pub const MOUSE_BINDING_SERIAL: &str = "__mouse__";
pub const MOUSE_BINDING_KEYS: [&str; 3] = ["Back", "Forward", "Middle"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl MouseButton {
    pub fn binding_key(self) -> Option<&'static str> {
        match self {
            Self::Back => Some("Back"),
            Self::Forward => Some("Forward"),
            Self::Middle => Some("Middle"),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum MouseInput {
    Button { button: MouseButton },
    Wheel { delta: i32, horizontal: bool },
}
