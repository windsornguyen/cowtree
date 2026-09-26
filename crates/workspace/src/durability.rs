// Copyright (c) 2026 Windsor Nguyen

//! Flush a capture's files and parents before publishing its record or directory.

use std::{
    fs::{self, File},
    os::unix::fs::MetadataExt,
    path::Path,
};

use rustix::fs::{CWD, Mode, OFlags, RenameFlags};

use crate::{Error, Result, error::Issue, records};

pub(crate) fn publish_directory(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source).map_err(|error| Error::io(source, error))?;
    if !metadata.is_dir() {
        return Err(Issue::InvalidRoot.at(source));
    }
    let flags = RenameFlags::NOREPLACE;
    rustix::fs::renameat_with(CWD, source, CWD, target, flags)
        .map_err(|error| Error::io(target, error.into()))?;
    if let Some(parent) = target.parent() {
        records::sync_directory(parent)?;
    }
    if source.parent() != target.parent() {
        if let Some(parent) = source.parent() {
            records::sync_directory(parent)?;
        }
    }
    Ok(())
}

pub(crate) fn sync_tree<'a>(
    root: &Path,
    files: impl IntoIterator<Item = &'a Path>,
    directories: impl IntoIterator<Item = &'a Path>,
) -> Result<()> {
    let root_file = open(root, true)?;
    let device = root_file.metadata().map_err(|error| Error::io(root, error))?.dev();
    for path in files {
        flush(path, device, false)?;
    }
    for path in directories {
        flush(path, device, true)?;
    }
    crate::fault::io_error("tree-sync").map_err(|error| Error::io(root, error))?;
    root_file.sync_all().map_err(|error| Error::io(root, error))?;
    #[cfg(target_os = "macos")]
    rustix::fs::fcntl_fullfsync(&root_file).map_err(|error| Error::io(root, error.into()))?;
    Ok(())
}

fn open(path: &Path, directory: bool) -> Result<File> {
    let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    if directory {
        flags |= OFlags::DIRECTORY;
    }
    rustix::fs::open(path, flags, Mode::empty())
        .map(File::from)
        .map_err(|error| Error::io(path, error.into()))
}

fn flush(path: &Path, device: u64, directory: bool) -> Result<()> {
    let file = open(path, directory)?;
    let metadata = file.metadata().map_err(|error| Error::io(path, error))?;
    if metadata.dev() != device {
        return Err(Error::io(path, std::io::ErrorKind::CrossesDevices.into()));
    }
    if !directory && !metadata.is_file() {
        return Err(Issue::UnsupportedEntry.at(path));
    }
    file.sync_all().map_err(|error| Error::io(path, error))
}
