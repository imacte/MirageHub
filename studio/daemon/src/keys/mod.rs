//! Macro bindings, vendor reports, Fn translation and native Windows playback.
pub mod codes;
pub mod inject;
pub mod macros;
pub mod service;

pub use macros::MacroStore;
pub use service::KeyService;
