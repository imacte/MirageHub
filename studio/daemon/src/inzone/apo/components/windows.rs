// SPDX-License-Identifier: GPL-3.0-only
//! Windows-only trust, UAC and driver-store operations. No PowerShell dependency.
use super::package;
use crate::{Error, Result};
use std::{
    ffi::OsStr,
    fs,
    io::Write,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree},
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, WinTrust::*,
        SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{CreateDirectoryW, FILE_ATTRIBUTE_REPARSE_POINT},
    System::{
        Com::CoTaskMemFree,
        Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE},
    },
    UI::Shell::*,
};
const DRIVERS: [&str; 6] = [
    "INZONEHeadset.inf",
    "INZONEHeadset.sys",
    "inzoneheadset.cat",
    "inzoneapo.inf",
    "INZONEVirtualizer.dll",
    "inzoneext.inf",
];
pub fn resource_directory() -> Result<PathBuf> {
    Ok(known_folder(&FOLDERID_ProgramData)?
        .join("MirageHub/SonyAudio")
        .join(super::VERSION))
}
pub fn publish_resources(root: &Path) -> Result<()> {
    // audiodg cannot read a private LocalAppData directory. This directory inherits
    // ProgramData's Users read access; executable drivers stay in the private cache.
    let destination = resource_directory()?;
    fs::create_dir_all(&destination)?;
    for entry in package::manifest()?
        .into_iter()
        .filter(|e| !DRIVERS.contains(&e.name.as_str()))
    {
        let bytes = package::verified_bytes(&root.join("files"), &entry)?;
        if package::verify_file(&destination, &entry.name).is_ok() {
            continue;
        }
        let temporary = destination.join(format!("{}.partial", entry.name));
        let result = (|| -> Result<()> {
            let mut file = fs::File::create(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, destination.join(&entry.name))?;
            Ok(())
        })();
        let _ = fs::remove_file(temporary);
        result?;
    }
    Ok(())
}
fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

