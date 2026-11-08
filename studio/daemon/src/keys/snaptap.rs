//! Host-side Snap Tap state machine, ported from openrazer-win #14
//! (9fe3e5dba5899e0eb6e58beda0b4f732a92210af, GPL-2.0-or-later).
//! Physical releases always pass. Only repeats of a suppressed key are swallowed;
//! switching direction adds a release, and resuming happens after the callback.
use crate::keyboard::KeyEvent;
use std::collections::{BTreeMap, BTreeSet};

pub const BYPASS_VKS: [u32; 4] = [0xa4, 0xa5, 0x5b, 0x5c];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyId {
    pub scan: u32,
    pub extended: bool,
}
impl From<KeyEvent> for KeyId {
    fn from(event: KeyEvent) -> Self {
        Self {
            scan: event.scan,
            extended: event.extended,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decision {
    pub swallow: bool,
    /// Release this key before forwarding the physical event.
    pub release: Option<KeyId>,
    /// Schedule take_resumes after the physical release has been forwarded.
    pub later: bool,
}

struct Pair {
    keys: [KeyId; 2],
    held: [bool; 2],
    vks: [u32; 2],
    active: Option<usize>,
    pending: Option<usize>,
}
impl Pair {
    fn clear(&mut self) {
        self.held = [false; 2];
        self.active = None;
        self.pending = None;
    }
}

#[derive(Default)]
pub struct Engine {
    pairs: Vec<Pair>,
    index: BTreeMap<KeyId, (usize, usize)>,
    bypass: BTreeSet<u32>,
    pub paused: bool,
}
impl Engine {
    /// Call only on the hook thread, with pairs already validated by settings.
    pub fn configure(&mut self, pairs: &[[KeyId; 2]]) {
        self.pairs.clear();
        self.index.clear();
        self.bypass.clear();
        for keys in pairs {
            let index = self.pairs.len();
            self.index.insert(keys[0], (index, 0));
            self.index.insert(keys[1], (index, 1));
            self.pairs.push(Pair {
                keys: *keys,
                held: [false; 2],
                vks: [0; 2],
                active: None,
                pending: None,
            });
        }
    }

    pub fn handle(&mut self, event: KeyEvent) -> Decision {
        if event.injected || self.paused {
            return Decision::default();
        }
        if BYPASS_VKS.contains(&event.vk) {
            if event.down {
                if self.bypass.is_empty() {
                    self.forget();
                }
                self.bypass.insert(event.vk);
            } else {
                self.bypass.remove(&event.vk);
            }
            return Decision::default();
        }
        if !self.bypass.is_empty() {
            return Decision::default();
        }
        let Some(&(pair, key)) = self.index.get(&event.into()) else {
            return Decision::default();
        };
        let pair = &mut self.pairs[pair];
        let other = 1 - key;
        if event.down {
            if pair.held[key] {
                if pair.active == Some(key) {
                    return Decision::default();
                }
                if pair.active.is_some() {
                    return Decision {
                        swallow: true,
                        ..Default::default()
                    };
                }
                pair.active = Some(key);
                return Decision::default();
            }
            pair.held[key] = true;
            pair.vks[key] = event.vk;
            let takeover = pair.active == Some(other);
            pair.active = Some(key);
            if takeover {
                pair.pending = None;
                return Decision {
                    release: Some(pair.keys[other]),
                    ..Default::default()
                };
            }
        } else {
            pair.held[key] = false;
            if pair.pending == Some(key) {
                pair.pending = None;
            }
            if pair.active != Some(key) {
                return Decision::default();
            }
            if pair.held[other] {
                pair.active = Some(other);
                pair.pending = Some(other);
                return Decision {
                    later: true,
                    ..Default::default()
                };
            }
            pair.active = None;
        }
        Decision::default()
    }

    pub fn take_resumes(&mut self) -> Vec<KeyId> {
        self.pairs
            .iter_mut()
            .filter_map(|pair| pair.pending.take().map(|key| pair.keys[key]))
            .collect()
    }

    /// A preceding Fn filter swallowed this release; never resume it later.
    pub fn swallowed(&mut self, event: KeyEvent) {
        if event.down || event.injected {
            return;
        }
        self.bypass.remove(&event.vk);
        if let Some(&(pair, key)) = self.index.get(&event.into()) {
            let pair = &mut self.pairs[pair];
            pair.held[key] = false;
            if pair.pending == Some(key) {
                pair.pending = None;
            }
            if pair.active == Some(key) {
                pair.active = None;
            }
        }
    }

    fn forget(&mut self) {
        for pair in &mut self.pairs {
            pair.clear();
        }
    }

    /// A new foreground window/desktop must never inherit a held key.
    pub fn reset(&mut self, is_down: impl Fn(u32) -> bool) {
        self.forget();
        self.bypass = BYPASS_VKS.into_iter().filter(|vk| is_down(*vk)).collect();
    }

    /// Windows may have silently removed an idle hook. Only the active key
    /// confirmed by Windows survives; suppressed keys cannot be recovered.
    pub fn rearm(&mut self, is_down: impl Fn(u32) -> bool) {
        for pair in &mut self.pairs {
            let keep = pair.active.filter(|key| is_down(pair.vks[*key]));
            pair.clear();
            if let Some(key) = keep {
                pair.active = Some(key);
                pair.held[key] = true;
            }
        }
        self.bypass = BYPASS_VKS.into_iter().filter(|vk| is_down(*vk)).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const A: KeyId = KeyId {
        scan: 30,
        extended: false,
    };
    const D: KeyId = KeyId {
        scan: 32,
        extended: false,
    };
    fn event(key: KeyId, down: bool) -> KeyEvent {
        KeyEvent {
            scan: key.scan,
            extended: key.extended,
            vk: if key == A { 65 } else { 68 },
            down,
            injected: false,
            ours: false,
            time: 0,
        }
    }
    fn engine() -> Engine {
        let mut engine = Engine::default();
        engine.configure(&[[A, D]]);
        engine
    }
    #[test]
    fn last_press_wins_and_resume_waits_until_after_the_physical_release() {
        let mut e = engine();
        assert_eq!(e.handle(event(A, true)), Decision::default());
        assert_eq!(e.handle(event(D, true)).release, Some(A));
        assert!(e.handle(event(A, true)).swallow);
        assert_eq!(e.handle(event(D, true)), Decision::default());
        assert!(e.handle(event(D, false)).later);
        assert_eq!(e.take_resumes(), vec![A]);
        assert!(e.take_resumes().is_empty());
        assert_eq!(e.handle(event(A, false)), Decision::default());
    }
    #[test]
    fn released_repressed_reset_and_fn_swallowed_keys_cancel_pending_resumes() {
        for cancel in 0..4 {
            let mut e = engine();
            e.handle(event(A, true));
            e.handle(event(D, true));
            assert!(e.handle(event(D, false)).later);
            match cancel {
                0 => {
                    assert!(!e.handle(event(A, false)).swallow);
                }
                1 => {
                    e.handle(event(D, true));
                }
                2 => e.reset(|_| false),
                _ => e.swallowed(event(A, false)),
            }
            assert!(e.take_resumes().is_empty());
        }
    }
    #[test]
    fn injections_and_extended_keys_do_not_change_a_pair() {
        let mut e = engine();
        e.handle(event(A, true));
        let mut injected = event(D, true);
        injected.injected = true;
        assert_eq!(e.handle(injected), Decision::default());
        assert_eq!(
            e.handle(event(
                KeyId {
                    extended: true,
                    ..D
                },
                true
            )),
            Decision::default()
        );
        assert_eq!(e.handle(event(D, true)).release, Some(A));
    }
    #[test]
    fn alt_and_windows_bypass_but_control_does_not() {
        for vk in BYPASS_VKS.into_iter().chain([0xa2]) {
            let mut e = engine();
            e.handle(event(A, true));
            e.handle(KeyEvent {
                vk,
                scan: 56,
                ..event(A, true)
            });
            let decision = e.handle(event(D, true));
            assert_eq!(decision.release, if vk == 0xa2 { Some(A) } else { None });
        }
    }
    #[test]
    fn rearm_forgets_suppressed_keys_and_does_not_invent_input() {
        let mut e = engine();
        e.handle(event(A, true));
        e.handle(event(D, true));
        e.rearm(|vk| vk == 68);
        assert!(e.take_resumes().is_empty());
        assert_eq!(e.handle(event(D, false)), Decision::default());
        assert!(e.take_resumes().is_empty());
    }
    #[test]
    fn arbitrary_press_release_sequences_never_swallow_a_release() {
        // All 4^7 sequences of A/D press/release, including repeats and missed
        // releases; defer/resume after each callback as the Windows host does.
        for sequence in 0..(1u32 << 14) {
            let mut e = engine();
            let mut value = sequence;
            for _ in 0..7 {
                let code = value & 3;
                value >>= 2;
                let key = if code & 1 == 0 { A } else { D };
                let down = code & 2 == 0;
                let decision = e.handle(event(key, down));
                assert!(down || !decision.swallow);
                let resumes = e.take_resumes();
                assert!(resumes.len() <= 1);
                assert!(e
                    .pairs
                    .iter()
                    .all(|pair| pair.active.is_none_or(|key| pair.held[key])));
            }
        }
    }
}
