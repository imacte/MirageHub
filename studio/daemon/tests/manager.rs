use openrazer_daemon::{
    ble::{Advertiser, BleBackend, GattLink},
    database,
    hid::{HidBackend, HidHandle, HidInfo, InputReader},
    manager::{lock_device, DeviceManager},
    persistence::Persistence,
    protocol::report::Report,
    Error, Result,
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Hid {
    devices: Mutex<Vec<HidInfo>>,
    opens: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
}
impl HidBackend for Hid {
    fn enumerate(&self, vid: Option<u16>, _: Option<u16>) -> Result<Vec<HidInfo>> {
        assert_eq!(vid, Some(0x1532));
        Ok(self.devices.lock().unwrap().clone())
    }
    fn open(&self, _: &HidInfo) -> Result<Box<dyn HidHandle>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Handle {
            closes: self.closes.clone(),
            last: vec![],
        }))
    }
    fn open_input(&self, _: &HidInfo) -> Result<Box<dyn InputReader>> {
        Err(Error::Io("unused input".into()))
    }
}
struct Handle {
    closes: Arc<AtomicUsize>,
    last: Vec<u8>,
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}
impl HidHandle for Handle {
    fn send_feature(&mut self, data: &[u8]) -> Result<()> {
        self.last = data.to_vec();
        Ok(())
    }
    fn get_feature(&mut self, _: usize, _: u8) -> Result<Vec<u8>> {
        let mut report = Report::unpack(&self.last[1..])?;
        report.status = 2;
        report.arguments.fill(0);
        report.set_bytes(0, b"MANAGER-ORACLE");
        let mut packet = vec![0];
        packet.extend_from_slice(&report.pack());
        Ok(packet)
    }
    fn send_output(&mut self, _: &[u8]) -> Result<()> {
        Ok(())
    }
}
fn mouse() -> HidInfo {
    let meta = database::devices()
        .iter()
        .find(|m| m.device_type == "mouse")
        .unwrap();
    HidInfo {
        path: "fake".into(),
        vendor_id: meta.vid,
        product_id: meta.pid,
        feature_length: 91,
        ..Default::default()
    }
}

#[test]
fn hid_rescans_keep_the_same_object_then_close_removed_handles_and_report_unknown_models() {
    let hid = Arc::new(Hid::default());
    let info = mouse();
    hid.devices.lock().unwrap().push(info.clone());
    let manager = DeviceManager::new(hid.clone(), None, Persistence::memory().shared());
    let first = manager.scan(false).unwrap().remove(0);
    assert_eq!(hid.opens.load(Ordering::SeqCst), 0); // Discovery does not change a device.
    let serial = lock_device(&first).serial();
    assert!(Arc::ptr_eq(&first, &manager.by_serial(&serial).unwrap()));
    assert!(Arc::ptr_eq(
        &first,
        &manager.by_pid(info.product_id).unwrap()
    ));
    assert_eq!(manager.find(&format!(" {:04X} ", info.product_id)).len(), 1);
    assert!(Arc::ptr_eq(&first, &manager.scan(false).unwrap()[0]));
    assert_eq!(hid.opens.load(Ordering::SeqCst), 1);
    *hid.devices.lock().unwrap() = vec![HidInfo {
        product_id: 0xffff,
        product: "Unknown Razer".into(),
        ..info
    }];
    assert!(manager.scan(false).unwrap().is_empty());
    assert_eq!(hid.closes.load(Ordering::SeqCst), 1);
    assert_eq!(manager.unsupported(), [(0xffff, "Unknown Razer".into())]);
    manager.close().unwrap();
}

