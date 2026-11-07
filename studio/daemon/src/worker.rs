use std::{
    sync::{Condvar, Mutex},
    time::Duration,
};
#[derive(Default)]
pub struct Cancellation {
    stopped: Mutex<bool>,
    changed: Condvar,
}
impl Cancellation {
    pub fn stop(&self) {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.changed.notify_all();
    }
    pub fn stopped(&self) -> bool {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn wait(&self, duration: Duration) -> bool {
        let state = self.stopped.lock().unwrap_or_else(|e| e.into_inner());
        let (state, _) = self
            .changed
            .wait_timeout_while(state, duration, |state| !*state)
            .unwrap_or_else(|e| e.into_inner());
        *state
    }
}
