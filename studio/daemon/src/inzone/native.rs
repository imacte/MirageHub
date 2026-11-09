// SPDX-License-Identifier: GPL-3.0-only
// Serial settings and discovery based on OpenInzone, Copyright (C) 2026 penguinwokrs.
//! Bounded overlapped I/O. Cancellation is drained before buffers are released.
use crate::{hid::HidInfo, Error, Result};
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    time::Duration,
};
use windows_sys::{
    core::GUID,
    Win32::{
        Devices::{Communication::*, DeviceAndDriverInstallation::*},
        Foundation::*,
        Storage::FileSystem::*,
        System::{Threading::*, IO::*},
    },
};

struct Handle(HANDLE);
unsafe impl Send for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn failure(call: &str) -> Error {
    Error::Io(format!("{call}: {}", std::io::Error::last_os_error()))
}
pub struct Stream {
    handle: Handle,
    event: Handle,
}
impl Stream {
    pub fn open(path: &str, serial: bool) -> Result<Self> {
        if path.contains('\0') {
            return Err(Error::Invalid("NUL in device path".into()));
        }
        let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let raw = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                if serial {
                    0
                } else {
                    FILE_SHARE_READ | FILE_SHARE_WRITE
                },
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE || raw.is_null() {
            return Err(failure(if serial {
                "Cannot open INZONE H9/H7 serial port; close INZONE Hub or another controller using it"
            } else {
                "Cannot open INZONE HID output"
            }));
        }
        let handle = Handle(raw);
        if serial {
            let mut dcb: DCB = unsafe { zeroed() };
            dcb.DCBlength = size_of::<DCB>() as u32;
            if unsafe { GetCommState(handle.0, &mut dcb) } == 0 {
                return Err(failure("GetCommState"));
            }
            dcb.BaudRate = 460800;
            dcb.ByteSize = 8;
            dcb.Parity = NOPARITY;
            dcb.StopBits = ONESTOPBIT;
            dcb._bitfield = 1 | (1 << 4) | (1 << 12);
            if unsafe { SetCommState(handle.0, &dcb) } == 0 {
                return Err(failure("SetCommState"));
            }
            let timeouts = COMMTIMEOUTS {
                ReadIntervalTimeout: u32::MAX,
                ReadTotalTimeoutMultiplier: u32::MAX,
                ReadTotalTimeoutConstant: 60000,
                WriteTotalTimeoutMultiplier: 0,
                WriteTotalTimeoutConstant: 1000,
            };
            if unsafe { SetCommTimeouts(handle.0, &timeouts) } == 0 {
                return Err(failure("SetCommTimeouts"));
            }
            if unsafe { PurgeComm(handle.0, PURGE_RXCLEAR | PURGE_TXCLEAR) } == 0 {
                return Err(failure("PurgeComm"));
            }
        }
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            return Err(failure("CreateEventW"));
        }
        Ok(Self {
            handle,
            event: Handle(event),
        })
    }
    fn transfer(&mut self, bytes: &mut [u8], write: bool, timeout: Duration) -> Result<usize> {
        let mut ov: OVERLAPPED = unsafe { zeroed() };
        ov.hEvent = self.event.0;
        unsafe {
            ResetEvent(self.event.0);
        }
        let started = unsafe {
            if write {
                WriteFile(
                    self.handle.0,
                    bytes.as_ptr(),
                    bytes.len() as u32,
                    null_mut(),
                    &mut ov,
                )
            } else {
                ReadFile(
                    self.handle.0,
                    bytes.as_mut_ptr(),
                    bytes.len() as u32,
                    null_mut(),
                    &mut ov,
                )
            }
        };
        if started == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(failure("INZONE I/O"));
        }
        let wait = if started != 0 {
            WAIT_OBJECT_0
        } else {
            unsafe {
                WaitForSingleObject(
                    self.event.0,
                    timeout.as_millis().min(u32::MAX as u128 - 1) as u32,
                )
            }
        };
        let mut count = 0;
        if wait != WAIT_OBJECT_0 {
            unsafe {
                CancelIoEx(self.handle.0, &ov);
            }
            let done = unsafe { GetOverlappedResult(self.handle.0, &ov, &mut count, 1) };
            // A read completing during cancellation still belongs to the stream.
            if done != 0 {
                return Ok(count as usize);
            }
            if wait == WAIT_TIMEOUT
                && !write
                && unsafe { GetLastError() } == ERROR_OPERATION_ABORTED
            {
                return Ok(0);
            }
            return Err(Error::Io("INZONE I/O timed out or was cancelled".into()));
        }
        if unsafe { GetOverlappedResult(self.handle.0, &ov, &mut count, 0) } == 0 {
            return Err(failure("INZONE completion"));
        }
        Ok(count as usize)
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let count = self.transfer(&mut bytes.to_vec(), true, Duration::from_secs(1))?;
        if count != bytes.len() {
            return Err(Error::Io("Short INZONE write".into()));
        }
        Ok(())
    }
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        let mut bytes = vec![0; 256];
        let count = self.transfer(&mut bytes, false, timeout)?;
        bytes.truncate(count);
        Ok((count != 0).then_some(bytes))
    }
}

pub struct SerialIo {
    path: String,
    stream: Option<Stream>,
}
impl SerialIo {
    pub fn new(path: String) -> Self {
        Self { path, stream: None }
    }
}
impl super::Io for SerialIo {
    fn write(&mut self, data: &[u8]) -> Result<()> {
        if self.stream.is_none() {
            self.stream = Some(Stream::open(&self.path, true)?);
        }
        self.stream.as_mut().unwrap().write(data)
    }
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        self.stream
            .as_mut()
            .ok_or_else(|| Error::Device("Serial port closed".into()))?
            .read(timeout)
    }
    fn close(&mut self) {
        self.stream = None;
    }
}

/// Enumerate only the known Sony serial dongle; no unrelated serial ports are opened.
pub fn serial_receivers() -> Result<Vec<HidInfo>> {
    let guid = GUID::from_u128(0x86e0d1e0_8089_11d0_9ce4_08003e301f73);
    let set = unsafe {
        SetupDiGetClassDevsW(
            &guid,
            null(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if set == -1 {
        return Err(failure("Enumerate INZONE serial interfaces"));
    }
    struct Set(isize);
    impl Drop for Set {
        fn drop(&mut self) {
            unsafe {
                SetupDiDestroyDeviceInfoList(self.0);
            }
        }
    }
    let set = Set(set);
    let mut found = Vec::new();
    for index in 0.. {
        let mut interface: SP_DEVICE_INTERFACE_DATA = unsafe { zeroed() };
        interface.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        if unsafe { SetupDiEnumDeviceInterfaces(set.0, null(), &guid, index, &mut interface) } == 0
        {
            if unsafe { GetLastError() } == ERROR_NO_MORE_ITEMS {
                break;
            }
            return Err(failure("Enumerate serial interface"));
        }
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
        let mut buffer = vec![0u64; (required as usize).div_ceil(8)];
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
        let chars = unsafe {
            std::slice::from_raw_parts(
                buffer.as_ptr().cast::<u8>().add(offset).cast::<u16>(),
                (required as usize - offset) / 2,
            )
        };
        let path = String::from_utf16_lossy(
            &chars[..chars.iter().position(|b| *b == 0).unwrap_or(chars.len())],
        );
        if path.to_ascii_lowercase().contains("vid_054c&pid_0e53") {
            found.push(HidInfo {
                path,
                vendor_id: 0x054c,
                product_id: 0x0e53,
                product: "Sony INZONE H9 / H7".into(),
                ..Default::default()
            });
        }
    }
    Ok(found)
}
