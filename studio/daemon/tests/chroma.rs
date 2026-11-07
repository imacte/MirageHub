use openrazer_daemon::{
    chroma::{
        bridge::{ChromaBridge, EffectControl},
        grid,
        rest::ChromaServer,
    },
    database,
    device::Device,
    manager::SharedDevice,
    persistence::{self, Persistence, SharedStore},
    Error, Result,
};
use serde_json::{json, Map, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct State {
    calls: Vec<(String, Vec<Value>, Map<String, Value>)>,
    restored: usize,
    fail: bool,
    pause: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
}
struct FakeDevice {
    serial: String,
    info: database::DeviceInfo,
    caps: Value,
    state: Arc<Mutex<State>>,
    store: SharedStore,
    argb: bool,
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
        self.caps.clone()
    }
    fn call(&mut self, method: &str, args: &[Value], kwargs: &Map<String, Value>) -> Result<Value> {
        let mut state = self.state.lock().unwrap();
        if state.fail {
            return Err(Error::Io("unplugged".into()));
        }
        state
            .calls
            .push((method.into(), args.to_vec(), kwargs.clone()));
        if method == "set_static" {
            let zone = kwargs
                .get("zone")
                .and_then(Value::as_str)
                .unwrap_or("backlight");
            persistence::lock(&self.store).set(&self.serial, zone, "colors", json!(args));
        }
        if method == "set_custom" {
            persistence::lock(&self.store).set(
                &self.serial,
                "backlight",
                "effect",
                json!("custom"),
            );
        }
        if method == "set_zone_colours" {
            persistence::lock(&self.store).set_device(
                &self.serial,
                "zone_colours",
                args[0].clone(),
            );
        }
        let pause = state.pause.take();
        drop(state);
        if let Some((entered, release)) = pause {
            entered.send(()).unwrap();
            release.recv_timeout(Duration::from_secs(2)).unwrap();
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
        let mut state = self.state.lock().unwrap();
        state.restored += 1;
        if state.fail {
            Err(Error::Io("unplugged".into()))
        } else {
            Ok(())
        }
    }
    fn close(&mut self) {}
    fn custom_mode_first(&self) -> bool {
        self.argb
    }
}
#[derive(Default)]
struct Effects {
    cleared: Mutex<Vec<String>>,
    restored: Mutex<Vec<String>>,
}
impl EffectControl for Effects {
    fn clear_serial(&self, serial: &str) {
        self.cleared.lock().unwrap().push(serial.into());
    }
    fn restore(&self, device: &SharedDevice) {
        self.restored
            .lock()
            .unwrap()
            .push(device.lock().unwrap().serial());
    }
}
fn device(
    serial: &str,
    kind: &str,
    caps: Value,
    store: SharedStore,
    argb: bool,
) -> (SharedDevice, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State::default()));
    let mut info = database::lookup(0x1532, 0x0203).unwrap().clone();
    info.device_type = kind.into();
    (
        Arc::new(Mutex::new(Box::new(FakeDevice {
            serial: serial.into(),
            info,
            caps,
            state: state.clone(),
            store,
            argb,
        }))),
        state,
    )
}
fn keyboard_caps() -> Value {
    json!({"matrix_dimensions":[6,22],"custom_frame":true,"zones":{"backlight":{"effects":["static"]}}})
}
fn solid(colour: u32) -> Value {
    json!({"effect":"CHROMA_STATIC","param":{"color":colour}})
}
struct Rig {
    bridge: Arc<ChromaBridge>,
    devices: Arc<Mutex<Vec<SharedDevice>>>,
    state: Arc<Mutex<State>>,
    store: SharedStore,
    effects: Arc<Effects>,
    now: Arc<Mutex<f64>>,
}
impl Rig {
    fn new() -> Self {
        let store = Persistence::memory().shared();
        let (keyboard, state) = device("KBD", "keyboard", keyboard_caps(), store.clone(), false);
        let devices = Arc::new(Mutex::new(vec![keyboard]));
        let source = devices.clone();
        let effects = Arc::new(Effects::default());
        let now = Arc::new(Mutex::new(0.0));
        let clock = now.clone();
        let bridge = Arc::new(ChromaBridge::with_clock(
            Arc::new(move || source.lock().unwrap().clone()),
            Some(effects.clone()),
            Some(store.clone()),
            None,
            Arc::new(move || *clock.lock().unwrap()),
        ));
        Self {
            bridge,
            devices,
            state,
            store,
            effects,
            now,
        }
    }
}
fn until(mut condition: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < end, "asynchronous Chroma action timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn game_frames_preserve_the_saved_lighting_and_restore_it_on_exit() {
    let rig = Rig::new();
    {
        let mut store = persistence::lock(&rig.store);
        store.set("KBD", "backlight", "effect", json!("static"));
        store.set("KBD", "backlight", "colors", json!([0, 0, 255]));
    }
    let saved = persistence::lock(&rig.store).as_json();
    let session = rig.bridge.open(&json!({"title":"游戏"}));
    let mut look = vec![vec![0u32; 22]; 6];
    look[3][2] = 0x00ff00;
    rig.bridge
        .show(
            session,
            "keyboard",
            &json!({"effect":"CHROMA_CUSTOM","param":look}),
        )
        .unwrap();
    rig.bridge.render();
    rig.bridge.render();
    assert!(rig.bridge.drawing("KBD"));
    assert_eq!(persistence::lock(&rig.store).as_json(), saved);
    let state = rig.state.lock().unwrap();
    let frames: Vec<_> = state
        .calls
        .iter()
        .filter(|(method, _, _)| method == "set_key_row")
        .collect();
    assert_eq!(frames.len(), 1);
    let bytes: Vec<u8> = serde_json::from_value(frames[0].1[0].clone()).unwrap();
    let offset = 3 * (3 + 22 * 3) + 3 + 2 * 3;
    assert_eq!(&bytes[offset..offset + 3], &[0, 255, 0]);
    drop(state);
    assert_eq!(*rig.effects.cleared.lock().unwrap(), ["KBD"]);
    rig.bridge.close(session);
    rig.bridge.render();
    assert!(!rig.bridge.drawing("KBD"));
    assert_eq!(rig.state.lock().unwrap().restored, 1);
    assert_eq!(*rig.effects.restored.lock().unwrap(), ["KBD"]);
    persistence::lock(&rig.store).set("KBD", "backlight", "effect", json!("none"));
    assert_eq!(
        persistence::lock(&rig.store).get("KBD", "backlight", "effect"),
        "none"
    );
}

