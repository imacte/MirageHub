//! Win32 HID and SetupAPI backend. Each OS resource has a single owner.

use super::{cache::DescriptionCache, HidBackend, HidHandle, HidInfo, InputReader};
use crate::{Error, Result};
use std::{
    collections::BTreeSet,
    ffi::c_void,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::{Mutex, OnceLock},
    time::Duration,
};
use windows_sys::Win32::{
    Devices::{DeviceAndDriverInstallation::*, HumanInterfaceDevice::*},
    Foundation::*,
    Storage::FileSystem::*,
    System::{Threading::*, IO::*},
};

pub struct NativeHid;

#[derive(serde::Serialize)]
pub struct AttachedDevice {
    pub instance_id: String,
    pub description: String,
    pub device_class: String,
    pub product_id: Option<u16>,
    pub transport: String,
}

fn identifier(instance: &str, prefix: &str) -> Option<u16> {
    let upper = instance.to_ascii_uppercase();
    let position = upper
        .find(&format!("{prefix}_"))
        .or_else(|| upper.find(&format!("{prefix}&")))?
        + prefix.len()
        + 1;
    let digits: String = upper[position..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .take(8)
        .collect();
    let width = if digits.len() >= 8 {
        8
    } else if digits.len() >= 4 {
        4
    } else {
        return None;
    };
    u16::from_str_radix(&digits[width - 4..width], 16).ok()
}

/// Read-only PnP inventory, including paired devices which expose no HID path.
pub fn attached_devices(vendor_id: u16) -> Result<Vec<AttachedDevice>> {
    let handle = unsafe {
        SetupDiGetClassDevsW(null(), null(), null_mut(), DIGCF_PRESENT | DIGCF_ALLCLASSES)
    };
    if handle == INVALID_HANDLE_VALUE as isize {
        return Err(failure("cannot enumerate attached devices"));
    }
    let set = DeviceSet(handle);
    let mut found = Vec::new();
    for index in 0.. {
        let mut data: SP_DEVINFO_DATA = unsafe { zeroed() };
        data.cbSize = size_of::<SP_DEVINFO_DATA>() as u32;
        if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut data) } == 0 {
            if unsafe { GetLastError() } == ERROR_NO_MORE_ITEMS {
                break;
            }
            return Err(failure("cannot enumerate device node"));
        }
        let mut buffer = [0u16; 512];
        if unsafe {
            SetupDiGetDeviceInstanceIdW(
                set.0,
                &data,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                null_mut(),
            )
        } == 0
        {
            continue;
        }
        let decode = |b: &[u16]| {
            String::from_utf16_lossy(&b[..b.iter().position(|v| *v == 0).unwrap_or(b.len())])
        };
        let instance_id = decode(&buffer);
        if identifier(&instance_id, "VID") != Some(vendor_id) {
            continue;
        }
        let property = |kind: u32| {
            let mut value = [0u16; 512];
            let ok = unsafe {
                SetupDiGetDeviceRegistryPropertyW(
                    set.0,
                    &data,
                    kind,
                    null_mut(),
                    value.as_mut_ptr().cast(),
                    size_of_val(&value) as u32,
                    null_mut(),
                )
            };
            if ok == 0 {
                String::new()
            } else {
                decode(&value)
            }
        };
        let friendly = property(SPDRP_FRIENDLYNAME);
        let description = if friendly.is_empty() {
            property(SPDRP_DEVICEDESC)
        } else {
            friendly
        };
        let head = instance_id
            .split('\\')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let transport = match head.as_str() {
            "BTHENUM" | "BTHHFENUM" => "Bluetooth",
            "BTHLEDEVICE" => "Bluetooth LE",
            "SWD" => "software",
            other => other,
        }
        .to_string();
        found.push(AttachedDevice {
            product_id: identifier(&instance_id, "PID"),
            instance_id,
            description,
            device_class: property(SPDRP_CLASS),
            transport,
        });
    }
    found.sort_by_key(|d| (d.transport.clone(), d.product_id, d.description.len()));
    Ok(found)
}

struct Handle(HANDLE);
// Handles may move between threads; this wrapper has no shared access and
// all device operations require &mut self on the owning transport/reader.
unsafe impl Send for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

fn failure(context: &str) -> Error {
    Error::Io(format!("{context}: {}", std::io::Error::last_os_error()))
}
pub(crate) fn wide(text: &str) -> Result<Vec<u16>> {
    if text.contains('\0') {
        return Err(Error::Invalid("a Windows path cannot contain NUL".into()));
    }
    Ok(text.encode_utf16().chain(Some(0)).collect())
}

