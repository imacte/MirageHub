//! Chroma SDK REST sessions and conversion to native device frames.
pub mod bridge;
pub mod grid;
pub mod rest;

pub use bridge::ChromaBridge;
pub use rest::ChromaServer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChromaError {
    NoSession,
    Invalid(String),
    Io(String),
}
impl std::fmt::Display for ChromaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSession => f.write_str("no such session"),
            Self::Invalid(s) | Self::Io(s) => f.write_str(s),
        }
    }
}
impl std::error::Error for ChromaError {}
impl From<crate::Error> for ChromaError {
    fn from(error: crate::Error) -> Self {
        Self::Invalid(error.to_string())
    }
}
pub type Result<T> = std::result::Result<T, ChromaError>;
