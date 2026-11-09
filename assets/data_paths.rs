//! Product data and the one-time, non-destructive RsRazer settings migration.
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};
pub fn base_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        })
}
pub fn data_dir() -> PathBuf {
    base_dir().join("mirage-hub")
}
pub fn migrate_rsrazer() -> io::Result<()> {
    migrate(&base_dir().join("rsrazer"), &data_dir())
}

/// Only durable settings move. Endpoints, authentication tokens, executables,
/// logs, cached releases and openrazer-win data are deliberately excluded.
pub fn migrate(old: &Path, new: &Path) -> io::Result<()> {
    const FILES: &[&str] = &[
        "persistence.json",
        "profiles.json",
        "macros.json",
        "integrations.json",
        "snaptap.json",
        "gui.json",
        "studio/settings.json",
    ];
    let marker = new.join("rsrazer-migrated.txt");
    if marker.exists() || !old.is_dir() {
        return Ok(());
    }
    for relative in FILES {
        let source = old.join(relative);
        if !source.exists() || new.join(relative).exists() {
            continue;
        }
        if source.symlink_metadata()?.file_type().is_symlink()
            || source.parent().is_some_and(|p| {
                p.symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cannot migrate a linked settings file",
            ));
        }
        copy_new(&new.join(relative), &fs::read(source)?)?;
    }
    copy_new(
        &marker,
        b"Copied durable RsRazer settings; originals preserved.\n",
    )
}
fn copy_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "migration-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    // Create exclusively, and never replace a setting created by another process.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        match fs::hard_link(&tmp, path) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            result => result,
        }
    })();
    let _ = fs::remove_file(tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_preserves_sources_and_new_settings_but_never_copies_endpoint_or_reimports() {
        let root = std::env::temp_dir().join(format!(
            "mirage-migration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let old = root.join("rsrazer");
        let new = root.join("mirage-hub");
        fs::create_dir_all(old.join("studio")).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(old.join("profiles.json"), b"old profiles").unwrap();
        fs::write(old.join("persistence.json"), b"old state").unwrap();
        fs::write(old.join("daemon.json"), b"old token").unwrap();
        fs::write(old.join("studio/settings.json"), b"language and macros").unwrap();
        fs::write(new.join("persistence.json"), b"new state").unwrap();
        migrate(&old, &new).unwrap();
        assert_eq!(
            fs::read(new.join("profiles.json")).unwrap(),
            b"old profiles"
        );
        assert_eq!(
            fs::read(new.join("persistence.json")).unwrap(),
            b"new state"
        );
        assert_eq!(
            fs::read(new.join("studio/settings.json")).unwrap(),
            b"language and macros"
        );
        assert!(!new.join("daemon.json").exists());
        assert!(old.join("profiles.json").exists());
        fs::remove_file(new.join("profiles.json")).unwrap();
        migrate(&old, &new).unwrap();
        assert!(!new.join("profiles.json").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
