//! Metadata is embedded directly from the upstream synchronization output.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    #[serde(rename = "type")]
    pub device_type: String,
    #[serde(default)]
    pub class_name: String,
    #[serde(default)]
    pub has_matrix: bool,
    pub matrix_dims: Option<[usize; 2]>,
    #[serde(default)]
    pub dedicated_macro_keys: bool,
    #[serde(default)]
    pub driver_mode: bool,
    pub wave_dirs: Option<Vec<u8>>,
    pub poll_rates: Option<Vec<u32>>,
    pub dpi_max: Option<u32>,
    pub image: Option<String>,
    #[serde(default)]
    pub images: BTreeMap<String, String>,
    #[serde(default)]
    pub methods: BTreeSet<String>,
    #[serde(default = "hid_transport")]
    pub transport: String,
}

fn hid_transport() -> String {
    "hid".into()
}

impl DeviceInfo {
    pub fn driver(&self) -> &str {
        match self.device_type.as_str() {
            "keyboard" | "keypad" => "kbd",
            "mouse" => "mouse",
            _ => "accessory",
        }
    }

    pub fn key(&self) -> String {
        format!("{:04x}:{:04x}", self.vid, self.pid)
    }

    pub fn has(&self, method: &str) -> bool {
        self.methods.contains(method)
    }

    pub fn wave_dirs(&self) -> &[u8] {
        self.wave_dirs
            .as_deref()
            .filter(|v| !v.is_empty())
            .unwrap_or(&[1, 2])
    }

    pub fn zones(&self) -> Vec<&str> {
        [
            "backlight",
            "logo",
            "scroll",
            "left",
            "right",
            "charging",
            "fast_charging",
            "fully_charged",
        ]
        .into_iter()
        .filter(|zone| {
            self.methods
                .iter()
                .any(|m| m.starts_with(&format!("set_{zone}_")))
                || (*zone == "backlight"
                    && (self.has("set_static_effect") || self.has("set_spectrum_effect")))
        })
        .collect()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name, "vid": self.vid, "pid": self.pid, "type": self.device_type,
            "driver": self.driver(), "has_matrix": self.has_matrix, "matrix_dims": self.matrix_dims,
            "dedicated_macro_keys": self.dedicated_macro_keys, "wave_dirs": self.wave_dirs(),
            "poll_rates": self.poll_rates, "dpi_max": self.dpi_max, "image": self.image,
            "images": self.images, "transport": self.transport, "methods": self.methods, "zones": self.zones(),
        })
    }
}

pub fn devices() -> &'static [DeviceInfo] {
    static DEVICES: OnceLock<Vec<DeviceInfo>> = OnceLock::new();
    DEVICES.get_or_init(|| {
        #[derive(Deserialize)]
        struct Database {
            devices: Vec<DeviceInfo>,
        }
        let mut entries =
            serde_json::from_str::<Database>(include_str!("../../../data/devices.json"))
                .expect("the embedded device database must be valid")
                .devices;
        entries.sort_by(|a, b| (&a.device_type, &a.name).cmp(&(&b.device_type, &b.name)));
        entries
    })
}

pub fn lookup(vid: u16, pid: u16) -> Option<&'static DeviceInfo> {
    devices().iter().find(|d| d.vid == vid && d.pid == pid)
}
