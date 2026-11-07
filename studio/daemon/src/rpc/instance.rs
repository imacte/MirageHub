use crate::Result;

/// Same named object as the existing Python daemon. Ownership is the lifetime
/// of the handle, not mutex ownership, so shutdown may happen on another thread.
pub struct SingleInstance {
    #[cfg(windows)]
    _handle: std::os::windows::io::OwnedHandle,
}
impl SingleInstance {
    pub const NAME: &'static str = "Local\\openrazer-win-daemon";
    pub fn acquire(name: &str) -> Result<Option<Self>> {
        #[cfg(windows)]
        {
            use std::{
                os::windows::io::{FromRawHandle, OwnedHandle},
                ptr,
            };
            use windows_sys::Win32::{
                Foundation::{GetLastError, ERROR_ALREADY_EXISTS},
                System::Threading::CreateMutexW,
            };
            let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
            unsafe {
                let handle = CreateMutexW(ptr::null(), 0, name.as_ptr());
                if handle.is_null() {
                    return Err(std::io::Error::last_os_error().into());
                }
                let existed = GetLastError() == ERROR_ALREADY_EXISTS;
                let handle = OwnedHandle::from_raw_handle(handle);
                Ok((!existed).then_some(Self { _handle: handle }))
            }
        }
        #[cfg(not(windows))]
        {
            let _ = name;
            Err(crate::Error::NotSupported(
                "the daemon requires Windows".into(),
            ))
        }
    }
}