#[test]
fn latest_session_wins_and_heartbeat_extends_ownership() {
    let rig = Rig::new();
    let first = rig.bridge.open(&json!({"title":"One"}));
    let second = rig.bridge.open(&json!({"title":"Two"}));
    rig.bridge.show(first, "keyboard", &solid(255)).unwrap();
    rig.bridge
        .show(second, "keyboard", &solid(0xff0000))
        .unwrap();
    assert_eq!(rig.bridge.status()["showing"], "One"); // Equal timestamps preserve insertion order.
    *rig.now.lock().unwrap() = 1.0;
    rig.bridge
        .show(second, "keyboard", &solid(0xff0000))
        .unwrap();
    rig.bridge.render();
    assert_eq!(rig.bridge.status()["showing"], "Two");
    *rig.now.lock().unwrap() = 15.0;
    assert_eq!(rig.bridge.heartbeat(second).unwrap(), 1);
    rig.bridge.expire();
    assert_eq!(rig.bridge.status()["apps"].as_array().unwrap().len(), 2);
    *rig.now.lock().unwrap() = 29.0;
    rig.bridge.expire();
    assert_eq!(rig.bridge.status()["apps"], json!(["Two"]));
    *rig.now.lock().unwrap() = 31.0;
    rig.bridge.expire();
    rig.bridge.render();
    assert_eq!(rig.bridge.status()["apps"], json!([]));
    assert!(!rig.bridge.drawing("KBD"));
}

