use super::{bridge::VERSION, grid, ChromaBridge, ChromaError};
use crate::{
    net::{InterruptibleStream, LoopbackListener},
    worker::Cancellation,
    Result,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader, Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    sync::{Arc, Mutex, MutexGuard},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const PORT: u16 = 54235;
pub const MAX_BODY: usize = 512 * 1024;
const MAX_HEADERS: usize = 32 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: usize = 64;
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub payload: Option<Value>,
}
fn reply(status: u16, payload: Value) -> Response {
    Response {
        status,
        payload: Some(payload),
    }
}
fn invalid(message: impl ToString) -> Response {
    reply(400, json!({"result":87,"error":message.to_string()}))
}
fn ids(body: &Value) -> std::result::Result<Vec<String>, ChromaError> {
    let values = body["ids"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![body["id"].clone()]);
    values
        .into_iter()
        .map(|v| match v {
            Value::String(s) => Ok(s),
            Value::Null => Ok("None".into()),
            Value::Number(n) => Ok(n.to_string()),
            Value::Bool(b) => Ok(if b { "True" } else { "False" }.into()),
            _ => Err(ChromaError::Invalid("effect ids must be strings".into())),
        })
        .collect()
}
fn session_path(path: &str) -> Option<(u64, Option<&str>)> {
    let path = path.strip_suffix('/').unwrap_or(path);
    let parts: Vec<_> = path.split('/').collect();
    if !(parts.len() == 4 || parts.len() == 5)
        || !parts[0].is_empty()
        || parts[1] != "sessions"
        || parts[3] != "chromasdk"
        || parts[2].is_empty()
        || !parts[2].bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let what = parts.get(4).copied();
    if what.is_some_and(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_lowercase())) {
        return None;
    }
    Some((parts[2].parse().ok()?, what))
}
pub fn route(
    bridge: &ChromaBridge,
    base: &str,
    method: &str,
    path: &str,
    body: &Value,
) -> Response {
    if method == "OPTIONS" {
        return Response {
            status: 204,
            payload: None,
        };
    }
    if !body.is_object() {
        return invalid("the request is not a JSON object");
    }
    let path = path.split('?').next().unwrap_or(path);
    if path.trim_end_matches('/') == "/razer/chromasdk" {
        return match method {
            "GET" => reply(
                200,
                json!({"core":VERSION,"device":VERSION,"version":VERSION}),
            ),
            "POST" => {
                let number = bridge.open(body);
                reply(
                    200,
                    json!({"sessionid":number,"uri":format!("{base}/sessions/{number}/chromasdk")}),
                )
            }
            _ => reply(405, json!({"result":87})),
        };
    }
    let Some((number, what)) = session_path(path) else {
        return reply(404, json!({"result":1168,"error":"nothing here"}));
    };
    let response = (|| -> super::Result<Response> {
        match what {
            None if method == "DELETE" => {
                bridge.close(number);
            }
            Some("heartbeat") if matches!(method, "GET" | "PUT" | "POST") => {
                return Ok(reply(200, json!({"tick":bridge.heartbeat(number)?})))
            }
            Some(kind) if grid::dimensions(kind).is_some() && method == "PUT" => {
                bridge.show(number, kind, body)?
            }
            Some(kind) if grid::dimensions(kind).is_some() && method == "POST" => {
                return Ok(reply(
                    200,
                    json!({"id":bridge.create(number,kind,body)?,"result":0}),
                ))
            }
            Some("effect") if method == "PUT" => bridge.apply(number, &ids(body)?)?,
            Some("effect") if method == "DELETE" => bridge.delete(number, &ids(body)?)?,
            _ => return Ok(reply(405, json!({"result":87}))),
        }
        Ok(reply(200, json!({"result":0})))
    })();
    match response {
        Ok(reply) => reply,
        Err(ChromaError::NoSession) => reply(404, json!({"result":1168,"error":"no such session"})),
        Err(ChromaError::Invalid(message)) => invalid(message),
        Err(ChromaError::Io(message)) => reply(500, json!({"result":87,"error":message})),
    }
}

