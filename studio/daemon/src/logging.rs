//! A small rotating log for a daemon normally started without a console.
use crate::Result;
use log::{LevelFilter, Log, Metadata, Record};
use std::{
    fs::{self, File, OpenOptions},
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
const MAX_BYTES: u64 = 512 * 1024;
struct Writer {
    path: PathBuf,
    file: Option<File>,
}
impl Writer {
    fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        Ok(Self {
            path: path.into(),
            file: Some(OpenOptions::new().create(true).append(true).open(path)?),
        })
    }
    fn backup(&self, number: u8) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(format!(".{number}"));
        PathBuf::from(name)
    }
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.file.is_none() {
            self.file = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
        }
        let size = self
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map_or(0, |m| m.len());
        if size + bytes.len() as u64 > MAX_BYTES {
            self.file.take();
            let _ = fs::remove_file(self.backup(2));
            let _ = fs::rename(self.backup(1), self.backup(2));
            // If rotation is refused, skip this message rather than allowing
            // an unbounded log. The next write reopens and checks the size.
            fs::rename(&self.path, self.backup(1))?;
        }
        if self.file.is_none() {
            self.file = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
        }
        if let Some(file) = &mut self.file {
            file.write_all(bytes)?;
            file.flush()?;
        }
        Ok(())
    }
}
struct Logger {
    writer: Mutex<Writer>,
    level: LevelFilter,
    console: bool,
}
impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.level
    }
    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let message: String = record.args().to_string().chars().take(8192).collect();
        let line = format!(
            "{}  {:<5} {}: {message}\n",
            timestamp(),
            record.level(),
            record.target()
        );
        let _ = self
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .write(line.as_bytes());
        if self.console {
            eprint!("{line}");
        }
    }
    fn flush(&self) {
        if let Some(f) = &mut self.writer.lock().unwrap_or_else(|e| e.into_inner()).file {
            let _ = f.flush();
        }
    }
}
pub fn configure(path: &Path, verbose: bool) -> Result<()> {
    let level = if verbose {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    let console = std::io::stderr().is_terminal();
    let writer = Writer::open(path)?;
    log::set_boxed_logger(Box::new(Logger {
        writer: Mutex::new(writer),
        level,
        console,
    }))
    .map_err(|e| crate::Error::Io(e.to_string()))?;
    log::set_max_level(level);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!(
            "uncaught panic in {}: {info}",
            std::thread::current().name().unwrap_or("unnamed thread")
        );
        log::logger().flush();
        if console {
            previous(info);
        }
    }));
    Ok(())
}
pub fn finish() {
    log::logger().flush();
}
#[cfg(windows)]
fn timestamp() -> String {
    use windows_sys::Win32::{Foundation::SYSTEMTIME, System::SystemInformation::GetLocalTime};
    let mut value: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe {
        GetLocalTime(&mut value);
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        value.wYear,
        value.wMonth,
        value.wDay,
        value.wHour,
        value.wMinute,
        value.wSecond,
        value.wMilliseconds
    )
}
#[cfg(not(windows))]
fn timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotation_keeps_two_backups_and_the_current_file() {
        let dir = std::env::temp_dir().join(format!(
            "openrazer-log-{}",
            crate::rpc::new_token().unwrap()
        ));
        let path = dir.join("daemon.log");
        let mut writer = Writer::open(&path).unwrap();
        for i in 0..4 {
            writer.write(&vec![b'A' + i; MAX_BYTES as usize]).unwrap();
        }
        drop(writer);
        assert_eq!(fs::read(&path).unwrap()[0], b'D');
        assert_eq!(fs::read(dir.join("daemon.log.1")).unwrap()[0], b'C');
        assert_eq!(fs::read(dir.join("daemon.log.2")).unwrap()[0], b'B');
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 3);
        fs::remove_dir_all(dir).unwrap();
    }
}
