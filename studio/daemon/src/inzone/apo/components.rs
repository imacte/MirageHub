// SPDX-License-Identifier: GPL-3.0-only
//! Optional Sony components, independent of the application updater and shell scripts.
pub mod package;
#[cfg(windows)]
mod windows;
use crate::{Error, Result};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

pub const VERSION: &str = "1.0.20.0";
pub const MANIFEST: &str = include_str!("../../../../../assets/sony-audio-manifest.json");
pub fn directory() -> PathBuf {
    crate::data_paths::data_dir()
        .join("sony-audio")
        .join(VERSION)
}
#[derive(Clone, Default)]
struct Job {
    phase: &'static str,
    error: String,
}
fn job() -> &'static Mutex<Job> {
    static JOB: OnceLock<Mutex<Job>> = OnceLock::new();
    JOB.get_or_init(|| Mutex::new(Job::default()))
}
#[cfg(windows)]
fn phase(value: &'static str) {
    job().lock().unwrap_or_else(|p| p.into_inner()).phase = value;
}
pub fn state() -> Value {
    let state = job().lock().unwrap_or_else(|p| p.into_inner()).clone();
    json!({"version":VERSION,"phase":if state.phase.is_empty() {if directory().join("files/ready.json").exists() {"ready"} else {"absent"}} else {state.phase},"error":state.error})
}
#[cfg(windows)]
pub(super) use package::verify_file;
#[cfg(windows)]
pub(super) fn resource_dir() -> Result<PathBuf> {
    let mut candidates = vec![windows::resource_directory()?];
    if let Some(base) = std::env::var_os("PROGRAMDATA") {
        candidates.push(PathBuf::from(base).join("Sony/INZONE Hub/VirtualizeStandard"));
    }
    for folder in candidates {
        if [
            "downmix.hki",
            "standard_hrtf.hki",
            "WH-G500_standard.ba",
            "alc.cfg",
        ]
        .iter()
        .all(|name| verify_file(&folder, name).is_ok())
        {
            return Ok(folder);
        }
    }
    Err(Error::NotSupported(
        "Prepare Sony audio components to use standard spatial sound".into(),
    ))
}
/// Also used by the development probe; installation remains a separate explicit action.
pub fn prepare(root: &std::path::Path) -> Result<()> {
    #[cfg(windows)]
    {
        package::prepare(root, windows::verify_signature, |value| {
            if value != "ready" {
                phase(value);
            }
        })?;
        windows::publish_resources(root)?;
        phase("ready");
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        Err(Error::NotSupported(
            "Sony Authenticode verification requires Windows".into(),
        ))
    }
}
pub fn start(install: bool) -> Result<()> {
    #[cfg(windows)]
    {
        let mut current = job().lock().unwrap_or_else(|p| p.into_inner());
        if matches!(
            current.phase,
            "starting" | "downloading" | "verifying" | "extracting" | "installing"
        ) {
            return Err(Error::Invalid(
                "Sony component operation is already running".into(),
            ));
        }
        current.phase = if install { "installing" } else { "starting" };
        current.error.clear();
        std::thread::spawn(move || {
            let result = if install {
                windows::elevate(&directory()).map(|restart| {
                    phase(if restart {
                        "restart_required"
                    } else {
                        "installed"
                    })
                })
            } else {
                prepare(&directory())
            };
            if let Err(e) = result {
                let mut state = job().lock().unwrap_or_else(|p| p.into_inner());
                state.phase = "failed";
                state.error = e.to_string();
            }
        });
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = install;
        Err(Error::NotSupported("Sony drivers require Windows".into()))
    }
}
/// Called only by the daemon's dedicated UAC entry point; never through device RPC.
pub fn install_elevated(root: &std::path::Path) -> Result<i32> {
    #[cfg(windows)]
    {
        windows::install(root)
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        Err(Error::NotSupported("Sony drivers require Windows".into()))
    }
}
