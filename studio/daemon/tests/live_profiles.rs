//! Behavioral cases from openrazer-win#16 (287c2414), exercised through the
//! Rust daemon and HID emulator. No real input hooks or hardware are opened.
use rsrazer_daemon::{
    database::{self, DeviceInfo},
    demo::DemoBackend,
    device::{Device, StandardDevice},
    hid::HidBackend,
    manager::{lock_device, SharedDevice},
    persistence::{self, SharedStore},
    profiles::ProfileStore,
    rpc::{self, Service},
    service::{DaemonService, Options, Registry, Stores},
    transport::Transport,
    Error, Result,
};
use serde_json::{json, Map, Value};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Faults {
    ignore_poll: bool,
    reset_poll_on_restore: bool,
    fail_restore: bool,
    calls: Vec<String>,
}
struct Hardware {
    inner: StandardDevice,
    faults: Arc<Mutex<Faults>>,
}
impl Device for Hardware {
    fn info(&self) -> &DeviceInfo {
        self.inner.info()
    }
    fn persistence(&self) -> &SharedStore {
        self.inner.persistence()
    }
    fn serial(&mut self) -> String {
        self.inner.serial()
    }
    fn firmware(&mut self) -> Result<String> {
        self.inner.firmware()
    }
    fn capabilities(&self) -> Value {
        self.inner.capabilities()
    }
    fn call(&mut self, method: &str, args: &[Value], kwargs: &Map<String, Value>) -> Result<Value> {
        let mut faults = self.faults.lock().unwrap();
        faults.calls.push(method.into());
        if method == "set_poll_rate" && faults.ignore_poll {
            return Ok(Value::Null);
        }
        drop(faults);
        self.inner.call(method, args, kwargs)
    }
    fn enter_driver_mode(&mut self, handled: bool) -> Result<()> {
        self.inner.enter_driver_mode(handled)
    }
    fn leave_driver_mode(&mut self) -> Result<()> {
        self.inner.leave_driver_mode()
    }
    fn restore(&mut self) -> Result<()> {
        let faults = self.faults.lock().unwrap();
        if faults.fail_restore {
            return Err(Error::Device("unplugged halfway".into()));
        }
        self.inner.restore()?;
        if faults.reset_poll_on_restore {
            let serial = self.inner.serial();
            let expected = persistence::lock(self.inner.persistence()).get_device(
                &serial,
                "poll_rate",
                Value::Null,
            );
            self.inner
                .call("set_poll_rate", &[json!(8000)], &Map::new())?;
            persistence::lock(self.inner.persistence()).set_device(&serial, "poll_rate", expected);
        }
        Ok(())
    }
    fn close(&mut self) {
        self.inner.close();
    }
}
struct Devices(Vec<SharedDevice>);
impl Registry for Devices {
    fn devices(&self) -> Vec<SharedDevice> {
        self.0.clone()
    }
    fn scan(&self, _: bool) -> Result<Vec<SharedDevice>> {
        Ok(self.devices())
    }
    fn unsupported(&self) -> Vec<(u16, String)> {
        vec![]
    }
    fn close(&self) -> Result<()> {
        Ok(())
    }
}
struct Fixture {
    service: Arc<DaemonService>,
    path: PathBuf,
    serials: Vec<String>,
    devices: Vec<SharedDevice>,
    faults: Vec<Arc<Mutex<Faults>>>,
}
impl Fixture {
    fn new(pids: &[u16]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rsrazer-live-profiles-{}",
            rpc::new_token().unwrap()
        ));
        let stores = Stores::load(&path);
        let backend = Arc::new(DemoBackend::with_product_ids(pids).unwrap());
        let mut devices = Vec::new();
        let mut serials = Vec::new();
        let mut faults = Vec::new();
        for pid in pids {
            let info = backend
                .enumerate(Some(0x1532), Some(*pid))
                .unwrap()
                .remove(0);
            let mut inner = StandardDevice::new(
                database::lookup(0x1532, *pid).unwrap().clone(),
                Box::new(Transport::new(backend.clone(), info, 0, None, None)),
                stores.persistence.clone(),
            );
            serials.push(inner.serial());
            let fault = Arc::new(Mutex::new(Faults::default()));
            let device: SharedDevice = Arc::new(Mutex::new(Box::new(Hardware {
                inner,
                faults: fault.clone(),
            })));
            devices.push(device);
            faults.push(fault);
        }
        let service = DaemonService::new(
            Arc::new(Devices(devices.clone())),
            stores,
            None,
            None,
            Options {
                chroma_port: 0,
                ..Options::default()
            },
        );
        service.start();
        Self {
            service,
            path,
            serials,
            devices,
            faults,
        }
    }
    fn call(&self, method: &str, params: Value) -> Value {
        self.service
            .dispatch(method, params.as_object().unwrap())
            .unwrap()
    }
    fn dev(&self, index: usize, method: &str, args: Value) -> Value {
        self.call(
            "device.call",
            json!({"serial":self.serials[index],"method":method,"args":args}),
        )
    }
    fn profile(&self, action: &str, index: usize, name: &str) -> Value {
        self.call(
            &format!("profile.{action}"),
            json!({"serial":self.serials[index],"name":name}),
        )
    }
    fn store(&self) -> ProfileStore {
        ProfileStore::load(self.path.join("profiles.json"))
    }
    fn active(&self, index: usize) -> Option<String> {
        self.store().active(&self.serials[index]).map(str::to_owned)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.service.stop();
        // Only known files inside this test's unique temporary directory.
        for file in ["profiles.json", "persistence.json", "snaptap.json"] {
            let _ = fs::remove_file(self.path.join(file));
        }
        let _ = fs::remove_dir(&self.path);
    }
}

