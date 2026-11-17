use super::*;
use std::{
    cell::RefCell,
    ptr,
    sync::mpsc,
    thread::{self, JoinHandle},
};
use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::*,
};

thread_local! {
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

unsafe extern "system" fn procedure(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code >= 0 && data != 0 {
        let raw = unsafe { &*(data as *const MSLLHOOKSTRUCT) };
        let button = match message as u32 {
            WM_MBUTTONDOWN | WM_MBUTTONUP => Some(MouseButton::Middle),
            WM_XBUTTONDOWN | WM_XBUTTONUP => match raw.mouseData >> 16 {
                1 => Some(MouseButton::Back),
                2 => Some(MouseButton::Forward),
                _ => None,
            },
            _ => None,
        };
        if let Some(button) = button {
            let event = MouseEvent {
                button,
                down: matches!(message as u32, WM_MBUTTONDOWN | WM_XBUTTONDOWN),
                injected: raw.flags & LLMHF_INJECTED != 0,
                ours: raw.dwExtraInfo == crate::keyboard::INJECTION_MARKER,
            };
            let handler = HANDLER.with(|slot| slot.borrow().clone());
            if handler.is_some_and(|handler| handler(event)) {
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) }
}

pub struct MouseHook {
    thread_id: u32,
    worker: Option<JoinHandle<()>>,
}
impl MouseHook {
    pub fn start(handler: Handler) -> Result<Self> {
        let (ready, receive) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("mirage-mouse-hook".into())
            .spawn(move || unsafe {
                let mut message: MSG = std::mem::zeroed();
                PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_NOREMOVE);
                HANDLER.with(|slot| *slot.borrow_mut() = Some(handler));
                let hook = SetWindowsHookExW(
                    WH_MOUSE_LL,
                    Some(procedure),
                    GetModuleHandleW(ptr::null()),
                    0,
                );
                if hook.is_null() {
                    let _ = ready.send(Err(crate::Error::from(std::io::Error::last_os_error())));
                } else {
                    let _ = ready.send(Ok(GetCurrentThreadId()));
                    while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                    UnhookWindowsHookEx(hook);
                }
                HANDLER.with(|slot| slot.borrow_mut().take());
            })?;
        match receive.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                worker: Some(worker),
            }),
            result => {
                let _ = worker.join();
                Err(result
                    .ok()
                    .and_then(Result::err)
                    .unwrap_or_else(|| crate::Error::Io("mouse hook failed to start".into())))
            }
        }
    }
    pub fn running(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }
}
impl Drop for MouseHook {
    fn drop(&mut self) {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
