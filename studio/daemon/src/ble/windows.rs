//! Windows Runtime GATT and advertisement access; no optional Python runtime.
//! A deadline is shared across each whole operation, including cold-link retries.

use super::{remaining, Advertisements, Advertiser, BleBackend, GattLink};
use crate::{protocol::ble, Error, Result};
use serde_json::{json, Value};
use std::{
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use windows::{
    core::{RuntimeType, GUID},
    Devices::Bluetooth::{
        Advertisement::{
            BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
            BluetoothLEAdvertisementWatcherStoppedEventArgs, BluetoothLEScanningMode,
        },
        BluetoothCacheMode, BluetoothConnectionStatus, BluetoothLEDevice,
        GenericAttributeProfile::{
            GattCharacteristic, GattClientCharacteristicConfigurationDescriptorValue,
            GattCommunicationStatus, GattDeviceService, GattValueChangedEventArgs, GattWriteOption,
        },
    },
    Foundation::TypedEventHandler,
    Storage::Streams::{DataReader, DataWriter, IBuffer},
    Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
};
use windows_future::IAsyncOperation;

impl From<windows::core::Error> for Error {
    fn from(error: windows::core::Error) -> Self {
        Self::Device(format!("Windows: {error}"))
    }
}

struct Apartment;
impl Apartment {
    fn new() -> Result<Self> {
        // Called only by dedicated BLE threads, never by Studio's UI apartment.
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}

fn wait<T: RuntimeType + Send + 'static>(
    operation: IAsyncOperation<T>,
    deadline: Instant,
) -> Result<T> {
    let (send, receive) = mpsc::sync_channel(1);
    operation.when(move |value| {
        let _ = send.send(value);
    })?;
    let outcome = remaining(deadline).and_then(|duration| {
        receive
            .recv_timeout(duration)
            .map_err(|_| Error::Device("Windows Bluetooth operation timed out".into()))
    });
    match outcome {
        Ok(result) => result.map_err(Into::into),
        Err(error) => {
            let _ = operation.Cancel();
            Err(error)
        }
    }
}
fn read_buffer(buffer: &IBuffer) -> windows::core::Result<Vec<u8>> {
    let reader = DataReader::FromBuffer(buffer)?;
    let mut bytes = vec![0; reader.UnconsumedBufferLength()? as usize];
    reader.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

pub struct NativeBle;
impl BleBackend for NativeBle {
    fn scan(&self, duration: Duration, company: Option<u16>) -> Result<Vec<Advertiser>> {
        thread::Builder::new()
            .name("openrazer-ble-scan".into())
            .spawn(move || scan(duration, company))?
            .join()
            .map_err(|_| Error::Device("Bluetooth scan worker stopped".into()))?
    }
    fn connect(
        &self,
        address: u64,
        uncached: bool,
        deadline: Instant,
    ) -> Result<Box<dyn GattLink>> {
        Connection::open(address, uncached, deadline)
            .map(|link| Box::new(link) as Box<dyn GattLink>)
    }
}

struct Watcher {
    watcher: BluetoothLEAdvertisementWatcher,
    received: i64,
    stopped: Option<i64>,
}
impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.watcher.Stop();
        let _ = self.watcher.RemoveReceived(self.received);
        if let Some(token) = self.stopped {
            let _ = self.watcher.RemoveStopped(token);
        }
    }
}
fn scan(duration: Duration, company: Option<u16>) -> Result<Vec<Advertiser>> {
    let _apartment = Apartment::new()?;
    let entries = Arc::new(Mutex::new(Advertisements::default()));
    let captured = entries.clone();
    let watcher = BluetoothLEAdvertisementWatcher::new()?;
    watcher.SetScanningMode(BluetoothLEScanningMode::Active)?;
    let received = watcher.Received(&TypedEventHandler::<
        BluetoothLEAdvertisementWatcher,
        BluetoothLEAdvertisementReceivedEventArgs,
    >::new(move |_, args| {
        let args = args.ok()?;
        let advertisement = args.Advertisement()?;
        let address = args.BluetoothAddress()?;
        let name = advertisement
            .LocalName()
            .map(|name| name.to_string())
            .unwrap_or_default();
        let rssi = args.RawSignalStrengthInDBm().unwrap_or_default();
        let mut sections = Vec::new();
        if let Ok(data) = advertisement.ManufacturerData() {
            for section in data {
                if let Ok(company) = section.CompanyId() {
                    let payload = section
                        .Data()
                        .and_then(|buffer| read_buffer(&buffer))
                        .unwrap_or_default();
                    sections.push((company, payload));
                }
            }
        }
        captured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .add(address, &name, rssi, &sections);
        Ok(())
    }))?;
    let mut guard = Watcher {
        watcher,
        received,
        stopped: None,
    };
    let (send, receive) = mpsc::sync_channel(1);
    guard.stopped = Some(guard.watcher.Stopped(&TypedEventHandler::<
        BluetoothLEAdvertisementWatcher,
        BluetoothLEAdvertisementWatcherStoppedEventArgs,
    >::new(move |_, args| {
        let _ = send.try_send(args.ok()?.Error()?.0);
        Ok(())
    }))?);
    guard.watcher.Start()?;
    if let Ok(error) = receive.recv_timeout(duration) {
        if error != 0 {
            return Err(Error::Device(format!(
                "Bluetooth scan stopped (error {error})"
            )));
        }
    }
    drop(guard);
    let result = entries
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .finish(company);
    Ok(result)
}

