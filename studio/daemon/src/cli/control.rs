//! CLI operations share the daemon protocol. Direct mode owns only discovery
//! and device handles; it never starts integrations, hooks or effect workers.
use super::{connect, print_json, rpc_error, Arguments};
use crate::{
    effects::frame::Frame,
    manager::{lock_device, DeviceManager},
    persistence,
    profiles::{self, ProfileStore},
    rpc::{Client, SingleInstance},
    Error, Result,
};
use serde_json::{json, Map, Value};
use std::sync::Arc;

struct Direct {
    manager: DeviceManager,
    profiles: ProfileStore,
    snaptap: crate::keys::snaptap_settings::SettingsStore,
    _instance: SingleInstance,
}
impl Direct {
    fn snapshot(&self, device: &mut dyn crate::device::Device) -> profiles::ProfileState {
        let mut state = profiles::snapshot(device);
        if crate::service::snaptap_applies(device) {
            let settings = self.snaptap.snapshot();
            state.snaptap = Some(json!({"enabled":settings.enabled,"pairs":settings.pairs}));
        }
        state
    }
    fn switch(
        &mut self,
        device: &mut dyn crate::device::Device,
        profile: &profiles::Profile,
    ) -> Result<()> {
        self.profiles.activate(&device.serial(), &profile.name)?;
        let applied = (|| {
            profiles::apply(device, &profile.state)?;
            if crate::service::snaptap_applies(device) {
                if let Some(stored) = profile.state.snaptap.as_ref().and_then(Value::as_object) {
                    let mut changes = Map::new();
                    if let Some(pairs) = stored.get("pairs") {
                        changes.insert("pairs".into(), pairs.clone());
                    }
                    if let Some(enabled) = stored.get("enabled").and_then(Value::as_bool) {
                        if !enabled || self.snaptap.snapshot().acknowledged {
                            changes.insert("enabled".into(), json!(enabled));
                        }
                    }
                    if !changes.is_empty() {
                        self.snaptap.update(&changes)?;
                    }
                }
            }
            profiles::verify_poll_rate(device, &profile.state.device)
        })();
        let saved = persistence::lock(&self.manager.persistence).save();
        applied?;
        saved
    }
}
impl Drop for Direct {
    fn drop(&mut self) {
        let _ = self.manager.close();
    }
}
enum Backend {
    Remote(Client),
    Direct(Box<Direct>),
}
impl Backend {
    fn open(args: &Arguments) -> Result<Self> {
        if !args.flag("--direct") {
            return connect(args).map(Self::Remote);
        }
        let name = crate::runtime::instance_name(&args.directory(), args.flag("--emulate"))?;
        let instance = SingleInstance::acquire(&name)?.ok_or_else(|| {
            Error::Invalid("the daemon is running; omit --direct or stop it first".into())
        })?;
        let store =
            persistence::Persistence::load(args.directory().join("persistence.json")).shared();
        let manager = if args.flag("--emulate") {
            DeviceManager::new(Arc::new(crate::demo::DemoBackend::new()?), None, store)
        } else {
            #[cfg(windows)]
            {
                DeviceManager::native(store, true)
            }
            #[cfg(not(windows))]
            {
                return Err(Error::NotSupported(
                    "native hardware requires Windows".into(),
                ));
            }
        };
        let direct = Direct {
            manager,
            profiles: ProfileStore::load(args.directory().join("profiles.json")),
            snaptap: crate::keys::snaptap_settings::SettingsStore::load(
                args.directory().join("snaptap.json"),
            ),
            _instance: instance,
        };
        direct.manager.scan(true)?;
        Ok(Self::Direct(Box::new(direct)))
    }
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let Self::Direct(direct) = self else {
            let Self::Remote(client) = self else {
                unreachable!()
            };
            return client.call(method, params).map_err(rpc_error);
        };
        if method == "devices.list" {
            return Ok(json!(direct.manager.devices().iter().map(|device| {
                let mut d = lock_device(device);
                let serial = d.serial();
                let firmware = d.firmware().ok();
                let info = d.info();
                json!({"serial":serial,"name":info.name,"type":info.device_type,"pid":info.pid,"firmware":firmware})
            }).collect::<Vec<_>>()));
        }
        if method == "effect.set" {
            return Err(Error::NotSupported(
                "software effects need the daemon; omit --direct".into(),
            ));
        }
        let serial = params["serial"].as_str().unwrap_or_default();
        let device = direct
            .manager
            .by_serial(serial)
            .ok_or_else(|| Error::Device(format!("no such device: {serial}")))?;
        let mut device = lock_device(&device);
        let name = params["name"].as_str().unwrap_or_default();
        let result = match method {
            "device.capabilities" => device.capabilities(),
            "device.call" => device.call(
                params["method"].as_str().unwrap_or_default(),
                params["args"].as_array().map_or(&[], Vec::as_slice),
                params["kwargs"].as_object().unwrap_or(&Map::new()),
            )?,
            "profile.list" => json!(direct.profiles.summaries(serial)),
            "profile.save" => {
                let state = direct.snapshot(device.as_mut());
                let profile = direct.profiles.put_with_active(serial, name, state, true)?;
                json!({"name":profile.name,"saved":profile.saved})
            }
            "profile.create" => {
                let state = if let Some(source) = params["source"].as_str() {
                    direct.profiles.get(serial, source)?.state
                } else {
                    let mut state = profiles::default_state(device.as_mut());
                    if crate::service::snaptap_applies(device.as_ref()) {
                        state.snaptap = Some(
                            json!({"enabled":false,"pairs":crate::keys::snaptap_settings::Settings::default().pairs}),
                        );
                    }
                    state
                };
                let profile = direct.profiles.create(serial, name, state)?;
                direct.switch(device.as_mut(), &profile)?;
                json!({"name":profile.name,"saved":profile.saved})
            }
            "profile.load" => {
                let profile = direct.profiles.get(serial, name)?;
                direct.switch(device.as_mut(), &profile)?;
                json!({"name":profile.name})
            }
            "profile.delete" => {
                direct.profiles.delete(serial, name)?;
                json!({"name":name})
            }
            _ => return Err(Error::NotSupported(format!("{method} needs the daemon"))),
        };
        persistence::lock(&direct.manager.persistence).save()?;
        if method == "device.call"
            && !crate::service::read_only(params["method"].as_str().unwrap_or_default())
            && direct.profiles.active(serial).is_some()
        {
            let state = direct.snapshot(device.as_mut());
            direct.profiles.update_active(serial, state)?;
        }
        Ok(result)
    }
    fn device(
        &mut self,
        serial: &str,
        method: &str,
        args: Value,
        zone: Option<&str>,
    ) -> Result<Value> {
        self.call(
            "device.call",
            json!({"serial":serial,"method":method,"args":args,
            "kwargs":zone.map_or(json!({}), |z| json!({"zone":z}))}),
        )
    }
    fn devices(&mut self, selector: Option<&str>) -> Result<Vec<Value>> {
        let devices = self
            .call("devices.list", json!({}))?
            .as_array()
            .cloned()
            .ok_or_else(|| Error::Device("invalid device list from the daemon".into()))?;
        select(devices, selector)
    }
}

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}
fn select(devices: Vec<Value>, selector: Option<&str>) -> Result<Vec<Value>> {
    let Some(selector) = selector.filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("all"))
    else {
        return Ok(devices);
    };
    let needle = selector.to_lowercase();
    let exact: Vec<_> = devices
        .iter()
        .filter(|d| {
            string(d, "serial").to_lowercase() == needle
                || format!("{:04x}", d["pid"].as_u64().unwrap_or_default()) == needle
        })
        .cloned()
        .collect();
    if !exact.is_empty() {
        return Ok(exact);
    }
    let matched: Vec<_> = devices
        .into_iter()
        .filter(|d| string(d, "name").to_lowercase().contains(&needle))
        .collect();
    if matched.is_empty() {
        return Err(Error::Invalid(format!(
            "no device matches {selector:?}; try: rsrazer-daemon list"
        )));
    }
    Ok(matched)
}

