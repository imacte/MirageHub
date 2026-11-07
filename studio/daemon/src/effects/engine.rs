use super::{
    audio::{self, AudioMeter},
    frame::{Colour, Frame},
    keys::KeySource,
    layers,
    render::{self, Renderer, SOFTWARE_EFFECTS, ZONE_EFFECTS},
};
use crate::{
    device::Device,
    integrations::{read_accent, ACCENT_POLL},
    manager::{lock_device, SharedDevice},
    persistence::{self, SharedStore},
    worker::Cancellation,
    Error, Result,
};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub trait Environment: Send + Sync {
    fn audio(&self) -> Box<dyn AudioMeter>;
    fn accent(&self) -> Option<Colour>;
}
pub struct NativeEnvironment;
impl Environment for NativeEnvironment {
    fn audio(&self) -> Box<dyn AudioMeter> {
        audio::open_meter()
    }
    fn accent(&self) -> Option<Colour> {
        read_accent()
    }
}

/// Keep mode/frame ordering identical to the hardware backends. A BLE colour
/// that follows the Windows accent must remain held even when no frame changes.
pub fn push_frame(
    device: &mut dyn Device,
    frame: &Frame,
    still: bool,
    custom_mode_set: &mut bool,
    cancelled: bool,
) -> Result<()> {
    if cancelled {
        return Ok(());
    }
    let empty = Map::new();
    if still && device.capabilities()["zone_colours"] == true {
        let colours: Vec<_> = (0..frame.columns).map(|c| frame.get(0, c)).collect();
        device.call("set_zone_colours", &[json!(colours)], &empty)?;
        return Ok(());
    }
    if device.custom_mode_first() {
        if !*custom_mode_set {
            device.call("set_custom", &[], &empty)?;
            *custom_mode_set = true;
        }
        device.call("set_key_row", &[json!(frame.payload())], &empty)?;
    } else {
        device.call("set_key_row", &[json!(frame.payload())], &empty)?;
        device.call("set_custom", &[], &empty)?;
    }
    Ok(())
}

