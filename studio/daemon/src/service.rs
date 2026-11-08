//! Service composition and the explicitly allowlisted API used by Studio.
//! Lifecycle locks never protect the device registry or daemon.version: radio
//! scans and hardware retries must not make a live daemon look unavailable.
use crate::{
    chroma::{bridge::EffectControl, ChromaBridge, ChromaServer},
    effects::{engine::EffectEngine, frame::Colour, render::zone_order},
    integrations::{self, AccentWatch, IntegrationSettings},
    keys::{codes::parse_sequence, KeyService, MacroStore},
    manager::{lock_device, DeviceManager, SharedDevice},
    persistence::{self, Persistence, SharedStore},
    profiles::{self, ProfileStore},
    rpc::{self, RpcError, Service, DEVICE_NOT_FOUND, METHOD_NOT_FOUND, NOT_SUPPORTED},
    Result,
};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

pub const HOTPLUG_INTERVAL: Duration = Duration::from_secs(3);
pub const UPSTREAM_VERSION: &str = include_str!("../../../data/upstream-version.txt").trim_ascii();
pub const DEVICE_METHODS: &[&str] = &[
    "set_none",
    "set_on",
    "set_static",
    "set_spectrum",
    "set_wave",
    "set_wheel",
    "set_reactive",
    "trigger_reactive",
    "set_blinking",
    "set_custom",
    "set_breath_random",
    "set_breath_single",
    "set_breath_dual",
    "set_breath_triple",
    "set_starlight_random",
    "set_starlight_single",
    "set_starlight_dual",
    "set_brightness",
    "get_brightness",
    "set_key_row",
    "set_custom_frame",
    "set_zone_colours",
    "get_dpi",
    "set_dpi",
    "get_dpi_stages",
    "set_dpi_stages",
    "get_poll_rate",
    "set_poll_rate",
    "supported_poll_rates",
    "get_scroll_mode",
    "set_scroll_mode",
    "get_scroll_acceleration",
    "set_scroll_acceleration",
    "get_scroll_smart_reel",
    "set_scroll_smart_reel",
    "get_battery_level",
    "is_charging",
    "get_idle_time",
    "set_idle_time",
    "get_low_battery_threshold",
    "set_low_battery_threshold",
    "get_audio",
    "get_microphone_muted",
    "set_sidetone",
    "set_sidetone_level",
    "set_equalizer",
    "set_equalizer_preset",
    "set_equalizer_bands",
    "set_do_not_disturb",
    "set_dongle_led",
    "get_game_mode",
    "set_game_mode",
    "get_macro_mode",
    "set_macro_mode",
    "set_macro_effect",
    "set_fn_toggle",
    "get_keyboard_layout",
    "get_device_mode",
    "set_device_mode",
    "restore",
    "available_effects",
];
const HARDWARE_EFFECTS: &[&str] = &[
    "set_none",
    "set_on",
    "set_static",
    "set_spectrum",
    "set_wave",
    "set_wheel",
    "set_reactive",
    "set_blinking",
    "set_breath_random",
    "set_breath_single",
    "set_breath_dual",
    "set_breath_triple",
    "set_starlight_random",
    "set_starlight_single",
    "set_starlight_dual",
    "set_zone_colours",
];
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}

pub trait Registry: Send + Sync {
    fn devices(&self) -> Vec<SharedDevice>;
    fn scan(&self, force_bluetooth: bool) -> Result<Vec<SharedDevice>>;
    fn scan_usb(&self) -> Result<Vec<SharedDevice>> {
        self.scan(false)
    }
    fn first_sweep_pending(&self) -> bool {
        false
    }
    fn hid_infos(&self) -> Option<Vec<crate::hid::HidInfo>> {
        None
    }
    fn unsupported(&self) -> Vec<(u16, String)>;
    fn close(&self) -> Result<()>;
    fn warnings(&self) -> Vec<String> {
        Vec::new()
    }
}
impl Registry for DeviceManager {
    fn scan_usb(&self) -> Result<Vec<SharedDevice>> {
        self.scan_usb()
    }
    fn first_sweep_pending(&self) -> bool {
        self.first_sweep_pending()
    }
    fn hid_infos(&self) -> Option<Vec<crate::hid::HidInfo>> {
        Some(self.hid_infos())
    }
    fn devices(&self) -> Vec<SharedDevice> {
        self.devices()
    }
    fn scan(&self, force_bluetooth: bool) -> Result<Vec<SharedDevice>> {
        self.scan(force_bluetooth)
    }
    fn unsupported(&self) -> Vec<(u16, String)> {
        self.unsupported()
    }
    fn close(&self) -> Result<()> {
        self.close()
    }
    fn warnings(&self) -> Vec<String> {
        self.warnings()
    }
}

