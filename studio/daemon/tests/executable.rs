#![cfg(windows)]
use openrazer_daemon::rpc::{Client, Endpoint};
use openrazer_studio_core::rpc::Client as StudioClient;
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
            openrazer_daemon::rpc::new_token().unwrap()
        ));
        Self {
            directory,
            child: None,
        }
    }
    fn command(&self, args: &[&str]) -> Command {
        let executable = std::env::var_os("OPENRAZER_TEST_DAEMON")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_openrazer-win")));
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

#[test]
fn native_executable_serves_studio_with_devices_profiles_macros_and_restart_persistence() {
    let mut session = Session::new();
    session.start();
    let mut client = session.client();
    let version = client.call("daemon.version", json!({})).unwrap();
    assert_eq!(version["backend"], "fake-rust");
    assert_eq!(version["devices"], 4);
    assert_eq!(version["pid"], session.child.as_ref().unwrap().id());
    let devices = client.call("devices.list", json!({})).unwrap();
    assert_eq!(devices.as_array().unwrap().len(), 4);
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
        4
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
        4
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
