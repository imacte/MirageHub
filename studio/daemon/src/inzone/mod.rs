// SPDX-License-Identifier: GPL-3.0-only
// Based on OpenInzone, Copyright (C) 2026 penguinwokrs.
//! Framed HID and serial transports for Sony INZONE receivers.
pub mod microphone;
#[cfg(windows)]
pub mod native;
use crate::{
    hid::{HidBackend, HidInfo, InputReader},
    protocol::inzone as protocol,
    Error, Result,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub trait Io: Send {
    fn write(&mut self, packet: &[u8]) -> Result<()>;
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>>;
    fn close(&mut self);
}

pub struct HidIo {
    backend: Arc<dyn HidBackend>,
    info: HidInfo,
    reader: Option<Box<dyn InputReader>>,
}
impl HidIo {
    pub fn new(backend: Arc<dyn HidBackend>, info: HidInfo) -> Self {
        Self {
            backend,
            info,
            reader: None,
        }
    }
}
impl Io for HidIo {
    fn write(&mut self, packet: &[u8]) -> Result<()> {
        if self.reader.is_none() {
            self.reader = Some(self.backend.open_input(&self.info)?);
        }
        for report in protocol::hid_reports(packet) {
            self.backend.write_interrupt(&self.info, &report)?;
        }
        Ok(())
    }
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        let reader = self
            .reader
            .as_mut()
            .ok_or_else(|| Error::Device("INZONE input is not open".into()))?;
        Ok(reader
            .read(timeout)?
            .as_deref()
            .and_then(protocol::hid_payload)
            .map(<[u8]>::to_vec))
    }
    fn close(&mut self) {
        self.reader = None;
    }
}

pub struct Session {
    io: Box<dyn Io>,
    reassembler: protocol::Reassembler,
    next: u16,
}
impl Session {
    pub fn new(io: Box<dyn Io>) -> Self {
        Self {
            io,
            reassembler: Default::default(),
            next: 1,
        }
    }
    pub fn request(&mut self, event: u8, value: Option<&[u8]>) -> Result<Vec<u8>> {
        let transaction = self.next;
        self.next = self.next.wrapping_add(1).max(1);
        let result = (|| {
            self.io.write(&protocol::command(
                event,
                value.is_some(),
                transaction,
                value.unwrap_or_default(),
            )?)?;
            let deadline = Instant::now() + Duration::from_millis(1500);
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                if let Some(bytes) = self.io.read(left)? {
                    for packet in self.reassembler.add(&bytes) {
                        if packet.matches(event, transaction, value.is_some()) {
                            return Ok(packet.param);
                        }
                    }
                }
            }
            Err(Error::Device(format!("INZONE did not answer event {event:02x}; check headset power and receiver connection")))
        })();
        if result.is_err() {
            self.close();
        }
        result
    }
    pub fn close(&mut self) {
        self.io.close();
        self.reassembler = Default::default();
    }
}