pub(super) fn colour(text: &str) -> Result<[u8; 3]> {
    let text = text.trim().to_lowercase();
    let named = match text.as_str() {
        "red" => Some([255, 0, 0]),
        "green" | "razer" => Some([0, 255, 0]),
        "blue" => Some([0, 0, 255]),
        "white" => Some([255, 255, 255]),
        "black" => Some([0, 0, 0]),
        "yellow" => Some([255, 255, 0]),
        "cyan" => Some([0, 255, 255]),
        "magenta" => Some([255, 0, 255]),
        "orange" => Some([255, 96, 0]),
        "purple" => Some([128, 0, 255]),
        "pink" => Some([255, 64, 128]),
        _ => None,
    };
    if let Some(value) = named {
        return Ok(value);
    }
    if text.contains(',') {
        let values: std::result::Result<Vec<i64>, _> =
            text.split(',').map(|v| v.trim().parse()).collect();
        if let Ok(v) = values {
            if v.len() == 3 {
                return Ok([
                    v[0].clamp(0, 255) as u8,
                    v[1].clamp(0, 255) as u8,
                    v[2].clamp(0, 255) as u8,
                ]);
            }
        }
    } else {
        let hex = text.trim_start_matches('#');
        if hex.is_ascii()
            && hex.bytes().all(|c| c.is_ascii_hexdigit())
            && [3, 6].contains(&hex.len())
        {
            let hex = if hex.len() == 3 {
                hex.chars().flat_map(|c| [c, c]).collect::<String>()
            } else {
                hex.into()
            };
            return Ok(std::array::from_fn(|i| {
                u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("validated hex")
            }));
        }
    }
    Err(Error::Invalid(format!(
        "invalid colour {text:?}; use a name, #rrggbb or r,g,b"
    )))
}
fn integer(text: &str) -> Result<i64> {
    text.parse()
        .map_err(|_| Error::Invalid(format!("cannot read {text:?} as an integer")))
}
fn on_off(text: &str) -> Result<bool> {
    match text {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(Error::Invalid("say on or off".into())),
    }
}
fn has_effect(caps: &Value, zone: &str, effect: &str) -> bool {
    caps["zones"][zone]["effects"]
        .as_array()
        .is_some_and(|v| v.iter().any(|e| e == effect))
}
fn zone(caps: &Value, effect: Option<&str>) -> String {
    let Some(zones) = caps["zones"].as_object() else {
        return "backlight".into();
    };
    let primary = if zones.contains_key("backlight") {
        "backlight"
    } else {
        zones.keys().next().map_or("backlight", String::as_str)
    };
    let supports = |z: &str| {
        effect.map_or(caps["zones"][z]["brightness"] == true, |e| {
            has_effect(caps, z, e)
        })
    };
    if supports(primary) {
        return primary.into();
    }
    zones
        .keys()
        .find(|z| supports(z))
        .map_or_else(|| primary.into(), String::clone)
}
fn colour_zones(caps: &Value) -> Vec<String> {
    if let Some(order) = caps["zone_order"].as_array().filter(|v| !v.is_empty()) {
        return order
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
    }
    let mut zones: Vec<_> = caps["zones"]
        .as_object()
        .into_iter()
        .flat_map(|v| v.keys())
        .filter(|z| has_effect(caps, z, "static"))
        .cloned()
        .collect();
    if zones.len() > 1 {
        zones.retain(|z| z != "backlight");
    }
    zones
}
fn summary(device: &Value) -> Value {
    json!({"name":device["name"],"serial":device["serial"],"type":device["type"],
        "pid":format!("{:04x}",device["pid"].as_u64().unwrap_or_default()),"firmware":device["firmware"]})
}

