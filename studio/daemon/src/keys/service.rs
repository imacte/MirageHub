//! The hook only updates key state and emits translated input. Device I/O,
//! persistence and timed macro playback run on separate, joinable workers.
use super::{
    codes::{self, Output, ACTIONS, MACRO_KEYS, MACRO_TARGETS},
    inject::{self, Injector, Launcher, NativeInjector, NativeLauncher},
    macros::{self, Action, MacroEvent, MacroStore},
};
use crate::{
    database::DeviceInfo,
    hid::{HidBackend, HidInfo},
    keyboard::{KeyEvent, KeyboardHook},
    manager::{lock_device, SharedDevice},
    worker::Cancellation,
    Error, Result,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex, MutexGuard, OnceLock,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Deserialize)]
pub struct KeyData {
    pub default_table: String,
    pub fn_code: u8,
    pub report_id: u8,
    pub no_translation_pids: BTreeSet<String>,
    pub tables: BTreeMap<String, Vec<(String, String)>>,
    pub tables_by_pid: BTreeMap<String, String>,
    pub vendor_codes: BTreeMap<String, String>,
}
pub fn key_data() -> &'static KeyData {
    static DATA: OnceLock<KeyData> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(include_str!("../../../../data/keys.json"))
            .expect("embedded key data is valid")
    })
}
pub fn is_event_collection(info: &HidInfo) -> bool {
    info.input_length != 0
        && ((info.usage_page == 1 && info.usage == 0) || info.usage_page >= 0xff00)
}
pub fn parse_report(report: &[u8], report_id: u8) -> Option<BTreeSet<u8>> {
    (report.first() == Some(&report_id))
        .then(|| report[1..].iter().copied().filter(|v| *v != 0).collect())
}
pub fn wants(info: &DeviceInfo) -> bool {
    info.driver() == "kbd"
        && info.driver_mode
        && !key_data()
            .no_translation_pids
            .contains(&format!("{:04x}", info.pid))
}
pub fn table_for(pid: u16) -> BTreeMap<String, String> {
    let data = key_data();
    let name = data
        .tables_by_pid
        .get(&format!("{pid:04x}"))
        .unwrap_or(&data.default_table);
    data.tables
        .get(name)
        .into_iter()
        .flatten()
        .cloned()
        .collect()
}

