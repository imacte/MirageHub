// SPDX-License-Identifier: GPL-3.0-only
//! Portable download, MSI/CAB extraction and integrity checks. No shell execution.
use super::{MANIFEST, VERSION};
use crate::{Error, Result};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};
const URL: &str =
    "https://info.update.sony.net/HP002/APID001WN00/contents/0012/INZONEHub_Setup_1.0.20.0.exe";
const SIZE: u64 = 144917976;
const SHA256: &str = "6128d97b5b46c28f04dff4715b1b79c781437844a5afa94c37b6bdc7a856e2f6";
#[derive(serde::Deserialize)]
pub struct Entry {
    pub key: String,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}
pub fn manifest() -> Result<Vec<Entry>> {
    Ok(serde_json::from_str(MANIFEST)?)
}
fn hash(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn verified_bytes(root: &Path, entry: &Entry) -> Result<Vec<u8>> {
    let mut file = File::open(root.join(&entry.name))?;
    if file.metadata()?.len() != entry.bytes {
        return Err(Error::Invalid(format!(
            "Sony component size mismatch: {}",
            entry.name
        )));
    }
    let mut bytes = Vec::new();
    (&mut file).take(entry.bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != entry.bytes || hash(&bytes) != entry.sha256 {
        return Err(Error::Invalid(format!(
            "Sony component SHA-256 mismatch: {}",
            entry.name
        )));
    }
    Ok(bytes)
}
pub fn verify_file(root: &Path, name: &str) -> Result<()> {
    let entries = manifest()?;
    let entry = entries
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| Error::Invalid("Unknown Sony component".into()))?;
    verified_bytes(root, entry).map(|_| ())
}
fn package_valid(path: &Path) -> Result<()> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() != SIZE {
        return Err(Error::Invalid("Sony package size mismatch".into()));
    }
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        digest.update(&buffer[..count]);
        if total > SIZE {
            break;
        }
    }
    let actual = digest
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if total != SIZE || actual != SHA256 {
        return Err(Error::Invalid("Sony package SHA-256 mismatch".into()));
    }
    Ok(())
}
struct Temporary {
    path: PathBuf,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
struct Stage {
    path: PathBuf,
    names: Vec<String>,
}
impl Drop for Stage {
    fn drop(&mut self) {
        for name in &self.names {
            let _ = fs::remove_file(self.path.join(name));
        }
        let _ = fs::remove_dir(&self.path);
    }
}
fn download(path: &Path) -> Result<()> {
    let partial = Temporary {
        path: path.with_extension("download"),
    };
    let response = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(60))
        .timeout(Duration::from_secs(900))
        .build()
        .get(URL)
        .call()
        .map_err(|e| Error::Io(format!("Sony download: {e}")))?;
    let mut source = response.into_reader().take(SIZE + 1);
    let mut target = File::create(&partial.path)?;
    let count = std::io::copy(&mut source, &mut target)?;
    target.sync_all()?;
    drop(target);
    if count != SIZE {
        return Err(Error::Invalid(
            "Sony download has an unexpected length".into(),
        ));
    }
    package_valid(&partial.path)?;
    fs::rename(&partial.path, path)?;
    Ok(())
}
/// Signature checking is supplied by the host platform; parsing and hashing are portable.
pub fn prepare(
    root: &Path,
    signature: impl Fn(&Path) -> Result<()>,
    phase: impl Fn(&'static str),
) -> Result<()> {
    let files = root.join("files");
    let entries = manifest()?;
    if entries.iter().all(|e| verified_bytes(&files, e).is_ok()) {
        for name in [
            "INZONEHeadset.sys",
            "inzoneheadset.cat",
            "INZONEVirtualizer.dll",
        ] {
            signature(&files.join(name))?;
        }
        phase("ready");
        return Ok(());
    }
    fs::create_dir_all(root)?;
    let package = root.join(format!("INZONEHub_Setup_{VERSION}.exe"));
    if package.exists() {
        if let Err(e) = package_valid(&package) {
            fs::remove_file(&package)?;
            return Err(e);
        }
    } else {
        phase("downloading");
        download(&package)?;
    }
    phase("verifying");
    signature(&package)?;
    // Pinned offset/length apply only to the exact outer bytes validated above.
    let msi = Temporary {
        path: root.join("main.msi"),
    };
    let cab = Temporary {
        path: root.join("Data1.cab"),
    };
    let mut input = File::open(&package)?;
    input.seek(SeekFrom::Start(7212312))?;
    let mut output = File::create(&msi.path)?;
    if std::io::copy(&mut input.take(136134144), &mut output)? != 136134144 {
        return Err(Error::Invalid("Truncated Sony MSI".into()));
    }
    output.sync_all()?;
    drop(output);
    signature(&msi.path)?;
    phase("extracting");
    let mut package_db = msi::open(&msi.path)?;
    let mut output = File::create(&cab.path)?;
    let mut stream = package_db.read_stream("Data1.cab")?;
    if std::io::copy(&mut stream.by_ref().take(123448661), &mut output)? != 123448660 {
        return Err(Error::Invalid("Unexpected Sony cabinet size".into()));
    }
    drop(stream);
    drop(package_db);
    drop(output);
    let mut cabinet = cab::Cabinet::new(File::open(&cab.path)?)?;
    // Archive names never become output paths. Extract only entries in the compiled manifest.
    let order: Vec<_> = cabinet
        .folder_entries()
        .flat_map(|folder| folder.file_entries())
        .map(|file| file.name().to_owned())
        .collect();
    let mut payload = Vec::new();
    for key in order {
        let Some(entry) = entries.iter().find(|e| e.key == key) else {
            continue;
        };
        if payload.iter().any(|(name, _)| name == &entry.name) {
            return Err(Error::Invalid("Duplicate Sony CAB entry".into()));
        }
        let mut bytes = Vec::new();
        cabinet
            .read_file(&key)?
            .take(entry.bytes + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != entry.bytes || hash(&bytes) != entry.sha256 {
            return Err(Error::Invalid(format!(
                "Sony extracted file failed verification: {}",
                entry.name
            )));
        }
        payload.push((entry.name.clone(), bytes));
    }
    if payload.len() != entries.len() {
        return Err(Error::Invalid(
            "Sony cabinet is missing required components".into(),
        ));
    }
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| Error::Io(e.to_string()))?;
    let stage_path = root.join(format!("staging-{:032x}", u128::from_le_bytes(random)));
    fs::create_dir(&stage_path)?;
    let stage = Stage {
        path: stage_path,
        names: entries.iter().map(|e| e.name.clone()).collect(),
    };
    for (name, bytes) in payload {
        let path = stage.path.join(&name);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        if matches!(
            name.as_str(),
            "INZONEHeadset.sys" | "inzoneheadset.cat" | "INZONEVirtualizer.dll"
        ) {
            signature(&path)?;
        }
    }
    fs::create_dir_all(&files)?;
    for name in &stage.names {
        fs::rename(stage.path.join(name), files.join(name))?;
    }
    fs::write(
        files.join("ready.json"),
        format!("{{\"version\":\"{VERSION}\",\"verified\":true}}"),
    )?;
    fs::remove_file(package)?;
    phase("ready");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_is_unique_and_cannot_escape_output_directory() {
        let entries = manifest().unwrap();
        let mut names = std::collections::HashSet::new();
        let mut keys = std::collections::HashSet::new();
        assert_eq!(entries.len(), 23);
        assert_eq!(entries.iter().map(|e| e.bytes).sum::<u64>(), 2858082);
        for e in entries {
            assert!(!e.name.contains(['/', '\\', ':']));
            assert!(names.insert(e.name.to_lowercase()));
            assert!(keys.insert(e.key));
            assert_eq!(e.sha256.len(), 64);
        }
    }
    #[test]
    fn matching_size_does_not_accept_tampered_bytes() {
        let root = std::env::temp_dir().join(format!("mirage-sony-hash-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let entry = Entry {
            key: "test".into(),
            name: "test.bin".into(),
            bytes: 4,
            sha256: hash(b"good"),
        };
        fs::write(root.join("test.bin"), b"good").unwrap();
        assert_eq!(verified_bytes(&root, &entry).unwrap(), b"good");
        fs::write(root.join("test.bin"), b"evil").unwrap();
        assert!(verified_bytes(&root, &entry).is_err());
        fs::write(root.join("test.bin"), b"shorter").unwrap();
        assert!(verified_bytes(&root, &entry).is_err());
        fs::remove_file(root.join("test.bin")).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
