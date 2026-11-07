//! Input encoding is pure; only NativeInjector calls SendInput.
use super::{
    codes::Output,
    macros::{Action, MacroEvent},
};
use crate::{worker::Cancellation, Error, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, time::Duration};

pub const EXTENDED: u32 = 1;
pub const KEY_UP: u32 = 2;
pub const UNICODE: u32 = 4;
pub const SCAN_CODE: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Injection {
    pub vk: u16,
    pub scan: u16,
    pub flags: u32,
}

pub fn key_input(output: Output, down: bool) -> Injection {
    let mut flags = if down { 0 } else { KEY_UP };
    match output {
        Output::Physical(key) => {
            if key.extended {
                flags |= EXTENDED;
            }
            if key.vk == 0x13 || key.scan >= 0x64 {
                Injection {
                    vk: key.vk,
                    scan: key.scan,
                    flags,
                }
            } else {
                Injection {
                    vk: 0,
                    scan: key.scan,
                    flags: flags | SCAN_CODE,
                }
            }
        }
        Output::Virtual(vk) => {
            if matches!(vk, 0x21..=0x28 | 0x2d | 0x2e | 0x5b..=0x5d | 0x6f | 0x90 | 0xa3 | 0xa5 | 0xad..=0xb3 | 0xb7)
            {
                flags |= EXTENDED;
            }
            Injection { vk, scan: 0, flags }
        }
    }
}
pub fn raw_input(event: &MacroEvent) -> Injection {
    Injection {
        vk: if event.scan != 0 { 0 } else { event.vk },
        scan: event.scan,
        flags: if event.down { 0 } else { KEY_UP }
            | if event.ext { EXTENDED } else { 0 }
            | if event.scan != 0 { SCAN_CODE } else { 0 },
    }
}
pub fn text_inputs(text: &str) -> Vec<Injection> {
    text.encode_utf16()
        .flat_map(|scan| {
            [
                Injection {
                    vk: 0,
                    scan,
                    flags: UNICODE,
                },
                Injection {
                    vk: 0,
                    scan,
                    flags: UNICODE | KEY_UP,
                },
            ]
        })
        .collect()
}

pub trait Injector: Send + Sync {
    fn emit(&self, events: &[Injection]) -> Result<()>;
    fn key(&self, output: Output, down: bool) -> Result<()> {
        self.emit(&[key_input(output, down)])
    }
    fn raw(&self, event: &MacroEvent) -> Result<()> {
        self.emit(&[raw_input(event)])
    }
    fn text(&self, text: &str) -> Result<()> {
        self.emit(&text_inputs(text))
    }
}
pub trait Launcher: Send + Sync {
    fn launch(&self, target: &str) -> Result<()>;
}
pub struct NativeInjector;
pub struct NativeLauncher;

struct HeldKeys<'a> {
    injector: &'a dyn Injector,
    held: BTreeSet<(u16, u16, bool)>,
}
impl Drop for HeldKeys<'_> {
    fn drop(&mut self) {
        for &(vk, scan, ext) in &self.held {
            let _ = self.injector.raw(&MacroEvent {
                vk,
                scan,
                ext,
                down: false,
                delay: 0,
            });
        }
    }
}
pub(crate) fn play(
    action: &Action,
    injector: &dyn Injector,
    launcher: &dyn Launcher,
    cancel: &Cancellation,
) -> Result<()> {
    if cancel.stopped() {
        return Ok(());
    }
    match action {
        Action::Keys { events } => {
            let mut held = HeldKeys {
                injector,
                held: BTreeSet::new(),
            };
            for event in events {
                if cancel.wait(Duration::from_millis(u64::from(event.delay))) {
                    break;
                }
                let identity = (event.vk, event.scan, event.ext);
                // Also release a down event whose delivery only partially succeeded.
                if event.down {
                    held.held.insert(identity);
                }
                injector.raw(event)?;
                if !event.down {
                    held.held.remove(&identity);
                }
            }
        }
        Action::Text { text } => injector.text(text)?,
        Action::Launch { target } => launcher.launch(target)?,
        Action::Disabled => (),
    }
    Ok(())
}

