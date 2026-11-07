use crate::{persistence, Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    #[serde(default = "loopback")]
    pub host: String,
    pub port: u16,
    pub token: String,
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub version: String,
}
fn loopback() -> String {
    "127.0.0.1".into()
}
impl Endpoint {
    pub fn read(path: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }
    pub fn publish(&self, path: &Path) -> Result<PublishedEndpoint> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        // Create a new file with its DACL already set. Replacing an older endpoint
        // also replaces any permissive ACL inherited by the old Python file.
        let temp = parent.join(format!(".daemon-{}.tmp", new_token()?));
        let result = (|| {
            let mut file = secret_file(&temp)?;
            file.write_all(&serde_json::to_vec(self)?)?;
            file.sync_all()?;
            drop(file);
            persistence::replace(&temp, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result?;
        Ok(PublishedEndpoint {
            path: path.into(),
            endpoint: self.clone(),
        })
    }
}

/// Keep this alive until the listener and device workers have stopped.
pub struct PublishedEndpoint {
    path: PathBuf,
    endpoint: Endpoint,
}
impl PublishedEndpoint {
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for PublishedEndpoint {
    fn drop(&mut self) {
        // A stale guard must never remove a newer daemon's discovery file.
        if Endpoint::read(&self.path).is_ok_and(|e| e == self.endpoint) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn new_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| Error::Io(format!("random token: {e}")))?;
    Ok(base64url(&bytes))
}
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut result = String::new();
    for chunk in bytes.chunks(3) {
        let bits = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        for index in 0..chunk.len() + 1 {
            result.push(ALPHABET[((bits >> (18 - 6 * index)) & 63) as usize] as char);
        }
    }
    result
}

#[cfg(unix)]
fn secret_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}
#[cfg(windows)]
fn secret_file(path: &Path) -> std::io::Result<fs::File> {
    use std::{
        io,
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{GetLastError, LocalFree, GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            },
            GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        },
        Storage::FileSystem::{CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    struct Local(*mut std::ffi::c_void);
    impl Drop for Local {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = OwnedHandle::from_raw_handle(token);
        let mut size = 0;
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            ptr::null_mut(),
            0,
            &mut size,
        );
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        // TOKEN_USER contains pointers; allocate suitably aligned backing storage.
        let mut data = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            data.as_mut_ptr().cast(),
            size,
            &mut size,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let user = &*data.as_ptr().cast::<TOKEN_USER>();
        let mut sid = ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
            return Err(io::Error::last_os_error());
        }
        let _sid = Local(sid.cast());
        let mut len = 0;
        while *sid.add(len) != 0 {
            len += 1;
        }
        let sid_text = String::from_utf16_lossy(std::slice::from_raw_parts(sid, len));
        // P protects the DACL from inheritance. Only this user and LocalSystem
        // can read the bearer token, even in a broadly accessible directory.
        let sddl: Vec<u16> = format!("D:P(A;;FA;;;SY)(A;;FA;;;{sid_text})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let _descriptor = Local(descriptor);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let handle = CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::from_raw_os_error(GetLastError() as i32));
        }
        Ok(fs::File::from_raw_handle(handle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_encoding_matches_urlsafe_base64_without_padding() {
        for (input, output) in [
            (b"".as_slice(), ""),
            (b"f", "Zg"),
            (b"fo", "Zm8"),
            (b"foo", "Zm9v"),
            (&[255, 255, 254], "___-"),
        ] {
            assert_eq!(base64url(input), output);
        }
        let first = new_token().unwrap();
        assert_eq!(first.len(), 43);
        assert!(first
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)));
        assert_ne!(first, new_token().unwrap());
    }
}
