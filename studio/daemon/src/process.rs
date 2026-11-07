//! The background process must not retain inherited pipe handles from the CLI
//! which launched it (e.g. a caller waiting for `daemon start`'s captured output).
#[cfg(windows)]
use std::os::windows::{
    ffi::OsStrExt,
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use std::{ffi::OsString, io, path::Path};

#[cfg(windows)]
pub struct Child {
    handle: OwnedHandle,
}
#[cfg(not(windows))]
pub struct Child {
    child: std::process::Child,
}

#[cfg(windows)]
fn quoted_line(program: &Path, args: &[OsString]) -> io::Result<Vec<u16>> {
    let mut line = Vec::new();
    for arg in std::iter::once(program.as_os_str()).chain(args.iter().map(OsString::as_os_str)) {
        if !line.is_empty() {
            line.push(b' ' as u16);
        }
        line.push(b'"' as u16);
        let mut slashes = 0;
        for ch in arg.encode_wide() {
            if ch == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "argument contains NUL",
                ));
            }
            if ch == b'\\' as u16 {
                slashes += 1;
                continue;
            }
            line.extend(std::iter::repeat_n(
                b'\\' as u16,
                if ch == b'"' as u16 {
                    slashes * 2 + 1
                } else {
                    slashes
                },
            ));
            slashes = 0;
            line.push(ch);
        }
        line.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
        line.push(b'"' as u16);
    }
    line.push(0);
    Ok(line)
}
pub fn spawn_hidden(program: &Path, args: &[OsString]) -> io::Result<Child> {
    #[cfg(windows)]
    unsafe {
        use std::ptr;
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{
                CreateProcessW, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, PROCESS_INFORMATION,
                STARTUPINFOW,
            },
        };
        let mut command = quoted_line(program, args)?;
        let app: Vec<_> = program.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut startup: STARTUPINFOW = std::mem::zeroed();
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut process: PROCESS_INFORMATION = std::mem::zeroed();
        // No stdio or unrelated inheritable handles cross this process boundary.
        if CreateProcessW(
            app.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP,
            ptr::null(),
            ptr::null(),
            &startup,
            &mut process,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        CloseHandle(process.hThread);
        Ok(Child {
            handle: OwnedHandle::from_raw_handle(process.hProcess),
        })
    }
    #[cfg(not(windows))]
    {
        use std::process::{Command, Stdio};
        Ok(Child {
            child: Command::new(program)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?,
        })
    }
}
impl Child {
    pub fn try_wait(&mut self) -> io::Result<Option<u32>> {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::{
                Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
                System::Threading::{GetExitCodeProcess, WaitForSingleObject},
            };
            match WaitForSingleObject(self.handle.as_raw_handle(), 0) {
                WAIT_TIMEOUT => Ok(None),
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    if GetExitCodeProcess(self.handle.as_raw_handle(), &mut code) == 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(Some(code))
                }
                _ => Err(io::Error::last_os_error()),
            }
        }
        #[cfg(not(windows))]
        {
            Ok(self
                .child
                .try_wait()?
                .map(|status| status.code().unwrap_or(1) as u32))
        }
    }
    pub fn terminate(&mut self) -> io::Result<()> {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::{
                Foundation::WAIT_OBJECT_0,
                System::Threading::{TerminateProcess, WaitForSingleObject},
            };
            if TerminateProcess(self.handle.as_raw_handle(), 1) == 0 {
                return Err(io::Error::last_os_error());
            }
            if WaitForSingleObject(self.handle.as_raw_handle(), 3000) != WAIT_OBJECT_0 {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "child did not terminate",
                ));
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            self.child.kill()?;
            self.child.wait()?;
            Ok(())
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn quoted_arguments_round_trip_spaces_unicode_quotes_and_trailing_backslashes() {
        use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
        let args: Vec<OsString> = [
            "",
            "中文 文件夹",
            "C:\\path with spaces\\",
            "a\"b",
            "\\\\\"quoted\\",
            "%PATH% & $()",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let line = quoted_line(Path::new("C:\\app.exe"), &args).unwrap();
        unsafe {
            let mut count = 0;
            let values = CommandLineToArgvW(line.as_ptr(), &mut count);
            assert!(!values.is_null());
            let mut actual = Vec::new();
            for i in 1..count as usize {
                let start = *values.add(i);
                let mut len = 0;
                while *start.add(len) != 0 {
                    len += 1;
                }
                actual.push(String::from_utf16_lossy(std::slice::from_raw_parts(
                    start, len,
                )));
            }
            LocalFree(values.cast());
            assert_eq!(
                actual,
                args.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>()
            );
        }
    }
}
