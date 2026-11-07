//! A loopback listener which cannot have its port taken over on Windows.
use crate::worker::Cancellation;
use std::{
    cell::Cell,
    io::{Read, Write},
    net::TcpStream,
    sync::Arc,
    time::{Duration, Instant},
};
use std::{io, net::TcpListener, ops::Deref};

/// Nonblocking socket I/O with cancellable readiness waits. Socket receive
/// timeouts are not used as a polling primitive: Winsock says a connection
/// which times out in a blocking recv must be closed, rather than resumed.
pub struct InterruptibleStream {
    stream: TcpStream,
    cancel: Arc<Cancellation>,
    read_deadline: Cell<Option<Instant>>,
    write_deadline: Cell<Option<Instant>>,
}
impl InterruptibleStream {
    pub fn new(stream: TcpStream, cancel: Arc<Cancellation>) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            cancel,
            read_deadline: Cell::new(None),
            write_deadline: Cell::new(None),
        })
    }
    pub fn set_read_deadline(&self, deadline: Option<Instant>) {
        self.read_deadline.set(deadline);
    }
    pub fn set_write_deadline(&self, deadline: Option<Instant>) {
        self.write_deadline.set(deadline);
    }
    fn check(&self, deadline: Option<Instant>) -> io::Result<()> {
        if self.cancel.stopped() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "server stopped",
            ));
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "request timed out"));
        }
        Ok(())
    }
    fn wait_ready(&self, writing: bool, deadline: Option<Instant>) -> io::Result<()> {
        self.check(deadline)?;
        let delay = deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_millis(100))
            .min(Duration::from_millis(100));
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawSocket;
            use windows_sys::Win32::Networking::WinSock::{
                WSAGetLastError, WSAPoll, POLLRDNORM, POLLWRNORM, SOCKET_ERROR, WSAPOLLFD,
            };
            let mut fd = WSAPOLLFD {
                fd: self.stream.as_raw_socket() as _,
                events: if writing { POLLWRNORM } else { POLLRDNORM },
                revents: 0,
            };
            let wait_ms = delay.as_millis().max(1) as i32;
            if unsafe { WSAPoll(&mut fd, 1, wait_ms) } == SOCKET_ERROR {
                return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
            }
        }
        #[cfg(not(windows))]
        {
            let _ = writing;
            self.cancel.wait(delay.min(Duration::from_millis(2)));
        }
        self.check(deadline)
    }
}
impl Read for InterruptibleStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check(self.read_deadline.get())?;
            match self.stream.read(buffer) {
                Ok(count) => return Ok(count),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.wait_ready(false, self.read_deadline.get())?
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }
}
impl Write for InterruptibleStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        loop {
            self.check(self.write_deadline.get())?;
            match self.stream.write(buffer) {
                Ok(count) => return Ok(count),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.wait_ready(true, self.write_deadline.get())?
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct LoopbackListener {
    listener: TcpListener,
    // Fields drop in declaration order: close the socket before releasing WSA.
    #[cfg(windows)]
    _winsock: Winsock,
}
impl Deref for LoopbackListener {
    type Target = TcpListener;
    fn deref(&self) -> &TcpListener {
        &self.listener
    }
}
#[cfg(not(windows))]
impl LoopbackListener {
    pub fn bind(port: u16) -> io::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(([127, 0, 0, 1], port))?,
        })
    }
}
#[cfg(windows)]
struct Winsock;
#[cfg(windows)]
impl Drop for Winsock {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Networking::WinSock::WSACleanup();
        }
    }
}
#[cfg(windows)]
impl LoopbackListener {
    pub fn bind(port: u16) -> io::Result<Self> {
        use std::os::windows::io::{AsRawSocket, FromRawSocket, OwnedSocket};
        use windows_sys::Win32::Networking::WinSock::*;
        fn error() -> io::Error {
            io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
        }
        unsafe {
            let mut data: WSADATA = std::mem::zeroed();
            let code = WSAStartup(0x0202, &mut data);
            if code != 0 {
                return Err(io::Error::from_raw_os_error(code));
            }
            let winsock = Winsock;
            let raw = socket(AF_INET as i32, SOCK_STREAM, IPPROTO_TCP);
            if raw == INVALID_SOCKET {
                return Err(error());
            }
            let owned = OwnedSocket::from_raw_socket(raw as _);
            let exclusive = 1i32;
            if setsockopt(
                owned.as_raw_socket() as _,
                SOL_SOCKET,
                SO_EXCLUSIVEADDRUSE,
                (&exclusive as *const i32).cast(),
                std::mem::size_of_val(&exclusive) as i32,
            ) == SOCKET_ERROR
            {
                return Err(error());
            }
            let address = SOCKADDR_IN {
                sin_family: AF_INET,
                sin_port: port.to_be(),
                sin_addr: IN_ADDR {
                    S_un: IN_ADDR_0 {
                        S_addr: u32::from_ne_bytes([127, 0, 0, 1]),
                    },
                },
                sin_zero: [0; 8],
            };
            if bind(
                owned.as_raw_socket() as _,
                (&address as *const SOCKADDR_IN).cast(),
                std::mem::size_of_val(&address) as i32,
            ) == SOCKET_ERROR
            {
                return Err(error());
            }
            if listen(owned.as_raw_socket() as _, SOMAXCONN as i32) == SOCKET_ERROR {
                return Err(error());
            }
            Ok(Self {
                listener: TcpListener::from(owned),
                _winsock: winsock,
            })
        }
    }
}
