use openrazer_daemon::{
    database,
    device::Device,
    hid::{HidBackend, HidHandle, HidInfo, InputReader},
    keyboard::KeyEvent,
    keys::{
        codes::{self, Output},
        inject::{Injection, Injector, Launcher, KEY_UP},
        macros::{self, MacroStore},
        service::{is_event_collection, KeyService},
    },
    manager::SharedDevice,
    persistence::{Persistence, SharedStore},
    Error, Result,
};
use serde_json::{json, Map, Value};
use std::{
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct OutputRecorder {
    events: Mutex<Vec<Injection>>,
    launches: Mutex<Vec<String>>,
    fail_scan: Option<u16>,
}
impl Injector for OutputRecorder {
    fn emit(&self, events: &[Injection]) -> Result<()> {
        self.events.lock().unwrap().extend_from_slice(events);
        if events
            .iter()
            .any(|e| Some(e.scan) == self.fail_scan && e.flags & KEY_UP == 0)
        {
            return Err(Error::Io("injected test failure".into()));
        }
        Ok(())
    }
}
impl Launcher for OutputRecorder {
    fn launch(&self, target: &str) -> Result<()> {
        self.launches.lock().unwrap().push(target.into());
        Ok(())
    }
}
impl OutputRecorder {
    fn take(&self) -> Vec<Injection> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
}
struct Reader(mpsc::Receiver<Vec<u8>>);
impl InputReader for Reader {
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        match self.0.recv_timeout(timeout) {
            Ok(report) => Ok(Some(report)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(_) => Err(Error::Io("unplugged".into())),
        }
    }
}
struct Backend {
    reader: Mutex<Option<Reader>>,
    info: HidInfo,
}
impl HidBackend for Backend {
    fn enumerate(&self, _: Option<u16>, _: Option<u16>) -> Result<Vec<HidInfo>> {
        Ok(vec![self.info.clone()])
    }
    fn open(&self, _: &HidInfo) -> Result<Box<dyn HidHandle>> {
        unreachable!()
    }
    fn open_input(&self, _: &HidInfo) -> Result<Box<dyn InputReader>> {
        self.reader
            .lock()
            .unwrap()
            .take()
            .map(|r| Box::new(r) as Box<dyn InputReader>)
            .ok_or_else(|| Error::Io("not readable".into()))
    }
}
#[derive(Default)]
struct DeviceState {
    calls: Vec<(String, Vec<Value>)>,
    brightness: f64,
}
struct FakeDevice {
    info: database::DeviceInfo,
    store: SharedStore,
    state: Arc<Mutex<DeviceState>>,
    serial: String,
}
impl Device for FakeDevice {
    fn info(&self) -> &database::DeviceInfo {
        &self.info
    }
    fn persistence(&self) -> &SharedStore {
        &self.store
    }
    fn serial(&mut self) -> String {
        self.serial.clone()
    }
    fn firmware(&mut self) -> Result<String> {
        Ok("test".into())
    }
    fn capabilities(&self) -> Value {
        json!({})
    }
    fn call(&mut self, method: &str, args: &[Value], _: &Map<String, Value>) -> Result<Value> {
        let mut state = self.state.lock().unwrap();
        state.calls.push((method.into(), args.into()));
        if method == "get_brightness" {
            return Ok(json!(state.brightness));
        }
        if method == "set_brightness" {
            state.brightness = args[0].as_f64().unwrap();
        }
        Ok(Value::Null)
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
    fn close(&mut self) {}
}
struct Rig {
    service: KeyService,
    device: SharedDevice,
    output: Arc<OutputRecorder>,
    state: Arc<Mutex<DeviceState>>,
    reports: mpsc::Sender<Vec<u8>>,
}
impl Rig {
    fn new(pid: u16) -> Self {
        let (send, receive) = mpsc::channel();
        let backend = Arc::new(Backend {
            reader: Mutex::new(Some(Reader(receive))),
            info: HidInfo {
                usage_page: 1,
                usage: 0,
                input_length: 16,
                ..Default::default()
            },
        });
        let output = Arc::new(OutputRecorder::default());
        let state = Arc::new(Mutex::new(DeviceState {
            brightness: 50.0,
            ..Default::default()
        }));
        let device: SharedDevice = Arc::new(Mutex::new(Box::new(FakeDevice {
            info: database::lookup(0x1532, pid).unwrap().clone(),
            store: Persistence::memory().shared(),
            state: state.clone(),
            serial: "KEYBOARD".into(),
        })));
        let mut service = KeyService::new(
            backend,
            Arc::new(MacroStore::memory()),
            output.clone(),
            output.clone(),
            false,
        );
        service.start().unwrap();
        Self {
            service,
            device,
            output,
            state,
            reports: send,
        }
    }
    fn attached() -> Self {
        let mut rig = Self::new(0x0203);
        assert!(rig.service.attach(&rig.device).unwrap());
        rig
    }
    fn report(&self, codes: &[u8]) {
        let mut bytes = vec![4];
        bytes.extend(codes);
        self.service.handle_report("KEYBOARD", &bytes);
    }
    fn outputs(&self) -> Vec<Injection> {
        self.service.flush_inputs(Duration::from_secs(2)).unwrap();
        self.output.take()
    }
    fn bind(&self, key: &str, action: Value) {
        self.service.macros().set("KEYBOARD", key, &action).unwrap();
    }
}
fn event(name: &str, down: bool, time: u32) -> KeyEvent {
    let e = codes::key_named(name).unwrap().event(down);
    KeyEvent {
        vk: u32::from(e.vk),
        scan: u32::from(e.scan),
        extended: e.ext,
        down,
        injected: false,
        ours: false,
        time,
    }
}
fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "asynchronous action did not finish"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn takeover_requires_readable_vendor_reports_and_a_supported_keyboard() {
    let mut rig = Rig::new(0x0210); // Firmware-handled Blade keyboard.
    assert!(!rig.service.attach(&rig.device).unwrap());
    let mut rig = Rig::new(0x0203);
    assert!(!rig.service.handles(&rig.device));
    rig.service.stop();
    assert!(!rig.service.attach(&rig.device).unwrap());
    rig.service.start().unwrap();
    assert!(rig.service.attach(&rig.device).unwrap());
    assert_eq!(
        rig.service.macro_keys(&rig.device),
        ["M1", "M2", "M3", "M4", "M5"]
    );
    rig.service.detach(&rig.device);
    assert!(!rig.service.attach(&rig.device).unwrap()); // Reader cannot be opened again.
    for (page, usage, length, expected) in [
        (1, 6, 16, false),
        (1, 2, 16, false),
        (1, 0, 16, true),
        (0xff00, 1, 16, true),
        (0xff00, 1, 0, false),
    ] {
        assert_eq!(
            is_event_collection(&HidInfo {
                usage_page: page,
                usage,
                input_length: length,
                ..Default::default()
            }),
            expected
        );
    }
}

#[test]
fn real_reader_dispatches_default_macro_keys_and_ignores_other_reports() {
    let rig = Rig::attached();
    rig.reports.send(vec![3, 0x20]).unwrap();
    rig.reports.send(vec![4, 0x20, 0x20, 0]).unwrap();
    rig.reports.send(vec![4, 0]).unwrap();
    until(|| rig.output.events.lock().unwrap().len() == 2);
    let events = rig.outputs();
    assert_eq!(
        events
            .iter()
            .map(|e| (e.vk, e.flags & KEY_UP != 0))
            .collect::<Vec<_>>(),
        [(0x7c, false), (0x7c, true)]
    );
    assert!(rig.service.errors().is_empty());
}

#[test]
fn fn_release_repeat_injected_input_and_game_mode_match_physical_keys() {
    let rig = Rig::attached();
    assert!(!rig.service.handle_key(event("f1", true, 0)));
    rig.report(&[1]);
    let mut injected = event("f1", true, 0);
    injected.injected = true;
    assert!(!rig.service.handle_key(injected));
    assert!(rig.service.handle_key(event("f1", true, 0)));
    assert!(rig.service.handle_key(event("f1", true, 0)));
    rig.report(&[]); // Releasing Fn first still releases Mute.
    assert!(rig.service.handle_key(event("f1", false, 0)));
    assert_eq!(
        rig.outputs()
            .iter()
            .map(|e| (e.vk, e.flags & KEY_UP != 0))
            .collect::<Vec<_>>(),
        [(0xad, false), (0xad, false), (0xad, true)]
    );
    rig.report(&[1]);
    assert!(rig.service.handle_key(event("f10", true, 0)));
    assert!(rig.service.handle_key(event("f10", true, 0))); // Repeat must not toggle back.
    assert!(rig.service.handle_key(event("f10", false, 0)));
    until(|| {
        rig.service.status(std::slice::from_ref(&rig.device))["devices"][0]["game_mode"] == true
    });
    assert!(rig.service.handle_key(event("win", true, 0)));
    assert!(rig.service.handle_key(event("win", false, 0)));
    assert!(rig.service.handle_key(event("f12", true, 0)));
    assert!(rig.service.handle_key(event("f12", false, 0)));
    until(|| rig.state.lock().unwrap().brightness == 60.0);
}

#[test]
fn macro_binding_replaces_default_and_disabled_does_nothing() {
    let rig = Rig::attached();
    rig.bind("M1", json!({"type":"text","text":"你好😀"}));
    rig.bind("M2", json!({"type":"launch","target":"notepad.exe"}));
    rig.bind("M3", json!({"type":"disabled"}));
    rig.report(&[0x20, 0x21, 0x22]);
    rig.report(&[]);
    until(|| {
        rig.output.events.lock().unwrap().len() == 8
            && rig.output.launches.lock().unwrap().len() == 1
    });
    assert_eq!(
        rig.outputs(),
        openrazer_daemon::keys::inject::text_inputs("你好😀")
    );
    assert_eq!(*rig.output.launches.lock().unwrap(), ["notepad.exe"]);
}

#[test]
fn fn_recording_selects_a_macro_key_preserves_timing_and_can_cancel() {
    let rig = Rig::attached();
    rig.report(&[1]);
    rig.service.handle_key(event("f9", true, 0));
    rig.service.handle_key(event("f9", false, 0));
    until(|| !rig.service.status(&[])["recording"].is_null());
    rig.report(&[0x21]);
    rig.report(&[]);
    assert_eq!(rig.service.status(&[])["recording"]["key"], "M2");
    assert!(!rig.service.handle_key(event("a", true, 100)));
    assert!(!rig.service.handle_key(event("a", false, 180)));
    rig.report(&[1]);
    rig.service.handle_key(event("f9", true, 200));
    rig.service.handle_key(event("f9", false, 210));
    until(|| rig.service.macros().get("KEYBOARD", "M2").is_some());
    let saved = rig.service.macros().get("KEYBOARD", "M2").unwrap();
    assert_eq!(
        saved["events"],
        json!([{"vk":65,"scan":30,"ext":false,"down":true,"delay":0},{"vk":65,"scan":30,"ext":false,"down":false,"delay":80}])
    );
    rig.service.start_recording("KEYBOARD", "M2").unwrap();
    rig.service.handle_key(event("b", true, 0));
    rig.report(&[0x21]); // Pressing the recording's own macro key cancels.
    assert!(rig.service.status(&[])["recording"].is_null());
    assert_eq!(rig.service.macros().get("KEYBOARD", "M2"), Some(saved));
}

#[test]
fn rpc_recording_empty_clears_binding_and_excludes_injected_events() {
    let rig = Rig::attached();
    rig.bind("M4", json!({"type":"text","text":"old"}));
    rig.service.start_recording("KEYBOARD", "M4").unwrap();
    let mut input = event("a", true, 0);
    input.injected = true;
    rig.service.handle_key(input);
    assert_eq!(rig.service.stop_recording().unwrap(), None);
    assert_eq!(rig.service.macros().get("KEYBOARD", "M4"), None);
    rig.service.start_recording("KEYBOARD", "M3").unwrap();
    rig.service.handle_key(event("a", true, 0));
    rig.service.handle_key(event("a", false, 40));
    let saved = rig.service.stop_recording().unwrap().unwrap();
    assert_eq!(saved["events"][1]["delay"], 40);
    assert!(rig.service.start_recording("missing", "M1").is_err());
}

#[test]
fn unfinished_or_failed_macros_release_every_held_key() {
    let rig = Rig::attached();
    let events = codes::parse_sequence("shift+a").unwrap();
    rig.service
        .play(&json!({"type":"keys","events":&events[..2]}))
        .unwrap();
    assert_eq!(
        rig.output
            .take()
            .iter()
            .filter(|e| e.flags & KEY_UP != 0)
            .count(),
        2
    );
    let failure = Arc::new(OutputRecorder {
        fail_scan: Some(0x1e),
        ..Default::default()
    });
    let service = KeyService::new(
        Arc::new(Backend {
            reader: Mutex::new(None),
            info: HidInfo::default(),
        }),
        Arc::new(MacroStore::memory()),
        failure.clone(),
        failure.clone(),
        false,
    );
    assert!(service
        .play(&json!({"type":"keys","events":&events[..2]}))
        .is_err());
    let released: Vec<_> = failure
        .take()
        .into_iter()
        .filter(|e| e.flags & KEY_UP != 0)
        .map(|e| e.scan)
        .collect();
    assert!(released.contains(&0x2a) && released.contains(&0x1e));
}

#[test]
fn stopping_cancels_long_macros_and_releases_fn_and_default_keys() {
    let mut rig = Rig::attached();
    let action = json!({"type":"keys","events":[{"vk":162,"scan":29,"down":true},{"vk":65,"scan":30,"delay":30000}]});
    assert!(rig.service.play_async("KEYBOARD", "M2", &action).unwrap());
    until(|| !rig.output.events.lock().unwrap().is_empty());
    assert!(!rig.service.play_async("KEYBOARD", "M2", &action).unwrap());
    rig.report(&[1, 0x20]);
    rig.service.handle_key(event("f1", true, 0));
    rig.service.flush_inputs(Duration::from_secs(2)).unwrap();
    let start = Instant::now();
    rig.service.stop();
    assert!(start.elapsed() < Duration::from_secs(2));
    let events = rig.output.take();
    for (vk, scan) in [(0, 29), (0xad, 0), (0x7c, 0x64)] {
        assert!(
            events
                .iter()
                .any(|e| e.vk == vk && e.scan == scan && e.flags & KEY_UP != 0),
            "missing release for {vk}/{scan}"
        );
    }
    assert!(!rig.service.running());
    assert!(!rig.service.handle_key(event("f1", true, 0)));
}

#[test]
fn mini_fn_macro_keys_work_and_detach_does_not_leave_recording_active() {
    let mut rig = Rig::new(0x0258); // BlackWidow V3 Mini has Fn macro targets.
    assert!(rig.service.attach(&rig.device).unwrap());
    let table = openrazer_daemon::keys::service::table_for(0x0258);
    let source = table
        .iter()
        .find(|(_, target)| *target == "KEY_MACRO1")
        .unwrap()
        .0;
    let key = codes::output_for(source).unwrap().event(true);
    rig.report(&[1]);
    let mut input = KeyEvent {
        vk: u32::from(key.vk),
        scan: u32::from(key.scan),
        extended: key.ext,
        down: true,
        injected: false,
        ours: false,
        time: 0,
    };
    assert!(rig.service.handle_key(input));
    input.down = false;
    assert!(rig.service.handle_key(input));
    assert_eq!(rig.outputs().len(), 2);
    rig.service.start_recording("KEYBOARD", "M1").unwrap();
    rig.service.detach(&rig.device);
    assert!(rig.service.status(&[])["recording"].is_null());
}

#[test]
fn old_macro_json_round_trips_unicode_and_rejects_invalid_updates() {
    let root = std::env::temp_dir().join(format!("openrazer-macro-test-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("macros.json");
    std::fs::write(
        &path,
        r#"{"SERIAL":{"M1":{"type":"text","text":"中文😀"}},"invalid":12}"#,
    )
    .unwrap();
    let store = MacroStore::load(path.clone());
    assert_eq!(store.get("SERIAL", "M1").unwrap()["text"], "中文😀");
    let clean = store
        .set(
            "SERIAL",
            "M2",
            &json!({"type":"keys","events":[{"vk":"65","delay":1.9,"ext":0,"down":"yes"}]}),
        )
        .unwrap();
    assert_eq!(
        clean["events"][0],
        json!({"vk":65,"scan":0,"delay":1,"ext":false,"down":true})
    );
    assert!(store
        .set("SERIAL", "M1", &json!({"type":"text","text":""}))
        .is_err());
    assert_eq!(
        MacroStore::load(path.clone()).get("SERIAL", "M1").unwrap()["text"],
        "中文😀"
    );
    assert!(store.delete("SERIAL", "M2").unwrap());
    assert!(!store.delete("SERIAL", "M2").unwrap());
    for bad in [
        Value::Null,
        json!({}),
        json!({"type":"keys","events":[]}),
        json!({"type":"keys","events":[{"scan":65536}]}),
        json!({"type":"keys","events":[{"vk":1,"delay":30001}]}),
        json!({"type":"launch","target":"  "}),
    ] {
        assert!(macros::validate(&bad).is_err());
    }
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn keyboard_output_handles_pause_high_function_keys_media_and_utf16() {
    use openrazer_daemon::keys::inject::{key_input, raw_input, text_inputs};
    assert_eq!(
        key_input(codes::key_named("pause").unwrap(), true),
        Injection {
            vk: 0x13,
            scan: 0x45,
            flags: 0
        }
    );
    assert_eq!(
        key_input(codes::key_named("f24").unwrap(), true),
        Injection {
            vk: 0x87,
            scan: 0x76,
            flags: 0
        }
    );
    assert_eq!(key_input(Output::Virtual(0xad), true).flags, 1);
    let chord = codes::parse_sequence("ctrl+c, win+shift+s").unwrap();
    assert_eq!(
        chord.iter().map(|e| e.down).collect::<Vec<_>>(),
        [true, true, false, false, true, true, true, false, false, false]
    );
    assert_eq!(raw_input(&chord[0]).vk, 0);
    assert_eq!(
        text_inputs("😀").iter().map(|e| e.scan).collect::<Vec<_>>(),
        [0xd83d, 0xd83d, 0xde00, 0xde00]
    );
}

#[test]
fn slow_input_delivery_cannot_block_the_keyboard_callback() {
    struct SlowOutput {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        once: std::sync::atomic::AtomicBool,
    }
    impl Injector for SlowOutput {
        fn emit(&self, _: &[Injection]) -> Result<()> {
            if !self.once.swap(true, std::sync::atomic::Ordering::SeqCst) {
                self.entered.send(()).unwrap();
                // A timeout bounds cleanup even if the assertion fails.
                let _ = self
                    .release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2));
            }
            Ok(())
        }
    }
    let rig = Rig::new(0x0203);
    let (_reports, receiver) = mpsc::channel();
    let backend = Arc::new(Backend {
        reader: Mutex::new(Some(Reader(receiver))),
        info: HidInfo {
            usage_page: 1,
            usage: 0,
            input_length: 16,
            ..Default::default()
        },
    });
    let (entered, receive) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let output = Arc::new(SlowOutput {
        entered,
        release: Mutex::new(wait),
        once: std::sync::atomic::AtomicBool::new(false),
    });
    let mut service = KeyService::new(
        backend,
        Arc::new(MacroStore::memory()),
        output,
        rig.output.clone(),
        false,
    );
    service.start().unwrap();
    service.attach(&rig.device).unwrap();
    service.handle_report("KEYBOARD", &[4, 1]);
    let start = Instant::now();
    assert!(service.handle_key(event("f1", true, 0)));
    assert!(start.elapsed() < Duration::from_millis(500));
    receive.recv_timeout(Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    assert!(!service.handle_key(event("a", true, 0)));
    assert!(service.handle_key(event("f1", false, 0)));
    assert!(start.elapsed() < Duration::from_millis(500));
    release.send(()).unwrap();
    service.flush_inputs(Duration::from_secs(1)).unwrap();
}
