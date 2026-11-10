// SPDX-License-Identifier: GPL-3.0-only
// Sony control semantics ported from OpenInzone, Copyright (C) 2026 penguinwokrs.
use super::{Arguments, Device};
use crate::{
    database::DeviceInfo,
    hid::HidInfo,
    inzone::{Io, Session},
    persistence::SharedStore,
    protocol::inzone as p,
    Error, Result,
};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub struct InzoneDevice {
    metadata: DeviceInfo,
    info: HidInfo,
    store: SharedStore,
    session: Session,
    serial: String,
    model: Option<u8>,
    raw: BTreeMap<u8, Vec<u8>>,
    discovery: Option<Instant>,
    native_microphone: bool,
}
#[derive(Clone, Copy)]
struct Setting {
    key: &'static str,
    event: u8,
    index: usize,
    width: usize,
    min: u8,
    max: u8,
    kind: &'static str,
}
const SETTINGS: &[Setting] = &[
    Setting {
        key: "headphone_volume",
        event: 0x21,
        index: 1,
        width: 3,
        min: 0,
        max: 30,
        kind: "range",
    },
    Setting {
        key: "headphone_muted",
        event: 0x21,
        index: 0,
        width: 3,
        min: 0,
        max: 1,
        kind: "toggle",
    },
    Setting {
        key: "game_chat_balance",
        event: 0x22,
        index: 0,
        width: 1,
        min: 0,
        max: 100,
        kind: "range",
    },
    Setting {
        key: "sidetone_level",
        event: 0x23,
        index: 0,
        width: 2,
        min: 0,
        max: 10,
        kind: "range",
    },
    Setting {
        key: "microphone_muted",
        event: 0x24,
        index: 0,
        width: 3,
        min: 0,
        max: 1,
        kind: "toggle",
    },
    Setting {
        key: "ambient_mode",
        event: 0x41,
        index: 0,
        width: 4,
        min: 0,
        max: 2,
        kind: "choice",
    },
    Setting {
        key: "ambient_level",
        event: 0x41,
        index: 1,
        width: 4,
        min: 1,
        max: 20,
        kind: "range",
    },
    Setting {
        key: "voice_focus",
        event: 0x41,
        index: 3,
        width: 4,
        min: 0,
        max: 1,
        kind: "toggle",
    },
    Setting {
        key: "auto_power_off",
        event: 0x81,
        index: 0,
        width: 1,
        min: 0,
        max: 180,
        kind: "choice",
    },
    Setting {
        key: "voice_guidance",
        event: 0x84,
        index: 0,
        width: 1,
        min: 0,
        max: 1,
        kind: "toggle",
    },
    Setting {
        key: "voice_guidance_language",
        event: 0x83,
        index: 0,
        width: 1,
        min: 0,
        max: 2,
        kind: "choice",
    },
    Setting {
        key: "bluetooth_auto_switch",
        event: 0x8e,
        index: 0,
        width: 1,
        min: 0,
        max: 1,
        kind: "toggle",
    },
];
impl InzoneDevice {
    pub fn new(metadata: DeviceInfo, info: HidInfo, io: Box<dyn Io>, store: SharedStore) -> Self {
        let hash = info.path.to_ascii_lowercase().bytes().fold(0xcbf29ce484222325u64,|hash,byte|(hash^u64::from(byte)).wrapping_mul(0x100000001b3));
        let serial = if info.path.starts_with("emulated:") {
            info.serial.clone()
        } else if info.serial.trim().is_empty() {
            format!("SONY{:04X}{hash:016X}", info.product_id)
        } else {
            format!("SONY{}", info.serial.trim())
        };
        Self {
            metadata,
            info,
            store,
            session: Session::new(io),
            serial,
            model: None,
            raw: BTreeMap::new(),
            discovery: None,
            native_microphone: false,
        }
    }
    pub fn with_native_microphone(mut self, enabled: bool) -> Self {
        self.native_microphone = enabled;
        self
    }
    fn identify(&mut self) -> Result<()> {
        if self.model.is_some() {
            return Ok(());
        }
        let bytes = self.session.request(2, None)?;
        if bytes.len() < 6 {
            return Err(Error::Device("Short INZONE model reply".into()));
        }
        self.model = Some(bytes[0]);
        if let Some(name) = p::model_name(bytes[0]) {
            self.metadata.name = name.into();
        }
        // The serial used by the registry remains stable while a headset powers off/on.
        Ok(())
    }
    fn read(&mut self, event: u8) -> Result<Vec<u8>> {
        let bytes = self.session.request(event, None)?;
        self.raw.insert(event, bytes.clone());
        Ok(bytes)
    }
    fn refresh(&mut self) -> Result<bool> {
        let linked = self.session.request(1, None)?;
        if !linked.first().is_some_and(|b| *b != 0 && *b != 0xff) {
            self.raw.clear();
            self.discovery = None;
            return Ok(false);
        }
        self.identify()?;
        let discover = self
            .discovery
            .is_none_or(|at| at.elapsed() > Duration::from_secs(60));
        let parts = [
            self.session.request(6, None).ok(),
            self.session.request(7, None).ok(),
            self.session.request(8, None).ok(),
        ];
        let map = p::capability_map(
            [
                parts[0].as_deref(),
                parts[1].as_deref(),
                parts[2].as_deref(),
            ],
            self.model,
        );
        // Invalid map parts never turn stale values into confirmed capabilities.
        let previous = std::mem::take(&mut self.raw);
        self.raw = map;
        let mut events: Vec<u8> = SETTINGS.iter().map(|s| s.event).chain(Some(4)).collect();
        events.sort_unstable();
        events.dedup();
        for event in events {
            if self.raw.contains_key(&event) {
                continue;
            }
            if event == 0x8e && self.model != Some(4) {
                continue;
            }
            if discover
                || previous
                    .get(&event)
                    .is_some_and(|bytes| bytes.iter().any(|b| *b != 0xff))
            {
                let _ = self.read(event);
            }
        }
        self.discovery = Some(Instant::now());
        Ok(true)
    }
    fn valid(&self, setting: Setting, bytes: &[u8]) -> bool {
        if bytes.len() != setting.width {
            return false;
        }
        let value = bytes[setting.index];
        if !(setting.min..=setting.max).contains(&value) {
            return false;
        }
        if setting.key == "auto_power_off" {
            return if matches!(self.model, Some(0 | 3)) {
                [0, 5, 15, 30, 60, 180].contains(&value)
            } else {
                [0, 15].contains(&value)
            };
        }
        true
    }
    fn settings(&self) -> Vec<Value> {
        SETTINGS.iter().filter_map(|s| {
            let bytes=self.raw.get(&s.event)?;
            if !self.valid(*s,bytes) {return None;}
            let value=bytes[s.index];
            let readonly=s.key=="microphone_muted" && self.model!=Some(4);
            let choices:Vec<u8>=match s.key {
                "auto_power_off" if matches!(self.model,Some(0|3)) => vec![0,5,15,30,60,180],
                "auto_power_off" => vec![0,15],
                _ if s.kind=="choice" => (s.min..=s.max).collect(), _=>vec![],
            };
            Some(json!({"key":s.key,"method":format!("set_{}",s.key),"value":value,"kind":s.kind,"min":s.min,"max":s.max,"choices":choices,"readonly":readonly,
                "enabled": !matches!(s.key,"ambient_level"|"voice_focus") || self.raw.get(&0x41).and_then(|b|b.first())==Some(&2)}))
        }).collect()
    }
    fn battery(&mut self) -> Result<Value> {
        let bytes = self.read(4)?;
        if !matches!(bytes.len(), 2 | 6) {
            return Err(Error::Device("Invalid INZONE battery reply".into()));
        }
        Ok(battery_parts(&bytes))
    }
    fn audio(&mut self) -> Result<Value> {
        if !self.refresh()? {
            return Ok(json!({"connected":false,"settings":[]}));
        }
        let mute = self
            .raw
            .get(&0x24)
            .and_then(|b| b.first())
            .filter(|v| **v <= 1)
            .map(|v| *v == 1);
        let mut settings = self.settings();
        if self.native_microphone {
            if let Ok(level) =
                crate::inzone::microphone::level(self.info.vendor_id, self.info.product_id, None)
            {
                settings.push(json!({"key":"microphone_level","method":"set_microphone_level","kind":"range","min":0,"max":100,"value":level}));
            }
        }
        Ok(
            json!({"connected":true,"model":self.model,"settings":settings,"microphone_muted":mute,
            "sony_apo": if self.native_microphone && self.model==Some(3) && self.info.product_id==0x0ebf {crate::inzone::apo::state()} else {Value::Null},
            "battery_parts":self.raw.get(&4).map(|bytes|battery_parts(bytes))}),
        )
    }
    fn set(&mut self, setting: Setting, value: u8) -> Result<Value> {
        self.identify()?;
        if setting.key == "microphone_muted" && self.model != Some(4) {
            return Err(Error::NotSupported(
                "This INZONE headset mutes with its physical microphone control".into(),
            ));
        }
        let mut bytes = self.read(setting.event)?;
        if !self.valid(setting, &bytes) {
            return Err(Error::NotSupported(format!(
                "INZONE does not report {}",
                setting.key
            )));
        }
        bytes[setting.index] = value;
        if !self.valid(setting, &bytes) {
            return Err(Error::Invalid(format!("Invalid value for {}", setting.key)));
        }
        let reply = self.session.request(setting.event, Some(&bytes))?;
        if !self.valid(setting, &reply) || reply[setting.index] != value {
            return Err(Error::Device(
                "INZONE did not confirm the requested setting".into(),
            ));
        }
        self.raw.insert(setting.event, reply);
        Ok(Value::Null)
    }
}
pub fn battery_parts(bytes: &[u8]) -> Value {
    if !matches!(bytes.len(), 2 | 6) {
        return Value::Null;
    }
    let names: &[&str] = if bytes.len() == 6 {
        &["left", "right", "case"]
    } else {
        &["headset"]
    };
    json!(names.iter().enumerate().map(|(index,name)|json!({"part":name,"percent":(bytes[index*2+1]<=100).then_some(bytes[index*2+1]),"status":bytes[index*2],"snapshot":*name=="case"})).collect::<Vec<_>>())
}
impl Device for InzoneDevice {
    fn info(&self) -> &DeviceInfo {
        &self.metadata
    }
    fn persistence(&self) -> &SharedStore {
        &self.store
    }
    fn serial(&mut self) -> String {
        self.serial.clone()
    }
    fn firmware(&mut self) -> Result<String> {
        self.identify()?;
        Err(Error::NotSupported(
            "INZONE firmware version format has not been verified".into(),
        ))
    }
    fn capabilities(&self) -> Value {
        let connection = if matches!(self.model,Some(0|1|3|4|5)) || self.model.is_none() && matches!(self.info.product_id,0x0ebf|0x0ec2|0x0e53) {"wireless"} else {"usb"};
        json!({"name":self.metadata.name,"type":"headset","vid":self.metadata.vid,"pid":self.metadata.pid,"protocol":"inzone","zones":{},"battery":true,"readback":true,"audio":{"settings":true},"transport":{"path":self.info.path,"connection":connection,"serial":self.info.product_id==0x0e53}})
    }
    fn enter_driver_mode(&mut self, _: bool) -> Result<()> {
        Ok(())
    }
    fn leave_driver_mode(&mut self) -> Result<()> {
        Ok(())
    }
    fn restore(&mut self) -> Result<()> {
        Ok(())
    }
    fn close(&mut self) {
        self.session.close();
    }
    fn call(
        &mut self,
        method: &str,
        values: &[Value],
        kwargs: &Map<String, Value>,
    ) -> Result<Value> {
        let args = Arguments {
            args: values,
            kwargs,
        };
        match method {
            "available_effects" => Ok(json!([])),
            "restore" => Ok(Value::Null),
            "get_audio" => self.audio(),
            "set_sony_effects" | "set_sony_equalizer" | "set_sony_bands" | "set_sony_spatial" | "set_sony_drc" | "set_sony_auto_gain" | "restore_sony_microphone" | "restore_sony_audio" | "prepare_sony_audio" | "install_sony_audio" => {
                if !self.native_microphone || self.info.product_id != 0x0ebf {
                    return Err(Error::NotSupported("Sony APO is available only for a physical H5 receiver".into()));
                }
                self.identify()?;
                if self.model != Some(3) {return Err(Error::NotSupported("Sony APO controls are currently verified only for H5".into()));}
                crate::inzone::apo::change(method, &args.value("value", 0, Some(Value::Null))?)
            }
            "set_microphone_level" => {
                if !self.native_microphone {
                    return Err(Error::NotSupported(
                        "No Windows microphone for this backend".into(),
                    ));
                }
                let value = args.number("value", 0, None)?;
                if !(0..=100).contains(&value) {
                    return Err(Error::Invalid("Microphone level must be 0..100".into()));
                }
                crate::inzone::microphone::level(
                    self.info.vendor_id,
                    self.info.product_id,
                    Some(value as u8),
                )?;
                Ok(Value::Null)
            }
            "get_battery_parts" => self.battery(),
            "get_battery_level" => {
                let parts = self.battery()?;
                Ok(parts
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|part| part["part"] != "case")
                    .filter_map(|part| part["percent"].as_u64())
                    .min()
                    .map_or(Value::Null, |v| json!(v)))
            }
            "is_charging" => Ok(Value::Null), // Battery status bytes are not a documented charging flag.
            "get_microphone_muted" => {
                let bytes = self.read(0x24)?;
                Ok(bytes
                    .first()
                    .filter(|v| **v <= 1)
                    .map_or(Value::Null, |v| json!(*v == 1)))
            }
            _ => {
                let key = method.strip_prefix("set_").unwrap_or_default();
                let setting = *SETTINGS.iter().find(|s| s.key == key).ok_or_else(|| {
                    Error::NotSupported(format!("INZONE does not support {method}"))
                })?;
                let value = if setting.kind == "toggle" {
                    u8::from(args.boolean("enabled", 0, None)?)
                } else {
                    u8::try_from(args.number("value", 0, None)?)
                        .map_err(|_| Error::Invalid("INZONE setting must be 0..255".into()))?
                };
                self.set(setting, value)
            }
        }
    }
}
