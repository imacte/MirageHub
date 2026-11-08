use rsrazer_daemon::{
    database::{self, DeviceInfo},
    device::Device,
    effects::{
        engine::{EffectEngine, NativeEnvironment},
        keys::KeySource,
    },
    manager::{lock_device, SharedDevice},
    persistence::{self, SharedStore},
    rpc::{self, RpcServer, Service},
    service::{DaemonService, Options, Registry, Stores},
    Error, Result,
};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct State {
    attempts: usize,
    failures: usize,
    restored: usize,
    entered: usize,
    left: usize,
    calls: Vec<(String, Vec<Value>, Map<String, Value>)>,
    brightness: BTreeMap<String, Value>,
    colours: Vec<Value>,
}
struct Hardware {
    info: DeviceInfo,
    serial: String,
    store: SharedStore,
    state: Arc<Mutex<State>>,
    caps: Value,
}
impl Device for Hardware {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }
    fn persistence(&self) -> &SharedStore {
        &self.store
    }
    fn serial(&mut self) -> String {
        self.serial.clone()
    }
    fn firmware(&mut self) -> Result<String> {
        Ok("1.15".into())
    }
    fn capabilities(&self) -> Value {
        self.caps.clone()
    }
    fn call(&mut self, method: &str, args: &[Value], kwargs: &Map<String, Value>) -> Result<Value> {
        let mut state = self.state.lock().unwrap();
        state
            .calls
            .push((method.into(), args.into(), kwargs.clone()));
        let zone = kwargs
            .get("zone")
            .and_then(Value::as_str)
            .unwrap_or("backlight");
        match method {
            "get_brightness" => Ok(state.brightness.get(zone).cloned().unwrap_or(json!(75))),
            "set_brightness" => {
                let value = args
                    .first()
                    .ok_or_else(|| Error::Invalid("missing brightness".into()))?;
                state.brightness.insert(zone.into(), value.clone());
                persistence::lock(&self.store).set(&self.serial, zone, "brightness", value.clone());
                Ok(Value::Null)
            }
            "set_static" => {
                persistence::lock(&self.store).set(&self.serial, zone, "effect", json!("static"));
                persistence::lock(&self.store).set(&self.serial, zone, "colors", json!(args));
                Ok(Value::Null)
            }
            "set_spectrum" => {
                persistence::lock(&self.store).set(&self.serial, zone, "effect", json!("spectrum"));
                Ok(Value::Null)
            }
            "set_zone_colours" => {
                state.colours = args[0].as_array().unwrap().clone();
                persistence::lock(&self.store).set_device(
                    &self.serial,
                    "zone_colours",
                    args[0].clone(),
                );
                Ok(Value::Null)
            }
            "zone_colours_now" => Ok(json!(state.colours)),
            "set_wheel" => Err(Error::NotSupported("no wheel".into())),
            "get_audio" => Err(Error::Device("disconnected".into())),
            _ => Ok(Value::Null),
        }
    }
    fn enter_driver_mode(&mut self, _: bool) -> Result<()> {
        self.state.lock().unwrap().entered += 1;
        Ok(())
    }
    fn leave_driver_mode(&mut self) -> Result<()> {
        self.state.lock().unwrap().left += 1;
        Ok(())
    }
    fn restore(&mut self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.attempts += 1;
        if state.failures > 0 {
            state.failures -= 1;
            return Err(Error::Io("Bluetooth link not ready".into()));
        }
        state.restored += 1;
        Ok(())
    }
    fn close(&mut self) {}
}
fn hardware(store: SharedStore, serial: &str, ears: bool) -> (SharedDevice, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State {
        colours: vec![json!([255, 0, 0]), json!([0, 0, 255])],
        ..State::default()
    }));
    let mut info = database::lookup(0x1532, if ears { 0x562 } else { 0x203 })
        .unwrap()
        .clone();
    info.dedicated_macro_keys = !ears;
    let caps = if ears {
        json!({"custom_frame":true,"matrix_dimensions":[1,2],"zone_colours":true,"zone_order":["left","right"],
        "zones":{"left":{"effects":["none","static"],"brightness":true},"right":{"effects":["none","static"],"brightness":true}}})
    } else {
        json!({"custom_frame":true,"matrix_dimensions":[6,22],"zones":{"backlight":{"effects":["none","static","spectrum"],"brightness":true}}})
    };
    (
        Arc::new(Mutex::new(Box::new(Hardware {
            info,
            serial: serial.into(),
            store,
            state: state.clone(),
            caps,
        }))),
        state,
    )
}

