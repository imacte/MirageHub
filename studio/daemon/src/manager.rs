//! Device discovery preserves live handles and separates slow radio sweeps
//! from the registry used by RPC clients and animation workers.

use crate::{
    ble::{Advertiser, BleBackend, BleTransport, SCAN_SECONDS},
    database::{self, DeviceInfo},
    device::{BlackSharkDevice, BleDevice, Device, KrakenDevice, StandardDevice},
    hid::{HidBackend, HidInfo},
    persistence::{self, SharedStore},
    protocol::{blackshark, ble},
    recipes::recipes,
    transport::{self, ReportChannel, Transport},
    Result,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

pub type SharedDevice = Arc<Mutex<Box<dyn Device>>>;
pub fn lock_device(device: &SharedDevice) -> MutexGuard<'_, Box<dyn Device>> {
    device.lock().unwrap_or_else(|error| error.into_inner())
}
const VENDOR_ID: u16 = 0x1532;

#[derive(Default)]
struct Registry {
    devices: Vec<(String, SharedDevice)>,
    hid_infos: Vec<HidInfo>,
    unsupported: Vec<(u16, String)>,
    identities: BTreeMap<u64, u64>,
    warnings: Vec<String>,
}

pub struct DeviceManager {
    hid: Arc<dyn HidBackend>,
    bluetooth: Option<Arc<dyn BleBackend>>,
    pub persistence: SharedStore,
    registry: Mutex<Registry>,
    scan_lock: Mutex<()>,
    swept_at: Mutex<Option<Instant>>,
    created_at: Instant,
    bluetooth_interval: Duration,
    first_sweep_done: AtomicBool,
}
impl DeviceManager {
    pub fn new(
        hid: Arc<dyn HidBackend>,
        bluetooth: Option<Arc<dyn BleBackend>>,
        persistence: SharedStore,
    ) -> Self {
        Self {
            hid,
            bluetooth,
            persistence,
            registry: Mutex::new(Registry::default()),
            scan_lock: Mutex::new(()),
            swept_at: Mutex::new(None),
            created_at: Instant::now(),
            bluetooth_interval: Duration::from_secs(60),
            first_sweep_done: AtomicBool::new(false),
        }
    }
    #[cfg(windows)]
    pub fn native(persistence: SharedStore, bluetooth: bool) -> Self {
        Self::new(
            Arc::new(crate::hid::windows::NativeHid),
            bluetooth.then(|| Arc::new(crate::ble::windows::NativeBle) as Arc<dyn BleBackend>),
            persistence,
        )
    }
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn sweep(&self, force: bool, warnings: &mut Vec<String>) -> Option<Vec<Advertiser>> {
        let backend = self.bluetooth.as_ref()?;
        let now = Instant::now();
        let mut last = self.swept_at.lock().unwrap_or_else(|e| e.into_inner());
        if !force && !bluetooth_due(now, self.created_at, *last, self.bluetooth_interval) {
            return None;
        }
        *last = Some(now);
        let result = backend.scan(SCAN_SECONDS, Some(ble::COMPANY_ID));
        match result {
            Ok(mut advertisers) => {
                advertisers.sort_by_key(|a| std::cmp::Reverse(a.rssi));
                Some(advertisers)
            }
            Err(error) => {
                warnings.push(format!("Bluetooth scan failed: {error}"));
                None
            }
        }
    }
    pub fn scan(&self, force_bluetooth: bool) -> Result<Vec<SharedDevice>> {
        let result = self.scan_inner(true, force_bluetooth);
        // Finish only after discovery publishes its result, including errors
        // from a disabled radio. The UI must not keep searching forever.
        if self
            .swept_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            self.first_sweep_done.store(true, Ordering::Release);
        }
        result
    }
    /// Publish USB devices before waiting for advertisements (upstream #15).
    pub fn scan_usb(&self) -> Result<Vec<SharedDevice>> {
        self.scan_inner(false, false)
    }
    pub fn first_sweep_pending(&self) -> bool {
        self.bluetooth.is_some() && !self.first_sweep_done.load(Ordering::Acquire)
    }
    pub fn hid_infos(&self) -> Vec<HidInfo> {
        self.registry().hid_infos.clone()
    }
    fn scan_inner(
        &self,
        sweep_bluetooth: bool,
        force_bluetooth: bool,
    ) -> Result<Vec<SharedDevice>> {
        let _scan = self.scan_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut warnings = Vec::new();
        let advertisers = if sweep_bluetooth {
            self.sweep(force_bluetooth, &mut warnings)
        } else {
            None
        };
        let hid_infos = self.hid.enumerate(Some(VENDOR_ID), None)?;
        let mut by_pid: BTreeMap<u16, Vec<HidInfo>> = BTreeMap::new();
        for info in hid_infos.iter().cloned() {
            by_pid.entry(info.product_id).or_default().push(info);
        }
        // These snapshots permit list/get requests during device I/O and radio sweeps.
        let (mut devices, mut identities) = {
            let registry = self.registry();
            (registry.devices.clone(), registry.identities.clone())
        };
        let mut seen = BTreeSet::new();
        let mut unsupported = Vec::new();
        for (pid, infos) in by_pid {
            let Some(meta) = database::lookup(VENDOR_ID, pid) else {
                unsupported.push((pid, infos[0].product.clone()));
                continue;
            };
            if meta.transport == "ble" {
                continue;
            }
            let key = meta.key();
            seen.insert(key.clone());
            if devices.iter().any(|(k, _)| k == &key) {
                continue;
            }
            match self.build_hid(meta, &infos) {
                Ok(Some(device)) => devices.push((key, Arc::new(Mutex::new(device)))),
                Ok(None) => warnings.push(format!(
                    "{} exposes no usable control report; another program may hold it exclusively",
                    meta.name
                )),
                Err(error) => warnings.push(format!("cannot open {}: {error}", meta.name)),
            }
        }
        if let Some(advertisers) = &advertisers {
            for advertiser in advertisers {
                let Some(meta) = match_advertiser(advertiser) else {
                    continue;
                };
                let address = advertiser.address;
                if let Some(classic) = advertiser.classic_address {
                    identities.insert(address, classic);
                }
                let twin = identities.get(&address).and_then(|classic| {
                    devices.iter().find(|(key, _)| {
                        key.strip_prefix("ble:")
                            .and_then(|value| u64::from_str_radix(value, 16).ok())
                            .is_some_and(|other| {
                                other != address && identities.get(&other) == Some(classic)
                            })
                    })
                });
                if let Some((key, _)) = twin {
                    seen.insert(key.clone());
                    continue;
                }
                let key = format!("ble:{address:012x}");
                seen.insert(key.clone());
                if devices.iter().any(|(k, _)| k == &key) {
                    continue;
                }
                let Some(backend) = &self.bluetooth else {
                    continue;
                };
                match BleTransport::new(address, backend.clone()) {
                    Ok(transport) => {
                        let mut device = BleDevice::new(
                            meta.clone(),
                            Box::new(transport),
                            self.persistence.clone(),
                        );
                        device.classic_address = identities.get(&address).copied();
                        devices.push((key, Arc::new(Mutex::new(Box::new(device)))));
                    }
                    Err(error) => {
                        warnings.push(format!("cannot open {} over Bluetooth: {error}", meta.name))
                    }
                }
            }
        }
        devices.retain(|(key, device)| {
            if seen.contains(key)
                || (key.starts_with("ble:")
                    && (advertisers.is_none() || lock_device(device).is_connected()))
            {
                return true;
            }
            lock_device(device).close();
            false
        });
        let result = devices.iter().map(|(_, device)| device.clone()).collect();
        *self.registry() = Registry {
            devices,
            hid_infos,
            unsupported,
            identities,
            warnings,
        };
        Ok(result)
    }
    fn build_hid(&self, meta: &DeviceInfo, infos: &[HidInfo]) -> Result<Option<Box<dyn Device>>> {
        let device: Box<dyn Device> = if recipes().is_kraken(meta.pid) {
            let Some(control) = transport::select_kraken(infos) else {
                return Ok(None);
            };
            let transport = Transport::new(
                self.hid.clone(),
                control.clone(),
                600,
                None,
                Some(control.clone()),
            );
            Box::new(KrakenDevice::new(
                meta.clone(),
                Box::new(transport),
                self.persistence.clone(),
            ))
        } else if [blackshark::PID_WIRED, blackshark::PID_WIRELESS].contains(&meta.pid) {
            let Some(control) = transport::select_blackshark(infos) else {
                return Ok(None);
            };
            Box::new(BlackSharkDevice::new(
                meta.clone(),
                Box::new(ReportChannel::new(self.hid.clone(), control.clone())),
                self.persistence.clone(),
            ))
        } else {
            let params = recipes().transport_params(meta.driver(), meta.pid)?;
            let Some(control) = transport::select_control(infos, Some(params.index)) else {
                return Ok(None);
            };
            let argb = infos
                .iter()
                .find(|info| info.feature_length == 321)
                .cloned();
            Box::new(StandardDevice::new(
                meta.clone(),
                Box::new(Transport::new(
                    self.hid.clone(),
                    control.clone(),
                    params.wait_us,
                    argb,
                    None,
                )),
                self.persistence.clone(),
            ))
        };
        Ok(Some(device))
    }
    pub fn devices(&self) -> Vec<SharedDevice> {
        self.registry()
            .devices
            .iter()
            .map(|(_, device)| device.clone())
            .collect()
    }
    pub fn unsupported(&self) -> Vec<(u16, String)> {
        self.registry().unsupported.clone()
    }
    pub fn warnings(&self) -> Vec<String> {
        self.registry().warnings.clone()
    }
    pub fn by_serial(&self, serial: &str) -> Option<SharedDevice> {
        self.devices()
            .into_iter()
            .find(|device| lock_device(device).serial() == serial)
    }
    pub fn by_pid(&self, pid: u16) -> Option<SharedDevice> {
        self.devices()
            .into_iter()
            .find(|device| lock_device(device).info().pid == pid)
    }
    pub fn find(&self, needle: &str) -> Vec<SharedDevice> {
        let needle = needle.trim().to_lowercase();
        let mut matches = Vec::new();
        for device in self.devices() {
            let mut locked = lock_device(&device);
            if needle == locked.serial().to_lowercase()
                || needle == format!("{:04x}", locked.info().pid)
            {
                drop(locked);
                return vec![device];
            }
            if locked.info().name.to_lowercase().contains(&needle) {
                matches.push(device.clone());
            }
        }
        matches
    }
    pub fn close(&self) -> Result<()> {
        let _scan = self.scan_lock.lock().unwrap_or_else(|e| e.into_inner());
        let devices = std::mem::take(&mut self.registry().devices);
        for (_, device) in devices {
            lock_device(&device).close();
        }
        persistence::lock(&self.persistence).save()
    }
}

