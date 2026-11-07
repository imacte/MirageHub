//! Studio's existing JSON-RPC interface. A token is checked on every message;
//! the endpoint is published with owner-only access before hardware discovery.
mod client;
mod endpoint;
mod instance;
mod server;
pub use client::Client;
pub use endpoint::{new_token, Endpoint, PublishedEndpoint};
pub use instance::SingleInstance;
pub use server::{RpcServer, Service};

use serde::Serialize;
use serde_json::Value;
use std::fmt;

pub const MAX_MESSAGE_SIZE: usize = 1 << 20;
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;
pub const UNAUTHORIZED: i32 = -32000;
pub const DEVICE_NOT_FOUND: i32 = -32001;
pub const NOT_SUPPORTED: i32 = -32002;
pub const DEVICE_ERROR: i32 = -32003;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}
impl RpcError {
    pub fn new(code: i32, message: impl ToString) -> Self {
        Self {
            code,
            message: message.to_string(),
            data: None,
        }
    }
    pub fn invalid(message: impl ToString) -> Self {
        Self::new(INVALID_PARAMS, message)
    }
}
impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RpcError {}
impl From<crate::Error> for RpcError {
    fn from(error: crate::Error) -> Self {
        let code = match &error {
            crate::Error::Invalid(_) => INVALID_PARAMS,
            crate::Error::NotSupported(_) => NOT_SUPPORTED,
            crate::Error::Device(_) => DEVICE_ERROR,
            crate::Error::Io(_) => INTERNAL_ERROR,
        };
        Self::new(code, error)
    }
}
pub type Result<T> = std::result::Result<T, RpcError>;

pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
