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
    pub fn with_snaptap(
        _: Handler,
        _: Option<Arc<crate::keys::snaptap_host::Controller>>,
        _: Arc<dyn crate::keys::inject::Injector>,
    ) -> Result<Self> {
        Err(Error::NotSupported("keyboard hooks require Windows".into()))
    }
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

    #[test]
    fn disabled_snaptap_reconfigures_on_the_hook_thread_without_injecting_input() {
        use crate::keys::{
            inject::{Injection, Injector},
            snaptap_host::Controller,
            snaptap_settings::SettingsStore,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct CountingInjector(AtomicUsize);
        impl Injector for CountingInjector {
            fn emit(&self, _: &[Injection]) -> Result<()> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
        let controller = Controller::new(Arc::new(SettingsStore::memory()));
        let injector = Arc::new(CountingInjector(AtomicUsize::new(0)));
        let mut hook = KeyboardHook::with_snaptap(
            Arc::new(|_| false),
            Some(controller.clone()),
            injector.clone(),
        )
        .unwrap();
        assert!(hook.running() && controller.attached());
        controller
            .update(
                serde_json::json!({"pairs":[["w","s"]],"deny":["cs2.exe","test.exe"]})
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            controller.status()["pairs"],
            serde_json::json!([["KEY_W", "KEY_S"]])
        );
        assert_eq!(controller.status()["running"], false);
        hook.stop();
        assert!(!controller.attached());
        assert_eq!(injector.0.load(Ordering::SeqCst), 0);
    }
}

#[cfg(windows)]
#[path = "keyboard_windows.rs"]
mod native;
