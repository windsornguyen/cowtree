// Copyright (c) 2026 Windsor Nguyen

//! Populate an owned Git worktree with one native batch, excluding untracked files.
//!
//! Validate the complete path set before creating directories. Check parents in
//! traversal order, create each parent once, then clone regular files or preserve
//! link text. The caller owns the root, rollback, and final Git content check.

use crate::{Error, FileMode, Operation, Result, TrackedFile, clone::clone_regular};
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
    create_parents(&target, parents, cancellation)?;
    clone_entries(entries, |entry| {
        cancellation.check()?;
        populate(&source, &target, entry)
    })?;
    cancellation.check()?;
    Ok(())
}

/// Join every clone worker before the caller may roll back its owned directory.
fn clone_entries(
    entries: &[TrackedFile],
    copy: impl Fn(&TrackedFile) -> Result<()> + Sync,
) -> Result<()> {
    // Four workers won the APFS 512/8192-file sweep. More increased contention.
    // See docs/native-profiling.rst. Recheck on a different clone backend.
    let mut batches = entries.chunks(entries.len().div_ceil(4).max(1));
    let Some(first) = batches.next() else {
        return Ok(());
    };
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        let copy = &copy;
        for batch in batches {
            let worker = std::thread::Builder::new()
                .name("cowtree-clone".into())
                .spawn_scoped(scope, move || batch.iter().try_for_each(copy))
                .map_err(Error::WorkerStart)?;
            workers.push(worker);
        }
        let mut result = first.iter().try_for_each(copy);
        for worker in workers {
            let outcome = match worker.join() {
                Ok(outcome) => outcome,
                Err(panic) => std::panic::resume_unwind(panic),
            };
            result = result.and(outcome);
        }
        result
    })
}

pub(crate) fn parents(entries: &[TrackedFile]) -> Result<BTreeSet<PathBuf>> {
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

pub(crate) fn create_parents(
    target: &Path,
    parents: BTreeSet<PathBuf>,
    cancellation: &crate::Cancellation,
) -> Result<()> {
    for parent in parents {
        cancellation.check()?;
        let path = target.join(parent);
        fs::create_dir(&path)
            .map_err(|error| Error::io(Operation::CreateDirectory, &path, error))?;
    }
    Ok(())
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
    clone_regular(&source, &target, &metadata)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Mutex, mpsc},
        time::Duration,
    };

    #[test]
    fn failed_population_joins_an_inflight_clone_before_returning()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        if !crate::inspect_path(root.path())?.supported() {
            return Ok(());
        }
        let source = root.path().join("source");
        let target = root.path().join("target");
        fs::create_dir(&source)?;
        fs::create_dir(&target)?;
        fs::write(source.join("blocked"), b"independent bytes")?;
        let entries = [
            TrackedFile::new("missing".into(), FileMode::Regular)?,
            TrackedFile::new("blocked".into(), FileMode::Regular)?,
        ];
        let (entered, started) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let wait = Mutex::new(wait);
        let (finished, result) = mpsc::channel();
        std::thread::scope(|scope| -> std::result::Result<(), Box<dyn std::error::Error>> {
            scope.spawn(|| {
                let copied = clone_entries(&entries, |entry| {
                    if entry.path() == Path::new("blocked") {
                        entered.send(()).map_err(|error| channel_error(error.to_string()))?;
                        wait.lock()
                            .map_err(|error| channel_error(error.to_string()))?
                            .recv_timeout(Duration::from_secs(5))
                            .map_err(|error| channel_error(error.to_string()))?;
                    }
                    populate(&source, &target, entry)
                });
                let _ = finished.send(copied);
            });
            started.recv_timeout(Duration::from_secs(5))?;
            let premature = result.recv_timeout(Duration::from_millis(50));
            release.send(())?;
            assert!(matches!(premature, Err(mpsc::RecvTimeoutError::Timeout)));
            assert!(result.recv_timeout(Duration::from_secs(5))?.is_err());
            assert_eq!(fs::read(target.join("blocked"))?, b"independent bytes");
            Ok(())
        })
    }

    fn channel_error(message: String) -> Error {
        Error::io(
            Operation::Clone,
            Path::new("test synchronization"),
            std::io::Error::other(message),
        )
    }
}
