use super::{
    grid::{self, Grid},
    ChromaError, Result,
};
use crate::{
    effects::{
        engine::{push_frame, EffectEngine},
        frame::{Colour, Frame},
    },
    manager::{lock_device, SharedDevice},
    persistence::{self, SharedStore},
    worker::Cancellation,
};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        mpsc::{self, SyncSender},
        Arc, Mutex, MutexGuard,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const SESSION_TIMEOUT: f64 = 15.0;
pub const VERSION: &str = "3.36.1";
pub type DeviceSource = Arc<dyn Fn() -> Vec<SharedDevice> + Send + Sync>;
pub type ReleaseCallback = Arc<dyn Fn(&SharedDevice) -> crate::Result<()> + Send + Sync>;
pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;
pub trait EffectControl: Send + Sync {
    fn clear_serial(&self, serial: &str);
    fn restore(&self, device: &SharedDevice);
}
impl EffectControl for Mutex<EffectEngine> {
    fn clear_serial(&self, serial: &str) {
        lock(self).clear_serial(serial);
    }
    fn restore(&self, device: &SharedDevice) {
        lock(self).restore(device);
    }
}
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
pub fn kind_of(device_type: &str) -> &str {
    match device_type {
        "keyboard" | "mouse" | "headset" | "keypad" => device_type,
        "mousemat" => "mousepad",
        _ => "chromalink",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    Ears(Vec<Colour>),
    Frame {
        rows: usize,
        columns: usize,
        colours: Vec<Colour>,
    },
    Zones(BTreeMap<String, Colour>),
}
impl Output {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Ears(c) => json!(["ears", c]),
            Self::Frame {
                rows,
                columns,
                colours,
            } => json!(["frame", rows, columns, colours]),
            Self::Zones(z) => json!(["zones", z.iter().collect::<Vec<_>>()]),
        }
    }
}
pub fn output(caps: &Value, kind: &str, look: &Grid) -> Result<Option<Output>> {
    let dimensions: [usize; 2] = if caps["matrix_dimensions"].is_null() {
        [1, 1]
    } else {
        serde_json::from_value(caps["matrix_dimensions"].clone())
            .map_err(|_| ChromaError::Invalid("invalid matrix dimensions".into()))?
    };
    let [rows, columns] = dimensions;
    Frame::new(rows, columns)?;
    let count = caps["zone_order"].as_array().map_or(0, Vec::len);
    if count != 0 && caps["zone_colours"] == true {
        let mut colours = if kind == "headset" {
            grid::strip(look)
        } else {
            grid::resample(&grid::strip(look), count)
        };
        colours.resize(count, colours.first().copied().unwrap_or(grid::BLACK));
        colours.truncate(count);
        return Ok(Some(Output::Ears(colours)));
    }
    if caps["custom_frame"] == true && rows * columns > 1 {
        let colours = if matches!(kind, "keyboard" | "keypad") || (kind == "mouse" && rows > 1) {
            (0..rows)
                .flat_map(|r| (0..columns).map(move |c| grid::at(look, r, c)))
                .collect()
        } else if kind == "mouse" {
            grid::mouse_strip(look, columns)
        } else {
            grid::resample(&grid::strip(look), rows * columns)
        };
        return Ok(Some(Output::Frame {
            rows,
            columns,
            colours,
        }));
    }
    let average = grid::average(&grid::strip(look));
    let zones: BTreeMap<_, _> = caps["zones"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, v)| {
            v["effects"]
                .as_array()
                .is_some_and(|e| e.iter().any(|v| v == "static"))
        })
        .map(|(zone, _)| {
            (
                zone.clone(),
                if kind == "mouse" {
                    grid::mouse_zone(look, zone)
                } else {
                    average
                },
            )
        })
        .collect();
    Ok((!zones.is_empty()).then_some(Output::Zones(zones)))
}

