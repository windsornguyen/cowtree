// Copyright (c) 2026 Windsor Nguyen

//! Clone one regular file without replacing an existing destination.
//!
//! Open the source once, create the destination exclusively, then preserve its
//! metadata. A failed operation removes only the destination it owns. Callers
//! keep source writers quiescent and parent directory identities stable.

use crate::{Error, Operation, Result, platform};
use std::{fs, path::Path};

/// Create an independent copy-on-write file and preserve permissions and times.
pub fn clone_file(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| Error::io(Operation::Inspect, source, error))?;
    if !metadata.is_file() {
        return Err(Error::InvalidSource { path: source.to_path_buf() });
    }
    match fs::symlink_metadata(target) {
        Ok(_) => {
            return Err(Error::io(
                Operation::Open,
                target,
                std::io::ErrorKind::AlreadyExists.into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::io(Operation::Inspect, target, error)),
    }
    let file =
        platform::open_source(source).map_err(|error| Error::io(Operation::Open, source, error))?;
    let metadata = file.metadata().map_err(|error| Error::io(Operation::Inspect, source, error))?;
    if !metadata.is_file() {
        return Err(Error::InvalidSource { path: source.to_path_buf() });
    }
    platform::clone(&file, target, &metadata)
}

#[cfg(any(target_os = "linux", windows))]
pub(crate) fn preserve(file: &fs::File, metadata: &fs::Metadata) -> std::io::Result<()> {
    let times =
        fs::FileTimes::new().set_accessed(metadata.accessed()?).set_modified(metadata.modified()?);
    file.set_times(times)?;
    file.set_permissions(metadata.permissions())
}

pub(crate) fn cleanup(target: &Path, original: Error) -> Error {
    match fs::remove_file(target) {
        Ok(()) => original,
        Err(source) => {
            Error::Cleanup { path: target.to_path_buf(), original: Box::new(original), source }
        }
    }
}
