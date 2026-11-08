#![cfg(windows)]
use rsrazer_core::rpc::Client as StudioClient;
use rsrazer_daemon::rpc::{Client, Endpoint};
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Session {
    directory: PathBuf,
    child: Option<Child>,
}
impl Session {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "openrazer-native-exe-{} 中文 config",
            rsrazer_daemon::rpc::new_token().unwrap()
        ));
        Self {
            directory,
            child: None,
        }
    }
    fn command(&self, args: &[&str]) -> Command {
        let executable = std::env::var_os("OPENRAZER_TEST_DAEMON")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_rsrazer-daemon")));
        let mut cmd = Command::new(executable);
        cmd.creation_flags(0x08000000)
            .args(args)
            .arg("--emulate")
            .arg("--config-dir")
            .arg(&self.directory);
        cmd
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn endpoint(&self) -> PathBuf {
        self.directory.join("daemon.json")
    }
    fn start(&mut self) {
        self.child = Some(
            self.command(&["daemon", "run"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
                let mut errors = String::new();
                if let Some(stderr) = self.child.as_mut().unwrap().stderr.as_mut() {
                    stderr.read_to_string(&mut errors).unwrap();
                }
                panic!("native daemon exited {status}: {errors}");
            }
            if let Ok(mut client) = Client::connect(&self.endpoint()) {
                if client
                    .call("daemon.version", json!({}))
                    .is_ok_and(|v| v["state"] == "ready")
                {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "native daemon did not become ready; log: {}",
                fs::read_to_string(self.directory.join("daemon.log")).unwrap_or_default()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
    fn client(&self) -> StudioClient {
        let endpoint = Endpoint::read(&self.endpoint()).unwrap();
        StudioClient::connect_to(
            serde_json::from_value(serde_json::to_value(endpoint).unwrap()).unwrap(),
        )
        .unwrap()
    }
    fn stop(&mut self) {
        if self.child.is_none() {
            return;
        }
        let reply = self.client().call("daemon.shutdown", json!({})).unwrap();
        assert_eq!(reply, json!({"stopping":true}));
        self.wait_exit();
    }
    fn wait_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "native daemon did not exit");
            thread::sleep(Duration::from_millis(25));
        }
        self.child.take();
        assert!(!self.endpoint().exists());
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if self.child.is_none() && self.endpoint().exists() {
            if let Ok(mut client) = Client::connect(&self.endpoint()) {
                let _ = client.call("daemon.shutdown", json!({}));
                let deadline = Instant::now() + Duration::from_secs(5);
                while self.endpoint().exists() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(25));
                }
            }
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Remove only the unique test directory created under the OS temp path.
        if self.directory.is_absolute()
            && self.directory.starts_with(std::env::temp_dir())
            && self
                .directory
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("openrazer-native-exe-")
        {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}
fn ok(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn native_cli_controls_devices_and_keeps_profiles_macros_and_integrations_compatible() {
    let mut session = Session::new();
    session.start();
    let devices = ok(session.run(&["list", "--json"]));
    assert_eq!(
        devices.as_array().unwrap().len(),
        rsrazer_daemon::demo::PRODUCT_IDS.len()
    );
    assert!(devices
        .as_array()
        .unwrap()
        .iter()
        .all(|d| d["pid"].as_str().is_some_and(|p| p.len() == 4)));
    assert!(success(session.run(&["list"])).contains("BlackWidow"));
    let info = ok(session.run(&["info", "0078", "--json"]));
    assert_eq!(info.as_array().unwrap().len(), 1);
    assert_eq!(info[0]["capabilities"]["dpi"], true);
    assert_eq!(
        ok(session.run(&["dpi", "2400", "3200", "-d", "0078", "--json"]))[0]["dpi"],
        json!([2400, 3200])
    );
    assert_eq!(
        ok(session.run(&["poll-rate", "500", "-d", "0078", "--json"]))[0]["poll_rate"],
        500
    );
    let brightness = ok(session.run(&["brightness", "40", "-d", "0203", "--json"]))[0]
        ["brightness"]
        .as_f64()
        .unwrap();
    assert!((brightness - 40.0).abs() < 1.0);
    success(session.run(&["effect", "static", "#f08", "-d", "0203", "--quiet"]));
    success(session.run(&["profile", "save", "夜间", "-d", "0203"]));
    success(session.run(&["profile", "save", "白天", "-d", "0203"]));
    success(session.run(&["effect", "static", "blue", "-d", "0203"]));
    success(session.run(&["profile", "delete", "白天", "-d", "0203"]));
    success(session.run(&["profile", "load", "夜间", "-d", "0203"]));
    assert_eq!(
        ok(session.run(&["profile", "--json", "-d", "0203"]))[0]["profiles"][0]["name"],
        "夜间"
    );
    let mut client = session.client();
    let keyboard = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["pid"] == "0203")
        .unwrap()["serial"]
        .as_str()
        .unwrap();
    let saved = client.call("daemon.persistence", json!({})).unwrap();
    assert_eq!(saved[keyboard]["zones"]["backlight"]["effect"], "static");
    assert_eq!(
        &saved[keyboard]["zones"]["backlight"]["colors"]
            .as_array()
            .unwrap()[..3],
        &[json!(255), json!(0), json!(136)]
    );
    success(session.run(&["effect", "custom", "cyan", "-d", "0203"]));
    success(session.run(&["effect", "ripple", "green", "-d", "0203"]));
    assert_eq!(
        client.call("effect.status", json!({})).unwrap()["active"][keyboard],
        "ripple"
    );
    success(session.run(&["effect", "none", "-d", "0203"]));
    assert!(client.call("effect.status", json!({})).unwrap()["active"]
        .get(keyboard)
        .is_none());
    assert!(
        !ok(session.run(&["zones", "red", "-d", "0203", "--json"]))[0]["zones"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(!ok(session.run(&["battery", "--json"]))
        .as_array()
        .unwrap()
        .is_empty());
    success(session.run(&["headset", "sidetone", "8", "-d", "0565"]));
    assert_eq!(
        ok(session.run(&["keyswitch", "--json"]))[0]["keyswitch_optimization"],
        "typing"
    );
    success(session.run(&["keyswitch", "gaming", "-d", "026b"]));
    assert_eq!(
        ok(session.run(&["keyswitch", "--json", "-d", "026b"]))[0]["keyswitch_optimization"],
        "gaming"
    );
    assert!(!session
        .run(&["keyswitch", "fast", "-d", "026b"])
        .status
        .success());
    assert_eq!(
        ok(session.run(&["headset", "-d", "0565", "--json"]))[0]["audio"]["sidetone_level"],
        8
    );
    success(session.run(&["headset", "eq", "-3,0,0,0,0,0,0,2,3,3", "-d", "0565"]));
    assert_eq!(
        ok(session.run(&["headset", "-d", "0565", "--json"]))[0]["audio"]["equalizer_bands"],
        json!([-3, 0, 0, 0, 0, 0, 0, 2, 3, 3])
    );
    success(session.run(&["headset", "dnd", "on", "-d", "0565"]));
    success(session.run(&["headset", "sleep", "30", "-d", "0565"]));
    success(session.run(&["headset", "led", "battery", "-d", "0565"]));
    for binding in [
        vec!["--keys", "ctrl+shift+esc"],
        vec!["--text", "你好 Rust"],
        vec!["--launch", "C:\\Test Path\\演示.exe"],
        vec!["--disable"],
    ] {
        let mut args = vec!["macro", "set", "M1", "-d", "0203"];
        args.extend(binding);
        success(session.run(&args));
        success(session.run(&["macro", "play", "M1", "-d", "0203"]));
    }
    assert_eq!(
        ok(session.run(&["macro", "list", "--json", "-d", "0203"]))[0]["bindings"]["M1"]["type"],
        "disabled"
    );
    success(session.run(&["macro", "clear", "M1", "-d", "0203"]));
    assert!(
        ok(session.run(&["macro", "list", "--json", "-d", "0203"]))[0]["bindings"]
            .get("M1")
            .is_none()
    );
    let chroma = ok(session.run(&["integration", "chroma", "on", "--json"]));
    assert_eq!(chroma["chroma"]["listening"], true);
    assert!(chroma["chroma"]["port"].as_u64().unwrap() > 0);
    assert_eq!(
        ok(session.run(&["integration", "chroma", "off", "--json"]))["chroma"]["enabled"],
        false
    );
    success(session.run(&["profile", "delete", "夜间", "-d", "0203"]));
    assert!(
        ok(session.run(&["profile", "--json", "-d", "0203"]))[0]["profiles"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Direct access must not race the running daemon for the same devices/files.
    assert!(!session.run(&["--direct", "list"]).status.success());
    session.stop();
}

#[test]
fn native_cli_validates_before_io_and_direct_mode_needs_no_daemon() {
    let session = Session::new();
    for args in [
        vec!["effect"],
        vec!["effect", "unknown"],
        vec!["effect", "static", "中文"],
        vec!["effect", "static", "red", "--speed", "9"],
        vec!["dpi", "oops"],
        vec!["brightness", "NaN"],
        vec!["headset", "eq", "1,2"],
        vec!["integration", "chroma", "typo"],
        vec!["macro", "set", "M1", "--text", "x", "--disable"],
        vec!["macro", "clear"],
        vec!["profile", "load"],
        vec!["battery", "ignored"],
        vec!["list", "--no-keys"],
        vec!["autostart", "enable"],
    ] {
        let output = session.run(&args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !session.directory.exists(),
            "invalid command touched configuration: {args:?}"
        );
    }
    assert_eq!(
        ok(session.run(&["--direct", "list", "--json"]))
            .as_array()
            .unwrap()
            .len(),
        rsrazer_daemon::demo::PRODUCT_IDS.len()
    );
    assert!(!session.endpoint().exists());
    success(session.run(&["--direct", "effect", "static", "red", "-d", "0203"]));
    success(session.run(&["--direct", "profile", "save", "本地", "-d", "0203"]));
    assert_eq!(
        ok(session.run(&["--direct", "profile", "list", "-d", "0203", "--json"]))[0]["profiles"][0]
            ["name"],
        "本地"
    );
    success(session.run(&["--direct", "profile", "load", "本地", "-d", "0203"]));
    assert!(!session
        .run(&["--direct", "effect", "ripple", "-d", "0203"])
        .status
        .success());
    let diagnostic = ok(session.run(&["doctor", "--json"]));
    assert_eq!(diagnostic["backend"], "native Rust");
    assert!(diagnostic["hid"].as_array().unwrap().len() >= 4);
    assert_eq!(diagnostic["bluetooth"], json!([]));
    assert!(!session.endpoint().exists());
}

#[test]
fn snaptap_cli_requires_consent_and_persists_pairs_and_program_lists() {
    let mut session = Session::new();
    session.start();
    let snap = ok(session.run(&["snaptap", "--json"]));
    assert_eq!(snap["enabled"], false);
    assert_eq!(snap["deny"], json!(["cs2.exe"]));
    assert!(!session.run(&["snaptap", "on"]).status.success());
    success(session.run(&["snaptap", "on", "--accept"]));
    success(session.run(&["snaptap", "pair", "add", "w", "s"]));
    assert!(!session
        .run(&["snaptap", "pair", "add", "a", "f"])
        .status
        .success());
    success(session.run(&["snaptap", "deny", "add", "Game.EXE"]));
    success(session.run(&["snaptap", "allow", "add", "another.exe"]));
    session.stop();
    session.start();
    let snap = ok(session.run(&["snaptap", "status", "--json"]));
    assert_eq!(
        snap["pairs"],
        json!([["KEY_A", "KEY_D"], ["KEY_W", "KEY_S"]])
    );
    assert_eq!(snap["deny"], json!(["cs2.exe", "game.exe"]));
    assert_eq!(snap["allow"], json!(["another.exe"]));
    assert_eq!(snap["acknowledged"], true);
    assert_eq!(snap["enabled"], true);
    // The emulator never installs a native input filter or emits user input.
    assert_eq!(snap["running"], false);
    success(session.run(&["snaptap", "off"]));
    success(session.run(&["snaptap", "pair", "remove", "s", "w"]));
    success(session.run(&["snaptap", "deny", "remove", "game.exe"]));
    success(session.run(&["snaptap", "allow", "remove", "another.exe"]));
    let snap = ok(session.run(&["snaptap", "--json"]));
    assert_eq!(snap["pairs"], json!([["KEY_A", "KEY_D"]]));
    assert_eq!(snap["deny"], json!(["cs2.exe"]));
    assert_eq!(snap["allow"], json!([]));
    assert_eq!(snap["enabled"], false);
    session.stop();
}

#[test]
fn native_executable_serves_studio_with_devices_profiles_macros_and_restart_persistence() {
    let mut session = Session::new();
    session.start();
    let mut client = session.client();
    let version = client.call("daemon.version", json!({})).unwrap();
    assert_eq!(version["backend"], "fake-rust");
    assert_eq!(version["devices"], rsrazer_daemon::demo::PRODUCT_IDS.len());
    assert_eq!(version["pid"], session.child.as_ref().unwrap().id());
    let devices = client.call("devices.list", json!({})).unwrap();
    assert_eq!(
        devices.as_array().unwrap().len(),
        rsrazer_daemon::demo::PRODUCT_IDS.len()
    );
    assert!(devices
        .as_array()
        .unwrap()
        .iter()
        .all(|d| d["serial"].as_str().unwrap().starts_with("DEMO")));
    let serial = |kind: &str| {
        devices
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["type"] == kind)
            .unwrap()["serial"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let keyboard = serial("keyboard");
    let mouse = serial("mouse");
    let headset = serial("headset");
    client
        .device(&keyboard, "set_brightness", vec![json!(40)], None)
        .unwrap();
    let brightness = client
        .device(&keyboard, "get_brightness", vec![], None)
        .unwrap()
        .as_f64()
        .unwrap();
    assert!((brightness - 40.0).abs() < 0.5);
    client
        .device(&mouse, "set_dpi", vec![json!(2400), json!(3200)], None)
        .unwrap();
    assert_eq!(
        client.device(&mouse, "get_dpi", vec![], None).unwrap(),
        json!([2400, 3200])
    );
    client
        .device(&mouse, "set_poll_rate", vec![json!(500)], None)
        .unwrap();
    assert_eq!(
        client
            .device(&mouse, "get_poll_rate", vec![], None)
            .unwrap(),
        500
    );
    assert_eq!(
        client
            .device(&headset, "get_battery_level", vec![], None)
            .unwrap(),
        85
    );
    client
        .device(
            &headset,
            "set_equalizer_bands",
            vec![json!([-3, 0, 0, 0, 0, 0, 0, 2, 3, 3])],
            None,
        )
        .unwrap();
    let sound = client.device(&headset, "get_audio", vec![], None).unwrap();
    assert!(sound.is_object());
    client
        .call("profile.save", json!({"serial":keyboard,"name":"夜间配置"}))
        .unwrap();
    client
        .call(
            "macro.set",
            json!({"serial":keyboard,"key":"M1","action":{"type":"text","text":"测试文字😀"}}),
        )
        .unwrap();
    assert_eq!(
        client.call("keys.status", json!({})).unwrap()["running"],
        true
    );
    // In emulation, playback/launches exercise the scheduler but never send
    // input to the user's desktop or open a real program.
    client
        .call("macro.play", json!({"serial":keyboard,"key":"M1"}))
        .unwrap();
    client
        .call(
            "effect.set",
            json!({"serial":keyboard,"effect":"wave_soft"}),
        )
        .unwrap();
    assert_eq!(
        client.call("effect.status", json!({})).unwrap()["active"][&keyboard],
        "wave_soft"
    );
    drop(client);
    session.stop();
    assert!(session.directory.join("persistence.json").is_file());
    assert!(session.directory.join("profiles.json").is_file());
    assert!(session.directory.join("macros.json").is_file());
    let log = fs::read_to_string(session.directory.join("daemon.log")).unwrap();
    assert!(log.contains("(Rust) listening"));
    assert!(log.contains("stopped"));
    session.start();
    let mut client = session.client();
    assert_eq!(
        client
            .call("profile.list", json!({"serial":keyboard}))
            .unwrap()[0]["name"],
        "夜间配置"
    );
    assert_eq!(
        client
            .call("macro.list", json!({"serial":keyboard}))
            .unwrap()["bindings"]["M1"]["text"],
        "测试文字😀"
    );
    assert_eq!(
        client.call("effect.status", json!({})).unwrap()["active"][&keyboard],
        "wave_soft"
    );
    // Real mice retain DPI themselves. Like the original backend, restore()
    // restores lighting; a fresh emulator has new virtual hardware registers.
    assert_eq!(
        client.call("daemon.persistence", json!({})).unwrap()[&mouse]["dpi"],
        json!([2400, 3200])
    );
    let brightness = client
        .device(&keyboard, "get_brightness", vec![], None)
        .unwrap()
        .as_f64()
        .unwrap();
    assert!((brightness - 40.0).abs() < 0.5);
    session.stop();
}

#[test]
fn duplicate_process_leaves_the_live_daemon_and_endpoint_intact() {
    let mut session = Session::new();
    session.start();
    let before = Endpoint::read(&session.endpoint()).unwrap();
    let result = session.run(&["daemon", "run"]);
    assert!(result.status.success());
    assert_eq!(Endpoint::read(&session.endpoint()).unwrap(), before);
    assert!(session
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    assert_eq!(ok(session.run(&["daemon", "status"]))["pid"], before.pid);
    assert_eq!(
        ok(session.run(&["rpc", "devices.list"]))
            .as_array()
            .unwrap()
            .len(),
        rsrazer_daemon::demo::PRODUCT_IDS.len()
    );
    session.stop();
}

#[test]
fn invalid_bind_is_rejected_before_writing_configuration_and_occupied_port_is_reported() {
    let invalid = Session::new();
    let result = invalid.run(&["daemon", "run", "--host", "0.0.0.0"]);
    assert!(!result.status.success());
    assert!(!invalid.directory.exists());
    let mut first = Session::new();
    first.start();
    let port = Endpoint::read(&first.endpoint()).unwrap().port.to_string();
    let other = Session::new();
    let result = other.run(&["daemon", "run", "--port", &port]);
    assert!(!result.status.success());
    assert!(!other.endpoint().exists());
    assert!(other.directory.join("daemon.log").is_file());
    assert_eq!(
        first.client().call("daemon.version", json!({})).unwrap()["devices"],
        rsrazer_daemon::demo::PRODUCT_IDS.len()
    );
    first.stop();
}

#[test]
fn optional_engines_remain_disabled_and_chroma_uses_a_separate_ephemeral_port() {
    let mut session = Session::new();
    // Start the process through the background-launch command used by CLI users.
    let output = session.run(&["daemon", "start", "--no-effects", "--no-keys"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut client = session.client();
    assert_eq!(
        client.call("effect.status", json!({})).unwrap()["enabled"],
        false
    );
    assert_eq!(
        client.call("keys.status", json!({})).unwrap()["running"],
        false
    );
    let enabled = client
        .call("integration.set", json!({"name":"chroma","enabled":true}))
        .unwrap();
    assert_eq!(enabled["chroma"]["listening"], true);
    assert!(enabled["chroma"]["port"].as_u64().unwrap() > 0);
    assert_ne!(
        enabled["chroma"]["port"],
        Endpoint::read(&session.endpoint()).unwrap().port
    );
    assert_eq!(
        client
            .call("integration.set", json!({"name":"chroma","enabled":false}))
            .unwrap()["chroma"]["listening"],
        false
    );
    drop(client);
    let stopped = session.run(&["daemon", "stop"]);
    assert!(stopped.status.success());
    assert!(!session.endpoint().exists());
    assert_eq!(session.run(&["daemon", "status"]).status.code(), Some(1));
    // No child handle is owned in this case; daemon stop above must finish it.
    session.child = None;
}

#[test]
fn onboard_cli_and_rpc_map_buttons_upload_and_guard_macro_deletion() {
    let mut session = Session::new();
    session.start();
    let status = ok(session.run(&["onboard", "--json", "-d", "00a7"]));
    assert_eq!(status[0]["side_plate"], 4);
    assert_eq!(status[0]["macros"]["count"], 0);
    success(session.run(&[
        "onboard",
        "key",
        "0x50",
        "A",
        "--modifiers",
        "1",
        "--store",
        "5",
        "--layer",
        "1",
        "-d",
        "00a7",
    ]));
    assert_eq!(
        ok(session.run(&[
            "onboard", "action", "0x50", "--store", "5", "--layer", "1", "--json", "-d", "00a7"
        ]))[0]["result"],
        json!([2, 2, 1, 4])
    );
    success(session.run(&[
        "onboard",
        "macro-add",
        "7",
        "down A; wait 300; up A",
        "-d",
        "00a7",
    ]));
    assert_eq!(
        ok(session.run(&["onboard", "macro-length", "7", "--json", "-d", "00a7"]))[0]["result"],
        7
    );
    success(session.run(&[
        "onboard",
        "macro-bind",
        "0x55",
        "7",
        "--store",
        "5",
        "--layer",
        "1",
        "--while-held",
        "-d",
        "00a7",
    ]));
    let refused = session.run(&["onboard", "macro-delete", "7", "-d", "00a7"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8(refused.stderr)
        .unwrap()
        .contains("still bound"));
    success(session.run(&[
        "onboard", "mouse", "0x55", "4", "--store", "5", "--layer", "1", "-d", "00a7",
    ]));
    success(session.run(&["onboard", "macro-delete", "7", "-d", "00a7"]));
    assert_eq!(
        ok(session.run(&["onboard", "macros", "--json", "-d", "00a7"]))[0]["result"]["count"],
        0
    );
    assert!(!session
        .run(&["onboard", "key", "0x50", "A", "-d", "0078"])
        .status
        .success());
    assert!(!session
        .run(&["onboard", "macro-add", "8", "down NOT_A_KEY", "-d", "00a7"])
        .status
        .success());
    assert!(!session
        .run(&["onboard", "status", "--while-held", "-d", "00a7"])
        .status
        .success());
    // Named keys and textual steps are also accepted directly over RPC.
    let mut client = session.client();
    client
        .call(
            "device.call",
            json!({"serial":"00a7","method":"set_button_key","args":[0x50,"F5"]}),
        )
        .unwrap();
    assert_eq!(
        client
            .call(
                "device.call",
                json!({"serial":"00a7","method":"get_button_action","args":[0x50]})
            )
            .unwrap(),
        json!([2, 2, 0, 62])
    );
    session.stop();
    success(session.run(&["--direct", "onboard", "key", "0x50", "B", "-d", "00a7"]));
}

#[test]
fn profile_cli_creates_copies_marks_active_and_keeps_edits_across_restart() {
    let mut session = Session::new();
    session.start();
    success(session.run(&["profile", "create", "桌面", "-d", "0078"]));
    success(session.run(&["dpi", "800", "-d", "0078"]));
    success(session.run(&[
        "profile", "create", "游戏", "--source", "桌面", "-d", "0078",
    ]));
    success(session.run(&["dpi", "1600", "-d", "0078"]));
    let output = session.run(&["profile", "list", "-d", "0078"]);
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .any(|line| line == "  * 游戏"));
    session.stop();
    session.start();
    let profiles = ok(session.run(&["profile", "--json", "-d", "0078"]));
    assert_eq!(profiles[0]["profiles"][1]["active"], true);
    success(session.run(&["profile", "load", "桌面", "-d", "0078"]));
    assert_eq!(
        ok(session.run(&["dpi", "--json", "-d", "0078"]))[0]["dpi"],
        json!([800, 800])
    );
    assert!(!session
        .run(&["profile", "create", "游戏", "-d", "0078"])
        .status
        .success());
    session.stop();
    // Direct mode follows the same on-disk active profile without starting hooks.
    success(session.run(&[
        "--direct", "profile", "create", "副本", "--source", "游戏", "-d", "0078",
    ]));
    success(session.run(&["--direct", "dpi", "2000", "-d", "0078"]));
    let profiles: Value =
        serde_json::from_slice(&fs::read(session.directory.join("profiles.json")).unwrap())
            .unwrap();
    let entries = profiles
        .as_object()
        .unwrap()
        .values()
        .find(|v| v.as_array().is_some_and(|a| a.len() == 3))
        .unwrap();
    assert_eq!(entries[2]["active"], true);
    assert_eq!(entries[2]["device"]["dpi"], json!([2000, 2000]));
    assert_eq!(entries[1]["device"]["dpi"], json!([1600, 1600]));
}