fn open(path: &str, writable: bool, input: bool) -> Result<Handle> {
    let path = wide(path)?;
    let accesses: &[u32] = if input {
        &[GENERIC_READ | GENERIC_WRITE, GENERIC_READ]
    } else if writable {
        &[GENERIC_READ | GENERIC_WRITE, GENERIC_WRITE, 0]
    } else {
        &[0]
    };
    for access in accesses {
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                *access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                if input { FILE_FLAG_OVERLAPPED } else { 0 },
                null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE && !handle.is_null() {
            return Ok(Handle(handle));
        }
    }
    Err(failure("cannot open HID collection"))
}

fn string(
    handle: HANDLE,
    getter: unsafe extern "system" fn(HANDLE, *mut c_void, u32) -> BOOLEAN,
) -> String {
    let mut buffer = [0_u16; 256];
    if unsafe {
        getter(
            handle,
            buffer.as_mut_ptr().cast(),
            size_of::<[u16; 256]>() as u32,
        )
    } == 0
    {
        return String::new();
    }
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

fn describe(path: String) -> Result<(HidInfo, bool)> {
    let handle = open(&path, false, false)?;
    let mut attributes: HIDD_ATTRIBUTES = unsafe { zeroed() };
    attributes.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
    if unsafe { HidD_GetAttributes(handle.0, &mut attributes) } == 0 {
        return Err(failure("HidD_GetAttributes"));
    }
    let mut caps: HIDP_CAPS = unsafe { zeroed() };
    let mut preparsed = 0;
    let mut complete = false;
    if unsafe { HidD_GetPreparsedData(handle.0, &mut preparsed) } != 0 {
        unsafe {
            complete = HidP_GetCaps(preparsed, &mut caps) == HIDP_STATUS_SUCCESS;
            HidD_FreePreparsedData(preparsed);
        }
    }
    Ok((
        HidInfo {
            interface: HidInfo::path_index(&path, "&mi_"),
            collection: HidInfo::path_index(&path, "&col"),
            path,
            vendor_id: attributes.VendorID,
            product_id: attributes.ProductID,
            version: attributes.VersionNumber,
            usage_page: caps.UsagePage,
            usage: caps.Usage,
            feature_length: caps.FeatureReportByteLength,
            input_length: caps.InputReportByteLength,
            output_length: caps.OutputReportByteLength,
            manufacturer: string(handle.0, HidD_GetManufacturerString),
            product: string(handle.0, HidD_GetProductString),
            serial: string(handle.0, HidD_GetSerialNumberString),
        },
        complete,
    ))
}

impl HidBackend for NativeHid {
    fn write_interrupt(&self, info: &HidInfo, data: &[u8]) -> Result<()> {
        crate::inzone::native::Stream::open(&info.path, false)?.write(data)
    }
    fn enumerate(&self, vendor_id: Option<u16>, product_id: Option<u16>) -> Result<Vec<HidInfo>> {
        static CACHE: OnceLock<Mutex<DescriptionCache>> = OnceLock::new();
        // Serialize enumeration and cache pruning, including filtered callers.
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut present = BTreeSet::new();
        let mut guid = unsafe { zeroed() };
        unsafe {
            HidD_GetHidGuid(&mut guid);
        }
        let set = unsafe {
            SetupDiGetClassDevsW(
                &guid,
                null(),
                null_mut(),
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )
        };
        if set == -1 {
            return Err(failure("SetupDiGetClassDevsW"));
        }
        let set = DeviceSet(set);
        let mut found = Vec::new();
        let mut member = 0;
        loop {
            let mut interface: SP_DEVICE_INTERFACE_DATA = unsafe { zeroed() };
            interface.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if unsafe { SetupDiEnumDeviceInterfaces(set.0, null(), &guid, member, &mut interface) }
                == 0
            {
                if unsafe { GetLastError() } == ERROR_NO_MORE_ITEMS {
                    break;
                }
                return Err(failure("SetupDiEnumDeviceInterfaces"));
            }
            member += 1;
            let mut required = 0;
            unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    set.0,
                    &interface,
                    null_mut(),
                    0,
                    &mut required,
                    null_mut(),
                );
            }
            if required < size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 {
                continue;
            }
            // u64 backing storage gives the variable-length structure its
            // required alignment on both x86 and x86_64.
            let mut buffer = vec![0_u64; (required as usize).div_ceil(8)];
            let detail = buffer
                .as_mut_ptr()
                .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
            unsafe {
                (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            }
            if unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    set.0,
                    &interface,
                    detail,
                    required,
                    null_mut(),
                    null_mut(),
                )
            } == 0
            {
                continue;
            }
            let offset = std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
            let path = unsafe {
                std::slice::from_raw_parts(
                    buffer.as_ptr().cast::<u8>().add(offset).cast::<u16>(),
                    (required as usize - offset) / 2,
                )
            };
            let end = path.iter().position(|c| *c == 0).unwrap_or(path.len());
            let path = String::from_utf16_lossy(&path[..end]);
            // Include other vendors/products so a filtered scan cannot evict
            // their descriptors from the shared cache.
            present.insert(path.clone());
            let lower = path.to_ascii_lowercase();
            if vendor_id.is_some_and(|v| !lower.contains(&format!("vid_{v:04x}")))
                || product_id.is_some_and(|p| !lower.contains(&format!("pid_{p:04x}")))
            {
                continue;
            }
            if let Ok(info) = cache.get_or_describe(&path, || describe(path.clone())) {
                if vendor_id.is_none_or(|v| info.vendor_id == v)
                    && product_id.is_none_or(|p| info.product_id == p)
                {
                    found.push(info);
                }
            }
        }
        cache.retain_present(&present);
        Ok(found)
    }
    fn open(&self, info: &HidInfo) -> Result<Box<dyn HidHandle>> {
        Ok(Box::new(NativeHandle(open(&info.path, true, false)?)))
    }
    fn open_input(&self, info: &HidInfo) -> Result<Box<dyn InputReader>> {
        let handle = open(&info.path, false, true)?;
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            return Err(failure("CreateEventW"));
        }
        unsafe {
            HidD_SetNumInputBuffers(handle.0, 64);
        }
        Ok(Box::new(NativeReader {
            handle,
            event: Handle(event),
            length: usize::from(info.input_length).max(1),
        }))
    }
}

