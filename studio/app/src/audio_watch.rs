//! Foreground-app audio effects switching with readback and bounded retries.
use crate::{
    audio_profiles::{Library, Profile},
    worker::{AudioChange, Command, Snapshot},
};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

struct State {
    curve: Profile,
    by_rule: bool,
    completed: bool,
    pending: String,
    attempted: Instant,
}

#[derive(Default)]
pub struct Watch {
    states: HashMap<String, State>,
    reads: HashMap<String, Instant>,
    paused: bool,
}

impl Watch {
    pub fn tick(
        &mut self,
        library: &Library,
        exe: &str,
        connected: &[String],
        snap: &Snapshot,
        now: Instant,
    ) -> Vec<Command> {
        if !library.auto_switch {
            self.states.clear();
            self.reads.clear();
            return Vec::new();
        }
        self.states.retain(|serial, _| connected.contains(serial));
        self.reads.retain(|serial, _| connected.contains(serial));
        if exe.eq_ignore_ascii_case(mirage_hub_core::install::APP_EXE) {
            self.paused = true;
            return Vec::new();
        }
        let resumed = std::mem::take(&mut self.paused);
        let mut commands = Vec::new();
        for serial in connected {
            if !snap
                .devices
                .iter()
                .any(|d| d.serial == *serial && d.vid == 0x054c && d.pid == 0x0ebf)
            {
                continue;
            }
            // Only return to a default after a rule changed this device. Keep
            // retrying that return until acknowledged, without undoing later
            // manual selections whenever an unrelated window gains focus.
            let returning = self
                .states
                .get(serial)
                .is_some_and(|s| s.by_rule || !s.completed);
            let Some((curve, by_rule)) = library.desired(serial, exe, returning) else {
                continue;
            };
            let detail = snap.details.get(serial);
            let audio = detail.and_then(|d| d.audio.as_ref());
            // Load device state while the window is closed, and refresh unavailable APOs.
            if audio.is_none_or(|a| {
                a["sony_apo"]["available"] != true
                    || a["sony_apo"]["hub_running"] == true
                    || a["sony_apo"]["effects_disabled"] == true
            }) {
                if let Some(state) = self.states.get_mut(serial) {
                    state.completed = false;
                }
                if self
                    .reads
                    .get(serial)
                    .is_none_or(|at| now.duration_since(*at) >= Duration::from_secs(10))
                {
                    commands.push(Command::ReadAudio(serial.clone()));
                    self.reads.insert(serial.clone(), now);
                }
                continue;
            }
            if let Some(state) = self.states.get_mut(serial) {
                if state.curve == curve {
                    state.by_rule = by_rule;
                    if resumed {
                        state.completed = false;
                    }
                    let acknowledged = detail.is_some_and(|d| {
                        d.audio_completed.get("set_sony_effects") == Some(&state.pending)
                    });
                    let matches = audio.and_then(|a| {
                        crate::audio_profiles::AudioEffects::from_state(&a["sony_apo"])
                    }) == Some(curve.effects.clone());
                    if acknowledged && matches && !resumed {
                        state.completed = true;
                    }
                    if state.completed
                        || (!resumed
                            && now.duration_since(state.attempted) < Duration::from_secs(10))
                    {
                        continue;
                    }
                }
            }
            let change = AudioChange::new("set_sony_effects", serde_json::json!(curve.effects));
            self.states.insert(
                serial.clone(),
                State {
                    curve,
                    by_rule,
                    completed: false,
                    pending: change.id.clone(),
                    attempted: now,
                },
            );
            commands.push(Command::Audio {
                serial: serial.clone(),
                change,
            });
        }
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_profiles::test_effects;
    use crate::worker::{Detail, Device};
    use serde_json::json;

    #[test]
    fn matching_bands_alone_do_not_acknowledge_an_effect_profile() {
        use mirage_hub_core::audio_effects::{DynamicRange, Spatial};
        let mut library = Library::default();
        let mut effects = test_effects(2.);
        effects.equalizer = false;
        effects.drc = DynamicRange::High;
        effects.spatial = Spatial::Standard;
        library
            .add("H5", Profile::new("game", effects.clone()).unwrap())
            .unwrap();
        library.link("H5", "game", "game.exe").unwrap();
        let mut snap = Snapshot::default();
        snap.devices.push(Device {
            serial: "H5".into(),
            vid: 0x054c,
            pid: 0x0ebf,
            ..Default::default()
        });
        let mut audio = json!({"sony_apo":test_effects(2.)});
        audio["sony_apo"]["available"] = json!(true);
        snap.details.insert(
            "H5".into(),
            Detail {
                audio: Some(audio),
                ..Default::default()
            },
        );
        let mut watch = Watch::default();
        let now = Instant::now();
        let connected = vec!["H5".into()];
        let sent = watch.tick(&library, "game.exe", &connected, &snap, now);
        let Command::Audio { change, .. } = &sent[0] else {
            panic!()
        };
        assert_eq!(change.value, json!(effects));
        snap.details
            .get_mut("H5")
            .unwrap()
            .audio_completed
            .insert(change.method.clone(), change.id.clone());
        let retried = watch.tick(
            &library,
            "game.exe",
            &connected,
            &snap,
            now + Duration::from_secs(10),
        );
        assert_eq!(retried.len(), 1);
        let Command::Audio { change, .. } = &retried[0] else {
            panic!()
        };
        let detail = snap.details.get_mut("H5").unwrap();
        detail
            .audio_completed
            .insert(change.method.clone(), change.id.clone());
        detail.audio.as_mut().unwrap()["sony_apo"] = json!(effects);
        detail.audio.as_mut().unwrap()["sony_apo"]["available"] = json!(true);
        assert!(watch
            .tick(
                &library,
                "game.exe",
                &connected,
                &snap,
                now + Duration::from_secs(11)
            )
            .is_empty());
    }

    #[test]
    fn rules_edits_readback_failure_default_and_reconnect() {
        let mut library = Library::default();
        for (name, gain) in [("game", 2.), ("desktop", 0.)] {
            library
                .add("H5", Profile::new(name, test_effects(gain)).unwrap())
                .unwrap();
        }
        library.link("H5", "game", "game.exe").unwrap();
        library.defaults.insert("H5".into(), "desktop".into());
        let mut snap = Snapshot::default();
        snap.devices.push(Device {
            serial: "H5".into(),
            vid: 0x054c,
            pid: 0x0ebf,
            ..Default::default()
        });
        snap.details.insert(
            "H5".into(),
            Detail {
                audio: Some(json!({"sony_apo":{"available":true,"equalizer":true,"drc":"off","spatial":"off","bands":vec![0.;10]}})),
                ..Default::default()
            },
        );
        let devices = vec!["H5".to_string()];
        let now = Instant::now();
        let mut watch = Watch::default();
        assert!(watch
            .tick(&library, "desktop.exe", &devices, &snap, now)
            .is_empty());
        let sent = watch.tick(&library, "GAME.EXE", &devices, &snap, now);
        let Command::Audio { change, .. } = &sent[0] else {
            panic!()
        };
        // A failed write is acknowledged with the old curve; retry after backoff.
        snap.details
            .get_mut("H5")
            .unwrap()
            .audio_completed
            .insert("set_sony_effects".into(), change.id.clone());
        assert!(watch
            .tick(
                &library,
                "game.exe",
                &devices,
                &snap,
                now + Duration::from_secs(1)
            )
            .is_empty());
        let retried = watch.tick(
            &library,
            "game.exe",
            &devices,
            &snap,
            now + Duration::from_secs(10),
        );
        let Command::Audio { change, .. } = &retried[0] else {
            panic!()
        };
        let detail = snap.details.get_mut("H5").unwrap();
        detail
            .audio_completed
            .insert("set_sony_effects".into(), change.id.clone());
        detail.audio.as_mut().unwrap()["sony_apo"]["bands"] = json!(vec![2.; 10]);
        assert!(watch
            .tick(
                &library,
                "game.exe",
                &devices,
                &snap,
                now + Duration::from_secs(11)
            )
            .is_empty());
        // Editing a saved curve takes effect without needing another foreground transition.
        library
            .update(
                "H5",
                "game",
                Profile::new("game", test_effects(3.)).unwrap(),
            )
            .unwrap();
        assert_eq!(
            watch
                .tick(
                    &library,
                    "game.exe",
                    &devices,
                    &snap,
                    now + Duration::from_secs(12)
                )
                .len(),
            1
        );
        let fallback = watch.tick(
            &library,
            "desktop.exe",
            &devices,
            &snap,
            now + Duration::from_secs(13),
        );
        assert!(
            matches!(&fallback[0], Command::Audio {change, ..} if change.value["bands"] == json!(vec![0.;10]))
        );
        // Defaults also keep retrying until their own acknowledgement arrives.
        let retry = watch.tick(
            &library,
            "desktop.exe",
            &devices,
            &snap,
            now + Duration::from_secs(23),
        );
        let Command::Audio { change, .. } = &retry[0] else {
            panic!()
        };
        let detail = snap.details.get_mut("H5").unwrap();
        detail
            .audio_completed
            .insert("set_sony_effects".into(), change.id.clone());
        detail.audio.as_mut().unwrap()["sony_apo"]["bands"] = json!(vec![0.; 10]);
        assert!(watch
            .tick(
                &library,
                "desktop.exe",
                &devices,
                &snap,
                now + Duration::from_secs(23)
            )
            .is_empty());
        // Once the default was restored, later unrelated apps must not undo a manual edit.
        snap.details.get_mut("H5").unwrap().audio.as_mut().unwrap()["sony_apo"]["bands"] =
            json!(vec![4.; 10]);
        assert!(watch
            .tick(
                &library,
                "other.exe",
                &devices,
                &snap,
                now + Duration::from_secs(23)
            )
            .is_empty());
        watch.tick(
            &library,
            "desktop.exe",
            &[],
            &snap,
            now + Duration::from_secs(24),
        );
        assert_eq!(
            watch
                .tick(
                    &library,
                    "game.exe",
                    &devices,
                    &snap,
                    now + Duration::from_secs(25)
                )
                .len(),
            1
        );
        library.auto_switch = false;
        assert!(watch
            .tick(
                &library,
                "game.exe",
                &devices,
                &snap,
                now + Duration::from_secs(26)
            )
            .is_empty());
    }

    #[test]
    fn mirage_focus_pauses_writes_and_returning_reapplies_the_rule() {
        let mut library = Library::default();
        library
            .add("H5", Profile::new("game", test_effects(2.)).unwrap())
            .unwrap();
        library.link("H5", "game", "game.exe").unwrap();
        let mut snap = Snapshot::default();
        snap.devices.push(Device {
            serial: "H5".into(),
            vid: 0x054c,
            pid: 0x0ebf,
            ..Default::default()
        });
        snap.details.insert(
            "H5".into(),
            Detail {
                audio: Some(json!({"sony_apo":{"available":true}})),
                ..Default::default()
            },
        );
        let connected = vec!["H5".into()];
        let mut watch = Watch::default();
        let now = Instant::now();
        assert_eq!(
            watch
                .tick(&library, "game.exe", &connected, &snap, now)
                .len(),
            1
        );
        assert!(watch
            .tick(&library, "mirage-hub.exe", &connected, &snap, now)
            .is_empty());
        assert_eq!(
            watch
                .tick(&library, "game.exe", &connected, &snap, now)
                .len(),
            1
        );
        // Without a default, leaving the app keeps the curve as it is.
        assert!(watch
            .tick(&library, "desktop.exe", &connected, &snap, now)
            .is_empty());
        snap.details.get_mut("H5").unwrap().audio.as_mut().unwrap()["sony_apo"]["hub_running"] =
            json!(true);
        assert!(matches!(
            &watch.tick(&library, "game.exe", &connected, &snap, now)[0],
            Command::ReadAudio(_)
        ));
        assert!(watch
            .tick(
                &library,
                "game.exe",
                &connected,
                &snap,
                now + Duration::from_secs(1)
            )
            .is_empty());
    }
}
