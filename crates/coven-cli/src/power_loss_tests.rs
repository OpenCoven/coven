//! Test VFS: retain only images observed at successful SQLite xSync calls.
//! Restoring those images discards an unsynced WAL suffix, unlike killing a process.
use anyhow::{Context, Result};
use rusqlite::ffi;
use std::{
    collections::BTreeMap,
    ffi::CStr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Default)]
struct Capture {
    images: BTreeMap<PathBuf, Vec<u8>>,
    fail_sync: bool,
    sync_flags: Vec<i32>,
}
type Images = Arc<Mutex<Capture>>;
struct ObservedFile {
    original: usize,
    methods: Box<ffi::sqlite3_io_methods>,
    path: PathBuf,
    images: Images,
}
fn files() -> &'static Mutex<BTreeMap<usize, ObservedFile>> {
    static FILES: OnceLock<Mutex<BTreeMap<usize, ObservedFile>>> = OnceLock::new();
    FILES.get_or_init(Default::default)
}
fn captures() -> &'static Mutex<BTreeMap<PathBuf, Images>> {
    static CAPTURES: OnceLock<Mutex<BTreeMap<PathBuf, Images>>> = OnceLock::new();
    CAPTURES.get_or_init(Default::default)
}
static ORIGINAL_VFS: OnceLock<usize> = OnceLock::new();

// The wrapper retains the native sqlite3_file layout and delegates every method
// except sync/close unchanged. Its copied method table lives until native close.
unsafe extern "C" fn open(
    _vfs: *mut ffi::sqlite3_vfs,
    name: *const std::ffi::c_char,
    file: *mut ffi::sqlite3_file,
    flags: i32,
    out_flags: *mut i32,
) -> i32 {
    let original = *ORIGINAL_VFS.get().unwrap() as *mut ffi::sqlite3_vfs;
    let result = ((*original).xOpen.unwrap())(original, name, file, flags, out_flags);
    if result != ffi::SQLITE_OK || name.is_null() {
        return result;
    }
    let path = normalize(&PathBuf::from(
        CStr::from_ptr(name).to_string_lossy().as_ref(),
    ));
    let images = captures().lock().unwrap().get(&path).cloned();
    if let Some(images) = images {
        let original = (*file).pMethods;
        let mut methods = Box::new(std::ptr::read(original));
        methods.xSync = Some(sync);
        methods.xClose = Some(close);
        (*file).pMethods = &*methods;
        files().lock().unwrap().insert(
            file as usize,
            ObservedFile {
                original: original as usize,
                methods,
                path,
                images,
            },
        );
    }
    result
}
unsafe extern "C" fn sync(file: *mut ffi::sqlite3_file, flags: i32) -> i32 {
    let (original, path, images) = {
        let files = files().lock().unwrap();
        let observed = files.get(&(file as usize)).unwrap();
        (
            observed.original as *const ffi::sqlite3_io_methods,
            observed.path.clone(),
            Arc::clone(&observed.images),
        )
    };
    if std::mem::take(&mut images.lock().unwrap().fail_sync) {
        return ffi::SQLITE_IOERR_FSYNC;
    }
    let result = ((*original).xSync.unwrap())(file, flags);
    if result != ffi::SQLITE_OK {
        return result;
    }
    let mut size = 0;
    let result = ((*original).xFileSize.unwrap())(file, &mut size);
    if result != ffi::SQLITE_OK {
        return result;
    }
    let Ok(size) = i32::try_from(size) else {
        return ffi::SQLITE_IOERR;
    };
    let mut bytes = vec![0; size as usize];
    if size > 0 {
        let result = ((*original).xRead.unwrap())(file, bytes.as_mut_ptr().cast(), size, 0);
        if result != ffi::SQLITE_OK {
            return result;
        }
    }
    let mut capture = images.lock().unwrap();
    capture.images.insert(path, bytes);
    capture.sync_flags.push(flags);
    ffi::SQLITE_OK
}
unsafe extern "C" fn close(file: *mut ffi::sqlite3_file) -> i32 {
    let observed = files().lock().unwrap().remove(&(file as usize)).unwrap();
    let original = observed.original as *const ffi::sqlite3_io_methods;
    (*file).pMethods = original;
    let result = ((*original).xClose.unwrap())(file);
    drop(observed.methods);
    result
}

pub(crate) struct PowerLoss {
    database: PathBuf,
    wal: PathBuf,
    images: Images,
}
impl PowerLoss {
    pub(crate) fn observe(database: &Path) -> Self {
        static REGISTER: OnceLock<()> = OnceLock::new();
        REGISTER.get_or_init(|| unsafe {
            assert_eq!(ffi::sqlite3_initialize(), ffi::SQLITE_OK);
            let original = ffi::sqlite3_vfs_find(std::ptr::null());
            assert!(!original.is_null());
            ORIGINAL_VFS.set(original as usize).unwrap();
            let mut wrapper = Box::new(std::ptr::read(original));
            wrapper.zName = c"coven-power-loss-test".as_ptr();
            wrapper.pNext = std::ptr::null_mut();
            wrapper.xOpen = Some(open);
            // SQLite retains this test-only registration for the process lifetime.
            assert_eq!(
                ffi::sqlite3_vfs_register(Box::into_raw(wrapper), 1),
                ffi::SQLITE_OK
            );
        });
        let database = normalize(database);
        let wal = with_suffix(&database, "-wal");
        let images = Images::default();
        let mut captures = captures().lock().unwrap();
        captures.insert(database.clone(), Arc::clone(&images));
        captures.insert(wal.clone(), Arc::clone(&images));
        Self {
            database,
            wal,
            images,
        }
    }
    /// Freeze before dropping connections: a graceful close itself checkpoints.
    pub(crate) fn synced_images(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        self.images.lock().unwrap().images.clone()
    }
    pub(crate) fn fail_next_sync(&self) {
        self.images.lock().unwrap().fail_sync = true;
    }
    pub(crate) fn sync_flags(&self) -> Vec<i32> {
        self.images.lock().unwrap().sync_flags.clone()
    }
    pub(crate) fn restore(&self, images: BTreeMap<PathBuf, Vec<u8>>) -> Result<()> {
        anyhow::ensure!(
            !files()
                .lock()
                .unwrap()
                .values()
                .any(|file| Arc::ptr_eq(&file.images, &self.images)),
            "close every connection before restoring the power-loss image"
        );
        std::fs::write(
            &self.database,
            images
                .get(&self.database)
                .context("database was never synced")?,
        )?;
        match images.get(&self.wal) {
            Some(bytes) => std::fs::write(&self.wal, bytes)?,
            None => {
                let _ = std::fs::remove_file(&self.wal);
            }
        }
        let _ = std::fs::remove_file(with_suffix(&self.database, "-shm"));
        Ok(())
    }
}
impl Drop for PowerLoss {
    fn drop(&mut self) {
        let mut captures = captures().lock().unwrap();
        captures.remove(&self.database);
        captures.remove(&self.wal);
    }
}

fn normalize(path: &Path) -> PathBuf {
    path.parent()
        .unwrap()
        .canonicalize()
        .unwrap()
        .join(path.file_name().unwrap())
}
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}
