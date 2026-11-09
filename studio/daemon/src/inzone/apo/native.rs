// SPDX-License-Identifier: GPL-3.0-only
use super::{components, document::Document, invalid};
use crate::{Error, Result};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};
use windows::Win32::{
    Media::Audio::{
        eRender, IAudioClient, IDeviceTopology, IMMDeviceEnumerator, MMDeviceEnumerator,
        DEVICE_STATE_ACTIVE,
    },
    System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    },
};
use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};

struct Endpoint {
    guid: String,
    path: PathBuf,
    bound: bool,
    disabled: bool,
}
fn endpoint() -> Result<Endpoint> {
    let work = || -> windows::core::Result<Vec<Endpoint>> {
        unsafe {
            let init = CoInitializeEx(None, COINIT_MULTITHREADED);
            if init.is_err() && init.0 != 0x80010106u32 as i32 {
                init.ok()?;
            }
            struct Com(bool);
            impl Drop for Com {
                fn drop(&mut self) {
                    if self.0 {
                        unsafe { CoUninitialize() }
                    }
                }
            }
            let _com = Com(init.is_ok());
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            let mut found = Vec::new();
            for i in 0..devices.GetCount()? {
                let device = devices.Item(i)?;
                let Ok(topology): windows::core::Result<IDeviceTopology> =
                    device.Activate(CLSCTX_ALL, None)
                else {
                    continue;
                };
                let Ok(connector) = topology.GetConnector(0) else {
                    continue;
                };
                let Ok(connected) = connector.GetDeviceIdConnectedTo() else {
                    continue;
                };
                let name = connected
                    .to_string()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                CoTaskMemFree(Some(connected.0.cast()));
                if !name.contains("vid_054c&pid_0ebf") {
                    continue;
                }
                let Ok(client): windows::core::Result<IAudioClient> =
                    device.Activate(CLSCTX_ALL, None)
                else {
                    continue;
                };
                let Ok(format) = client.GetMixFormat() else {
                    continue;
                };
                let supported = (*format).nChannels == 8 && (*format).nSamplesPerSec == 48000;
                CoTaskMemFree(Some(format.cast()));
                // H5 Game is eight-channel, Chat is stereo. Do not depend on a user-renamable label.
                if !supported {
                    continue;
                }
                let id = device.GetId()?;
                let name = id.to_string().unwrap_or_default();
                CoTaskMemFree(Some(id.0.cast()));
                let Some(guid) = name
                    .strip_prefix("{0.0.0.00000000}.")
                    .filter(|s| valid_guid(s))
                else {
                    continue;
                };
                let registry=RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\MMDevices\\Audio\\Render\\{guid}\\FxProperties"));
                let mut bound = false;
                let mut disabled = false;
                if let Ok(key) = registry {
                    bound = key
                        .get_value::<String, _>("{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},5")
                        .is_ok_and(|v| {
                            v.eq_ignore_ascii_case("{91E0E40B-B337-4FBA-B5D6-A2A6A5ECC93D}")
                        })
                        && key
                            .get_value::<String, _>("{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},7")
                            .is_ok_and(|v| {
                                v.eq_ignore_ascii_case("{C3E23499-5356-4092-A10A-9821EF789E01}")
                            });
                    disabled = key
                        .get_value::<u32, _>("{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5")
                        .unwrap_or(0)
                        != 0;
                }
                let Some(roaming) = std::env::var_os("APPDATA") else {
                    continue;
                };
                found.push(Endpoint {
                    guid: guid.into(),
                    path: PathBuf::from(roaming)
                        .join("Sony/INZONE Hub/APO")
                        .join(format!("{guid}.yaml")),
                    bound,
                    disabled,
                });
            }
            Ok(found)
        }
    };
    let mut found = work().map_err(|e| Error::Io(format!("Sony audio endpoint: {e}")))?;
    if found.len() != 1 {
        return Err(Error::NotSupported("No unique H5 Game endpoint at 48 kHz; connect one H5 receiver and select its 7.1 Game output".into()));
    }
    Ok(found.remove(0))
}
fn valid_guid(s: &str) -> bool {
    s.len() == 38
        && s.starts_with('{')
        && s.ends_with('}')
        && s.as_bytes()[1..37].iter().enumerate().all(|(i, &b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

fn hub_running() -> Result<bool> {
    unsafe {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            System::Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
        };
        let handle = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(handle, &mut entry);
        let mut found = false;
        while ok != 0 {
            let length = entry
                .szExeFile
                .iter()
                .position(|v| *v == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..length])
                .eq_ignore_ascii_case("INZONEHub.exe")
            {
                found = true;
                break;
            }
            ok = Process32NextW(handle, &mut entry);
        }
        CloseHandle(handle);
        Ok(found)
    }
}
fn backup(endpoint: &Endpoint) -> PathBuf {
    crate::data_paths::data_dir()
        .join("sony-audio/backups")
        .join(format!("{}.yaml", endpoint.guid))
}
fn load(endpoint: &Endpoint) -> Result<(String, bool)> {
    match fs::read_to_string(&endpoint.path) {
        Ok(text) => Ok((text, false)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut folders = vec![components::directory().join("files")];
            if let Some(base) = std::env::var_os("PROGRAMDATA") {
                folders.push(PathBuf::from(base).join("Sony/INZONE Hub/APO"));
            }
            for folder in folders {
                if components::verify_file(&folder, "Control.yaml").is_ok() {
                    return Ok((fs::read_to_string(folder.join("Control.yaml"))?, true));
                }
            }
            Err(Error::NotSupported(
                "Prepare Sony audio components to create the Game output configuration".into(),
            ))
        }
        Err(e) => Err(e.into()),
    }
}
pub fn state() -> Result<Value> {
    let endpoint = endpoint()?;
    let running = hub_running()?;
    if !endpoint.bound {
        return Ok(json!({"available":false,"reason":"missing_driver","hub_running":running}));
    }
    let (text, new) = load(&endpoint)?;
    let doc = Document::parse(&text)?;
    let mut state = doc.state()?;
    state["available"] = json!(true);
    state["hub_running"] = json!(running);
    state["effects_disabled"] = json!(endpoint.disabled);
    state["needs_initialization"] = json!(new);
    state["backup"] = json!(backup(&endpoint).is_file());
    state["endpoint"] = json!(endpoint.guid);
    state["resources"] = json!(components::resource_dir().is_ok());
    Ok(state)
}
pub fn change(method: &str, value: &Value) -> Result<()> {
    static WRITES: Mutex<()> = Mutex::new(());
    let _lock = WRITES.lock().unwrap_or_else(|p| p.into_inner());
    if hub_running()? {
        return Err(Error::NotSupported(
            "Exit INZONE Hub from its tray menu before changing Sony audio settings".into(),
        ));
    }
    let endpoint = endpoint()?;
    if !endpoint.bound || endpoint.disabled {
        return Err(Error::NotSupported(
            "Enable the Sony APO and Windows audio enhancements for the H5 Game output first"
                .into(),
        ));
    }
    let (before, new) = load(&endpoint)?;
    let mut doc = Document::parse(&before)?;
    let after = match method {
        "restore_sony_audio" => {
            let original = fs::read_to_string(backup(&endpoint))?;
            Document::parse(&original)?;
            original
        }
        _ => {
            if new {
                doc.spatial(false, &components::resource_dir()?.to_string_lossy())?;
            }
            match method {
                "set_sony_equalizer" => {
                    doc.equalizer(value.as_bool().ok_or_else(|| invalid("Expected boolean"))?)?
                }
                "set_sony_bands" => {
                    let bands: Vec<f64> = serde_json::from_value(value.clone())?;
                    doc.set_bands(&bands)?;
                }
                "set_sony_spatial" => doc.spatial(
                    value.as_bool().ok_or_else(|| invalid("Expected boolean"))?,
                    &components::resource_dir()?.to_string_lossy(),
                )?,
                _ => return Err(invalid("Unknown Sony audio operation")),
            }
            doc.render()
        }
    };
    if !new && after == before {
        return Ok(());
    }
    if !new {
        let path = backup(&endpoint);
        fs::create_dir_all(path.parent().unwrap())?;
        save_original(&path, &before)?;
    }
    // Do not overwrite a concurrent Hub/profile update observed while preparing this edit.
    if (!new && fs::read_to_string(&endpoint.path)? != before)
        || (new && endpoint.path.exists())
        || hub_running()?
    {
        return Err(Error::Io(
            "Sony configuration changed concurrently; refresh and retry".into(),
        ));
    }
    write_atomic(&endpoint.path, after.as_bytes())?;
    if fs::read_to_string(&endpoint.path)? != after {
        return Err(Error::Io(
            "Sony configuration readback did not match".into(),
        ));
    }
    Ok(())
}
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| invalid("Missing configuration directory"))?,
    )?;
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(|e| Error::Io(e.to_string()))?;
    let temp = path.with_extension(format!("{:016x}.partial", u64::from_le_bytes(random)));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        Ok(())
    })();
    let _ = fs::remove_file(temp);
    result
}
fn save_original(path: &Path, text: &str) -> Result<()> {
    if path.exists() {
        Document::parse(&fs::read_to_string(path)?)?;
        return Ok(());
    }
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(|e| Error::Io(e.to_string()))?;
    let temp = path.with_extension(format!("{:016x}.backup", u64::from_le_bytes(random)));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        match fs::hard_link(&temp, path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Document::parse(&fs::read_to_string(path)?)?;
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    })();
    let _ = fs::remove_file(temp);
    result
}
