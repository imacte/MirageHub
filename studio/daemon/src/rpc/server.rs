use super::{
    truthy, Endpoint, RpcError, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, MAX_MESSAGE_SIZE,
    PARSE_ERROR, UNAUTHORIZED,
};
use crate::{
    net::{InterruptibleStream, LoopbackListener},
    worker::Cancellation,
};
use serde_json::{json, Map, Value};
use std::{
    io::{self, BufRead, BufReader, Write},
    net::TcpStream,
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub trait Service: Send + Sync {
    fn dispatch(&self, method: &str, params: &Map<String, Value>) -> super::Result<Value>;
}
const MAX_CONNECTIONS: usize = 64;
const IO_TIMEOUT: Duration = Duration::from_secs(5);

enum ReadError {
    Io,
    Message(RpcError),
}
impl From<io::Error> for ReadError {
    fn from(_: io::Error) -> Self {
        Self::Io
    }
}

fn read_message(
    reader: &mut BufReader<InterruptibleStream>,
) -> std::result::Result<Option<Value>, ReadError> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |i| i + 1);
        if line.len() + count > MAX_MESSAGE_SIZE {
            return Err(ReadError::Message(RpcError::new(
                INVALID_REQUEST,
                "message too large",
            )));
        }
        let first = line.is_empty();
        line.extend_from_slice(&available[..count]);
        reader.consume(count);
        // Authenticated connections may sit idle indefinitely, but a message
        // that has begun must finish within one absolute, bounded deadline.
        if first {
            reader
                .get_ref()
                .set_read_deadline(Some(Instant::now() + IO_TIMEOUT));
        }
        if line.last() == Some(&b'\n') {
            break;
        }
    }
    let value: Value = serde_json::from_slice(&line).map_err(|e| {
        ReadError::Message(RpcError::new(PARSE_ERROR, format!("malformed JSON: {e}")))
    })?;
    if !value.is_object() {
        return Err(ReadError::Message(RpcError::new(
            INVALID_REQUEST,
            "expected a JSON object",
        )));
    }
    Ok(Some(value))
}

fn send(
    stream: &mut InterruptibleStream,
    id: &Value,
    result: super::Result<Value>,
) -> io::Result<()> {
    let value = match result {
        Ok(result) => json!({"jsonrpc":"2.0", "id":id, "result":result}),
        Err(error) => json!({"jsonrpc":"2.0", "id":id, "error":error}),
    };
    let mut line = serde_json::to_vec(&value)?;
    line.push(b'\n');
    stream.set_write_deadline(Some(Instant::now() + IO_TIMEOUT));
    stream.write_all(&line)
}

fn authenticated(received: Option<&str>, expected: &str) -> bool {
    let received = received.unwrap_or("").as_bytes();
    let expected = expected.as_bytes();
    if received.len() != expected.len() {
        return false;
    }
    received
        .iter()
        .zip(expected)
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

fn dispatch(service: &dyn Service, message: &Value) -> super::Result<(Value, bool)> {
    let method = message["method"]
        .as_str()
        .ok_or_else(|| RpcError::new(INVALID_REQUEST, "missing method"))?;
    let empty = Map::new();
    let params = match message.get("params").filter(|v| truthy(v)) {
        Some(value) => value
            .as_object()
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "params must be an object"))?,
        None => &empty,
    };
    if method == "daemon.shutdown" {
        return Ok((json!({"stopping":true}), true));
    }
    service.dispatch(method, params).map(|v| (v, false))
}

