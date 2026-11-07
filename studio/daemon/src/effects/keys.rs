//! Bounded queues decouple the OS hook from rendering and HID transfers.
use crate::keyboard::{matrix_key, KeyboardHook};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex, Weak,
    },
    time::Instant,
};

#[derive(Clone)]
pub struct KeyPress {
    pub name: String,
    pub at: Instant,
}
pub struct KeySource {
    subscribers: Arc<Mutex<BTreeMap<u64, mpsc::SyncSender<KeyPress>>>>,
    lifecycle: Mutex<Option<KeyboardHook>>,
    next: AtomicU64,
    native: bool,
}
impl KeySource {
    pub fn new(native: bool) -> Arc<Self> {
        Arc::new(Self {
            subscribers: Arc::new(Mutex::new(BTreeMap::new())),
            lifecycle: Mutex::new(None),
            next: AtomicU64::new(1),
            native,
        })
    }
    pub fn subscribe(self: &Arc<Self>) -> Subscription {
        let mut hook = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (send, receive) = mpsc::sync_channel(256);
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, send);
        if hook.is_none() && self.native {
            let subscribers = self.subscribers.clone();
            *hook = KeyboardHook::start(Arc::new(move |event| {
                if event.down {
                    if let Some(name) = matrix_key(event.vk, event.extended) {
                        broadcast(
                            &subscribers,
                            KeyPress {
                                name,
                                at: Instant::now(),
                            },
                        );
                    }
                }
                false // Effects only observe; the original input always passes through.
            }))
            .ok();
        }
        Subscription {
            id,
            receive,
            source: Arc::downgrade(self),
        }
    }
    fn unsubscribe(&self, id: u64) {
        let mut hook = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        let idle = {
            let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
            subscribers.remove(&id);
            subscribers.is_empty()
        };
        if idle {
            hook.take();
        }
    }
    pub fn running(&self) -> bool {
        if self.native {
            self.lifecycle
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .is_some_and(KeyboardHook::running)
        } else {
            !self
                .subscribers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        }
    }
    pub fn press(&self, name: &str) {
        broadcast(
            &self.subscribers,
            KeyPress {
                name: name.into(),
                at: Instant::now(),
            },
        );
    }
    pub fn stop(&self) {
        let mut hook = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        hook.take();
    }
}
fn broadcast(subscribers: &Mutex<BTreeMap<u64, mpsc::SyncSender<KeyPress>>>, press: KeyPress) {
    let subscribers = subscribers.lock().unwrap_or_else(|e| e.into_inner());
    for subscriber in subscribers.values() {
        let _ = subscriber.try_send(press.clone());
    }
}
pub struct Subscription {
    id: u64,
    pub receive: mpsc::Receiver<KeyPress>,
    source: Weak<KeySource>,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(source) = self.source.upgrade() {
            source.unsubscribe(self.id);
        }
    }
}
