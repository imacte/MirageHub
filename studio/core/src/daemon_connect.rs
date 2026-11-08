//! Shared by Studio, the classic panel and the native CLI.
//! Startup policy from openrazer-win #15: reject a dead local PID before TCP,
//! try loopback briefly, but retain the full timeout when the daemon may live.
use std::{
    io,
    net::{SocketAddr, TcpStream},
    time::Duration,
};

pub fn connect(address: &SocketAddr, pid: Option<u32>) -> io::Result<TcpStream> {
    const FULL: Duration = Duration::from_secs(2);
    if !address.ip().is_loopback() {
        return TcpStream::connect_timeout(address, FULL);
    }
    if pid.is_some_and(|pid| pid != 0 && !is_alive(pid)) {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "daemon process has exited",
        ));
    }
    match TcpStream::connect_timeout(address, Duration::from_millis(250)) {
        Err(error) if retry(error.kind(), pid, is_alive) => {
            TcpStream::connect_timeout(address, FULL)
        }
        result => result,
    }
}

fn retry(kind: io::ErrorKind, pid: Option<u32>, alive: impl Fn(u32) -> bool) -> bool {
    kind == io::ErrorKind::TimedOut && pid.is_none_or(|pid| pid == 0 || alive(pid))
}

#[cfg(windows)]
fn alive_after_open_failed(error: u32) -> bool {
    error != windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER
}

#[cfg(windows)]
pub fn is_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return alive_after_open_failed(GetLastError());
        }
        let mut code = 0;
        let known = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        !known || code == 259 // STILL_ACTIVE; access denied always means unknown/alive.
    }
}

#[cfg(not(windows))]
pub fn is_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn retry_only_when_slow_and_not_known_dead() {
        use io::ErrorKind::*;
        assert!(retry(TimedOut, Some(7), |_| true));
        assert!(retry(TimedOut, None, |_| false));
        assert!(retry(TimedOut, Some(0), |_| false));
        assert!(!retry(TimedOut, Some(7), |_| false));
        assert!(!retry(ConnectionRefused, Some(7), |_| true));
        assert!(!retry(Other, None, |_| true));
    }

    #[test]
    fn live_daemon_and_legacy_endpoint_connect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        assert!(connect(&address, Some(std::process::id())).is_ok());
        assert!(connect(&address, None).is_ok());
        assert!(is_alive(std::process::id()));
    }

    #[cfg(windows)]
    #[test]
    fn dead_pid_is_rejected_on_the_entire_loopback_range() {
        // An open listener would accept a TCP connection. A dead PID must
        // reject it before connecting, including alternate 127.x addresses.
        for host in ["127.0.0.1", "127.42.0.1", "::1"] {
            let listener = TcpListener::bind((host, 0)).unwrap();
            assert!(connect(&listener.local_addr().unwrap(), Some(0xffff_fff0)).is_err());
        }
        assert!(!is_alive(0xffff_fff0));
        assert!(!alive_after_open_failed(87));
        assert!(alive_after_open_failed(5));
    }
}
