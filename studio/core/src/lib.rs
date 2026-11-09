//! What Mirage Hub and its installer have in common.
//!
//! The installer has to stop the daemon and the app before it can replace
//! them, knows where they live and how they start with Windows, and looks like
//! the app it installs; so all of that lives here rather than twice.

pub mod i18n;
pub mod connection;
pub mod install;
pub mod paths;
#[path = "../../../assets/data_paths.rs"]
pub mod data_paths;
pub mod process;
pub mod rpc;
mod daemon_connect;
pub mod theme;
pub mod widgets;
pub mod chrome;
#[cfg(windows)]
pub mod window;

pub use i18n::{t, tf};
#[path = "../../../assets/branding.rs"]
pub mod branding;
