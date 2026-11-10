//! A user-initiated, four-second microphone check. Audio stays in memory and is
//! discarded after playback or cancellation. The window owns the lifetime.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[cfg(windows)]
mod native;

#[derive(Clone, Default)]
pub enum Phase {
    #[default]
    Starting,
    Recording {
        progress: f32,
        peak: f32,
    },
    Playing {
        progress: f32,
    },
    Finished,
    Stopped,
    Failed(String),
}
impl Phase {
    pub fn active(&self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Recording { .. } | Self::Playing { .. }
        )
    }
}

pub struct Test {
    pub serial: String,
    state: Arc<Mutex<Phase>>,
    stop: Arc<AtomicBool>,
}
impl Test {
    pub fn start(serial: String, input: String, output: String) -> Self {
        let state = Arc::new(Mutex::new(Phase::Starting));
        let stop = Arc::new(AtomicBool::new(false));
        let test = Self {
            serial,
            state: state.clone(),
            stop: stop.clone(),
        };
        let failed = state.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("microphone-test".into())
            .spawn(move || {
                #[cfg(windows)]
                let result = native::run(&input, &output, &stop, &state);
                #[cfg(not(windows))]
                let result: Result<(), String> = {
                    let _ = (input, output);
                    Err("Microphone test requires Windows".into())
                };
                *state.lock().unwrap_or_else(|p| p.into_inner()) = if stop.load(Ordering::Relaxed) {
                    Phase::Stopped
                } else {
                    match result {
                        Ok(()) => Phase::Finished,
                        Err(error) => Phase::Failed(error),
                    }
                };
            })
        {
            *failed.lock().unwrap_or_else(|p| p.into_inner()) = Phase::Failed(error.to_string());
        }
        test
    }
    pub fn phase(&self) -> Phase {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Decode interleaved PCM to a bounded mono value. Non-finite samples are silence.
#[cfg(any(windows, test))]
fn sample(bytes: &[u8], float: bool) -> f32 {
    let value = match (float, bytes.len()) {
        (true, 4) => f32::from_le_bytes(bytes.try_into().unwrap()),
        (false, 2) => i16::from_le_bytes(bytes.try_into().unwrap()) as f32 / 32768.,
        (false, 3) => {
            (i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8) as f32 / 8388608.
        }
        (false, 4) => i32::from_le_bytes(bytes.try_into().unwrap()) as f32 / 2147483648.,
        _ => 0.,
    };
    if value.is_finite() {
        value.clamp(-1., 1.)
    } else {
        0.
    }
}

#[cfg(any(windows, test))]
fn encode(value: f32, bytes: &mut [u8], float: bool) {
    let value = if value.is_finite() {
        value.clamp(-1., 1.)
    } else {
        0.
    };
    match (float, bytes.len()) {
        (true, 4) => bytes.copy_from_slice(&value.to_le_bytes()),
        (false, 2) => {
            bytes.copy_from_slice(&((value * 32768.).clamp(-32768., 32767.) as i16).to_le_bytes())
        }
        (false, 3) => bytes.copy_from_slice(
            &((value * 8388608.).clamp(-8388608., 8388607.) as i32).to_le_bytes()[..3],
        ),
        (false, 4) => bytes.copy_from_slice(
            &((value as f64 * 2147483648.).clamp(-2147483648., 2147483647.) as i32).to_le_bytes(),
        ),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pcm_roundtrip_and_nonfinite_input_are_bounded() {
        for (float, size) in [(true, 4), (false, 2), (false, 3), (false, 4)] {
            for value in [-1., -0.5, 0., 0.5, 1.] {
                let mut bytes = vec![0; size];
                encode(value, &mut bytes, float);
                assert!((sample(&bytes, float) - value).abs() < 0.00004);
            }
        }
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(sample(&value.to_le_bytes(), true), 0.);
        }
    }
}
