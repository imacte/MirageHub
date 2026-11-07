//! One message-loop thread per low-level hook. The callback does no device I/O.
use crate::{Error, Result};
use std::sync::Arc;

pub const INJECTION_MARKER: usize = 0x52415a52;
#[derive(Clone, Copy, Debug)]
pub struct KeyEvent {
    pub vk: u32,
    pub scan: u32,
    pub extended: bool,
    pub down: bool,
    pub injected: bool,
    pub ours: bool,
    pub time: u32,
}
pub type Handler = Arc<dyn Fn(KeyEvent) -> bool + Send + Sync>;

#[cfg(windows)]
pub use native::KeyboardHook;
#[cfg(not(windows))]
pub struct KeyboardHook;
#[cfg(not(windows))]
impl KeyboardHook {
    pub fn start(_: Handler) -> Result<Self> {
        Err(Error::NotSupported("keyboard hooks require Windows".into()))
    }
    pub fn running(&self) -> bool {
        false
    }
    pub fn stop(&mut self) {}
}

pub fn matrix_key(vk: u32, extended: bool) -> Option<String> {
    if vk == 0x0d && extended {
        return Some("ENTER".into());
    }
    if (0x30..=0x39).contains(&vk) || (0x41..=0x5a).contains(&vk) {
        return char::from_u32(vk).map(|c| c.to_string());
    }
    if (0x60..=0x69).contains(&vk) {
        return Some(format!("NP{}", vk - 0x60));
    }
    if (0x70..=0x7b).contains(&vk) {
        return Some(format!("F{}", vk - 0x6f));
    }
    let name = match vk {
        0x1b => "ESC",
        0xc0 => "BACKTICK",
        0x09 => "TAB",
        0x14 => "CAPSLK",
        0xa0 => "LEFTSHIFT",
        0xa1 => "RIGHTSHIFT",
        0xa2 => "LEFTCTRL",
        0xa3 => "RIGHTCTRL",
        0xa4 => "LEFTALT",
        0xa5 => "RIGHTALT",
        0x20 => "SPACE",
        0x0d => "RETURN",
        0x08 => "BACKSPACE",
        0x5b | 0x5c => "SUPER",
        0x5d => "CTXMENU",
        0x2c => "PRTSCR",
        0x91 => "SCRLK",
        0x13 => "PAUSE",
        0x2d => "INS",
        0x24 => "HOME",
        0x21 => "PAGEUP",
        0x2e => "DELETE",
        0x23 => "END",
        0x22 => "PAGEDOWN",
        0x26 => "UPARROW",
        0x28 => "DOWNARROW",
        0x25 => "LEFTARROW",
        0x27 => "RIGHTARROW",
        0x90 => "NUMLK",
        0x6f => "NPFORWARDSLASH",
        0x6a => "NPASTERISK",
        0x6d => "NPDASH",
        0x6b => "NPPLUS",
        0x6e => "NPPERIOD",
        0xbd => "DASH",
        0xbb => "EQUALS",
        0xdb => "LEFTSQUAREBRACKET",
        0xdd => "RIGHTSQUAREBRACKET",
        0xdc => "BACKSLASH",
        0xba => "SEMICOLON",
        0xde => "APOSTROPHE",
        0xbc => "COMMA",
        0xbe => "PERIOD",
        0xbf => "FORWARDSLASH",
        _ => return None,
    };
    Some(name.into())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn native_observer_hook_installs_and_removes_without_synthesizing_input() {
        for _ in 0..2 {
            let mut hook = KeyboardHook::start(Arc::new(|_| false))
                .expect("WH_KEYBOARD_LL should install on Windows");
            assert!(hook.running());
            hook.stop();
            assert!(!hook.running());
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        cell::RefCell,
        ptr,
        sync::{
            atomic::{AtomicBool, AtomicU32, Ordering},
            mpsc,
        },
        thread::{self, JoinHandle},
        time::Duration,
    };
    use windows_sys::Win32::{
        Foundation::{LPARAM, LRESULT, WPARAM},
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::WindowsAndMessaging::*,
    };
    thread_local! {static CALLBACK:RefCell<Option<Handler>>=RefCell::new(None);}

    pub struct KeyboardHook {
        worker: Option<JoinHandle<()>>,
        id: Arc<AtomicU32>,
        running: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
    }
    impl KeyboardHook {
        pub fn start(handler: Handler) -> Result<Self> {
            let id = Arc::new(AtomicU32::new(0));
            let running = Arc::new(AtomicBool::new(false));
            let stop = Arc::new(AtomicBool::new(false));
            let (thread_id, is_running, stopping) = (id.clone(), running.clone(), stop.clone());
            let (ready, receive) = mpsc::sync_channel(1);
            let worker = thread::Builder::new()
                .name("openrazer-keyboard".into())
                .spawn(move || {
                    unsafe {
                        let mut message: MSG = std::mem::zeroed();
                        // Create the queue before publishing the thread id for WM_QUIT.
                        PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_NOREMOVE);
                        thread_id.store(GetCurrentThreadId(), Ordering::Release);
                        CALLBACK.with(|slot| *slot.borrow_mut() = Some(handler));
                        let hook = SetWindowsHookExW(
                            WH_KEYBOARD_LL,
                            Some(procedure),
                            GetModuleHandleW(ptr::null()),
                            0,
                        );
                        if hook.is_null() {
                            thread_id.store(0, Ordering::Release);
                            CALLBACK.with(|slot| slot.borrow_mut().take());
                            let _ = ready
                                .send(Err(Error::Io(std::io::Error::last_os_error().to_string())));
                            return;
                        }
                        is_running.store(true, Ordering::Release);
                        let _ = ready.send(Ok(()));
                        while !stopping.load(Ordering::Acquire)
                            && GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0
                        {
                            TranslateMessage(&message);
                            DispatchMessageW(&message);
                        }
                        UnhookWindowsHookEx(hook);
                        CALLBACK.with(|slot| slot.borrow_mut().take());
                        thread_id.store(0, Ordering::Release);
                        is_running.store(false, Ordering::Release);
                    }
                })?;
            let mut hook = Self {
                worker: Some(worker),
                id,
                running,
                stop,
            };
            match receive.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(())) => Ok(hook),
                result => {
                    hook.stop();
                    Err(result
                        .ok()
                        .and_then(std::result::Result::err)
                        .unwrap_or_else(|| Error::Io("keyboard hook did not start".into())))
                }
            }
        }
        pub fn running(&self) -> bool {
            self.running.load(Ordering::Acquire)
        }
        pub fn stop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let id = self.id.load(Ordering::Acquire);
                if id != 0 && !worker.is_finished() {
                    unsafe {
                        PostThreadMessageW(id, WM_QUIT, 0, 0);
                    }
                }
                let _ = worker.join();
                self.id.store(0, Ordering::Release);
            }
        }
    }
    impl Drop for KeyboardHook {
        fn drop(&mut self) {
            self.stop();
        }
    }
    unsafe extern "system" fn procedure(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
        if code >= 0
            && !(data as *const KBDLLHOOKSTRUCT).is_null()
            && matches!(
                message as u32,
                WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP
            )
        {
            let event = unsafe { &*(data as *const KBDLLHOOKSTRUCT) };
            let input = KeyEvent {
                vk: event.vkCode,
                scan: event.scanCode,
                extended: event.flags & LLKHF_EXTENDED != 0,
                down: matches!(message as u32, WM_KEYDOWN | WM_SYSKEYDOWN),
                injected: event.flags & LLKHF_INJECTED != 0,
                ours: event.dwExtraInfo == INJECTION_MARKER,
                time: event.time,
            };
            let suppress = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                CALLBACK.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .is_some_and(|callback| callback(input))
                })
            }))
            .unwrap_or(false);
            if suppress {
                return 1;
            }
        }
        unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) }
    }
}
