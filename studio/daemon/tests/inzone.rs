// SPDX-License-Identifier: GPL-3.0-only
// Captured wire fixtures from OpenInzone, Copyright (C) 2026 penguinwokrs.
use mirage_hub_daemon::{
    database,
    device::{Device, InzoneDevice},
    hid::HidInfo,
    inzone::{Io, Session},
    persistence::Persistence,
    protocol::inzone as p,
    Result,
};
use serde_json::{json, Map};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
const GET_REPLY: &[u8] = &[
    4, 0xff, 0x0a, 0, 0x96, 0xc3, 0x14, 0x22, 0x10, 1, 0, 0x32, 0xd2,
];
#[test]
fn captured_packets_match_and_wrong_or_malformed_replies_are_rejected() {
    assert_eq!(
        p::command(0x22, false, 1, &[]).unwrap(),
        [1, 0, 0xfc, 8, 0x96, 0xc3, 0x41, 0x22, 1, 1, 0, 0xbe]
    );
    assert_eq!(
        p::command(0x22, true, 0x65, &[30]).unwrap(),
        [1, 0, 0xfc, 9, 0x96, 0xc3, 0x41, 0x22, 2, 0x65, 0, 30, 0x41]
    );
    let packet = p::Packet::parse(GET_REPLY).unwrap();
    assert!(packet.matches(0x22, 1, false));
    assert!(!packet.matches(0x22, 1, true));
    assert!(!packet.matches(0x22, 2, false));
    let mut wrong = GET_REPLY.to_vec();
    wrong[6] = 0x12;
    wrong[12] = wrong[12].wrapping_sub(2);
    assert!(!p::Packet::parse(&wrong).unwrap().matches(0x22, 1, false));
    for index in 0..GET_REPLY.len() {
        let mut corrupt = GET_REPLY.to_vec();
        corrupt[index] ^= 1;
        assert!(p::Packet::parse(&corrupt).is_none());
    }
    assert_eq!(p::command(1, false, 1, &[]).unwrap()[6], 0x21);
    assert!(p::command(0xa0, true, 1, &[0; 51]).is_err());
}
#[test]
fn reports_reassemble_fragments_and_multiple_packets_without_accepting_bad_framing() {
    let mut stream = p::Reassembler::default();
    assert!(stream.add(&[0, 0x55, 4]).is_empty());
    assert!(stream.add(&GET_REPLY[1..7]).is_empty());
    let mut tail = GET_REPLY[7..].to_vec();
    tail.extend(GET_REPLY);
    assert_eq!(stream.add(&tail).len(), 2);
    let bytes = vec![7; 130];
    let reports = p::hid_reports(&bytes);
    assert_eq!(reports.len(), 3);
    assert_eq!(
        reports
            .iter()
            .flat_map(|r| p::hid_payload(r).unwrap().iter().copied())
            .collect::<Vec<_>>(),
        bytes
    );
    let mut bad = reports[0].clone();
    bad[1] = 63;
    assert!(p::hid_payload(&bad).is_none());
    assert!(p::hid_payload(&[2, 0]).is_none());
}
#[test]
fn capability_maps_distinguish_absence_unknown_and_short_h9_parts() {
    let buds = [
        4, 0, 0x44, 0, 0x43, 0xff, 0x61, 0, 0x15, 0xff, 0x32, 0, 0xff,
    ];
    let mic = [0, 0xff, 0xff, 2, 20, 0xff, 0, 1, 1, 1];
    let h9 = [3, 1, 1, 1, 2, 5, 0];
    let map = p::capability_map([Some(&buds), Some(&mic), Some(&h9)], Some(0));
    assert_eq!(map[&4].len(), 6);
    assert_eq!(map[&0x24], [0, 0xff, 0xff]);
    assert_eq!(map[&0x81], [5]);
    assert_eq!(map[&0x84], [0xff]);
    let unknown = p::capability_map([Some(&buds), Some(&mic), Some(&h9)], Some(5));
    assert!(!unknown.contains_key(&0x81));
    assert_eq!(unknown[&0x22], [50]);
    assert!(p::capability_map([Some(&[0; 10]), Some(&[0; 8]), Some(&[0; 3])], Some(0)).is_empty());
}

