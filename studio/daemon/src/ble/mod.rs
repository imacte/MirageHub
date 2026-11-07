//! Bluetooth links live on an owning worker thread. Requests, animation frames,
//! colour refreshes and disconnects are serialized there, including WinRT objects.

#[cfg(windows)]
pub mod windows;

use crate::{protocol::ble as protocol, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{mpsc, Arc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);
pub const SCAN_SECONDS: Duration = Duration::from_secs(6);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advertiser {
    pub address: u64,
    pub name: String,
    pub rssi: i16,
    pub product_id: Option<u16>,
    pub classic_address: Option<u64>,
}

/// Advertisements and scan responses carry different pieces of one identity.
#[derive(Default)]
pub struct Advertisements(BTreeMap<u64, (Advertiser, BTreeSet<u16>)>);
impl Advertisements {
    pub fn add(&mut self, address: u64, name: &str, rssi: i16, sections: &[(u16, Vec<u8>)]) {
        let (entry, companies) = self.0.entry(address).or_insert_with(|| {
            (
                Advertiser {
                    address,
                    ..Default::default()
                },
                BTreeSet::new(),
            )
        });
        entry.rssi = rssi;
        if !name.is_empty() {
            entry.name = name.into();
        }
        for (company, payload) in sections {
            companies.insert(*company);
            if *company == protocol::COMPANY_ID {
                entry.product_id = protocol::product_id(payload).or(entry.product_id);
                entry.classic_address =
                    protocol::classic_address(payload).or(entry.classic_address);
            }
        }
    }
    pub fn finish(&self, company: Option<u16>) -> Vec<Advertiser> {
        let mut result: Vec<_> = self
            .0
            .values()
            .filter(|(_, companies)| company.is_none_or(|id| companies.contains(&id)))
            .map(|(entry, _)| entry.clone())
            .collect();
        result.sort_by_key(|entry| std::cmp::Reverse(entry.rssi));
        result
    }
}

pub trait BleChannel: Send {
    fn address(&self) -> u64;
    fn write(&mut self, payload: &[u8], hold: bool) -> Result<()>;
    fn request(&mut self, opcode: u8, payload: &[u8]) -> Result<Vec<u8>>;
    fn release(&mut self);
    fn is_connected(&self) -> bool;
    fn describe(&mut self) -> Result<Value>;
    fn close(&mut self);
}

/// These objects are constructed and destroyed on the transport's worker.
/// No Send bound is needed on a link; platform handles never leave that thread.
pub trait GattLink {
    fn write(&mut self, payload: &[u8], deadline: Instant) -> Result<()>;
    fn subscribe(&mut self, deadline: Instant) -> Result<()>;
    fn discard_notifications(&mut self);
    fn notification(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>>;
    fn is_connected(&self) -> bool;
    fn describe(&self) -> Result<Value>;
}
pub trait BleBackend: Send + Sync {
    fn scan(&self, duration: Duration, company: Option<u16>) -> Result<Vec<Advertiser>>;
    fn connect(&self, address: u64, uncached: bool, deadline: Instant)
        -> Result<Box<dyn GattLink>>;
}

#[derive(Clone, Copy)]
pub struct Timing {
    pub keepalive: Duration,
    pub idle_disconnect: Duration,
    pub resolve_backoff: Duration,
    pub reply_timeout: Duration,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            keepalive: Duration::from_secs(2),
            idle_disconnect: Duration::from_secs(5),
            resolve_backoff: Duration::from_millis(400),
            reply_timeout: Duration::from_millis(1500),
        }
    }
}

enum Operation {
    Write(Vec<u8>, bool),
    Request(u8, Vec<u8>),
    Release,
    Connected,
    Describe,
    Close,
}
struct Command {
    operation: Operation,
    deadline: Instant,
    reply: mpsc::SyncSender<Result<Value>>,
}