pub struct Stores {
    pub persistence: SharedStore,
    pub profiles: ProfileStore,
    pub macros: Arc<MacroStore>,
    pub integrations: IntegrationSettings,
}
impl Stores {
    pub fn load(directory: &Path) -> Self {
        Self {
            persistence: Persistence::load(directory.join("persistence.json")).shared(),
            profiles: ProfileStore::load(directory.join("profiles.json")),
            macros: Arc::new(MacroStore::load(directory.join("macros.json"))),
            integrations: IntegrationSettings::load(directory.join("integrations.json")),
        }
    }
    pub fn memory() -> Self {
        Self {
            persistence: Persistence::memory().shared(),
            profiles: ProfileStore::memory(),
            macros: Arc::new(MacroStore::memory()),
            integrations: IntegrationSettings::memory(),
        }
    }
}
pub struct Options {
    pub backend_name: String,
    pub chroma_port: u16,
    pub retry_delays: [Duration; 4],
}
impl Default for Options {
    fn default() -> Self {
        Self {
            backend_name: "windows-rust".into(),
            chroma_port: crate::chroma::rest::PORT,
            retry_delays: [5, 15, 30, 60].map(Duration::from_secs),
        }
    }
}
struct Pending {
    device: SharedDevice,
    name: String,
    attempts: usize,
    retry_at: Instant,
}
pub struct DaemonService {
    // Drop the accent worker before its captured Chroma/effects references.
    accent: Mutex<AccentWatch>,
    chroma_server: Mutex<Option<ChromaServer>>,
    chroma: Arc<ChromaBridge>,
    effects: Option<Arc<Mutex<EffectEngine>>>,
    keys: Option<Mutex<KeyService>>,
    registry: Arc<dyn Registry>,
    persistence: SharedStore,
    profiles: Mutex<ProfileStore>,
    macros: Arc<MacroStore>,
    integrations: Mutex<IntegrationSettings>,
    integration_gate: Mutex<()>,
    accent_enabled: Arc<AtomicBool>,
    accent_colour: Arc<Mutex<Option<Colour>>>,
    chroma_error: Mutex<Option<String>>,
    options: Options,
    scan_gate: Mutex<()>,
    started: AtomicBool,
    cleaned: AtomicBool,
    state: AtomicU8,
    pending: Mutex<BTreeMap<usize, Pending>>,
    warnings: Mutex<VecDeque<String>>,
    created: Instant,
}

