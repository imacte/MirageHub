use rsrazer_daemon::{
    ble::{Advertisements, Advertiser, BleBackend, BleChannel, BleTransport, GattLink, Timing},
    database,
    device::{BleDevice, Device},
    persistence::Persistence,
    protocol::ble,
    Error, Result,
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct State {
    events: Vec<String>,
    connects_fail: usize,
    writes_fail: usize,
    requests_ignored: usize,
    packets: VecDeque<Vec<u8>>,
    signals: Option<mpsc::Sender<String>>,
}
impl State {
    fn record(&mut self, event: String) {
        self.events.push(event.clone());
        if let Some(sender) = &self.signals {
            let _ = sender.send(event);
        }
    }
}
struct Backend(Arc<Mutex<State>>);
impl BleBackend for Backend {
    fn scan(&self, _duration: Duration, _company: Option<u16>) -> Result<Vec<Advertiser>> {
        Ok(Vec::new())
    }
    fn connect(
        &self,
        _address: u64,
        uncached: bool,
        _deadline: Instant,
    ) -> Result<Box<dyn GattLink>> {
        let mut state = self.0.lock().unwrap();
        state.record(format!("connect:{uncached}"));
        if state.connects_fail > 0 {
            state.connects_fail -= 1;
            return Err(Error::Device("cold link".into()));
        }
        Ok(Box::new(Link(self.0.clone())))
    }
}
struct Link(Arc<Mutex<State>>);
impl Drop for Link {
    fn drop(&mut self) {
        self.0.lock().unwrap().record("drop".into());
    }
}
impl GattLink for Link {
    fn write(&mut self, payload: &[u8], _deadline: Instant) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        state.record(format!("write:{}", payload[0]));
        if state.writes_fail > 0 {
            state.writes_fail -= 1;
            return Err(Error::Device("broken link".into()));
        }
        if payload[0] == ble::BATTERY {
            if state.requests_ignored > 0 {
                state.requests_ignored -= 1;
            } else {
                state.packets.extend([
                    vec![1],
                    vec![ble::CHARGING, 1, 1, 1],
                    vec![ble::BATTERY, 1, 2, 99],
                    vec![ble::BATTERY, 2, 1, 87],
                ]);
            }
        }
        Ok(())
    }
    fn subscribe(&mut self, _deadline: Instant) -> Result<()> {
        self.0.lock().unwrap().record("subscribe".into());
        Ok(())
    }
    fn discard_notifications(&mut self) {
        let mut state = self.0.lock().unwrap();
        state.record("discard".into());
        state.packets.clear();
    }
    fn notification(&mut self, _deadline: Instant) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().unwrap().packets.pop_front())
    }
    fn is_connected(&self) -> bool {
        true
    }
    fn describe(&self) -> Result<Value> {
        Ok(json!({"name":"fixture"}))
    }
}
fn timing() -> Timing {
    Timing {
        resolve_backoff: Duration::ZERO,
        keepalive: Duration::from_secs(30),
        idle_disconnect: Duration::from_secs(30),
        reply_timeout: Duration::from_millis(100),
    }
}
fn transport(state: Arc<Mutex<State>>, timing: Timing) -> BleTransport {
    BleTransport::with_timing(0x112233445566, Arc::new(Backend(state)), timing).unwrap()
}

#[test]
fn cold_links_retry_uncached_and_broken_links_are_rebuilt_before_the_next_write() {
    let state = Arc::new(Mutex::new(State {
        connects_fail: 2,
        writes_fail: 1,
        ..Default::default()
    }));
    let mut link = transport(state.clone(), timing());
    link.write(&ble::brightness_command(127), false).unwrap();
    assert!(link.is_connected());
    link.close();
    assert!(!link.is_connected());
    assert_eq!(
        state.lock().unwrap().events,
        [
            "connect:false",
            "connect:true",
            "connect:true",
            "write:193",
            "drop",
            "connect:false",
            "write:193",
            "drop"
        ]
    );
}

#[test]
fn requests_subscribe_before_writing_skip_stray_packets_and_retry_a_lost_request() {
    let state = Arc::new(Mutex::new(State {
        requests_ignored: 1,
        ..Default::default()
    }));
    let mut link = transport(state.clone(), timing());
    assert_eq!(link.request(ble::BATTERY, &[]).unwrap(), [87]);
    assert_eq!(
        state.lock().unwrap().events,
        [
            "connect:false",
            "subscribe",
            "discard",
            "write:33",
            "subscribe",
            "discard",
            "write:33"
        ]
    );
}