#[cfg(not(windows))]
impl Injector for NativeInjector {
    fn emit(&self, _: &[Injection]) -> Result<()> {
        Err(Error::NotSupported(
            "input injection requires Windows".into(),
        ))
    }
}
#[cfg(not(windows))]
impl Launcher for NativeLauncher {
    fn launch(&self, _: &str) -> Result<()> {
        Err(Error::NotSupported(
            "launch bindings require Windows".into(),
        ))
    }
}

#[cfg(windows)]
impl Injector for NativeInjector {
    fn emit(&self, events: &[Injection]) -> Result<()> {
        use crate::keyboard::INJECTION_MARKER;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
        };
        if events.is_empty() {
            return Ok(());
        }
        let native: Vec<_> = events
            .iter()
            .map(|event| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: event.vk,
                        wScan: event.scan,
                        dwFlags: event.flags,
                        time: 0,
                        dwExtraInfo: INJECTION_MARKER,
                    },
                },
            })
            .collect();
        let count = u32::try_from(native.len())
            .map_err(|_| Error::Invalid("too many input events".into()))?;
        let sent =
            unsafe { SendInput(count, native.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
        if sent != count {
            let error = std::io::Error::last_os_error();
            let mut held = BTreeSet::new();
            for event in events.iter().take(sent as usize) {
                let identity = (event.vk, event.scan, event.flags & !KEY_UP);
                if event.flags & KEY_UP == 0 {
                    held.insert(identity);
                } else {
                    held.remove(&identity);
                }
            }
            // A partial Unicode batch may have stopped between a down/up pair.
            let releases: Vec<_> = held
                .into_iter()
                .map(|(vk, scan, flags)| INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: vk,
                            wScan: scan,
                            dwFlags: flags | KEY_UP,
                            time: 0,
                            dwExtraInfo: INJECTION_MARKER,
                        },
                    },
                })
                .collect();
            if !releases.is_empty() {
                unsafe {
                    SendInput(
                        releases.len() as u32,
                        releases.as_ptr(),
                        std::mem::size_of::<INPUT>() as i32,
                    );
                }
            }
            return Err(Error::Io(format!(
                "SendInput sent {sent} of {count} events: {error}"
            )));
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Launcher for NativeLauncher {
    fn launch(&self, target: &str) -> Result<()> {
        use std::{path::Path, ptr};
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::{
                Com::{
                    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
                    COINIT_DISABLE_OLE1DDE,
                },
                Threading::{
                    CreateProcessW, CREATE_NEW_CONSOLE, PROCESS_INFORMATION, STARTUPINFOW,
                },
            },
            UI::{
                Shell::{
                    ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
                },
                WindowsAndMessaging::SW_SHOWNORMAL,
            },
        };
        if target.contains('\0') {
            return Err(Error::Invalid(
                "launch target contains a null character".into(),
            ));
        }
        let mut command: Vec<_> = target.encode_utf16().chain(Some(0)).collect();
        if target.contains("://") || Path::new(target).exists() {
            unsafe {
                let initialized = CoInitializeEx(
                    ptr::null(),
                    (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
                );
                if initialized < 0 {
                    return Err(Error::Io(format!(
                        "cannot initialize shell COM: {initialized:#x}"
                    )));
                }
                let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
                info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
                // This playback thread has no message pump and exits after
                // launching. Complete any DDE hand-off before uninitializing.
                info.fMask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
                info.lpFile = command.as_ptr();
                info.nShow = SW_SHOWNORMAL;
                let launched = ShellExecuteExW(&mut info);
                let error = std::io::Error::last_os_error();
                CoUninitialize();
                if launched == 0 {
                    return Err(error.into());
                }
            }
            return Ok(());
        }
        unsafe {
            let mut startup: STARTUPINFOW = std::mem::zeroed();
            startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            let mut process: PROCESS_INFORMATION = std::mem::zeroed();
            // The user supplies a Windows command line; no command shell is involved.
            if CreateProcessW(
                ptr::null(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                CREATE_NEW_CONSOLE,
                ptr::null(),
                ptr::null(),
                &startup,
                &mut process,
            ) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            CloseHandle(process.hThread);
            CloseHandle(process.hProcess);
        }
        Ok(())
    }
}
