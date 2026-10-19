//! Held native executable identity. Windows execution proof remains required.
use super::{physical_path, read_pe, stream_sha256, windows_path, Architecture, MAX_PATH_UNITS};
use crate::{
    job::{self, ContainedProcess},
    Failure,
};
use std::{
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawHandle, BorrowedHandle},
    },
};
use windows_sys::Win32::{
    Storage::FileSystem::{
        GetDriveTypeW, GetFileType, GetFinalPathNameByHandleW, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_TYPE_DISK, VOLUME_NAME_NT,
    },
    System::WindowsProgramming::DRIVE_FIXED,
};

fn checked_kind(file: &File, directory: bool) -> Result<(), Failure> {
    let metadata = file.metadata().map_err(|_| Failure::InvalidLaunch)?;
    // SAFETY: The file owns a live handle; this query does not dereference caller memory.
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
        return Err(Failure::InvalidLaunch);
    }
    Ok(())
}
fn directory(units: &[u16]) -> Result<File, Failure> {
    // LIST_DIRECTORY supplies actual read-data access; attribute-only opens do not
    // establish the sharing exclusion needed to prevent rename/reparse mutation.
    let file = OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(OsString::from_wide(units))
        .map_err(|_| Failure::InvalidLaunch)?;
    checked_kind(&file, true)?;
    Ok(file)
}
fn native_root(file: &File) -> Result<Vec<u16>, Failure> {
    let mut name = vec![0; MAX_PATH_UNITS + 1];
    // SAFETY: Owned root directory handle and bounded writable UTF-16 output.
    let count = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            name.as_mut_ptr(),
            name.len() as u32,
            VOLUME_NAME_NT,
        )
    } as usize;
    if count == 0 || count >= name.len() {
        return Err(Failure::InvalidLaunch);
    }
    name.truncate(count);
    Ok(name)
}

