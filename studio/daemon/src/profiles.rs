//! Active profiles, ported from Ar4ikov/openrazer-win#16 (287c2414).
//! Loading explicitly applies performance and brightness; ordinary reconnects
//! still restore lighting only. Existing profiles.json files remain readable.

use crate::{device::Device, persistence, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, fs, path::PathBuf};

pub const MAX_PROFILES: usize = 10;
pub const MAX_NAME_LENGTH: usize = 32;

pub fn clean_name(name: &str) -> Result<String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err(Error::Invalid("a profile needs a name".into()));
    }
    if name.chars().count() > MAX_NAME_LENGTH {
        return Err(Error::Invalid(format!(
            "profile names are limited to {MAX_NAME_LENGTH} characters"
        )));
    }
    Ok(name)
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileState {
    #[serde(default)]
    pub device: Map<String, Value>,
    #[serde(default)]
    pub zones: BTreeMap<String, Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snaptap: Option<Value>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub saved: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub active: bool,
    #[serde(flatten)]
    pub state: ProfileState,
}
pub struct ProfileStore {
    pub path: Option<PathBuf>,
    pub limit: usize,
    data: BTreeMap<String, Vec<Profile>>,
}
impl ProfileStore {
    pub fn memory() -> Self {
        Self {
            path: None,
            limit: MAX_PROFILES,
            data: BTreeMap::new(),
        }
    }
    pub fn load(path: PathBuf) -> Self {
        let mut data: BTreeMap<String, Vec<Profile>> = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        // A hand-edited file must not give one device two active profiles.
        for profiles in data.values_mut() {
            let mut found = false;
            for profile in profiles {
                profile.active &= !found;
                found |= profile.active;
            }
        }
        Self {
            path: Some(path),
            limit: MAX_PROFILES,
            data,
        }
    }
    fn replace(&mut self, data: BTreeMap<String, Vec<Profile>>) -> Result<()> {
        if let Some(path) = &self.path {
            persistence::atomic_write(path, &serde_json::to_vec_pretty(&data)?)?;
        }
        // Callers hold the store mutex through the atomic replacement. Failed
        // writes leave both the previous file and the in-memory state intact.
        self.data = data;
        Ok(())
    }
    pub fn names(&self, serial: &str) -> Vec<String> {
        self.data
            .get(serial)
            .into_iter()
            .flatten()
            .map(|p| p.name.clone())
            .collect()
    }
    pub fn summaries(&self, serial: &str) -> Vec<Value> {
        self.data.get(serial).into_iter().flatten().map(|p|json!({"name":p.name,"saved":p.saved,"active":p.active,"zones":p.state.zones.keys().collect::<Vec<_>>()})).collect()
    }
    pub fn active(&self, serial: &str) -> Option<&str> {
        self.data
            .get(serial)?
            .iter()
            .find(|p| p.active)
            .map(|p| p.name.as_str())
    }
    pub fn get(&self, serial: &str, name: &str) -> Result<Profile> {
        let wanted = clean_name(name)?.to_lowercase();
        self.data
            .get(serial)
            .into_iter()
            .flatten()
            .find(|p| p.name.to_lowercase() == wanted)
            .cloned()
            .ok_or_else(|| Error::Invalid(format!("no profile called {name:?}")))
    }
    pub fn put(&mut self, serial: &str, name: &str, state: ProfileState) -> Result<Profile> {
        self.put_with_active(serial, name, state, false)
    }
    pub fn put_with_active(
        &mut self,
        serial: &str,
        name: &str,
        state: ProfileState,
        activate: bool,
    ) -> Result<Profile> {
        let name = clean_name(name)?;
        let mut profile = Profile {
            name,
            saved: Some(local_timestamp()),
            active: activate,
            state,
        };
        let mut data = self.data.clone();
        let profiles = data.entry(serial.into()).or_default();
        if let Some(existing) = profiles
            .iter_mut()
            .find(|p| p.name.to_lowercase() == profile.name.to_lowercase())
        {
            profile.active |= existing.active;
            *existing = profile.clone();
        } else {
            if profiles.len() >= self.limit {
                return Err(Error::Invalid(format!(
                    "this device already has {} profiles; delete one first",
                    self.limit
                )));
            }
            profiles.push(profile.clone());
        }
        if activate {
            for p in profiles {
                p.active = p.name.eq_ignore_ascii_case(&profile.name);
            }
        }
        self.replace(data)?;
        Ok(profile)
    }
    pub fn create(&mut self, serial: &str, name: &str, state: ProfileState) -> Result<Profile> {
        let name = clean_name(name)?;
        if self.get(serial, &name).is_ok() {
            return Err(Error::Invalid(format!(
                "there is already a profile called {name:?}"
            )));
        }
        self.put(serial, &name, state)
    }
    pub fn activate(&mut self, serial: &str, name: &str) -> Result<String> {
        let profile = self.get(serial, name)?;
        if self.active(serial) == Some(&profile.name) {
            return Ok(profile.name);
        }
        let mut data = self.data.clone();
        for p in data.get_mut(serial).expect("profile exists") {
            p.active = p.name == profile.name;
        }
        self.replace(data)?;
        Ok(profile.name)
    }
    pub fn update_active(&mut self, serial: &str, state: ProfileState) -> Result<bool> {
        let Some(name) = self.active(serial).map(str::to_owned) else {
            return Ok(false);
        };
        if self.get(serial, &name)?.state == state {
            return Ok(false);
        }
        self.put(serial, &name, state)?;
        Ok(true)
    }
    pub fn delete(&mut self, serial: &str, name: &str) -> Result<()> {
        let wanted = clean_name(name)?.to_lowercase();
        let mut data = self.data.clone();
        let profiles = data
            .get_mut(serial)
            .ok_or_else(|| Error::Invalid(format!("no profile called {name:?}")))?;
        let position = profiles
            .iter()
            .position(|p| p.name.to_lowercase() == wanted)
            .ok_or_else(|| Error::Invalid(format!("no profile called {name:?}")))?;
        profiles.remove(position);
        if profiles.is_empty() {
            data.remove(serial);
        }
        self.replace(data)
    }
    pub fn forget(&mut self, serial: &str) -> Result<()> {
        let mut data = self.data.clone();
        data.remove(serial);
        self.replace(data)
    }
    pub fn as_json(&self) -> Value {
        serde_json::to_value(&self.data).expect("JSON profile values")
    }
}