#[test]
fn saving_activates_per_device_and_only_mutations_update_the_active_profile() {
    let f = Fixture::new(&[0x026b, 0x0078]);
    f.profile("save", 0, "Desktop");
    f.profile("save", 0, "Gaming");
    assert_eq!(f.active(0).as_deref(), Some("Gaming"));
    assert_eq!(f.active(1), None);
    f.dev(0, "set_keyswitch_optimization", json!(["gaming"]));
    f.dev(0, "set_static", json!([0, 0, 255]));
    let gaming = f.store().get(&f.serials[0], "Gaming").unwrap();
    assert_eq!(gaming.state.device["keyswitch_optimization"], "gaming");
    assert_eq!(
        &gaming.state.zones["backlight"]["colors"]
            .as_array()
            .unwrap()[..3],
        &[json!(0), json!(0), json!(255)]
    );
    let path = f.path.join("profiles.json");
    let before = fs::read(&path).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    f.dev(0, "get_keyswitch_optimization", json!([]));
    f.dev(0, "get_poll_rate", json!([]));
    f.profile("load", 0, "gaming"); // Already active: reapply without touching the file.
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    f.profile("delete", 0, "Gaming");
    assert_eq!(f.active(0), None);
    let before = fs::read(&path).unwrap();
    f.dev(0, "set_keyswitch_optimization", json!(["typing"]));
    assert_eq!(fs::read(path).unwrap(), before);
    assert!(f
        .store()
        .get(&f.serials[0], "Desktop")
        .unwrap()
        .state
        .device
        .get("keyswitch_optimization")
        .is_none());
}

#[test]
fn both_huntsmans_capture_initial_polling_then_restore_modes_and_rates() {
    for pid in [0x026b, 0x026c] {
        for action in ["save", "create"] {
            let f = Fixture::new(&[pid]);
            let initial = f.dev(0, "get_poll_rate", json!([]));
            f.profile(action, 0, "Desktop");
            f.profile("create", 0, "Gaming");
            f.dev(0, "set_keyswitch_optimization", json!(["gaming"]));
            f.dev(0, "set_poll_rate", json!([8000]));
            for (name, rate, mode) in [
                ("Desktop", initial.clone(), "typing"),
                ("Gaming", json!(8000), "gaming"),
            ] {
                f.profile("load", 0, name);
                assert_eq!(f.dev(0, "get_poll_rate", json!([])), rate);
                // A saved legacy-style profile without an explicit mode leaves
                // it alone; a new default profile explicitly chooses typing.
                if name == "Gaming" || action == "create" {
                    assert_eq!(f.dev(0, "get_keyswitch_optimization", json!([])), mode);
                }
                assert_eq!(
                    f.store().get(&f.serials[0], name).unwrap().state.device["poll_rate"],
                    rate
                );
            }
            f.profile("load", 0, "Desktop");
            f.dev(0, "set_poll_rate", json!([500]));
            f.profile("load", 0, "Gaming");
            assert_eq!(f.dev(0, "get_poll_rate", json!([])), 8000);
            f.profile("load", 0, "Desktop");
            assert_eq!(f.dev(0, "get_poll_rate", json!([])), 500);
        }
    }
}