#[test]
fn three_unanswered_requests_fail_and_a_later_request_can_recover() {
    let state = Arc::new(Mutex::new(State {
        requests_ignored: 3,
        ..Default::default()
    }));
    let mut link = transport(state.clone(), timing());
    assert!(link
        .request(ble::BATTERY, &[])
        .unwrap_err()
        .to_string()
        .contains("3 attempts"));
    assert_eq!(
        state
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|e| *e == "write:33")
            .count(),
        3
    );
    assert_eq!(link.request(ble::BATTERY, &[]).unwrap(), [87]);
}

fn until(receiver: &mpsc::Receiver<String>, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let event = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        if event == expected {
            return;
        }
    }
}

#[test]
fn held_colour_is_refreshed_and_releasing_it_allows_idle_disconnect_and_reuse() {
    let (sender, receiver) = mpsc::channel();
    let state = Arc::new(Mutex::new(State {
        signals: Some(sender),
        ..Default::default()
    }));
    let mut intervals = timing();
    intervals.keepalive = Duration::from_millis(20);
    intervals.idle_disconnect = Duration::from_millis(30);
    let mut link = transport(state.clone(), intervals);
    link.write(&[77], true).unwrap();
    until(&receiver, "write:77");
    until(&receiver, "write:77"); // Actual worker timer, not a manual refresh.
    link.write(&[78], false).unwrap();
    until(&receiver, "write:78");
    until(&receiver, "write:77"); // A non-held animation frame preserves the held colour.
    link.release();
    until(&receiver, "drop");
    assert!(!link.is_connected());
    link.write(&[79], false).unwrap();
    assert!(link.is_connected());
    drop(link);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|e| *e == "drop")
            .count(),
        2
    );
}

#[test]
fn scan_responses_merge_names_manufacturers_and_classic_addresses_before_filtering() {
    let mut seen = Advertisements::default();
    seen.add(1, "Razer Stereo", -75, &[]);
    seen.add(
        1,
        "",
        -70,
        &[(ble::COMPANY_ID, vec![5, 98, 0, 6, 5, 4, 3, 2, 1])],
    );
    seen.add(1, "", -69, &[(ble::COMPANY_ID, vec![5])]); // Truncation must not erase the identity.
    seen.add(2, "strong", -30, &[(ble::COMPANY_ID, vec![5, 98])]);
    seen.add(3, "other", -10, &[(42, vec![5, 98])]);
    let result = seen.finish(Some(ble::COMPANY_ID));
    assert_eq!(result.iter().map(|a| a.address).collect::<Vec<_>>(), [2, 1]);
    assert_eq!(result[1].name, "Razer Stereo");
    assert_eq!(result[1].product_id, Some(0x562));
    assert_eq!(result[1].classic_address, Some(0x010203040506));
    assert_eq!(seen.finish(None).len(), 3);
}

struct FailingChannel;
impl BleChannel for FailingChannel {
    fn address(&self) -> u64 {
        1
    }
    fn write(&mut self, _: &[u8], _: bool) -> Result<()> {
        Err(Error::Device("offline".into()))
    }
    fn request(&mut self, _: u8, _: &[u8]) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }
    fn release(&mut self) {}
    fn is_connected(&self) -> bool {
        false
    }
    fn describe(&mut self) -> Result<Value> {
        Ok(Value::Null)
    }
    fn close(&mut self) {}
}
#[test]
fn bluetooth_restore_propagates_failure_for_retry_without_saving_failed_changes() {
    let store = Persistence::memory().shared();
    let mut device = BleDevice::new(
        database::lookup(0x1532, 0x562).unwrap().clone(),
        Box::new(FailingChannel),
        store.clone(),
    );
    device.restore().unwrap(); // A default is not a saved colour.
    device.enter_driver_mode(false).unwrap(); // Takeover is best effort.
    let serial = device.serial();
    store
        .lock()
        .unwrap()
        .set(&serial, "backlight", "effect", json!("static"));
    store
        .lock()
        .unwrap()
        .set_device(&serial, "zone_colours", json!([1, 2, 3, 4, 5, 6]));
    let before = store.lock().unwrap().as_json();
    assert!(device.restore().is_err());
    assert!(device
        .call(
            "set_static",
            &[json!(9), json!(8), json!(7)],
            &Default::default()
        )
        .is_err());
    assert_eq!(store.lock().unwrap().as_json(), before);
    assert!(matches!(
        device.call("get_battery_level", &[], &Default::default()),
        Err(Error::NotSupported(_))
    ));
    // The old Python helper could emit a malformed two-byte colour body.
    // Validate incomplete RGB triples before contacting any hardware.
    assert!(matches!(
        device.call("set_zone_colours", &[json!([[1]])], &Default::default()),
        Err(Error::Invalid(_))
    ));
}