/// An image/ancestry read lock, borrowed through creation of a suspended owned process.
/// This does not attest dependency DLLs, toolchains, policy or authority.
pub struct LockedImage {
    program: OsString,
    _file: File,
    _directories: Vec<File>,
}
impl LockedImage {
    pub fn open(program: &OsStr, expected_sha256: &str) -> Result<Self, Failure> {
        if !cfg!(target_arch = "x86_64")
            || expected_sha256.len() != 64
            || !expected_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Failure::InvalidLaunch);
        }
        let original: Vec<u16> = program.encode_wide().take(MAX_PATH_UNITS + 1).collect();
        let layout = windows_path(&original)?;
        let mut drive = original[..layout.root_end].to_vec();
        drive.push(0);
        // SAFETY: Valid bounded NUL-terminated DOS-drive root, not a device/UNC input.
        if unsafe { GetDriveTypeW(drive.as_ptr()) } != DRIVE_FIXED {
            return Err(Failure::InvalidLaunch);
        }
        let root = directory(&original[..layout.root_end])?;
        // Bind subsequent lookups and CreateProcess to the kernel volume, not a
        // session-local DOS drive alias which could change after this root open.
        // Only pure physical volume roots are accepted; SUBST/subdirectory roots refuse.
        let (program, layout) = physical_path(&original, &native_root(&root)?)?;
        let mut directories = Vec::with_capacity(layout.component_ends.len());
        directories.push(root);
        for &end in layout
            .component_ends
            .iter()
            .take(layout.component_ends.len() - 1)
        {
            directories.push(directory(&program[..end])?);
        }
        let program = OsString::from_wide(&program);
        let mut file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&program)
            .map_err(|_| Failure::InvalidLaunch)?;
        checked_kind(&file, false)?;
        let length = file.metadata().map_err(|_| Failure::InvalidLaunch)?.len();
        if read_pe(&mut file, length)?.architecture != Architecture::X64 {
            return Err(Failure::InvalidLaunch);
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|_| Failure::InvalidLaunch)?;
        if stream_sha256(&mut file, length)? != expected_sha256 {
            return Err(Failure::InvalidLaunch);
        }
        Ok(Self {
            program,
            _file: file,
            _directories: directories,
        })
    }
    /// Keep every read lock alive through the actual creation-time Job Object operation.
    pub fn spawn(
        &self,
        args: &[String],
        cwd: &OsStr,
        stdio: [BorrowedHandle<'_>; 3],
    ) -> Result<ContainedProcess, Failure> {
        job::spawn_suspended(&self.program, args, cwd, stdio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, File, OpenOptions},
        os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle},
        ptr::null,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_WRITE, PAGE_READWRITE,
    };

    struct WritableView(windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS);
    impl Drop for WritableView {
        fn drop(&mut self) {
            // SAFETY: This wrapper owns the sole view and never forms references into it.
            unsafe { UnmapViewOfFile(self.0) };
        }
    }
    fn directory_writer(path: &std::path::Path) -> std::io::Result<File> {
        OpenOptions::new()
            .write(true)
            .share_mode(
                FILE_SHARE_READ
                    | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE
                    | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE,
            )
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    }
    fn set_junction(path: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
        use windows_sys::Win32::System::{
            Ioctl::FSCTL_SET_REPARSE_POINT, SystemServices::IO_REPARSE_TAG_MOUNT_POINT,
            IO::DeviceIoControl,
        };
        let canonical: Vec<u16> = fs::canonicalize(target)?
            .as_os_str()
            .encode_wide()
            .collect();
        let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
        if !canonical.starts_with(&prefix) {
            return Err(std::io::Error::other(
                "fixture requires a canonical local target",
            ));
        }
        let substitute: Vec<u16> = r"\??\"
            .encode_utf16()
            .chain(canonical[4..].iter().copied())
            .collect();
        // Mount-point data: four u16 offsets/lengths, substitute name + NUL,
        // and an empty NUL-terminated print name. Offsets/lengths are bytes.
        let substitute_bytes = substitute.len() * 2;
        let data_bytes = 8 + substitute_bytes + 4;
        if data_bytes + 8 > 16 * 1024 {
            return Err(std::io::Error::other(
                "fixture reparse data exceeds native cap",
            ));
        }
        let mut data = Vec::with_capacity(data_bytes + 8);
        data.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
        for value in [
            data_bytes as u16,
            0,
            0,
            substitute_bytes as u16,
            (substitute_bytes + 2) as u16,
            0,
        ] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for unit in substitute.into_iter().chain([0, 0]) {
            data.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(data.len(), data_bytes + 8);
        let directory = directory_writer(path)?;
        let mut returned = 0;
        // SAFETY: Owned non-overlapped directory handle; a bounded initialized
        // mount-point record remains live through this synchronous operation.
        if unsafe {
            DeviceIoControl(
                directory.as_raw_handle(),
                FSCTL_SET_REPARSE_POINT,
                data.as_ptr().cast(),
                data.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    fn subject() -> (tempfile::TempDir, std::path::PathBuf, String) {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("subject.exe");
        fs::copy(std::env::current_exe().unwrap(), &program).unwrap();
        let mut file = File::open(&program).unwrap();
        let length = file.metadata().unwrap().len();
        let sha = crate::image::stream_sha256(&mut file, length).unwrap();
        (root, program, sha)
    }
    #[test]
    fn holds_exact_file_and_ancestry_against_write_rename_and_hardlink_mutation() {
        let (root, program, sha) = subject();
        let alias = root.path().join("alias.exe");
        fs::hard_link(&program, &alias).unwrap();
        let image = LockedImage::open(program.as_os_str(), &sha).unwrap();
        for path in [&program, &alias] {
            assert!(OpenOptions::new().write(true).open(path).is_err());
        }
        assert!(fs::rename(&program, root.path().join("replaced.exe")).is_err());
        assert!(fs::rename(root.path(), root.path().with_extension("moved")).is_err());
        drop(image);
        assert!(OpenOptions::new().write(true).open(&program).is_ok());
    }
    #[test]
    fn refuses_junction_ancestry_and_excludes_directory_reparse_writers() {
        let (root, program, sha) = subject();
        let link = root.path().join("junction");
        fs::create_dir(&link).unwrap();
        set_junction(&link, root.path()).unwrap();
        let redirected = link.join("subject.exe");
        assert_eq!(
            fs::metadata(&redirected).unwrap().len(),
            fs::metadata(&program).unwrap().len()
        );
        assert!(matches!(
            LockedImage::open(redirected.as_os_str(), &sha),
            Err(Failure::InvalidLaunch)
        ));
        fs::remove_dir(&link).unwrap();

        let writer = directory_writer(root.path()).unwrap();
        assert!(matches!(
            LockedImage::open(program.as_os_str(), &sha),
            Err(Failure::InvalidLaunch)
        ));
        drop(writer);
        let image = LockedImage::open(program.as_os_str(), &sha).unwrap();
        assert!(directory_writer(root.path()).is_err());
        drop(image);
        assert!(directory_writer(root.path()).is_ok());
    }
    #[test]
    fn refuses_mismatch_format_and_existing_writer_without_leaking_locks() {
        let (root, program, sha) = subject();
        assert!(matches!(
            LockedImage::open(program.as_os_str(), &"0".repeat(64)),
            Err(crate::Failure::InvalidLaunch)
        ));
        let writer = OpenOptions::new().write(true).open(&program).unwrap();
        assert!(matches!(
            LockedImage::open(program.as_os_str(), &sha),
            Err(crate::Failure::InvalidLaunch)
        ));
        drop(writer);
        let invalid = root.path().join("invalid.exe");
        fs::write(&invalid, b"not an executable").unwrap();
        assert!(matches!(
            LockedImage::open(invalid.as_os_str(), &sha),
            Err(crate::Failure::InvalidLaunch)
        ));
        assert!(OpenOptions::new().write(true).open(&program).is_ok());
        assert!(matches!(
            LockedImage::open(program.as_os_str(), "PRIVATE path payload"),
            Err(crate::Failure::InvalidLaunch)
        ));
    }
    #[test]
    fn refuses_preexisting_writable_mapping_even_after_writer_handle_closes() {
        let (_root, program, sha) = subject();
        let writer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&program)
            .unwrap();
        // SAFETY: Owned regular file, unnamed read/write mapping over its existing size.
        let raw = unsafe {
            CreateFileMappingW(writer.as_raw_handle(), null(), PAGE_READWRITE, 0, 0, null())
        };
        assert!(!raw.is_null());
        // SAFETY: Successful creation returned an independently owned mapping handle.
        let mapping = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: Mapping is live; offset zero maps the copied fixture file.
        let view = unsafe { MapViewOfFile(mapping.as_raw_handle(), FILE_MAP_WRITE, 0, 0, 0) };
        assert!(!view.Value.is_null());
        let view = WritableView(view);
        drop(writer);
        let refused = matches!(
            LockedImage::open(program.as_os_str(), &sha),
            Err(crate::Failure::InvalidLaunch)
        );
        drop(view);
        drop(mapping);
        assert!(
            refused,
            "a live writable mapping must exclude an exact-image read lock"
        );
        let image = LockedImage::open(program.as_os_str(), &sha).unwrap();
        drop(image);
    }
    #[test]
    fn spawns_the_locked_image_without_entering_target_before_resume() {
        let (root, program, sha) = subject();
        let image = LockedImage::open(program.as_os_str(), &sha).unwrap();
        let null_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("NUL")
            .unwrap();
        let args = vec![
            "--ignored".into(),
            "--exact".into(),
            "image::native::tests::pinned_fixture_exits".into(),
        ];
        let mut child = image
            .spawn(
                &args,
                root.path().as_os_str(),
                [
                    null_file.as_handle(),
                    null_file.as_handle(),
                    null_file.as_handle(),
                ],
            )
            .unwrap();
        assert!(!root.path().join("image-started.marker").exists());
        child.resume().unwrap();
        assert_eq!(child.wait_for_exit(30_000).unwrap(), Some(0));
        child.terminate_and_wait().unwrap();
        drop(child);
        drop(image);
        assert_eq!(
            fs::read(root.path().join("image-started.marker")).unwrap(),
            b"started"
        );
        // Allow kernel process teardown; this is a cleanup hang guard, not a promptness claim.
        let started = Instant::now();
        loop {
            if OpenOptions::new().write(true).open(&program).is_ok() {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "image lock release hung after {:?}",
                started.elapsed()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    #[ignore = "pinned child fixture entered only by its owning native test"]
    fn pinned_fixture_exits() {
        fs::write("image-started.marker", b"started").unwrap();
        std::process::exit(0);
    }
}