#[test]
fn captured_h5_map_shows_timer_language_and_audio_without_noise_cancelling() {
    let part1 = [3, 0, 0x46, 0, 15, 50, 50, 10, 100];
    let part2 = [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    let part3 = [0xff, 0xff, 0xff, 0xff, 0xff, 5, 0xff, 2];
    let map = p::capability_map([Some(&part1), Some(&part2), Some(&part3)], Some(3));
    assert_eq!(map[&4], [0, 70]);
    assert_eq!(map[&0x21], [0, 15, 50]);
    assert_eq!(map[&0x23], [10, 100]);
    assert_eq!(map[&0x81], [5]);
    assert_eq!(map[&0x83], [2]);
    assert_eq!(map[&0x84], [0xff]);
    assert_eq!(map[&0x41], [0xff; 4]);
    assert!(p::capability_map([None, None, Some(&part3[..7])], Some(3)).is_empty());
}

#[derive(Default)]
struct State {
    values: BTreeMap<u8, Vec<u8>>,
    written: Vec<(u8, Vec<u8>)>,
}
struct Wire {
    state: Arc<Mutex<State>>,
    input: VecDeque<Vec<u8>>,
}
fn reply(event: u8, kind: u8, tid: u16, param: &[u8]) -> Vec<u8> {
    let mut data = vec![
        4,
        0xff,
        (9 + param.len()) as u8,
        0,
        0x96,
        0xc3,
        0x10 | p::destination(event),
        event,
        kind,
        tid as u8,
        (tid >> 8) as u8,
    ];
    data.extend(param);
    data.push(data[3..].iter().fold(0u8, |s, b| s.wrapping_add(*b)));
    data
}
impl Io for Wire {
    fn write(&mut self, packet: &[u8]) -> Result<()> {
        let event = packet[7];
        let tid = u16::from_le_bytes([packet[9], packet[10]]);
        let set = packet[8] == 2;
        let mut state = self.state.lock().unwrap();
        if set {
            let bytes = packet[11..packet.len() - 1].to_vec();
            state.written.push((event, bytes.clone()));
            state.values.insert(event, bytes);
        }
        let bytes = state
            .values
            .get(&event)
            .cloned()
            .unwrap_or_else(|| vec![0xff]);
        // An unrelated notification arriving before our response must not satisfy a SET.
        self.input
            .push_back(reply(event, 0x20, tid.wrapping_sub(1), &[0xff]));
        self.input
            .push_back(reply(event, if set { 0x20 } else { 0x10 }, tid, &bytes));
        Ok(())
    }
    fn read(&mut self, _: Duration) -> Result<Option<Vec<u8>>> {
        Ok(self.input.pop_front())
    }
    fn close(&mut self) {
        self.input.clear();
    }
}
fn device(model: u8) -> (InzoneDevice, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State {
        values: BTreeMap::from([
            (1, vec![1]),
            (2, vec![model, 0, 0, 0, 0, 0]),
            (0x21, vec![0, 15, 0xff]),
            (0x24, vec![0, 0xff, 0xff]),
            (0x41, vec![2, 20, 0xff, 0]),
            (0x81, vec![15]),
        ]),
        ..Default::default()
    }));
    let io = Wire {
        state: state.clone(),
        input: VecDeque::new(),
    };
    let device = InzoneDevice::new(
        database::lookup(0x054c, 0x0ebf).unwrap().clone(),
        HidInfo {
            vendor_id: 0x054c,
            product_id: 0x0ebf,
            path: "test-inzone".into(),
            ..Default::default()
        },
        Box::new(io),
        Persistence::memory().shared(),
    );
    (device, state)
}
#[test]
fn writes_preserve_sibling_bytes_and_reject_unsupported_mute_and_invalid_settings() {
    let (mut device, state) = device(3);
    device
        .call("set_headphone_volume", &[json!(20)], &Map::new())
        .unwrap();
    assert_eq!(
        state.lock().unwrap().written.last().unwrap(),
        &(0x21, vec![0, 20, 0xff])
    );
    device
        .call("set_ambient_mode", &[json!(1)], &Map::new())
        .unwrap();
    assert_eq!(
        state.lock().unwrap().written.last().unwrap(),
        &(0x41, vec![1, 20, 0xff, 0])
    );
    let count = state.lock().unwrap().written.len();
    assert!(device
        .call("set_microphone_muted", &[json!(true)], &Map::new())
        .is_err());
    assert!(device
        .call("set_headphone_volume", &[json!(31)], &Map::new())
        .is_err());
    assert!(device
        .call("set_auto_power_off", &[json!(7)], &Map::new())
        .is_err());
    assert_eq!(state.lock().unwrap().written.len(), count);
}
#[test]
fn buds_mute_preserves_unknown_level_and_session_filters_notifications() {
    let (mut device, state) = device(4);
    device
        .call("set_microphone_muted", &[json!(true)], &Map::new())
        .unwrap();
    assert_eq!(
        state.lock().unwrap().written.last().unwrap(),
        &(0x24, vec![1, 0xff, 0xff])
    );
    let mut session = Session::new(Box::new(Wire {
        state,
        input: VecDeque::new(),
    }));
    assert_eq!(session.request(0x21, None).unwrap(), [0, 15, 0xff]);
}

#[test]
fn missing_bud_batteries_are_unknown_and_case_is_not_the_headset_charge() {
    let (mut device,state)=device(4);
    state.lock().unwrap().values.insert(4,vec![0,0xff,0,45,0,90]);
    let parts=device.call("get_battery_parts",&[],&Map::new()).unwrap();
    assert!(parts[0]["percent"].is_null());assert_eq!(parts[1]["percent"],45);assert_eq!(parts[2]["snapshot"],true);
    assert_eq!(device.call("get_battery_level",&[],&Map::new()).unwrap(),45);
    assert!(device.call("is_charging",&[],&Map::new()).unwrap().is_null());
    state.lock().unwrap().values.insert(4,vec![0,0xff,0,0xff,0,90]);
    assert!(device.call("get_battery_level",&[],&Map::new()).unwrap().is_null());
    assert!(device.call("set_microphone_level",&[json!(0)],&Map::new()).is_err());
}

#[test]
fn emulated_h5_has_only_reported_settings_and_cannot_touch_a_real_microphone() {
    use mirage_hub_daemon::{demo::DemoBackend,manager::{DeviceManager,lock_device}};
    let manager=DeviceManager::new(Arc::new(DemoBackend::new().unwrap()),None,Persistence::memory().shared());
    manager.scan_usb().unwrap();
    let shared=manager.find("054c:0ebf").remove(0);
    let mut device=lock_device(&shared);
    let audio=device.call("get_audio",&[],&Map::new()).unwrap();
    assert_eq!(audio["connected"],true);
    let settings=audio["settings"].as_array().unwrap();
    let timer=settings.iter().find(|s|s["key"]=="auto_power_off").unwrap();
    assert_eq!(timer["choices"],json!([0,5,15,30,60,180]));
    assert!(settings.iter().all(|s|s["key"]!="ambient_mode" && s["key"]!="microphone_level"));
    assert_eq!(settings.iter().find(|s|s["key"]=="microphone_muted").unwrap()["readonly"],true);
    assert!(device.call("set_microphone_level",&[json!(0)],&Map::new()).is_err());
}