fn timeout(
    reader: &BufReader<InterruptibleStream>,
    deadline: Instant,
    cancel: &Cancellation,
) -> io::Result<()> {
    if cancel.stopped() {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "server stopped"));
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "request timed out"));
    }
    reader.get_ref().set_read_deadline(Some(deadline));
    Ok(())
}
fn timed_out(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}
fn line(
    reader: &mut BufReader<InterruptibleStream>,
    limit: usize,
    deadline: Instant,
    cancel: &Cancellation,
) -> io::Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        timeout(reader, deadline, cancel)?;
        let available = match reader.fill_buf() {
            Ok(bytes) => bytes,
            Err(error) if timed_out(&error) => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete HTTP line",
                ))
            };
        }
        let end = available.iter().position(|b| *b == b'\n').map(|p| p + 1);
        let count = end.unwrap_or(available.len());
        if count > limit.saturating_sub(bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request headers are too large",
            ));
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if end.is_some() {
            break;
        }
    }
    if !bytes.ends_with(b"\r\n") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid HTTP line ending",
        ));
    }
    bytes.truncate(bytes.len() - 2);
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid HTTP header"))
}
struct Request {
    method: String,
    path: String,
    body: Value,
    close: bool,
}
fn request(
    reader: &mut BufReader<InterruptibleStream>,
    cancel: &Cancellation,
) -> io::Result<Option<Request>> {
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let Some(first) = line(reader, 8192, deadline, cancel)? else {
        return Ok(None);
    };
    let parts: Vec<_> = first.split_whitespace().collect();
    if parts.len() != 3 || !matches!(parts[2], "HTTP/1.1" | "HTTP/1.0") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid HTTP request",
        ));
    }
    let (method, path) = (parts[0].to_owned(), parts[1].to_owned());
    let mut close = parts[2] == "HTTP/1.0";
    let mut length = None;
    let mut used = first.len() + 2;
    let mut expect = false;
    loop {
        let header = line(reader, MAX_HEADERS - used, deadline, cancel)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "incomplete HTTP headers")
        })?;
        used += header.len() + 2;
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid HTTP header",
            ));
        };
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid HTTP header name",
            ));
        }
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "content-length" => {
                if length.is_some()
                    || value.is_empty()
                    || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid Content-Length",
                    ));
                }
                let count = value.parse::<usize>().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length")
                })?;
                if count > MAX_BODY {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "the request is too large",
                    ));
                }
                length = Some(count);
            }
            "transfer-encoding" => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Transfer-Encoding is not supported",
                ))
            }
            "connection" => {
                let tokens: Vec<_> = value
                    .split(',')
                    .map(|s| s.trim().to_ascii_lowercase())
                    .collect();
                if tokens.iter().any(|s| s == "close") {
                    close = true;
                } else if tokens.iter().any(|s| s == "keep-alive") {
                    close = false;
                }
            }
            "expect" => {
                if !value.eq_ignore_ascii_case("100-continue") {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unsupported Expect header",
                    ));
                }
                expect = true;
            }
            _ => (),
        }
    }
    if expect {
        reader
            .get_ref()
            .set_write_deadline(Some(Instant::now() + REQUEST_TIMEOUT));
        reader
            .get_mut()
            .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let mut bytes = vec![0; length.unwrap_or(0)];
    let mut offset = 0;
    while offset < bytes.len() {
        timeout(reader, deadline, cancel)?;
        let count = match reader.read(&mut bytes[offset..]) {
            Ok(count) => count,
            Err(error) if timed_out(&error) => continue,
            Err(error) => return Err(error),
        };
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete request body",
            ));
        }
        offset += count;
    }
    let body = if bytes.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        serde_json::from_slice(&bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the request is not JSON"))?
    };
    Ok(Some(Request {
        method,
        path,
        body,
        close,
    }))
}
fn write_response(
    stream: &mut InterruptibleStream,
    response: &Response,
    close: bool,
) -> io::Result<()> {
    stream.set_write_deadline(Some(Instant::now() + REQUEST_TIMEOUT));
    let body = response
        .payload
        .as_ref()
        .map(serde_json::to_vec)
        .transpose()?
        .unwrap_or_default();
    let reason = match response.status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        _ => "Internal Server Error",
    };
    let headers=format!("HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: {}\r\n\r\n",response.status,body.len(),if close{"close"}else{"keep-alive"});
    stream.write_all(headers.as_bytes())?;
    stream.write_all(&body)
}
fn connection(
    stream: TcpStream,
    bridge: Arc<ChromaBridge>,
    base: String,
    cancel: Arc<Cancellation>,
) {
    let Ok(stream) = InterruptibleStream::new(stream, cancel.clone()) else {
        return;
    };
    let mut reader = BufReader::new(stream);
    while !cancel.stopped() {
        match request(&mut reader, &cancel) {
            Ok(Some(request)) => {
                let response = route(
                    &bridge,
                    &base,
                    &request.method,
                    &request.path,
                    &request.body,
                );
                if write_response(reader.get_mut(), &response, request.close).is_err()
                    || request.close
                {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                if error.kind() == io::ErrorKind::InvalidData {
                    let _ = write_response(reader.get_mut(), &invalid(error), true);
                } else if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) {
                    let _ = write_response(
                        reader.get_mut(),
                        &reply(408, json!({"result":87,"error":"request timed out"})),
                        true,
                    );
                }
                break;
            }
        }
    }
}

