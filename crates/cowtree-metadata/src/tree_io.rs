// Copyright (c) 2026 Windsor Nguyen

//! Validate physical paths without following symlink ancestors; materialize owned files.

use crate::{Entry, EntryKind, Error, ResourcePath, Result, objects::ObjectId};
use std::{
    fs, io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub(crate) fn io(path: &Path, source: io::Error) -> Error {
    Error::Io { path: path.into(), source }
}
pub(crate) fn sync(path: &Path) -> Result<()> {
    crate::durability::sync_directory(path).map_err(|source| io(path, source))
}
pub(crate) fn identity(root: &Path) -> Result<(i64, i64)> {
    let metadata = fs::symlink_metadata(root).map_err(|source| io(root, source))?;
    if !metadata.is_dir() {
        return Err(Error::BindingChanged(root.display().to_string()));
    }
    let device = i64::try_from(metadata.dev())
        .map_err(|_| Error::BindingChanged(root.display().to_string()))?;
    let inode = i64::try_from(metadata.ino())
        .map_err(|_| Error::BindingChanged(root.display().to_string()))?;
    Ok((device, inode))
}
pub(crate) fn checked(root: &Path, path: &ResourcePath) -> Result<PathBuf> {
    let mut cursor = root.to_path_buf();
    let parts: Vec<_> = path.as_str().split('/').collect();
    for part in &parts[..parts.len() - 1] {
        cursor.push(part);
        match fs::symlink_metadata(&cursor) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(Error::PathConflict(path.as_str().into())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(io(&cursor, source)),
        }
    }
    Ok(root.join(path.as_str()))
}
pub(crate) fn read(
    root: &Path,
    path: &ResourcePath,
    limit: u64,
) -> Result<Option<(Entry, Vec<u8>)>> {
    let full = match checked(root, path) {
        // A file or symlink is a terminal entry in our logical tree; never follow it.
        Err(Error::PathConflict(_)) => return Ok(None),
        result => result?,
    };
    let metadata = match fs::symlink_metadata(&full) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io(&full, source)),
    };
    if metadata.is_dir() {
        return Ok(None);
    }
    if metadata.len() > limit {
        return Err(Error::Limit(crate::LimitKind::ObjectBytes));
    }
    let (kind, bytes) = if metadata.file_type().is_symlink() {
        let target = fs::read_link(&full).map_err(|source| io(&full, source))?;
        let value = target.to_str().ok_or_else(|| Error::InvalidSymlink(path.as_str().into()))?;
        (EntryKind::Symlink, value.as_bytes().to_vec())
    } else if metadata.is_file() && metadata.nlink() == 1 {
        let kind = if metadata.permissions().mode() & 0o111 != 0 {
            EntryKind::Executable
        } else {
            EntryKind::File
        };
        (kind, fs::read(&full).map_err(|source| io(&full, source))?)
    } else {
        return Err(Error::PathConflict(path.as_str().into()));
    };
    if bytes.len() as u64 > limit {
        return Err(Error::Limit(crate::LimitKind::ObjectBytes));
    }
    Ok(Some((Entry { object: ObjectId::from_bytes(&bytes), kind }, bytes)))
}
pub(crate) fn parents(root: &Path, path: &ResourcePath) -> Result<()> {
    let full = checked(root, path)?;
    let parent = full.parent().ok_or_else(|| Error::PathConflict(path.as_str().into()))?;
    let relative =
        parent.strip_prefix(root).map_err(|_| Error::PathConflict(path.as_str().into()))?;
    let mut cursor = root.to_path_buf();
    for component in relative.components() {
        let previous = cursor.clone();
        cursor.push(component);
        match fs::create_dir(&cursor) {
            Ok(()) => sync(&previous)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if !fs::symlink_metadata(&cursor).map_err(|source| io(&cursor, source))?.is_dir() {
                    return Err(Error::PathConflict(path.as_str().into()));
                }
            }
            Err(source) => return Err(io(&cursor, source)),
        }
    }
    Ok(())
}
pub(crate) fn remove_empty_parents(root: &Path, path: &Path) -> Result<()> {
    let mut cursor = path.parent();
    while let Some(directory) = cursor {
        if directory == root {
            break;
        }
        match fs::remove_dir(directory) {
            Ok(()) => {
                if let Some(parent) = directory.parent() {
                    sync(parent)?;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::NotFound
                ) =>
            {
                break;
            }
            Err(source) => return Err(io(directory, source)),
        }
        cursor = directory.parent();
    }
    Ok(())
}

/// Ask the native filesystem about names that may alias; ASCII-distinct lower-case groups need no probe.
pub(crate) fn validate_names(authority: &Path, snapshot: &crate::Snapshot) -> Result<()> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in snapshot.keys() {
        let mut parent = String::new();
        for component in path.as_str().split('/') {
            groups.entry(parent.clone()).or_default().insert(component.into());
            if !parent.is_empty() {
                parent.push('/');
            }
            parent.push_str(component);
        }
    }
    // Git control components are never published, but their native aliases are also reserved.
    for names in groups.values_mut() {
        names.insert(".git".into());
    }
    for names in groups.values() {
        let folded: BTreeSet<_> = names.iter().map(|name| name.to_ascii_lowercase()).collect();
        if names.iter().all(|name| name.is_ascii()) && folded.len() == names.len() {
            continue;
        }
        let probe = tempfile::Builder::new()
            .prefix(".namespace-")
            .tempdir_in(authority)
            .map_err(|source| io(authority, source))?;
        for name in names {
            let path = probe.path().join(name);
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    return Err(Error::PathConflict(name.clone()));
                }
                Err(source) => return Err(io(&path, source)),
            }
        }
    }
    Ok(())
}

/// Refuse a missing native CoW primary before an installation or import gets journaled.
pub(crate) fn probe_clone(authority: &Path) -> Result<()> {
    let probe = tempfile::Builder::new()
        .prefix(".reflink-probe-")
        .tempdir_in(authority)
        .map_err(|source| io(authority, source))?;
    let source = probe.path().join("source");
    let target = probe.path().join("target");
    fs::write(&source, b"cowtree").map_err(|error| io(&source, error))?;
    reflink_copy::reflink(&source, &target)
        .map_err(|source| Error::UnsupportedFilesystem { path: authority.into(), source })
}
