//! One bounded scheduling snapshot per manager, never an admission cache.
use super::*;
use crate::mobile_memory::config::{validate_private_directory, validate_private_file};
use std::{fs, io, path::Path, time::SystemTime};

#[derive(PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

fn stamps(home: &Path, names: &[&str]) -> Result<Vec<Option<Stamp>>> {
    let directory = home.join("mobile");
    validate_private_directory(&directory)?;
    names
        .iter()
        .map(|name| {
            let path = directory.join(name);
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            validate_private_file(&path)?;
            Ok(Some(Stamp {
                length: metadata.len(),
                modified: metadata.modified()?,
                created: metadata.created().ok(),
                #[cfg(unix)]
                identity: {
                    use std::os::unix::fs::MetadataExt;
                    (
                        metadata.dev(),
                        metadata.ino(),
                        metadata.ctime(),
                        metadata.ctime_nsec(),
                    )
                },
            }))
        })
        .collect()
}

pub(super) struct Cache<T> {
    stamp: Option<Vec<Option<Stamp>>>,
    pub(super) value: Option<T>,
}

impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self {
            stamp: None,
            value: None,
        }
    }
}

impl<T> Cache<T> {
    pub(super) fn refresh(
        &mut self,
        home: &Path,
        names: &[&str],
        force: bool,
        load: impl FnOnce() -> Result<T>,
    ) {
        let Ok(before) = stamps(home, names) else {
            self.stamp = None;
            self.value = None;
            return;
        };
        if !force && self.stamp.as_ref() == Some(&before) {
            return;
        }
        self.value = load().ok();
        // Never label a raced read with the newer file's stamp. Discard the
        // snapshot and try on the next tick; no unbounded read/retry loop.
        if stamps(home, names).ok().as_ref() == Some(&before) {
            self.stamp = Some(before);
        } else {
            self.stamp = None;
            self.value = None;
        }
    }
}