pub struct BleTransport {
    address: u64,
    commands: Option<mpsc::Sender<Command>>,
    worker: Option<JoinHandle<()>>,
}
impl BleTransport {
    pub fn new(address: u64, backend: Arc<dyn BleBackend>) -> Result<Self> {
        Self::with_timing(address, backend, Timing::default())
    }
    pub fn with_timing(address: u64, backend: Arc<dyn BleBackend>, timing: Timing) -> Result<Self> {
        if timing.keepalive.is_zero()
            || timing.idle_disconnect.is_zero()
            || timing.reply_timeout.is_zero()
        {
            return Err(Error::Invalid("Bluetooth timeouts must be positive".into()));
        }
        let (sender, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name(format!("openrazer-ble-{address:012x}"))
            .spawn(move || Worker::new(address, backend, timing).run(receiver))?;
        Ok(Self {
            address,
            commands: Some(sender),
            worker: Some(worker),
        })
    }
    fn execute(&self, operation: Operation) -> Result<Value> {
        let deadline = Instant::now() + CALL_TIMEOUT;
        let (reply, receive) = mpsc::sync_channel(1);
        self.commands
            .as_ref()
            .ok_or_else(|| Error::Device("Bluetooth worker closed".into()))?
            .send(Command {
                operation,
                deadline,
                reply,
            })
            .map_err(|_| Error::Device("Bluetooth worker stopped".into()))?;
        receive
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| Error::Device("Bluetooth did not answer within 30 seconds".into()))?
    }
}
impl BleChannel for BleTransport {
    fn address(&self) -> u64 {
        self.address
    }
    fn write(&mut self, payload: &[u8], hold: bool) -> Result<()> {
        self.execute(Operation::Write(payload.to_vec(), hold))
            .map(|_| ())
    }
    fn request(&mut self, opcode: u8, payload: &[u8]) -> Result<Vec<u8>> {
        serde_json::from_value(self.execute(Operation::Request(opcode, payload.to_vec()))?)
            .map_err(Into::into)
    }
    fn release(&mut self) {
        let _ = self.execute(Operation::Release);
    }
    fn is_connected(&self) -> bool {
        self.execute(Operation::Connected)
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }
    fn describe(&mut self) -> Result<Value> {
        self.execute(Operation::Describe)
    }
    fn close(&mut self) {
        let _ = self.execute(Operation::Close);
    }
}
impl Drop for BleTransport {
    fn drop(&mut self) {
        self.commands.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Worker {
    address: u64,
    backend: Arc<dyn BleBackend>,
    timing: Timing,
    link: Option<Box<dyn GattLink>>,
    held: Option<Vec<u8>>,
    last_used: Instant,
}
impl Worker {
    fn new(address: u64, backend: Arc<dyn BleBackend>, timing: Timing) -> Self {
        Self {
            address,
            backend,
            timing,
            link: None,
            held: None,
            last_used: Instant::now(),
        }
    }
    fn resolve(&mut self, deadline: Instant) -> Result<&mut Box<dyn GattLink>> {
        if self.link.is_none() {
            let mut error = Error::Device("Bluetooth connection failed".into());
            for attempt in 0..3 {
                remaining(deadline)?;
                match self.backend.connect(self.address, attempt > 0, deadline) {
                    Ok(link) => {
                        self.link = Some(link);
                        break;
                    }
                    Err(e) => error = e,
                }
                if attempt < 2 {
                    thread::sleep(self.timing.resolve_backoff.min(remaining(deadline)?));
                }
            }
            if self.link.is_none() {
                return Err(error);
            }
        }
        Ok(self.link.as_mut().expect("resolved link"))
    }
    fn write(&mut self, bytes: &[u8], deadline: Instant) -> Result<()> {
        remaining(deadline)?;
        if self
            .resolve(deadline)
            .and_then(|link| link.write(bytes, deadline))
            .is_err()
        {
            self.link.take();
            remaining(deadline)?;
            self.resolve(deadline)?.write(bytes, deadline)?;
        }
        Ok(())
    }
    fn request(&mut self, opcode: u8, bytes: &[u8], deadline: Instant) -> Result<Value> {
        let request = protocol::message(opcode, bytes, 0)?;
        for _ in 0..3 {
            let timeout = self.timing.reply_timeout;
            let link = self.resolve(deadline)?;
            link.subscribe(deadline)?; // Must precede the first request.
            link.discard_notifications();
            remaining(deadline)?;
            link.write(&request, deadline)?;
            let reply_deadline = deadline.min(Instant::now() + timeout);
            while Instant::now() < reply_deadline {
                let Some(packet) = link.notification(reply_deadline)? else {
                    break;
                };
                if let Ok((received, _, body)) = protocol::parse_response(&packet) {
                    if received == opcode {
                        return Ok(serde_json::json!(body));
                    }
                }
            }
        }
        Err(Error::Device(format!(
            "device {:012X} did not answer 0x{opcode:02x} in 3 attempts",
            self.address
        )))
    }
    fn run(mut self, receiver: mpsc::Receiver<Command>) {
        loop {
            let command = if self.link.is_some() {
                let interval = if self.held.is_some() {
                    self.timing.keepalive
                } else {
                    self.timing.idle_disconnect
                };
                match receiver.recv_timeout(interval.saturating_sub(self.last_used.elapsed())) {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Some(bytes) = self.held.clone() {
                            if self.write(&bytes, Instant::now() + CALL_TIMEOUT).is_err() {
                                self.link.take();
                            }
                            self.last_used = Instant::now();
                        } else {
                            self.link.take();
                        }
                        continue;
                    }
                }
            } else {
                let Ok(command) = receiver.recv() else { break };
                command
            };
            let result = remaining(command.deadline)
                .and_then(|_| self.perform(command.operation, command.deadline));
            let _ = command.reply.send(result);
        }
    }
    fn perform(&mut self, operation: Operation, deadline: Instant) -> Result<Value> {
        match operation {
            Operation::Write(bytes, hold) => {
                self.write(&bytes, deadline)?;
                if hold {
                    self.held = Some(bytes);
                }
                self.last_used = Instant::now();
            }
            Operation::Request(opcode, bytes) => {
                let value = self.request(opcode, &bytes, deadline)?;
                self.last_used = Instant::now();
                return Ok(value);
            }
            Operation::Release => self.held = None,
            Operation::Connected => {
                return Ok(Value::Bool(
                    self.link.as_ref().is_some_and(|link| link.is_connected()),
                ))
            }
            Operation::Describe => return self.resolve(deadline)?.describe(),
            Operation::Close => {
                self.held = None;
                self.link.take();
            }
        }
        Ok(Value::Null)
    }
}

pub(crate) fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|time| !time.is_zero())
        .ok_or_else(|| Error::Device("Bluetooth operation timed out".into()))
}
