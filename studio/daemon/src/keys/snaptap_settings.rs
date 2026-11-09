//! Snap Tap settings and consent, compatible with openrazer-win #14.
use super::{
    codes::{self, Output},
    snaptap::{KeyId, BYPASS_VKS},
};
use crate::{persistence::atomic_write, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::BTreeSet, fs, path::PathBuf, sync::Mutex};

pub const WARNING: &str = "Snap Tap changes your key presses in software. Some games forbid this: Counter-Strike 2 may kick you from official matches, and tournaments or other platforms may go further. Turn it off in those games. It does nothing in games running as administrator, and it may not work in every game.";
pub const WARNING_SHORT: &str = "Some games forbid this. Turn it off in those games.";
pub const SCOPE: &str = "applies to every keyboard";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub version: u8,
    pub enabled: bool,
    pub acknowledged: bool,
    pub pairs: Vec<[String; 2]>,
    pub deny: Vec<String>,
    pub allow: Vec<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: false,
            acknowledged: false,
            pairs: vec![["KEY_A".into(), "KEY_D".into()]],
            deny: vec!["cs2.exe".into()],
            allow: vec![],
        }
    }
}
impl Settings {
    pub fn key_ids(&self) -> Vec<[KeyId; 2]> {
        self.pairs
            .iter()
            .map(|pair| {
                pair.each_ref()
                    .map(|name| key(name).expect("validated key").1)
            })
            .collect()
    }
}

fn key(name: &str) -> Result<(String, KeyId)> {
    let name = name.trim().to_ascii_uppercase();
    let name = name.strip_prefix("KEY_").unwrap_or(&name);
    let Output::Physical(key) = codes::key_named(name)? else {
        return Err(Error::Invalid(
            "Snap Tap needs a physical key with a scan code".into(),
        ));
    };
    if BYPASS_VKS.contains(&u32::from(key.vk)) || key.vk == 0x13 || key.scan == 0 {
        return Err(Error::Invalid(format!(
            "{name} cannot be in a Snap Tap pair"
        )));
    }
    let canonical = codes::KEYS
        .iter()
        .find(|(_, value)| *value == key)
        .expect("physical key is in table")
        .0;
    Ok((
        canonical.into(),
        KeyId {
            scan: u32::from(key.scan),
            extended: key.extended,
        },
    ))
}

pub fn offered_keys() -> Vec<String> {
    let mut keys: Vec<_> = codes::KEYS
        .iter()
        .filter(|(name, _)| key(name).is_ok())
        .collect();
    keys.sort_by_key(|(name, key)| (!(0x41..=0x5a).contains(&key.vk), *name));
    keys.into_iter().map(|(name, _)| (*name).into()).collect()
}

pub fn clean_pairs(value: &Value) -> Result<Vec<[String; 2]>> {
    let pairs = value
        .as_array()
        .ok_or_else(|| Error::Invalid("pairs must be a list of two-key lists".into()))?;
    let mut used = BTreeSet::new();
    pairs
        .iter()
        .map(|pair| {
            let pair = pair
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or_else(|| Error::Invalid("a pair needs exactly two keys".into()))?;
            let mut names = [String::new(), String::new()];
            for (index, value) in pair.iter().enumerate() {
                let name = value
                    .as_str()
                    .ok_or_else(|| Error::Invalid("a key name must be a string".into()))?;
                let (name, id) = key(name)?;
                if !used.insert(id) {
                    return Err(Error::Invalid(format!("{name} is already in a pair")));
                }
                names[index] = name;
            }
            Ok(names)
        })
        .collect()
}

pub fn clean_programs(value: &Value) -> Result<Vec<String>> {
    let names = value
        .as_array()
        .ok_or_else(|| Error::Invalid("programs must be a list of executable names".into()))?;
    let mut result = Vec::new();
    for name in names {
        let name = name
            .as_str()
            .ok_or_else(|| Error::Invalid("an executable name must be a string".into()))?
            .trim()
            .to_lowercase();
        if name.len() <= 4
            || !name.ends_with(".exe")
            || name
                .chars()
                .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
        {
            return Err(Error::Invalid(format!(
                "{name:?} is not an executable name such as game.exe"
            )));
        }
        if !result.contains(&name) {
            result.push(name);
        }
    }
    Ok(result)
}

