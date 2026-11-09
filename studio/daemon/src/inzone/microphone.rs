// SPDX-License-Identifier: GPL-3.0-only
// Endpoint topology matching based on OpenInzone, Copyright (C) 2026 penguinwokrs.
//! Change only the capture endpoint attached to the selected Sony USB product.
use crate::{Error, Result};
#[cfg(windows)]
pub fn level(vid: u16, pid: u16, set: Option<u8>) -> Result<u8> {
    use windows::Win32::{
        Media::Audio::{
            eCapture, Endpoints::IAudioEndpointVolume, IDeviceTopology, IMMDeviceEnumerator,
            MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
            COINIT_MULTITHREADED,
        },
    };
    let work = || -> windows::core::Result<Option<u8>> {
        unsafe {
            let initialized = CoInitializeEx(None, COINIT_MULTITHREADED);
            // RPC workers usually initialize here. If a GUI apartment already owns
            // this thread, use it without balancing someone else's initialization.
            struct Com(bool);
            impl Drop for Com {
                fn drop(&mut self) {
                    if self.0 {
                        unsafe {
                            CoUninitialize();
                        }
                    }
                }
            }
            if initialized.is_err() && initialized.0 != 0x80010106u32 as i32 {
                initialized.ok()?;
            }
            let _com = Com(initialized.is_ok());
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let endpoints = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
            let needle = format!("vid_{vid:04x}&pid_{pid:04x}");
            let mut matching = Vec::new();
            for index in 0..endpoints.GetCount()? {
                let device = endpoints.Item(index)?;
                let Ok(topology): windows::core::Result<IDeviceTopology> =
                    device.Activate(CLSCTX_ALL, None)
                else {
                    continue;
                };
                let Ok(connector) = topology.GetConnector(0) else {
                    continue;
                };
                let Ok(id) = connector.GetDeviceIdConnectedTo() else {
                    continue;
                };
                let name = id.to_string().unwrap_or_default();
                CoTaskMemFree(Some(id.0.cast()));
                if name.to_ascii_lowercase().contains(&needle) {
                    matching.push(device);
                }
            }
            // Never alter an arbitrary microphone when two identical receivers exist.
            if matching.len() != 1 {
                return Ok(None);
            }
            let volume: IAudioEndpointVolume = matching[0].Activate(CLSCTX_ALL, None)?;
            if let Some(value) = set {
                volume.SetMasterVolumeLevelScalar(
                    f32::from(value) / 100.0,
                    &windows::core::GUID::zeroed(),
                )?;
            }
            Ok(Some(
                (volume.GetMasterVolumeLevelScalar()?.clamp(0.0, 1.0) * 100.0).round() as u8,
            ))
        }
    };
    work()
        .map_err(|e| Error::Io(format!("INZONE microphone: {e}")))?
        .ok_or_else(|| {
            Error::NotSupported(
                "No unique Windows microphone endpoint for this INZONE receiver".into(),
            )
        })
}
#[cfg(not(windows))]
pub fn level(_vid: u16, _pid: u16, _set: Option<u8>) -> Result<u8> {
    Err(Error::NotSupported(
        "Microphone control requires Windows".into(),
    ))
}