#[derive(Clone)]
struct Reference {
    id: u64,
    serial: String,
    device: SharedDevice,
}
struct Binding {
    reference: Reference,
    table: BTreeMap<String, String>,
    fn_held: bool,
    held: BTreeSet<u8>,
    pressed_fn: BTreeSet<String>,
    defaulted: BTreeMap<String, Output>,
    game_mode: bool,
    seen: BTreeSet<String>,
    outputs_held: BTreeSet<Output>,
}
struct Recording {
    binding: Reference,
    key: Option<String>,
    events: Vec<MacroEvent>,
    last: Option<u32>,
}
impl Recording {
    fn new(binding: Reference, key: Option<String>) -> Self {
        Self {
            binding,
            key,
            events: Vec::new(),
            last: None,
        }
    }
    fn add(&mut self, event: KeyEvent) {
        // Keep one overflow event so validation rejects an oversized recording,
        // without growing memory for as long as the user leaves recording on.
        if self.events.len() > macros::MAX_EVENTS {
            return;
        }
        let delay = self
            .last
            .map(|last| event.time.saturating_sub(last))
            .unwrap_or(0)
            .min(macros::MAX_DELAY_MS);
        self.last = Some(event.time);
        self.events.push(MacroEvent {
            vk: event.vk as u16,
            scan: event.scan as u16,
            ext: event.extended,
            down: event.down,
            delay,
        });
    }
}
#[derive(Default)]
struct State {
    bindings: Vec<Binding>,
    recording: Option<Recording>,
    next_id: u64,
}
enum Job {
    Perform(Reference, String),
    Led(Reference, bool, bool),
}
enum Effect {
    Inject(Output, bool),
    Job(Job),
    Play(String, String, Action),
}
enum InputJob {
    Key(Output, bool),
    Barrier(mpsc::Sender<()>),
}
struct Playback {
    cancel: Arc<Cancellation>,
    worker: JoinHandle<()>,
}
struct HeldOutputs {
    injector: Arc<dyn Injector>,
    keys: BTreeSet<Output>,
}
impl Drop for HeldOutputs {
    fn drop(&mut self) {
        for key in &self.keys {
            let _ = self.injector.key(*key, false);
        }
    }
}
type Errors = Arc<Mutex<VecDeque<String>>>;
fn report_error(errors: &Errors, error: impl ToString) {
    let mut errors = lock(errors);
    if errors.len() == 32 {
        errors.pop_front();
    }
    errors.push_back(error.to_string());
}
struct Core {
    state: Mutex<State>,
    // Serialise translation with detach's queued release of held outputs.
    // No thread holding this gate calls SendInput: that call can wait for the
    // hook thread, which may itself be waiting to process a physical key.
    output_gate: Mutex<()>,
    macros: Arc<MacroStore>,
    injector: Arc<dyn Injector>,
    launcher: Arc<dyn Launcher>,
    enabled: AtomicBool,
    jobs: Mutex<Option<SyncSender<Job>>>,
    inputs: Mutex<Option<mpsc::Sender<InputJob>>>,
    playing: Mutex<BTreeMap<(String, String), Playback>>,
    errors: Errors,
}
impl Core {
    fn queue(&self, job: Job) {
        if let Some(sender) = lock(&self.jobs).as_ref() {
            if sender.try_send(job).is_err() {
                report_error(&self.errors, "keyboard action queue is full or stopped");
            }
        }
    }
    fn dispatch(&self, effects: Vec<Effect>) {
        for effect in effects {
            let result = match effect {
                Effect::Inject(output, down) => self.queue_input(output, down),
                Effect::Job(job) => {
                    self.queue(job);
                    Ok(())
                }
                Effect::Play(serial, key, action) => {
                    self.play_async(&serial, &key, action).map(|_| ())
                }
            };
            if let Err(error) = result {
                report_error(&self.errors, error);
            }
        }
    }
    fn queue_input(&self, output: Output, down: bool) -> Result<()> {
        lock(&self.inputs)
            .as_ref()
            .ok_or_else(|| Error::Io("input sender is stopped".into()))?
            .send(InputJob::Key(output, down))
            .map_err(|_| Error::Io("input sender is stopped".into()))
    }
    fn synthesize(binding: &mut Binding, output: Output, down: bool, effects: &mut Vec<Effect>) {
        if down {
            binding.outputs_held.insert(output);
        } else {
            binding.outputs_held.remove(&output);
        }
        effects.push(Effect::Inject(output, down));
    }
    fn macro_key(
        &self,
        state: &mut State,
        index: usize,
        name: &str,
        default: Option<&str>,
        down: bool,
        effects: &mut Vec<Effect>,
    ) {
        let binding = &mut state.bindings[index];
        if !down {
            if let Some(output) = binding.defaulted.remove(name) {
                Self::synthesize(binding, output, false, effects);
            }
            return;
        }
        if let Some(recording) = state
            .recording
            .as_mut()
            .filter(|r| r.binding.id == binding.reference.id)
        {
            if recording.key.is_none() {
                recording.key = Some(name.into());
                effects.push(Effect::Job(Job::Led(
                    binding.reference.clone(),
                    true,
                    false,
                )));
            } else if recording.key.as_deref() == Some(name) {
                state.recording = None;
                effects.push(Effect::Job(Job::Led(
                    binding.reference.clone(),
                    false,
                    true,
                )));
            }
            return;
        }
        match self.macros.get(&binding.reference.serial, name) {
            None => {
                if let Some(output) = default.and_then(codes::output_for) {
                    binding.defaulted.insert(name.into(), output);
                    Self::synthesize(binding, output, true, effects);
                }
            }
            Some(value) => match macros::validate(&value) {
                Ok(Action::Disabled) => (),
                Ok(action) => effects.push(Effect::Play(
                    binding.reference.serial.clone(),
                    name.into(),
                    action,
                )),
                Err(error) => report_error(&self.errors, error),
            },
        }
    }
    fn handle_report(&self, id: u64, codes: BTreeSet<u8>) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        let _gate = lock(&self.output_gate);
        let mut effects = Vec::new();
        {
            let mut state = lock(&self.state);
            let Some(index) = state.bindings.iter().position(|b| b.reference.id == id) else {
                return;
            };
            let data = key_data();
            let binding = &mut state.bindings[index];
            binding.fn_held = codes.contains(&data.fn_code);
            let held: BTreeSet<_> = codes.into_iter().filter(|c| *c != data.fn_code).collect();
            let changes: Vec<_> = held
                .difference(&binding.held)
                .map(|c| (*c, true))
                .chain(binding.held.difference(&held).map(|c| (*c, false)))
                .collect();
            binding.held = held;
            for (code, down) in changes {
                let target = data
                    .vendor_codes
                    .get(&format!("{code:02x}"))
                    .map(String::as_str);
                if let Some((_, name)) = MACRO_KEYS.iter().find(|(c, _)| *c == code) {
                    state.bindings[index].seen.insert((*name).into());
                    self.macro_key(&mut state, index, name, target, down, &mut effects);
                } else if let Some(output) = target.and_then(codes::output_for) {
                    Self::synthesize(&mut state.bindings[index], output, down, &mut effects);
                }
            }
        }
        self.dispatch(effects);
    }
    fn handle_key(&self, event: KeyEvent) -> bool {
        if event.injected || !self.enabled.load(Ordering::Acquire) {
            return false;
        }
        let _gate = lock(&self.output_gate);
        let mut effects = Vec::new();
        let mut swallow = false;
        {
            let mut state = lock(&self.state);
            if matches!(event.vk, 0x5b | 0x5c) && state.bindings.iter().any(|b| b.game_mode) {
                return true;
            }
            let mut translated = None;
            if let Some(source) = codes::source_name(event.scan, event.extended, event.vk) {
                for (index, binding) in state.bindings.iter_mut().enumerate() {
                    let Some(target) = binding.table.get(source).cloned() else {
                        continue;
                    };
                    let repeat = binding.pressed_fn.contains(source);
                    if event.down && (binding.fn_held || repeat) {
                        binding.pressed_fn.insert(source.into());
                    } else if !event.down && repeat {
                        binding.pressed_fn.remove(source);
                    } else {
                        continue;
                    }
                    translated = Some((index, target, event.down && repeat));
                    swallow = true;
                    break;
                }
            }
            if let Some((index, target, repeat)) = translated {
                if let Some((_, action)) = ACTIONS.iter().find(|(name, _)| *name == target) {
                    if event.down && !repeat {
                        effects.push(Effect::Job(Job::Perform(
                            state.bindings[index].reference.clone(),
                            (*action).into(),
                        )));
                    }
                } else if let Some((_, name)) =
                    MACRO_TARGETS.iter().find(|(name, _)| *name == target)
                {
                    if !repeat {
                        let default = match *name {
                            "M1" => "KEY_F13",
                            "M2" => "KEY_F14",
                            "M3" => "KEY_F15",
                            _ => "KEY_F16",
                        };
                        self.macro_key(
                            &mut state,
                            index,
                            name,
                            Some(default),
                            event.down,
                            &mut effects,
                        );
                    }
                } else if let Some(output) = codes::output_for(&target) {
                    Self::synthesize(&mut state.bindings[index], output, event.down, &mut effects);
                }
            }
            if !swallow {
                if let Some(recording) = state.recording.as_mut().filter(|r| r.key.is_some()) {
                    recording.add(event);
                }
            }
        }
        self.dispatch(effects);
        swallow
    }
    fn current(&self, reference: &Reference) -> bool {
        lock(&self.state)
            .bindings
            .iter()
            .any(|b| b.reference.id == reference.id)
    }
    fn macro_led(&self, reference: &Reference, recording: bool, blinking: bool) {
        let mut device = lock_device(&reference.device);
        let kwargs = Map::new();
        // LEDs are optional; a failure must not prevent recording a macro.
        if device
            .call("set_macro_mode", &[json!(recording)], &kwargs)
            .is_ok()
        {
            let _ = device.call(
                "set_macro_effect",
                &[json!(u8::from(recording && blinking))],
                &kwargs,
            );
        }
    }
    fn perform(&self, reference: &Reference, action: &str) -> Result<()> {
        if !self.current(reference) {
            return Ok(());
        }
        match action {
            "macro_record" => {
                let previous = {
                    let mut state = lock(&self.state);
                    if state
                        .recording
                        .as_ref()
                        .is_some_and(|r| r.binding.id == reference.id)
                    {
                        state.recording.take()
                    } else {
                        state.recording = Some(Recording::new(reference.clone(), None));
                        None
                    }
                };
                if let Some(recording) = previous {
                    self.finish(recording)?;
                } else {
                    self.macro_led(reference, true, true);
                }
            }
            "game_mode" => {
                let value = {
                    let mut state = lock(&self.state);
                    let Some(binding) = state
                        .bindings
                        .iter_mut()
                        .find(|b| b.reference.id == reference.id)
                    else {
                        return Ok(());
                    };
                    binding.game_mode = !binding.game_mode;
                    binding.game_mode
                };
                let _ = lock_device(&reference.device).call(
                    "set_game_mode",
                    &[json!(value)],
                    &Map::new(),
                );
            }
            "brightness_down" | "brightness_up" => {
                let mut device = lock_device(&reference.device);
                let current = device
                    .call("get_brightness", &[], &Map::new())
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(50.0);
                let step = if action == "brightness_up" {
                    10.0
                } else {
                    -10.0
                };
                device.call(
                    "set_brightness",
                    &[json!((current + step).clamp(0.0, 100.0))],
                    &Map::new(),
                )?;
            }
            _ => (),
        }
        Ok(())
    }
    fn finish(&self, recording: Recording) -> Result<Option<Value>> {
        self.macro_led(&recording.binding, false, true);
        let Some(key) = recording.key else {
            return Ok(None);
        };
        if recording.events.is_empty() {
            self.macros.delete(&recording.binding.serial, &key)?;
            return Ok(None);
        }
        let value = serde_json::to_value(Action::Keys {
            events: recording.events,
        })?;
        match self.macros.set(&recording.binding.serial, &key, &value) {
            Ok(value) => Ok(Some(value)),
            Err(Error::Invalid(message)) => {
                report_error(&self.errors, message);
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
    fn play_async(&self, serial: &str, key: &str, action: Action) -> Result<bool> {
        let mut playing = lock(&self.playing);
        if !self.enabled.load(Ordering::Acquire) {
            return Ok(false);
        }
        playing.retain(|_, v| !v.worker.is_finished());
        let identity = (serial.to_owned(), key.to_owned());
        if playing.contains_key(&identity) {
            return Ok(false);
        }
        if playing.len() >= 64 {
            return Err(Error::Invalid("too many macros are playing".into()));
        }
        let cancel = Arc::new(Cancellation::default());
        let (stop, injector, launcher, errors) = (
            cancel.clone(),
            self.injector.clone(),
            self.launcher.clone(),
            self.errors.clone(),
        );
        let worker = thread::Builder::new()
            .name("openrazer-macro".into())
            .spawn(move || {
                if let Err(error) =
                    inject::play(&action, injector.as_ref(), launcher.as_ref(), &stop)
                {
                    report_error(&errors, error);
                }
            })?;
        playing.insert(identity, Playback { cancel, worker });
        Ok(true)
    }
    fn stop_playback(&self, serial: Option<&str>) {
        let removed = {
            let mut playing = lock(&self.playing);
            let keys: Vec<_> = playing
                .keys()
                .filter(|(s, _)| serial.is_none_or(|serial| serial == s))
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|key| playing.remove(&key))
                .collect::<Vec<_>>()
        };
        for task in &removed {
            task.cancel.stop();
        }
        for task in removed {
            let _ = task.worker.join();
        }
    }
}

struct Readers {
    id: u64,
    cancel: Arc<Cancellation>,
    workers: Vec<JoinHandle<()>>,
}
impl Drop for Readers {
    fn drop(&mut self) {
        self.cancel.stop();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
pub struct KeyService {
    backend: Arc<dyn HidBackend>,
    core: Arc<Core>,
    native_hook: bool,
    hook: Option<KeyboardHook>,
    action_worker: Option<JoinHandle<()>>,
    input_worker: Option<JoinHandle<()>>,
    input_cancel: Arc<Cancellation>,
    readers: Vec<Readers>,
}
impl KeyService {
    pub fn native(backend: Arc<dyn HidBackend>, macros: Arc<MacroStore>) -> Self {
        Self::new(
            backend,
            macros,
            Arc::new(NativeInjector),
            Arc::new(NativeLauncher),
            true,
        )
    }
    pub fn new(
        backend: Arc<dyn HidBackend>,
        macros: Arc<MacroStore>,
        injector: Arc<dyn Injector>,
        launcher: Arc<dyn Launcher>,
        native_hook: bool,
    ) -> Self {
        Self {
            backend,
            core: Arc::new(Core {
                state: Mutex::new(State::default()),
                output_gate: Mutex::new(()),
                macros,
                injector,
                launcher,
                enabled: AtomicBool::new(false),
                jobs: Mutex::new(None),
                inputs: Mutex::new(None),
                playing: Mutex::new(BTreeMap::new()),
                errors: Arc::new(Mutex::new(VecDeque::new())),
            }),
            native_hook,
            hook: None,
            action_worker: None,
            input_worker: None,
            input_cancel: Arc::new(Cancellation::default()),
            readers: Vec::new(),
        }
    }
    pub fn start(&mut self) -> Result<bool> {
        if self.running() {
            return Ok(true);
        }
        self.stop();
        self.input_cancel = Arc::new(Cancellation::default());
        let (send, receive) = mpsc::channel();
        *lock(&self.core.inputs) = Some(send);
        let (injector, errors) = (self.core.injector.clone(), self.core.errors.clone());
        let cancel = self.input_cancel.clone();
        self.input_worker = Some(
            thread::Builder::new()
                .name("openrazer-key-output".into())
                .spawn(move || {
                    let mut held = HeldOutputs {
                        injector: injector.clone(),
                        keys: BTreeSet::new(),
                    };
                    while let Ok(job) = receive.recv() {
                        match job {
                            InputJob::Key(output, down) => {
                                if !cancel.stopped() {
                                    if down {
                                        held.keys.insert(output);
                                    }
                                    match injector.key(output, down) {
                                        Ok(()) => {
                                            if !down {
                                                held.keys.remove(&output);
                                            }
                                        }
                                        Err(error) => report_error(&errors, error),
                                    }
                                }
                            }
                            InputJob::Barrier(send) => {
                                let _ = send.send(());
                            }
                        }
                    }
                })?,
        );
        let (send, receive) = mpsc::sync_channel(256);
        *lock(&self.core.jobs) = Some(send);
        let core = Arc::downgrade(&self.core);
        self.action_worker = Some(
            thread::Builder::new()
                .name("openrazer-key-actions".into())
                .spawn(move || {
                    while let Ok(job) = receive.recv() {
                        let Some(core) = core.upgrade() else {
                            break;
                        };
                        if !core.enabled.load(Ordering::Acquire) {
                            continue;
                        }
                        let result = match job {
                            Job::Perform(reference, action) => core.perform(&reference, &action),
                            Job::Led(reference, recording, blinking) => {
                                if core.current(&reference) {
                                    core.macro_led(&reference, recording, blinking);
                                }
                                Ok(())
                            }
                        };
                        if let Err(error) = result {
                            report_error(&core.errors, error);
                        }
                    }
                })?,
        );
        if self.native_hook {
            let core = Arc::downgrade(&self.core);
            match KeyboardHook::start(Arc::new(move |event| {
                core.upgrade().is_some_and(|core| core.handle_key(event))
            })) {
                Ok(hook) => self.hook = Some(hook),
                Err(error) => {
                    self.stop();
                    return Err(error);
                }
            }
        }
        self.core.enabled.store(true, Ordering::Release);
        Ok(true)
    }
    pub fn running(&self) -> bool {
        self.core.enabled.load(Ordering::Acquire)
            && (!self.native_hook || self.hook.as_ref().is_some_and(KeyboardHook::running))
    }
    pub fn stop(&mut self) {
        self.core.enabled.store(false, Ordering::Release);
        self.hook.take();
        self.input_cancel.stop();
        for readers in &self.readers {
            readers.cancel.stop();
        }
        self.core.stop_playback(None);
        lock(&self.core.jobs).take();
        if let Some(worker) = self.action_worker.take() {
            let _ = worker.join();
        }
        let ids: Vec<_> = lock(&self.core.state)
            .bindings
            .iter()
            .map(|b| b.reference.id)
            .collect();
        for id in ids {
            self.detach_id(id);
        }
        lock(&self.core.inputs).take();
        if let Some(worker) = self.input_worker.take() {
            let _ = worker.join();
        }
    }
    pub fn attach(&mut self, device: &SharedDevice) -> Result<bool> {
        if !self.running() {
            return Ok(false);
        }
        let (serial, pid) = {
            let mut device = lock_device(device);
            if !wants(device.info()) {
                return Ok(false);
            }
            (device.serial(), device.info().pid)
        };
        if lock(&self.core.state)
            .bindings
            .iter()
            .any(|b| b.reference.serial == serial)
        {
            return Ok(true);
        }
        let mut inputs = Vec::new();
        for info in self.backend.enumerate(Some(0x1532), Some(pid))? {
            if is_event_collection(&info) {
                match self.backend.open_input(&info) {
                    Ok(reader) => inputs.push(reader),
                    Err(error) => report_error(&self.core.errors, error),
                }
            }
        }
        if inputs.is_empty() {
            return Ok(false);
        }
        let id = {
            let mut state = lock(&self.core.state);
            state.next_id += 1;
            let id = state.next_id;
            state.bindings.push(Binding {
                reference: Reference {
                    id,
                    serial,
                    device: device.clone(),
                },
                table: table_for(pid),
                fn_held: false,
                held: BTreeSet::new(),
                pressed_fn: BTreeSet::new(),
                defaulted: BTreeMap::new(),
                game_mode: false,
                seen: BTreeSet::new(),
                outputs_held: BTreeSet::new(),
            });
            id
        };
        self.readers.push(Readers {
            id,
            cancel: Arc::new(Cancellation::default()),
            workers: Vec::new(),
        });
        for mut reader in inputs {
            let cancel = self
                .readers
                .last()
                .expect("reader group added")
                .cancel
                .clone();
            let core = Arc::downgrade(&self.core);
            let worker = thread::Builder::new()
                .name("openrazer-vendor-keys".into())
                .spawn(move || {
                    while !cancel.stopped() {
                        let report = reader.read(Duration::from_millis(500));
                        let Some(core) = core.upgrade() else {
                            break;
                        };
                        match report {
                            Ok(Some(bytes)) => {
                                if let Some(codes) = parse_report(&bytes, key_data().report_id) {
                                    core.handle_report(id, codes);
                                }
                            }
                            Ok(None) => (),
                            Err(error) => {
                                report_error(&core.errors, error);
                                break;
                            }
                        }
                    }
                });
            match worker {
                Ok(worker) => self
                    .readers
                    .last_mut()
                    .expect("reader group added")
                    .workers
                    .push(worker),
                Err(error) => {
                    self.detach_id(id);
                    return Err(error.into());
                }
            }
        }
        Ok(true)
    }
    pub fn detach(&mut self, device: &SharedDevice) {
        let id = lock(&self.core.state)
            .bindings
            .iter()
            .find(|b| Arc::ptr_eq(&b.reference.device, device))
            .map(|b| b.reference.id);
        if let Some(id) = id {
            self.detach_id(id);
        }
    }
    fn detach_id(&mut self, id: u64) {
        let removed = {
            let _gate = lock(&self.core.output_gate);
            let binding = {
                let mut state = lock(&self.core.state);
                let Some(index) = state.bindings.iter().position(|b| b.reference.id == id) else {
                    return;
                };
                if state.recording.as_ref().is_some_and(|r| r.binding.id == id) {
                    state.recording = None;
                }
                state.bindings.remove(index)
            };
            for output in &binding.outputs_held {
                if let Err(error) = self.core.queue_input(*output, false) {
                    report_error(&self.core.errors, error);
                }
            }
            binding.reference.serial
        };
        self.readers.retain(|readers| readers.id != id);
        self.core.stop_playback(Some(&removed));
    }
    pub fn handles(&self, device: &SharedDevice) -> bool {
        lock(&self.core.state)
            .bindings
            .iter()
            .any(|b| Arc::ptr_eq(&b.reference.device, device))
    }
    pub fn handle_key(&self, event: KeyEvent) -> bool {
        self.core.handle_key(event)
    }
    pub fn handle_report(&self, serial: &str, bytes: &[u8]) {
        let id = lock(&self.core.state)
            .bindings
            .iter()
            .find(|b| b.reference.serial == serial)
            .map(|b| b.reference.id);
        if let (Some(id), Some(codes)) = (id, parse_report(bytes, key_data().report_id)) {
            self.core.handle_report(id, codes);
        }
    }
    pub fn macros(&self) -> &Arc<MacroStore> {
        &self.core.macros
    }
    pub fn play(&self, action: &Value) -> Result<()> {
        inject::play(
            &macros::validate(action)?,
            self.core.injector.as_ref(),
            self.core.launcher.as_ref(),
            &Cancellation::default(),
        )
    }
    pub fn play_async(&self, serial: &str, key: &str, action: &Value) -> Result<bool> {
        self.core.play_async(serial, key, macros::validate(action)?)
    }
    pub fn start_recording(&self, serial: &str, key: &str) -> Result<()> {
        let reference = {
            let mut state = lock(&self.core.state);
            let reference = state
                .bindings
                .iter()
                .find(|b| b.reference.serial == serial)
                .map(|b| b.reference.clone())
                .ok_or_else(|| Error::Invalid(format!("keyboard is not handled: {serial}")))?;
            state.recording = Some(Recording::new(reference.clone(), Some(key.into())));
            reference
        };
        self.core.macro_led(&reference, true, false);
        Ok(())
    }
    pub fn stop_recording(&self) -> Result<Option<Value>> {
        let recording = lock(&self.core.state).recording.take();
        recording
            .map(|r| self.core.finish(r))
            .transpose()
            .map(Option::flatten)
    }
    pub fn macro_keys(&self, device: &SharedDevice) -> Vec<String> {
        let (info, serial) = {
            let mut device = lock_device(device);
            (device.info().clone(), device.serial())
        };
        macro_keys(&info, &serial, &lock(&self.core.state))
    }
    pub fn status(&self, devices: &[SharedDevice]) -> Value {
        let devices: Vec<_> = devices
            .iter()
            .map(|d| {
                let mut d = lock_device(d);
                (d.info().clone(), d.serial())
            })
            .collect();
        let state = lock(&self.core.state);
        let rows:Vec<_> = devices.iter().filter_map(|(info,serial)| {
            let binding = state.bindings.iter().find(|b| b.reference.serial == *serial);
            if !wants(info) && binding.is_none() { return None; }
            Some(json!({"serial":serial,"name":info.name,"handled":binding.is_some(),"game_mode":binding.is_some_and(|b| b.game_mode),"macro_keys":macro_keys(info,serial,&state)}))
        }).collect();
        json!({"running":self.running(),"devices":rows,"recording":state.recording.as_ref().map(|r| json!({"serial":r.binding.serial,"key":r.key,"events":r.events.len()}))})
    }
    pub fn errors(&self) -> Vec<String> {
        lock(&self.core.errors).iter().cloned().collect()
    }
    /// Wait for translated inputs already queued, without synthesizing any.
    pub fn flush_inputs(&self, timeout: Duration) -> Result<()> {
        let (send, receive) = mpsc::channel();
        lock(&self.core.inputs)
            .as_ref()
            .ok_or_else(|| Error::Io("input sender is stopped".into()))?
            .send(InputJob::Barrier(send))
            .map_err(|_| Error::Io("input sender is stopped".into()))?;
        receive
            .recv_timeout(timeout)
            .map_err(|_| Error::Io("input sender did not drain".into()))
    }
}
impl Drop for KeyService {
    fn drop(&mut self) {
        self.stop();
    }
}

fn macro_keys(info: &DeviceInfo, serial: &str, state: &State) -> Vec<String> {
    let mut keys = BTreeSet::new();
    if info.dedicated_macro_keys {
        keys.extend(["M1", "M2", "M3", "M4", "M5"].map(str::to_owned));
    }
    if wants(info) {
        for target in table_for(info.pid).values() {
            if let Some((_, name)) = MACRO_TARGETS
                .iter()
                .find(|(target_name, _)| *target_name == target)
            {
                keys.insert((*name).into());
            }
        }
    }
    if let Some(binding) = state.bindings.iter().find(|b| b.reference.serial == serial) {
        keys.extend(binding.seen.clone());
    }
    let mut keys: Vec<_> = keys.into_iter().collect();
    keys.sort_by_key(|key| {
        MACRO_KEYS
            .iter()
            .position(|(_, name)| *name == key)
            .unwrap_or(MACRO_KEYS.len())
    });
    keys
}