#[test]
fn stages_are_replayed_before_dpi_and_mouse_profiles_do_not_carry_snaptap() {
    let f = Fixture::new(&[0x0078]);
    f.dev(0, "set_dpi_stages", json!([2, [[400, 400], [800, 800]]]));
    f.dev(0, "set_dpi", json!([1200, 1300]));
    f.dev(0, "set_poll_rate", json!([500]));
    f.profile("save", 0, "Desktop");
    f.profile("save", 0, "Gaming");
    f.dev(0, "set_dpi_stages", json!([1, [[2000, 2000]]]));
    f.dev(0, "set_dpi", json!([1600, 1600]));
    f.dev(0, "set_poll_rate", json!([1000]));
    f.faults[0].lock().unwrap().calls.clear();
    f.profile("load", 0, "Desktop");
    let calls = f.faults[0].lock().unwrap().calls.clone();
    assert!(
        calls.iter().position(|s| s == "set_dpi_stages")
            < calls.iter().position(|s| s == "set_dpi")
    );
    assert_eq!(f.dev(0, "get_dpi", json!([])), json!([1200, 1300]));
    assert_eq!(f.dev(0, "get_poll_rate", json!([])), 500);
    assert_eq!(
        f.dev(0, "get_dpi_stages", json!([])),
        json!([2, [[400, 400], [800, 800]]])
    );
    assert!(f
        .store()
        .get(&f.serials[0], "Desktop")
        .unwrap()
        .state
        .snaptap
        .is_none());
}

#[test]
fn polling_readback_retries_after_lighting_and_failed_switch_preserves_the_previous_profile() {
    let f = Fixture::new(&[0x026b]);
    f.dev(0, "set_poll_rate", json!([1000]));
    f.profile("save", 0, "Desktop");
    f.profile("create", 0, "Gaming");
    f.dev(0, "set_poll_rate", json!([8000]));
    f.faults[0].lock().unwrap().reset_poll_on_restore = true;
    f.profile("load", 0, "Desktop");
    assert_eq!(f.dev(0, "get_poll_rate", json!([])), 1000);
    f.profile("load", 0, "Gaming");
    let gaming = f.store().get(&f.serials[0], "Gaming").unwrap().state;
    f.faults[0].lock().unwrap().ignore_poll = true;
    let error = f
        .service
        .dispatch(
            "profile.load",
            json!({"serial":f.serials[0],"name":"Desktop"})
                .as_object()
                .unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code, rpc::DEVICE_ERROR);
    assert!(error.message.contains("8000") && error.message.contains("1000"));
    assert_eq!(f.active(0).as_deref(), Some("Desktop"));
    assert_eq!(
        f.store()
            .get(&f.serials[0], "Desktop")
            .unwrap()
            .state
            .device["poll_rate"],
        1000
    );
    f.dev(0, "set_brightness", json!([22]));
    assert!(f.store().get(&f.serials[0], "Gaming").unwrap().state == gaming);
    f.faults[0].lock().unwrap().ignore_poll = false;
    f.faults[0].lock().unwrap().fail_restore = true;
    assert!(f
        .service
        .dispatch(
            "profile.load",
            json!({"serial":f.serials[0],"name":"Gaming"})
                .as_object()
                .unwrap()
        )
        .is_err());
    assert_eq!(f.active(0).as_deref(), Some("Gaming"));
    f.faults[0].lock().unwrap().fail_restore = false;
}

