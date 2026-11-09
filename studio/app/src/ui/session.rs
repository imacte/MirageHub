//! Save editable UI state, never textures, fonts or device connections.

use super::{app::{App, Page}, device::DeviceUi, edit::DeviceEdit, lighting::LightingUi, macros::MacrosUi};
use serde::Deserialize;
use std::{collections::HashMap, io};

#[derive(Default, Deserialize)]
#[serde(default)]
struct Session {
    page: Page,
    selected: Option<String>,
    device: DeviceUi,
    edits: HashMap<String, DeviceEdit>,
    lighting: LightingUi,
    macros: MacrosUi,
}

fn path() -> std::path::PathBuf { mirage_hub_core::paths::studio_dir().join("session.json") }

pub fn save(app: &App) -> io::Result<()> {
    let value = serde_json::json!({
        "page": app.page, "selected": app.selected, "device": app.device_ui,
        "edits": app.edits, "lighting": app.lighting, "macros": app.macros,
    });
    mirage_hub_core::paths::write_atomic(&path(), &serde_json::to_vec(&value)?)
}

pub fn restore(app: &mut App) {
    let path = path();
    if !std::fs::metadata(&path).is_ok_and(|m| m.len() <= 16 * 1024 * 1024) { return; }
    let Some(saved) = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<Session>(&bytes).ok()) else { return };
    app.page = saved.page;
    app.selected = saved.selected;
    app.device_ui = saved.device;
    app.edits = saved.edits;
    app.lighting = saved.lighting;
    app.macros = saved.macros;
}
