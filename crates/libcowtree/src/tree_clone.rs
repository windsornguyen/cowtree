// Copyright (c) 2026 Windsor Nguyen

//! Materialize a captured tree and reject mutations before returning its manifest.

use crate::{CaptureMode, Error, Result, TreeEntry, TreePolicy};
#[cfg(unix)]
use crate::{Operation, TreeKind};
#[cfg(unix)]
use std::fs;
use std::path::Path;

#[cfg(unix)]
pub fn clone_tree(source: &Path, target: &Path, policy: &TreePolicy) -> Result<Vec<TreeEntry>> {
    clone_with(source, target, policy, crate::clone_file)
}

#[cfg(unix)]
fn clone_with(
    source: &Path,
    target: &Path,
    policy: &TreePolicy,
    clone: impl Fn(&Path, &Path) -> Result<()>,
) -> Result<Vec<TreeEntry>> {
    let source =
        source.canonicalize().map_err(|error| Error::io(Operation::Inspect, source, error))?;
    let target = std::path::absolute(target)
        .map_err(|error| Error::io(Operation::Inspect, target, error))?;
    if target.starts_with(&source) {
        return Err(Error::InvalidTarget { path: target });
    }
    fs::create_dir(&target)
        .map_err(|error| Error::io(Operation::CreateDirectory, &target, error))?;
    match populate_with(&source, &target, policy, CaptureMode::Content, clone) {
        Ok(entries) => Ok(entries),
        Err(original) => match fs::remove_dir_all(&target) {
            Ok(()) => Err(original),
            Err(source) => {
                Err(Error::Cleanup { path: target, original: Box::new(original), source })
            }
        },
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::clone_with;
    use crate::{Error, Operation, TreePolicy};
    use std::{error::Error as StdError, fs, path::Path};

    fn corrupt(source: &Path, target: &Path) -> crate::Result<()> {
        crate::clone_file(source, target)?;
        let metadata =
            fs::metadata(target).map_err(|error| Error::io(Operation::Inspect, target, error))?;
        fs::write(target, b"changed\n")
            .map_err(|error| Error::io(Operation::Clone, target, error))?;
        let output =
            fs::File::open(target).map_err(|error| Error::io(Operation::Open, target, error))?;
        let times = fs::FileTimes::new().set_modified(
            metadata.modified().map_err(|error| Error::io(Operation::Inspect, target, error))?,
        );
        output.set_times(times).map_err(|error| Error::io(Operation::Metadata, target, error))?;
        Ok(())
    }

    #[test]
    fn corrupted_clone_never_becomes_a_completed_tree() -> Result<(), Box<dyn StdError>> {
        let directory = tempfile::tempdir()?;
        let report = crate::inspect_path(directory.path())?;
        if std::env::var_os("COWTREE_EXPECT_SUPPORTED").is_some_and(|value| value == "1") {
            assert!(report.supported());
        }
        if !report.supported() {
            return Ok(());
        }
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        fs::create_dir(&source)?;
        fs::write(source.join("file"), b"original")?;
        let result = clone_with(&source, &target, &TreePolicy::default(), corrupt);
        assert!(matches!(result, Err(Error::SourceChanged { .. })));
        assert!(!target.exists());
        assert_eq!(fs::read(source.join("file"))?, b"original");
        Ok(())
    }
}

#[cfg(not(unix))]
pub fn clone_tree(_source: &Path, _target: &Path, _policy: &TreePolicy) -> Result<Vec<TreeEntry>> {
    Err(Error::UnsupportedPlatform)
}

#[cfg(unix)]
pub fn populate_tree(
    source: &Path,
    target: &Path,
    policy: &TreePolicy,
    capture: CaptureMode,
) -> Result<Vec<TreeEntry>> {
    populate_with(source, target, policy, capture, crate::clone_file)
}

#[cfg(not(unix))]
pub fn populate_tree(
    _source: &Path,
    _target: &Path,
    _policy: &TreePolicy,
    _capture: CaptureMode,
) -> Result<Vec<TreeEntry>> {
    Err(Error::UnsupportedPlatform)
}

#[cfg(unix)]
fn populate_with(
    source: &Path,
    target: &Path,
    policy: &TreePolicy,
    capture: CaptureMode,
    clone: impl Fn(&Path, &Path) -> Result<()>,
) -> Result<Vec<TreeEntry>> {
    use std::os::unix::fs::PermissionsExt;

    check_target(source, target)?;
    let entries = crate::scan_tree(source, policy, capture)?;
    for entry in &entries {
        let destination = target.join(&entry.path);
        match entry.kind {
            TreeKind::Directory => fs::create_dir(&destination)
                .map_err(|error| Error::io(Operation::CreateDirectory, &destination, error))?,
            TreeKind::Symlink => {
                let link = entry
                    .link
                    .as_ref()
                    .ok_or_else(|| Error::SourceChanged { path: entry.path.clone() })?;
                std::os::unix::fs::symlink(link, &destination)
                    .map_err(|error| Error::io(Operation::CreateLink, &destination, error))?;
            }
            TreeKind::File => {
                clone(&source.join(&entry.path), &destination)?;
                let copied = crate::tree_scan::capture_entry(
                    target,
                    &entry.path,
                    entry.classification,
                    policy,
                    capture,
                )?;
                if copied != *entry {
                    return Err(Error::SourceChanged { path: entry.path.clone() });
                }
            }
        }
    }
    let after = crate::scan_tree(source, policy, capture)?;
    if after != entries
        || (capture == CaptureMode::Metadata
            && entries.iter().zip(&after).any(|(before, after)| before.identity != after.identity))
    {
        return Err(Error::SourceChanged { path: source.into() });
    }
    for entry in entries.iter().rev().filter(|entry| entry.kind == TreeKind::Directory) {
        let directory = target.join(&entry.path);
        fs::set_permissions(&directory, fs::Permissions::from_mode(entry.mode))
            .map_err(|error| Error::io(Operation::Metadata, &directory, error))?;
        let time = timestamp(entry.mtime_ns)?;
        let times = rustix::fs::Timestamps { last_access: time, last_modification: time };
        rustix::fs::utimensat(rustix::fs::CWD, &directory, &times, rustix::fs::AtFlags::empty())
            .map_err(|error| Error::io(Operation::Metadata, &directory, error.into()))?;
    }
    Ok(entries)
}

#[cfg(unix)]
fn check_target(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(target)
        .map_err(|error| Error::io(Operation::Inspect, target, error))?;
    let source =
        source.canonicalize().map_err(|error| Error::io(Operation::Inspect, source, error))?;
    let resolved =
        target.canonicalize().map_err(|error| Error::io(Operation::Inspect, target, error))?;
    if metadata.is_symlink() || !metadata.is_dir() || resolved.starts_with(&source) {
        return Err(Error::InvalidTarget { path: target.into() });
    }
    for child in
        fs::read_dir(target).map_err(|error| Error::io(Operation::Inspect, target, error))?
    {
        let child = child.map_err(|error| Error::io(Operation::Inspect, target, error))?;
        if child.file_name() != ".git" {
            return Err(Error::TargetContainsData { path: target.into() });
        }
    }
    Ok(())
}

#[cfg(unix)]
fn timestamp(nanoseconds: i128) -> Result<rustix::fs::Timespec> {
    let seconds = i64::try_from(nanoseconds.div_euclid(1_000_000_000)).map_err(|error| {
        Error::io(Operation::Metadata, Path::new("."), std::io::Error::other(error))
    })?;
    let fraction = i64::try_from(nanoseconds.rem_euclid(1_000_000_000)).map_err(|error| {
        Error::io(Operation::Metadata, Path::new("."), std::io::Error::other(error))
    })?;
    Ok(rustix::fs::Timespec { tv_sec: seconds, tv_nsec: fraction })
}