/// Effects are stopped before taking the device lock; joining a renderer while
/// holding that lock would wait forever for the renderer's final hardware call.
fn paint(
    device: &SharedDevice,
    colour: Colour,
    effects: Option<&Mutex<EffectEngine>>,
    chroma: Option<&ChromaBridge>,
) -> Result<()> {
    let (serial, caps) = {
        let mut d = lock_device(device);
        (d.serial(), d.capabilities())
    };
    if chroma.is_some_and(|c| c.drawing(&serial)) {
        return Ok(());
    }
    let zones: Vec<_> = caps["zones"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| {
            value["effects"]
                .as_array()
                .is_some_and(|e| e.iter().any(|v| v == "static"))
        })
        .map(|(zone, _)| zone.clone())
        .collect();
    if zones.is_empty() {
        return Ok(());
    }
    if let Some(effects) = effects {
        lock(effects).stop_for(device)?;
    }
    let mut device = lock_device(device);
    if caps["zone_colours"] == true {
        device.call("set_zone_colours", &[json!([colour])], &Map::new())?;
    } else {
        for zone in zones {
            device.call(
                "set_static",
                &colour.map(|v| json!(v)),
                &Map::from_iter([("zone".into(), json!(zone))]),
            )?;
        }
    }
    Ok(())
}
impl DaemonService {
    pub fn new(
        registry: Arc<dyn Registry>,
        stores: Stores,
        effects: Option<EffectEngine>,
        keys: Option<KeyService>,
        options: Options,
    ) -> Arc<Self> {
        let effects = effects.map(|e| Arc::new(Mutex::new(e)));
        let accent_enabled = Arc::new(AtomicBool::new(false));
        let accent_colour = Arc::new(Mutex::new(None));
        let source = registry.clone();
        let (enabled, colour, render, store) = (
            accent_enabled.clone(),
            accent_colour.clone(),
            effects.clone(),
            stores.persistence.clone(),
        );
        let chroma = Arc::new(ChromaBridge::new(
            Arc::new(move || source.devices()),
            effects
                .as_ref()
                .map(|e| e.clone() as Arc<dyn EffectControl>),
            Some(stores.persistence.clone()),
            Some(Arc::new(move |device| {
                let colour = *lock(&colour);
                if enabled.load(Ordering::Acquire) {
                    if let Some(colour) = colour {
                        paint(device, colour, render.as_deref(), None)?;
                        persistence::lock(&store).save()?;
                    }
                }
                Ok(())
            })),
        ));
        let (source, render, store, game, colour) = (
            registry.clone(),
            effects.clone(),
            stores.persistence.clone(),
            chroma.clone(),
            accent_colour.clone(),
        );
        let accent = AccentWatch::new(Arc::new(move |value| {
            *lock(&colour) = Some(value);
            for device in source.devices() {
                let _ = paint(&device, value, render.as_deref(), Some(&game));
            }
            persistence::lock(&store).save()
        }));
        Arc::new(Self {
            accent: Mutex::new(accent),
            chroma_server: Mutex::new(None),
            chroma,
            effects,
            keys: keys.map(Mutex::new),
            registry,
            persistence: stores.persistence,
            profiles: Mutex::new(stores.profiles),
            macros: stores.macros,
            integrations: Mutex::new(stores.integrations),
            integration_gate: Mutex::new(()),
            accent_enabled,
            accent_colour,
            chroma_error: Mutex::new(None),
            options,
            scan_gate: Mutex::new(()),
            started: AtomicBool::new(false),
            cleaned: AtomicBool::new(false),
            state: AtomicU8::new(0),
            pending: Mutex::new(BTreeMap::new()),
            warnings: Mutex::new(VecDeque::new()),
            created: Instant::now(),
        })
    }
    #[cfg(windows)]
    pub fn native(directory: &Path, enable_effects: bool, enable_keys: bool) -> Arc<Self> {
        Self::native_with_options(directory, enable_effects, enable_keys, Options::default())
    }
    #[cfg(windows)]
    pub fn native_with_options(
        directory: &Path,
        enable_effects: bool,
        enable_keys: bool,
        options: Options,
    ) -> Arc<Self> {
        let stores = Stores::load(directory);
        let hid: Arc<dyn crate::hid::HidBackend> = Arc::new(crate::hid::windows::NativeHid);
        let manager = Arc::new(DeviceManager::new(
            hid.clone(),
            Some(Arc::new(crate::ble::windows::NativeBle)),
            stores.persistence.clone(),
        ));
        let effects = enable_effects.then(|| EffectEngine::new(stores.persistence.clone()));
        let keys = enable_keys.then(|| KeyService::native(hid, stores.macros.clone()));
        Self::new(manager, stores, effects, keys, options)
    }
    pub fn chroma(&self) -> &Arc<ChromaBridge> {
        &self.chroma
    }
    pub fn warnings(&self) -> Vec<String> {
        lock(&self.warnings).iter().cloned().collect()
    }
    fn warn(&self, error: impl ToString) {
        let error = error.to_string();
        log::warn!("{error}");
        let mut warnings = lock(&self.warnings);
        if warnings.len() >= 64 {
            warnings.pop_front();
        }
        warnings.push_back(error);
    }
    pub fn start(&self) {
        let _scan = lock(&self.scan_gate);
        if self.state.load(Ordering::Acquire) == 2 || self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(keys) = &self.keys {
            if let Err(e) = lock(keys).start() {
                self.warn(e);
            }
        }
        match self.registry.scan_usb() {
            Ok(devices) => {
                for device in devices {
                    self.initialise(&device);
                }
            }
            Err(error) => self.warn(error),
        }
        for warning in self.registry.warnings() {
            self.warn(warning);
        }
        {
            let _integrations = lock(&self.integration_gate);
            if self.state.load(Ordering::Acquire) == 2 {
                return;
            }
            for name in integrations::NAMES {
                let enabled = lock(&self.integrations).enabled(name);
                if let Err(error) = self.toggle(name, enabled) {
                    self.warn(error);
                }
            }
        }
        let _ = self
            .state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
    }
    fn initialise(&self, device: &SharedDevice) -> bool {
        if self.state.load(Ordering::Acquire) == 2 {
            return false;
        }
        let result = (|| -> Result<()> {
            let handled = match &self.keys {
                Some(keys) => {
                    lock(keys).attach_with_infos(device, self.registry.hid_infos().as_deref())?
                }
                None => false,
            };
            {
                let mut d = lock_device(device);
                d.enter_driver_mode(handled)?;
                d.restore()?;
            }
            if let Some(effects) = &self.effects {
                lock(effects).restore(device);
            }
            Ok(())
        })();
        let id = Arc::as_ptr(device) as usize;
        match result {
            Ok(()) => {
                lock(&self.pending).remove(&id);
                true
            }
            Err(error) => {
                let name = lock_device(device).info().name.clone();
                let mut pending = lock(&self.pending);
                let attempts = pending.get(&id).map_or(1, |p| p.attempts.saturating_add(1));
                let delay = self.options.retry_delays[(attempts - 1).min(3)];
                self.warn(format!(
                    "could not set up {name}: {error}; retrying in {}s",
                    delay.as_secs()
                ));
                pending.insert(
                    id,
                    Pending {
                        device: device.clone(),
                        name,
                        attempts,
                        retry_at: Instant::now() + delay,
                    },
                );
                false
            }
        }
    }
    pub fn poll_hotplug(&self, force_bluetooth: bool) -> Result<()> {
        let _scan = lock(&self.scan_gate);
        if self.state.load(Ordering::Acquire) == 2 {
            return Ok(());
        }
        let before = self.registry.devices();
        let current = self.registry.scan(force_bluetooth)?;
        for warning in self.registry.warnings() {
            self.warn(warning);
        }
        if self.state.load(Ordering::Acquire) == 2 {
            return Ok(());
        }
        // Detach the old object before attaching a replacement with the same
        // serial, otherwise a stale key binding/renderer would keep its handle.
        for device in &before {
            if current.iter().any(|d| Arc::ptr_eq(d, device)) {
                continue;
            }
            if let Some(keys) = &self.keys {
                lock(keys).detach(device);
            }
            let serial = lock_device(device).serial();
            self.chroma.release(&serial);
            if let Some(effects) = &self.effects {
                lock(effects).clear_serial(&serial);
            }
        }
        let ready_to_retry: Vec<_> = {
            let mut pending = lock(&self.pending);
            pending.retain(|_, p| current.iter().any(|d| Arc::ptr_eq(d, &p.device)));
            pending
                .values()
                .filter(|p| Instant::now() >= p.retry_at)
                .map(|p| p.device.clone())
                .collect()
        };
        for device in &current {
            if !before.iter().any(|d| Arc::ptr_eq(d, device)) && self.initialise(device) {
                self.paint_latest_accent(device);
            }
        }
        for device in ready_to_retry {
            if self.initialise(&device) {
                self.paint_latest_accent(&device);
            }
        }
        Ok(())
    }
    pub fn first_sweep_pending(&self) -> bool {
        self.registry.first_sweep_pending()
    }
    fn paint_latest_accent(&self, device: &SharedDevice) {
        if self.accent_enabled.load(Ordering::Acquire) {
            let colour = *lock(&self.accent_colour);
            if let Some(colour) = colour {
                if let Err(error) =
                    paint(device, colour, self.effects.as_deref(), Some(&self.chroma))
                {
                    self.warn(error);
                }
            }
        }
    }
    /// Mark stopping immediately, even while a radio sweep is in progress.
    /// The runtime calls this before joining its hotplug worker.
    pub fn request_stop(&self) {
        self.state.store(2, Ordering::Release);
    }
    pub fn stop(&self) {
        self.request_stop();
        let _scan = lock(&self.scan_gate);
        if self.cleaned.swap(true, Ordering::AcqRel) {
            return;
        }
        let _integrations = lock(&self.integration_gate);
        let _ = self.toggle("accent", false);
        let _ = self.toggle("chroma", false);
        if let Some(effects) = &self.effects {
            lock(effects).stop();
        }
        for device in self.registry.devices() {
            if let Err(e) = lock_device(&device).leave_driver_mode() {
                self.warn(e);
            }
        }
        if let Some(keys) = &self.keys {
            lock(keys).stop();
        }
        if let Err(e) = self.registry.close() {
            self.warn(e);
        }
        lock(&self.pending).clear();
    }
    fn toggle(&self, name: &str, enabled: bool) -> Result<()> {
        if name == "accent" {
            self.accent_enabled.store(enabled, Ordering::Release);
            if enabled {
                lock(&self.accent).start()?;
            } else {
                lock(&self.accent).stop();
                *lock(&self.accent_colour) = None;
            }
        } else if name == "chroma" {
            let mut server = lock(&self.chroma_server);
            if !enabled {
                if let Some(mut server) = server.take() {
                    server.stop();
                }
                self.chroma.stop();
            } else if server.is_none() {
                match ChromaServer::start(self.chroma.clone(), self.options.chroma_port) {
                    Ok(listener) => {
                        if let Err(error) = self.chroma.start() {
                            *lock(&self.chroma_error) = Some(error.to_string());
                            return Err(error);
                        }
                        *lock(&self.chroma_error) = None;
                        *server = Some(listener);
                    }
                    Err(error) => {
                        let message = format!(
                            "cannot listen for Chroma on port {}: {error}",
                            self.options.chroma_port
                        );
                        *lock(&self.chroma_error) = Some(message.clone());
                        self.warn(message);
                    }
                }
            }
        }
        Ok(())
    }
    fn integration_status(&self) -> Value {
        let switches = lock(&self.integrations).as_json();
        let colour = integrations::read_accent();
        let server = lock(&self.chroma_server);
        let mut chroma = self.chroma.status();
        let data = chroma.as_object_mut().expect("Chroma status is an object");
        data.extend(Map::from_iter([
            ("enabled".into(), switches["chroma"].clone()),
            (
                "listening".into(),
                json!(server.as_ref().is_some_and(|s| s.listening())),
            ),
            (
                "port".into(),
                json!(server
                    .as_ref()
                    .map_or(self.options.chroma_port, |s| s.address().port())),
            ),
            ("error".into(), json!(*lock(&self.chroma_error))),
        ]));
        json!({"accent":{"enabled":switches["accent"],"available":colour.is_some(),"colour":colour},"chroma":chroma})
    }
    fn device(&self, serial: &str) -> rpc::Result<SharedDevice> {
        let devices = self.registry.devices();
        for device in &devices {
            if lock_device(device).serial() == serial {
                return Ok(device.clone());
            }
        }
        let needle = serial.trim().to_lowercase();
        let mut found = Vec::new();
        for device in devices {
            let mut d = lock_device(&device);
            if d.serial().to_lowercase() == needle || format!("{:04x}", d.info().pid) == needle {
                drop(d);
                return Ok(device);
            }
            if d.info().name.to_lowercase().contains(&needle) {
                found.push(device.clone());
            }
        }
        if found.len() == 1 {
            Ok(found.remove(0))
        } else {
            Err(RpcError::new(
                DEVICE_NOT_FOUND,
                format!("no such device: {serial}"),
            ))
        }
    }
    fn list(&self) -> Value {
        json!(self.registry.devices().iter().map(|device| {
            let mut d = lock_device(device);
            let (serial, firmware) = (d.serial(), d.firmware().ok());
            let info = d.info();
            json!({"serial":serial,"name":info.name,"type":info.device_type,"vid":info.vid,"pid":info.pid,
                "image":info.image,"images":info.images,"firmware":firmware})
        }).collect::<Vec<_>>())
    }
    fn call_device(&self, p: &Parameters<'_>) -> rpc::Result<Value> {
        let method = p.string("method")?;
        if !DEVICE_METHODS.contains(&method) {
            return Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("method not exposed over RPC: {method}"),
            ));
        }
        let device = self.device(p.string("serial")?)?;
        let args = p.array("args")?;
        let kwargs = p.object("kwargs")?;
        let zone = kwargs
            .get("zone")
            .and_then(Value::as_str)
            .unwrap_or("backlight");
        if let Some(effects) = &self.effects {
            let mut engine = lock(effects);
            let individual = matches!(method, "set_static" | "set_none")
                && engine.drawing(&device)
                && zone_order(&lock_device(&device).capabilities())
                    .iter()
                    .any(|v| v == zone);
            if individual {
                let colour: Vec<_> = args
                    .iter()
                    .cloned()
                    .chain(
                        ["red", "green", "blue"]
                            .map(|k| kwargs.get(k).cloned().unwrap_or(Value::Null)),
                    )
                    .take(3)
                    .collect();
                engine
                    .set_zone_effect(
                        &device,
                        zone,
                        if method == "set_static" {
                            "static"
                        } else {
                            "none"
                        },
                        Map::from_iter([(
                            "colour".into(),
                            if method == "set_static" {
                                json!(colour)
                            } else {
                                Value::Null
                            },
                        )]),
                    )
                    .map_err(RpcError::invalid)?;
                persistence::lock(&self.persistence).save()?;
                return Ok(Value::Null);
            }
            if HARDWARE_EFFECTS.contains(&method) && zone == "backlight" {
                engine.stop_for(&device)?;
            }
        }
        let value = lock_device(&device).call(method, &args, &kwargs)?;
        persistence::lock(&self.persistence).save()?;
        Ok(value)
    }
}