fn connection(
    stream: TcpStream,
    service: Arc<dyn Service>,
    token: Arc<str>,
    cancel: Arc<Cancellation>,
    requested: Arc<Cancellation>,
) {
    let Ok(stream) = InterruptibleStream::new(stream, cancel.clone()) else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let mut has_authenticated = false;
    while !cancel.stopped() {
        reader
            .get_ref()
            .set_read_deadline((!has_authenticated).then(|| Instant::now() + IO_TIMEOUT));
        let message = match read_message(&mut reader) {
            Ok(Some(message)) => message,
            Ok(None) | Err(ReadError::Io) => return,
            Err(ReadError::Message(error)) => {
                let _ = send(reader.get_mut(), &Value::Null, Err(error));
                return;
            }
        };
        let id = &message["id"];
        if !authenticated(message["token"].as_str(), &token) {
            if send(
                reader.get_mut(),
                id,
                Err(RpcError::new(UNAUTHORIZED, "invalid or missing token")),
            )
            .is_err()
                || !has_authenticated
            {
                return;
            }
            continue;
        }
        has_authenticated = true;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dispatch(service.as_ref(), &message)
        }))
        .unwrap_or_else(|_| Err(RpcError::new(INTERNAL_ERROR, "unhandled service error")));
        match result {
            Ok((result, shutdown)) => {
                let sent = id.is_null() || send(reader.get_mut(), id, Ok(result)).is_ok();
                // Send the acknowledgement before the owner cancels sockets.
                // Even if the client disappeared, its authenticated stop stands.
                if shutdown {
                    requested.stop();
                    return;
                }
                if !sent {
                    return;
                }
            }
            Err(error) => {
                if send(reader.get_mut(), id, Err(error)).is_err() {
                    return;
                }
            }
        }
    }
}

pub struct RpcServer {
    endpoint: Endpoint,
    cancel: Arc<Cancellation>,
    requested: Arc<Cancellation>,
    worker: Option<JoinHandle<()>>,
}
impl RpcServer {
    /// Bind before starting device discovery. All hardware work belongs to the
    /// service and runs outside the accept loop, including the initial scan.
    pub fn start(service: Arc<dyn Service>, port: u16) -> crate::Result<Self> {
        let listener = LoopbackListener::bind(port)?;
        listener.set_nonblocking(true)?;
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port: listener.local_addr()?.port(),
            token: super::new_token()?,
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").into(),
        };
        let token: Arc<str> = endpoint.token.clone().into();
        let cancel = Arc::new(Cancellation::default());
        let requested = Arc::new(Cancellation::default());
        let (stop, shutdown) = (cancel.clone(), requested.clone());
        let worker = thread::Builder::new()
            .name("openrazer-rpc".into())
            .spawn(move || {
                let mut workers: Vec<JoinHandle<()>> = Vec::new();
                while !stop.stopped() {
                    workers.retain(|worker| !worker.is_finished());
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if workers.len() >= MAX_CONNECTIONS {
                                continue;
                            }
                            let (service, token, stop, requested) = (
                                service.clone(),
                                token.clone(),
                                stop.clone(),
                                shutdown.clone(),
                            );
                            if let Ok(worker) = thread::Builder::new()
                                .name("openrazer-rpc-client".into())
                                .spawn(move || connection(stream, service, token, stop, requested))
                            {
                                workers.push(worker);
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            stop.wait(Duration::from_millis(20));
                        }
                        Err(_) => {
                            shutdown.stop();
                            break;
                        }
                    }
                }
                stop.stop();
                for worker in workers {
                    let _ = worker.join();
                }
                // Keep Winsock alive until every accepted socket has closed.
                drop(listener);
            })?;
        Ok(Self {
            endpoint,
            cancel,
            requested,
            worker: Some(worker),
        })
    }
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
    pub fn stopping(&self) -> bool {
        self.requested.stopped()
    }
    pub fn request_shutdown(&self) {
        self.requested.stop();
    }
    pub fn wait_for_shutdown(&self, timeout: Duration) -> bool {
        self.requested.wait(timeout)
    }
    pub fn listening(&self) -> bool {
        self.worker.as_ref().is_some_and(|w| !w.is_finished())
    }
    pub fn stop(&mut self) {
        self.requested.stop();
        self.cancel.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for RpcServer {
    fn drop(&mut self) {
        self.stop();
    }
}
