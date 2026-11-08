//! Thread boundary for Snap Tap. The engine belongs to the keyboard hook thread;
//! settings and status callers never hold an engine lock in that callback.
use super::{
    inject::{Injection, EXTENDED, KEY_UP, SCAN_CODE},
    snaptap::{Decision, Engine, KeyId},
    snaptap_settings::{self, Settings, SettingsStore, SCOPE, WARNING, WARNING_SHORT},
};
use crate::{keyboard::KeyEvent, Error, Result};
use serde_json::{json, Map, Value};
use std::{
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

pub enum Command {
    Configure(Settings, mpsc::SyncSender<()>),
}

#[derive(Clone)]
pub struct Attachment {
    pub owner: u32,
    pub send: mpsc::Sender<Command>,
    pub wake: Arc<dyn Fn() -> bool + Send + Sync>,
}

#[derive(Clone, Default)]
pub struct RuntimeStatus {
    pub running: bool,
    pub foreground: Option<String>,
    pub paused: Option<String>,
    pub resets: u64,
    pub rearms: u64,
    pub failed_sends: u64,
    pub hook_errors: u64,
    pub latency_us: Value,
}

pub struct Controller {
    pub settings: Arc<SettingsStore>,
    gate: Mutex<()>,
    attachment: Mutex<Option<Attachment>>,
    runtime: Mutex<RuntimeStatus>,
}
impl Controller {
    pub fn new(settings: Arc<SettingsStore>) -> Arc<Self> {
        Arc::new(Self {
            settings,
            gate: Mutex::new(()),
            attachment: Mutex::new(None),
            runtime: Mutex::new(RuntimeStatus::default()),
        })
    }
    /// Installing a hook and taking its first settings snapshot are atomic
    /// against RPC changes, avoiding a lost update during daemon startup.
    pub fn attach(&self, attachment: Attachment) -> Settings {
        let _gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        *self.attachment.lock().unwrap_or_else(|e| e.into_inner()) = Some(attachment);
        self.settings.snapshot()
    }
    pub fn detach(&self, owner: u32) {
        let mut attachment = self.attachment.lock().unwrap_or_else(|e| e.into_inner());
        if attachment.as_ref().is_some_and(|a| a.owner == owner) {
            *attachment = None;
            self.publish(RuntimeStatus::default());
        }
    }
    pub fn attached(&self) -> bool {
        self.attachment
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    pub fn update(&self, changes: &Map<String, Value>) -> Result<Settings> {
        let _gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        let settings = self.settings.update(changes)?;
        let attachment = self
            .attachment
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(attachment) = attachment {
            let (done, receive) = mpsc::sync_channel(1);
            attachment
                .send
                .send(Command::Configure(settings.clone(), done))
                .map_err(|_| Error::Device("Snap Tap hook stopped".into()))?;
            if !(attachment.wake)() {
                return Err(Error::Device("cannot notify the Snap Tap hook".into()));
            }
            receive
                .recv_timeout(Duration::from_secs(2))
                .map_err(|_| Error::Device("Snap Tap hook did not apply settings".into()))?;
        }
        Ok(settings)
    }

    pub fn publish(&self, state: RuntimeStatus) {
        *self.runtime.lock().unwrap_or_else(|e| e.into_inner()) = state;
    }
    pub fn status(&self) -> Value {
        let settings = self.settings.snapshot();
        let state = self
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let latency = if state.latency_us.is_null() {
            json!({"samples":0,"p50":null,"p99":null,"max":null})
        } else {
            state.latency_us
        };
        json!({"enabled":settings.enabled,"acknowledged":settings.acknowledged,
            "pairs":settings.pairs,"deny":settings.deny,"allow":settings.allow,
            "running":state.running,"foreground":state.foreground,"paused":state.paused,
            "scope":SCOPE,"warning":WARNING,"warning_short":WARNING_SHORT,
            "keys":snaptap_settings::offered_keys(),"latency_us":latency,
            "resets":state.resets,"rearms":state.rearms,"failed_sends":state.failed_sends,"hook_errors":state.hook_errors})
    }
}

/// Only the hook thread mutates a Host; no mutex is acquired for key decisions.
pub struct Host {
    pub controller: Arc<Controller>,
    pub state: RuntimeStatus,
    pub settings: Settings,
    engine: Engine,
    samples: [u64; 256],
    count: usize,
    next: usize,
}
impl Host {
    pub fn new(controller: Arc<Controller>, settings: Settings) -> Self {
        let mut host = Self {
            controller,
            state: RuntimeStatus::default(),
            settings: Settings::default(),
            engine: Engine::default(),
            samples: [0; 256],
            count: 0,
            next: 0,
        };
        host.configure(settings);
        host
    }
    pub fn configure(&mut self, settings: Settings) {
        let pairs = if settings.enabled {
            settings.key_ids()
        } else {
            vec![]
        };
        self.engine.configure(&pairs);
        self.engine.paused = true;
        self.state.running = settings.enabled;
        self.state.foreground = None;
        self.state.paused = settings.enabled.then(|| "unknown program".into());
        self.settings = settings;
        self.publish();
    }
    pub fn system_change(&mut self, foreground: Option<String>, is_down: impl Fn(u32) -> bool) {
        self.state.resets += 1;
        self.engine.reset(is_down);
        self.state.foreground = if self.settings.enabled {
            foreground
        } else {
            None
        };
        self.state.paused = self
            .settings
            .enabled
            .then(|| {
                snaptap_settings::pause_reason(self.state.foreground.as_deref(), &self.settings)
            })
            .flatten()
            .map(str::to_string);
        self.engine.paused = !self.settings.enabled || self.state.paused.is_some();
        self.publish();
    }
    pub fn unavailable_watcher(&mut self) {
        self.engine.reset(|_| false);
        self.engine.paused = true;
        self.state.paused = Some("system watcher unavailable".into());
        self.publish();
    }
    pub fn handle(&mut self, event: KeyEvent, swallowed_by_fn: bool) -> Decision {
        if !self.settings.enabled {
            return Decision::default();
        }
        if swallowed_by_fn {
            self.engine.swallowed(event);
            Decision::default()
        } else {
            self.engine.handle(event)
        }
    }
    pub fn resumes(&mut self) -> Vec<Injection> {
        self.engine
            .take_resumes()
            .into_iter()
            .map(|key| injection(key, true))
            .collect()
    }
    pub fn cancel_resumes(&mut self) {
        self.engine.take_resumes();
    }
    pub fn rearm(&mut self, is_down: impl Fn(u32) -> bool) {
        self.engine.rearm(is_down);
        self.state.rearms += 1;
        self.publish();
    }
    pub fn hook_error(&mut self, is_down: impl Fn(u32) -> bool) {
        self.engine.reset(is_down);
        self.state.hook_errors += 1;
    }
    pub fn record_latency(&mut self, elapsed: Duration) {
        self.samples[self.next] = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
        self.next = (self.next + 1) % self.samples.len();
        self.count = (self.count + 1).min(self.samples.len());
    }
    pub fn publish(&mut self) {
        if self.count != 0 {
            let mut samples = self.samples[..self.count].to_vec();
            samples.sort_unstable();
            self.state.latency_us = json!({"samples":self.count,"p50":samples[(self.count-1)/2],"p99":samples[(self.count-1)*99/100],"max":samples[self.count-1]});
        }
        self.controller.publish(self.state.clone());
    }
}

pub fn injection(key: KeyId, down: bool) -> Injection {
    Injection {
        vk: 0,
        scan: key.scan as u16,
        flags: SCAN_CODE | if key.extended { EXTENDED } else { 0 } | if down { 0 } else { KEY_UP },
    }
}

pub struct Service {
    pub controller: Arc<Controller>,
    pub native: bool,
    started: bool,
    standalone: Option<crate::keyboard::KeyboardHook>,
}
impl Service {
    pub fn new(controller: Arc<Controller>) -> Self {
        Self {
            controller,
            native: false,
            started: false,
            standalone: None,
        }
    }
    pub fn start(&mut self) -> Result<()> {
        self.started = true;
        self.reconcile()
    }
    fn reconcile(&mut self) -> Result<()> {
        let enabled = self.controller.settings.snapshot().enabled;
        if !enabled || !self.native || !self.started {
            self.standalone.take();
        } else if !self.controller.attached() {
            // --no-keys disables Fn/macros, while explicitly enabled Snap Tap
            // owns a hook only for as long as it needs one.
            self.standalone = Some(crate::keyboard::KeyboardHook::with_snaptap(
                Arc::new(|_| false),
                Some(self.controller.clone()),
                Arc::new(super::inject::NativeInjector),
            )?);
        }
        Ok(())
    }
    pub fn update(&mut self, changes: &Map<String, Value>) -> Result<Value> {
        self.controller.update(changes)?;
        self.reconcile()?;
        Ok(self.controller.status())
    }
    pub fn stop(&mut self) {
        self.started = false;
        self.standalone.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(scan: u32, down: bool) -> KeyEvent {
        KeyEvent {
            vk: if scan == 30 { 65 } else { 68 },
            scan,
            down,
            extended: false,
            injected: false,
            ours: false,
            time: 0,
        }
    }
    fn host() -> Host {
        let controller = Controller::new(Arc::new(SettingsStore::memory()));
        let settings = controller
            .update(
                json!({"enabled":true,"acknowledged":true})
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        let mut host = Host::new(controller, settings);
        host.system_change(Some("game.exe".into()), |_| false);
        host
    }
    #[test]
    fn foreground_changes_drop_pending_keys_and_deny_list_is_enforced() {
        let mut host = host();
        host.handle(event(30, true), false);
        host.handle(event(32, true), false);
        assert!(host.handle(event(32, false), false).later);
        host.system_change(Some("cs2.exe".into()), |_| false);
        assert!(host.resumes().is_empty());
        assert_eq!(host.controller.status()["paused"], "denied");
        assert_eq!(host.handle(event(30, true), false), Decision::default());
        host.system_change(None, |_| false);
        assert_eq!(host.controller.status()["paused"], "unknown program");
    }
    #[test]
    fn fn_swallowed_releases_and_configuration_changes_cancel_resumes() {
        let mut host = host();
        host.handle(event(30, true), false);
        host.handle(event(32, true), false);
        host.handle(event(32, false), false);
        host.handle(event(30, false), true);
        assert!(host.resumes().is_empty());
        host.handle(event(30, true), false);
        host.handle(event(32, true), false);
        host.handle(event(32, false), false);
        host.configure(Settings::default());
        assert!(host.resumes().is_empty());
        assert!(!host.state.running);
    }
    #[test]
    fn resumes_use_scan_codes_and_only_after_callback() {
        let mut host = host();
        host.handle(event(30, true), false);
        let release = host.handle(event(32, true), false).release.unwrap();
        assert_eq!(injection(release, false).flags, SCAN_CODE | KEY_UP);
        assert!(host.handle(event(32, false), false).later);
        assert_eq!(
            host.resumes(),
            vec![Injection {
                vk: 0,
                scan: 30,
                flags: SCAN_CODE
            }]
        );
        assert!(host.resumes().is_empty());
    }
}
