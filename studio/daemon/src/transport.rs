//! Serialized device transfers and collection selection. The owner holds one
//! mutable transport for an entire recipe, so requests cannot interleave.

use crate::{
    hid::{HidBackend, HidHandle, HidInfo, InputReader},
    protocol::{
        blackshark,
        report::{status_name, Report, BUSY, NOT_SUPPORTED, SUCCESSFUL},
    },
    recipes::Sender,
    Error, Result,
};
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

pub fn select_control(candidates: &[HidInfo], preferred: Option<u8>) -> Option<&HidInfo> {
    candidates
        .iter()
        .filter(|i| i.feature_length == 91)
        .min_by_key(|i| {
            (
                preferred.is_some() && i.interface != preferred,
                i.interface.unwrap_or(0),
                i.collection.unwrap_or(0),
            )
        })
}
pub fn select_kraken(candidates: &[HidInfo]) -> Option<&HidInfo> {
    candidates
        .iter()
        .filter(|i| i.output_length == 37)
        .min_by_key(|i| {
            (
                i.interface != Some(3),
                i.interface.unwrap_or(0),
                i.collection.unwrap_or(0),
            )
        })
}
pub fn select_blackshark(candidates: &[HidInfo]) -> Option<&HidInfo> {
    candidates
        .iter()
        .filter(|i| i.usage_page >= 0xff00 && i.output_length >= 64 && i.input_length >= 64)
        .min_by_key(|i| {
            (
                match i.usage_page {
                    blackshark::USAGE_PAGE => 0,
                    blackshark::OTHER_USAGE_PAGE => 1,
                    _ => 2,
                },
                i.interface != Some(3),
                i.collection.unwrap_or(0),
            )
        })
}

pub trait DeviceTransport: Sender + Send {
    fn info(&self) -> &HidInfo;
    fn wait(&self) -> Duration;
    fn argb_info(&self) -> Option<&HidInfo> {
        None
    }
    fn kraken_info(&self) -> Option<&HidInfo> {
        None
    }
    fn send_kraken(&mut self, _reports: &[[u8; 37]], _settle: Duration) -> Result<()> {
        Err(Error::NotSupported(
            "device has no Kraken control interface".into(),
        ))
    }
    fn close(&mut self);
}

pub struct Transport {
    backend: Arc<dyn HidBackend>,
    pub info: HidInfo,
    pub argb: Option<HidInfo>,
    pub kraken: Option<HidInfo>,
    wait: Duration,
    handle: Option<Box<dyn HidHandle>>,
    argb_handle: Option<Box<dyn HidHandle>>,
    kraken_handle: Option<Box<dyn HidHandle>>,
}

impl Transport {
    pub fn new(
        backend: Arc<dyn HidBackend>,
        info: HidInfo,
        wait_us: u64,
        argb: Option<HidInfo>,
        kraken: Option<HidInfo>,
    ) -> Self {
        Self {
            backend,
            info,
            argb,
            kraken,
            wait: Duration::from_micros(wait_us.max(1000)),
            handle: None,
            argb_handle: None,
            kraken_handle: None,
        }
    }
}

impl Sender for Transport {
    fn send(&mut self, request: &Report, want_response: bool) -> Result<Option<Report>> {
        if self.handle.is_none() {
            self.handle = Some(self.backend.open(&self.info)?);
        }
        let handle = self.handle.as_mut().expect("opened above");
        let mut data = Vec::with_capacity(91);
        data.push(0);
        data.extend_from_slice(&request.pack());
        let mut last_error = Error::Device("no usable response from device".into());
        for attempt in 0..5 {
            let transfer = (|| {
                handle.send_feature(&data)?;
                thread::sleep(self.wait);
                if !want_response {
                    return Ok(None);
                }
                let raw = handle.get_feature(91, 0)?;
                let response = Report::unpack(raw.get(1..).unwrap_or(&[]))?;
                if !request.matches(&response) {
                    return Err(Error::Device("response does not match request".into()));
                }
                match response.status {
                    SUCCESSFUL | BUSY => Ok(Some(response)),
                    NOT_SUPPORTED => {
                        Err(Error::NotSupported("device replied not supported".into()))
                    }
                    status => Err(Error::Device(format!(
                        "device replied {}",
                        status_name(status)
                    ))),
                }
            })();
            match transfer {
                Ok(response) => return Ok(response),
                Err(error @ Error::NotSupported(_)) => return Err(error),
                Err(error) => last_error = error,
            }
            if attempt < 4 {
                thread::sleep(Duration::from_millis(10));
            }
        }
        Err(last_error)
    }
    fn send_argb(&mut self, channel: u8, colours: &[u8]) -> Result<()> {
        let info = self
            .argb
            .as_ref()
            .ok_or_else(|| Error::NotSupported("device has no ARGB interface".into()))?;
        if colours.len() > 315 || !colours.len().is_multiple_of(3) {
            return Err(Error::Invalid(
                "ARGB frame exceeds 105 RGB colours or has incomplete triples".into(),
            ));
        }
        if self.argb_handle.is_none() {
            self.argb_handle = Some(self.backend.open(info)?);
        }
        let mut report = [0; 321];
        report[..6].copy_from_slice(&[
            0x5b,
            if channel < 5 { 4 } else { 0x84 },
            channel,
            channel,
            0,
            (colours.len() / 3).saturating_sub(1) as u8,
        ]);
        report[6..6 + colours.len()].copy_from_slice(colours);
        self.argb_handle
            .as_mut()
            .expect("opened above")
            .send_feature(&report)
    }
}

