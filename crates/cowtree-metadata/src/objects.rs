// Copyright (c) 2026 Windsor Nguyen

//! Immutable content-addressed bytes owned by a trusted coordinator.
//!
//! 1. The coordinator establishes a construction pin before calling `put`.
//! 2. Hold a shared directory lock while writing and syncing a temporary file.
//! 3. Publish its digest name with an exclusive hard link, then sync the directory.
//! 4. Verify bytes on every read and before accepting an existing digest name.
//!
//! The coordinator owns retention and excludes collection while readers or builders
//! hold pins. This module never interprets metadata or promises retention itself.
//! Only the coordinator may mutate the directory. Interrupted writes may leave
//! `.pending-` files, which are excluded from the published object inventory.
//! Temporary collection takes the exclusive directory lock, so abandoned temporary
//! files can be removed even when their content hash remains pinned. Lock ordering
//! is metadata then directory: `put` never enters SQLite while holding its lock.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use rustix::fs::FlockOperation;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::durability::{sync_directory, sync_file};

/// Canonical lowercase SHA-256 of an object's complete byte sequence.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ObjectId(String);

impl ObjectId {
    /// Parse exactly 64 lowercase hexadecimal digits.
    pub fn parse(value: &str) -> Result<Self, ObjectError> {
        if value.len() != 64
            || !value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ObjectError::InvalidIdentifier { value: value.to_owned() });
        }
        Ok(Self(value.to_owned()))
    }

    /// Compute the identity of an entire object.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }

    /// Return the canonical digest used as the published filename.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ObjectId {
    type Error = ObjectError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ObjectId> for String {
    fn from(value: ObjectId) -> Self {
        value.0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Object storage failures preserve the relevant path or expected identity.
#[derive(Debug, Error)]
pub enum ObjectError {
    /// A digest was not canonical lowercase SHA-256.
    #[error("invalid object identifier: {value:?}")]
    InvalidIdentifier {
        /// Rejected identifier.
        value: String,
    },
    /// A published object is absent; removal is deliberately not idempotent.
    #[error("object {id} is missing at {path}")]
    Missing {
        /// Missing content identity.
        id: ObjectId,
        /// Expected object path.
        path: PathBuf,
    },
    /// Persisted bytes disagree with their name.
    #[error("object at {path} has digest {actual}, expected {expected}")]
    Corrupt {
        /// Path whose contents failed verification.
        path: PathBuf,
        /// Digest encoded in the object's name.
        expected: ObjectId,
        /// Digest computed from the current bytes.
        actual: ObjectId,
    },
    /// An unexpected filename or file type occupies the owned directory.
    #[error("unexpected object store entry: {path}")]
    UnexpectedEntry {
        /// Invalid entry, including directories and symbolic links.
        path: PathBuf,
    },
    /// A filesystem operation failed.
    #[error("object store I/O at {path}: {source}")]
    Io {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Original operating-system error.
        #[source]
        source: io::Error,
    },
}

/// Immutable objects; metadata transactions and pins belong to the coordinator.
#[derive(Clone, Debug)]
pub struct ObjectStore {
    /// Directory containing digest-named files and temporary publication files.
    root: PathBuf,
}

impl ObjectStore {
    /// Open or create an owned object directory under an existing parent directory.
    pub fn open(root: PathBuf) -> Result<Self, ObjectError> {
        match fs::create_dir(&root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(ObjectError::Io { path: root, source }),
        }
        let metadata = fs::symlink_metadata(&root)
            .map_err(|source| ObjectError::Io { path: root.clone(), source })?;
        if !metadata.is_dir() {
            return Err(ObjectError::UnexpectedEntry { path: root });
        }
        let root = fs::canonicalize(&root)
            .map_err(|source| ObjectError::Io { path: root.clone(), source })?;
        let parent =
            root.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
        sync_directory(parent)
            .map_err(|source| ObjectError::Io { path: parent.to_owned(), source })?;
        Ok(Self { root })
    }

    /// Publish durable bytes while the caller holds a construction pin.
    ///
    /// A failed flush may leave a published but unreferenced object. The caller must
    /// not commit a reference when this returns an error. Collection owns orphans.
    pub fn put(&self, bytes: &[u8]) -> Result<ObjectId, ObjectError> {
        let id = ObjectId::from_bytes(bytes);
        let target = self.path(&id);
        let _publication = self.lock(FlockOperation::LockShared)?;
        let mut temporary = tempfile::Builder::new()
            .prefix(&format!(".pending-{id}-"))
            .tempfile_in(&self.root)
            .map_err(|source| ObjectError::Io { path: self.root.clone(), source })?;
        let temporary_path = temporary.path().to_owned();
        crate::fault::checkpoint("after-object-temp-created");
        temporary
            .write_all(bytes)
            .and_then(|()| sync_file(temporary.as_file()))
            .map_err(|source| ObjectError::Io { path: temporary_path.clone(), source })?;
        match fs::hard_link(&temporary_path, &target) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let (file, _) = self.verified(&id)?;
                sync_file(&file)
                    .map_err(|source| ObjectError::Io { path: target.clone(), source })?;
            }
            Err(source) => return Err(ObjectError::Io { path: target, source }),
        }
        temporary.close().map_err(|source| ObjectError::Io { path: temporary_path, source })?;
        sync_directory(&self.root)
            .map_err(|source| ObjectError::Io { path: self.root.clone(), source })?;
        Ok(id)
    }

    /// Publish one bounded group while the caller excludes collection.
    ///
    /// Every payload completes file writeout. On macOS the final directory flush
    /// also flushes the device cache for the group; callers commit progress afterward.
    /// Existing verified objects are flushed without rewriting a temporary duplicate.
    pub(crate) fn put_batch(&self, payloads: &[Vec<u8>]) -> Result<(), ObjectError> {
        let _publication = self.lock(FlockOperation::LockShared)?;
        for bytes in payloads {
            let id = ObjectId::from_bytes(bytes);
            let target = self.path(&id);
            match self.verified(&id) {
                Ok((file, _)) => {
                    writeout_file(&file)
                        .map_err(|source| ObjectError::Io { path: target, source })?;
                    continue;
                }
                Err(ObjectError::Missing { .. }) => {}
                Err(error) => return Err(error),
            }
            let mut temporary = tempfile::Builder::new()
                .prefix(&format!(".pending-{id}-"))
                .tempfile_in(&self.root)
                .map_err(|source| ObjectError::Io { path: self.root.clone(), source })?;
            let temporary_path = temporary.path().to_owned();
            crate::fault::checkpoint("after-object-temp-created");
            temporary
                .write_all(bytes)
                .and_then(|()| writeout_file(temporary.as_file()))
                .map_err(|source| ObjectError::Io { path: temporary_path.clone(), source })?;
            match fs::hard_link(&temporary_path, &target) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let (file, _) = self.verified(&id)?;
                    writeout_file(&file)
                        .map_err(|source| ObjectError::Io { path: target, source })?;
                }
                Err(source) => return Err(ObjectError::Io { path: target, source }),
            }
            temporary.close().map_err(|source| ObjectError::Io { path: temporary_path, source })?;
        }
        crate::fault::checkpoint("before-object-group-flush");
        crate::fault::io_error("object-group-flush")
            .map_err(|source| ObjectError::Io { path: self.root.clone(), source })?;
        sync_directory(&self.root)
            .map_err(|source| ObjectError::Io { path: self.root.clone(), source })?;
        crate::fault::checkpoint("after-object-group-flush");
        Ok(())
    }

    /// Read complete bytes while the caller holds a retention pin; verify the digest.
    pub fn read(&self, id: &ObjectId) -> Result<Vec<u8>, ObjectError> {
        self.verified(id).map(|(_, bytes)| bytes)
    }

    /// Durably remove an object after the coordinator has excluded every pin.
    ///
    /// Missing objects are errors. A flush failure after unlink leaves an uncertain
    /// persistence outcome; callers must not interpret it as successful collection.
    pub fn remove(&self, id: &ObjectId) -> Result<(), ObjectError> {
        let path = self.path(id);
        fs::remove_file(&path).map_err(|source| self.file_error(id, source))?;
        sync_directory(&self.root)
            .map_err(|source| ObjectError::Io { path: self.root.clone(), source })
    }

    /// Remove abandoned temporary publications after excluding every active writer.
    ///
    /// Every `put` holds a shared directory lock from before temporary creation until
    /// publication and cleanup complete. The exclusive lock proves that remaining
    /// temporary files have no active writer, regardless of their hash's retention.
    /// The caller may hold the metadata writer transaction; `put` never acquires it.
    /// Errors can follow partial cleanup; only success confirms the directory flush.
    pub fn remove_pending(&self) -> Result<usize, ObjectError> {
        let _collection = self.lock(FlockOperation::LockExclusive)?;
        let entries = fs::read_dir(&self.root)
            .map_err(|source| ObjectError::Io { path: self.root.clone(), source })?;
        let mut removed = 0;
        for entry in entries {
            let entry =
                entry.map_err(|source| ObjectError::Io { path: self.root.clone(), source })?;
            let path = entry.path();
            let (_, pending) = entry_identity(&entry)?;
            if pending {
                validate_regular_file(&entry)?;
                fs::remove_file(&path).map_err(|source| ObjectError::Io { path, source })?;
                removed += 1;
            }
        }
        sync_directory(&self.root)
            .map_err(|source| ObjectError::Io { path: self.root.clone(), source })?;
        Ok(removed)
    }

    /// List published identities in stable order, excluding pending publications.
    ///
    /// This is an inventory, not a retained snapshot. The coordinator supplies the
    /// exclusion needed by collection. Object bytes are verified by `read`.
    pub fn ids(&self) -> Result<Vec<ObjectId>, ObjectError> {
        let entries = fs::read_dir(&self.root)
            .map_err(|source| ObjectError::Io { path: self.root.clone(), source })?;
        let mut ids = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|source| ObjectError::Io { path: self.root.clone(), source })?;
            let (id, pending) = entry_identity(&entry)?;
            if !pending {
                validate_regular_file(&entry)?;
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }

    fn path(&self, id: &ObjectId) -> PathBuf {
        self.root.join(id.as_str())
    }

    fn lock(&self, operation: FlockOperation) -> Result<File, ObjectError> {
        let directory = File::open(&self.root)
            .map_err(|source| ObjectError::Io { path: self.root.clone(), source })?;
        rustix::fs::flock(&directory, operation)
            .map_err(|source| ObjectError::Io { path: self.root.clone(), source: source.into() })?;
        Ok(directory)
    }

    fn verified(&self, id: &ObjectId) -> Result<(File, Vec<u8>), ObjectError> {
        let path = self.path(id);
        let metadata = fs::symlink_metadata(&path).map_err(|source| self.file_error(id, source))?;
        if !metadata.is_file() {
            return Err(ObjectError::UnexpectedEntry { path });
        }
        let mut file = File::open(&path).map_err(|source| self.file_error(id, source))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|source| self.file_error(id, source))?;
        let actual = ObjectId::from_bytes(&bytes);
        if actual != *id {
            return Err(ObjectError::Corrupt { path, expected: id.clone(), actual });
        }
        Ok((file, bytes))
    }

    fn file_error(&self, id: &ObjectId, source: io::Error) -> ObjectError {
        let path = self.path(id);
        if source.kind() == io::ErrorKind::NotFound {
            return ObjectError::Missing { id: id.clone(), path };
        }
        ObjectError::Io { path, source }
    }
}

