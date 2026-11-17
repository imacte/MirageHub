use super::{desktop::Desktop, sample_rgba, Colour, WallpaperReader};
use std::{
    error::Error,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, Instant},
};
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};
use windows_sys::Win32::{
    Foundation::{BOOL, HWND, LPARAM},
    UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetAncestor, GetClassNameW,
        IsWindowVisible, GA_ROOT,
    },
};

type CaptureError = Box<dyn Error + Send + Sync>;
type Capture = CaptureControl<Frames, CaptureError>;
const RETRY: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_secs(1);
const FRAME_INTERVAL: Duration = Duration::from_millis(250);

struct Frames {
    latest: Arc<Mutex<Option<Colour>>>,
    sampled_at: Option<Instant>,
}
impl GraphicsCaptureApiHandler for Frames {
    type Flags = Arc<Mutex<Option<Colour>>>;
    type Error = CaptureError;

    fn new(context: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { latest: context.flags, sampled_at: None })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if self.sampled_at.is_some_and(|at| at.elapsed() < FRAME_INTERVAL) {
            return Ok(());
        }
        self.sampled_at = Some(Instant::now());
        let mut buffer = frame.buffer()?;
        let (width, height, pitch) = (
            buffer.width() as usize,
            buffer.height() as usize,
            buffer.row_pitch() as usize,
        );
        if let Some(next) = sample_rgba(buffer.as_raw_buffer(), width, height, pitch) {
            *self.latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(next);
        }
        Ok(())
    }
}

#[derive(Default)]
struct State {
    target: Option<WallpaperWindow>,
    capture: Option<Capture>,
    attempted_at: Option<Instant>,
    use_host: bool,
    desktop: Desktop,
    polled_at: Option<Instant>,
    colour: Option<Colour>,
}
impl State {
    fn stop_capture(&mut self) {
        if let Some(capture) = self.capture.take() {
            let _ = capture.stop();
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.stop_capture();
    }
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    latest: Arc<Mutex<Option<Colour>>>,
}
impl Shared {
    fn sample(&self) -> Option<Colour> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.polled_at.is_some_and(|at| at.elapsed() < POLL) {
            return state.colour;
        }
        state.polled_at = Some(Instant::now());
        let target = wallpaper_window();
        if state.target != target {
            state.stop_capture();
            *self.latest.lock().unwrap_or_else(|e| e.into_inner()) = None;
            state.target = target;
            state.attempted_at = None;
            state.use_host = false;
        }
        let live = target.and_then(|target| self.sample_live(&mut state, target));
        let colour = live.or_else(|| state.desktop.sample());
        state.colour = colour;
        colour
    }

    fn sample_live(&self, state: &mut State, target: WallpaperWindow) -> Option<Colour> {
        let latest = *self.latest.lock().unwrap_or_else(|e| e.into_inner());
        let retry_ready = state.attempted_at.is_none_or(|at| at.elapsed() >= RETRY);
        let closed = state.capture.as_ref().is_some_and(Capture::is_finished);
        // Some Windows versions accept a child HWND but never deliver its frames.
        let stalled = !state.use_host && latest.is_none() && retry_ready;
        if state.capture.is_some() && (closed || stalled) {
            state.stop_capture();
            state.use_host |= latest.is_none();
        }
        if state.capture.is_none() && retry_ready {
            state.attempted_at = Some(Instant::now());
            let start = |window, cursor| Frames::start_free_threaded(Settings::new(
                Window::from_raw_hwnd(window as *mut _),
                cursor,
                DrawBorderSettings::Default,
                SecondaryWindowSettings::Default,
                MinimumUpdateIntervalSettings::Default,
                DirtyRegionSettings::Default,
                ColorFormat::Rgba8,
                self.latest.clone(),
            ));
            let capture = |window| start(window, CursorCaptureSettings::WithoutCursor)
                // Older Windows versions cannot configure cursor capture.
                .or_else(|_| start(window, CursorCaptureSettings::Default));
            let result = if state.use_host {
                capture(target.host)
            } else {
                capture(target.renderer).or_else(|_| {
                    state.use_host = true;
                    capture(target.host)
                })
            };
            match result {
                Ok(capture) => state.capture = Some(capture),
                Err(error) => log::warn!("could not capture desktop wallpaper: {error}"),
            }
        }
        latest
    }
}

struct Reader(Arc<Shared>);
impl WallpaperReader for Reader {
    fn sample(&mut self) -> Option<Colour> {
        self.0.sample()
    }
}

pub fn open() -> Box<dyn WallpaperReader> {
    static SHARED: OnceLock<Mutex<Weak<Shared>>> = OnceLock::new();
    let mut weak = SHARED.get_or_init(|| Mutex::new(Weak::new()))
        .lock().unwrap_or_else(|e| e.into_inner());
    let shared = weak.upgrade().unwrap_or_else(|| {
        let shared = Arc::new(Shared::default());
        *weak = Arc::downgrade(&shared);
        shared
    });
    Box::new(Reader(shared))
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 128];
    let length = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WallpaperWindow {
    renderer: isize,
    host: isize,
}

unsafe extern "system" fn find_wallpaper_child(hwnd: HWND, context: LPARAM) -> BOOL {
    if IsWindowVisible(hwnd) != 0 && class_name(hwnd).starts_with("WPEDesktop") {
        *(context as *mut WallpaperWindow) = WallpaperWindow {
            renderer: hwnd as isize,
            host: GetAncestor(hwnd, GA_ROOT) as isize,
        };
        return 0;
    }
    1
}

unsafe extern "system" fn find_wallpaper_root(hwnd: HWND, context: LPARAM) -> BOOL {
    // Restrict discovery to Explorer's desktop hosts. Never capture the
    // Wallpaper Engine settings window or arbitrary foreground windows.
    if IsWindowVisible(hwnd) != 0 && matches!(class_name(hwnd).as_str(), "Progman" | "WorkerW") {
        EnumChildWindows(hwnd, Some(find_wallpaper_child), context);
        if (*(context as *const WallpaperWindow)).renderer != 0 {
            return 0;
        }
    }
    1
}

fn wallpaper_window() -> Option<WallpaperWindow> {
    let mut wallpaper = WallpaperWindow::default();
    unsafe {
        EnumWindows(Some(find_wallpaper_root), &mut wallpaper as *mut _ as LPARAM);
    }
    (wallpaper.renderer != 0 && wallpaper.host != 0).then_some(wallpaper)
}
