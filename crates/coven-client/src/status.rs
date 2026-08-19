use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ClientError;

const STATUS_WRITE_OPERATION: &str = "failed to write owner-only Windows daemon status";
const WINDOWS_OWNER_ONLY_FILE_DACL_SDDL: &str = "D:P(A;;GA;;;OW)";

pub fn write_owner_only_windows_daemon_status(
    coven_home: &Path,
    contents: &[u8],
) -> Result<(), ClientError> {
    let status_path = coven_home.join("daemon.json");
    let temporary_path = temporary_status_path(&status_path);
    let write_result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)
            .map_err(status_io_error)?;
        file.write_all(contents).map_err(status_io_error)?;
        if !contents.ends_with(b"\n") {
            file.write_all(b"\n").map_err(status_io_error)?;
        }
        file.sync_all().map_err(status_io_error)?;
        drop(file);
        set_owner_only_file_security(&temporary_path)?;
        replace_status_file(&temporary_path, &status_path)
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    write_result
}

fn temporary_status_path(status_path: &Path) -> PathBuf {
    static NEXT_TEMPORARY_STATUS: AtomicU64 = AtomicU64::new(0);

    let sequence = NEXT_TEMPORARY_STATUS.fetch_add(1, Ordering::Relaxed);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    status_path.with_file_name(format!(
        ".daemon-status-{}-{timestamp}-{sequence}.tmp",
        std::process::id()
    ))
}

fn set_owner_only_file_security(path: &Path) -> Result<(), ClientError> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows_sys::Win32::Security::{
        Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW,
            SE_FILE_OBJECT,
        },
        GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION,
    };

    let descriptor_sddl: Vec<u16> = OsStr::new(WINDOWS_OWNER_ONLY_FILE_DACL_SDDL)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor_sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(status_io_error(std::io::Error::last_os_error()));
    }
    let _descriptor = LocalAllocation(descriptor);
    let mut dacl_present = 0;
    let mut dacl = ptr::null_mut();
    let mut dacl_defaulted = 0;
    if unsafe {
        GetSecurityDescriptorDacl(
            descriptor,
            &mut dacl_present,
            &mut dacl,
            &mut dacl_defaulted,
        )
    } == 0
        || dacl_present == 0
        || dacl.is_null()
    {
        return Err(ClientError::Discovery(
            "owner-only Windows daemon status descriptor had no DACL".to_owned(),
        ));
    }

    let owner = CurrentWindowsUser::read()?;
    let mut path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let status = unsafe {
        SetNamedSecurityInfoW(
            path.as_mut_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            owner.sid(),
            ptr::null_mut(),
            dacl,
            ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(status_io_error(std::io::Error::from_raw_os_error(
            status as i32,
        )));
    }
    Ok(())
}

fn replace_status_file(temporary_path: &Path, status_path: &Path) -> Result<(), ClientError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let temporary: Vec<u16> = temporary_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let destination: Vec<u16> = status_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    if unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(status_io_error(std::io::Error::last_os_error()));
    }
    Ok(())
}

fn status_io_error(source: std::io::Error) -> ClientError {
    ClientError::Io {
        operation: STATUS_WRITE_OPERATION,
        source,
    }
}

struct CurrentWindowsUser {
    words: Vec<usize>,
}

impl CurrentWindowsUser {
    fn read() -> Result<Self, ClientError> {
        use std::mem::size_of;
        use std::ptr;
        use windows_sys::Win32::{
            Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER},
            Security::{GetLengthSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER},
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        };

        let mut process_token = ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut process_token) } == 0 {
            return Err(status_io_error(std::io::Error::last_os_error()));
        }
        let _token = Handle(process_token);
        let mut bytes = 0;
        let initial = unsafe {
            GetTokenInformation(process_token, TokenUser, ptr::null_mut(), 0, &mut bytes)
        };
        if initial != 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || bytes == 0 {
            return Err(ClientError::Discovery(
                "unable to size current Windows user token for daemon status".to_owned(),
            ));
        }
        let word_len = (bytes as usize)
            .max(1)
            .div_ceil(std::mem::size_of::<usize>());
        let mut words = vec![0_usize; word_len];
        if unsafe {
            GetTokenInformation(
                process_token,
                TokenUser,
                words.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
            )
        } == 0
        {
            return Err(status_io_error(std::io::Error::last_os_error()));
        }
        if (bytes as usize) < size_of::<TOKEN_USER>()
            || bytes as usize > words.len() * size_of::<usize>()
        {
            return Err(ClientError::Discovery(
                "current Windows user token had an invalid daemon status size".to_owned(),
            ));
        }
        let user = unsafe { &*words.as_ptr().cast::<TOKEN_USER>() };
        if user.User.Sid.is_null() || unsafe { GetLengthSid(user.User.Sid) } == 0 {
            return Err(ClientError::Discovery(
                "current Windows user token had no valid daemon status SID".to_owned(),
            ));
        }
        Ok(Self { words })
    }

    fn sid(&self) -> windows_sys::Win32::Security::PSID {
        use windows_sys::Win32::Security::TOKEN_USER;

        let user = unsafe { &*self.words.as_ptr().cast::<TOKEN_USER>() };
        user.User.Sid
    }
}

const _: () = assert!(
    std::mem::align_of::<usize>()
        >= std::mem::align_of::<windows_sys::Win32::Security::TOKEN_USER>()
);

struct Handle(windows_sys::Win32::Foundation::HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

struct LocalAllocation(*mut std::ffi::c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_status_dacl_does_not_inherit_directory_aces() {
        assert_eq!(WINDOWS_OWNER_ONLY_FILE_DACL_SDDL, "D:P(A;;GA;;;OW)");
    }
}