fn bluetooth_due(
    now: Instant,
    created: Instant,
    last: Option<Instant>,
    interval: Duration,
) -> bool {
    let Some(last) = last else { return true };
    let interval = if now.duration_since(created) < Duration::from_secs(180) {
        interval.min(Duration::from_secs(10))
    } else {
        interval
    };
    now.duration_since(last) >= interval
}
fn match_advertiser(advertiser: &Advertiser) -> Option<&'static DeviceInfo> {
    advertiser
        .product_id
        .and_then(|pid| database::lookup(VENDOR_ID, pid))
        .filter(|m| m.transport == "ble")
        .or_else(|| {
            (advertiser.name == "Razer Stereo")
                .then(|| database::lookup(VENDOR_ID, 0x562))
                .flatten()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_starts_quickly_then_uses_the_normal_radio_interval() {
        let created = Instant::now();
        assert!(bluetooth_due(
            created,
            created,
            None,
            Duration::from_secs(60)
        ));
        assert!(!bluetooth_due(
            created + Duration::from_secs(9),
            created,
            Some(created),
            Duration::from_secs(60)
        ));
        assert!(bluetooth_due(
            created + Duration::from_secs(10),
            created,
            Some(created),
            Duration::from_secs(60)
        ));
        let last = created + Duration::from_secs(180);
        assert!(!bluetooth_due(
            last + Duration::from_secs(10),
            created,
            Some(last),
            Duration::from_secs(60)
        ));
        assert!(bluetooth_due(
            last + Duration::from_secs(60),
            created,
            Some(last),
            Duration::from_secs(60)
        ));
    }
}