impl DeviceTransport for Transport {
    fn info(&self) -> &HidInfo {
        &self.info
    }
    fn wait(&self) -> Duration {
        self.wait
    }
    fn argb_info(&self) -> Option<&HidInfo> {
        self.argb.as_ref()
    }
    fn kraken_info(&self) -> Option<&HidInfo> {
        self.kraken.as_ref()
    }
    fn send_kraken(&mut self, reports: &[[u8; 37]], settle: Duration) -> Result<()> {
        let info = self
            .kraken
            .as_ref()
            .ok_or_else(|| Error::NotSupported("device has no Kraken control interface".into()))?;
        if self.kraken_handle.is_none() {
            self.kraken_handle = Some(self.backend.open(info)?);
        }
        for report in reports {
            self.kraken_handle
                .as_mut()
                .expect("opened above")
                .send_output(report)?;
            thread::sleep(settle);
        }
        Ok(())
    }
    fn close(&mut self) {
        self.handle = None;
        self.argb_handle = None;
        self.kraken_handle = None;
    }
}

pub struct ReportChannel {
    backend: Arc<dyn HidBackend>,
    pub info: HidInfo,
    pub timeout: Duration,
    writer: Option<Box<dyn HidHandle>>,
    reader: Option<Box<dyn InputReader>>,
}
pub trait CommandChannel: Send {
    fn info(&self) -> &HidInfo;
    fn exchange(
        &mut self,
        request: &[u8],
        accept: &dyn Fn(&[u8]) -> bool,
    ) -> Result<Option<Vec<u8>>>;
    fn close(&mut self);
}
impl CommandChannel for ReportChannel {
    fn info(&self) -> &HidInfo {
        &self.info
    }
    fn exchange(
        &mut self,
        request: &[u8],
        accept: &dyn Fn(&[u8]) -> bool,
    ) -> Result<Option<Vec<u8>>> {
        ReportChannel::exchange(self, request, accept)
    }
    fn close(&mut self) {
        ReportChannel::close(self);
    }
}
impl ReportChannel {
    pub fn new(backend: Arc<dyn HidBackend>, info: HidInfo) -> Self {
        Self {
            backend,
            info,
            timeout: Duration::from_millis(800),
            writer: None,
            reader: None,
        }
    }
    pub fn exchange(
        &mut self,
        request: &[u8],
        accept: &dyn Fn(&[u8]) -> bool,
    ) -> Result<Option<Vec<u8>>> {
        let result = (|| {
            // Open input first: Windows only queues notifications for handles
            // that already exist when the request is sent.
            if self.reader.is_none() {
                self.reader = Some(self.backend.open_input(&self.info)?);
            }
            if self.writer.is_none() {
                self.writer = Some(self.backend.open(&self.info)?);
            }
            let mut data = request.to_vec();
            data.resize(data.len().max(usize::from(self.info.output_length)), 0);
            self.writer
                .as_mut()
                .expect("opened above")
                .send_output(&data)?;
            let deadline = Instant::now() + self.timeout;
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                if let Some(answer) = self.reader.as_mut().expect("opened above").read(left)? {
                    if accept(&answer) {
                        return Ok(Some(answer));
                    }
                }
            }
            Ok(None)
        })();
        if result.is_err() {
            self.close();
        }
        result
    }
    pub fn close(&mut self) {
        self.writer = None;
        self.reader = None;
    }
}