#[test]
fn prepared_effects_only_show_when_applied_and_remain_after_delete() {
    let rig = Rig::new();
    let session = rig.bridge.open(&json!({}));
    let id = rig.bridge.create(session, "keyboard", &solid(255)).unwrap();
    assert_eq!(id.len(), 36);
    assert_eq!(&id[14..15], "4");
    assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
    assert!(rig.bridge.status()["showing"].is_null());
    rig.bridge
        .apply(session, std::slice::from_ref(&id))
        .unwrap();
    rig.bridge.render();
    rig.bridge
        .delete(session, std::slice::from_ref(&id))
        .unwrap();
    assert!(rig.bridge.apply(session, &[id]).is_err());
    assert!(rig.bridge.drawing("KBD"));
    assert!(rig.bridge.heartbeat(999).is_err());
}

#[test]
fn ears_mouse_zones_and_argb_have_the_correct_mapping_and_write_order() {
    let store = Persistence::memory().shared();
    let (ears, ear_state) = device(
        "EARS",
        "headset",
        json!({"matrix_dimensions":[1,2],"zone_order":["left","right"],"zone_colours":true}),
        store.clone(),
        false,
    );
    let (mouse, mouse_state) = device(
        "MOUSE",
        "mouse",
        json!({"matrix_dimensions":null,"zones":{"logo":{"effects":["static"]},"scroll":{"effects":["static"]}}}),
        store.clone(),
        false,
    );
    let (argb, argb_state) = device(
        "ARGB",
        "accessory",
        json!({"matrix_dimensions":[1,80],"custom_frame":true}),
        store.clone(),
        true,
    );
    let bridge = ChromaBridge::new(
        Arc::new(move || vec![ears.clone(), mouse.clone(), argb.clone()]),
        None,
        Some(store.clone()),
        None,
    );
    let session = bridge.open(&json!({}));
    bridge
        .show(
            session,
            "headset",
            &json!({"effect":"CHROMA_CUSTOM","param":[255,0xff0000,0,0,0]}),
        )
        .unwrap();
    let mut mouse_grid = vec![vec![0u32; 7]; 9];
    mouse_grid[7][3] = 0x00ff00;
    mouse_grid[2][3] = 255;
    bridge
        .show(
            session,
            "mouse",
            &json!({"effect":"CHROMA_CUSTOM2","param":mouse_grid}),
        )
        .unwrap();
    bridge.show(session, "chromalink", &solid(255)).unwrap();
    bridge.render();
    assert_eq!(
        ear_state.lock().unwrap().calls[0].1[0],
        json!([[255, 0, 0], [0, 0, 255]])
    );
    let calls = mouse_state.lock().unwrap().calls.clone();
    assert!(calls
        .iter()
        .any(|(_, a, k)| k["zone"] == "logo" && *a == vec![json!(0), json!(255), json!(0)]));
    assert_eq!(
        argb_state
            .lock()
            .unwrap()
            .calls
            .iter()
            .map(|(m, _, _)| m.as_str())
            .collect::<Vec<_>>(),
        ["set_custom", "set_key_row"]
    );
    mouse_grid[2][3] = 0x010203;
    bridge
        .show(
            session,
            "mouse",
            &json!({"effect":"CHROMA_CUSTOM2","param":mouse_grid}),
        )
        .unwrap();
    bridge
        .show(session, "chromalink", &solid(0x00ff00))
        .unwrap();
    bridge.render();
    assert_eq!(mouse_state.lock().unwrap().calls.len(), 3);
    assert_eq!(mouse_state.lock().unwrap().calls[2].2["zone"], "scroll");
    assert_eq!(
        argb_state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(m, _, _)| m == "set_custom")
            .count(),
        1
    );
    assert!(persistence::lock(&store)
        .get_device("EARS", "zone_colours", Value::Null)
        .is_null());
}