pub fn pause_reason(exe: Option<&str>, settings: &Settings) -> Option<&'static str> {
    let Some(exe) = exe.filter(|name| !name.is_empty()) else {
        return Some("unknown program");
    };
    if settings.deny.iter().any(|name| name == exe) {
        return Some("denied");
    }
    if !settings.allow.is_empty() && !settings.allow.iter().any(|name| name == exe) {
        return Some("not allowed");
    }
    None
}

pub struct SettingsStore {
    path: Option<PathBuf>,
    data: Mutex<Settings>,
}
impl SettingsStore {
    pub fn memory() -> Self {
        Self {
            path: None,
            data: Mutex::new(Settings::default()),
        }
    }

    pub fn load(path: PathBuf) -> Self {
        let value: Value = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(Value::Null);
        let mut data = Settings::default();
        if value["version"] == 1 {
            if let Some(value) = value.get("pairs") {
                if let Ok(pairs) = clean_pairs(value) {
                    data.pairs = pairs;
                }
            }
            if let Some(value) = value.get("deny") {
                if let Ok(deny) = clean_programs(value) {
                    data.deny = deny;
                }
            }
            if let Some(value) = value.get("allow") {
                if let Ok(allow) = clean_programs(value) {
                    data.allow = allow;
                }
            }
            data.acknowledged = value["acknowledged"] == true;
            data.enabled = data.acknowledged && value["enabled"] == true;
        }
        Self {
            path: Some(path),
            data: Mutex::new(data),
        }
    }

