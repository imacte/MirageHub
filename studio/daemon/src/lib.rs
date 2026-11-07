//! The native device backend. Protocol and device data are shared with the
//! existing port, so migrating the service does not change hardware commands.

pub mod ble;
pub mod database;
pub mod device;
pub mod effects;
pub mod hid;
pub mod integrations;
pub mod keyboard;
pub mod keys;
pub mod manager;
pub mod persistence;
pub mod profiles;
pub mod protocol;
pub mod recipes;
pub mod transport;
mod worker;

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Invalid(String),
    NotSupported(String),
    Device(String),
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message)
            | Self::NotSupported(message)
            | Self::Device(message)
            | Self::Io(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Invalid(error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
