use crate::{
    rpc::{PublishedEndpoint, RpcServer, SingleInstance},
    service::{DaemonService, HOTPLUG_INTERVAL},
    worker::Cancellation,
    Result,
};
use std::{
    path::Path,
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

/// Owns all threads and the discovery file. Start publishes the endpoint before
/// the first scan; stop joins clients and workers before removing it/the mutex.
pub struct RunningDaemon {
    server: RpcServer,
    service: Arc<DaemonService>,
    cancel: Arc<Cancellation>,
    watcher: Option<JoinHandle<()>>,
    published: Option<PublishedEndpoint>,
    instance: Option<SingleInstance>,
}
impl RunningDaemon {
    pub fn start(
        service: Arc<DaemonService>,
        directory: &Path,
        port: u16,
        instance: SingleInstance,
    ) -> Result<Self> {
        let server = RpcServer::start(service.clone(), port)?;
        let published = server.endpoint().publish(&directory.join("daemon.json"))?;
        log::info!(
            "mirage-hub-daemon {} (Rust) listening on {}:{}; endpoint {}",
            env!("CARGO_PKG_VERSION"),
            server.endpoint().host,
            server.endpoint().port,
            published.path().display()
        );
        let cancel = Arc::new(Cancellation::default());
        let (stop, devices) = (cancel.clone(), service.clone());
        let watcher = thread::Builder::new()
            .name("openrazer-devices".into())
            .spawn(move || {
                devices.start();
                log::info!("initial USB device scan finished");
                if !stop.stopped() && devices.first_sweep_pending() {
                    if let Err(error) = devices.poll_hotplug(false) {
                        log::warn!("initial Bluetooth scan: {error}");
                    }
                }
                while !stop.wait(HOTPLUG_INTERVAL) {
                    if let Err(error) = devices.poll_hotplug(false) {
                        log::warn!("hotplug scan: {error}");
                    }
                }
            })?;
        Ok(Self {
            server,
            service,
            cancel,
            watcher: Some(watcher),
            published: Some(published),
            instance: Some(instance),
        })
    }
    pub fn endpoint(&self) -> &crate::rpc::Endpoint {
        self.server.endpoint()
    }
    pub fn wait_for_shutdown(&self, timeout: Duration) -> bool {
        self.server.wait_for_shutdown(timeout)
    }
    pub fn request_shutdown(&self) {
        self.server.request_shutdown();
        self.service.request_stop();
        self.cancel.stop();
    }
    pub fn stop(&mut self) {
        if self.instance.is_none() {
            return;
        }
        self.request_shutdown();
        self.server.stop();
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
        self.service.stop();
        self.published.take();
        self.instance.take();
        log::info!("stopped");
    }
}
impl Drop for RunningDaemon {
    fn drop(&mut self) {
        self.stop();
    }
}

/// An emulator uses its own instance and configuration, allowing tests/demo
/// sessions alongside the user's real daemon without claiming its HID handles.
pub fn instance_name(directory: &Path, emulate: bool) -> Result<String> {
    if !emulate {
        return Ok(SingleInstance::NAME.into());
    }
    std::fs::create_dir_all(directory)?;
    let path = std::fs::canonicalize(directory)?
        .to_string_lossy()
        .to_lowercase();
    let hash = path.as_bytes().iter().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    });
    Ok(format!("Local\\mirage-hub-daemon-emulator-{hash:016x}"))
}
