//! Output peak metering, not an audio recording or a capture stream.
pub trait AudioMeter {
    fn peak(&mut self) -> f64;
}
pub struct SilentMeter;
impl AudioMeter for SilentMeter {
    fn peak(&mut self) -> f64 {
        0.0
    }
}
pub fn open_meter() -> Box<dyn AudioMeter> {
    #[cfg(windows)]
    if let Ok(meter) = native::WindowsMeter::new() {
        return Box::new(meter);
    }
    Box::new(SilentMeter)
}

#[cfg(windows)]
mod native {
    use super::AudioMeter;
    use std::{
        marker::PhantomData,
        rc::Rc,
        time::{Duration, Instant},
    };
    use windows::Win32::{
        Media::Audio::{
            eMultimedia, eRender, Endpoints::IAudioMeterInformation, IMMDeviceEnumerator,
            MMDeviceEnumerator,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
        },
    };
    pub struct WindowsMeter {
        meter: Option<IAudioMeterInformation>,
        opened_at: Option<Instant>,
        _thread: PhantomData<Rc<()>>,
    }
    impl WindowsMeter {
        pub fn new() -> windows::core::Result<Self> {
            unsafe {
                CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            }
            Ok(Self {
                meter: None,
                opened_at: None,
                _thread: PhantomData,
            })
        }
        fn open() -> windows::core::Result<IAudioMeterInformation> {
            unsafe {
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                enumerator
                    .GetDefaultAudioEndpoint(eRender, eMultimedia)?
                    .Activate(CLSCTX_ALL, None)
            }
        }
    }
    impl AudioMeter for WindowsMeter {
        fn peak(&mut self) -> f64 {
            if self
                .opened_at
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(3))
            {
                self.meter.take();
                self.meter = Self::open().ok();
                self.opened_at = Some(Instant::now());
            }
            match self
                .meter
                .as_ref()
                .map(|meter| unsafe { meter.GetPeakValue() })
            {
                Some(Ok(value)) if value.is_finite() => value.clamp(0.0, 1.0) as f64,
                Some(Err(_)) => {
                    self.meter.take();
                    0.0
                }
                _ => 0.0,
            }
        }
    }
    impl Drop for WindowsMeter {
        fn drop(&mut self) {
            self.meter.take();
            unsafe {
                CoUninitialize();
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn native_output_meter_initializes_com_and_returns_a_bounded_level() {
            let mut meter = WindowsMeter::new().unwrap();
            for _ in 0..2 {
                assert!((0.0..=1.0).contains(&meter.peak()));
            }
        }
    }
}
