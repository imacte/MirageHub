//! Device API shared by the RPC service, effects engine and hardware backends.

mod blackshark;
mod ble;
mod kraken;
mod inzone;
mod onboard;
mod standard;
pub use blackshark::BlackSharkDevice;
pub use ble::BleDevice;
pub use kraken::KrakenDevice;
pub use inzone::InzoneDevice;
pub use standard::StandardDevice;

use crate::{database::DeviceInfo, persistence::SharedStore, Error, Result};
use serde_json::{json, Map, Value};

pub trait Device: Send {
    fn info(&self) -> &DeviceInfo;
    fn persistence(&self) -> &SharedStore;
    fn serial(&mut self) -> String;
    fn firmware(&mut self) -> Result<String>;
    fn capabilities(&self) -> Value;
    fn call(&mut self, method: &str, args: &[Value], kwargs: &Map<String, Value>) -> Result<Value>;
    fn enter_driver_mode(&mut self, keys_handled: bool) -> Result<()>;
    fn leave_driver_mode(&mut self) -> Result<()>;
    fn restore(&mut self) -> Result<()>;
    fn close(&mut self);
    fn is_connected(&self) -> bool {
        true
    }
    fn custom_mode_first(&self) -> bool {
        false
    }
}

pub(crate) struct Arguments<'a> {
    pub args: &'a [Value],
    pub kwargs: &'a Map<String, Value>,
}
impl Arguments<'_> {
    pub fn value(&self, name: &str, position: usize, default: Option<Value>) -> Result<Value> {
        if self.args.get(position).is_some() && self.kwargs.contains_key(name) {
            return Err(Error::Invalid(format!("duplicate argument {name}")));
        }
        self.args
            .get(position)
            .or_else(|| self.kwargs.get(name))
            .cloned()
            .or(default)
            .ok_or_else(|| Error::Invalid(format!("missing argument {name}")))
    }
    pub fn number(&self, name: &str, position: usize, default: Option<i64>) -> Result<i64> {
        integer(&self.value(name, position, default.map(|v| json!(v)))?)
    }
    pub fn float(&self, name: &str, position: usize, default: Option<f64>) -> Result<f64> {
        let value = self.value(name, position, default.map(|v| json!(v)))?;
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
            .filter(|v| v.is_finite())
            .ok_or_else(|| Error::Invalid(format!("{name} must be a finite number")))
    }
    pub fn boolean(&self, name: &str, position: usize, default: Option<bool>) -> Result<bool> {
        let value = self.value(name, position, default.map(|v| json!(v)))?;
        value
            .as_bool()
            .or_else(|| value.as_i64().map(|n| n != 0))
            .ok_or_else(|| Error::Invalid(format!("{name} must be boolean")))
    }
    pub fn zone(&self, position: usize) -> Result<String> {
        self.value("zone", position, Some(json!("backlight")))?
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::Invalid("zone must be a string".into()))
    }
}

pub(crate) fn integer(value: &Value) -> Result<i64> {
    value
        .as_i64()
        .or_else(|| value.as_bool().map(i64::from))
        .or_else(|| {
            value
                .as_f64()
                .filter(|v| v.is_finite() && *v >= i64::MIN as f64 && *v < i64::MAX as f64)
                .map(|v| v as i64)
        })
        .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
        .ok_or_else(|| Error::Invalid("expected an integer".into()))
}

pub(crate) fn bindings(value: Value) -> Map<String, Value> {
    value.as_object().expect("literal bindings object").clone()
}

pub(crate) fn restore_lighting(device: &mut dyn Device) {
    let serial = device.serial();
    let capabilities = device.capabilities();
    let zones = crate::persistence::ZONES
        .into_iter()
        .filter(|zone| capabilities["zones"].get(*zone).is_some())
        .collect::<Vec<_>>();
    for zone in zones {
        let state = crate::persistence::lock(device.persistence()).saved_zone(&serial, zone);
        let Some(state) = state else { continue };
        let kwargs = bindings(json!({"zone":zone}));
        if let Some(brightness) = state.get("brightness").filter(|v| !v.is_null()) {
            if device
                .call("set_brightness", std::slice::from_ref(brightness), &kwargs)
                .is_err()
            {
                continue;
            }
        }
        let Some(effect) = state.get("effect").and_then(Value::as_str) else {
            continue;
        };
        let mut colours = state
            .get("colors")
            .and_then(Value::as_array)
            .cloned()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| {
                vec![
                    json!(0),
                    json!(255),
                    json!(0),
                    json!(0),
                    json!(255),
                    json!(255),
                    json!(0),
                    json!(0),
                    json!(255),
                ]
            });
        colours.resize(9, json!(0));
        let speed = state.get("speed").cloned().unwrap_or(json!(1));
        let args = match effect {
            "static" | "breath_single" | "blinking" => colours[..3].to_vec(),
            "breath_dual" => colours[..6].to_vec(),
            "breath_triple" => colours,
            "reactive" | "starlight_single" => {
                let mut args = colours[..3].to_vec();
                args.push(speed);
                args
            }
            "starlight_dual" => {
                let mut args = colours[..6].to_vec();
                args.push(speed);
                args
            }
            "starlight_random" => vec![speed],
            "wave" | "wheel" => vec![state.get("wave_dir").cloned().unwrap_or(json!(1))],
            "none" | "spectrum" | "breath_random" => vec![],
            _ => continue,
        };
        let _ = device.call(&format!("set_{effect}"), &args, &kwargs);
    }
}
