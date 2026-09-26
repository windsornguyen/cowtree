// Copyright (c) 2026 Windsor Nguyen

//! Verify physical file images independently of their snapshot access policy.

use std::{
    fs::{self, File},
    io::Read,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt, symlink},
    },
    path::Path,
    time::SystemTime,
};

use cowtree_metadata::{Entry, EntryKind, FileKind, objects::ObjectId};
use sha2::{Digest, Sha256};

use crate::{Error, Result, error::Issue, records};

pub(crate) fn same(actual: Option<&Entry>, expected: Option<&Entry>) -> bool {
    match (actual, expected) {
        (None, None) => true,
        (Some(actual), Some(expected)) => {
            actual.object == expected.object && actual.kind.file_kind() == expected.kind.file_kind()
        }
        _ => false,
    }
}

pub(crate) fn fingerprint(path: &Path) -> Result<Option<Entry>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(Error::io(path, error)),
    };
    if metadata.is_dir() {
        return Ok(None);
    }
    if metadata.is_symlink() {
        let link = fs::read_link(path).map_err(|error| Error::io(path, error))?;
        return Ok(Some(Entry {
            object: ObjectId::from_bytes(link.as_os_str().as_bytes()),
            kind: EntryKind::Symlink,
        }));
    }
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(Issue::UnsupportedEntry.at(path));
    }
    let file = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
    )
    .map_err(|error| Error::io(path, error.into()))?;
    let mut file = File::from(file);
    let actual = file.metadata().map_err(|error| Error::io(path, error))?;
    if !actual.is_file()
        || actual.nlink() != 1
        || actual.dev() != metadata.dev()
        || actual.ino() != metadata.ino()
    {
        return Err(Issue::SourceChanged.at(path));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| Error::io(path, error))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let kind = if metadata.mode() & 0o100 != 0 { EntryKind::Executable } else { EntryKind::File };
    Ok(Some(Entry { object: ObjectId::parse(&hex::encode(digest.finalize()))?, kind }))
}

pub(crate) fn save(source: &Path, target: &Path, entry: &Entry) -> Result<()> {
    if entry.kind.file_kind() == FileKind::Symlink {
        let link = fs::read_link(source).map_err(|error| Error::io(source, error))?;
        symlink(link, target).map_err(|error| Error::io(target, error))?;
    } else {
        cowtree::clone_file(source, target)?;
        sync(target)?;
    }
    if !same(fingerprint(target)?.as_ref(), Some(entry)) {
        return Err(Issue::SourceChanged.at(source));
    }
    Ok(())
}

pub(crate) fn stage(source: &Path, target: &Path, entry: &Entry) -> Result<()> {
    match entry.kind.file_kind() {
        FileKind::Symlink => {
            let bytes = fs::read(source).map_err(|error| Error::io(source, error))?;
            if ObjectId::from_bytes(&bytes) != entry.object {
                return Err(Issue::CorruptObject.at(source));
            }
            let link = std::str::from_utf8(&bytes).map_err(|_| Issue::NonUtf8.at(source))?;
            symlink(link, target).map_err(|error| Error::io(target, error))?;
        }
        kind => {
            cowtree::clone_file(source, target)?;
            let mode = if kind == FileKind::Executable { 0o755 } else { 0o644 };
            fs::set_permissions(target, fs::Permissions::from_mode(mode))
                .map_err(|error| Error::io(target, error))?;
            let file = File::open(target).map_err(|error| Error::io(target, error))?;
            let now = SystemTime::now();
            file.set_times(fs::FileTimes::new().set_accessed(now).set_modified(now))
                .map_err(|error| Error::io(target, error))?;
            records::sync_file(&file).map_err(|error| Error::io(target, error))?;
        }
    }
    if !same(fingerprint(target)?.as_ref(), Some(entry)) {
        return Err(Issue::CorruptObject.at(source));
    }
    Ok(())
}

pub(crate) fn sync(path: &Path) -> Result<()> {
    let file = File::open(path).map_err(|error| Error::io(path, error))?;
    records::sync_file(&file).map_err(|error| Error::io(path, error))
}
