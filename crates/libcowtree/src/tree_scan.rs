// Copyright (c) 2026 Windsor Nguyen

//! Traverse a quiescent tree without following links or reading derived payloads.

use crate::{CaptureMode, Error, Result, TreeEntry, TreePolicy};
#[cfg(unix)]
use crate::{FileIdentity, Hardlinks, Operation, PathClass, TreeKind};
#[cfg(unix)]
use sha2::{Digest, Sha256};
use std::path::Path;
#[cfg(unix)]
use std::{ffi::OsString, fs, io::Read, path::PathBuf};

#[cfg(unix)]
pub fn scan_tree(root: &Path, policy: &TreePolicy, capture: CaptureMode) -> Result<Vec<TreeEntry>> {
    let metadata =
        fs::symlink_metadata(root).map_err(|error| Error::io(Operation::Inspect, root, error))?;
    if !metadata.is_dir() || metadata.is_symlink() {
        return Err(Error::InvalidSource { path: root.into() });
    }
    let mut pending = vec![PathBuf::new()];
    let mut entries = Vec::new();
    while let Some(relative) = pending.pop() {
        let directory = root.join(&relative);
        for child in fs::read_dir(&directory)
            .map_err(|error| Error::io(Operation::Inspect, &directory, error))?
        {
            let child = child.map_err(|error| Error::io(Operation::Inspect, &directory, error))?;
            let path = logical_child(&relative, child.file_name());
            let mut classification = policy.classify(path.as_os_str());
            if classification == PathClass::Ephemeral {
                if !policy.has_selected_child(path.as_os_str()) {
                    continue;
                }
                let kind = child
                    .file_type()
                    .map_err(|error| Error::io(Operation::Inspect, &child.path(), error))?;
                if !kind.is_dir() || kind.is_symlink() {
                    continue;
                }
                classification = PathClass::Derived;
            }
            let entry = capture_entry(root, &path, classification, policy, capture)?;
            if entry.kind == TreeKind::Directory {
                pending.push(path);
            }
            entries.push(entry);
        }
    }
    entries.sort_by(|left, right| path_order(&left.path, &right.path));
    Ok(entries)
}

#[cfg(not(unix))]
pub fn scan_tree(
    _root: &Path,
    _policy: &TreePolicy,
    _capture: CaptureMode,
) -> Result<Vec<TreeEntry>> {
    Err(Error::UnsupportedPlatform)
}

#[cfg(unix)]
pub(crate) fn capture_entry(
    root: &Path,
    path: &Path,
    classification: PathClass,
    policy: &TreePolicy,
    capture: CaptureMode,
) -> Result<TreeEntry> {
    use std::os::unix::fs::MetadataExt;

    let source = root.join(path);
    let before = fs::symlink_metadata(&source)
        .map_err(|error| Error::io(Operation::Inspect, &source, error))?;
    let mut link = None;
    let mut digest = None;
    let kind = if before.is_dir() {
        TreeKind::Directory
    } else if before.is_symlink() {
        let text = fs::read_link(&source)
            .map_err(|error| Error::io(Operation::ReadLink, &source, error))?;
        if classification == PathClass::Derived {
            crate::tree_links::validate(root, path, &text, policy)?;
        }
        digest = Some(hex::encode(Sha256::digest(text.as_os_str().as_encoded_bytes())));
        link = Some(text);
        TreeKind::Symlink
    } else if before.is_file() {
        if before.nlink() != 1
            && !(classification == PathClass::Derived && policy.hardlinks() == Hardlinks::Clone)
        {
            return Err(Error::Hardlink { path: path.into() });
        }
        if classification == PathClass::Source && capture == CaptureMode::Content {
            digest = Some(hash(&source)?);
        }
        TreeKind::File
    } else {
        return Err(Error::UnsupportedFile { path: path.into() });
    };
    let after = fs::symlink_metadata(&source)
        .map_err(|error| Error::io(Operation::Inspect, &source, error))?;
    if identity(&before) != identity(&after)
        || before.mode() != after.mode()
        || before.len() != after.len()
        || modified(&before) != modified(&after)
    {
        return Err(Error::SourceChanged { path: path.into() });
    }
    Ok(TreeEntry {
        path: path.into(),
        kind,
        classification,
        mode: before.mode() & 0o7777,
        size: before.len(),
        mtime_ns: modified(&before),
        digest,
        link,
        identity: if capture == CaptureMode::Metadata { Some(identity(&before)) } else { None },
    })
}

#[cfg(unix)]
fn hash(path: &Path) -> Result<String> {
    let mut source = crate::platform::open_source(path)
        .map_err(|error| Error::io(Operation::Open, path, error))?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count =
            source.read(&mut buffer).map_err(|error| Error::io(Operation::Inspect, path, error))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

#[cfg(unix)]
fn identity(metadata: &fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;
    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        changed_ns: i128::from(metadata.ctime()) * 1_000_000_000
            + i128::from(metadata.ctime_nsec()),
    }
}

#[cfg(unix)]
fn modified(metadata: &fs::Metadata) -> i128 {
    use std::os::unix::fs::MetadataExt;
    i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec())
}

#[cfg(unix)]
fn logical_child(parent: &Path, name: OsString) -> PathBuf {
    let mut path = parent.as_os_str().to_os_string();
    if !path.is_empty() {
        path.push("/");
    }
    path.push(name);
    path.into()
}

#[cfg(unix)]
fn path_order(left: &Path, right: &Path) -> std::cmp::Ordering {
    let left = left.as_os_str().as_encoded_bytes();
    let right = right.as_os_str().as_encoded_bytes();
    let scalar = |path: &[u8]| {
        path.utf8_chunks()
            .flat_map(|chunk| {
                chunk
                    .valid()
                    .chars()
                    .map(u32::from)
                    .chain(chunk.invalid().iter().map(|byte| 0xdc00 + u32::from(*byte)))
            })
            .collect::<Vec<_>>()
    };
    match (std::str::from_utf8(left), std::str::from_utf8(right)) {
        (Ok(left), Ok(right)) => left.cmp(right),
        _ => scalar(left).cmp(&scalar(right)),
    }
}