pub(super) const EFFECTS: &[&str] = &[
    "none",
    "on",
    "static",
    "spectrum",
    "wave",
    "wheel",
    "reactive",
    "blinking",
    "custom",
    "breath_random",
    "breath_single",
    "breath_dual",
    "breath_triple",
    "starlight_random",
    "starlight_single",
    "starlight_dual",
    "ripple",
    "ripple_random",
    "music",
    "accent",
    "spectrum_soft",
    "wave_soft",
    "breath_soft",
    "starlight_soft",
];

/// Validate all syntax before discovery or a write to any device.
pub(super) fn validate(args: &Arguments) -> Result<()> {
    let p = &args.positional;
    match p[0].as_str() {
        "effect" => {
            if !EFFECTS.contains(&p[1].as_str()) {
                return Err(Error::Invalid(format!(
                    "unknown effect {}; available: {}",
                    p[1],
                    EFFECTS.join(", ")
                )));
            }
            for c in [
                p.get(2).map(String::as_str),
                args.value("--colour2"),
                args.value("--colour3"),
            ]
            .into_iter()
            .flatten()
            {
                colour(c)?;
            }
            for (name, min, max) in [("--speed", 1, 4), ("--direction", 0, 2)] {
                if let Some(v) = args.value(name) {
                    if !(min..=max).contains(&integer(v)?) {
                        return Err(Error::Invalid(format!(
                            "{name} must be from {min} to {max}"
                        )));
                    }
                }
            }
        }
        "zones" => {
            for c in &p[1..] {
                colour(c)?;
            }
        }
        "profile" => {
            let action = p.get(1).map_or("list", String::as_str);
            if !["list", "save", "load", "delete", "create"].contains(&action) {
                return Err(Error::Invalid(format!("unknown profile action {action}")));
            }
            if action == "list" && p.len() > 2 {
                return Err(Error::Invalid("profile list does not take a name".into()));
            }
            if action != "list" {
                profiles::clean_name(
                    p.get(2)
                        .ok_or_else(|| Error::Invalid(format!("profile {action} needs a name")))?,
                )?;
            }
            if let Some(source) = args.value("--source") {
                if action != "create" {
                    return Err(Error::Invalid("--source is only for profile create".into()));
                }
                profiles::clean_name(source)?;
            }
        }
        "brightness" => {
            if let Some(v) = p.get(1) {
                let level = v
                    .parse::<f64>()
                    .map_err(|_| Error::Invalid("brightness must be a number".into()))?;
                if !level.is_finite() {
                    return Err(Error::Invalid("brightness must be finite".into()));
                }
            }
        }
        "dpi" | "poll-rate" => {
            for v in &p[1..] {
                integer(v)?;
            }
        }
        "keyswitch" => {
            if let Some(mode) = p.get(1) {
                if !["typing", "gaming"].contains(&mode.to_ascii_lowercase().as_str()) {
                    return Err(Error::Invalid(
                        "keyswitch mode must be typing or gaming".into(),
                    ));
                }
            }
        }
        "headset" => {
            if let Some(setting) = p.get(1) {
                let value = p
                    .get(2)
                    .ok_or_else(|| Error::Invalid(format!("{setting} needs a value")))?;
                match setting.as_str() {
                    "sidetone" => {
                        if !["on", "off"].contains(&value.as_str()) {
                            integer(value)?;
                        }
                    }
                    "eq" => {
                        if value.contains(',') {
                            let gains: Vec<_> =
                                value.split(',').map(integer).collect::<Result<_>>()?;
                            if gains.len() != 10 {
                                return Err(Error::Invalid("equalizer needs ten bands".into()));
                            }
                        }
                    }
                    "dnd" => {
                        on_off(value)?;
                    }
                    "led" => {
                        if !["off", "link", "battery", "warning"].contains(&value.as_str()) {
                            return Err(Error::Invalid(
                                "LED mode must be off, link, battery or warning".into(),
                            ));
                        }
                    }
                    "sleep" => {
                        integer(value)?
                            .checked_mul(60)
                            .ok_or_else(|| Error::Invalid("sleep time is too large".into()))?;
                    }
                    _ => return Err(Error::Invalid(format!("unknown headset setting {setting}"))),
                }
            }
        }
        "integration" => {
            if let Some(name) = p.get(1) {
                if !["accent", "chroma"].contains(&name.as_str()) {
                    return Err(Error::Invalid(format!("unknown integration {name}")));
                }
                on_off(
                    p.get(2)
                        .ok_or_else(|| Error::Invalid("integration needs on or off".into()))?,
                )?;
            }
        }
        "macro" => {
            let action = p.get(1).map_or("list", String::as_str);
            if !["list", "set", "clear", "record", "play", "watch"].contains(&action) {
                return Err(Error::Invalid(format!("unknown macro action {action}")));
            }
            if ["list", "watch"].contains(&action) {
                if p.len() > 2 {
                    return Err(Error::Invalid(format!(
                        "macro {action} does not take a key"
                    )));
                }
            } else if p.len() < 3 {
                return Err(Error::Invalid(format!(
                    "macro {action} needs a key, such as M1"
                )));
            }
            let count = ["--keys", "--text", "--launch"]
                .iter()
                .filter(|k| args.value(k).is_some())
                .count()
                + usize::from(args.flag("--disable"));
            if action == "set" && count != 1 {
                return Err(Error::Invalid(
                    "choose exactly one of --keys, --text, --launch or --disable".into(),
                ));
            }
            if action != "set" && count != 0 {
                return Err(Error::Invalid(
                    "binding options are only valid with macro set".into(),
                ));
            }
            if let Some(keys) = args.value("--keys") {
                crate::keys::codes::parse_sequence(keys)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn run(args: &Arguments) -> Result<i32> {
    match args.positional[0].as_str() {
        "integration" => return integration(args),
        "macro" => return macros(args),
        _ => {}
    }
    let mut backend = Backend::open(args)?;
    let command = args.positional[0].as_str();
    let selector = args.value("--device").or_else(|| {
        (command == "info")
            .then(|| args.positional.get(1).map(String::as_str))
            .flatten()
    });
    let devices = backend.devices(selector)?;
    if command == "list" {
        if args.flag("--json") {
            print_json(&json!(devices.iter().map(summary).collect::<Vec<_>>()));
        } else if devices.is_empty() {
            println!(
                "No supported Razer devices found. Run rsrazer-daemon doctor to diagnose detection."
            );
            return Ok(1);
        } else {
            for d in &devices {
                println!(
                    "{:<40} {:<9} {:<18} {}",
                    string(d, "name"),
                    string(d, "type"),
                    string(d, "serial"),
                    string(d, "firmware")
                );
            }
        }
        return Ok(0);
    }
    let mut results = Vec::new();
    let mut failed = false;
    for device in &devices {
        let serial = string(device, "serial");
        let name = string(device, "name");
        let result = (|| -> Result<Option<Value>> {
            let caps = backend.call("device.capabilities", json!({"serial":serial}))?;
            if command == "info" {
                let mut result = summary(device);
                for (key, flag, getter) in [
                    ("battery", "battery", "get_battery_level"),
                    ("charging", "battery", "is_charging"),
                    ("dpi", "dpi", "get_dpi"),
                    ("poll_rate", "poll_rate", "get_poll_rate"),
                    (
                        "keyswitch_optimization",
                        "keyswitch_optimization",
                        "get_keyswitch_optimization",
                    ),
                ] {
                    if caps[flag] == true {
                        result[key] = backend
                            .device(serial, getter, json!([]), None)
                            .unwrap_or(Value::Null);
                    }
                }
                result["capabilities"] = caps;
                return Ok(Some(result));
            }
            if ["effect", "brightness"].contains(&command)
                && selector.is_none()
                && caps["zones"].as_object().is_none_or(|v| v.is_empty())
            {
                return Ok(None);
            }
            match command {
                "effect" => apply_effect(&mut backend, args, serial, &caps),
                "zones" => {
                    let names = colour_zones(&caps);
                    if names.is_empty() {
                        return Ok(None);
                    }
                    let mut colours = args.positional[1..]
                        .iter()
                        .map(|c| colour(c))
                        .collect::<Result<Vec<_>>>()?;
                    if colours.is_empty() {
                        return Ok(Some(json!({"zones":names})));
                    }
                    if colours.len() == 1 {
                        colours = vec![colours[0]; names.len()];
                    }
                    if colours.len() != names.len() {
                        return Err(Error::Invalid(format!(
                            "has {} zones ({}); give that many colours, or one",
                            names.len(),
                            names.join(", ")
                        )));
                    }
                    if caps["zone_colours"] == true {
                        backend.device(serial, "set_zone_colours", json!([colours]), None)?;
                    } else {
                        for (zone, c) in names.iter().zip(&colours) {
                            backend.device(serial, "set_static", json!(c), Some(zone))?;
                        }
                    }
                    Ok(Some(json!({"zones":names,"colours":colours})))
                }
                "profile" => {
                    let action = args.positional.get(1).map_or("list", String::as_str);
                    let mut params = json!({"serial":serial});
                    if action != "list" {
                        params["name"] = json!(args.positional[2]);
                    }
                    if let Some(source) = args.value("--source") {
                        params["source"] = json!(source);
                    }
                    let result = backend.call(&format!("profile.{action}"), params)?;
                    Ok(Some(if action == "list" {
                        json!({"profiles":result})
                    } else {
                        json!({"action":action,"profile":result})
                    }))
                }
                "brightness" => {
                    let zone = args
                        .value("--zone")
                        .map(str::to_string)
                        .unwrap_or_else(|| zone(&caps, None));
                    if let Some(value) = args.positional.get(1) {
                        backend.device(
                            serial,
                            "set_brightness",
                            json!([value.parse::<f64>().expect("validated")]),
                            Some(&zone),
                        )?;
                    }
                    Ok(Some(
                        json!({"zone":zone,"brightness":backend.device(serial,"get_brightness",json!([]),Some(&zone))?}),
                    ))
                }
                "dpi" | "poll-rate" => {
                    let flag = if command == "dpi" { "dpi" } else { "poll_rate" };
                    if caps[flag] != true {
                        return Ok(None);
                    }
                    if let Some(value) = args.positional.get(1) {
                        let x = integer(value)?;
                        let values = if flag == "dpi" {
                            json!([x, args.positional.get(2).map_or(Ok(x), |v| integer(v))?])
                        } else {
                            json!([x])
                        };
                        backend.device(serial, &format!("set_{flag}"), values, None)?;
                    }
                    Ok(Some(
                        json!({flag:backend.device(serial,&format!("get_{flag}"),json!([]),None)?}),
                    ))
                }
                "keyswitch" => {
                    if caps["keyswitch_optimization"] != true {
                        return Ok(None);
                    }
                    if let Some(mode) = args.positional.get(1) {
                        backend.device(
                            serial,
                            "set_keyswitch_optimization",
                            json!([mode]),
                            None,
                        )?;
                    }
                    Ok(Some(
                        json!({"keyswitch_optimization":backend.device(serial, "get_keyswitch_optimization", json!([]), None)?}),
                    ))
                }
                "battery" => {
                    if caps["battery"] != true {
                        return Ok(None);
                    }
                    Ok(Some(
                        json!({"battery":backend.device(serial,"get_battery_level",json!([]),None)?,"charging":backend.device(serial,"is_charging",json!([]),None)?}),
                    ))
                }
                "headset" => {
                    if !crate::rpc::truthy(&caps["audio"]) {
                        return Ok(None);
                    }
                    if let Some(setting) = args.positional.get(1) {
                        set_headset(&mut backend, serial, setting, &args.positional[2])?;
                    }
                    let audio = backend.device(serial, "get_audio", json!([]), None)?;
                    let mut result = json!({"audio":audio});
                    if result["audio"]["connected"] == true {
                        result["battery"] =
                            backend.device(serial, "get_battery_level", json!([]), None)?;
                        result["charging"] =
                            backend.device(serial, "is_charging", json!([]), None)?;
                        result["idle_time"] =
                            backend.device(serial, "get_idle_time", json!([]), None)?;
                    }
                    Ok(Some(result))
                }
                _ => unreachable!("validated CLI command"),
            }
        })();
        match result {
            Ok(Some(mut value)) => {
                if command != "info" {
                    value["device"] = json!(name);
                    value["serial"] = json!(serial);
                }
                if !args.flag("--json") && !args.flag("--quiet") {
                    show(&value);
                }
                results.push(value);
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("{name}: {error}");
                failed = true;
            }
        }
    }
    if args.flag("--json") {
        print_json(&json!(results));
    } else if results.is_empty() && !failed && !args.flag("--quiet") {
        println!("No connected device supports this operation.");
    }
    Ok(i32::from(
        failed || (command == "headset" && results.is_empty()),
    ))
}

fn show(value: &Value) {
    let name = value
        .get("device")
        .or_else(|| value.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    println!("{name}");
    if let Some(profiles) = value.get("profiles").and_then(Value::as_array) {
        for profile in profiles {
            let active = if profile["active"] == true { "*" } else { " " };
            println!(
                "  {active} {}",
                profile["name"].as_str().unwrap_or_default()
            );
        }
        return;
    }
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            if !["device", "name"].contains(&key.as_str()) {
                println!(
                    "  {key:<14} {}",
                    value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_string)
                );
            }
        }
    }
}

fn apply_effect(
    backend: &mut Backend,
    args: &Arguments,
    serial: &str,
    caps: &Value,
) -> Result<Option<Value>> {
    let requested = args.positional[1].as_str();
    let c = colour(args.positional.get(2).map_or("green", String::as_str))?;
    let c2 = colour(args.value("--colour2").unwrap_or("blue"))?;
    let c3 = colour(args.value("--colour3").unwrap_or("red"))?;
    let zone = args.value("--zone").map(str::to_string).unwrap_or_else(|| {
        zone(
            caps,
            Some(
                if ["ripple", "ripple_random", "music", "accent"].contains(&requested) {
                    "custom"
                } else {
                    requested
                },
            ),
        )
    });
    let effect = if has_effect(caps, &zone, requested) {
        requested
    } else {
        match requested {
            "spectrum" => "spectrum_soft",
            "wave" => "wave_soft",
            "breath_single" => "breath_soft",
            "starlight_single" => "starlight_soft",
            _ => requested,
        }
    };
    let speed = integer(args.value("--speed").unwrap_or("1"))?;
    if [
        "ripple",
        "ripple_random",
        "music",
        "accent",
        "spectrum_soft",
        "wave_soft",
        "breath_soft",
        "starlight_soft",
    ]
    .contains(&effect)
    {
        let mut options = json!({"colour":c,"refresh_rate":0.040});
        if !["ripple", "ripple_random"].contains(&effect)
            && caps["zone_order"]
                .as_array()
                .is_some_and(|v| v.contains(&json!(zone)))
        {
            options["zone"] = json!(zone);
        }
        backend.call(
            "effect.set",
            json!({"serial":serial,"effect":effect,"options":options}),
        )?;
    } else if effect == "custom" {
        let dims = caps["matrix_dimensions"]
            .as_array()
            .filter(|v| v.len() == 2)
            .ok_or_else(|| Error::NotSupported("this device has no custom matrix".into()))?;
        let mut frame = Frame::new(
            dims[0].as_u64().unwrap_or(0) as usize,
            dims[1].as_u64().unwrap_or(0) as usize,
        )?;
        frame.fill(c);
        backend.device(serial, "set_key_row", json!([frame.payload()]), None)?;
        backend.device(serial, "set_custom", json!([]), None)?;
    } else {
        let mut values = Vec::new();
        if [
            "static",
            "blinking",
            "breath_single",
            "breath_dual",
            "breath_triple",
            "reactive",
            "starlight_single",
            "starlight_dual",
        ]
        .contains(&effect)
        {
            values.extend(c.map(|v| json!(v)));
        }
        if ["breath_dual", "breath_triple", "starlight_dual"].contains(&effect) {
            values.extend(c2.map(|v| json!(v)));
        }
        if effect == "breath_triple" {
            values.extend(c3.map(|v| json!(v)));
        }
        if [
            "reactive",
            "starlight_single",
            "starlight_dual",
            "starlight_random",
        ]
        .contains(&effect)
        {
            values.push(json!(speed));
        }
        if ["wave", "wheel"].contains(&effect) {
            values.push(json!(integer(args.value("--direction").unwrap_or("1"))?));
        }
        backend.device(serial, &format!("set_{effect}"), json!(values), Some(&zone))?;
    }
    Ok(Some(
        json!({"effect":requested,"zone":zone,"host_rendered":effect!=requested || effect.ends_with("_soft") || ["ripple","ripple_random","music","accent"].contains(&effect)}),
    ))
}

fn set_headset(backend: &mut Backend, serial: &str, setting: &str, value: &str) -> Result<()> {
    let (method, args) = match setting {
        "sidetone" => {
            if !["on", "off"].contains(&value) {
                backend.device(serial, "set_sidetone_level", json!([integer(value)?]), None)?;
            }
            ("set_sidetone", json!([value != "off"]))
        }
        "eq" => {
            if ["on", "off"].contains(&value) {
                ("set_equalizer", json!([on_off(value)?]))
            } else if value.contains(',') {
                (
                    "set_equalizer_bands",
                    json!([value.split(',').map(integer).collect::<Result<Vec<_>>>()?]),
                )
            } else {
                ("set_equalizer_preset", json!([value]))
            }
        }
        "dnd" => ("set_do_not_disturb", json!([on_off(value)?])),
        "led" => ("set_dongle_led", json!([value])),
        "sleep" => (
            "set_idle_time",
            json!([integer(value)?
                .checked_mul(60)
                .ok_or_else(|| Error::Invalid("sleep time is too large".into()))?]),
        ),
        _ => unreachable!("validated setting"),
    };
    backend.device(serial, method, args, None)?;
    Ok(())
}

fn integration(args: &Arguments) -> Result<i32> {
    let mut client = connect(args)?;
    let status = if let Some(name) = args.positional.get(1) {
        client
            .call(
                "integration.set",
                json!({"name":name,"enabled":on_off(&args.positional[2])?}),
            )
            .map_err(rpc_error)?
    } else {
        client
            .call("integration.status", json!({}))
            .map_err(rpc_error)?
    };
    if args.flag("--json") {
        print_json(&status);
    } else if !args.flag("--quiet") {
        show(&status);
    }
    Ok(i32::from(
        status["chroma"]["enabled"] == true && status["chroma"]["listening"] != true,
    ))
}

fn macros(args: &Arguments) -> Result<i32> {
    let action = args.positional.get(1).map_or("list", String::as_str);
    if action == "watch" {
        return super::system::watch(args);
    }
    let mut backend = Backend::Remote(connect(args)?);
    let mut devices = backend.devices(args.value("--device"))?;
    if args.value("--device").is_none() {
        let mut selected = Vec::new();
        for device in devices {
            let info = backend.call("macro.list", json!({"serial":device["serial"]}))?;
            if info["keys"].as_array().is_some_and(|v| !v.is_empty())
                || info["bindings"].as_object().is_some_and(|v| !v.is_empty())
            {
                selected.push(device);
            }
        }
        devices = selected;
    }
    if devices.is_empty() {
        return Err(Error::Invalid(
            "no keyboard with macro keys is connected".into(),
        ));
    }
    if action == "list" {
        let mut results = Vec::new();
        for device in devices {
            let mut info = backend.call("macro.list", json!({"serial":device["serial"]}))?;
            info["device"] = device["name"].clone();
            info["serial"] = device["serial"].clone();
            if !args.flag("--json") && !args.flag("--quiet") {
                show(&info);
            }
            results.push(info);
        }
        if args.flag("--json") {
            print_json(&json!(results));
        }
        return Ok(0);
    }
    if devices.len() != 1 {
        return Err(Error::Invalid(
            "several keyboards match; pick one with --device".into(),
        ));
    }
    let key = args.positional[2].to_uppercase();
    let mut params = json!({"serial":devices[0]["serial"],"key":key});
    let result = match action {
        "set" => {
            params["action"] = if let Some(keys) = args.value("--keys") {
                json!({"type":"keys","events":crate::keys::codes::parse_sequence(keys)?})
            } else if let Some(text) = args.value("--text") {
                json!({"type":"text","text":text})
            } else if let Some(target) = args.value("--launch") {
                json!({"type":"launch","target":target})
            } else {
                json!({"type":"disabled"})
            };
            backend.call("macro.set", params)?
        }
        "clear" => backend.call("macro.delete", params)?,
        "play" => backend.call("macro.play", params)?,
        "record" => {
            backend.call("macro.record_start", params)?;
            eprintln!("Recording {key}: type the macro, then press Enter here.");
            // Read on a worker so Ctrl+C can stop recording and release keys.
            let _console = super::ConsoleHandler::install();
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            std::thread::spawn(move || {
                let _ = tx.send(std::io::stdin().read_line(&mut String::new()));
            });
            let read_result = loop {
                if super::EXIT_REQUESTED.load(std::sync::atomic::Ordering::Acquire) {
                    break Ok(0);
                }
                match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(result) => break result,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break Err(std::io::Error::other("recording input closed")),
                }
            };
            let saved = backend.call("macro.record_stop", json!({}))?;
            read_result?;
            saved
        }
        _ => unreachable!("validated macro action"),
    };
    if args.flag("--json") {
        print_json(&result);
    } else if !args.flag("--quiet") {
        println!("{key}: {action} complete");
    }
    Ok(0)
}