#[test]
fn a_failed_or_replaced_device_cannot_leave_lighting_held() {
    let rig = Rig::new();
    let session = rig.bridge.open(&json!({}));
    rig.bridge.show(session, "keyboard", &solid(255)).unwrap();
    rig.bridge.render();
    let (replacement, state) = device("KBD", "keyboard", keyboard_caps(), rig.store.clone(), false);
    *rig.devices.lock().unwrap() = vec![replacement];
    rig.bridge.render();
    assert!(!state.lock().unwrap().calls.is_empty());
    assert_eq!(rig.state.lock().unwrap().restored, 1);
    state.lock().unwrap().fail = true;
    rig.bridge
        .show(session, "keyboard", &solid(0x00ff00))
        .unwrap();
    rig.bridge.render();
    assert!(!rig.bridge.drawing("KBD"));
    assert!(!rig.bridge.errors().is_empty());
    persistence::lock(&rig.store).set("KBD", "backlight", "effect", json!("none"));
    assert_eq!(
        persistence::lock(&rig.store).get("KBD", "backlight", "effect"),
        "none"
    );
}

#[test]
fn the_worker_restores_devices_and_stops_without_waiting_for_its_idle_timer() {
    let rig = Rig::new();
    rig.bridge.start().unwrap();
    let session = rig.bridge.open(&json!({}));
    rig.bridge.show(session, "keyboard", &solid(255)).unwrap();
    until(|| rig.bridge.drawing("KBD"));
    let start = Instant::now();
    rig.bridge.stop();
    assert!(start.elapsed() < Duration::from_millis(500));
    assert!(!rig.bridge.drawing("KBD"));
    assert_eq!(rig.state.lock().unwrap().restored, 1);
    rig.bridge.start().unwrap();
    rig.bridge.stop();
}

struct Client(BufReader<TcpStream>);
impl Client {
    fn connect(address: SocketAddr) -> Self {
        let stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        Self(BufReader::new(stream))
    }
    fn receive(&mut self) -> (u16, BTreeHeaders, Value) {
        let mut line = String::new();
        self.0.read_line(&mut line).unwrap();
        let status = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        let mut headers = BTreeHeaders::new();
        loop {
            line.clear();
            self.0.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            let (key, value) = line.trim_end().split_once(':').unwrap();
            headers.insert(key.to_ascii_lowercase(), value.trim().into());
        }
        let length = headers
            .get("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut bytes = vec![0; length];
        self.0.read_exact(&mut bytes).unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, headers, body)
    }
    fn send(
        &mut self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, BTreeHeaders, Value) {
        let body = body
            .map(|b| serde_json::to_string(&b).unwrap())
            .unwrap_or_default();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        self.0.get_mut().write_all(request.as_bytes()).unwrap();
        self.receive()
    }
}
type BTreeHeaders = std::collections::BTreeMap<String, String>;

#[test]
fn rest_supports_the_full_session_api_on_a_persistent_connection() {
    let rig = Rig::new();
    let mut server = ChromaServer::start(rig.bridge.clone(), 0).unwrap();
    assert!(server.address().ip().is_loopback());
    let mut client = Client::connect(server.address());
    assert_eq!(
        client.send("GET", "/razer/chromasdk", None).2["version"],
        "3.36.1"
    );
    let (code, headers, _) = client.send("OPTIONS", "/razer/chromasdk", None);
    assert_eq!(code, 204);
    assert_eq!(headers["access-control-allow-origin"], "*");
    let opened = client
        .send(
            "POST",
            "/razer/chromasdk/",
            Some(json!({"title":"中文游戏"})),
        )
        .2;
    let path = format!("/sessions/{}/chromasdk", opened["sessionid"]);
    assert_eq!(
        client.send("PUT", &format!("{path}/heartbeat"), None).2["tick"],
        1
    );
    assert_eq!(
        client
            .send(
                "PUT",
                &format!("{path}/keyboard?ignored=yes"),
                Some(solid(255))
            )
            .0,
        200
    );
    let id = client
        .send(
            "POST",
            &format!("{path}/keyboard"),
            Some(json!({"effect":"CHROMA_NONE"})),
        )
        .2["id"]
        .clone();
    assert_eq!(
        client
            .send("PUT", &format!("{path}/effect"), Some(json!({"id":id})))
            .2["result"],
        0
    );
    assert_eq!(
        client
            .send(
                "DELETE",
                &format!("{path}/effect"),
                Some(json!({"ids":[id]}))
            )
            .0,
        200
    );
    assert_eq!(
        client
            .send("PUT", &format!("{path}/effect"), Some(json!({"id":id})))
            .0,
        400
    );
    assert_eq!(
        client
            .send("PUT", "/sessions/999/chromasdk/keyboard", Some(solid(255)))
            .0,
        404
    );
    assert_eq!(
        client
            .send(
                "PUT",
                &format!("{path}/keyboard"),
                Some(json!({"effect":"CHROMA_WAVE"}))
            )
            .0,
        400
    );
    assert_eq!(
        client
            .send("PUT", &format!("{path}/effect"), Some(json!(["invalid"])))
            .0,
        400
    );
    assert_eq!(client.send("DELETE", &path, None).0, 200);
    assert_eq!(rig.bridge.status()["apps"], json!([]));
    let start = Instant::now();
    server.stop();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!server.listening());
}

