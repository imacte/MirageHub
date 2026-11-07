use crate::{
    effects::frame::Colour, persistence::atomic_write, worker::Cancellation, Error, Result,
};
use serde_json::{json, Map, Value};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};
pub const ACCENT_POLL: Duration = Duration::from_secs(2);
pub const NAMES: [&str; 2] = ["accent", "chroma"];

pub fn read_accent() -> Option<Colour> {
    #[cfg(windows)]
    {
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\DWM")
            .ok()?;
        let value: u32 = key.get_value("AccentColor").ok()?;
        Some([value as u8, (value >> 8) as u8, (value >> 16) as u8])
    }
    #[cfg(not(windows))]
    None
}
pub struct IntegrationSettings {
    path: Option<PathBuf>,
    data: Map<String, Value>,
}
impl IntegrationSettings {
    pub fn memory() -> Self {
        Self {
            path: None,
            data: Map::new(),
        }
    }
    pub fn load(path: PathBuf) -> Self {
        let data = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            data,
        }
    }
    pub fn enabled(&self, name: &str) -> bool {
        self.data.get(name) == Some(&Value::Bool(true))
    }
    pub fn set(&mut self, name: &str, enabled: bool) -> Result<()> {
        if !NAMES.contains(&name) {
            return Err(Error::Invalid(format!("unknown integration {name}")));
        }
        self.data.insert(name.into(), json!(enabled));
        if let Some(path) = &self.path {
            atomic_write(path, &serde_json::to_vec_pretty(&self.data)?)?;
        }
        Ok(())
    }
    pub fn as_json(&self) -> Value {
        json!({"accent":self.enabled("accent"),"chroma":self.enabled("chroma")})
    }
}

pub struct AccentWatch {
    read: Arc<dyn Fn() -> Option<Colour> + Send + Sync>,
    apply: Arc<dyn Fn(Colour) -> Result<()> + Send + Sync>,
    colour: Arc<Mutex<Option<Colour>>>,
    interval: Duration,
    worker: Option<(Arc<Cancellation>, JoinHandle<()>)>,
}
impl AccentWatch {
    pub fn new(apply: Arc<dyn Fn(Colour) -> Result<()> + Send + Sync>) -> Self {
        Self::with_reader(apply, Arc::new(read_accent), ACCENT_POLL)
    }
    pub fn with_reader(
        apply: Arc<dyn Fn(Colour) -> Result<()> + Send + Sync>,
        read: Arc<dyn Fn() -> Option<Colour> + Send + Sync>,
        interval: Duration,
    ) -> Self {
        Self {
            read,
            apply,
            colour: Arc::new(Mutex::new(None)),
            interval: interval.max(Duration::from_millis(1)),
            worker: None,
        }
    }
    pub fn running(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|(_, worker)| !worker.is_finished())
    }
    pub fn colour(&self) -> Option<Colour> {
        *self.colour.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn check(&self, force: bool) -> Option<Colour> {
        Self::check_inner(&self.read, &self.apply, &self.colour, force)
    }
    fn check_inner(
        read: &Arc<dyn Fn() -> Option<Colour> + Send + Sync>,
        apply: &Arc<dyn Fn(Colour) -> Result<()> + Send + Sync>,
        state: &Mutex<Option<Colour>>,
        force: bool,
    ) -> Option<Colour> {
        let colour = read()?;
        let changed = {
            let mut previous = state.lock().unwrap_or_else(|e| e.into_inner());
            let changed = force || *previous != Some(colour);
            *previous = Some(colour);
            changed
        };
        if changed {
            let _ = apply(colour);
        }
        Some(colour)
    }
    pub fn start(&mut self) -> Result<()> {
        if self.running() {
            return Ok(());
        }
        self.stop();
        let cancel = Arc::new(Cancellation::default());
        let stop = cancel.clone();
        let (read, apply, colour, interval) = (
            self.read.clone(),
            self.apply.clone(),
            self.colour.clone(),
            self.interval,
        );
        let worker = thread::Builder::new()
            .name("openrazer-accent".into())
            .spawn(move || {
                Self::check_inner(&read, &apply, &colour, true);
                while !stop.wait(interval) {
                    Self::check_inner(&read, &apply, &colour, false);
                }
            })?;
        self.worker = Some((cancel, worker));
        Ok(())
    }
    pub fn stop(&mut self) {
        if let Some((cancel, worker)) = self.worker.take() {
            cancel.stop();
            let _ = worker.join();
        }
        *self.colour.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}
impl Drop for AccentWatch {
    fn drop(&mut self) {
        self.stop();
    }
}
