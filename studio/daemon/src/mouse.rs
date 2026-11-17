//! Global software mouse bindings. Injected events never trigger bindings.
use crate::{macro_input::MouseButton, Result};
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct MouseEvent {
    pub button: MouseButton,
    pub down: bool,
    pub injected: bool,
    pub ours: bool,
}
pub type Handler = Arc<dyn Fn(MouseEvent) -> bool + Send + Sync>;

#[cfg(windows)]
#[path = "mouse_windows.rs"]
mod native;
#[cfg(windows)]
pub use native::MouseHook;

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn native_mouse_observer_starts_and_stops_without_injecting_input() {
        let hook = MouseHook::start(Arc::new(|_| false)).unwrap();
        assert!(hook.running());
        drop(hook);
    }
}

#[cfg(not(windows))]
pub struct MouseHook;
#[cfg(not(windows))]
impl MouseHook {
    pub fn start(_: Handler) -> Result<Self> {
        Err(crate::Error::NotSupported(
            "mouse bindings require Windows".into(),
        ))
    }
    pub fn running(&self) -> bool {
        false
    }
}
