//! Opt-in counters at actual content-read/store-lock boundaries, per fixture.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Counts {
    pub reads: usize,
    pub locks: usize,
}
static COUNTS: LazyLock<Mutex<BTreeMap<PathBuf, Counts>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

pub(super) struct Measurement(Vec<PathBuf>);
impl Measurement {
    pub fn start(paths: Vec<PathBuf>) -> Self {
        let mut counts = COUNTS.lock().unwrap();
        for path in &paths {
            assert!(counts.insert(path.clone(), Counts::default()).is_none());
        }
        Self(paths)
    }
    pub fn count(&self, path: &Path) -> Counts {
        COUNTS.lock().unwrap()[path]
    }
}
impl Drop for Measurement {
    fn drop(&mut self) {
        let mut counts = COUNTS.lock().unwrap();
        for path in &self.0 {
            counts.remove(path);
        }
    }
}
pub(super) fn read(path: &Path) {
    if let Some(counts) = COUNTS.lock().unwrap().get_mut(path) {
        counts.reads += 1;
    }
}
pub(super) fn lock(path: &Path) {
    if let Some(counts) = COUNTS.lock().unwrap().get_mut(path) {
        counts.locks += 1;
    }
}
