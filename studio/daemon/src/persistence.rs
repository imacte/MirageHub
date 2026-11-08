//! The original per-user JSON schema, including the distinction between a
//! displayed default and a setting the user actually saved.

use crate::{Error, Result};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

pub const ZONES: [&str; 8] = [
    "backlight",
    "logo",
    "scroll",
    "left",
    "right",
    "charging",
    "fast_charging",
    "fully_charged",
];
pub type SharedStore = Arc<Mutex<Persistence>>;
pub fn lock(store: &SharedStore) -> MutexGuard<'_, Persistence> {
    store.lock().unwrap_or_else(|error| error.into_inner())
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        })
        .join("rsrazer")
}

pub fn default_zone() -> Map<String, Value> {
    json!({"effect":"spectrum", "brightness":75, "colors":[0,255,0,0,255,255,0,0,255], "speed":1, "wave_dir":1})
        .as_object().expect("literal object").clone()
}

pub struct Persistence {
    pub path: Option<PathBuf>,
    data: Map<String, Value>,
    dirty: bool,
    held: BTreeSet<String>,
}
impl Persistence {
    pub fn memory() -> Self {
        Self {
            path: None,
            data: Map::new(),
            dirty: false,
            held: BTreeSet::new(),
        }
    }
    pub fn load(path: PathBuf) -> Self {
        let data = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        Self {
            path: Some(path),
            data,
            dirty: false,
            held: BTreeSet::new(),
        }
    }
    pub fn shared(self) -> SharedStore {
        Arc::new(Mutex::new(self))
    }
    pub fn save(&mut self) -> Result<()> {
        if self.dirty {
            if let Some(path) = &self.path {
                atomic_write(path, &serde_json::to_vec_pretty(&self.data)?)?;
            }
            self.dirty = false;
        }
        Ok(())
    }
    pub fn saved_zone(&self, serial: &str, zone: &str) -> Option<Map<String, Value>> {
        self.data
            .get(serial)?
            .get("zones")?
            .get(zone)?
            .as_object()
            .cloned()
    }
    pub fn zone(&self, serial: &str, zone: &str) -> Map<String, Value> {
        let mut state = default_zone();
        if let Some(saved) = self.saved_zone(serial, zone) {
            state.extend(saved);
        }
        state
    }
    pub fn get(&self, serial: &str, zone: &str, key: &str) -> Value {
        self.saved_zone(serial, zone)
            .and_then(|v| v.get(key).cloned())
            .or_else(|| default_zone().get(key).cloned())
            .unwrap_or(Value::Null)
    }
    fn device(&mut self, serial: &str) -> &mut Map<String, Value> {
        object(self.data.entry(serial).or_insert_with(|| json!({})))
    }
    pub fn set(&mut self, serial: &str, zone: &str, key: &str, value: Value) {
        if self.held.contains(serial) {
            return;
        }
        let zones = object(
            self.device(serial)
                .entry("zones")
                .or_insert_with(|| json!({})),
        );
        object(zones.entry(zone).or_insert_with(|| json!({}))).insert(key.into(), value);
        self.dirty = true;
    }
    pub fn get_device(&mut self, serial: &str, key: &str, default: Value) -> Value {
        self.device(serial).get(key).cloned().unwrap_or(default)
    }
    pub fn device_values(&mut self, serial: &str) -> Map<String, Value> {
        self.device(serial).clone()
    }
    pub fn set_device(&mut self, serial: &str, key: &str, value: Value) {
        if self.held.contains(serial) && key == "zone_colours" {
            return;
        }
        self.device(serial).insert(key.into(), value);
        self.dirty = true;
    }
    pub fn clear_device(&mut self, serial: &str, key: &str) {
        if let Some(device) = self.data.get_mut(serial).and_then(Value::as_object_mut) {
            self.dirty |= device.remove(key).is_some();
        }
    }
    pub fn hold_lighting(&mut self, serial: &str) {
        self.held.insert(serial.into());
    }
    pub fn release_lighting(&mut self, serial: &str) {
        self.held.remove(serial);
    }
    pub fn forget(&mut self, serial: &str) {
        self.data.remove(serial);
        self.dirty = true;
    }
    pub fn all_serials(&self) -> Vec<String> {
        self.data.keys().cloned().collect()
    }
    pub fn as_json(&self) -> Value {
        Value::Object(self.data.clone())
    }
}

fn object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("ensured object")
}

pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(directory)?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::Invalid("configuration path has no filename".into()))?;
    for _ in 0..100 {
        let mut temp_name = name.to_os_string();
        temp_name.push(format!(
            ".{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let temp = directory.join(temp_name);
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        let result = (|| {
            file.write_all(contents)?;
            file.sync_all()?;
            drop(file);
            replace(&temp, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        return result;
    }
    Err(Error::Io(
        "cannot allocate a temporary configuration file".into(),
    ))
}

#[cfg(not(windows))]
pub(crate) fn replace(from: &Path, to: &Path) -> Result<()> {
    Ok(fs::rename(from, to)?)
}
#[cfg(windows)]
pub(crate) fn replace(from: &Path, to: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let from: Vec<_> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observing_defaults_does_not_save_an_effect() {
        let mut store = Persistence::memory();
        assert_eq!(store.zone("device", "logo")["effect"], "spectrum");
        assert!(store.saved_zone("device", "logo").is_none());
        store.set("device", "logo", "brightness", json!(37));
        assert_eq!(
            store.saved_zone("device", "logo").unwrap(),
            json!({"brightness":37}).as_object().unwrap().clone()
        );
    }
    #[test]
    fn game_lighting_is_temporary_but_device_settings_are_saved() {
        let mut store = Persistence::memory();
        store.set("device", "logo", "effect", json!("static"));
        store.hold_lighting("device");
        store.set("device", "logo", "effect", json!("custom"));
        store.set_device("device", "zone_colours", json!([[0, 0, 0]]));
        store.set_device("device", "dpi", json!([800, 800]));
        assert_eq!(store.get("device", "logo", "effect"), "static");
        assert!(store
            .get_device("device", "zone_colours", Value::Null)
            .is_null());
        assert_eq!(
            store.get_device("device", "dpi", Value::Null),
            json!([800, 800])
        );
        store.release_lighting("device");
        store.set("device", "logo", "effect", json!("none"));
        assert_eq!(store.get("device", "logo", "effect"), "none");
    }
    #[test]
    fn atomic_replacement_preserves_the_schema_and_leaves_no_temporary_files() {
        let dir = std::env::temp_dir().join(format!(
            "openrazer-rust-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("配置.json");
        let mut store = Persistence::load(path.clone());
        store.set("serial", "backlight", "brightness", json!(20));
        store.save().unwrap();
        store.set("serial", "backlight", "brightness", json!(40));
        store.save().unwrap();
        let loaded = Persistence::load(path.clone());
        assert_eq!(
            loaded.as_json(),
            json!({"serial":{"zones":{"backlight":{"brightness":40}}}})
        );
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