#[derive(Default)]
struct Devices {
    current: Mutex<Vec<SharedDevice>>,
    next: Mutex<Option<Vec<SharedDevice>>>,
    force: AtomicBool,
    scans: AtomicUsize,
    gate: (Mutex<bool>, Condvar),
    fail_scan: AtomicBool,
}
impl Registry for Devices {
    fn devices(&self) -> Vec<SharedDevice> {
        self.current.lock().unwrap().clone()
    }
    fn scan(&self, force: bool) -> Result<Vec<SharedDevice>> {
        self.scans.fetch_add(1, Ordering::SeqCst);
        self.force.store(force, Ordering::SeqCst);
        let _guard = self
            .gate
            .1
            .wait_while(self.gate.0.lock().unwrap(), |blocked| *blocked)
            .unwrap();
        if self.fail_scan.swap(false, Ordering::SeqCst) {
            return Err(Error::Io("scan failed".into()));
        }
        if let Some(next) = self.next.lock().unwrap().take() {
            *self.current.lock().unwrap() = next;
        }
        Ok(self.devices())
    }
    fn unsupported(&self) -> Vec<(u16, String)> {
        vec![(0xffff, "unknown model".into())]
    }
    fn close(&self) -> Result<()> {
        self.current.lock().unwrap().clear();
        Ok(())
    }
}
struct Fixture {
    service: Arc<DaemonService>,
    registry: Arc<Devices>,
    store: SharedStore,
    device: SharedDevice,
    state: Arc<Mutex<State>>,
}
impl Fixture {
    fn new(effects: bool, ears: bool, failures: usize) -> Self {
        let stores = Stores::memory();
        let store = stores.persistence.clone();
        let (device, state) = hardware(store.clone(), "S", ears);
        state.lock().unwrap().failures = failures;
        let registry = Arc::new(Devices::default());
        *registry.next.lock().unwrap() = Some(vec![device.clone()]);
        let engine = effects.then(|| {
            EffectEngine::with_environment(
                store.clone(),
                KeySource::new(false),
                Arc::new(NativeEnvironment),
            )
        });
        let service = DaemonService::new(
            registry.clone(),
            stores,
            engine,
            None,
            Options {
                backend_name: "fake-rust".into(),
                chroma_port: 0,
                retry_delays: [Duration::ZERO; 4],
            },
        );
        Self {
            service,
            registry,
            store,
            device,
            state,
        }
    }
    fn call(&self, method: &str, params: Value) -> Value {
        self.service
            .dispatch(method, params.as_object().unwrap())
            .unwrap()
    }
    fn error(&self, method: &str, params: Value) -> i32 {
        self.service
            .dispatch(method, params.as_object().unwrap())
            .unwrap_err()
            .code
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.service.stop();
    }
}
fn eventually(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "condition timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn startup_is_observable_before_scan_finishes_and_retries_failed_devices() {
    let f = Fixture::new(false, false, 2);
    *f.registry.gate.0.lock().unwrap() = true;
    let service = f.service.clone();
    let starter = thread::spawn(move || service.start());
    eventually(|| f.registry.scans.load(Ordering::SeqCst) == 1);
    let server = RpcServer::start(f.service.clone(), 0).unwrap();
    let ep = serde_json::from_value(serde_json::to_value(server.endpoint()).unwrap()).unwrap();
    let mut client = rsrazer_core::rpc::Client::connect_to(ep).unwrap();
    let version = client.call("daemon.version", json!({})).unwrap();
    *f.registry.gate.0.lock().unwrap() = false;
    f.registry.gate.1.notify_all();
    starter.join().unwrap();
    assert_eq!(version["state"], "starting");
    assert_eq!(version["devices"], 0);
    assert_eq!(
        f.call("daemon.version", json!({}))["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.service.poll_hotplug(false).unwrap();
    f.service.poll_hotplug(false).unwrap();
    assert_eq!(f.state.lock().unwrap().restored, 1);
    let version = f.call("daemon.version", json!({}));
    assert_eq!(version["pending"], json!([]));
    assert_eq!(version["state"], "ready");
    assert_eq!(version["backend"], "fake-rust");
}

#[test]
fn runtime_starts_bluetooth_immediately_after_usb_and_publishes_pending_state() {
    use rsrazer_daemon::{rpc::SingleInstance, runtime::RunningDaemon};
    use std::sync::mpsc;

    struct StartupRegistry {
        entered: mpsc::SyncSender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        done: AtomicBool,
        usb: AtomicBool,
    }
    impl Registry for StartupRegistry {
        fn devices(&self) -> Vec<SharedDevice> {
            vec![]
        }
        fn scan_usb(&self) -> Result<Vec<SharedDevice>> {
            self.usb.store(true, Ordering::SeqCst);
            Ok(vec![])
        }
        fn scan(&self, _: bool) -> Result<Vec<SharedDevice>> {
            assert!(self.usb.load(Ordering::SeqCst));
            if !self.done.load(Ordering::SeqCst) {
                self.entered.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
                self.done.store(true, Ordering::SeqCst);
            }
            Ok(vec![])
        }
        fn first_sweep_pending(&self) -> bool {
            !self.done.load(Ordering::SeqCst)
        }
        fn unsupported(&self) -> Vec<(u16, String)> {
            vec![]
        }
        fn close(&self) -> Result<()> {
            Ok(())
        }
    }
    let (entered, receive) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::channel();
    let registry = Arc::new(StartupRegistry {
        entered,
        release: Mutex::new(wait),
        done: AtomicBool::new(false),
        usb: AtomicBool::new(false),
    });
    let service = DaemonService::new(
        registry,
        Stores::memory(),
        None,
        None,
        Options {
            chroma_port: 0,
            ..Options::default()
        },
    );
    let token = rpc::new_token().unwrap();
    let directory = std::env::temp_dir().join(format!("rsrazer-startup-test-{token}"));
    let instance = SingleInstance::acquire(&format!("Local\\rsrazer-startup-test-{token}"))
        .unwrap()
        .unwrap();
    let mut daemon = RunningDaemon::start(service.clone(), &directory, 0, instance).unwrap();
    // Less than HOTPLUG_INTERVAL: the first radio sweep must not wait for it.
    let immediate = receive.recv_timeout(Duration::from_secs(2));
    let pending = rpc::Client::connect_to(daemon.endpoint().clone())
        .and_then(|mut client| client.call("daemon.version", json!({})));
    release.send(()).unwrap();
    eventually(|| !service.first_sweep_pending());
    let ready = rpc::Client::connect_to(daemon.endpoint().clone())
        .unwrap()
        .call("daemon.version", json!({}))
        .unwrap();
    daemon.stop();
    std::fs::remove_dir(&directory).unwrap();
    immediate.unwrap();
    let pending = pending.unwrap();
    assert_eq!(pending["state"], "ready");
    assert_eq!(pending["first_sweep_pending"], true);
    assert_eq!(ready["first_sweep_pending"], false);
}

#[test]
fn scan_failures_do_not_end_startup_and_rescan_forces_bluetooth() {
    let f = Fixture::new(false, false, 0);
    f.registry.fail_scan.store(true, Ordering::SeqCst);
    f.service.start();
    assert_eq!(f.call("daemon.version", json!({}))["state"], "ready");
    assert_eq!(f.call("daemon.version", json!({}))["devices"], 0);
    assert_eq!(f.call("daemon.rescan", json!({}))[0]["serial"], "S");
    assert!(f.registry.force.load(Ordering::SeqCst));
    assert!(f
        .service
        .warnings()
        .iter()
        .any(|e| e.contains("scan failed")));
}

#[test]
fn rpc_lookup_allowlist_validation_and_error_codes_match_the_service_contract() {
    let f = Fixture::new(false, false, 0);
    f.service.start();
    let list = f.call("devices.list", json!({}));
    assert_eq!(list[0]["firmware"], "1.15");
    assert_eq!(list[0]["pid"], 0x203);
    for serial in ["S", "s", "0203", "BlackWidow"] {
        assert_eq!(
            f.call("device.capabilities", json!({"serial":serial}))["custom_frame"],
            true
        );
    }
    assert_eq!(
        f.error("device.call", json!({"serial":"S","method":"close"})),
        rpc::METHOD_NOT_FOUND
    );
    assert_eq!(f.error("_device", json!({})), rpc::METHOD_NOT_FOUND);
    assert_eq!(
        f.error("device.capabilities", json!({"serial":"missing"})),
        rpc::DEVICE_NOT_FOUND
    );
    assert_eq!(
        f.error("device.call", json!({"serial":"S","method":"set_wheel"})),
        rpc::NOT_SUPPORTED
    );
    assert_eq!(
        f.error("device.call", json!({"serial":"S","method":"get_audio"})),
        rpc::DEVICE_ERROR
    );
    for (method, params) in [
        ("daemon.version", json!({"extra":true})),
        ("device.capabilities", json!({})),
        (
            "device.call",
            json!({"serial":"S","method":"set_static","args":123}),
        ),
        (
            "device.call",
            json!({"serial":"S","method":"set_static","kwargs":[1]}),
        ),
        (
            "device.call",
            json!({"serial":"S","method":"set_brightness"}),
        ),
    ] {
        assert_eq!(f.error(method, params), rpc::INVALID_PARAMS);
    }
    assert_eq!(
        f.call("devices.unsupported", json!({})),
        json!([{"pid":65535,"product":"unknown model"}])
    );
}

#[test]
fn software_effects_are_forgotten_on_hardware_override_and_preserved_on_unplug() {
    let f = Fixture::new(true, false, 0);
    f.service.start();
    f.call("effect.set", json!({"serial":"S","effect":"wave_soft"}));
    eventually(|| !f.state.lock().unwrap().calls.is_empty());
    assert_eq!(
        f.call("effect.status", json!({}))["active"]["S"],
        "wave_soft"
    );
    f.call(
        "device.call",
        json!({"serial":"S","method":"set_static","args":[1,2,3]}),
    );
    assert_eq!(f.call("effect.status", json!({}))["active"], json!({}));
    assert!(persistence::lock(&f.store)
        .get_device("S", "software", Value::Null)
        .is_null());
    f.call("effect.set", json!({"serial":"S","effect":"wave_soft"}));
    *f.registry.next.lock().unwrap() = Some(vec![]);
    f.service.poll_hotplug(false).unwrap();
    assert_eq!(f.call("effect.status", json!({}))["active"], json!({}));
    assert_eq!(
        persistence::lock(&f.store).get_device("S", "software", Value::Null)["effect"],
        "wave_soft"
    );
    *f.registry.next.lock().unwrap() = Some(vec![f.device.clone()]);
    f.service.poll_hotplug(false).unwrap();
    assert_eq!(
        f.call("effect.status", json!({}))["active"]["S"],
        "wave_soft"
    );
}

#[test]
fn replacing_the_same_serial_moves_rendering_to_the_new_device() {
    let f = Fixture::new(true, false, 0);
    f.service.start();
    f.call("effect.set", json!({"serial":"S","effect":"wave_soft"}));
    let (replacement, state) = hardware(f.store.clone(), "S", false);
    *f.registry.next.lock().unwrap() = Some(vec![replacement]);
    f.service.poll_hotplug(false).unwrap();
    eventually(|| !state.lock().unwrap().calls.is_empty());
    assert_eq!(state.lock().unwrap().restored, 1);
    let count = f.state.lock().unwrap().calls.len();
    thread::sleep(Duration::from_millis(100));
    assert_eq!(f.state.lock().unwrap().calls.len(), count);
}

#[test]
fn individual_ear_changes_preserve_the_other_ears_live_effect() {
    let f = Fixture::new(true, true, 0);
    f.service.start();
    f.call(
        "effect.set",
        json!({"serial":"S","effect":"breath_soft","options":{"zone":"right","colour":[0,0,255]}}),
    );
    f.call(
        "device.call",
        json!({"serial":"S","method":"set_static","args":[255,0,0],"kwargs":{"zone":"left"}}),
    );
    let saved = persistence::lock(&f.store).get_device("S", "software", Value::Null);
    assert_eq!(saved["options"]["zones"]["left"]["effect"], "static");
    assert_eq!(saved["options"]["zones"]["right"]["effect"], "breath_soft");
    f.call(
        "device.call",
        json!({"serial":"S","method":"set_none","kwargs":{"zone":"right"}}),
    );
    assert!(persistence::lock(&f.store)
        .get_device("S", "software", Value::Null)
        .is_null());
    assert_eq!(
        f.state.lock().unwrap().colours,
        json!([[255, 0, 0], [0, 0, 0]]).as_array().unwrap().clone()
    );
}

#[test]
fn profiles_restore_brightness_and_switch_between_live_and_hardware_lighting() {
    let f = Fixture::new(true, false, 0);
    f.service.start();
    f.call(
        "device.call",
        json!({"serial":"S","method":"set_brightness","args":[42]}),
    );
    assert_eq!(
        f.call(
            "profile.save",
            json!({"serial":"S","name":"  中文   配置  "})
        )["name"],
        "中文 配置"
    );
    f.call("effect.set", json!({"serial":"S","effect":"wave_soft"}));
    f.call("profile.save", json!({"serial":"S","name":"Wave"}));
    f.call(
        "device.call",
        json!({"serial":"S","method":"set_brightness","args":[12]}),
    );
    f.call("profile.load", json!({"serial":"S","name":"中文 配置"}));
    assert_eq!(f.state.lock().unwrap().brightness["backlight"], 42);
    assert_eq!(f.call("effect.status", json!({}))["active"], json!({}));
    f.call("profile.load", json!({"serial":"S","name":"wave"}));
    assert_eq!(
        f.call("effect.status", json!({}))["active"]["S"],
        "wave_soft"
    );
    assert_eq!(
        f.call("profile.list", json!({"serial":"S"}))
            .as_array()
            .unwrap()
            .len(),
        2
    );
    f.call("profile.delete", json!({"serial":"S","name":"WAVE"}));
    assert_eq!(
        f.error("profile.load", json!({"serial":"S","name":"wave"})),
        rpc::INVALID_PARAMS
    );
}

#[test]
fn macro_bindings_are_editable_without_hooks_and_sequences_use_the_shared_key_table() {
    let f = Fixture::new(false, false, 0);
    f.service.start();
    assert_eq!(
        f.call("keys.status", json!({})),
        json!({"running":false,"devices":[],"recording":null})
    );
    let list = f.call("macro.list", json!({"serial":"S"}));
    assert_eq!(list["keys"], json!(["M1", "M2", "M3", "M4", "M5"]));
    assert_eq!(list["handled"], false);
    let action = f.call(
        "macro.set",
        json!({"serial":"S","key":"M1","action":{"type":"keys","sequence":"ctrl+c, ctrl+v"}}),
    );
    assert_eq!(action["events"].as_array().unwrap().len(), 8);
    assert_eq!(
        f.call("macro.list", json!({"serial":"S"}))["bindings"]["M1"],
        action
    );
    assert_eq!(
        f.error("macro.play", json!({"serial":"S","key":"M1"})),
        rpc::NOT_SUPPORTED
    );
    assert_eq!(
        f.error("macro.record_start", json!({"serial":"S","key":"M1"})),
        rpc::NOT_SUPPORTED
    );
    assert_eq!(f.call("macro.record_stop", json!({})), Value::Null);
    assert_eq!(
        f.error(
            "macro.set",
            json!({"serial":"S","key":"M1","action":{"type":"keys","sequence":"not-a-key"}})
        ),
        rpc::INVALID_PARAMS
    );
    assert_eq!(
        f.call("macro.delete", json!({"serial":"S","key":"M1"}))["deleted"],
        true
    );
    assert_eq!(
        f.call("macro.delete", json!({"serial":"S","key":"M1"}))["deleted"],
        false
    );
}

#[test]
fn chroma_enable_disable_restores_software_lighting_without_changing_saved_settings() {
    let f = Fixture::new(true, false, 0);
    f.service.start();
    f.call("effect.set", json!({"serial":"S","effect":"wave_soft"}));
    let saved = f.call("daemon.persistence", json!({}));
    let status = f.call("integration.set", json!({"name":"chroma","enabled":true}));
    assert_eq!(status["chroma"]["listening"], true);
    let port = status["chroma"]["port"].as_u64().unwrap();
    assert!(port > 0);
    let game = f.service.chroma().open(&json!({"title":"Test game"}));
    f.service
        .chroma()
        .show(
            game,
            "keyboard",
            &json!({"effect":"CHROMA_STATIC","param":{"color":255}}),
        )
        .unwrap();
    eventually(|| f.service.chroma().drawing("S"));
    assert_eq!(f.call("effect.status", json!({}))["active"], json!({}));
    assert_eq!(f.call("daemon.persistence", json!({})), saved);
    let status = f.call("integration.set", json!({"name":"chroma","enabled":false}));
    assert_eq!(status["chroma"]["listening"], false);
    assert!(!f.service.chroma().drawing("S"));
    assert_eq!(
        f.call("effect.status", json!({}))["active"]["S"],
        "wave_soft"
    );
    assert_eq!(f.call("daemon.persistence", json!({})), saved);
    assert_eq!(
        f.error("integration.set", json!({"name":"unknown","enabled":true})),
        rpc::INVALID_PARAMS
    );
}

#[test]
fn disabling_optional_engines_and_repeated_stop_are_safe() {
    let f = Fixture::new(false, false, 0);
    f.service.start();
    f.service.start();
    assert_eq!(f.state.lock().unwrap().restored, 1);
    assert_eq!(
        f.error("effect.set", json!({"serial":"S","effect":"wave_soft"})),
        rpc::NOT_SUPPORTED
    );
    assert_eq!(
        f.call("effect.status", json!({})),
        json!({"enabled":false,"active":{}})
    );
    f.service.stop();
    f.service.stop();
    assert_eq!(f.state.lock().unwrap().left, 1);
    assert_eq!(f.call("daemon.version", json!({}))["state"], "stopping");
    assert_eq!(
        f.error("integration.set", json!({"name":"chroma","enabled":true})),
        rpc::INVALID_PARAMS
    );
}

#[test]
fn daemon_version_does_not_wait_for_a_locked_hardware_device() {
    let f = Fixture::new(false, false, 0);
    f.service.start();
    let guard = lock_device(&f.device);
    let version = f.call("daemon.version", json!({}));
    drop(guard);
    assert_eq!(version["devices"], 1);
}

#[test]
fn shutdown_during_initial_scan_prevents_late_device_initialisation() {
    let f = Fixture::new(true, false, 0);
    *f.registry.gate.0.lock().unwrap() = true;
    let service = f.service.clone();
    let starter = thread::spawn(move || service.start());
    eventually(|| f.registry.scans.load(Ordering::SeqCst) == 1);
    f.service.request_stop();
    let state = f.call("daemon.version", json!({}))["state"].clone();
    *f.registry.gate.0.lock().unwrap() = false;
    f.registry.gate.1.notify_all();
    starter.join().unwrap();
    assert_eq!(state, "stopping");
    assert_eq!(f.call("daemon.version", json!({}))["state"], "stopping");
    assert_eq!(f.state.lock().unwrap().entered, 0);
    f.service.stop();
    assert_eq!(f.state.lock().unwrap().left, 1);
}
