//! Windows tray host. This process never creates an egui context or an OpenGL
//! window. Each UI is a disposable child connected through inherited pipes.

use crate::{appwatch, ipc, settings::Settings, shell::{self, Tray, TrayAction}, updates::Updater, worker::{self, Command, Shared}};
use std::{io, process::{Child, Stdio}, sync::{atomic::{AtomicBool, Ordering}, mpsc, Arc, Condvar, Mutex}, time::{Duration, Instant}};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*,
};

const CLASS: &str = "Mirage Hub.TrayHost";
const OPEN: u32 = WM_APP + 21;
const RESTART: u32 = WM_APP + 22;
const QUIT: u32 = WM_APP + 23;
const SETTINGS: u32 = WM_APP + 24;
const CANCEL_QUIT: u32 = WM_APP + 25;
const CHILD_DONE: u32 = WM_APP + 26;

fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() }
fn post(window: usize, message: u32) { unsafe { PostMessageW(window as HWND, message, 0, 0); } }

#[derive(Default)]
struct Changed { revision: Mutex<u64>, wake: Condvar }
impl Changed {
    fn notify(&self) { *self.revision.lock().unwrap() += 1; self.wake.notify_all(); }
}

struct Runtime {
    shared: Shared,
    commands: worker::Sender,
    rules: appwatch::Rules,
    updates: Arc<Updater>,
    changed: Arc<Changed>,
    shutdown: AtomicBool,
}

impl Runtime {
    fn new() -> Arc<Self> {
        let settings = Settings::load();
        mirage_hub_core::i18n::set(settings.language);
        let shared = Shared::default();
        let serials = Arc::new(Mutex::new(Vec::new()));
        let changed = Arc::new(Changed::default());
        let wake = changed.clone();
        let commands = worker::spawn(shared.clone(), serials.clone(), move || wake.notify());
        let rules = Arc::new(Mutex::new(settings.clone()));
        appwatch::spawn(rules.clone(), commands.clone(), serials);
        let wake = changed.clone();
        let updates = Arc::new(Updater::start(move || wake.notify(), settings.check_updates));
        crate::daemon::refresh_autostart();
        Arc::new(Self { shared, commands, rules, updates, changed, shutdown: AtomicBool::new(false) })
    }
}

struct UiChild {
    process: Child,
    stop: Arc<AtomicBool>,
    closing: Arc<AtomicBool>,
    reader_done: Arc<AtomicBool>,
}
impl Drop for UiChild {
    fn drop(&mut self) { self.stop.store(true, Ordering::Release); }
}

fn launch(runtime: Arc<Runtime>, window: usize) -> io::Result<UiChild> {
    let logs = mirage_hub_core::paths::studio_dir();
    std::fs::create_dir_all(&logs)?;
    let stderr = std::fs::OpenOptions::new().create(true).append(true).open(logs.join("ui.log"))?;
    let mut process = std::process::Command::new(std::env::current_exe()?)
        .arg("--ui").env("MIRAGE_HUB_UI_CHILD", "1")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(stderr).spawn()?;
    let mut input = process.stdin.take().expect("piped stdin");
    let output = process.stdout.take().expect("piped stdout");
    let stop = Arc::new(AtomicBool::new(false));
    let closing = Arc::new(AtomicBool::new(false));
    let reader_done = Arc::new(AtomicBool::new(false));
    let read_runtime = runtime.clone();
    let read_closing = closing.clone();
    let done = reader_done.clone();
    std::thread::Builder::new().name("ui-commands".into()).spawn(move || {
        let mut reader = io::BufReader::new(output);
        while let Ok(request) = ipc::read::<ipc::Request>(&mut reader) {
            match request {
                ipc::Request::Command(command) => { let _ = read_runtime.commands.send(command); }
                ipc::Request::Settings(settings) => {
                    read_runtime.updates.set_enabled(settings.check_updates);
                    *read_runtime.rules.lock().unwrap() = settings;
                    post(window, SETTINGS);
                }
                ipc::Request::Update(action) => read_runtime.updates.dispatch(action),
                ipc::Request::Closing { quit_host } => {
                    read_closing.store(true, Ordering::Release);
                    if quit_host { post(window, QUIT); }
                }
                ipc::Request::CancelQuit => post(window, CANCEL_QUIT),
            }
        }
        done.store(true, Ordering::Release);
        post(window, CHILD_DONE);
    }).inspect_err(|_| { let _ = process.kill(); let _ = process.wait(); })?;
    let stopped = stop.clone();
    std::thread::Builder::new().name("ui-state".into()).spawn(move || {
        let mut first = true;
        let mut revision = 0;
        while !stopped.load(Ordering::Acquire) {
            let state = ipc::State {
                snapshot: runtime.shared.lock().unwrap().clone(),
                updates: runtime.updates.status(),
                just_updated: if first { runtime.updates.just_updated() } else { None },
                shutdown: runtime.shutdown.load(Ordering::Acquire),
            };
            first = false;
            if ipc::write(&mut input, &state).is_err() { break; }
            let guard = runtime.changed.revision.lock().unwrap();
            let (guard, _) = runtime.changed.wake.wait_timeout_while(guard, Duration::from_secs(2), |value| *value == revision).unwrap();
            revision = *guard;
        }
    }).inspect_err(|_| { let _ = process.kill(); let _ = process.wait(); })?;
    Ok(UiChild { process, stop, closing, reader_done })
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    DefWindowProcW(window, message, wparam, lparam)
}