struct NativeHandle(Handle);
impl HidHandle for NativeHandle {
    fn send_feature(&mut self, data: &[u8]) -> Result<()> {
        if data.is_empty() || data.len() > u32::MAX as usize {
            return Err(Error::Invalid("invalid HID feature length".into()));
        }
        // HID's API uses a mutable void pointer even for an outgoing buffer.
        let mut data = data.to_vec();
        if unsafe { HidD_SetFeature(self.0 .0, data.as_mut_ptr().cast(), data.len() as u32) } == 0 {
            return Err(failure("HidD_SetFeature"));
        }
        Ok(())
    }
    fn get_feature(&mut self, length: usize, report_id: u8) -> Result<Vec<u8>> {
        if length == 0 || length > 65536 {
            return Err(Error::Invalid("invalid HID feature length".into()));
        }
        let mut data = vec![0; length];
        data[0] = report_id;
        if unsafe { HidD_GetFeature(self.0 .0, data.as_mut_ptr().cast(), length as u32) } == 0 {
            return Err(failure("HidD_GetFeature"));
        }
        Ok(data)
    }
    fn send_output(&mut self, data: &[u8]) -> Result<()> {
        if data.is_empty() || data.len() > u32::MAX as usize {
            return Err(Error::Invalid("invalid HID output length".into()));
        }
        let mut data = data.to_vec();
        if unsafe { HidD_SetOutputReport(self.0 .0, data.as_mut_ptr().cast(), data.len() as u32) }
            == 0
        {
            return Err(failure("HidD_SetOutputReport"));
        }
        Ok(())
    }
}

struct NativeReader {
    handle: Handle,
    event: Handle,
    length: usize,
}
impl InputReader for NativeReader {
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        unsafe {
            ResetEvent(self.event.0);
        }
        let mut overlapped: OVERLAPPED = unsafe { zeroed() };
        overlapped.hEvent = self.event.0;
        let mut data = vec![0; self.length];
        let started = unsafe {
            ReadFile(
                self.handle.0,
                data.as_mut_ptr(),
                data.len() as u32,
                null_mut(),
                &mut overlapped,
            )
        };
        if started == 0 {
            if unsafe { GetLastError() } != ERROR_IO_PENDING {
                return Err(failure("ReadFile"));
            }
            let wait = unsafe {
                WaitForSingleObject(
                    self.event.0,
                    timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32,
                )
            };
            if wait != WAIT_OBJECT_0 {
                let error = failure("waiting for HID input");
                let mut transferred = 0;
                // CancelIoEx only requests cancellation. Both the OVERLAPPED
                // and buffer must remain alive until the operation completes:
                // https://learn.microsoft.com/windows/win32/api/ioapiset/nf-ioapiset-cancelioex
                unsafe {
                    CancelIoEx(self.handle.0, &overlapped);
                    GetOverlappedResult(self.handle.0, &overlapped, &mut transferred, 1);
                }
                return if wait == WAIT_TIMEOUT {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
        }
        let mut transferred = 0;
        if unsafe { GetOverlappedResult(self.handle.0, &overlapped, &mut transferred, 0) } == 0 {
            return Err(failure("reading HID input"));
        }
        data.truncate(transferred as usize);
        Ok(Some(data))
    }
}
