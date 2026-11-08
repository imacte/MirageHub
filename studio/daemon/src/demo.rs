//! In-process HID devices for the executable's --emulate mode. They exercise
//! the normal manager, transports and device APIs without opening real hardware.
use crate::{
    database,
    effects::{
        audio::{AudioMeter, SilentMeter},
        engine::{EffectEngine, Environment},
        frame::Colour,
        keys::KeySource,
    },
    hid::{HidBackend, HidHandle, HidInfo, InputReader},
    keys::{
        inject::{Injection, Injector, Launcher},
        KeyService,
    },
    manager::DeviceManager,
    protocol::{blackshark as bs, report::Report},
    recipes::recipes,
    service::{DaemonService, Options, Stores},
    Error, Result,
};
use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

#[path = "demo_onboard.rs"]
mod onboard;
pub const PRODUCT_IDS: [u16; 6] = [0x0203, 0x0078, 0x0c00, bs::PID_WIRELESS, 0x026b, 0x00a7];
struct State {
    onboard: onboard::State,
    serial: String,
    response: Vec<u8>,
    mode: [u8; 2],
    brightness: BTreeMap<u8, u8>,
    led_state: BTreeMap<u8, u8>,
    led_effect: BTreeMap<u8, u8>,
    led_rgb: BTreeMap<u8, [u8; 3]>,
    dpi: [u16; 2],
    stages: Vec<u8>,
    poll_rate: u32,
    idle: u16,
    threshold: u8,
    keyswitch: [u8; 5],
    keyswitch_store: [u8; 6],
    headset: BTreeMap<u8, Vec<u8>>,
    last_frame: Vec<u8>,
}
impl State {
    fn new(serial: String) -> Self {
        Self {
            onboard: onboard::State::default(),
            serial,
            response: vec![0; 91],
            mode: [0, 0],
            brightness: BTreeMap::new(),
            led_state: BTreeMap::new(),
            led_effect: BTreeMap::new(),
            led_rgb: BTreeMap::new(),
            dpi: [1800, 1800],
            stages: vec![
                1, 3, 0, 3, 32, 3, 32, 0, 0, 1, 7, 8, 7, 8, 0, 0, 2, 12, 128, 12, 128, 0, 0,
            ],
            poll_rate: 500,
            idle: 900,
            threshold: 12,
            keyswitch: [0, 20, 0, 40, 0],
            keyswitch_store: [1, 0, 20, 0, 40, 0],
            headset: BTreeMap::from([
                (bs::FIRMWARE, vec![1, 7, 0, 0]),
                (bs::EQ_PRESET, vec![0]),
                (bs::EQ_BANDS, vec![0; 10]),
                (bs::SIDETONE, vec![1]),
                (bs::SIDETONE_LEVEL, vec![5]),
                (bs::EQ_ENABLED, vec![1]),
                (bs::BATTERY, vec![85]),
                (bs::DO_NOT_DISTURB, vec![0]),
                (bs::CHARGING, vec![0]),
                (bs::IDLE_MINUTES, vec![30]),
                (bs::MIC_MUTED, vec![0]),
                (bs::DONGLE_LED, vec![1]),
            ]),
            last_frame: Vec::new(),
        }
    }
    fn answer(&mut self, request: Report) -> Report {
        if let Some(reply) = self.onboard.answer(&request) { return reply; }
        let mut response = request.clone();
        response.status = 2;
        let a = &request.arguments;
        match (request.command_class, request.command_id) {
            (0, 0x82) => {
                response.arguments.fill(0);
                response.set_bytes(0, self.serial.as_bytes());
            }
            (0, 0x81) => response.set_bytes(0, &[1, 15]),
            (0, 0x84) => response.set_bytes(0, &self.mode),
            (0, 0x04) => self.mode.copy_from_slice(&a[..2]),
            (2, 2) => self.keyswitch.copy_from_slice(&a[..5]),
            (2, 0x15) => self.keyswitch_store.copy_from_slice(&a[..6]),
            (2, 0x82) => response.set_bytes(0, &self.keyswitch),
            (3, 3) | (15, 4) => {
                self.brightness.insert(a[1], a[2]);
            }
            (3, 0x83) | (15, 0x84) => {
                response.arguments[2] = *self.brightness.get(&a[1]).unwrap_or(&255)
            }
            (3, 0) => {
                self.led_state.insert(a[1], a[2]);
            }
            (3, 0x80) => response.arguments[2] = *self.led_state.get(&a[1]).unwrap_or(&0),
            (3, 2) => {
                self.led_effect.insert(a[1], a[2]);
            }
            (3, 0x82) => response.arguments[2] = *self.led_effect.get(&a[1]).unwrap_or(&0),
            (3, 1) => {
                self.led_rgb.insert(a[1], [a[2], a[3], a[4]]);
            }
            (3, 0x81) => response.set_bytes(2, self.led_rgb.get(&a[1]).unwrap_or(&[0; 3])),
            (4, 5) => {
                self.dpi = [
                    u16::from_be_bytes([a[1], a[2]]),
                    u16::from_be_bytes([a[3], a[4]]),
                ]
            }
            (4, 0x85) => {
                response.set_bytes(1, &self.dpi[0].to_be_bytes());
                response.set_bytes(3, &self.dpi[1].to_be_bytes());
            }
            (4, 1) => self.dpi = [u16::from(a[0]) * 100, u16::from(a[1]) * 100],
            (4, 0x81) => response.set_bytes(
                0,
                &[
                    (self.dpi[0] / 100).min(255) as u8,
                    (self.dpi[1] / 100).min(255) as u8,
                ],
            ),
            (4, 6) => self.stages = a[1..].to_vec(),
            (4, 0x86) => response.set_bytes(1, &self.stages),
            (0, 5) => {
                self.poll_rate = match a[0] {
                    1 => 1000,
                    8 => 125,
                    _ => 500,
                }
            }
            (0, 0x85) => {
                response.arguments[0] = match self.poll_rate {
                    1000 => 1,
                    125 => 8,
                    _ => 2,
                }
            }
            (0, 0x40) => {
                self.poll_rate = match a[1] {
                    1 => 8000,
                    2 => 4000,
                    4 => 2000,
                    8 => 1000,
                    32 => 250,
                    64 => 125,
                    _ => 500,
                }
            }
            (0, 0xc0) => {
                response.arguments[1] = match self.poll_rate {
                    8000 => 1,
                    4000 => 2,
                    2000 => 4,
                    1000 => 8,
                    250 => 32,
                    125 => 64,
                    _ => 16,
                }
            }
            (7, 0x80) => response.arguments[1] = 140,
            (7, 0x84) => response.arguments[1] = 0,
            (7, 3) => self.idle = u16::from_be_bytes([a[0], a[1]]),
            (7, 0x83) => response.set_bytes(0, &self.idle.to_be_bytes()),
            (7, 1) => self.threshold = a[0],
            (7, 0x81) => response.arguments[0] = self.threshold,
            (3, 11) | (15, 3) | (3, 12) => self.last_frame = a.to_vec(),
            _ => (),
        }
        response
    }
    fn headset_answer(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() != 64
            || data[0] != 2
            || data[..62].iter().fold(0, |a, b| a ^ b) != data[62]
            || data[12] > 48
        {
            return Err(Error::Invalid("bad emulated headset request".into()));
        }
        let command = data[10];
        let key = command & !bs::WRITE;
        let (status, body) = if command & bs::WRITE != 0 {
            if let Some(value) = self.headset.get_mut(&key) {
                *value = data[13..13 + data[12] as usize].to_vec();
                if key == bs::EQ_BANDS {
                    for b in value {
                        *b = b.wrapping_sub(5);
                    }
                }
                (2, vec![0])
            } else {
                (5, vec![1])
            }
        } else if command == bs::SERIAL {
            (2, self.serial.as_bytes().to_vec())
        } else {
            self.headset
                .get(&command)
                .map_or((5, Vec::new()), |v| (2, v.clone()))
        };
        let mut response = bs::request(data[2], data[9], command, &body)?;
        response[1] = status;
        response[8] = 128;
        response[11] = 1;
        response[62] = response[1..62].iter().fold(0, |a, b| a ^ b);
        Ok(response.to_vec())
    }
}
struct Model {
    infos: Vec<HidInfo>,
    state: Mutex<State>,
    input: Mutex<VecDeque<Vec<u8>>>,
    available: Condvar,
}
pub struct DemoBackend {
    models: Vec<Arc<Model>>,
}
impl DemoBackend {
    pub fn new() -> Result<Self> {
        Self::with_product_ids(&PRODUCT_IDS)
    }
    pub fn with_product_ids(product_ids: &[u16]) -> Result<Self> {
        let mut models = Vec::new();
        for (index, &pid) in product_ids.iter().enumerate() {
            let meta = database::lookup(0x1532, pid)
                .ok_or_else(|| Error::Invalid(format!("unknown emulated product {pid:04x}")))?;
            let params = recipes().transport_params(meta.driver(), pid)?;
            let control = HidInfo {
                path: format!("emulated:1532:{pid:04x}"),
                vendor_id: 0x1532,
                product_id: pid,
                interface: Some(params.index),
                usage_page: 0xff00,
                usage: 2,
                feature_length: 91,
                product: meta.name.clone(),
                serial: format!("DEMO{:012}", index + 1),
                ..HidInfo::default()
            };
            let mut infos = vec![control.clone()];
            if meta.driver() == "accessory" {
                infos.push(HidInfo {
                    path: format!("{}:argb", control.path),
                    feature_length: 321,
                    ..control.clone()
                });
            }
            if meta.driver() == "kbd" {
                infos.push(HidInfo {
                    path: format!("{}:keys", control.path),
                    interface: Some(1),
                    usage_page: 1,
                    usage: 0,
                    feature_length: 0,
                    input_length: 16,
                    ..control.clone()
                });
            }
            if pid == bs::PID_WIRELESS {
                infos.push(HidInfo {
                    path: format!("{}:headset", control.path),
                    interface: Some(3),
                    usage_page: bs::USAGE_PAGE,
                    usage: 1,
                    feature_length: 0,
                    input_length: 64,
                    output_length: 64,
                    ..control.clone()
                });
            }
            models.push(Arc::new(Model {
                infos,
                state: Mutex::new(State::new(control.serial)),
                input: Mutex::new(VecDeque::new()),
                available: Condvar::new(),
            }));
        }
        Ok(Self { models })
    }
    fn find(&self, info: &HidInfo) -> Result<Arc<Model>> {
        self.models
            .iter()
            .find(|m| m.infos.iter().any(|i| i.path == info.path))
            .cloned()
            .ok_or_else(|| Error::Device("no such emulated device".into()))
    }
}
impl HidBackend for DemoBackend {
    fn enumerate(&self, vid: Option<u16>, pid: Option<u16>) -> Result<Vec<HidInfo>> {
        Ok(self
            .models
            .iter()
            .flat_map(|m| &m.infos)
            .filter(|i| {
                vid.is_none_or(|v| v == i.vendor_id) && pid.is_none_or(|p| p == i.product_id)
            })
            .cloned()
            .collect())
    }
    fn open(&self, info: &HidInfo) -> Result<Box<dyn HidHandle>> {
        Ok(Box::new(Handle {
            model: self.find(info)?,
            info: info.clone(),
        }))
    }
    fn open_input(&self, info: &HidInfo) -> Result<Box<dyn InputReader>> {
        if info.input_length == 0 {
            return Err(Error::Device("collection has no input report".into()));
        }
        Ok(Box::new(Reader(self.find(info)?)))
    }
}
struct Handle {
    model: Arc<Model>,
    info: HidInfo,
}
impl HidHandle for Handle {
    fn send_feature(&mut self, data: &[u8]) -> Result<()> {
        let mut state = self.model.state.lock().unwrap_or_else(|e| e.into_inner());
        if self.info.feature_length == 321 {
            if data.len() != 321 || data[0] != 0 {
                return Err(Error::Invalid("bad emulated ARGB report".into()));
            }
            state.last_frame = data.to_vec();
            return Ok(());
        }
        if data.len() != 91 || data[0] != 0 || data[3..89].iter().fold(0, |a, b| a ^ b) != data[89]
        {
            return Err(Error::Invalid("bad emulated HID report".into()));
        }
        let response = state.answer(Report::unpack(&data[1..])?).pack();
        state.response = std::iter::once(0).chain(response).collect();
        Ok(())
    }
    fn get_feature(&mut self, length: usize, report_id: u8) -> Result<Vec<u8>> {
        if report_id != 0 {
            return Err(Error::Invalid("unsupported report ID".into()));
        }
        let mut response = self
            .model
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .response
            .clone();
        response.resize(length, 0);
        Ok(response)
    }
    fn send_output(&mut self, data: &[u8]) -> Result<()> {
        let reply = self
            .model
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .headset_answer(data)?;
        let mut queue = self.model.input.lock().unwrap_or_else(|e| e.into_inner());
        if queue.len() >= 256 {
            queue.pop_front();
        }
        queue.push_back(reply);
        self.model.available.notify_one();
        Ok(())
    }
}
struct Reader(Arc<Model>);
impl InputReader for Reader {
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        let state = self.0.input.lock().unwrap_or_else(|e| e.into_inner());
        let (mut state, _) = self
            .0
            .available
            .wait_timeout_while(state, timeout, |q| q.is_empty())
            .unwrap_or_else(|e| e.into_inner());
        Ok(state.pop_front())
    }
}
struct Inputs;
impl Injector for Inputs {
    fn emit(&self, _: &[Injection]) -> Result<()> {
        Ok(())
    }
}
impl Launcher for Inputs {
    fn launch(&self, _: &str) -> Result<()> {
        Ok(())
    }
}
impl Environment for Inputs {
    fn audio(&self) -> Box<dyn AudioMeter> {
        Box::new(SilentMeter)
    }
    fn accent(&self) -> Option<Colour> {
        None
    }
}
pub fn service(
    directory: &Path,
    enable_effects: bool,
    enable_keys: bool,
    chroma_port: u16,
) -> Result<Arc<DaemonService>> {
    let stores = Stores::load(directory);
    let backend = Arc::new(DemoBackend::new()?);
    let manager = Arc::new(DeviceManager::new(
        backend.clone(),
        None,
        stores.persistence.clone(),
    ));
    let effects = enable_effects.then(|| {
        EffectEngine::with_environment(
            stores.persistence.clone(),
            KeySource::new(false),
            Arc::new(Inputs),
        )
    });
    let keys = enable_keys.then(|| {
        KeyService::new(
            backend,
            stores.macros.clone(),
            Arc::new(Inputs),
            Arc::new(Inputs),
            false,
        )
    });
    Ok(DaemonService::new(
        manager,
        stores,
        effects,
        keys,
        Options {
            backend_name: "fake-rust".into(),
            chroma_port,
            ..Options::default()
        },
    ))
}
