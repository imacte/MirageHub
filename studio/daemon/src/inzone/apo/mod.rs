// SPDX-License-Identifier: GPL-3.0-only
//! Sony's APO owns the Windows DSP; Mirage Hub edits its per-endpoint configuration.
pub mod components;
pub mod document;
#[cfg(windows)]
mod native;
use crate::{Error, Result};
use serde_json::{json, Value};

pub fn state() -> Value {
    #[cfg(windows)]
    let mut state =
        native::state().unwrap_or_else(|e| json!({"available":false,"error":e.to_string()}));
    #[cfg(not(windows))]
    let mut state = json!({"available":false,"reason":"windows_only"});
    #[cfg(windows)]
    {
        state["microphone"] = native::microphone_state()
            .unwrap_or_else(|e| json!({"available":false,"error":e.to_string()}));
    }
    state["components"] = components::state();
    state
}

pub fn change(method: &str, value: &Value) -> Result<Value> {
    if method == "prepare_sony_audio" || method == "install_sony_audio" {
        components::start(method == "install_sony_audio")?;
        return Ok(Value::Null);
    }
    #[cfg(windows)]
    {
        native::change(method, value)?;
        Ok(Value::Null)
    }
    #[cfg(not(windows))]
    {
        let _ = (method, value);
        Err(Error::NotSupported("Sony APO requires Windows".into()))
    }
}

#[cfg(windows)]
pub(super) fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