pub fn default_state(device: &mut dyn Device) -> ProfileState {
    let caps = device.capabilities();
    let mut state = ProfileState::default();
    if caps["keyswitch_optimization"] == true {
        state
            .device
            .insert("keyswitch_optimization".into(), json!("typing"));
    }
    capture_poll_rate(device, &mut state.device);
    state.zones = caps["zones"]
        .as_object()
        .into_iter()
        .flat_map(|z| z.keys())
        .map(|z| (z.clone(), persistence::default_zone()))
        .collect();
    state
}

pub fn snapshot(device: &mut dyn Device) -> ProfileState {
    let serial = device.serial();
    let capabilities = device.capabilities();
    let mut store = persistence::lock(device.persistence());
    let zones = capabilities["zones"]
        .as_object()
        .into_iter()
        .flat_map(|zones| zones.keys())
        .map(|zone| (zone.clone(), store.zone(&serial, zone)))
        .collect();
    let mut values = store.device_values(&serial);
    values.remove("zones");
    drop(store);
    capture_poll_rate(device, &mut values);
    ProfileState {
        device: values,
        zones,
        snaptap: None,
    }
}

fn capture_poll_rate(device: &mut dyn Device, stored: &mut Map<String, Value>) {
    if stored.get("poll_rate").is_none_or(Value::is_null)
        && device.capabilities()["poll_rate"] == true
    {
        if let Ok(rate) = device.call("get_poll_rate", &[], &Map::new()) {
            stored.insert("poll_rate".into(), rate);
        }
    }
}