#[test]
fn snaptap_is_global_but_a_profile_cannot_grant_consent_or_replace_exclusions() {
    let f = Fixture::new(&[0x026b, 0x026c]);
    f.profile("create", 0, "Desktop");
    f.profile("create", 1, "Other keyboard");
    // Load an imported profile with enabled=true and a bogus embedded consent.
    let path = f.path.join("profiles.json");
    let mut imported = f.store().get(&f.serials[0], "Desktop").unwrap().state;
    imported.snaptap =
        Some(json!({"enabled":true,"acknowledged":true,"pairs":[["W","S"]],"deny":[]}));
    // A fresh daemon must read the imported file; replacing only the external
    // file while a daemon is running intentionally does not hot-reload it.
    let mut stores = Stores::load(&f.path);
    stores
        .profiles
        .put(&f.serials[0], "Imported", imported)
        .unwrap();
    let other = DaemonService::new(
        Arc::new(Devices(f.devices.clone())),
        stores,
        None,
        None,
        Options {
            chroma_port: 0,
            ..Options::default()
        },
    );
    other
        .dispatch(
            "profile.load",
            json!({"serial":f.serials[0],"name":"Imported"})
                .as_object()
                .unwrap(),
        )
        .unwrap();
    let status = other.dispatch("snaptap.status", &Map::new()).unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["acknowledged"], false);
    assert_eq!(status["deny"], json!(["cs2.exe"]));
    assert_eq!(status["pairs"], json!([["KEY_W", "KEY_S"]]));
    other.stop();
    // The original fixture's store remains independently serialized.
    f.call(
        "snaptap.set",
        json!({"enabled":true,"acknowledged":true,"pairs":[["W","S"]],"deny":["custom.exe"]}),
    );
    for (index, name) in [(0, "Desktop"), (1, "Other keyboard")] {
        let state = f.store().get(&f.serials[index], name).unwrap().state;
        assert_eq!(
            state.snaptap.unwrap(),
            json!({"enabled":true,"pairs":[["KEY_W","KEY_S"]]})
        );
    }
    f.profile("create", 0, "New");
    assert_eq!(f.call("snaptap.status", json!({}))["enabled"], false);
    assert_eq!(
        f.call("snaptap.status", json!({}))["deny"],
        json!(["custom.exe"])
    );
    f.profile("load", 0, "Desktop");
    assert_eq!(f.call("snaptap.status", json!({}))["enabled"], true);
    assert!(path.exists());
}

#[test]
fn creating_defaults_and_copying_preserve_sources_and_refuse_duplicate_names() {
    let f = Fixture::new(&[0x026b]);
    f.dev(0, "set_keyswitch_optimization", json!(["gaming"]));
    f.dev(0, "set_static", json!([255, 0, 0]));
    f.profile("save", 0, "Red");
    let original = f.store().get(&f.serials[0], "Red").unwrap().state;
    f.profile("create", 0, "Default");
    assert_eq!(f.dev(0, "get_keyswitch_optimization", json!([])), "typing");
    f.call(
        "profile.create",
        json!({"serial":f.serials[0],"name":"Copy","source":"Red"}),
    );
    assert_eq!(f.dev(0, "get_keyswitch_optimization", json!([])), "gaming");
    assert!(f.store().get(&f.serials[0], "Copy").unwrap().state == original);
    let before = fs::read(f.path.join("profiles.json")).unwrap();
    assert!(f
        .service
        .dispatch(
            "profile.create",
            json!({"serial":f.serials[0],"name":"copy"})
                .as_object()
                .unwrap()
        )
        .is_err());
    assert_eq!(fs::read(f.path.join("profiles.json")).unwrap(), before);
    assert_eq!(f.active(0).as_deref(), Some("Copy"));
}

#[test]
fn concurrent_mutations_are_kept_together_in_memory_and_on_disk() {
    let f = Fixture::new(&[0x026b]);
    f.profile("create", 0, "Active");
    std::thread::scope(|scope| {
        for (method, args) in [
            ("set_keyswitch_optimization", json!(["gaming"])),
            ("set_poll_rate", json!([500])),
            ("set_brightness", json!([24])),
        ] {
            let f = &f;
            scope.spawn(move || f.dev(0, method, args));
        }
    });
    let profile = f.store().get(&f.serials[0], "Active").unwrap();
    assert_eq!(profile.state.device["keyswitch_optimization"], "gaming");
    assert_eq!(profile.state.device["poll_rate"], 500);
    assert_eq!(profile.state.zones["backlight"]["brightness"].as_f64(), Some(24.0));
    assert_eq!(f.dev(0, "get_poll_rate", json!([])), 500);
    assert_eq!(lock_device(&f.devices[0]).serial(), f.serials[0]);
}