#[derive(Default)]
struct Radio {
    advertisers: Mutex<Vec<Advertiser>>,
    failure: AtomicBool,
    sweeps: AtomicUsize,
    block: Mutex<Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>>,
}
impl BleBackend for Radio {
    fn scan(&self, _: Duration, _: Option<u16>) -> Result<Vec<Advertiser>> {
        self.sweeps.fetch_add(1, Ordering::SeqCst);
        if let Some((started, release)) = self.block.lock().unwrap().take() {
            started.send(()).unwrap();
            release.recv().unwrap();
        }
        if self.failure.load(Ordering::SeqCst) {
            return Err(Error::Device("radio unavailable".into()));
        }
        Ok(self.advertisers.lock().unwrap().clone())
    }
    fn connect(&self, _: u64, _: bool, _: Instant) -> Result<Box<dyn GattLink>> {
        Ok(Box::new(Link))
    }
}
struct Link;
impl GattLink for Link {
    fn write(&mut self, _: &[u8], _: Instant) -> Result<()> {
        Ok(())
    }
    fn subscribe(&mut self, _: Instant) -> Result<()> {
        Ok(())
    }
    fn discard_notifications(&mut self) {}
    fn notification(&mut self, _: Instant) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn is_connected(&self) -> bool {
        true
    }
    fn describe(&self) -> Result<Value> {
        Ok(Value::Null)
    }
}
fn advertiser(address: u64, classic: u64, rssi: i16) -> Advertiser {
    Advertiser {
        address,
        classic_address: Some(classic),
        name: "Razer Stereo".into(),
        rssi,
        ..Default::default()
    }
}

#[test]
fn radio_twins_deduplicate_but_two_headsets_and_silent_connected_devices_stay_separate() {
    let radio = Arc::new(Radio::default());
    *radio.advertisers.lock().unwrap() = vec![
        advertiser(1, 10, -70),
        advertiser(2, 10, -30),
        advertiser(3, 11, -50),
    ];
    let manager = DeviceManager::new(
        Arc::new(Hid::default()),
        Some(radio.clone()),
        Persistence::memory().shared(),
    );
    assert_eq!(manager.scan(false).unwrap().len(), 2);
    let kept = manager.by_serial("BLE000000000002").unwrap();
    assert_eq!(
        lock_device(&kept).capabilities()["bluetooth_address"],
        "00000000000A"
    );
    lock_device(&kept)
        .call(
            "set_static",
            &[json!(1), json!(2), json!(3)],
            &Default::default(),
        )
        .unwrap();
    radio.advertisers.lock().unwrap().clear();
    assert_eq!(manager.scan(false).unwrap().len(), 2); // No radio sweep is due yet.
    assert_eq!(radio.sweeps.load(Ordering::SeqCst), 1);
    assert_eq!(manager.scan(true).unwrap().len(), 1); // One silent device is still connected.
    assert!(Arc::ptr_eq(&kept, &manager.devices()[0]));
    lock_device(&kept).close();
    assert!(manager.scan(true).unwrap().is_empty());
}

#[test]
fn missed_identity_packets_keep_twins_deduplicated_and_radio_errors_do_not_remove_devices() {
    let radio = Arc::new(Radio::default());
    *radio.advertisers.lock().unwrap() = vec![advertiser(1, 10, -70), advertiser(2, 10, -30)];
    let manager = DeviceManager::new(
        Arc::new(Hid::default()),
        Some(radio.clone()),
        Persistence::memory().shared(),
    );
    manager.scan(true).unwrap();
    let mut later = advertiser(1, 10, -30);
    later.classic_address = None;
    *radio.advertisers.lock().unwrap() = vec![later];
    let devices = manager.scan(true).unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(lock_device(&devices[0]).serial(), "BLE000000000002");
    radio.failure.store(true, Ordering::SeqCst);
    assert_eq!(manager.scan(true).unwrap().len(), 1);
    assert!(manager.warnings()[0].contains("radio unavailable"));
}

#[test]
fn a_slow_radio_scan_does_not_hold_the_registry_lock() {
    let radio = Arc::new(Radio::default());
    let (started, wait_started) = mpsc::sync_channel(1);
    let (release, wait_release) = mpsc::channel();
    *radio.block.lock().unwrap() = Some((started, wait_release));
    let manager = Arc::new(DeviceManager::new(
        Arc::new(Hid::default()),
        Some(radio),
        Persistence::memory().shared(),
    ));
    let scanning = manager.clone();
    let scan = thread::spawn(move || scanning.scan(true));
    wait_started.recv_timeout(Duration::from_secs(3)).unwrap();
    let reading = manager.clone();
    let (send, receive) = mpsc::sync_channel(1);
    let read = thread::spawn(move || send.send(reading.devices().len()).unwrap());
    let result = receive.recv_timeout(Duration::from_secs(1));
    release.send(()).unwrap();
    scan.join().unwrap().unwrap();
    read.join().unwrap();
    assert_eq!(result.unwrap(), 0);
}
