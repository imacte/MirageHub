//! Named snapshots in the existing profiles.json format. Loading a profile
//! explicitly applies brightness; reconnecting a BLE device only restores colour.

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

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ProfileState {
    #[serde(default)]
    pub device: Map<String, Value>,
    #[serde(default)]
    pub zones: BTreeMap<String, Map<String, Value>>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub saved: Option<String>,
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
        let data = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            limit: MAX_PROFILES,
            data,
        }
    }
    fn save(&self) -> Result<()> {
        if let Some(path) = &self.path {
            persistence::atomic_write(path, &serde_json::to_vec_pretty(&self.data)?)?;
        }
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
        self.data.get(serial).into_iter().flatten().map(|p|json!({"name":p.name,"saved":p.saved,"zones":p.state.zones.keys().collect::<Vec<_>>()})).collect()
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
        let name = clean_name(name)?;
        let profile = Profile {
            name,
            saved: Some(local_timestamp()),
            state,
        };
        let profiles = self.data.entry(serial.into()).or_default();
        if let Some(existing) = profiles
            .iter_mut()
            .find(|p| p.name.to_lowercase() == profile.name.to_lowercase())
        {
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
        self.save()?;
        Ok(profile)
    }
    pub fn delete(&mut self, serial: &str, name: &str) -> Result<()> {
        let wanted = clean_name(name)?.to_lowercase();
        let profiles = self
            .data
            .get_mut(serial)
            .ok_or_else(|| Error::Invalid(format!("no profile called {name:?}")))?;
        let position = profiles
            .iter()
            .position(|p| p.name.to_lowercase() == wanted)
            .ok_or_else(|| Error::Invalid(format!("no profile called {name:?}")))?;
        profiles.remove(position);
        if profiles.is_empty() {
            self.data.remove(serial);
        }
        self.save()
    }
    pub fn forget(&mut self, serial: &str) -> Result<()> {
        self.data.remove(serial);
        self.save()
    }
    pub fn as_json(&self) -> Value {
        serde_json::to_value(&self.data).expect("JSON profile values")
    }
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
    ProfileState {
        device: values,
        zones,
    }
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
            let _ = device.call("set_brightness", std::slice::from_ref(level), &kwargs);
        }
    }
    device.restore()
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
