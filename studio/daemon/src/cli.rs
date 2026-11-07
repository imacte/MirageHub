use crate::{
    database, demo, logging, persistence,
    rpc::{Client, SingleInstance},
    runtime::{self, RunningDaemon},
    service::UPSTREAM_VERSION,
    Error, Result,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

pub struct Arguments {
    pub positional: Vec<String>,
    pub values: BTreeMap<String, String>,
    pub flags: BTreeSet<String>,
}
impl Arguments {
    pub fn parse(args: Vec<String>) -> Result<Self> {
        let mut parsed = Self {
            positional: Vec::new(),
            values: BTreeMap::new(),
            flags: BTreeSet::new(),
        };
        let mut args = args.into_iter();
        let mut positional_only = false;
        while let Some(arg) = args.next() {
            if positional_only {
                parsed.positional.push(arg);
                continue;
            }
            if arg == "--" {
                positional_only = true;
                continue;
            }
            let (raw, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(key, value)| (key, Some(value)));
            let key = match raw {
                "-h" => "--help",
                "-v" => "--verbose",
                "-q" => "--quiet",
                "-d" => "--device",
                "-z" => "--zone",
                "--color2" => "--colour2",
                "--color3" => "--colour3",
                _ => raw,
            };
            if [
                "--help",
                "--version",
                "--verbose",
                "--quiet",
                "--no-effects",
                "--no-keys",
                "--emulate",
                "--json",
                "--direct",
                "--disable",
            ]
            .contains(&key)
            {
                if inline.is_some() {
                    return Err(Error::Invalid(format!("{key} does not take a value")));
                }
                parsed.flags.insert(key.into());
            } else if [
                "--host",
                "--port",
                "--chroma-port",
                "--config-dir",
                "--log-file",
                "--device",
                "--zone",
                "--speed",
                "--direction",
                "--colour2",
                "--colour3",
                "--keys",
                "--text",
                "--launch",
            ]
            .contains(&key)
            {
                let value = inline
                    .map(str::to_string)
                    .or_else(|| args.next())
                    .ok_or_else(|| Error::Invalid(format!("{key} requires a value")))?;
                parsed.values.insert(key.into(), value);
            } else if arg.starts_with('-') && !arg.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
            {
                return Err(Error::Invalid(format!("unknown option {arg}")));
            } else {
                parsed.positional.push(arg);
            }
        }
        Ok(parsed)
    }
    pub fn flag(&self, key: &str) -> bool {
        self.flags.contains(key)
    }
    pub fn value(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
    pub fn directory(&self) -> PathBuf {
        self.value("--config-dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let root = persistence::config_dir();
                if self.flag("--emulate") {
                    root.join("demo")
                } else {
                    root
                }
            })
    }
    fn port(&self, key: &str, default: u16) -> Result<u16> {
        self.value(key).map_or(Ok(default), |v| {
            v.parse()
                .map_err(|_| Error::Invalid(format!("{key} must be a port from 0 to 65535")))
        })
    }
    fn validate_bind(&self) -> Result<()> {
        let host = self.value("--host").unwrap_or("127.0.0.1");
        if host != "127.0.0.1" {
            return Err(Error::Invalid("the daemon binds only to 127.0.0.1".into()));
        }
        self.port("--port", 0)?;
        self.port("--chroma-port", 0)?;
        Ok(())
    }
}
fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("JSON value")
    );
}
fn rpc_error(error: crate::rpc::RpcError) -> Error {
    Error::Device(error.to_string())
}
fn connect(args: &Arguments) -> Result<Client> {
    Client::connect(&args.directory().join("daemon.json")).map_err(rpc_error)
}
fn version(args: &Arguments) -> Result<Value> {
    connect(args)?
        .call("daemon.version", json!({}))
        .map_err(rpc_error)
}
pub fn run(raw: Vec<String>) -> Result<i32> {
    let args = Arguments::parse(raw)?;
    if args.flag("--version") {
        println!(
            "openrazer-win {} (openrazer {UPSTREAM_VERSION}; Rust backend)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(0);
    }
    if args.flag("--help") || args.positional.is_empty() {
        println!("OpenRazer for Windows — native Rust backend\n\nUsage: openrazer-win [options] <command>\n\nCommands:\n  daemon run|start|stop|restart|status\n  list                       List connected devices\n  supported [search]         List supported device models\n  rpc <method> [JSON]        Call a daemon method\n\nDaemon options:\n  --no-effects  --no-keys  --verbose\n  --emulate                  Use simulated devices and a separate demo configuration\n  --config-dir PATH          Use this configuration directory\n  --port PORT                Loopback RPC port (default: automatic)\n  --chroma-port PORT         Chroma REST port\n  --log-file PATH            Log file (default: CONFIG/daemon.log)\n\nOther options: --json  --version  --help");
        return Ok(0);
    }
    match args.positional[0].as_str() {
        "daemon" => daemon(&args),
        "supported" => {
            let search = args
                .positional
                .get(1)
                .map_or("", String::as_str)
                .to_lowercase();
            let devices: Vec<_> = database::devices()
                .iter()
                .filter(|d| {
                    d.name.to_lowercase().contains(&search) || format!("{:04x}", d.pid) == search
                })
                .collect();
            if args.flag("--json") {
                print_json(&json!(devices
                    .iter()
                    .map(|d| d.to_json())
                    .collect::<Vec<_>>()));
            } else {
                for d in devices {
                    println!("{:04x}  {:<10} {}", d.pid, d.device_type, d.name);
                }
            }
            Ok(0)
        }
        "list" => {
            let value = connect(&args)?
                .call("devices.list", json!({}))
                .map_err(rpc_error)?;
            print_json(&value);
            Ok(0)
        }
        "rpc" => {
            let method = args
                .positional
                .get(1)
                .ok_or_else(|| Error::Invalid("rpc requires a method name".into()))?;
            let params = args
                .positional
                .get(2)
                .map_or(Ok(json!({})), |v| serde_json::from_str(v))?;
            print_json(&connect(&args)?.call(method, params).map_err(rpc_error)?);
            Ok(0)
        }
        command => Err(Error::Invalid(format!("unknown command {command}"))),
    }
}
fn daemon(args: &Arguments) -> Result<i32> {
    args.validate_bind()?;
    match args.positional.get(1).map_or("start", String::as_str) {
        "run" => run_daemon(args),
        "status" => match version(args) {
            Ok(v) => {
                print_json(&v);
                Ok(0)
            }
            Err(_) => {
                println!("The daemon is not running.");
                Ok(1)
            }
        },
        "stop" => {
            stop_daemon(args)?;
            Ok(0)
        }
        "start" => {
            start_daemon(args)?;
            Ok(0)
        }
        "restart" => {
            if version(args).is_ok() {
                stop_daemon(args)?;
            }
            start_daemon(args)?;
            Ok(0)
        }
        value => Err(Error::Invalid(format!("unknown daemon action {value}"))),
    }
}
fn stop_daemon(args: &Arguments) -> Result<()> {
    let path = args.directory().join("daemon.json");
    connect(args)?
        .call("daemon.shutdown", json!({}))
        .map_err(rpc_error)?;
    let deadline = Instant::now() + Duration::from_secs(45);
    while path.exists() {
        if Instant::now() >= deadline {
            return Err(Error::Io(
                "the daemon has not finished shutting down".into(),
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
fn start_daemon(args: &Arguments) -> Result<()> {
    if version(args).is_ok() {
        return Ok(());
    }
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["daemon", "run", "--config-dir"])
        .arg(args.directory());
    for flag in ["--no-effects", "--no-keys", "--emulate", "--verbose"] {
        if args.flag(flag) {
            command.arg(flag);
        }
    }
    for flag in ["--port", "--chroma-port", "--log-file"] {
        if let Some(value) = args.value(flag) {
            command.args([flag, value]);
        }
    }
    let mut child = crate::process::spawn_hidden(
        std::path::Path::new(command.get_program()),
        &command
            .get_args()
            .map(|arg| arg.to_os_string())
            .collect::<Vec<_>>(),
    )?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if version(args).is_ok() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(Error::Io(format!(
                "daemon exited ({status}); see {}",
                args.directory().join("daemon.log").display()
            )));
        }
        if Instant::now() >= deadline {
            let _ = child.terminate();
            return Err(Error::Io("daemon did not answer within 30 seconds".into()));
        }
        thread::sleep(Duration::from_millis(100));
    }
}
fn run_daemon(args: &Arguments) -> Result<i32> {
    let directory = args.directory();
    let name = runtime::instance_name(&directory, args.flag("--emulate"))?;
    let Some(instance) = SingleInstance::acquire(&name)? else {
        return Ok(0);
    };
    let log_path = args
        .value("--log-file")
        .map(PathBuf::from)
        .unwrap_or_else(|| directory.join("daemon.log"));
    logging::configure(&log_path, args.flag("--verbose"))?;
    let port = args.port("--port", 0)?;
    let chroma_port = args.port(
        "--chroma-port",
        if args.flag("--emulate") {
            0
        } else {
            crate::chroma::rest::PORT
        },
    )?;
    let effects = !args.flag("--no-effects");
    let keys = !args.flag("--no-keys");
    let service = if args.flag("--emulate") {
        demo::service(&directory, effects, keys, chroma_port)?
    } else {
        #[cfg(windows)]
        {
            crate::service::DaemonService::native_with_options(
                &directory,
                effects,
                keys,
                crate::service::Options {
                    chroma_port,
                    ..crate::service::Options::default()
                },
            )
        }
        #[cfg(not(windows))]
        {
            return Err(Error::NotSupported(
                "native hardware requires Windows".into(),
            ));
        }
    };
    let _console = ConsoleHandler::install();
    let mut daemon = RunningDaemon::start(service, &directory, port, instance)?;
    while !daemon.wait_for_shutdown(Duration::from_millis(100))
        && !EXIT_REQUESTED.load(Ordering::Acquire)
    {}
    daemon.stop();
    Ok(0)
}
static EXIT_REQUESTED: AtomicBool = AtomicBool::new(false);
static EXIT_COMPLETE: AtomicBool = AtomicBool::new(false);
struct ConsoleHandler;
impl ConsoleHandler {
    fn install() -> Self {
        EXIT_REQUESTED.store(false, Ordering::Release);
        EXIT_COMPLETE.store(false, Ordering::Release);
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(console_event), 1);
        }
        Self
    }
}
impl Drop for ConsoleHandler {
    fn drop(&mut self) {
        EXIT_COMPLETE.store(true, Ordering::Release);
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(console_event), 0);
        }
    }
}
#[cfg(windows)]
unsafe extern "system" fn console_event(event: u32) -> i32 {
    if ![0, 1, 2, 5, 6].contains(&event) {
        return 0;
    }
    EXIT_REQUESTED.store(true, Ordering::Release);
    if event >= 2 {
        for _ in 0..45 {
            if EXIT_COMPLETE.load(Ordering::Acquire) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    1
}