#[test]
fn malformed_http_is_bounded_and_cannot_desynchronise_a_connection() {
    let rig = Rig::new();
    let mut server = ChromaServer::start(rig.bridge.clone(), 0).unwrap();
    for headers in [
        "Content-Length: -1\r\n",
        "Content-Length: 999999999\r\n",
        "Content-Length: 2\r\nContent-Length: 2\r\n",
        "Transfer-Encoding: chunked\r\n",
    ] {
        let mut client = Client::connect(server.address());
        client.0.get_mut().write_all(format!("POST /razer/chromasdk HTTP/1.1\r\n{headers}\r\n{{}}GET /razer/chromasdk HTTP/1.1\r\n\r\n").as_bytes()).unwrap();
        let answer = client.receive();
        assert_eq!(answer.0, 400);
        assert_eq!(answer.1["connection"], "close");
    }
    assert_eq!(rig.bridge.status()["apps"], json!([]));
    let mut partial = Client::connect(server.address());
    partial
        .0
        .get_mut()
        .write_all(b"POST /razer/chromasdk HTTP/1.1\r\nContent-Length: 100\r\n\r\n{")
        .unwrap();
    let start = Instant::now();
    server.stop();
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn expect_continue_and_pipelined_requests_work() {
    let rig = Rig::new();
    let server = ChromaServer::start(rig.bridge.clone(), 0).unwrap();
    let mut client = Client::connect(server.address());
    client
        .0
        .get_mut()
        .write_all(
            b"POST /razer/chromasdk HTTP/1.1\r\nContent-Length: 2\r\nExpect: 100-continue\r\n\r\n",
        )
        .unwrap();
    assert_eq!(client.receive().0, 100);
    client
        .0
        .get_mut()
        .write_all(b"{}GET /razer/chromasdk HTTP/1.1\r\n\r\n")
        .unwrap();
    assert_eq!(client.receive().0, 200);
    assert_eq!(client.receive().2["version"], "3.36.1");
}

#[test]
fn a_busy_port_is_not_shared() {
    let rig = Rig::new();
    let server = ChromaServer::start(rig.bridge.clone(), 0).unwrap();
    assert!(ChromaServer::start(rig.bridge.clone(), server.address().port()).is_err());
    assert!(std::net::TcpListener::bind(server.address()).is_err());
}

#[test]
fn the_existing_chroma_sdk_game_calls_reach_the_native_backend() {
    use rzchromasdk::*;
    struct Cleanup(Option<std::ffi::OsString>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            UnInit();
            if let Some(value) = &self.0 {
                std::env::set_var("OPENRAZER_CHROMA_PORT", value);
            } else {
                std::env::remove_var("OPENRAZER_CHROMA_PORT");
            }
        }
    }
    let rig = Rig::new();
    rig.bridge.start().unwrap();
    let server = ChromaServer::start(rig.bridge.clone(), 0).unwrap();
    let _cleanup = Cleanup(std::env::var_os("OPENRAZER_CHROMA_PORT"));
    std::env::set_var("OPENRAZER_CHROMA_PORT", server.address().port().to_string());
    let mut info = AppInfo {
        title: [0; 256],
        description: [0; 1024],
        author_name: [0; 256],
        author_contact: [0; 256],
        supported_device: 0x3f,
        category: 2,
    };
    let title: Vec<_> = "Native Rust Game".encode_utf16().collect();
    info.title[..title.len()].copy_from_slice(&title);
    assert_eq!(unsafe { InitSDK(&info) }, 0);
    let colour = 255u32;
    let mut effect = Guid::default();
    assert_eq!(
        unsafe { CreateKeyboardEffect(4, (&colour as *const u32).cast(), &mut effect) },
        0
    );
    assert_eq!(SetEffect(effect), 0);
    until(|| rig.bridge.drawing("KBD"));
    assert_eq!(rig.bridge.status()["showing"], "Native Rust Game");
    until(|| {
        rig.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(m, _, _)| m == "set_key_row")
    });
    assert_eq!(DeleteEffect(effect), 0);
    assert_eq!(SetEffect(effect), 1168);
    assert_eq!(UnInit(), 0);
    until(|| !rig.bridge.drawing("KBD"));
    assert_eq!(rig.state.lock().unwrap().restored, 1);
}