/// A second invocation can arrive while the first is still creating the tray.
fn open_existing() {
    let class = wide(CLASS);
    for _ in 0..50 {
        let window = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        if !window.is_null() {
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(window, &mut pid); AllowSetForegroundWindow(pid); }
            shell::show();
            post(window as usize, OPEN);
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn run(background: bool) -> io::Result<()> {
    if !shell::claim_single_instance() {
        if !background { open_existing(); }
        return Ok(());
    }
    let class = wide(CLASS);
    let title = wide("Mirage Hub Background");
    let window = unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let descriptor = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        if RegisterClassW(&descriptor) == 0 { return Err(io::Error::last_os_error()); }
        CreateWindowExW(0, class.as_ptr(), title.as_ptr(), 0, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), instance, std::ptr::null())
    };
    if window.is_null() { return Err(io::Error::last_os_error()); }
    let address = window as usize;
    let runtime = Runtime::new();
    let tray = Tray::new(move |action| post(address, match action { TrayAction::Open => OPEN, TrayAction::RestartDaemon => RESTART, TrayAction::Quit => QUIT }));
    mirage_hub_core::process::listen_for_quit(move || post(address, QUIT));
    unsafe { SetTimer(window, 1, 500, None); }
    let mut child: Option<UiChild> = None;
    let mut reopen = !background || tray.is_none();
    let mut quitting: Option<Instant> = None;
    let mut drained: Option<mpsc::Receiver<()>> = None;
    let mut message: MSG = unsafe { std::mem::zeroed() };
    loop {
        if reopen && child.is_none() && quitting.is_none() {
            reopen = false;
            match launch(runtime.clone(), address) {
                Ok(ui) => child = Some(ui),
                Err(error) => {
                    let text = wide(&error.to_string());
                    unsafe { MessageBoxW(window, text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR); }
                    if tray.is_none() { break; }
                }
            }
        }
        let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if result <= 0 { break; }
        if message.hwnd == window {
            match message.message {
                OPEN if quitting.is_none() => {
                    if let Some(ui) = &mut child {
                        if ui.closing.load(Ordering::Acquire) || ui.process.try_wait()?.is_some() { reopen = true; }
                        else { shell::show(); }
                    } else { reopen = true; }
                }
                RESTART => { let _ = runtime.commands.send(Command::RestartDaemon); }
                SETTINGS => {
                    mirage_hub_core::i18n::set(runtime.rules.lock().unwrap().language);
                    if let Some(tray) = &tray { tray.refresh_language(); }
                }
                QUIT => {
                    quitting.get_or_insert_with(Instant::now);
                    runtime.shutdown.store(true, Ordering::Release);
                    runtime.changed.notify();
                }
                CANCEL_QUIT => { quitting = None; runtime.shutdown.store(false, Ordering::Release); runtime.changed.notify(); }
                WM_TIMER | CHILD_DONE => {
                    if let Some(ui) = &mut child {
                        if quitting.is_some_and(|at| at.elapsed() > Duration::from_secs(5)) {
                            let _ = ui.process.kill();
                        }
                        if ui.process.try_wait()?.is_some() && ui.reader_done.load(Ordering::Acquire) {
                            child = None;
                            runtime.changed.notify();
                            if tray.is_none() { quitting.get_or_insert_with(Instant::now); }
                        }
                    }
                    if let Some(at) = quitting.filter(|_| child.is_none()) {
                        // The pipe reader has accepted every command before EOF.
                        // Let the worker finish them before the host terminates.
                        if drained.is_none() {
                            let (tx, rx) = mpsc::channel();
                            let _ = runtime.commands.send(Command::Drain(tx));
                            drained = Some(rx);
                        }
                        if drained.as_ref().unwrap().try_recv().is_ok() || at.elapsed() > Duration::from_secs(45) { break; }
                    }
                }
                _ => {}
            }
        }
        unsafe { TranslateMessage(&message); DispatchMessageW(&message); }
    }
    drop(tray); // Remove the tray icon on its owning thread.
    unsafe { KillTimer(window, 1); DestroyWindow(window); }
    Ok(())
}