pub fn verify_signature(path: &Path) -> Result<()> {
    unsafe {
        let path_wide = wide(path.as_os_str());
        let mut file: WINTRUST_FILE_INFO = std::mem::zeroed();
        file.cbStruct = std::mem::size_of_val(&file) as u32;
        file.pcwszFilePath = path_wide.as_ptr();
        let mut trust: WINTRUST_DATA = std::mem::zeroed();
        trust.cbStruct = std::mem::size_of_val(&trust) as u32;
        trust.dwUIChoice = WTD_UI_NONE;
        trust.fdwRevocationChecks = WTD_REVOKE_WHOLECHAIN;
        trust.dwUnionChoice = WTD_CHOICE_FILE;
        trust.Anonymous.pFile = &mut file;
        trust.dwStateAction = WTD_STATEACTION_VERIFY;
        trust.dwProvFlags = WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT;
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let result = WinVerifyTrust(
            ptr::null_mut(),
            &mut action,
            (&mut trust as *mut WINTRUST_DATA).cast(),
        );
        trust.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(
            ptr::null_mut(),
            &mut action,
            (&mut trust as *mut WINTRUST_DATA).cast(),
        );
        // Exact hashes pin the previously verified Sony/Microsoft publishers as well as payloads.
        if result != 0 {
            return Err(Error::Invalid(format!(
                "Sony Authenticode verification failed for {}: 0x{:08x}",
                path.display(),
                result as u32
            )));
        }
        Ok(())
    }
}
fn known_folder(id: &windows_sys::core::GUID) -> Result<PathBuf> {
    unsafe {
        let mut path = ptr::null_mut();
        let code = SHGetKnownFolderPath(id, 0, ptr::null_mut(), &mut path);
        if code < 0 {
            return Err(Error::Io(format!(
                "Windows folder lookup failed: 0x{:08x}",
                code as u32
            )));
        }
        let mut length = 0;
        while *path.add(length) != 0 {
            length += 1;
        }
        let value = std::ffi::OsString::from_wide(std::slice::from_raw_parts(path, length));
        CoTaskMemFree(path.cast());
        Ok(PathBuf::from(value))
    }
}
pub fn elevate(root: &Path) -> Result<bool> {
    for file in package::manifest()? {
        package::verified_bytes(&root.join("files"), &file)?;
    }
    let exe = std::env::current_exe()?;
    if exe
        .file_stem()
        .is_none_or(|name| !name.eq_ignore_ascii_case("mirage-hub-daemon"))
    {
        return Err(Error::Invalid(
            "Driver installation must be started from the Mirage Hub daemon".into(),
        ));
    }
    let file = wide(exe.as_os_str());
    let args = crate::process::quoted_line(
        Path::new("--sony-audio-install"),
        &[root.as_os_str().into()],
    )?;
    let verb = wide(OsStr::new("runas"));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of_val(&info) as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = args.as_ptr();
        info.nShow = 0;
        if ShellExecuteExW(&mut info) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if info.hProcess.is_null() {
            return Err(Error::Io(
                "Sony installer returned no process handle".into(),
            ));
        }
        let waited = WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1;
        let queried = GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        if waited != 0 || queried == 0 {
            return Err(Error::Io("Cannot read Sony installer result".into()));
        }
        match code {0=>Ok(false),3010=>Ok(true),_=>Err(Error::Io(format!("Sony driver installation failed (exit {code}); some drivers may already be installed. Check Windows Device Manager and retry.")))}
    }
}
struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        for name in DRIVERS {
            let _ = fs::remove_file(self.0.join(name));
        }
        let _ = fs::remove_dir(&self.0);
    }
}
pub fn install(root: &Path) -> Result<i32> {
    unsafe {
        use windows_sys::Win32::{
            Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        };
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
        let mut returned = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of_val(&elevation) as u32,
            &mut returned,
        );
        CloseHandle(token);
        if ok == 0 || elevation.TokenIsElevated == 0 {
            return Err(Error::Invalid(
                "Sony driver installation requires administrator approval".into(),
            ));
        }
    }
    // Read and authenticate bytes before creating anything privileged. No user-supplied filenames.
    let entries = package::manifest()?;
    let mut payload = Vec::new();
    for name in DRIVERS {
        let entry = entries
            .iter()
            .find(|e| e.name == name)
            .ok_or_else(|| Error::Invalid("Missing driver metadata".into()))?;
        payload.push((name, package::verified_bytes(&root.join("files"), entry)?));
    }
    let programs = known_folder(&FOLDERID_ProgramFiles)?;
    if fs::symlink_metadata(&programs)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(Error::Invalid(
            "Linked Program Files directory is not supported".into(),
        ));
    }
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| Error::Io(e.to_string()))?;
    let path = programs.join(format!(
        "MirageHub-SonyAudio-{:032x}",
        u128::from_le_bytes(random)
    ));
    unsafe {
        let sddl = wide(OsStr::new("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"));
        let mut descriptor = ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        ) == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let ok = CreateDirectoryW(wide(path.as_os_str()).as_ptr(), &attributes);
        let error = std::io::Error::last_os_error();
        LocalFree(descriptor);
        if ok == 0 {
            return Err(error.into());
        }
    }
    let stage = Stage(path);
    for (name, bytes) in payload {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(stage.0.join(name))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    for name in [
        "INZONEHeadset.sys",
        "inzoneheadset.cat",
        "INZONEVirtualizer.dll",
    ] {
        verify_signature(&stage.0.join(name))?;
    }
    let pnputil = known_folder(&FOLDERID_Windows)?.join("System32/pnputil.exe");
    let mut restart = false;
    for name in ["INZONEHeadset.inf", "inzoneapo.inf", "inzoneext.inf"] {
        let status = Command::new(&pnputil)
            .arg("/add-driver")
            .arg(stage.0.join(name))
            .arg("/install")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .status()?;
        match status.code() {
            Some(0) => {}
            Some(3010) => restart = true,
            code => return Err(Error::Io(format!("PnPUtil failed for {name}: {code:?}"))),
        }
    }
    Ok(if restart { 3010 } else { 0 })
}