// Batch callers hold the metadata writer lock until the directory's device flush.
// POSIX fsync writes macOS file data out without repeating the device-wide flush.
fn writeout_file(file: &File) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let result = rustix::fs::fsync(file).map_err(Into::into);
    #[cfg(not(target_os = "macos"))]
    let result = sync_file(file);
    result
}

fn entry_identity(entry: &fs::DirEntry) -> Result<(ObjectId, bool), ObjectError> {
    let path = entry.path();
    let name = entry.file_name();
    let name = name.to_str().ok_or_else(|| ObjectError::UnexpectedEntry { path: path.clone() })?;
    let Some(pending) = name.strip_prefix(".pending-") else {
        return ObjectId::parse(name).map(|id| (id, false));
    };
    let (digest, unique) = pending
        .split_once('-')
        .ok_or_else(|| ObjectError::UnexpectedEntry { path: path.clone() })?;
    if unique.is_empty() || !unique.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(ObjectError::UnexpectedEntry { path });
    }
    Ok((ObjectId::parse(digest)?, true))
}

fn validate_regular_file(entry: &fs::DirEntry) -> Result<(), ObjectError> {
    let path = entry.path();
    let kind =
        entry.file_type().map_err(|source| ObjectError::Io { path: path.clone(), source })?;
    if !kind.is_file() {
        return Err(ObjectError::UnexpectedEntry { path });
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn identifier_rejects_noncanonical_values() {
        for invalid in ["", "../outside", &"A".repeat(64), &"g".repeat(64), &"a".repeat(63)] {
            assert!(matches!(ObjectId::parse(invalid), Err(ObjectError::InvalidIdentifier { .. })));
        }
        assert_eq!(ObjectId::parse(&"a".repeat(64)).unwrap().as_str(), "a".repeat(64));
    }

    #[test]
    fn identity_uses_standard_sha256_and_checked_deserialization() {
        let id = ObjectId::from_bytes(b"abc");
        assert_eq!(id.as_str(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let encoded = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<ObjectId>(&encoded).unwrap(), id);
        assert!(serde_json::from_str::<ObjectId>("\"../outside\"").is_err());
    }

    #[test]
    fn repeated_publication_deduplicates_complete_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(directory.path().join("objects")).unwrap();
        let first = store.put(b"immutable").unwrap();
        assert_eq!(first, store.put(b"immutable").unwrap());
        assert_eq!(store.read(&first).unwrap(), b"immutable");
        assert_eq!(store.ids().unwrap(), vec![first]);
        assert_eq!(fs::read_dir(&store.root).unwrap().count(), 1);
    }

    #[test]
    fn corruption_is_rejected_on_read_and_deduplication() {
        let directory = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(directory.path().join("objects")).unwrap();
        let id = store.put(b"original").unwrap();
        fs::write(store.path(&id), b"corrupt").unwrap();
        assert!(matches!(store.read(&id), Err(ObjectError::Corrupt { .. })));
        assert!(matches!(store.put(b"original"), Err(ObjectError::Corrupt { .. })));
        assert_eq!(fs::read_dir(&store.root).unwrap().count(), 1);
    }

    #[test]
    fn identical_concurrent_publications_share_one_object() {
        let directory = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(directory.path().join("objects")).unwrap();
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        store.put(&vec![42; 65536]).unwrap()
                    })
                })
                .collect();
            let ids: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
            assert!(ids.windows(2).all(|pair| pair[0] == pair[1]));
        });
        assert_eq!(store.ids().unwrap().len(), 1);
        assert_eq!(fs::read_dir(&store.root).unwrap().count(), 1);
    }

    #[test]
    fn removal_is_durable_and_missing_is_explicit() {
        let directory = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(directory.path().join("objects")).unwrap();
        let id = store.put(b"delete").unwrap();
        store.remove(&id).unwrap();
        let reopened = ObjectStore::open(store.root.clone()).unwrap();
        assert!(reopened.ids().unwrap().is_empty());
        assert!(matches!(reopened.read(&id), Err(ObjectError::Missing { .. })));
        assert!(matches!(reopened.remove(&id), Err(ObjectError::Missing { .. })));
    }

    #[test]
    fn inventory_excludes_incomplete_publications() {
        let directory = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(directory.path().join("objects")).unwrap();
        let id = ObjectId::from_bytes(b"complete");
        fs::write(store.root.join(format!(".pending-{id}-interrupted")), b"partial").unwrap();
        assert!(store.ids().unwrap().is_empty());
        fs::write(store.root.join("unexpected"), b"invalid").unwrap();
        assert!(matches!(store.ids(), Err(ObjectError::InvalidIdentifier { .. })));
    }

    #[test]
    fn pending_collection_removes_abandoned_files_but_preserves_published_objects() {
        let directory = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(directory.path().join("objects")).unwrap();
        let live = ObjectId::from_bytes(b"active builder");
        let abandoned = ObjectId::from_bytes(b"abandoned builder");
        let published = store.put(b"published").unwrap();
        let live_path = store.root.join(format!(".pending-{live}-active"));
        let abandoned_path = store.root.join(format!(".pending-{abandoned}-dead"));
        fs::write(&live_path, b"partial").unwrap();
        fs::write(&abandoned_path, b"partial").unwrap();
        assert_eq!(store.remove_pending().unwrap(), 2);
        assert!(!live_path.exists());
        assert!(!abandoned_path.exists());
        assert_eq!(store.read(&published).unwrap(), b"published");
        assert_eq!(store.remove_pending().unwrap(), 0);
    }

    #[test]
    fn pending_collection_rejects_unknown_entries_without_deleting_them() {
        let directory = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(directory.path().join("objects")).unwrap();
        for name in [".pending-oldformat", ".pending-notadigest-unique", "unknown"] {
            let path = store.root.join(name);
            fs::write(&path, b"preserve").unwrap();
            assert!(store.remove_pending().is_err());
            assert_eq!(fs::read(&path).unwrap(), b"preserve");
            fs::remove_file(path).unwrap();
        }
    }
}
