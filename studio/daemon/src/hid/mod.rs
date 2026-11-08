//! Owned HID handles and bounded input reads. The device layer does not depend
//! on Windows, allowing the same operations to run against a recording backend.

#[cfg(windows)]
pub mod windows;
#[cfg(any(windows, test))]
mod cache;

use crate::Result;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HidInfo {
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub interface: Option<u8>,
    pub collection: Option<u8>,
    pub usage_page: u16,
    pub usage: u16,
    pub feature_length: u16,
    pub input_length: u16,
    pub output_length: u16,
    pub version: u16,
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
}

impl HidInfo {
    pub fn path_index(path: &str, marker: &str) -> Option<u8> {
        let path = path.to_ascii_lowercase();
        let position = path.find(marker)? + marker.len();
        u8::from_str_radix(path.get(position..position + 2)?, 16).ok()
    }
}

pub trait HidHandle: Send {
    fn send_feature(&mut self, data: &[u8]) -> Result<()>;
    fn get_feature(&mut self, length: usize, report_id: u8) -> Result<Vec<u8>>;
    fn send_output(&mut self, data: &[u8]) -> Result<()>;
}

pub trait InputReader: Send {
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>>;
}

pub trait HidBackend: Send + Sync {
    fn enumerate(&self, vendor_id: Option<u16>, product_id: Option<u16>) -> Result<Vec<HidInfo>>;
    fn open(&self, info: &HidInfo) -> Result<Box<dyn HidHandle>>;
    fn open_input(&self, info: &HidInfo) -> Result<Box<dyn InputReader>>;
}
