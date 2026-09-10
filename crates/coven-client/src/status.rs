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
        let mut file = create_owner_only_status_file(&temporary_path)?;
        file.write_all(contents)
            .map_err(|error| StatusWriteStage::WriteContents.io_error(error))?;
        if !contents.ends_with(b"\n") {
            file.write_all(b"\n")
                .map_err(|error| StatusWriteStage::WriteNewline.io_error(error))?;
        }
        file.sync_all()
            .map_err(|error| StatusWriteStage::SyncTemporary.io_error(error))?;
        drop(file);
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

fn create_owner_only_status_file(path: &Path) -> Result<std::fs::File, ClientError> {
    use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
    use std::ptr;
    use windows_sys::Win32::{
        Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            },
            SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{
            CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        },
    };

    let owner = CurrentWindowsUser::read()?;
    let mut owner_text = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(owner.sid(), &mut owner_text) } == 0 {
        return Err(StatusWriteStage::ConvertDescriptor.io_error(std::io::Error::last_os_error()));
    }
    let _owner_text = LocalAllocation(owner_text.cast());
    let mut owner_length = 0;
    while unsafe { *owner_text.add(owner_length) } != 0 {
        owner_length += 1;
    }
    let mut sddl: Vec<u16> = "O:".encode_utf16().collect();
    sddl.extend_from_slice(unsafe { std::slice::from_raw_parts(owner_text, owner_length) });
    sddl.extend(WINDOWS_OWNER_ONLY_FILE_DACL_SDDL.encode_utf16());
    sddl.push(0);
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(StatusWriteStage::ConvertDescriptor.io_error(std::io::Error::last_os_error()));
    }
    let _descriptor = LocalAllocation(descriptor);
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let mut path: Vec<u16> = path.as_os_str().encode_wide().collect();
    if path.contains(&0) {
        return Err(
            StatusWriteStage::CreateTemporary.io_error(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "status path contains a NUL",
            )),
        );
    }
    path.push(0);
    // Establish the explicit owner and protected DACL before any status bytes
    // exist. Inherited OWNER RIGHTS restrictions can forbid later ACL updates.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &security,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(StatusWriteStage::CreateTemporary.io_error(std::io::Error::last_os_error()));
    }
    // CreateFileW returned a new owned handle; File closes it exactly once.
    Ok(unsafe { std::fs::File::from_raw_handle(handle) })
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

    fn assert_status_file_is_owner_only(path: &Path) {
        use std::os::windows::ffi::OsStrExt;
        use std::ptr;
        use windows_sys::Win32::Security::{
            Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT},
            EqualSid, GetAce, GetSecurityDescriptorControl, ACCESS_ALLOWED_ACE,
            DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        };
        use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

        let expected_owner = CurrentWindowsUser::read().unwrap();
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut owner = ptr::null_mut();
        let mut dacl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        assert_eq!(
            unsafe {
                GetNamedSecurityInfoW(
                    path.as_ptr(),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    &mut owner,
                    ptr::null_mut(),
                    &mut dacl,
                    ptr::null_mut(),
                    &mut descriptor,
                )
            },
            0
        );
        let _descriptor = LocalAllocation(descriptor);
        assert_ne!(unsafe { EqualSid(owner, expected_owner.sid()) }, 0);
        let mut control = 0;
        let mut revision = 0;
        assert_ne!(
            unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
            0
        );
        assert_ne!(control & SE_DACL_PROTECTED, 0);
        assert!(!dacl.is_null());
        assert_eq!(unsafe { (*dacl).AceCount }, 1);
        let mut entry = ptr::null_mut();
        assert_ne!(unsafe { GetAce(dacl, 0, &mut entry) }, 0);
        let ace = unsafe { &*entry.cast::<ACCESS_ALLOWED_ACE>() };
        assert_eq!(ace.Header.AceType, 0); // ACCESS_ALLOWED_ACE_TYPE
        assert_eq!(ace.Header.AceFlags, 0);
        assert_eq!(ace.Mask, FILE_ALL_ACCESS);
        // OWNER RIGHTS SID is S-1-3-4: revision 1, one subauthority,
        // SECURITY_CREATOR_SID_AUTHORITY, SECURITY_CREATOR_OWNER_RIGHTS_RID.
        let owner_rights: [u32; 3] = [0x00000101, 0x03000000, 4];
        let ace_sid = (&ace.SidStart as *const u32).cast_mut().cast();
        assert_ne!(
            unsafe { EqualSid(ace_sid, owner_rights.as_ptr().cast_mut().cast()) },
            0
        );
    }

    #[test]
    fn secure_temporary_file_is_owner_only_before_writing_and_cannot_overwrite() {
        let home = TestHome::new();
        let path = home.0.join("temporary");
        let mut file = create_owner_only_status_file(&path).expect("create secure empty file");
        assert_status_file_is_owner_only(&path);
        file.write_all(b"original").unwrap();
        file.sync_all().unwrap();
        drop(file);
        let error = create_owner_only_status_file(&path).expect_err("exclusive create must fail");
        let ClientError::Io { source, .. } = error else {
            panic!("expected I/O error")
        };
        assert_eq!(source.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
    }

    #[test]
    fn secure_temporary_creation_rejects_nul_without_creating_truncated_path() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let home = TestHome::new();
        let path = home.0.join("must-not-exist");
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.extend([0, 120]);
        let invalid = PathBuf::from(std::ffi::OsString::from_wide(&wide));
        let error = create_owner_only_status_file(&invalid).expect_err("NUL must fail");
        let ClientError::Io { source, .. } = error else {
            panic!("expected I/O error")
        };
        assert_eq!(source.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!path.exists());
    }

    fn trace_creation_descriptor_matrix(directory: &Path, sid: &str, context: &str) {
        use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
        use std::ptr;
        use windows_sys::Win32::{
            Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE},
            Security::{
                Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SECURITY_ATTRIBUTES,
            },
            Storage::FileSystem::{
                CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ,
                FILE_SHARE_WRITE,
            },
        };

        for (label, sddl, access) in [
            ("inherited", None, GENERIC_WRITE),
            ("owner", Some(format!("O:{sid}")), GENERIC_WRITE),
            (
                "ow-default",
                Some("D:P(A;;GA;;;OW)".to_owned()),
                GENERIC_WRITE,
            ),
            (
                "ow-explicit",
                Some(format!("O:{sid}D:P(A;;GA;;;OW)")),
                GENERIC_WRITE,
            ),
            (
                "user-default",
                Some(format!("D:P(A;;GA;;;{sid})")),
                GENERIC_WRITE,
            ),
            (
                "user-explicit",
                Some(format!("O:{sid}D:P(A;;GA;;;{sid})")),
                GENERIC_WRITE,
            ),
            ("ow-zero-access", Some(format!("O:{sid}D:P(A;;GA;;;OW)")), 0),
        ] {
            let mut descriptor = ptr::null_mut();
            if let Some(sddl) = sddl {
                let encoded: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
                assert_ne!(
                    unsafe {
                        ConvertStringSecurityDescriptorToSecurityDescriptorW(
                            encoded.as_ptr(),
                            1,
                            &mut descriptor,
                            ptr::null_mut(),
                        )
                    },
                    0,
                    "convert creation probe descriptor"
                );
            }
            let _descriptor = LocalAllocation(descriptor);
            let security = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            let path = directory.join(format!("creation-probe-{label}.tmp"));
            let encoded: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let handle = unsafe {
                CreateFileW(
                    encoded.as_ptr(),
                    access,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    if descriptor.is_null() {
                        ptr::null()
                    } else {
                        &security
                    },
                    CREATE_NEW,
                    FILE_ATTRIBUTE_NORMAL,
                    ptr::null_mut(),
                )
            };
            let error = if handle == INVALID_HANDLE_VALUE {
                std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
            } else {
                drop(unsafe { std::fs::File::from_raw_handle(handle) });
                0
            };
            eprintln!("creation-probe:{context}:{label}:os={error}");
            let exists = path.try_exists().expect("inspect creation probe existence");
            eprintln!("creation-probe:{context}:{label}:exists={exists}");
            if exists {
                std::fs::remove_file(path).expect("remove creation probe file");
            }
        }
    }

    #[test]
    fn status_replacement_succeeds_with_inherited_modify_only_owner_rights() {
        use std::os::windows::ffi::OsStrExt;
        use std::ptr;
        use windows_sys::Win32::Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SetNamedSecurityInfoW, SE_FILE_OBJECT,
            },
            GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION,
        };

        let home = TestHome::new();
        let owner = CurrentWindowsUser::read().expect("read fixture owner");
        let mut sid_text = ptr::null_mut();
        assert_ne!(
            unsafe { ConvertSidToStringSidW(owner.sid(), &mut sid_text) },
            0
        );
        let _sid_text = LocalAllocation(sid_text.cast());
        let mut len = 0;
        while unsafe { *sid_text.add(len) } != 0 {
            len += 1;
        }
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(sid_text, len) })
            .expect("decode fixture owner");
        // Match the isolated writer's modify grant and inherited OWNER RIGHTS
        // restriction. No administrator ACE may mask the missing WRITE_DAC.
        let sddl: Vec<u16> = format!("D:P(A;OICI;0x001301bf;;;{sid})(A;OICI;RC;;;OW)")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = ptr::null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    1,
                    &mut descriptor,
                    ptr::null_mut(),
                )
            },
            0
        );
        let _descriptor = LocalAllocation(descriptor);
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = ptr::null_mut();
        assert_ne!(
            unsafe {
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)
            },
            0
        );
        assert_ne!(present, 0);
        let mut path: Vec<u16> = home.0.as_os_str().encode_wide().chain(Some(0)).collect();
        assert_eq!(
            unsafe {
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
            },
            0,
            "apply isolated fixture ACL"
        );

        // Prove the directory still permits ordinary creation before attributing
        // a denial to the explicit security descriptor used by the writer.
        let control_path = home.0.join("inherited-create-control.tmp");
        let control = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&control_path)
            .expect("ordinary creation under inherited modify-only rights");
        drop(control);
        std::fs::remove_file(&control_path).expect("remove inherited creation control");

        let ordinary = TestHome::new();
        trace_creation_descriptor_matrix(&ordinary.0, &sid, "ordinary");
        trace_creation_descriptor_matrix(&home.0, &sid, "restricted");

        write_owner_only_windows_daemon_status(&home.0, b"first")
            .expect("create secure status under inherited modify-only rights");
        assert_status_file_is_owner_only(&home.0.join("daemon.json"));
        write_owner_only_windows_daemon_status(&home.0, b"second")
            .expect("replace secure status under inherited modify-only rights");
        assert_status_file_is_owner_only(&home.0.join("daemon.json"));
        assert_eq!(
            std::fs::read(home.0.join("daemon.json")).unwrap(),
            b"second\n"
        );
        assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 1);
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