struct Connection {
    write: Option<GattCharacteristic>,
    notify: Option<GattCharacteristic>,
    services: Vec<GattDeviceService>,
    device: BluetoothLEDevice,
    token: Option<i64>,
    notifications: Option<mpsc::Receiver<Vec<u8>>>,
    address: u64,
    // Drop every WinRT interface before uninitializing its apartment.
    _apartment: Apartment,
}
impl Connection {
    fn open(address: u64, uncached: bool, deadline: Instant) -> Result<Self> {
        let apartment = Apartment::new()?;
        remaining(deadline)?;
        let device = wait(
            BluetoothLEDevice::FromBluetoothAddressAsync(address)?,
            deadline,
        )?;
        let mut link = Self {
            write: None,
            notify: None,
            services: Vec::new(),
            device,
            token: None,
            notifications: None,
            address,
            _apartment: apartment,
        };
        let mode = if uncached {
            BluetoothCacheMode::Uncached
        } else {
            BluetoothCacheMode::Cached
        };
        remaining(deadline)?;
        let result = wait(
            link.device.GetGattServicesWithCacheModeAsync(mode)?,
            deadline,
        )?;
        check_status(result.Status()?, "GATT service discovery")?;
        link.services = result.Services()?.into_iter().collect();
        let mut refused = false;
        let write_uuid = GUID::from_u128(0x416d0000_2d52_617a_6572_424c4501f40a);
        let notify_uuid = GUID::from_u128(0x416d0001_2d52_617a_6572_424c4501f40a);
        for service in &link.services {
            remaining(deadline)?;
            let result = wait(service.GetCharacteristicsAsync()?, deadline)?;
            let status = result.Status()?;
            if status == GattCommunicationStatus::AccessDenied {
                refused = true;
            }
            if status != GattCommunicationStatus::Success {
                continue;
            }
            for characteristic in result.Characteristics()? {
                let uuid = characteristic.Uuid()?;
                if uuid == write_uuid {
                    link.write = Some(characteristic);
                } else if uuid == notify_uuid {
                    link.notify = Some(characteristic);
                }
            }
        }
        if link.write.is_none() {
            let reason = if refused {
                "another program holds the GATT service exclusively; close Razer Synapse and retry"
            } else if link.services.is_empty() {
                "Windows returned no GATT services; the link may not be up yet"
            } else {
                "the lighting characteristic was not found"
            };
            return Err(Error::Device(format!("device {address:012X}: {reason}")));
        }
        Ok(link)
    }
}
fn check_status(status: GattCommunicationStatus, action: &str) -> Result<()> {
    if status == GattCommunicationStatus::Success {
        Ok(())
    } else {
        Err(Error::Device(format!(
            "{action} failed (status {})",
            status.0
        )))
    }
}
impl GattLink for Connection {
    fn write(&mut self, payload: &[u8], deadline: Instant) -> Result<()> {
        remaining(deadline)?;
        let writer = DataWriter::new()?;
        writer.WriteBytes(payload)?;
        let result = wait(
            self.write
                .as_ref()
                .expect("resolved write characteristic")
                .WriteValueWithResultAndOptionAsync(
                    &writer.DetachBuffer()?,
                    GattWriteOption::WriteWithoutResponse,
                )?,
            deadline,
        )?;
        check_status(result.Status()?, "Bluetooth write")
    }
    fn subscribe(&mut self, deadline: Instant) -> Result<()> {
        if self.token.is_some() {
            return Ok(());
        }
        let notify = self
            .notify
            .as_ref()
            .ok_or_else(|| Error::NotSupported("device exposes no notify characteristic".into()))?;
        let (send, receive) = mpsc::sync_channel(64);
        let token = notify.ValueChanged(&TypedEventHandler::<
            GattCharacteristic,
            GattValueChangedEventArgs,
        >::new(move |_, args| {
            if let Ok(buffer) = args.ok().and_then(|a| a.CharacteristicValue()) {
                if let Ok(bytes) = read_buffer(&buffer) {
                    let _ = send.try_send(bytes);
                }
            }
            Ok(())
        }))?;
        // Retain the token even on failure so Drop always unregisters it.
        self.token = Some(token);
        let result = remaining(deadline).and_then(|_| {
            let status = wait(
                notify.WriteClientCharacteristicConfigurationDescriptorAsync(
                    GattClientCharacteristicConfigurationDescriptorValue::Notify,
                )?,
                deadline,
            )?;
            check_status(status, "Bluetooth notification subscription")
        });
        if result.is_err() {
            let _ = notify.RemoveValueChanged(token);
            self.token = None;
        }
        result?;
        self.notifications = Some(receive);
        Ok(())
    }
    fn discard_notifications(&mut self) {
        if let Some(receiver) = &self.notifications {
            for _ in 0..64 {
                if receiver.try_recv().is_err() {
                    break;
                }
            }
        }
    }
    fn notification(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>> {
        let Some(receiver) = &self.notifications else {
            return Err(Error::Device("notification subscription is missing".into()));
        };
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Error::Device(
                "Bluetooth notification channel closed".into(),
            )),
        }
    }
    fn is_connected(&self) -> bool {
        self.device.ConnectionStatus().ok() == Some(BluetoothConnectionStatus::Connected)
    }
    fn describe(&self) -> Result<Value> {
        Ok(
            json!({"name":self.device.Name()?.to_string(),"address":self.address,
            "characteristic":ble::WRITE_CHARACTERISTIC,
            "properties":self.write.as_ref().expect("resolved write characteristic").CharacteristicProperties()?.0}),
        )
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let (Some(notify), Some(token)) = (&self.notify, self.token.take()) {
            let _ = notify.RemoveValueChanged(token);
        }
        for service in &self.services {
            let _ = service.Close();
        }
        let _ = self.device.Close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn winrt_completion_returns_results_and_a_hung_operation_respects_the_deadline() {
        let _apartment = Apartment::new().unwrap();
        let value = wait(
            IAsyncOperation::spawn(|| Ok(87u32)),
            Instant::now() + Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(value, 87);
        let (release, receive) = mpsc::channel();
        let (ended, wait_ended) = mpsc::channel();
        let slow = IAsyncOperation::spawn(move || {
            let _ = receive.recv_timeout(Duration::from_secs(3));
            let _ = ended.send(());
            Ok(9u32)
        });
        let result = wait(slow, Instant::now() + Duration::from_millis(20));
        let _ = release.send(());
        wait_ended.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(result.unwrap_err().to_string().contains("timed out"));
        assert_eq!(
            wait(
                IAsyncOperation::ready(Ok(42u32)),
                Instant::now() + Duration::from_secs(3)
            )
            .unwrap(),
            42
        );
    }
}