    pub fn snapshot(&self) -> Settings {
        self.data.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Hold the lock through atomic replacement so concurrent RPC writes cannot
    /// persist an older snapshot after a newer one. Failures change nothing.
    pub fn update(&self, changes: &Map<String, Value>) -> Result<Settings> {
        if let Some(name) = changes.keys().find(|name| {
            !["enabled", "acknowledged", "pairs", "deny", "allow"].contains(&name.as_str())
        }) {
            return Err(Error::Invalid(format!("unknown Snap Tap setting {name}")));
        }
        let mut current = self.data.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = current.clone();
        if let Some(value) = changes.get("pairs") {
            next.pairs = clean_pairs(value)?;
        }
        if let Some(value) = changes.get("deny") {
            next.deny = clean_programs(value)?;
        }
        if let Some(value) = changes.get("allow") {
            next.allow = clean_programs(value)?;
        }
        if let Some(value) = changes.get("acknowledged") {
            next.acknowledged |= value
                .as_bool()
                .ok_or_else(|| Error::Invalid("acknowledged must be boolean".into()))?;
        }
        if let Some(value) = changes.get("enabled") {
            next.enabled = value
                .as_bool()
                .ok_or_else(|| Error::Invalid("enabled must be boolean".into()))?;
            if next.enabled && !next.acknowledged {
                return Err(Error::Invalid(format!(
                    "the warning has to be acknowledged first: {WARNING}"
                )));
            }
        }
        if let Some(path) = &self.path {
            atomic_write(path, &serde_json::to_vec_pretty(&next)?)?;
        }
        *current = next.clone();
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "mirage-hub-snaptap-test-{}",
                crate::rpc::new_token().unwrap()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self) -> PathBuf {
            self.0.join("snaptap.json")
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.file());
            let _ = fs::remove_file(self.0.join("blocked"));
            let _ = fs::remove_dir(&self.0);
        }
    }

    #[test]
    fn loading_respects_empty_lists_and_never_enables_without_consent() {
        let directory = Directory::new();
        let value = json!({"version":1,"enabled":true,"acknowledged":false,"pairs":[],"deny":[],"allow":["Game.EXE"]});
        let bytes = serde_json::to_vec(&value).unwrap();
        fs::write(directory.file(), &bytes).unwrap();
        let store = SettingsStore::load(directory.file());
        let data = store.snapshot();
        assert!(!data.enabled && !data.acknowledged);
        assert!(data.pairs.is_empty() && data.deny.is_empty());
        assert_eq!(data.allow, ["game.exe"]);
        assert_eq!(fs::read(directory.file()).unwrap(), bytes);
    }

    #[test]
    fn failed_file_replacement_does_not_change_live_settings() {
        let directory = Directory::new();
        let blocker = directory.0.join("blocked");
        fs::write(&blocker, b"not a directory").unwrap();
        let store = SettingsStore::load(blocker.join("snaptap.json"));
        let before = store.snapshot();
        assert!(store
            .update(
                json!({"enabled":true,"acknowledged":true})
                    .as_object()
                    .unwrap()
            )
            .is_err());
        assert_eq!(store.snapshot(), before);
    }

    #[test]
    fn concurrent_updates_preserve_other_fields_and_the_last_disk_snapshot() {
        use std::sync::{Arc, Barrier};
        let directory = Directory::new();
        let store = Arc::new(SettingsStore::load(directory.file()));
        let barrier = Arc::new(Barrier::new(4));
        let changes = [
            json!({"acknowledged":true}),
            json!({"pairs":[["w","s"]]}),
            json!({"deny":[]}),
            json!({"allow":["game.exe"]}),
        ];
        let workers: Vec<_> = changes
            .into_iter()
            .map(|changes| {
                let (store, barrier) = (store.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    store.update(changes.as_object().unwrap()).unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let expected = Settings {
            acknowledged: true,
            pairs: vec![["KEY_W".into(), "KEY_S".into()]],
            deny: vec![],
            allow: vec!["game.exe".into()],
            ..Settings::default()
        };
        assert_eq!(store.snapshot(), expected);
        assert_eq!(SettingsStore::load(directory.file()).snapshot(), expected);
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn consent_is_required_and_an_invalid_update_is_atomic() {
        let store = SettingsStore::memory();
        let before = store.snapshot();
        assert!(!before.enabled && !before.acknowledged);
        assert!(store
            .update(json!({"enabled":true,"deny":[]}).as_object().unwrap())
            .is_err());
        assert_eq!(store.snapshot(), before);
        let enabled = store
            .update(
                json!({"enabled":true,"acknowledged":true,"deny":[]})
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert!(enabled.enabled && enabled.acknowledged);
        assert!(enabled.deny.is_empty());
        assert!(store
            .update(
                json!({"enabled":false,"pairs":[["a","a"]]})
                    .as_object()
                    .unwrap()
            )
            .is_err());
        assert_eq!(store.snapshot(), enabled);
        assert!(
            store
                .update(json!({"acknowledged":false}).as_object().unwrap())
                .unwrap()
                .acknowledged
        );
    }

    #[test]
    fn key_names_are_canonical_and_pairs_cannot_overlap_or_use_shortcuts() {
        assert_eq!(
            clean_pairs(&json!([[" a ", "key_d"]])).unwrap(),
            [["KEY_A", "KEY_D"]]
        );
        for invalid in [
            json!([["a", "d"], ["A", "s"]]),
            json!([["alt", "d"]]),
            json!([["pause", "a"]]),
            json!([["no-such-key", "d"]]),
            json!([["a"]]),
            json!([1]),
            json!(false),
        ] {
            assert!(clean_pairs(&invalid).is_err(), "accepted {invalid}");
        }
        assert!(clean_pairs(&json!([])).unwrap().is_empty());
        assert!(!offered_keys().contains(&"KEY_PAUSE".into()));
    }

    #[test]
    fn program_lists_are_explicit_and_unknown_foreground_is_paused() {
        assert_eq!(
            clean_programs(&json!([" CS2.EXE ", "cs2.exe"])).unwrap(),
            ["cs2.exe"]
        );
        for invalid in [
            json!(["C:\\game.exe"]),
            json!(["game"]),
            json!([".exe"]),
            json!(["*.exe"]),
            json!([false]),
        ] {
            assert!(clean_programs(&invalid).is_err());
        }
        let mut settings = Settings::default();
        assert_eq!(pause_reason(None, &settings), Some("unknown program"));
        assert_eq!(pause_reason(Some("cs2.exe"), &settings), Some("denied"));
        assert_eq!(pause_reason(Some("game.exe"), &settings), None);
        settings.allow = vec!["another.exe".into()];
        assert_eq!(
            pause_reason(Some("game.exe"), &settings),
            Some("not allowed")
        );
    }
}