pub struct ChromaServer {
    address: SocketAddr,
    cancel: Arc<Cancellation>,
    connections: Arc<Mutex<BTreeMap<u64, TcpStream>>>,
    worker: Option<JoinHandle<()>>,
}
impl ChromaServer {
    pub fn start(bridge: Arc<ChromaBridge>, port: u16) -> Result<Self> {
        let listener = LoopbackListener::bind(port)?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let cancel = Arc::new(Cancellation::default());
        let connections = Arc::new(Mutex::new(BTreeMap::<u64, TcpStream>::new()));
        let (stop, clients) = (cancel.clone(), connections.clone());
        let base = format!("http://localhost:{}", address.port());
        let worker = thread::Builder::new()
            .name("openrazer-chroma-rest".into())
            .spawn(move || {
                let mut workers: Vec<JoinHandle<()>> = Vec::new();
                let mut number = 0u64;
                while !stop.stopped() {
                    workers.retain(|w| !w.is_finished());
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if lock(&clients).len() >= MAX_CONNECTIONS {
                                continue;
                            }
                            let Ok(control) = stream.try_clone() else {
                                continue;
                            };
                            number += 1;
                            let id = number;
                            {
                                let mut clients = lock(&clients);
                                if stop.stopped() {
                                    break;
                                }
                                clients.insert(id, control);
                            }
                            let (bridge, base, stop, client_registry) =
                                (bridge.clone(), base.clone(), stop.clone(), clients.clone());
                            match thread::Builder::new()
                                .name("openrazer-chroma-client".into())
                                .spawn(move || {
                                    connection(stream, bridge, base, stop);
                                    lock(&client_registry).remove(&id);
                                }) {
                                Ok(worker) => workers.push(worker),
                                Err(_) => {
                                    lock(&clients).remove(&id);
                                }
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            stop.wait(Duration::from_millis(20));
                        }
                        Err(_) => break,
                    }
                }
                for stream in lock(&clients).values() {
                    let _ = stream.shutdown(Shutdown::Both);
                }
                for worker in workers {
                    let _ = worker.join();
                }
                // Keep WSA initialized until every accepted socket has closed.
                drop(listener);
            })?;
        Ok(Self {
            address,
            cancel,
            connections,
            worker: Some(worker),
        })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn listening(&self) -> bool {
        self.worker.as_ref().is_some_and(|w| !w.is_finished())
    }
    pub fn stop(&mut self) {
        self.cancel.stop();
        for stream in lock(&self.connections).values() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for ChromaServer {
    fn drop(&mut self) {
        self.stop();
    }
}
