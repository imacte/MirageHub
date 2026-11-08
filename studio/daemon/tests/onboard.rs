//! Protocol fixtures for OpenRazer #2962. The Rust HID emulator models flash
//! BUSY replies and missing macro IDs; these tests never open physical devices.
use rsrazer_daemon::{
    database,
    demo::DemoBackend,
    device::{Device, StandardDevice},
    hid::{HidBackend, HidHandle, HidInfo, InputReader},
    persistence::Persistence,
    protocol::{onboard, report::Report},
    transport::Transport,
    Error, Result,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Trace {
    sent: Vec<Report>,
    last: Option<Report>,
    lose_reply: Option<(u8, u8)>,
    busy_reads: usize,
    wrong_button: bool,
    wrong_length_after_upload: bool,
    uploaded: bool,
}
struct Backend {
    demo: DemoBackend,
    trace: Arc<Mutex<Trace>>,
}
struct Handle {
    inner: Box<dyn HidHandle>,
    trace: Arc<Mutex<Trace>>,
}
impl HidBackend for Backend {
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
        let mut trace = self.trace.lock().unwrap();
        trace.uploaded |= (request.command_class, request.command_id) == (6, 9);
        trace.sent.push(request.clone());
        trace.last = Some(request);
        drop(trace);
        self.inner.send_feature(data)
    }
    fn get_feature(&mut self, length: usize, id: u8) -> Result<Vec<u8>> {
        let data = self.inner.get_feature(length, id)?;
        let mut reply = Report::unpack(&data[1..])?;
        let command = (reply.command_class, reply.command_id);
        let mut trace = self.trace.lock().unwrap();
        if trace.lose_reply == Some(command) {
            trace.lose_reply = None;
            return Err(Error::Io(
                "lost acknowledgement after the device accepted the write".into(),
            ));
        }
        if command == (6, 0x88) && trace.busy_reads > 0 {
            trace.busy_reads -= 1;
            reply.status = 1;
            reply.arguments.fill(0);
        } else if command == (6, 0x88)
            && trace.uploaded
            && trace.wrong_length_after_upload
            && reply.status == 2
        {
            reply.set_bytes(2, &1u32.to_be_bytes());
        }
        if command == (2, 0x8c) && trace.wrong_button {
            reply.arguments[1] ^= 1;
        }
        let mut data = vec![0];
        data.extend(reply.pack());
        Ok(data)
    }
    fn send_output(&mut self, data: &[u8]) -> Result<()> {
        self.inner.send_output(data)
    }
}
fn device(pid: u16) -> (StandardDevice, Arc<Mutex<Trace>>) {
    let trace = Arc::new(Mutex::new(Trace::default()));
    let backend = Arc::new(Backend {
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
    trace.lock().unwrap().sent.clear();
    (device, trace)
}
fn call(d: &mut StandardDevice, method: &str, args: Value) -> Result<Value> {
    d.call(method, args.as_array().unwrap(), &Default::default())
}
fn sent(trace: &Arc<Mutex<Trace>>, command: (u8, u8)) -> Vec<Report> {
    trace
        .lock()
        .unwrap()
        .sent
        .iter()
        .filter(|r| (r.command_class, r.command_id) == command)
        .cloned()
        .collect()
}

#[test]
fn only_both_naga_v2_pro_connections_offer_onboard_memory() {
    for pid in [0x00a7, 0x00a8, 0x00a5, 0x00a6, 0x0078, 0x026b] {
        let (mut d, trace) = device(pid);
        let capable = matches!(pid, 0x00a7 | 0x00a8);
        assert_eq!(d.capabilities()["onboard_buttons"], capable);
        assert_eq!(d.capabilities()["onboard_macros"], capable);
        if capable {
            assert_eq!(call(&mut d, "get_side_plate", json!([])).unwrap(), 4);
            assert_eq!(
                call(&mut d, "get_onboard_buttons", json!([])).unwrap(),
                json!(onboard::BUTTONS)
            );
            let reports = sent(&trace, (0, 0xb9));
            assert_eq!((reports[0].transaction_id, reports[0].data_size), (0x1f, 1));
        } else {
            for method in onboard::METHODS {
                assert!(matches!(
                    call(&mut d, method, json!([])),
                    Err(Error::NotSupported(_))
                ));
            }
            assert!(trace.lock().unwrap().sent.is_empty());
        }
    }
}

#[test]
fn bindings_keep_store_layer_factory_bytes_and_release_only_the_live_store() {
    for pid in [0x00a7, 0x00a8] {
        let (mut d, trace) = device(pid);
        call(&mut d, "set_button_key", json!([0x50, 4, 1, 1, 1])).unwrap();
        let request = sent(&trace, (2, 0x0c)).remove(0);
        assert_eq!((request.transaction_id, request.data_size), (0x1f, 10));
        assert_eq!(&request.arguments[..10], &[1, 0x50, 1, 2, 2, 1, 4, 0, 0, 0]);
        let modes = sent(&trace, (0, 4));
        assert_eq!(
            modes
                .iter()
                .map(|r| r.arguments[..2].to_vec())
                .collect::<Vec<_>>(),
            vec![vec![3, 0], vec![0, 0]]
        );
        // The emulator returns flag 0 even when the selected layer is 1.
        assert_eq!(
            call(&mut d, "get_button_action", json!([0x50, 1, 1])).unwrap(),
            json!([2, 2, 1, 4])
        );
        trace.lock().unwrap().sent.clear();
        call(
            &mut d,
            "set_button_action",
            json!([0x50, [2, 1, 3, 4, 5], 0, 5]),
        )
        .unwrap();
        assert!(sent(&trace, (0, 4)).is_empty());
        assert_eq!(
            call(&mut d, "get_button_action", json!([0x50, 0, 5])).unwrap(),
            json!([2, 1, 3, 4, 5])
        );
        trace.lock().unwrap().wrong_button = true;
        assert!(call(&mut d, "get_button_action", json!([0x50, 0, 5])).is_err());
    }
}

#[test]
fn uploads_are_chunked_once_and_busy_reads_finish_before_binding() {
    for pid in [0x00a7, 0x00a8] {
        let (mut d, trace) = device(pid);
        trace.lock().unwrap().busy_reads = 2;
        let events = [1, 4, 0x11, 25, 2, 4].repeat(30); // 180 bytes, spanning three reports
        call(&mut d, "add_onboard_macro", json!([0x1234, events])).unwrap();
        let create = sent(&trace, (6, 8));
        assert_eq!(create.len(), 1);
        assert_eq!(&create[0].arguments[..6], &[0x12, 0x34, 0, 0, 0, 180]);
        let chunks = sent(&trace, (6, 9));
        assert_eq!(chunks.len(), 3);
        let mut bytes = Vec::new();
        for (index, r) in chunks.iter().enumerate() {
            assert_eq!(r.transaction_id, 0x1f);
            assert_eq!(
                u32::from_be_bytes(r.arguments[2..6].try_into().unwrap()),
                (index * 73) as u32
            );
            let length = usize::from(r.arguments[6]);
            assert_eq!(usize::from(r.data_size), 7 + length);
            bytes.extend_from_slice(&r.arguments[7..7 + length]);
        }
        assert_eq!(bytes, events);
        assert!(sent(&trace, (6, 0x88)).len() >= 6); // preflight + both BUSY windows + readback
        call(
            &mut d,
            "set_button_macro",
            json!([0x50, 0x1234, 0, false, 1]),
        )
        .unwrap();
        assert_eq!(
            call(&mut d, "get_button_action", json!([0x50])).unwrap(),
            json!([3, 3, 0x12, 0x34, 1])
        );
        call(
            &mut d,
            "set_button_macro",
            json!([0x51, 0x1234, 1, true, 5]),
        )
        .unwrap();
        assert_eq!(
            call(&mut d, "get_button_action", json!([0x51, 1, 5])).unwrap(),
            json!([4, 2, 0x12, 0x34])
        );
        assert_eq!(
            call(&mut d, "get_onboard_macro_length", json!([0x1234])).unwrap(),
            180
        );
    }
}

#[test]
fn deletion_checks_unattached_plates_both_layers_and_all_stores() {
    let (mut d, trace) = device(0x00a7);
    call(&mut d, "add_onboard_macro", json!([7, [1, 4, 2, 4]])).unwrap();
    call(&mut d, "set_button_macro", json!([0x4b, 7, 1, true, 5])).unwrap(); // 12-button plate; emulator has six-button plate
    let error = call(&mut d, "delete_onboard_macro", json!([7]))
        .unwrap_err()
        .to_string();
    assert!(error.contains("store 5") && error.contains("0x4b") && error.contains("layer 1"));
    assert!(sent(&trace, (6, 3)).is_empty());
    call(&mut d, "set_button_mouse", json!([0x4b, 4, 1, 5])).unwrap();
    call(&mut d, "delete_onboard_macro", json!([7])).unwrap();
    assert_eq!(sent(&trace, (6, 3)).len(), 1);
    assert_eq!(
        call(&mut d, "get_onboard_macro_length", json!([7])).unwrap(),
        0
    );
    assert_eq!(
        call(&mut d, "get_onboard_macro_storage", json!([])).unwrap()["deleted"],
        4
    );
}

#[test]
fn an_incomplete_firmware_list_never_allows_overwriting_a_live_macro() {
    let (mut d, trace) = device(0x00a8);
    for id in [1, 2] {
        call(&mut d, "add_onboard_macro", json!([id, [1, 4, 2, 4]])).unwrap();
    }
    call(&mut d, "delete_onboard_macro", json!([1])).unwrap();
    let inventory = call(&mut d, "get_onboard_macro_inventory", json!([])).unwrap();
    assert_eq!(inventory, json!({"count":1,"ids":[],"complete":false}));
    let before = sent(&trace, (6, 8)).len();
    assert!(call(&mut d, "add_onboard_macro", json!([2, [1, 5, 2, 5]])).is_err());
    assert_eq!(sent(&trace, (6, 8)).len(), before);
    assert_eq!(
        call(&mut d, "get_onboard_macro_length", json!([2])).unwrap(),
        4
    );
}

#[test]
fn ambiguous_writes_are_not_repeated_and_partial_uploads_are_removed() {
    for command in [(6, 8), (6, 9)] {
        let (mut d, trace) = device(0x00a7);
        trace.lock().unwrap().lose_reply = Some(command);
        assert!(call(&mut d, "add_onboard_macro", json!([7, [1, 4, 2, 4]])).is_err());
        assert_eq!(sent(&trace, command).len(), 1);
        assert_eq!(sent(&trace, (6, 3)).len(), 1);
        assert_eq!(
            call(&mut d, "get_onboard_macro_count", json!([])).unwrap(),
            0
        );
    }
    let (mut d, trace) = device(0x00a8);
    trace.lock().unwrap().lose_reply = Some((2, 0x0c));
    assert!(call(&mut d, "set_button_key", json!([0x50, 4])).is_err());
    assert_eq!(sent(&trace, (2, 0x0c)).len(), 1);
    assert_eq!(sent(&trace, (0, 4)).len(), 2); // still release an old held key
}

#[test]
fn a_wrong_upload_readback_is_an_error_and_does_not_leave_a_partial_macro() {
    let (mut d, trace) = device(0x00a7);
    trace.lock().unwrap().wrong_length_after_upload = true;
    assert!(call(&mut d, "add_onboard_macro", json!([7, [1, 4, 2, 4]])).is_err());
    assert_eq!(sent(&trace, (6, 8)).len(), 1);
    assert_eq!(sent(&trace, (6, 9)).len(), 1);
    assert_eq!(sent(&trace, (6, 3)).len(), 1);
    trace.lock().unwrap().wrong_length_after_upload = false;
    assert_eq!(
        call(&mut d, "get_onboard_macro_length", json!([7])).unwrap(),
        0
    );
}

#[test]
fn malformed_events_ids_stores_and_actions_fail_before_any_hid_transfer() {
    let (mut d, trace) = device(0x00a7);
    for (method, args) in [
        ("add_onboard_macro", json!([0, [1, 4, 2, 4]])),
        ("add_onboard_macro", json!([1, []])),
        ("add_onboard_macro", json!([1, [1]])),
        ("add_onboard_macro", json!([1, [0x12, 1]])),
        ("add_onboard_macro", json!([1, [0xff, 0]])),
        ("add_onboard_macro", json!([1, [1, 256]])),
        ("set_button_key", json!([0x50, 4, 0, 2])),
        ("set_button_key", json!([0x50, 4, 0, 0, 0])),
        ("set_button_mouse", json!([0x50, 6])),
        ("set_button_action", json!([0x50, [1, 5, 1]])),
        ("set_button_action", json!([0x50, []])),
        ("get_button_action", json!([0xff])),
        ("delete_onboard_macro", json!([0])),
    ] {
        assert!(call(&mut d, method, args).is_err(), "{method}");
        assert!(
            trace.lock().unwrap().sent.is_empty(),
            "{method} transferred before validating"
        );
    }
}
