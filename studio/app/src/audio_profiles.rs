//! Named Game-output effects, never Sony APO documents or resource paths.
use crate::settings::AppRule;
pub use mirage_hub_core::audio_effects::AudioEffects;
use mirage_hub_core::t;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub effects: AudioEffects,
}

impl Profile {
    pub fn new(name: &str, effects: AudioEffects) -> Result<Self, String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
            return Err(t("Use a name of 1–64 characters without control characters.").into());
        }
        effects.validate().map_err(|_| {
            t("An equalizer curve needs ten gains between -12 and +12 dB.").to_string()
        })?;
        Ok(Self {
            name: name.into(),
            effects,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub profiles: BTreeMap<String, Vec<Profile>>,
    pub rules: Vec<AppRule>,
    pub defaults: BTreeMap<String, String>,
    pub auto_switch: bool,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            profiles: BTreeMap::new(),
            rules: Vec::new(),
            defaults: BTreeMap::new(),
            auto_switch: true,
        }
    }
}

impl Library {
    pub fn presets(&self, serial: &str) -> &[Profile] {
        self.profiles
            .get(serial)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn get(&self, serial: &str, name: &str) -> Option<&Profile> {
        self.presets(serial).iter().find(|c| c.name == name)
    }

    pub fn add(&mut self, serial: &str, curve: Profile) -> Result<(), String> {
        let curve = Profile::new(&curve.name, curve.effects)?;
        if self
            .presets(serial)
            .iter()
            .any(|c| c.name.to_lowercase() == curve.name.to_lowercase())
        {
            return Err(t("An audio effects profile with this name already exists.").into());
        }
        if self.presets(serial).len() >= 64 {
            return Err(t("You can save up to 64 audio effects profiles per device.").into());
        }
        self.profiles.entry(serial.into()).or_default().push(curve);
        Ok(())
    }

    pub fn update(&mut self, serial: &str, old: &str, curve: Profile) -> Result<(), String> {
        let curve = Profile::new(&curve.name, curve.effects)?;
        if self
            .presets(serial)
            .iter()
            .any(|c| c.name != old && c.name.to_lowercase() == curve.name.to_lowercase())
        {
            return Err(t("An audio effects profile with this name already exists.").into());
        }
        let item = self
            .profiles
            .get_mut(serial)
            .and_then(|p| p.iter_mut().find(|c| c.name == old))
            .ok_or_else(|| t("Audio effects profile not found.").to_string())?;
        *item = curve.clone();
        for rule in &mut self.rules {
            if rule.serial == serial && rule.profile == old {
                rule.profile.clone_from(&curve.name);
            }
        }
        if self.defaults.get(serial).is_some_and(|n| n == old) {
            self.defaults.insert(serial.into(), curve.name);
        }
        Ok(())
    }

    pub fn remove(&mut self, serial: &str, name: &str) {
        if let Some(profiles) = self.profiles.get_mut(serial) {
            profiles.retain(|c| c.name != name);
        }
        self.rules
            .retain(|r| r.serial != serial || r.profile != name);
        if self.defaults.get(serial).is_some_and(|n| n == name) {
            self.defaults.remove(serial);
        }
    }

    pub fn link(&mut self, serial: &str, profile: &str, exe: &str) -> Result<(), String> {
        if self.get(serial, profile).is_none() {
            return Err(t("Audio effects profile not found.").into());
        }
        let exe = exe.trim().to_lowercase();
        if !exe.ends_with(".exe")
            || exe.len() <= 4
            || exe.contains(['/', '\\', ':'])
            || exe.chars().any(char::is_control)
        {
            return Err(t("Enter a program file name such as game.exe.").into());
        }
        self.rules
            .retain(|r| r.serial != serial || !r.exe.eq_ignore_ascii_case(&exe));
        self.rules.push(AppRule {
            serial: serial.into(),
            profile: profile.into(),
            exe,
        });
        Ok(())
    }

    pub fn desired(&self, serial: &str, exe: &str, leaving_rule: bool) -> Option<(Profile, bool)> {
        let rule = self
            .rules
            .iter()
            .find(|r| r.serial == serial && r.exe.eq_ignore_ascii_case(exe));
        let (name, by_rule) = match rule {
            Some(rule) => (&rule.profile, true),
            None if leaving_rule => (self.defaults.get(serial)?, false),
            None => return None,
        };
        let curve = self.get(serial, name)?;
        Some((
            Profile::new(&curve.name, curve.effects.clone()).ok()?,
            by_rule,
        ))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    format: String,
    version: u32,
    name: String,
    effects: AudioEffects,
}

pub fn decode(bytes: &[u8]) -> Result<Profile, String> {
    if bytes.len() > 16 * 1024 {
        return Err(t("Audio effects profile files must be smaller than 16 KiB.").into());
    }
    let file: File = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if file.format != "mirage-hub-audio-effects" || file.version != 1 {
        return Err(t("Unsupported audio effects profile format or version.").into());
    }
    Profile::new(&file.name, file.effects)
}

pub fn import(path: &Path) -> Result<Profile, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(16 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    decode(&bytes)
}

pub fn export(path: &Path, curve: &Profile) -> Result<(), String> {
    let curve = Profile::new(&curve.name, curve.effects.clone())?;
    let bytes = serde_json::to_vec_pretty(&File {
        format: "mirage-hub-audio-effects".into(),
        version: 1,
        name: curve.name,
        effects: curve.effects,
    })
    .map_err(|e| e.to_string())?;
    // A user-selected export directory may contain unrelated .partial files.
    // Use an exclusively created sibling and replace only the chosen destination.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temporary = path.with_extension(format!("{}.{}.tmp", std::process::id(), stamp));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    let result = file.write_all(&bytes).and_then(|()| file.sync_all());
    drop(file);
    let result = result.and_then(|()| std::fs::rename(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) fn test_effects(gain: f64) -> AudioEffects {
    use mirage_hub_core::audio_effects::{DynamicRange, Spatial};
    AudioEffects {
        equalizer: true,
        bands: [gain; 10],
        drc: DynamicRange::Off,
        spatial: Spatial::Off,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exporting_replaces_only_the_selected_file_and_imports_it() {
        let dir = std::env::temp_dir().join(format!(
            "mirage-eq-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let target = dir.join("curve.json");
        let unrelated = dir.join("curve.partial");
        std::fs::write(&unrelated, b"keep").unwrap();
        let mut curve = Profile::new("Game", test_effects(0.)).unwrap();
        export(&target, &curve).unwrap();
        curve.effects.bands[0] = 1.5;
        export(&target, &curve).unwrap();
        assert_eq!(import(&target).unwrap(), curve);
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");
        curve.effects.bands[0] = f64::INFINITY;
        assert!(export(&target, &curve).is_err());
        assert_eq!(import(&target).unwrap().effects.bands[0], 1.5);
        std::fs::remove_file(target).unwrap();
        std::fs::remove_file(unrelated).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn files_round_trip_and_reject_wrong_layout_or_values() {
        let file = File {
            format: "mirage-hub-audio-effects".into(),
            version: 1,
            name: "游戏".into(),
            effects: test_effects(1.5),
        };
        let bytes = serde_json::to_vec(&file).unwrap();
        assert_eq!(
            decode(&bytes).unwrap(),
            Profile::new("游戏", test_effects(1.5)).unwrap()
        );
        let valid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for (key, value) in [
            ("version", serde_json::json!(2)),
            ("format", serde_json::json!("other")),
            ("bands_db", serde_json::json!([0, 1])),
            ("bands_db", serde_json::json!(vec![13; 10])),
            ("path", serde_json::json!("anything")),
        ] {
            let mut bad = valid.clone();
            bad[key] = value;
            assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
        assert!(decode(&vec![b' '; 16 * 1024 + 1]).is_err());
        for (key, value) in [
            ("bands", serde_json::json!(vec![13; 10])),
            ("drc", serde_json::json!("custom")),
            ("spatial", serde_json::json!("custom")),
            ("equalizer", serde_json::json!("yes")),
            ("path", serde_json::json!("C:/Sony")),
        ] {
            let mut bad = valid.clone();
            bad["effects"][key] = value;
            assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
        let legacy = serde_json::json!({"format":"mirage-hub-equalizer","version":1,"name":"old","bands_db":vec![0.;10]});
        assert!(decode(&serde_json::to_vec(&legacy).unwrap()).is_err());
        assert!(Profile::new("test", test_effects(f64::NAN)).is_err());
    }

    #[test]
    fn rename_delete_and_duplicate_names_preserve_rule_integrity() {
        let mut library = Library::default();
        library
            .add("H5", Profile::new("Game", test_effects(2.)).unwrap())
            .unwrap();
        assert!(library
            .add("H5", Profile::new("game", test_effects(0.)).unwrap())
            .is_err());
        library.link("H5", "Game", "GAME.EXE").unwrap();
        library.defaults.insert("H5".into(), "Game".into());
        library
            .update("H5", "Game", Profile::new("FPS", test_effects(3.)).unwrap())
            .unwrap();
        assert_eq!(
            library.desired("H5", "Game.exe", false).unwrap().0.name,
            "FPS"
        );
        assert!(!library.desired("H5", "desktop.exe", true).unwrap().1);
        assert!(library.desired("H5", "desktop.exe", false).is_none());
        assert!(library.desired("OTHER", "game.exe", false).is_none());
        assert!(library.link("H5", "FPS", "C:\\game.exe").is_err());
        library.remove("H5", "FPS");
        assert!(library.rules.is_empty() && library.defaults.is_empty());
    }
}
