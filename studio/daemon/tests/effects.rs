use mirage_hub_daemon::{
    database,
    device::Device,
    effects::{
        audio::{AudioMeter, SilentMeter},
        engine::{push_frame, EffectEngine, Environment},
        frame::{Colour, Frame},
        keys::KeySource,
        render::Renderer,
        wallpaper::WallpaperReader,
    },
    manager::SharedDevice,
    persistence::{Persistence, SharedStore},
    Error, Result,
};
use serde_json::{json, Map, Value};
use std::{
    sync::{atomic::{AtomicBool, Ordering}, mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

struct Inputs;
impl Environment for Inputs {
    fn audio(&self) -> Box<dyn AudioMeter> {
        Box::new(SilentMeter)
    }
    fn accent(&self) -> Option<Colour> {
        Some([20, 40, 60])
    }
}
struct FakeDevice {
    info: database::DeviceInfo,
    store: SharedStore,
    serial: String,
    capabilities: Value,
    calls: mpsc::Sender<(String, Vec<Value>)>,
    colours: Vec<Colour>,
    argb: bool,
    fail: bool,
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
        Ok("fake".into())
    }
    fn capabilities(&self) -> Value {
        self.capabilities.clone()
    }
    fn call(&mut self, method: &str, args: &[Value], _: &Map<String, Value>) -> Result<Value> {
        if method == "zone_colours_now" {
            return Ok(json!(self.colours));
        }
        if self.fail {
            return Err(Error::Device("device unplugged".into()));
        }
        if method == "set_zone_colours" {
            self.colours = serde_json::from_value(args[0].clone())?;
        }
        self.calls.send((method.into(), args.to_vec())).unwrap();
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
    fn custom_mode_first(&self) -> bool {
        self.argb
    }
}
type Calls = mpsc::Receiver<(String, Vec<Value>)>;
fn device(
    serial: &str,
    store: SharedStore,
    zones: bool,
    argb: bool,
    fail: bool,
) -> (SharedDevice, Calls) {
    let (send, receive) = mpsc::channel();
    let dimensions = if zones { [1, 2] } else { [6, 22] };
    let capabilities = json!({"name":"keyboard","custom_frame":true,"matrix_dimensions":dimensions,
        "zone_order":if zones{vec!["left","right"]}else{vec![]},"zone_colours":zones});
    (
        Arc::new(Mutex::new(Box::new(FakeDevice {
            info: database::lookup(0x1532, 0x0203).unwrap().clone(),
            store,
            serial: serial.into(),
            capabilities,
            calls: send,
            colours: vec![[255, 0, 0], [0, 0, 255]],
            argb,
            fail,
        }))),
        receive,
    )
}
fn options(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}
fn engine(store: SharedStore) -> EffectEngine {
    EffectEngine::with_environment(store, KeySource::new(false), Arc::new(Inputs))
}
fn call(calls: &Calls) -> (String, Vec<Value>) {
    calls.recv_timeout(Duration::from_secs(3)).unwrap()
}

#[test]
fn static_layers_send_once_and_stopping_wakes_the_idle_worker() {
    let store = Persistence::memory().shared();
    let (device, calls) = device("S", store.clone(), false, false, false);
    let mut engine = engine(store);
    engine
        .set_effect(
            &device,
            "layers",
            options(json!({"layers":[{"kind":"static","cells":[[2,3]],"colours":[[9,8,7]]}]})),
        )
        .unwrap();
    let (method, args) = call(&calls);
    assert_eq!(method, "set_key_row");
    assert!(args[0].as_array().unwrap().len() > 3);
    assert_eq!(call(&calls).0, "set_custom");
    assert!(calls.recv_timeout(Duration::from_millis(100)).is_err());
    let at = Instant::now();
    engine.clear(&device);
    assert!(at.elapsed() < Duration::from_millis(500));
    assert!(!engine.drawing(&device));
    assert!(engine.saved("S").is_some());
    assert!(engine.restore(&device));
    call(&calls);
    call(&calls);
    assert!(engine.stop_for(&device).unwrap());
    assert!(engine.saved("S").is_none());
}

#[test]
fn key_driven_workers_share_input_and_release_it_when_the_last_effect_stops() {
    let store = Persistence::memory().shared();
    let (first, first_calls) = device("A", store.clone(), false, false, false);
    let (second, second_calls) = device("B", store.clone(), false, false, false);
    let mut engine = engine(store);
    engine
        .set_effect(&first, "ripple", options(json!({"refresh_rate":0.005})))
        .unwrap();
    engine
        .set_effect(&second, "ripple", options(json!({"refresh_rate":0.005})))
        .unwrap();
    assert!(engine.keys.running());
    engine.keys.press("G");
    assert_eq!(call(&first_calls).0, "set_key_row");
    assert_eq!(call(&second_calls).0, "set_key_row");
    engine.clear(&first);
    assert!(engine.keys.running());
    engine.clear(&second);
    assert!(!engine.keys.running());
    while first_calls.try_recv().is_ok() {}
    while second_calls.try_recv().is_ok() {}
    engine.keys.press("G");
    assert!(first_calls.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn zone_changes_preserve_the_other_ear_and_still_zones_are_given_back_to_hardware() {
    let store = Persistence::memory().shared();
    let (device, calls) = device("EARS", store.clone(), true, false, false);
    let mut engine = engine(store);
    engine
        .set_effect(
            &device,
            "music",
            options(json!({"zone":"left","colour":[0,255,0],"refresh_rate":0.005})),
        )
        .unwrap();
    assert_eq!(
        engine.zones_now(&device)["right"],
        json!({"effect":"static","colour":[0,0,255]})
    );
    call(&calls);
    call(&calls);
    let result=engine.set_effect(&device,"zones",options(json!({"zones":{"left":{"effect":"static","colour":[255,0,0]},"right":{"effect":"none"}}}))).unwrap();
    assert!(result["effect"].is_null());
    assert!(!engine.drawing(&device));
    assert!(engine.saved("EARS").is_none());
    let mut found = false;
    while let Ok((method, args)) = calls.try_recv() {
        if method == "set_zone_colours" {
            assert_eq!(args[0], json!([[255, 0, 0], [0, 0, 0]]));
            found = true;
        }
    }
    assert!(found);
    assert!(engine.set_effect(&device, "ripple", Map::new()).is_err());
    assert!(engine
        .set_effect(&device, "wave_soft", options(json!({"refresh_rate":0.0})))
        .is_err());
}

#[test]
fn accent_frames_are_held_and_unsupported_input_does_not_stop_the_current_effect() {
    let store = Persistence::memory().shared();
    let (device, calls) = device("EARS", store.clone(), true, false, false);
    let mut engine = engine(store);
    engine
        .set_effect(&device, "accent", options(json!({"refresh_rate":0.005})))
        .unwrap();
    let (method, args) = call(&calls);
    assert_eq!(method, "set_zone_colours");
    assert_eq!(args[0], json!([[20, 40, 60], [20, 40, 60]]));
    assert!(engine
        .set_effect(&device, "music", options(json!({"colour":[1]})))
        .is_err());
    assert!(engine.drawing(&device));
    assert!(calls.recv_timeout(Duration::from_millis(40)).is_err());
    engine.stop();
}

#[test]
fn keyboard_accent_uses_static_effect_for_all_lights_including_black() {
    struct Accent(Colour);
    impl Environment for Accent {
        fn audio(&self) -> Box<dyn AudioMeter> { Box::new(SilentMeter) }
        fn accent(&self) -> Option<Colour> { Some(self.0) }
    }

    for colour in [[20, 40, 60], [0, 0, 0]] {
        let store = Persistence::memory().shared();
        let (send, calls) = mpsc::channel();
        let device: SharedDevice = Arc::new(Mutex::new(Box::new(FakeDevice {
            info: database::lookup(0x1532, 0x02b9).unwrap().clone(),
            store: store.clone(),
            serial: "MINI".into(),
            capabilities: json!({
                "name":"Razer BlackWidow V4 Mini HyperSpeed (Wired)",
                "custom_frame":true, "matrix_dimensions":[5,14],
                "zones":{"backlight":{"effects":["static"]}}
            }),
            calls: send,
            colours: vec![],
            argb: false,
            fail: false,
        })));
        let mut engine = EffectEngine::with_environment(store, KeySource::new(false), Arc::new(Accent(colour)));
        engine.set_effect(&device, "accent", options(json!({"refresh_rate":0.005}))).unwrap();
        let (method, args) = call(&calls);
        assert_eq!(method, "set_static");
        assert_eq!(args, colour.map(|channel| json!(channel)).to_vec());
        assert!(calls.recv_timeout(Duration::from_millis(30)).is_err());
        engine.stop();
    }
}

#[test]
fn keyboard_accent_tracks_a_new_windows_colour() {
    struct ChangingAccent(Arc<Mutex<Colour>>);
    impl Environment for ChangingAccent {
        fn audio(&self) -> Box<dyn AudioMeter> { Box::new(SilentMeter) }
        fn accent(&self) -> Option<Colour> { Some(*self.0.lock().unwrap()) }
    }
    let store = Persistence::memory().shared();
    let (send, calls) = mpsc::channel();
    let device: SharedDevice = Arc::new(Mutex::new(Box::new(FakeDevice {
        info: database::lookup(0x1532, 0x02b9).unwrap().clone(),
        store: store.clone(),
        serial: "MINI".into(),
        capabilities: json!({"custom_frame":true,"matrix_dimensions":[5,14],"zones":{"backlight":{"effects":["static"]}}}),
        calls: send,
        colours: vec![],
        argb: false,
        fail: false,
    })));
    let source = Arc::new(Mutex::new([255, 0, 0]));
    let mut engine = EffectEngine::with_environment(store, KeySource::new(false), Arc::new(ChangingAccent(source.clone())));
    engine.set_effect(&device, "accent", options(json!({"refresh_rate":0.005}))).unwrap();
    assert_eq!(call(&calls), ("set_static".into(), vec![json!(255), json!(0), json!(0)]));
    *source.lock().unwrap() = [128, 0, 255];
    assert_eq!(calls.recv_timeout(Duration::from_secs(6)).unwrap(), ("set_static".into(), vec![json!(128), json!(0), json!(255)]));
    engine.stop();
}

#[test]
fn wallpaper_waits_for_a_frame_tracks_changes_and_releases_its_reader() {
    struct Reader {
        colour: Arc<Mutex<Option<Colour>>>,
        dropped: Arc<AtomicBool>,
    }
    impl WallpaperReader for Reader {
        fn sample(&mut self) -> Option<Colour> { *self.colour.lock().unwrap() }
    }
    impl Drop for Reader {
        fn drop(&mut self) { self.dropped.store(true, Ordering::SeqCst); }
    }
    struct Desktop(Arc<Mutex<Option<Colour>>>, Arc<AtomicBool>);
    impl Environment for Desktop {
        fn audio(&self) -> Box<dyn AudioMeter> { Box::new(SilentMeter) }
        fn accent(&self) -> Option<Colour> { None }
        fn wallpaper(&self) -> Box<dyn WallpaperReader> {
            Box::new(Reader { colour: self.0.clone(), dropped: self.1.clone() })
        }
    }
    let store = Persistence::memory().shared();
    let (send, calls) = mpsc::channel();
    let device: SharedDevice = Arc::new(Mutex::new(Box::new(FakeDevice {
        info: database::lookup(0x1532, 0x02b9).unwrap().clone(),
        store: store.clone(), serial: "WALL".into(),
        // A static-only device can follow the wallpaper too.
        capabilities: json!({"zones":{"backlight":{"effects":["static"]}}}),
        calls: send, colours: vec![], argb: false, fail: false,
    })));
    let colour = Arc::new(Mutex::new(None));
    let dropped = Arc::new(AtomicBool::new(false));
    let mut engine = EffectEngine::with_environment(
        store, KeySource::new(false), Arc::new(Desktop(colour.clone(), dropped.clone())),
    );
    engine.set_effect(&device, "wallpaper", options(json!({"refresh_rate":0.005}))).unwrap();
    assert!(calls.recv_timeout(Duration::from_millis(100)).is_err());
    *colour.lock().unwrap() = Some([0, 0, 0]);
    assert_eq!(call(&calls), ("set_static".into(), vec![json!(0), json!(0), json!(0)]));
    *colour.lock().unwrap() = Some([200, 80, 20]);
    assert_eq!(call(&calls), ("set_static".into(), vec![json!(200), json!(80), json!(20)]));
    assert_eq!(engine.status()["wallpaper_colours"]["WALL"], json!([200, 80, 20]));
    engine.set_effect(&device, "none", Map::new()).unwrap();
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn zones_can_follow_wallpaper_and_windows_accent_independently() {
    let caps = json!({"matrix_dimensions":[1,2],"zone_order":["left","right"]});
    let settings = options(json!({"zones":{
        "left":{"effect":"accent"}, "right":{"effect":"wallpaper"}
    }}));
    let mut renderer = Renderer::new("zones", settings, &caps).unwrap();
    assert!(renderer.step(0.0, 0.0, Some([255, 0, 0]), Some([0, 0, 255])));
    assert_eq!(renderer.frame.get(0, 0), [255, 0, 0]);
    assert_eq!(renderer.frame.get(0, 1), [0, 0, 255]);
    assert!(renderer.step(1.0, 0.0, None, Some([0, 255, 0])));
    assert_eq!(renderer.frame.get(0, 0), [255, 0, 0]);
    assert_eq!(renderer.frame.get(0, 1), [0, 255, 0]);
}

#[test]
fn failed_device_writes_end_the_worker_and_leave_the_saved_effect_for_reconnection() {
    let store = Persistence::memory().shared();
    let (device, _calls) = device("GONE", store.clone(), false, false, true);
    let mut engine = engine(store);
    engine
        .set_effect(&device, "wave_soft", options(json!({"refresh_rate":0.005})))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while engine.drawing(&device) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!engine.drawing(&device));
    assert!(engine.saved("GONE").is_some());
    assert_eq!(engine.errors()["GONE"], "device unplugged");
}

#[test]
fn argb_switches_modes_before_the_first_frame_and_other_devices_apply_after_each_frame() {
    for argb in [false, true] {
        let (device, calls) = device("PUSH", Persistence::memory().shared(), false, argb, false);
        let mut custom = false;
        let mut frame = Frame::new(1, 2).unwrap();
        frame.fill([1, 2, 3]);
        for _ in 0..2 {
            push_frame(
                device.lock().unwrap().as_mut(),
                &frame,
                false,
                &mut custom,
                false,
            )
            .unwrap();
        }
        push_frame(
            device.lock().unwrap().as_mut(),
            &frame,
            false,
            &mut custom,
            true,
        )
        .unwrap();
        let methods: Vec<_> = calls.try_iter().map(|(method, _)| method).collect();
        assert_eq!(
            methods,
            if argb {
                vec!["set_custom", "set_key_row", "set_key_row"]
            } else {
                vec!["set_key_row", "set_custom", "set_key_row", "set_custom"]
            }
        );
    }
}

#[test]
fn dimensions_and_json_times_cannot_overflow_or_drift() {
    assert!(Frame::new(usize::MAX, usize::MAX).is_err());
    let value: f64 = serde_json::from_str("1.4000000000000001").unwrap();
    assert_eq!(value.to_bits(), 1.4000000000000001_f64.to_bits());
}
