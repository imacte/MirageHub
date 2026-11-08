//! Wire expectations from openrazer-win #14 tests/test_keyswitch.py at
//! 9fe3e5dba5899e0eb6e58beda0b4f732a92210af. No physical device is opened.
use rsrazer_daemon::{
    database,
    demo::DemoBackend,
    device::{Device, StandardDevice},
    hid::{HidBackend, HidHandle, HidInfo, InputReader},
    persistence::{self, Persistence},
    protocol::report::Report,
    transport::Transport,
    Error, Result,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Trace {
    sent: Vec<Report>,
    fail: Option<u8>,
    marker: Option<u8>,
}
struct Recording {
    demo: DemoBackend,
    trace: Arc<Mutex<Trace>>,
}
struct Handle {
    inner: Box<dyn HidHandle>,
    trace: Arc<Mutex<Trace>>,
}
impl HidBackend for Recording {
    fn enumerate(&self, vid: Option<u16>, pid: Option<u16>) -> Result<Vec<HidInfo>> {
        self.demo.enumerate(vid, pid)
    }
    fn open(&self, info: &HidInfo) -> Result<Box<dyn HidHandle>> {
        Ok(Box::new(Handle {
            inner: self.demo.open(info)?,
            trace: self.trace.clone(),
        }))
    }
    fn open_input(&self, info: &HidInfo) -> Result<Box<dyn InputReader>> {
        self.demo.open_input(info)
    }
}
impl HidHandle for Handle {
    fn send_feature(&mut self, data: &[u8]) -> Result<()> {
        let request = Report::unpack(&data[1..])?;
        if request.command_class == 2 {
            let mut trace = self.trace.lock().unwrap();
            trace.sent.push(request.clone());
            if trace.fail == Some(request.command_id) {
                return Err(Error::NotSupported("emulated refusal".into()));
            }
        }
        self.inner.send_feature(data)
    }
    fn get_feature(&mut self, length: usize, id: u8) -> Result<Vec<u8>> {
        let mut data = self.inner.get_feature(length, id)?;
        let mut response = Report::unpack(&data[1..])?;
        if response.command_class == 2 && response.command_id == 0x82 {
            if let Some(marker) = self.trace.lock().unwrap().marker {
                response.arguments[1] = marker;
                data[1..].copy_from_slice(&response.pack());
            }
        }
        Ok(data)
    }
    fn send_output(&mut self, data: &[u8]) -> Result<()> {
        self.inner.send_output(data)
    }
}
fn device(pid: u16) -> (StandardDevice, Arc<Mutex<Trace>>) {
    let trace = Arc::new(Mutex::new(Trace::default()));
    let backend = Arc::new(Recording {
        demo: DemoBackend::with_product_ids(&[pid]).unwrap(),
        trace: trace.clone(),
    });
    let info = backend
        .enumerate(Some(0x1532), Some(pid))
        .unwrap()
        .remove(0);
    let mut device = StandardDevice::new(
        database::lookup(0x1532, pid).unwrap().clone(),
        Box::new(Transport::new(backend, info, 0, None, None)),
        Persistence::memory().shared(),
    );
    device.serial();
    (device, trace)
}
fn call(device: &mut StandardDevice, method: &str, args: &[Value]) -> Result<Value> {
    device.call(method, args, &Default::default())
}

#[test]
fn both_huntsmans_use_the_captured_two_command_sequence_and_read_back_modes() {
    for pid in [0x026b, 0x026c] {
        let (mut d, trace) = device(pid);
        assert_eq!(d.capabilities()["keyswitch_optimization"], true);
        assert_eq!(
            call(&mut d, "get_keyswitch_optimization", &[]).unwrap(),
            "typing"
        );
        for (mode, marker, interval) in [("gaming", 0, 0), ("typing", 20, 40)] {
            trace.lock().unwrap().sent.clear();
            call(
                &mut d,
                "set_keyswitch_optimization",
                &[json!(mode.to_ascii_uppercase())],
            )
            .unwrap();
            {
                let trace = trace.lock().unwrap();
                assert_eq!(trace.sent.len(), 2);
                let (first, second) = (&trace.sent[0], &trace.sent[1]);
                assert_eq!(
                    (first.command_id, first.data_size, first.transaction_id),
                    (2, 4, 0x1f)
                );
                assert_eq!(
                    (second.command_id, second.data_size, second.transaction_id),
                    (0x15, 5, 0x1f)
                );
                assert_eq!(&first.arguments[..5], &[0, marker, 0, interval, 0]);
                assert_eq!(&second.arguments[..6], &[1, 0, marker, 0, interval, 0]);
            }
            assert_eq!(
                call(&mut d, "get_keyswitch_optimization", &[]).unwrap(),
                mode
            );
            let read = trace.lock().unwrap().sent.last().unwrap().clone();
            assert_eq!(
                (read.command_id, read.data_size, read.transaction_id),
                (0x82, 4, 0x1f)
            );
            let serial = d.serial();
            assert_eq!(
                persistence::lock(d.persistence()).get_device(
                    &serial,
                    "keyswitch_optimization",
                    Value::Null
                ),
                mode
            );
        }
        trace.lock().unwrap().marker = Some(5);
        assert_eq!(
            call(&mut d, "get_keyswitch_optimization", &[]).unwrap(),
            "gaming"
        );
        trace.lock().unwrap().sent.clear();
        d.restore().unwrap();
        assert!(
            trace.lock().unwrap().sent.is_empty(),
            "lighting restore must leave firmware mode alone"
        );
    }
}

#[test]
fn invalid_modes_and_unsupported_devices_never_write_and_failures_never_save() {
    let (mut d, trace) = device(0x026b);
    call(&mut d, "set_keyswitch_optimization", &[json!("typing")]).unwrap();
    trace.lock().unwrap().sent.clear();
    let before = persistence::lock(d.persistence()).as_json();
    for value in [
        json!("fast"),
        json!(""),
        json!(0),
        json!(1),
        json!(true),
        Value::Null,
    ] {
        assert!(call(&mut d, "set_keyswitch_optimization", &[value]).is_err());
    }
    assert!(trace.lock().unwrap().sent.is_empty());
    trace.lock().unwrap().fail = Some(0x15);
    assert!(call(&mut d, "set_keyswitch_optimization", &[json!("gaming")]).is_err());
    assert_eq!(persistence::lock(d.persistence()).as_json(), before);
    let (mut d, trace) = device(0x0203);
    assert_eq!(d.capabilities()["keyswitch_optimization"], false);
    assert!(call(&mut d, "set_keyswitch_optimization", &[json!("gaming")]).is_err());
    assert!(call(&mut d, "get_keyswitch_optimization", &[]).is_err());
    assert!(trace.lock().unwrap().sent.is_empty());
}
