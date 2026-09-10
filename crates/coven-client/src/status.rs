use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{status_error::StatusWriteStage, ClientError};

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
            .map_err(|error| StatusWriteStage::CreateTemporary.io_error(error))?;
        file.write_all(contents)
            .map_err(|error| StatusWriteStage::WriteContents.io_error(error))?;
        if !contents.ends_with(b"\n") {
            file.write_all(b"\n")
                .map_err(|error| StatusWriteStage::WriteNewline.io_error(error))?;
        }
        file.sync_all()
            .map_err(|error| StatusWriteStage::SyncTemporary.io_error(error))?;
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
        return Err(StatusWriteStage::ConvertDescriptor.io_error(std::io::Error::last_os_error()));
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
        return Err(StatusWriteStage::ApplySecurity
            .io_error(std::io::Error::from_raw_os_error(status as i32)));
    }
    Ok(())
}

fn replace_status_file(temporary_path: &Path, status_path: &Path) -> Result<(), ClientError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::{ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION},
        Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH},
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        if unsafe {
            MoveFileExW(
                temporary.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } != 0
        {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if !matches!(
            error.raw_os_error(),
            Some(code)
                if code == ERROR_ACCESS_DENIED as i32
                    || code == ERROR_SHARING_VIOLATION as i32
        ) || std::time::Instant::now() >= deadline
        {
            return Err(StatusWriteStage::ReplaceStatus.io_error(error));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
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
            Security::{GetLengthSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER},
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        };

        let mut process_token = ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut process_token) } == 0 {
            return Err(StatusWriteStage::OpenToken.io_error(std::io::Error::last_os_error()));
        }
        let _token = Handle(process_token);
        let mut bytes = Self::token_buffer_size(process_token)?;
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
            return Err(StatusWriteStage::ReadToken.io_error(std::io::Error::last_os_error()));
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

    fn token_buffer_size(
        process_token: windows_sys::Win32::Foundation::HANDLE,
    ) -> Result<u32, ClientError> {
        use windows_sys::Win32::{
            Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER},
            Security::{GetTokenInformation, TokenUser},
        };

        let mut bytes = 0;
        let initial = unsafe {
            GetTokenInformation(
                process_token,
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut bytes,
            )
        };
        if initial == 0 {
            let code = unsafe { GetLastError() };
            if code != ERROR_INSUFFICIENT_BUFFER {
                return Err(StatusWriteStage::ReadToken
                    .io_error(std::io::Error::from_raw_os_error(code as i32)));
            }
        }
        if initial != 0 || bytes == 0 {
            return Err(ClientError::Discovery(
                "unable to size current Windows user token for daemon status".to_owned(),
            ));
        }
        Ok(bytes)
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

    struct TestHome(PathBuf);

    impl TestHome {
        fn new() -> Self {
            let path = temporary_status_path(&std::env::temp_dir().join("daemon.json"));
            std::fs::create_dir(&path).expect("create isolated writer test home");
            Self(path)
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_status_home_identifies_temporary_creation_and_preserves_os_error() {
        use windows_sys::Win32::Foundation::ERROR_PATH_NOT_FOUND;

        let parent = TestHome::new();
        let error = write_owner_only_windows_daemon_status(&parent.0.join("missing"), b"{}")
            .expect_err("missing parent must reject temporary creation");
        let ClientError::Io { operation, source } = error else {
            panic!("temporary creation must retain its I/O error");
        };
        assert_eq!(
            operation,
            "failed to write owner-only Windows daemon status: create-temporary-file"
        );
        assert_eq!(source.raw_os_error(), Some(ERROR_PATH_NOT_FOUND as i32));
        assert_eq!(std::fs::read_dir(&parent.0).unwrap().count(), 0);
    }

    #[test]
    fn blocked_status_replacement_identifies_operation_and_cleans_temporary_file() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION};

        let home = TestHome::new();
        let path = home.0.join("daemon.json");
        std::fs::write(&path, b"old").expect("create current status");
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .expect("hold a non-sharing status reader");
        let result = write_owner_only_windows_daemon_status(&home.0, b"new");
        drop(reader);
        let error = result.expect_err("non-sharing reader must prevent replacement");
        let ClientError::Io { operation, source } = error else {
            panic!("replacement must retain its I/O error");
        };
        assert_eq!(
            operation,
            "failed to write owner-only Windows daemon status: replace-status-file"
        );
        assert!(matches!(source.raw_os_error(), Some(code)
            if code == ERROR_ACCESS_DENIED as i32 || code == ERROR_SHARING_VIOLATION as i32));
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 1);
    }

    #[test]
    fn invalid_token_size_query_preserves_operation_and_os_error() {
        use windows_sys::Win32::Foundation::ERROR_INVALID_HANDLE;

        let error = CurrentWindowsUser::token_buffer_size(std::ptr::null_mut())
            .expect_err("null token must fail the sizing query");
        let ClientError::Io { operation, source } = error else {
            panic!("token sizing must retain its I/O error");
        };
        assert_eq!(
            operation,
            "failed to write owner-only Windows daemon status: read-process-token"
        );
        assert_eq!(source.raw_os_error(), Some(ERROR_INVALID_HANDLE as i32));
    }

    #[test]
    fn daemon_status_dacl_does_not_inherit_directory_aces() {
        assert_eq!(WINDOWS_OWNER_ONLY_FILE_DACL_SDDL, "D:P(A;;GA;;;OW)");
    }
}