/// Read back after *all* profile commands: lighting/mode commands can undo a
/// successful polling write on some firmware. Never replace the saved choice
/// with a mismatched readback.
pub fn verify_poll_rate(device: &mut dyn Device, stored: &Map<String, Value>) -> Result<()> {
    let Some(expected) = stored.get("poll_rate").filter(|v| !v.is_null()) else {
        return Ok(());
    };
    if device.capabilities()["poll_rate"] != true {
        return Ok(());
    }
    for attempt in 0..3 {
        let reported = device.call("get_poll_rate", &[], &Map::new())?;
        if reported == *expected {
            return Ok(());
        }
        if attempt == 2 {
            return Err(Error::Device(format!(
                "{} reports {reported} Hz after applying the saved {expected} Hz",
                device.info().name
            )));
        }
        device.call("set_poll_rate", std::slice::from_ref(expected), &Map::new())?;
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    unreachable!()
}
pub fn apply(device: &mut dyn Device, profile: &ProfileState) -> Result<()> {
    let serial = device.serial();
    {
        let mut store = persistence::lock(device.persistence());
        for (key, value) in &profile.device {
            if key != "zones" {
                store.set_device(&serial, key, value.clone());
            }
        }
        for (zone, values) in &profile.zones {
            for (key, value) in values {
                store.set(&serial, zone, key, value.clone());
            }
        }
    }
    let capabilities = device.capabilities();
    for (zone, values) in &profile.zones {
        if capabilities["zones"][zone]["brightness"] != true {
            continue;
        }
        if let Some(level) = values.get("brightness").filter(|v| !v.is_null()) {
            let kwargs = Map::from_iter([("zone".into(), json!(zone))]);
            device.call("set_brightness", std::slice::from_ref(level), &kwargs)?;
        }
    }
    apply_performance(device, &profile.device)?;
    device.restore()
}

fn apply_performance(device: &mut dyn Device, stored: &Map<String, Value>) -> Result<()> {
    let caps = device.capabilities();
    let kwargs = Map::new();
    if caps["keyswitch_optimization"] == true {
        if let Some(mode) = stored.get("keyswitch_optimization") {
            device.call(
                "set_keyswitch_optimization",
                std::slice::from_ref(mode),
                &kwargs,
            )?;
        }
    }
    if caps["dpi_stages"] == true {
        if let Some(stages) = stored
            .get("dpi_stages")
            .filter(|s| s["stages"].as_array().is_some_and(|a| !a.is_empty()))
        {
            device.call(
                "set_dpi_stages",
                &[
                    stages.get("active").cloned().unwrap_or(json!(1)),
                    stages["stages"].clone(),
                ],
                &kwargs,
            )?;
        }
    }
    // Stage selection can change the DPI, so restore the saved DPI afterwards.
    if caps["dpi"] == true {
        if let Some(dpi) = stored
            .get("dpi")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
        {
            device.call("set_dpi", &dpi[..dpi.len().min(2)], &kwargs)?;
        }
    }
    if caps["poll_rate"] == true {
        if let Some(rate) = stored.get("poll_rate").filter(|v| !v.is_null()) {
            device.call("set_poll_rate", std::slice::from_ref(rate), &kwargs)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn local_timestamp() -> String {
    use windows_sys::Win32::{Foundation::SYSTEMTIME, System::SystemInformation::GetLocalTime};
    let mut time = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    unsafe {
        GetLocalTime(&mut time);
    }
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond
    )
}
#[cfg(not(windows))]
fn local_timestamp() -> String {
    // The production host is Windows. Portable development builds use UTC.
    use std::time::{SystemTime, UNIX_EPOCH};
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = seconds / 86400 + 719468;
    let era = days / 146097;
    let day = days % 146097;
    let year = (day - day / 1460 + day / 36524 - day / 146096) / 365;
    let ordinal = day - (365 * year + year / 4 - year / 100);
    let month = (5 * ordinal + 2) / 153;
    let date = ordinal - (153 * month + 2) / 5 + 1;
    let year = year + era * 400 + u64::from(month >= 10);
    let month = if month < 10 { month + 3 } else { month - 9 };
    format!(
        "{year:04}-{month:02}-{date:02}T{:02}:{:02}:{:02}",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activation_is_unique_and_no_op_saves_keep_the_timestamp() {
        let mut store = ProfileStore::memory();
        store.put("S", "One", ProfileState::default()).unwrap();
        store.put("S", "Two", ProfileState::default()).unwrap();
        assert_eq!(store.active("S"), None);
        assert_eq!(store.activate("S", "one").unwrap(), "One");
        let before = store.as_json();
        assert!(!store.update_active("S", ProfileState::default()).unwrap());
        store.activate("S", "ONE").unwrap();
        assert_eq!(store.as_json(), before);
        store
            .put_with_active("S", "Two", ProfileState::default(), true)
            .unwrap();
        assert_eq!(store.active("S"), Some("Two"));
        assert!(!store.get("S", "One").unwrap().active);
        assert!(store.create("S", " two ", ProfileState::default()).is_err());
        store.delete("S", "Two").unwrap();
        assert_eq!(store.active("S"), None);
    }
    #[test]
    fn a_failed_file_write_cannot_activate_delete_or_replace_in_memory() {
        let blocked = std::env::temp_dir().join(format!(
            "rsrazer-profile-blocked-{}",
            crate::rpc::new_token().unwrap()
        ));
        fs::write(&blocked, "a file cannot contain profiles.json").unwrap();
        let mut store = ProfileStore::memory();
        store
            .put_with_active("S", "One", ProfileState::default(), true)
            .unwrap();
        store.put("S", "Two", ProfileState::default()).unwrap();
        store.path = Some(blocked.join("profiles.json"));
        let before = store.as_json();
        assert!(store.activate("S", "Two").is_err());
        assert!(store.delete("S", "One").is_err());
        assert!(store.create("S", "Three", ProfileState::default()).is_err());
        assert!(store.forget("S").is_err());
        assert_eq!(store.as_json(), before);
        fs::remove_file(blocked).unwrap();
    }
    #[test]
    fn names_are_unicode_and_replacement_preserves_order_at_the_limit() {
        assert_eq!(clean_name("  夜间\t 模式 \n").unwrap(), "夜间 模式");
        assert!(clean_name(&"夜".repeat(32)).is_ok());
        assert!(clean_name(&"夜".repeat(33)).is_err());
        assert!(clean_name("\t\n").is_err());
        let mut store = ProfileStore::memory();
        store.limit = 2;
        store.put("S1", "Night", ProfileState::default()).unwrap();
        store.put("S1", "白天", ProfileState::default()).unwrap();
        assert!(store.put("S1", "third", ProfileState::default()).is_err());
        let mut state = ProfileState::default();
        state.device.insert("dpi".into(), json!(800));
        store.put("S1", "NIGHT", state).unwrap();
        assert_eq!(store.names("S1"), ["NIGHT", "白天"]);
        let mut returned = store.get("S1", "night").unwrap();
        returned.state.device.insert("dpi".into(), json!(10));
        assert_eq!(store.get("S1", "night").unwrap().state.device["dpi"], 800);
        store.put("S2", "night", ProfileState::default()).unwrap();
        store.delete("S1", "night").unwrap();
        assert_eq!(store.names("S1"), ["白天"]);
        assert_eq!(store.names("S2"), ["night"]);
        store.forget("S1").unwrap();
        assert!(store.names("S1").is_empty());
    }
    #[test]
    fn existing_python_json_loads_and_unicode_round_trips_without_changing_the_schema() {
        let path = std::env::temp_dir().join(format!(
            "openrazer-profiles-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path,r#"{"BLE000000000001":[{"name":"夜间","saved":"2026-11-07T13:14:15","device":{"zone_colours":[1,2,3,4,5,6]},"zones":{"backlight":{"effect":"static","brightness":42}}}]}"#).unwrap();
        let mut store = ProfileStore::load(path.clone());
        let profile = store.get("BLE000000000001", "夜间").unwrap();
        assert_eq!(
            profile.state.device["zone_colours"],
            json!([1, 2, 3, 4, 5, 6])
        );
        store.put("BLE000000000001", "白天", profile.state).unwrap();
        let bytes = fs::read_to_string(&path).unwrap();
        assert!(bytes.contains("白天"));
        assert_eq!(ProfileStore::load(path.clone()).as_json(), store.as_json());
        assert_eq!(
            store.summaries("BLE000000000001")[0]["zones"],
            json!(["backlight"])
        );
        fs::write(&path, "{corrupt").unwrap();
        assert!(ProfileStore::load(path.clone()).names("S").is_empty());
        fs::remove_file(path).unwrap();
    }
}
