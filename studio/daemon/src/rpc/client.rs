use super::{Endpoint, Result, RpcError, INTERNAL_ERROR, MAX_MESSAGE_SIZE};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{IpAddr, SocketAddr, TcpStream},
    path::Path,
    time::Duration,
};

pub struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    endpoint: Endpoint,
    next_id: u64,
}
impl Client {
    pub fn connect(path: &Path) -> Result<Self> {
        let endpoint = Endpoint::read(path)
            .map_err(|e| RpcError::new(INTERNAL_ERROR, format!("daemon unavailable: {e}")))?;
        Self::connect_to(endpoint)
    }
    pub fn connect_to(endpoint: Endpoint) -> Result<Self> {
        let connect = || -> std::io::Result<(BufReader<TcpStream>, TcpStream)> {
            let ip: IpAddr = endpoint.host.parse().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid daemon address")
            })?;
            if !ip.is_loopback() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "daemon address is not loopback",
                ));
            }
            let stream = TcpStream::connect_timeout(
                &SocketAddr::new(ip, endpoint.port),
                Duration::from_secs(2),
            )?;
            stream.set_read_timeout(Some(Duration::from_secs(40)))?;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            stream.set_nodelay(true)?;
            let writer = stream.try_clone()?;
            Ok((BufReader::new(stream), writer))
        };
        let (reader, writer) = connect()
            .map_err(|e| RpcError::new(INTERNAL_ERROR, format!("daemon unavailable: {e}")))?;
        Ok(Self {
            reader,
            writer,
            endpoint,
            next_id: 0,
        })
    }
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let mut bytes=serde_json::to_vec(&json!({"jsonrpc":"2.0","id":self.next_id,"method":method,"params":params,"token":self.endpoint.token})).expect("JSON request");
        bytes.push(b'\n');
        let io_error =
            |e: std::io::Error| RpcError::new(INTERNAL_ERROR, format!("lost the daemon: {e}"));
        self.writer.write_all(&bytes).map_err(io_error)?;
        let mut line = Vec::new();
        self.reader
            .by_ref()
            .take((MAX_MESSAGE_SIZE * 4 + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(io_error)?;
        if line.len() > MAX_MESSAGE_SIZE * 4 || line.last() != Some(&b'\n') {
            return Err(RpcError::new(
                INTERNAL_ERROR,
                "incomplete or oversized daemon reply",
            ));
        }
        let value: Value =
            serde_json::from_slice(&line).map_err(|e| RpcError::new(INTERNAL_ERROR, e))?;
        if value["id"] != json!(self.next_id) {
            return Err(RpcError::new(
                INTERNAL_ERROR,
                "daemon response ID does not match request",
            ));
        }
        if let Some(error) = value.get("error") {
            return Err(RpcError {
                code: error["code"].as_i64().unwrap_or(INTERNAL_ERROR as i64) as i32,
                message: error["message"].as_str().unwrap_or("daemon error").into(),
                data: error.get("data").cloned(),
            });
        }
        Ok(value["result"].clone())
    }
}