struct Parameters<'a>(&'a Map<String, Value>);
impl Parameters<'_> {
    fn check(&self, required: &[&str], optional: &[&str]) -> rpc::Result<()> {
        for key in required {
            if !self.0.contains_key(*key) {
                return Err(RpcError::invalid(format!("missing argument {key}")));
            }
        }
        for key in self.0.keys() {
            if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
                return Err(RpcError::invalid(format!("unexpected argument {key}")));
            }
        }
        Ok(())
    }
    fn string(&self, key: &str) -> rpc::Result<&str> {
        self.0
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid(format!("{key} must be a string")))
    }
    fn value(&self, key: &str) -> &Value {
        self.0.get(key).unwrap_or(&Value::Null)
    }
    fn object(&self, key: &str) -> rpc::Result<Map<String, Value>> {
        let v = self.value(key);
        if !rpc::truthy(v) {
            return Ok(Map::new());
        }
        v.as_object()
            .cloned()
            .ok_or_else(|| RpcError::invalid(format!("{key} must be an object")))
    }
    fn array(&self, key: &str) -> rpc::Result<Vec<Value>> {
        let v = self.value(key);
        if !rpc::truthy(v) {
            return Ok(Vec::new());
        }
        v.as_array()
            .cloned()
            .ok_or_else(|| RpcError::invalid(format!("{key} must be an array")))
    }
}
impl Service for DaemonService {
    fn dispatch(&self, method: &str, params: &Map<String, Value>) -> rpc::Result<Value> {
        let p = Parameters(params);
        let (required, optional): (&[&str], &[&str]) = match method {
            "daemon.version"
            | "daemon.rescan"
            | "daemon.persistence"
            | "devices.list"
            | "devices.unsupported"
            | "effect.status"
            | "integration.status"
            | "keys.status"
            | "macro.record_stop" => (&[], &[]),
            "device.capabilities" | "profile.list" | "macro.list" => (&["serial"], &[]),
            "device.call" => (&["serial", "method"], &["args", "kwargs"]),
            "effect.set" => (&["serial", "effect"], &["options"]),
            "integration.set" => (&["name", "enabled"], &[]),
            "profile.save" | "profile.load" | "profile.delete" => (&["serial", "name"], &[]),
            "macro.set" => (&["serial", "key", "action"], &[]),
            "macro.delete" | "macro.record_start" | "macro.play" => (&["serial", "key"], &[]),
            _ => {
                return Err(RpcError::new(
                    METHOD_NOT_FOUND,
                    format!("unknown method: {method}"),
                ))
            }
        };
        p.check(required, optional)?;
        match method {
            "daemon.version" => {
                let mut pending: Vec<_> = lock(&self.pending)
                    .values()
                    .map(|v| v.name.clone())
                    .collect();
                pending.sort();
                Ok(
                    json!({"version":env!("CARGO_PKG_VERSION"),"upstream":UPSTREAM_VERSION,"pid":std::process::id(),
                    "uptime":(self.created.elapsed().as_secs_f64()*10.0).round()/10.0,"backend":self.options.backend_name,
                    "devices":self.registry.devices().len(),"state":match self.state.load(Ordering::Acquire) {0=>"starting",2=>"stopping",_=>"ready"},"pending":pending,
                    "first_sweep_pending":self.first_sweep_pending()}),
                )
            }
            "devices.list" => Ok(self.list()),
            "devices.unsupported" => Ok(json!(self
                .registry
                .unsupported()
                .into_iter()
                .map(|(pid, product)| json!({"pid":pid,"product":product}))
                .collect::<Vec<_>>())),
            "daemon.rescan" => {
                self.poll_hotplug(true)?;
                Ok(self.list())
            }
            "daemon.persistence" => Ok(persistence::lock(&self.persistence).as_json()),
            "device.capabilities" => {
                Ok(lock_device(&self.device(p.string("serial")?)?).capabilities())
            }
            "device.call" => self.call_device(&p),
            "effect.set" => {
                let effects = self
                    .effects
                    .as_ref()
                    .ok_or_else(|| RpcError::new(NOT_SUPPORTED, "software effects are disabled"))?;
                let device = self.device(p.string("serial")?)?;
                let result =
                    lock(effects).set_effect(&device, p.string("effect")?, p.object("options")?)?;
                persistence::lock(&self.persistence).save()?;
                Ok(result)
            }
            "effect.status" => Ok(self.effects.as_ref().map_or_else(
                || json!({"enabled":false,"active":{}}),
                |e| lock(e).status(),
            )),
            "integration.status" => Ok(self.integration_status()),
            "integration.set" => {
                let _gate = lock(&self.integration_gate);
                if self.state.load(Ordering::Acquire) == 2 {
                    return Err(RpcError::invalid("daemon is stopping"));
                }
                let name = p.string("name")?;
                let enabled = rpc::truthy(p.value("enabled"));
                lock(&self.integrations).set(name, enabled)?;
                self.toggle(name, enabled)?;
                Ok(self.integration_status())
            }
            "keys.status" => Ok(self.keys.as_ref().map_or_else(
                || json!({"running":false,"devices":[],"recording":null}),
                |k| lock(k).status(&self.registry.devices()),
            )),
            "macro.record_stop" => match &self.keys {
                Some(keys) => Ok(lock(keys).stop_recording()?.unwrap_or(Value::Null)),
                None => Ok(Value::Null),
            },
            "macro.list" => {
                let device = self.device(p.string("serial")?)?;
                let (mut keys, handled) = self.keys.as_ref().map_or((Vec::new(), false), |k| {
                    let k = lock(k);
                    (k.macro_keys(&device), k.handles(&device))
                });
                let mut d = lock_device(&device);
                if keys.is_empty() && d.info().dedicated_macro_keys {
                    keys = (1..=5).map(|i| format!("M{i}")).collect();
                }
                Ok(
                    json!({"keys":keys,"bindings":self.macros.bindings(&d.serial()),"handled":handled}),
                )
            }
            "macro.set" | "macro.delete" | "macro.record_start" | "macro.play" => {
                let device = self.device(p.string("serial")?)?;
                let serial = lock_device(&device).serial();
                let key = p.string("key")?;
                match method {
                    "macro.set" => {
                        let mut action = p.value("action").clone();
                        if action["type"] == "keys"
                            && action.get("events").is_none()
                            && action.get("sequence").is_some()
                        {
                            let sequence = action["sequence"]
                                .as_str()
                                .ok_or_else(|| RpcError::invalid("sequence must be a string"))?;
                            action = json!({"type":"keys","events":parse_sequence(sequence)?});
                        }
                        Ok(self.macros.set(&serial, key, &action)?)
                    }
                    "macro.delete" => Ok(json!({"deleted":self.macros.delete(&serial,key)?})),
                    "macro.record_start" => {
                        let keys = self.keys.as_ref().ok_or_else(|| {
                            RpcError::new(NOT_SUPPORTED, "macro keys are not being handled")
                        })?;
                        let keys = lock(keys);
                        if !keys.handles(&device) {
                            return Err(RpcError::new(
                                NOT_SUPPORTED,
                                "keys of this device are not being handled",
                            ));
                        }
                        keys.start_recording(&serial, key)?;
                        Ok(json!({"recording":key}))
                    }
                    _ => {
                        let action = self.macros.get(&serial, key).ok_or_else(|| {
                            RpcError::invalid(format!("nothing is bound to {key}"))
                        })?;
                        let keys = self.keys.as_ref().ok_or_else(|| {
                            RpcError::new(NOT_SUPPORTED, "macro keys are not being handled")
                        })?;
                        lock(keys).play_async(&serial, key, &action)?;
                        Ok(json!({"playing":key}))
                    }
                }
            }
            "profile.list" | "profile.save" | "profile.load" | "profile.delete" => {
                let device = self.device(p.string("serial")?)?;
                let serial = lock_device(&device).serial();
                if method == "profile.list" {
                    return Ok(json!(lock(&self.profiles).summaries(&serial)));
                }
                let name = p.string("name")?;
                match method {
                    "profile.save" => {
                        let state = profiles::snapshot(lock_device(&device).as_mut());
                        let saved = lock(&self.profiles).put(&serial, name, state)?;
                        Ok(json!({"name":saved.name,"saved":saved.saved}))
                    }
                    "profile.load" => {
                        let profile = lock(&self.profiles).get(&serial, name)?;
                        // Stop the old renderer before applying the snapshot; it
                        // must not overwrite the newly restored hardware state.
                        if let Some(effects) = &self.effects {
                            lock(effects).clear_serial(&serial);
                        }
                        profiles::apply(lock_device(&device).as_mut(), &profile.state)?;
                        if let Some(effects) = &self.effects {
                            let mut engine = lock(effects);
                            if profile
                                .state
                                .device
                                .get("software")
                                .is_some_and(rpc::truthy)
                            {
                                engine.restore(&device);
                            } else {
                                engine.stop_for(&device)?;
                            }
                        }
                        persistence::lock(&self.persistence).save()?;
                        Ok(json!({"name":profile.name}))
                    }
                    _ => {
                        lock(&self.profiles).delete(&serial, name)?;
                        Ok(json!({"name":name}))
                    }
                }
            }
            _ => unreachable!("method validated above"),
        }
    }
}
