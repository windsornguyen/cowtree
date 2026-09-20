// Copyright (c) 2026 Windsor Nguyen

//! Populate an owned Git worktree with one native batch, excluding untracked files.
//!
//! Validate the complete path set before creating directories. Check parents in
//! traversal order, create each parent once, then clone regular files or preserve
//! link text. The caller owns the root, rollback, and final Git content check.

use crate::{Error, FileMode, Operation, Result, TrackedFile, clone_file};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

/// Fill a private destination containing at most its caller-owned `.git` entry.
pub fn populate_tracked(
    source: &Path,
    target: &Path,
    entries: &[TrackedFile],
    cancellation: &crate::Cancellation,
) -> Result<()> {
    cancellation.check()?;
    require_directory(source)?;
    require_directory(target)?;
    let source =
        source.canonicalize().map_err(|error| Error::io(Operation::Inspect, source, error))?;
    let target =
        target.canonicalize().map_err(|error| Error::io(Operation::Inspect, target, error))?;
    if source == target {
        return Err(Error::InvalidTarget { path: target });
    }
    for entry in
        fs::read_dir(&target).map_err(|error| Error::io(Operation::Inspect, &target, error))?
    {
        let entry = entry.map_err(|error| Error::io(Operation::Inspect, &target, error))?;
        if entry.file_name() != ".git" {
            return Err(Error::InvalidTarget { path: target });
        }
    }
    let parents = parents(entries)?;
    for parent in &parents {
        require_directory(&source.join(parent))?;
    }
    for parent in parents {
        cancellation.check()?;
        let path = target.join(parent);
        fs::create_dir(&path)
            .map_err(|error| Error::io(Operation::CreateDirectory, &path, error))?;
    }
    for entry in entries {
        cancellation.check()?;
        populate(&source, &target, entry)?;
    }
    cancellation.check()?;
    Ok(())
}

fn parents(entries: &[TrackedFile]) -> Result<BTreeSet<PathBuf>> {
    let mut paths = BTreeSet::new();
    let mut parents = BTreeSet::new();
    for entry in entries {
        if !paths.insert(entry.path()) {
            return Err(Error::PathConflict { path: entry.path().into() });
        }
        for parent in entry.path().ancestors().skip(1) {
            if !parent.as_os_str().is_empty() {
                parents.insert(parent.to_path_buf());
            }
        }
    }
    for parent in &parents {
        if paths.contains(parent.as_path()) {
            return Err(Error::PathConflict { path: parent.clone() });
        }
    }
    Ok(parents)
}

fn require_directory(path: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| Error::io(Operation::Inspect, path, error))?;
    if !metadata.is_dir() || metadata.is_symlink() {
        return Err(Error::InvalidSource { path: path.to_path_buf() });
    }
    Ok(())
}

fn populate(source: &Path, target: &Path, entry: &TrackedFile) -> Result<()> {
    let source = source.join(entry.path());
    let target = target.join(entry.path());
    let metadata = fs::symlink_metadata(&source)
        .map_err(|error| Error::io(Operation::Inspect, &source, error))?;
    if entry.mode() == FileMode::Symlink {
        if !metadata.is_symlink() {
            return Err(Error::ModeMismatch { path: source });
        }
        let link = fs::read_link(&source)
            .map_err(|error| Error::io(Operation::ReadLink, &source, error))?;
        return symlink(&link, &target, &metadata)
            .map_err(|error| Error::io(Operation::CreateLink, &target, error));
    }
    if !metadata.is_file() {
        return Err(Error::ModeMismatch { path: source });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if (metadata.permissions().mode() & 0o100 != 0) != (entry.mode() == FileMode::Executable) {
            return Err(Error::ModeMismatch { path: source });
        }
    }
    clone_file(&source, &target)
}

#[cfg(unix)]
fn symlink(link: &Path, target: &Path, _metadata: &fs::Metadata) -> std::io::Result<()> {
    std::os::unix::fs::symlink(link, target)
}

#[cfg(windows)]
fn symlink(link: &Path, target: &Path, metadata: &fs::Metadata) -> std::io::Result<()> {
    use std::os::windows::fs::{MetadataExt, symlink_dir, symlink_file};
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;

    if metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0 {
        symlink_dir(link, target)
    } else {
        symlink_file(link, target)
    }
}
