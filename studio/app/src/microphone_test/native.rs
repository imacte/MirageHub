use super::{encode, sample, Phase};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
use windows::{
    core::{GUID, PCWSTR},
    Win32::{Media::Audio::*, System::Com::*},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const SECONDS: usize = 4;

struct Com;
impl Drop for Com {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
struct Format(*mut WAVEFORMATEX);
impl Drop for Format {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.cast())) }
    }
}
struct Stream {
    client: IAudioClient,
    channels: usize,
    bytes: usize,
    rate: usize,
    float: bool,
}
impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            let _ = self.client.Reset();
        }
    }
}

impl Stream {
    unsafe fn open(id: &str, capture: bool) -> Result<Self> {
        let prefix = if capture {
            "{0.0.1.00000000}."
        } else {
            "{0.0.0.00000000}."
        };
        if !id.starts_with(prefix) || id.contains('\0') {
            return Err("Invalid H5 endpoint".into());
        }
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let wide: Vec<u16> = id.encode_utf16().chain([0]).collect();
        let device = enumerator.GetDevice(PCWSTR(wide.as_ptr()))?;
        // Check topology again: never silently fall back to the system's default microphone.
        let topology: IDeviceTopology = device.Activate(CLSCTX_ALL, None)?;
        let connected = topology.GetConnector(0)?.GetDeviceIdConnectedTo()?;
        let name = connected
            .to_string()
            .unwrap_or_default()
            .to_ascii_lowercase();
        CoTaskMemFree(Some(connected.0.cast()));
        if !name.contains("vid_054c&pid_0ebf") {
            return Err("The selected endpoint is no longer the H5".into());
        }
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = Format(client.GetMixFormat()?);
        let base = *format.0;
        let channels = base.nChannels as usize;
        let rate = base.nSamplesPerSec as usize;
        let bytes = base.wBitsPerSample as usize / 8;
        let subtype = if base.wFormatTag == 0xfffe && base.cbSize >= 22 {
            let extended = format.0.cast::<WAVEFORMATEXTENSIBLE>().read_unaligned();
            extended.SubFormat
        } else {
            GUID::from_u128((base.wFormatTag as u128) << 96 | 0x00000010800000aa00389b71)
        };
        let float = subtype == GUID::from_u128(0x0000000300000010800000aa00389b71);
        let pcm = subtype == GUID::from_u128(0x0000000100000010800000aa00389b71);
        if !(8000..=192000).contains(&rate)
            || !(1..=8).contains(&channels)
            || (capture && channels > 2)
            || base.nBlockAlign as usize != channels * bytes
            || !((float && bytes == 4) || (pcm && [2, 3, 4].contains(&bytes)))
        {
            return Err("Unsupported microphone test audio format".into());
        }
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, format.0, None)?;
        Ok(Self {
            client,
            channels,
            bytes,
            rate,
            float,
        })
    }
    fn frame_bytes(&self) -> usize {
        self.channels * self.bytes
    }
}

pub fn run(
    input: &str,
    output: &str,
    stop: &AtomicBool,
    state: &Mutex<Phase>,
) -> std::result::Result<(), String> {
    unsafe { work(input, output, stop, state) }.map_err(|error| error.to_string())
}

unsafe fn work(input: &str, output: &str, stop: &AtomicBool, state: &Mutex<Phase>) -> Result<()> {
    CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    let _com = Com;
    let capture = Stream::open(input, true)?;
    let render = Stream::open(output, false)?;
    let reader: IAudioCaptureClient = capture.client.GetService()?;
    let writer: IAudioRenderClient = render.client.GetService()?;
    let frames = capture.rate * SECONDS;
    let mut recorded = Vec::with_capacity(frames);
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    capture.client.Start()?;
    let started = Instant::now();
    while recorded.len() < frames && !stop.load(Ordering::Relaxed) {
        if started.elapsed() > Duration::from_secs(SECONDS as u64 + 3) {
            return Err("Microphone did not deliver audio in time".into());
        }
        let mut peak = 0f32;
        while reader.GetNextPacketSize()? > 0
            && recorded.len() < frames
            && !stop.load(Ordering::Relaxed)
        {
            let mut data = std::ptr::null_mut();
            let mut count = 0;
            let mut flags = 0;
            reader.GetBuffer(&mut data, &mut count, &mut flags, None, None)?;
            let take = (count as usize).min(frames - recorded.len());
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                recorded.resize(recorded.len() + take, 0.);
            } else if !data.is_null() {
                let bytes = std::slice::from_raw_parts(data, take * capture.frame_bytes());
                for frame in bytes.chunks_exact(capture.frame_bytes()) {
                    let mono = frame
                        .chunks_exact(capture.bytes)
                        .map(|s| sample(s, capture.float))
                        .sum::<f32>()
                        / capture.channels as f32;
                    peak = peak.max(mono.abs());
                    recorded.push(mono);
                }
            }
            reader.ReleaseBuffer(count)?;
        }
        *state.lock().unwrap_or_else(|p| p.into_inner()) = Phase::Recording {
            progress: recorded.len() as f32 / frames as f32,
            peak,
        };
        std::thread::sleep(Duration::from_millis(10));
    }
    capture.client.Stop()?;
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    // PCM is heard once through this headset, with a short fade and no gain boost.
    let total = recorded.len() * render.rate / capture.rate;
    let capacity = render.client.GetBufferSize()?;
    let mut offset = 0;
    let mut playing = false;
    let started = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if started.elapsed() > Duration::from_secs(SECONDS as u64 + 4) {
            return Err("Headphone playback timed out".into());
        }
        let padding = render.client.GetCurrentPadding()?;
        if offset == total && padding == 0 {
            break;
        }
        let count = (capacity.saturating_sub(padding) as usize).min(total - offset);
        if count > 0 {
            let data = writer.GetBuffer(count as u32)?;
            let bytes = std::slice::from_raw_parts_mut(data, count * render.frame_bytes());
            bytes.fill(0);
            for (index, frame) in bytes.chunks_exact_mut(render.frame_bytes()).enumerate() {
                let dest = offset + index;
                let source = dest as f64 * capture.rate as f64 / render.rate as f64;
                let a = source as usize;
                let b = (a + 1).min(recorded.len() - 1);
                let value = recorded[a] + (recorded[b] - recorded[a]) * source.fract() as f32;
                let fade = (dest.min(total - dest) as f32 / (render.rate as f32 * 0.01)).min(1.);
                for channel in frame.chunks_exact_mut(render.bytes).take(2) {
                    encode(value * fade, channel, render.float);
                }
            }
            writer.ReleaseBuffer(count as u32, 0)?;
            offset += count;
            if !playing {
                render.client.Start()?;
                playing = true;
            }
        }
        *state.lock().unwrap_or_else(|p| p.into_inner()) = Phase::Playing {
            progress: offset.saturating_sub(padding as usize) as f32 / total.max(1) as f32,
        };
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
