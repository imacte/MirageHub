//! Native hook pump. Deferred resumes, foreground changes, configuration and
//! idle rearming all run on this same thread as the Snap Tap state machine.
use super::*;
use crate::keys::{
    inject::{Injection, Injector, NativeInjector},
    snaptap_host::{self, Attachment, Command, Controller, Host},
};
use std::{
    cell::{Cell, RefCell},
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM},
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{
            GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        Input::KeyboardAndMouse::GetAsyncKeyState,
        WindowsAndMessaging::*,
    },
};

const CONFIGURE: u32 = WM_APP + 17;
const RESUME: u32 = WM_APP + 18;
const SYSTEM_CHANGE: u32 = WM_APP + 19;
const REARM_AFTER: Duration = Duration::from_secs(30);

struct Context {
    handler: Handler,
    host: Option<Host>,
    injector: Arc<dyn Injector>,
    last_callback: Instant,
    last_window: usize,
    watched: bool,
}
thread_local! {
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
    static SYSTEM_DIRTY: Cell<bool> = const { Cell::new(false) };
}
fn with_context<T>(f: impl FnOnce(&mut Context) -> T) -> Option<T> {
    CONTEXT.with(|slot| slot.borrow_mut().as_mut().map(f))
}
fn key_down(vk: u32) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) < 0 }
}

fn foreground(window: HWND) -> Option<String> {
    if window.is_null() {
        return None;
    }
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(window, &mut pid);
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut path = vec![0u16; 32768];
        let mut length = path.len() as u32;
        let known = QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut length) != 0;
        CloseHandle(handle);
        if !known {
            return None;
        }
        String::from_utf16(&path[..length as usize])
            .ok()
            .and_then(|path| path.rsplit('\\').next().map(str::to_lowercase))
    }
}

fn system_change() {
    let enabled =
        with_context(|ctx| ctx.host.as_ref().is_some_and(|h| h.settings.enabled)).unwrap_or(false);
    let window = unsafe { GetForegroundWindow() };
    // Process path lookup happens on changes, never on every key event.
    let name = enabled.then(|| foreground(window)).flatten();
    with_context(|ctx| {
        ctx.last_window = window as usize;
        if let Some(host) = &mut ctx.host {
            host.system_change(name, key_down);
            if enabled && !ctx.watched {
                host.unavailable_watcher();
            }
        }
    });
}

fn refresh_foreground() {
    let window = unsafe { GetForegroundWindow() } as usize;
    let changed = SYSTEM_DIRTY.with(|dirty| dirty.replace(false))
        || with_context(|ctx| ctx.last_window != window).unwrap_or(false);
    if changed {
        system_change();
    }
}