struct Session {
    title: String,
    last_seen: f64,
    shown_at: f64,
    looks: BTreeMap<String, Grid>,
    effects: BTreeMap<String, (String, Grid)>,
    ticks: u64,
}
#[derive(Default)]
struct Sessions {
    next: u64,
    entries: BTreeMap<u64, Session>,
    drawing: BTreeSet<String>,
}
impl Sessions {
    fn get(&mut self, number: u64, now: f64) -> Result<&mut Session> {
        let session = self
            .entries
            .get_mut(&number)
            .ok_or(ChromaError::NoSession)?;
        session.last_seen = now;
        Ok(session)
    }
    fn active(&self) -> Option<&Session> {
        // Preserve the first session if two timestamps are exactly equal.
        self.entries.values().filter(|s| !s.looks.is_empty()).fold(
            None,
            |best: Option<&Session>, s| {
                if best.is_none_or(|b| s.shown_at > b.shown_at) {
                    Some(s)
                } else {
                    best
                }
            },
        )
    }
}
struct Taken {
    device: SharedDevice,
    shown: Option<Output>,
    custom_set: bool,
}
struct Shared {
    sessions: Mutex<Sessions>,
    taken: Mutex<BTreeMap<String, Taken>>,
    errors: Mutex<VecDeque<String>>,
    devices: DeviceSource,
    effects: Option<Arc<dyn EffectControl>>,
    store: Option<SharedStore>,
    on_release: Option<ReleaseCallback>,
    clock: Clock,
    wake: Mutex<Option<SyncSender<()>>>,
}
impl Shared {
    fn wake(&self) {
        if let Some(sender) = lock(&self.wake).as_ref() {
            let _ = sender.try_send(());
        }
    }
    fn error(&self, error: impl ToString) {
        let mut errors = lock(&self.errors);
        if errors.len() == 32 {
            errors.pop_front();
        }
        errors.push_back(error.to_string());
    }
    fn expire(&self) {
        let now = (self.clock)();
        lock(&self.sessions)
            .entries
            .retain(|_, s| now - s.last_seen <= SESSION_TIMEOUT);
    }
    fn release(&self, serial: &str, taken: &mut BTreeMap<String, Taken>) {
        let Some(taken) = taken.remove(serial) else {
            return;
        };
        lock(&self.sessions).drawing.remove(serial);
        if let Some(store) = &self.store {
            persistence::lock(store).release_lighting(serial);
        }
        let restored = { lock_device(&taken.device).restore() };
        if let Err(error) = restored {
            self.error(error);
            return;
        }
        if let Some(effects) = &self.effects {
            effects.restore(&taken.device);
        }
        if let Some(callback) = &self.on_release {
            if let Err(error) = callback(&taken.device) {
                self.error(error);
            }
        }
    }
    fn write(
        &self,
        device: &SharedDevice,
        output: &Output,
        before: Option<&Output>,
        custom_set: &mut bool,
    ) -> crate::Result<()> {
        let mut device = lock_device(device);
        match output {
            Output::Ears(colours) => {
                device.call("set_zone_colours", &[json!(colours)], &Map::new())?;
            }
            Output::Zones(zones) => {
                for (zone, colour) in zones {
                    if matches!(before,Some(Output::Zones(previous)) if previous.get(zone)==Some(colour))
                    {
                        continue;
                    }
                    device.call(
                        "set_static",
                        &colour.iter().map(|c| json!(c)).collect::<Vec<_>>(),
                        &Map::from_iter([("zone".into(), json!(zone))]),
                    )?;
                }
            }
            Output::Frame {
                rows,
                columns,
                colours,
            } => {
                let mut frame = Frame::new(*rows, *columns)?;
                for (index, colour) in colours.iter().enumerate() {
                    frame.set(index / columns, index % columns, *colour);
                }
                push_frame(device.as_mut(), &frame, false, custom_set, false)?;
            }
        }
        Ok(())
    }
    fn paint(
        &self,
        device: &SharedDevice,
        looks: &BTreeMap<String, Grid>,
        taken: &mut BTreeMap<String, Taken>,
    ) -> crate::Result<Option<String>> {
        let (serial, caps, kind) = {
            let mut device = lock_device(device);
            (
                device.serial(),
                device.capabilities(),
                kind_of(&device.info().device_type).to_owned(),
            )
        };
        let Some(look) = looks.get(&kind) else {
            return Ok(None);
        };
        let Some(output) =
            output(&caps, &kind, look).map_err(|e| crate::Error::Invalid(e.to_string()))?
        else {
            return Ok(None);
        };
        if taken
            .get(&serial)
            .is_some_and(|t| !Arc::ptr_eq(&t.device, device))
        {
            self.release(&serial, taken);
        }
        let item = taken.entry(serial.clone()).or_insert_with(|| {
            if let Some(effects) = &self.effects {
                effects.clear_serial(&serial);
            }
            if let Some(store) = &self.store {
                persistence::lock(store).hold_lighting(&serial);
            }
            lock(&self.sessions).drawing.insert(serial.clone());
            Taken {
                device: device.clone(),
                shown: None,
                custom_set: false,
            }
        });
        if item.shown.as_ref() != Some(&output) {
            self.write(device, &output, item.shown.as_ref(), &mut item.custom_set)?;
            item.shown = Some(output);
        }
        Ok(Some(serial))
    }
    fn render(&self) {
        let mut taken = lock(&self.taken);
        let looks = lock(&self.sessions)
            .active()
            .map(|s| s.looks.clone())
            .unwrap_or_default();
        let mut painted = BTreeSet::new();
        for device in (self.devices)() {
            match self.paint(&device, &looks, &mut taken) {
                Ok(Some(serial)) => {
                    painted.insert(serial);
                }
                Ok(None) => (),
                Err(error) => self.error(error),
            }
        }
        let release: Vec<_> = taken
            .keys()
            .filter(|s| !painted.contains(*s))
            .cloned()
            .collect();
        for serial in release {
            self.release(&serial, &mut taken);
        }
    }
}
struct Worker {
    cancel: Arc<Cancellation>,
    thread: JoinHandle<()>,
}
pub struct ChromaBridge {
    shared: Arc<Shared>,
    worker: Mutex<Option<Worker>>,
}
impl ChromaBridge {
    pub fn new(
        devices: DeviceSource,
        effects: Option<Arc<dyn EffectControl>>,
        store: Option<SharedStore>,
        on_release: Option<ReleaseCallback>,
    ) -> Self {
        let started = Instant::now();
        Self::with_clock(
            devices,
            effects,
            store,
            on_release,
            Arc::new(move || started.elapsed().as_secs_f64()),
        )
    }
    pub fn with_clock(
        devices: DeviceSource,
        effects: Option<Arc<dyn EffectControl>>,
        store: Option<SharedStore>,
        on_release: Option<ReleaseCallback>,
        clock: Clock,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                sessions: Mutex::new(Sessions::default()),
                taken: Mutex::new(BTreeMap::new()),
                errors: Mutex::new(VecDeque::new()),
                devices,
                effects,
                store,
                on_release,
                clock,
                wake: Mutex::new(None),
            }),
            worker: Mutex::new(None),
        }
    }
    pub fn start(&self) -> crate::Result<()> {
        let mut worker = lock(&self.worker);
        if worker.is_some() {
            return Ok(());
        }
        let cancel = Arc::new(Cancellation::default());
        let (send, receive) = mpsc::sync_channel(1);
        *lock(&self.shared.wake) = Some(send);
        let (shared, stop) = (self.shared.clone(), cancel.clone());
        let thread = thread::Builder::new()
            .name("mirage-hub-chroma".into())
            .spawn(move || {
                while !stop.stopped() {
                    let _ = receive.recv_timeout(Duration::from_millis(500));
                    if stop.stopped() {
                        break;
                    }
                    shared.expire();
                    shared.render();
                    if stop.wait(Duration::from_secs_f64(1.0 / 30.0)) {
                        break;
                    }
                }
            })?;
        *worker = Some(Worker { cancel, thread });
        Ok(())
    }
    pub fn stop(&self) {
        let mut worker = lock(&self.worker);
        if let Some(worker) = worker.take() {
            worker.cancel.stop();
            self.shared.wake();
            let _ = worker.thread.join();
        }
        lock(&self.shared.wake).take();
        lock(&self.shared.sessions).entries.clear();
        self.release_all();
    }
    pub fn open(&self, info: &Value) -> u64 {
        let title = match &info["title"] {
            Value::String(s) if !s.is_empty() => s.clone(),
            Value::Bool(true) => "True".into(),
            Value::Number(n) if n.as_f64() != Some(0.0) => n.to_string(),
            _ => "A Chroma app".into(),
        };
        let mut sessions = lock(&self.shared.sessions);
        sessions.next += 1;
        let number = sessions.next;
        sessions.entries.insert(
            number,
            Session {
                title: title.chars().take(80).collect(),
                last_seen: (self.shared.clock)(),
                shown_at: f64::NEG_INFINITY,
                looks: BTreeMap::new(),
                effects: BTreeMap::new(),
                ticks: 0,
            },
        );
        number
    }
    pub fn close(&self, number: u64) {
        lock(&self.shared.sessions).entries.remove(&number);
        self.shared.wake();
    }
    pub fn heartbeat(&self, number: u64) -> Result<u64> {
        let mut sessions = lock(&self.shared.sessions);
        let session = sessions.get(number, (self.shared.clock)())?;
        session.ticks += 1;
        Ok(session.ticks)
    }
    pub fn show(&self, number: u64, kind: &str, body: &Value) -> Result<()> {
        let look = grid::parse(kind, body)?;
        {
            let mut sessions = lock(&self.shared.sessions);
            let session = sessions.get(number, (self.shared.clock)())?;
            session.looks.insert(kind.into(), look);
            session.shown_at = (self.shared.clock)();
        }
        self.shared.wake();
        Ok(())
    }
    pub fn create(&self, number: u64, kind: &str, body: &Value) -> Result<String> {
        let look = grid::parse(kind, body)?;
        let mut sessions = lock(&self.shared.sessions);
        let session = sessions.get(number, (self.shared.clock)())?;
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|e| ChromaError::Io(e.to_string()))?;
        bytes[6] = (bytes[6] & 15) | 0x40;
        bytes[8] = (bytes[8] & 63) | 0x80;
        let hex: String = bytes.iter().map(|v| format!("{v:02x}")).collect();
        let id = format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        );
        session.effects.insert(id.clone(), (kind.into(), look));
        Ok(id)
    }
    pub fn apply(&self, number: u64, ids: &[String]) -> Result<()> {
        {
            let mut sessions = lock(&self.shared.sessions);
            let session = sessions.get(number, (self.shared.clock)())?;
            for id in ids {
                let (kind, look) = session
                    .effects
                    .get(id)
                    .ok_or_else(|| ChromaError::Invalid(format!("no effect {id}")))?;
                session.looks.insert(kind.clone(), look.clone());
            }
            session.shown_at = (self.shared.clock)();
        }
        self.shared.wake();
        Ok(())
    }
    pub fn delete(&self, number: u64, ids: &[String]) -> Result<()> {
        let mut sessions = lock(&self.shared.sessions);
        let session = sessions.get(number, (self.shared.clock)())?;
        for id in ids {
            session.effects.remove(id);
        }
        Ok(())
    }
    pub fn expire(&self) {
        self.shared.expire();
    }
    pub fn render(&self) {
        self.shared.render();
    }
    pub fn release(&self, serial: &str) {
        self.shared.release(serial, &mut lock(&self.shared.taken));
    }
    pub fn release_all(&self) {
        let mut taken = lock(&self.shared.taken);
        let serials: Vec<_> = taken.keys().cloned().collect();
        for serial in serials {
            self.shared.release(&serial, &mut taken);
        }
    }
    pub fn drawing(&self, serial: &str) -> bool {
        lock(&self.shared.sessions).drawing.contains(serial)
    }
    pub fn status(&self) -> Value {
        let sessions = lock(&self.shared.sessions);
        json!({"apps":sessions.entries.values().map(|s|&s.title).collect::<Vec<_>>(),"showing":sessions.active().map(|s|&s.title),"drawing":sessions.drawing})
    }
    pub fn errors(&self) -> Vec<String> {
        lock(&self.shared.errors).iter().cloned().collect()
    }
}
impl Drop for ChromaBridge {
    fn drop(&mut self) {
        self.stop();
    }
}