struct EffectWorker {
    effect: String,
    options: Map<String, Value>,
    colour: Colour,
    cancel: Arc<Cancellation>,
    handle: Option<JoinHandle<()>>,
    error: Arc<Mutex<Option<String>>>,
}
impl EffectWorker {
    fn start(
        device: SharedDevice,
        mut renderer: Renderer,
        keys: Arc<KeySource>,
        environment: Arc<dyn Environment>,
    ) -> Result<Self> {
        let effect = renderer.effect.clone();
        let options = renderer.options.clone();
        let colour = renderer.colour;
        let cancel = Arc::new(Cancellation::default());
        let stopping = cancel.clone();
        let error = Arc::new(Mutex::new(None));
        let failure = error.clone();
        let subscription = renderer.needs_keys().then(|| keys.subscribe());
        let handle = thread::Builder::new()
            .name(format!("openrazer-effect-{effect}"))
            .spawn(move || {
                let started = Instant::now();
                let mut last_accent = None;
                let mut meter = renderer.needs_audio().then(|| environment.audio());
                let mut custom = false;
                let mut first = true;
                let mut random = (SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64)
                    .max(1);
                while !stopping.stopped() {
                    if !(first && renderer.stack.is_some())
                        && stopping.wait(Duration::from_secs_f64(renderer.interval()))
                    {
                        break;
                    }
                    first = false;
                    if let Some(subscription) = &subscription {
                        for press in subscription.receive.try_iter().take(256) {
                            random ^= random << 13;
                            random ^= random >> 7;
                            random ^= random << 17;
                            renderer.on_key(
                                &press.name,
                                press.at.saturating_duration_since(started).as_secs_f64(),
                                (random >> 11) as f64 / (1u64 << 53) as f64,
                            );
                        }
                    }
                    let accent = if renderer.needs_accent()
                        && last_accent.is_none_or(|at: Instant| at.elapsed() >= ACCENT_POLL)
                    {
                        last_accent = Some(Instant::now());
                        environment.accent()
                    } else {
                        None
                    };
                    let peak = meter.as_mut().map(|meter| meter.peak()).unwrap_or(0.0);
                    if renderer.step(started.elapsed().as_secs_f64(), peak, accent) {
                        let mut device = lock_device(&device);
                        if let Err(error) = push_frame(
                            device.as_mut(),
                            &renderer.frame,
                            renderer.still,
                            &mut custom,
                            stopping.stopped(),
                        ) {
                            *failure.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(error.to_string());
                            break;
                        }
                    }
                }
                // The audio COM interfaces and key subscription drop on this thread.
                drop(meter);
                drop(subscription);
            })?;
        Ok(Self {
            effect,
            options,
            colour,
            cancel,
            handle: Some(handle),
            error,
        })
    }
    fn running(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }
    fn stop(&mut self) {
        self.cancel.stop();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
impl Drop for EffectWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct EffectEngine {
    store: SharedStore,
    pub keys: Arc<KeySource>,
    environment: Arc<dyn Environment>,
    workers: BTreeMap<String, EffectWorker>,
}
impl EffectEngine {
    pub fn new(store: SharedStore) -> Self {
        Self::with_environment(store, KeySource::new(true), Arc::new(NativeEnvironment))
    }
    pub fn with_environment(
        store: SharedStore,
        keys: Arc<KeySource>,
        environment: Arc<dyn Environment>,
    ) -> Self {
        Self {
            store,
            keys,
            environment,
            workers: BTreeMap::new(),
        }
    }
    pub fn stop(&mut self) {
        for worker in self.workers.values() {
            worker.cancel.stop();
        }
        self.workers.clear();
        self.keys.stop();
    }
    pub fn clear_serial(&mut self, serial: &str) {
        self.workers.remove(serial);
    }
    pub fn clear(&mut self, device: &SharedDevice) {
        let serial = lock_device(device).serial();
        self.clear_serial(&serial);
    }
    pub fn active_serials(&self) -> Vec<String> {
        self.workers.keys().cloned().collect()
    }
    pub fn saved(&self, serial: &str) -> Option<Value> {
        let value = persistence::lock(&self.store).get_device(serial, "software", Value::Null);
        value["effect"]
            .as_str()
            .is_some_and(|e| SOFTWARE_EFFECTS.contains(&e))
            .then_some(value)
    }
    pub fn forget(&self, serial: &str) {
        if self.saved(serial).is_some() {
            persistence::lock(&self.store).clear_device(serial, "software");
        }
    }
    pub fn restore(&mut self, device: &SharedDevice) -> bool {
        let serial = lock_device(device).serial();
        let Some(saved) = self.saved(&serial) else {
            return false;
        };
        let effect = saved["effect"].as_str().unwrap();
        self.set_effect(
            device,
            effect,
            saved["options"].as_object().cloned().unwrap_or_default(),
        )
        .is_ok()
    }
    pub fn stop_for(&mut self, device: &SharedDevice) -> Result<bool> {
        let serial = lock_device(device).serial();
        if !self.workers.contains_key(&serial) && self.saved(&serial).is_none() {
            return Ok(false);
        }
        self.set_effect(device, "none", Map::new())?;
        Ok(true)
    }
    pub fn drawing(&self, device: &SharedDevice) -> bool {
        let serial = lock_device(device).serial();
        self.workers.get(&serial).is_some_and(EffectWorker::running)
    }
    pub fn zones_now(&self, device: &SharedDevice) -> Value {
        let (serial, order) = {
            let mut device = lock_device(device);
            (device.serial(), render::zone_order(&device.capabilities()))
        };
        if let Some(worker) = self.workers.get(&serial).filter(|w| w.running()) {
            if worker.effect == "zones" {
                return worker.options["zones"].clone();
            }
            if ZONE_EFFECTS.contains(&worker.effect.as_str()) {
                return Value::Object(
                    order
                        .into_iter()
                        .map(|zone| (zone, json!({"effect":worker.effect,"colour":worker.colour})))
                        .collect(),
                );
            }
        }
        let colours = lock_device(device)
            .call("zone_colours_now", &[], &Map::new())
            .unwrap_or(Value::Null);
        Value::Object(
            order
                .into_iter()
                .enumerate()
                .map(|(i, zone)| {
                    let colour = colours
                        .get(i)
                        .and_then(|value| layers::colour(value).ok())
                        .unwrap_or([0; 3]);
                    (
                        zone,
                        json!({"effect":if colour==[0;3]{"none"}else{"static"},"colour":colour}),
                    )
                })
                .collect(),
        )
    }
    pub fn set_zone_effect(
        &mut self,
        device: &SharedDevice,
        zone: &str,
        effect: &str,
        options: Map<String, Value>,
    ) -> Result<Value> {
        let order = render::zone_order(&lock_device(device).capabilities());
        if !order.iter().any(|name| name == zone) {
            return Err(Error::NotSupported(format!(
                "device cannot light {zone} on its own"
            )));
        }
        let effect = if ["off", ""].contains(&effect) {
            "none"
        } else {
            effect
        };
        if !ZONE_EFFECTS.contains(&effect) {
            return Err(Error::NotSupported(format!(
                "{effect} cannot be drawn on one zone"
            )));
        }
        let mut zones = self.zones_now(device);
        zones[zone] = json!({"effect":effect,"colour":options.get("colour")});
        let mut settings = Map::new();
        settings.insert("zones".into(), zones);
        if let Some(refresh) = options.get("refresh_rate") {
            settings.insert("refresh_rate".into(), refresh.clone());
        }
        self.set_effect(device, "zones", settings)
    }
    fn show_still(
        &mut self,
        device: &SharedDevice,
        serial: &str,
        order: &[String],
        zones: Value,
    ) -> Result<Value> {
        self.clear_serial(serial);
        self.forget(serial);
        let colours: Vec<_> = order
            .iter()
            .map(|zone| {
                if zones[zone]["effect"] == "static" {
                    zones[zone]["colour"].clone()
                } else {
                    json!([0, 0, 0])
                }
            })
            .collect();
        let mut device = lock_device(device);
        if device.capabilities()["zone_colours"] == true {
            device.call("set_zone_colours", &[json!(colours)], &Map::new())?;
        } else {
            for (name, colour) in order.iter().zip(colours) {
                device.call(
                    "set_static",
                    colour.as_array().unwrap(),
                    &Map::from_iter([("zone".into(), json!(name))]),
                )?;
            }
        }
        Ok(json!({"effect":null,"zones":zones}))
    }
    pub fn set_effect(
        &mut self,
        device: &SharedDevice,
        effect: &str,
        mut options: Map<String, Value>,
    ) -> Result<Value> {
        if let Some(zone) = options.remove("zone").filter(|v| !v.is_null()) {
            let zone = zone
                .as_str()
                .ok_or_else(|| Error::Invalid("zone must be a name".into()))?;
            if !["", "backlight"].contains(&zone) {
                return self.set_zone_effect(device, zone, effect, options);
            }
        }
        let (serial, capabilities) = {
            let mut device = lock_device(device);
            (device.serial(), device.capabilities())
        };
        if ["none", "off", ""].contains(&effect) {
            self.clear_serial(&serial);
            self.forget(&serial);
            return Ok(json!({"effect":null}));
        }
        if !SOFTWARE_EFFECTS.contains(&effect) {
            return Err(Error::NotSupported(format!(
                "unknown software effect: {effect}"
            )));
        }
        if effect == "zones" {
            let order = render::zone_order(&capabilities);
            let zones = render::clean_zones(&order, options.get("zones").unwrap_or(&Value::Null))?;
            if order
                .iter()
                .all(|name| ["none", "static"].contains(&zones[name]["effect"].as_str().unwrap()))
            {
                return self.show_still(device, &serial, &order, zones);
            }
            options.insert("zones".into(), zones);
        }
        if !render::can_render(&capabilities) {
            return Err(Error::NotSupported(
                "device has no addressable matrix big enough for software effects".into(),
            ));
        }
        if ["ripple", "ripple_random"].contains(&effect) && !render::can_render_keys(&capabilities)
        {
            return Err(Error::NotSupported(
                "a ripple needs at least eight addressable cells".into(),
            ));
        }
        if effect == "layers" {
            let [rows, columns] = render::dimensions(&capabilities);
            options.insert(
                "layers".into(),
                serde_json::to_value(layers::clean_layers(
                    options.get("layers").unwrap_or(&Value::Null),
                    rows,
                    columns,
                )?)?,
            );
        }
        let renderer = Renderer::new(effect, options.clone(), &capabilities)?;
        let refresh = renderer.refresh;
        self.clear_serial(&serial);
        let worker = EffectWorker::start(
            device.clone(),
            renderer,
            self.keys.clone(),
            self.environment.clone(),
        )?;
        self.workers.insert(serial.clone(), worker);
        persistence::lock(&self.store).set_device(
            &serial,
            "software",
            json!({"effect":effect,"options":options}),
        );
        Ok(json!({"effect":effect,"refresh_rate":refresh,"keyboard_hook":self.keys.running()}))
    }
    pub fn status(&self) -> Value {
        let active: Map<String, Value> = self
            .workers
            .iter()
            .filter(|(_, w)| w.running())
            .map(|(s, w)| (s.clone(), json!(w.effect)))
            .collect();
        let options: Map<String, Value> = self
            .workers
            .iter()
            .filter(|(_, w)| w.running())
            .map(|(s, w)| (s.clone(), json!(w.options)))
            .collect();
        json!({"enabled":true,"active":active,"options":options,"keyboard_hook":self.keys.running(),"available":SOFTWARE_EFFECTS})
    }
    pub fn errors(&self) -> BTreeMap<String, String> {
        self.workers
            .iter()
            .filter_map(|(s, w)| {
                w.error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                    .map(|error| (s.clone(), error))
            })
            .collect()
    }
}
impl Drop for EffectEngine {
    fn drop(&mut self) {
        self.stop();
    }
}