struct WindowWatch(Vec<HWINEVENTHOOK>);
impl WindowWatch {
    fn new() -> Self {
        let mut hooks = Vec::new();
        for event in [EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_DESKTOPSWITCH] {
            let hook = unsafe {
                SetWinEventHook(
                    event,
                    event,
                    ptr::null_mut(),
                    Some(window_changed),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };
            if !hook.is_null() {
                hooks.push(hook);
            }
        }
        Self(hooks)
    }
}
impl Drop for WindowWatch {
    fn drop(&mut self) {
        for hook in &self.0 {
            unsafe {
                UnhookWinEvent(*hook);
            }
        }
    }
}
unsafe extern "system" fn window_changed(
    _: HWINEVENTHOOK,
    _: u32,
    _: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    SYSTEM_DIRTY.with(|dirty| dirty.set(true));
    unsafe {
        PostThreadMessageW(GetCurrentThreadId(), SYSTEM_CHANGE, 0, 0);
    }
}

fn send(events: &[Injection]) {
    if events.is_empty() {
        return;
    }
    let injector = with_context(|ctx| ctx.injector.clone());
    if injector.is_some_and(|injector| injector.emit(events).is_err()) {
        with_context(|ctx| {
            if let Some(host) = &mut ctx.host {
                host.state.failed_sends += 1;
            }
        });
    }
}

fn handle(input: KeyEvent) -> bool {
    with_context(|ctx| ctx.last_callback = Instant::now());
    if !input.injected
        && with_context(|ctx| ctx.host.as_ref().is_some_and(|host| host.settings.enabled))
            .unwrap_or(false)
    {
        refresh_foreground();
    }
    // Release the RefCell borrow before calling a filter or SendInput, which
    // can synchronously reenter this hook with an injected event.
    let handler = with_context(|ctx| ctx.handler.clone());
    let swallowed = handler.is_some_and(|handler| handler(input));
    let decision = with_context(|ctx| ctx.host.as_mut().map(|host| host.handle(input, swallowed)))
        .flatten()
        .unwrap_or_default();
    if let Some(key) = decision.release {
        send(&[snaptap_host::injection(key, false)]);
    }
    if decision.later && unsafe { PostThreadMessageW(GetCurrentThreadId(), RESUME, 0, 0) } == 0 {
        with_context(|ctx| {
            if let Some(host) = &mut ctx.host {
                host.cancel_resumes();
            }
        });
    }
    swallowed || decision.swallow
}

unsafe extern "system" fn procedure(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code < 0
        || (data as *const KBDLLHOOKSTRUCT).is_null()
        || !matches!(
            message as u32,
            WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP
        )
    {
        return unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) };
    }
    let started = Instant::now();
    let event = unsafe { &*(data as *const KBDLLHOOKSTRUCT) };
    let input = KeyEvent {
        vk: event.vkCode,
        scan: event.scanCode,
        extended: event.flags & LLKHF_EXTENDED != 0,
        down: matches!(message as u32, WM_KEYDOWN | WM_SYSKEYDOWN),
        injected: event.flags & LLKHF_INJECTED != 0,
        ours: event.dwExtraInfo == INJECTION_MARKER,
        time: event.time,
    };
    let suppress = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle(input)))
        .unwrap_or_else(|_| {
            with_context(|ctx| {
                if let Some(host) = &mut ctx.host {
                    host.hook_error(key_down);
                }
            });
            false
        });
    let result = if suppress {
        1
    } else {
        unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) }
    };
    if !input.injected {
        with_context(|ctx| {
            if let Some(host) = &mut ctx.host {
                host.record_latency(started.elapsed());
            }
        });
    }
    result
}

pub struct KeyboardHook {
    worker: Option<JoinHandle<()>>,
    id: Arc<AtomicU32>,
    running: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}
