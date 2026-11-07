use crate::{persistence::atomic_write, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, RwLock},
};

pub const MAX_EVENTS: usize = 1000;
pub const MAX_DELAY_MS: u32 = 30_000;
pub const MAX_TEXT: usize = 4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroEvent {
    pub vk: u16,
    pub scan: u16,
    pub ext: bool,
    pub down: bool,
    pub delay: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Action {
    Keys { events: Vec<MacroEvent> },
    Text { text: String },
    Launch { target: String },
    Disabled,
}
fn integer(value: Option<&Value>, default: i64) -> Option<i64> {
    match value {
        None => Some(default),
        Some(Value::Bool(v)) => Some(i64::from(*v)),
        Some(Value::String(s)) => s.trim().parse().ok(),
        Some(Value::Number(n)) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|v| v.is_finite() && *v >= i64::MIN as f64 && *v < i64::MAX as f64)
                .map(|v| v as i64)
        }),
        _ => None,
    }
}
fn truth(value: Option<&Value>, default: bool) -> bool {
    match value {
        None => default,
        Some(Value::Null) => false,
        Some(Value::Bool(v)) => *v,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}
pub fn validate(value: &Value) -> Result<Action> {
    let object = value
        .as_object()
        .ok_or_else(|| Error::Invalid("a binding is an object with a \"type\"".into()))?;
    Ok(match object.get("type").and_then(Value::as_str) {
        Some("keys") => {
            let events = object
                .get("events")
                .and_then(Value::as_array)
                .filter(|e| !e.is_empty())
                .ok_or_else(|| Error::Invalid("a key macro needs at least one event".into()))?;
            if events.len() > MAX_EVENTS {
                return Err(Error::Invalid(format!("at most {MAX_EVENTS} events")));
            }
            let events = events
                .iter()
                .map(|value| {
                    let event = value
                        .as_object()
                        .ok_or_else(|| Error::Invalid(format!("malformed event: {value}")))?;
                    let (vk, scan, delay) = (
                        integer(event.get("vk"), 0),
                        integer(event.get("scan"), 0),
                        integer(event.get("delay"), 0),
                    );
                    let (Some(vk), Some(scan), Some(delay)) = (vk, scan, delay) else {
                        return Err(Error::Invalid(format!("malformed event: {value}")));
                    };
                    if !(0..=255).contains(&vk)
                        || !(0..=65535).contains(&scan)
                        || (vk == 0 && scan == 0)
                    {
                        return Err(Error::Invalid(
                            "an event needs a virtual key or a scan code".into(),
                        ));
                    }
                    if !(0..=i64::from(MAX_DELAY_MS)).contains(&delay) {
                        return Err(Error::Invalid(format!("delays are 0-{MAX_DELAY_MS} ms")));
                    }
                    Ok(MacroEvent {
                        vk: vk as u16,
                        scan: scan as u16,
                        delay: delay as u32,
                        ext: truth(event.get("ext"), false),
                        down: truth(event.get("down"), true),
                    })
                })
                .collect::<Result<_>>()?;
            Action::Keys { events }
        }
        Some("text") => {
            let text = object
                .get("text")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| Error::Invalid("a text macro needs some text".into()))?;
            if text.chars().count() > MAX_TEXT {
                return Err(Error::Invalid(format!("at most {MAX_TEXT} characters")));
            }
            Action::Text { text: text.into() }
        }
        Some("launch") => {
            let target = object
                .get("target")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    Error::Invalid("a launch binding needs a program, file or URL".into())
                })?;
            Action::Launch {
                target: target.into(),
            }
        }
        Some("disabled") => Action::Disabled,
        _ => {
            return Err(Error::Invalid(
                "unknown binding type; expected keys, text, launch, disabled".into(),
            ))
        }
    })
}

/// Readers, including the keyboard hook, never wait for a filesystem write.
/// The writer lock keeps snapshots and atomic replacements in the same order.
pub struct MacroStore {
    path: Option<PathBuf>,
    data: RwLock<Map<String, Value>>,
    writer: Mutex<()>,
}
impl MacroStore {
    pub fn memory() -> Self {
        Self {
            path: None,
            data: RwLock::new(Map::new()),
            writer: Mutex::new(()),
        }
    }
    pub fn load(path: PathBuf) -> Self {
        let mut data = fs::read(&path)
            .ok()
            .and_then(|v| serde_json::from_slice::<Value>(&v).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        data.retain(|_, v| v.is_object());
        Self {
            path: Some(path),
            data: RwLock::new(data),
            writer: Mutex::new(()),
        }
    }
    pub fn bindings(&self, serial: &str) -> Value {
        self.data
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(serial)
            .cloned()
            .unwrap_or_else(|| json!({}))
    }
    pub fn get(&self, serial: &str, key: &str) -> Option<Value> {
        self.data
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(serial)?
            .get(key)
            .filter(|v| !v.is_null())
            .cloned()
    }
    pub fn set(&self, serial: &str, key: &str, action: &Value) -> Result<Value> {
        let clean = serde_json::to_value(validate(action)?)?;
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let snapshot = {
            let mut data = self.data.write().unwrap_or_else(|e| e.into_inner());
            data.entry(serial)
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .expect("serials are objects")
                .insert(key.into(), clean.clone());
            data.clone()
        };
        self.save(&snapshot)?;
        Ok(clean)
    }
    pub fn delete(&self, serial: &str, key: &str) -> Result<bool> {
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let snapshot = {
            let mut data = self.data.write().unwrap_or_else(|e| e.into_inner());
            let Some(keys) = data.get_mut(serial).and_then(Value::as_object_mut) else {
                return Ok(false);
            };
            if keys.remove(key).is_none() {
                return Ok(false);
            }
            if keys.is_empty() {
                data.remove(serial);
            }
            data.clone()
        };
        self.save(&snapshot)?;
        Ok(true)
    }
    fn save(&self, snapshot: &Map<String, Value>) -> Result<()> {
        if let Some(path) = &self.path {
            atomic_write(path, &serde_json::to_vec_pretty(snapshot)?)?;
        }
        Ok(())
    }
}