#[test]
fn custom_key_overlay_extended_border_and_even_sampling_are_preserved() {
    let mut extended = vec![vec![0u32; 24]; 8];
    extended[1][1] = 255;
    let mut keys = vec![vec![0u32; 22]; 6];
    keys[3][2] = 0x0100ff00;
    let grid = grid::parse(
        "keyboard",
        &json!({"effect":"CHROMA_CUSTOM2","param":{"color":extended,"key":keys}}),
    )
    .unwrap();
    assert_eq!(grid[0][0], [255, 0, 0]);
    assert_eq!(grid[3][2], [0, 255, 0]);
    assert_eq!(
        grid::resample(&[[1, 0, 0], [2, 0, 0], [3, 0, 0]], 5),
        [[1, 0, 0], [1, 0, 0], [2, 0, 0], [3, 0, 0], [3, 0, 0]]
    );
    assert!(grid::parse(
        "keyboard",
        &json!({"effect":"CHROMA_CUSTOM","param":[vec![0;33]]})
    )
    .is_err());
}

#[test]
fn session_requests_and_status_do_not_wait_for_a_slow_device_write() {
    let rig = Rig::new();
    let (entered, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    rig.state.lock().unwrap().pause = Some((entered, wait));
    let session = rig.bridge.open(&json!({}));
    rig.bridge.show(session, "keyboard", &solid(255)).unwrap();
    let bridge = rig.bridge.clone();
    let worker = std::thread::spawn(move || bridge.render());
    ready.recv_timeout(Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    assert!(rig.bridge.drawing("KBD"));
    assert_eq!(rig.bridge.heartbeat(session).unwrap(), 1);
    rig.bridge
        .show(session, "keyboard", &solid(0xff0000))
        .unwrap();
    assert_eq!(rig.bridge.status()["apps"], json!(["A Chroma app"]));
    assert!(start.elapsed() < Duration::from_millis(500));
    release.send(()).unwrap();
    worker.join().unwrap();
    rig.bridge.render();
    assert_eq!(
        rig.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(m, _, _)| m == "set_key_row")
            .count(),
        2
    );
}

#[test]
fn fragmented_requests_and_idle_keep_alive_connections_remain_usable() {
    let rig = Rig::new();
    let server = ChromaServer::start(rig.bridge.clone(), 0).unwrap();
    let mut client = Client::connect(server.address());
    client.0.get_mut().write_all(b"POST /razer/chro").unwrap();
    std::thread::sleep(Duration::from_millis(150));
    client
        .0
        .get_mut()
        .write_all(b"masdk HTTP/1.1\r\nContent-Length: 2\r\n\r\n{")
        .unwrap();
    std::thread::sleep(Duration::from_millis(150));
    client.0.get_mut().write_all(b"}").unwrap();
    let opened = client.receive();
    assert_eq!(opened.0, 200);
    std::thread::sleep(Duration::from_millis(250));
    let path = format!("/sessions/{}/chromasdk/heartbeat", opened.2["sessionid"]);
    assert_eq!(client.send("PUT", &path, None).2["tick"], 1);
    assert_eq!(client.send("PUT", &path, None).2["tick"], 2);
}