impl KeyboardHook {
    pub fn start(handler: Handler) -> Result<Self> {
        Self::with_snaptap(handler, None, Arc::new(NativeInjector))
    }
    pub fn with_snaptap(
        handler: Handler,
        controller: Option<Arc<Controller>>,
        injector: Arc<dyn Injector>,
    ) -> Result<Self> {
        let id = Arc::new(AtomicU32::new(0));
        let running = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_id, is_running, stopping) = (id.clone(), running.clone(), stop.clone());
        let (ready, receive) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("rsrazer-keyboard".into())
            .spawn(move || unsafe {
                let mut message: MSG = std::mem::zeroed();
                PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_NOREMOVE);
                let owner = GetCurrentThreadId();
                thread_id.store(owner, Ordering::Release);
                let mut hook = SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(procedure),
                    GetModuleHandleW(ptr::null()),
                    0,
                );
                if hook.is_null() {
                    thread_id.store(0, Ordering::Release);
                    let _ = ready.send(Err(Error::Io(std::io::Error::last_os_error().to_string())));
                    return;
                }
                let (command_send, commands) = mpsc::channel();
                let host = controller.as_ref().map(|controller| {
                    let settings = controller.attach(Attachment {
                        owner,
                        send: command_send,
                        wake: Arc::new(move || PostThreadMessageW(owner, CONFIGURE, 0, 0) != 0),
                    });
                    Host::new(controller.clone(), settings)
                });
                let watch = controller.as_ref().map(|_| WindowWatch::new());
                let timer = if controller.is_some() {
                    SetTimer(ptr::null_mut(), 0, 1000, None)
                } else {
                    0
                };
                let watched = watch.as_ref().is_some_and(|w| w.0.len() == 2) && timer != 0;
                CONTEXT.with(|slot| {
                    *slot.borrow_mut() = Some(Context {
                        handler,
                        host,
                        injector,
                        last_callback: Instant::now(),
                        last_window: 0,
                        watched,
                    })
                });
                system_change();
                is_running.store(true, Ordering::Release);
                let _ = ready.send(Ok(()));
                while !stopping.load(Ordering::Acquire)
                    && GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0
                {
                    match message.message {
                        CONFIGURE => {
                            for Command::Configure(settings, done) in commands.try_iter() {
                                with_context(|ctx| {
                                    if let Some(host) = &mut ctx.host {
                                        host.configure(settings);
                                    }
                                });
                                system_change();
                                let _ = done.send(());
                            }
                        }
                        RESUME => {
                            // WinEvent delivery can lag this posted message. Check
                            // the window itself before resuming a previously held key.
                            refresh_foreground();
                            let due = with_context(|ctx| ctx.host.as_mut().map(Host::resumes))
                                .flatten()
                                .unwrap_or_default();
                            send(&due);
                        }
                        SYSTEM_CHANGE => {
                            SYSTEM_DIRTY.with(|dirty| dirty.set(false));
                            system_change();
                        }
                        WM_TIMER if message.wParam == timer => {
                            let rearm = with_context(|ctx| {
                                ctx.host.as_ref().is_some_and(|h| h.settings.enabled)
                                    && ctx.last_callback.elapsed() >= REARM_AFTER
                            })
                            .unwrap_or(false);
                            if rearm {
                                let fresh = SetWindowsHookExW(
                                    WH_KEYBOARD_LL,
                                    Some(procedure),
                                    GetModuleHandleW(ptr::null()),
                                    0,
                                );
                                if !fresh.is_null() {
                                    UnhookWindowsHookEx(hook);
                                    hook = fresh;
                                    with_context(|ctx| {
                                        ctx.last_callback = Instant::now();
                                        if let Some(host) = &mut ctx.host {
                                            host.rearm(key_down);
                                        }
                                    });
                                } else {
                                    with_context(|ctx| {
                                        // Do not keep stale held keys or retry on every
                                        // timer tick if Windows cannot install a hook.
                                        ctx.last_callback = Instant::now();
                                        if let Some(host) = &mut ctx.host {
                                            host.hook_error(key_down);
                                        }
                                    });
                                }
                            }
                            with_context(|ctx| {
                                if let Some(host) = &mut ctx.host {
                                    host.publish();
                                }
                            });
                        }
                        _ => {
                            TranslateMessage(&message);
                            DispatchMessageW(&message);
                        }
                    }
                }
                if timer != 0 {
                    KillTimer(ptr::null_mut(), timer);
                }
                drop(watch);
                UnhookWindowsHookEx(hook);
                CONTEXT.with(|slot| slot.borrow_mut().take());
                if let Some(controller) = controller {
                    controller.detach(owner);
                }
                thread_id.store(0, Ordering::Release);
                is_running.store(false, Ordering::Release);
            })?;
        let mut hook = Self {
            worker: Some(worker),
            id,
            running,
            stop,
        };
        match receive.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(hook),
            result => {
                hook.stop();
                Err(result
                    .ok()
                    .and_then(std::result::Result::err)
                    .unwrap_or_else(|| Error::Io("keyboard hook did not start".into())))
            }
        }
    }
    pub fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let id = self.id.load(Ordering::Acquire);
            if id != 0 && !worker.is_finished() {
                unsafe {
                    PostThreadMessageW(id, WM_QUIT, 0, 0);
                }
            }
            let _ = worker.join();
            self.id.store(0, Ordering::Release);
        }
    }
}
impl Drop for KeyboardHook {
    fn drop(&mut self) {
        self.stop();
    }
}
