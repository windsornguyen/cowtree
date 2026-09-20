// Copyright (c) 2026 Windsor Nguyen

//! Resumable epoch-one imports from quiescent filesystem captures.
//!
//! A fixed manifest pins every intended object before publication. Each bounded
//! chunk keeps the SQLite writer lock through object durability and commits one
//! cursor. Only finish publishes epoch one; an interrupted chunk can leave extra
//! pinned objects, but cannot acknowledge progress or expose a partial snapshot.

use std::{collections::BTreeSet, fs::File, io::Read, os::unix::fs::MetadataExt, path::Path};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use rustix::fs::{AtFlags, FileType, Mode, OFlags, open, openat, readlinkat, statat};
use serde::{Deserialize, Serialize};

use crate::{
    Entry, EntryKind, Error, LimitKind, ResourcePath, Result, Snapshot, Store, objects::ObjectId,
    publication::validate_namespace,
};

const CHUNK_FILES: usize = 64;
const CHUNK_BYTES: u64 = 32 * 1024 * 1024;

/// Persisted import cursor; complete means the manifest became committed epoch one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImportProgress {
    /// Fixed content identity of the complete initial manifest.
    pub root: ObjectId,
    /// Number of entries whose objects were durably published.
    pub completed: usize,
    /// Number of entries in the fixed manifest.
    pub total: usize,
    /// Whether epoch one was committed, including after a lost reply.
    pub complete: bool,
}

struct Import {
    progress: ImportProgress,
    manifest: Snapshot,
    json: String,
}

impl Store {
    /// Bind the sole initial import to an immutable manifest, or resume that identity.
    pub fn begin_import(&mut self, manifest: Snapshot) -> Result<ImportProgress> {
        validate_namespace(&manifest, self.limits.max_paths)?;
        self.capacity()?;
        let json = serde_json::to_string(&manifest)?;
        let root = ObjectId::from_bytes(json.as_bytes());
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(import) = read_import(&tx)? {
            match_import(&import, &root)?;
            return Ok(import.progress);
        }
        require_pristine(&tx)?;
        tx.execute("INSERT INTO imports VALUES(1,?1,?2)", params![root.as_str(), json])?;
        tx.execute("INSERT INTO import_progress VALUES(1,0,0)", [])?;
        crate::fault::checkpoint("before-import-begin-commit");
        tx.commit()?;
        crate::fault::checkpoint("after-import-begin-commit");
        Ok(ImportProgress { root, completed: 0, total: manifest.len(), complete: false })
    }

    /// Copy up to 64 entries or 32 MiB from the fixed source and commit one cursor.
    ///
    /// A single larger object uses its own chunk, bounded by `max_object_bytes`.
    /// The caller owns source quiescence. Every entry is checked against the fixed
    /// digest and file kind; symlink parents and hard-linked regular files are refused.
    pub fn import_chunk(&mut self, root: &ObjectId, source: &Path) -> Result<ImportProgress> {
        self.capacity()?;
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut import = required_import(&tx, root)?;
        if import.progress.complete {
            return Ok(import.progress);
        }
        require_pristine(&tx)?;
        let source_directory = open_source(source)?;
        let mut payloads = Vec::new();
        let mut bytes = 0_u64;
        for (path, entry) in import.manifest.iter().skip(import.progress.completed) {
            let payload =
                read_source(&source_directory, source, path, entry, self.limits.max_object_bytes)?;
            let length = payload.len() as u64;
            if !payloads.is_empty() && bytes + length > CHUNK_BYTES {
                break;
            }
            bytes += length;
            payloads.push(payload);
            if payloads.len() == CHUNK_FILES || bytes >= CHUNK_BYTES {
                break;
            }
        }
        self.objects.put_batch(&payloads)?;
        import.progress.completed += payloads.len();
        tx.execute(
            "UPDATE import_progress SET completed=?1 WHERE singleton=1",
            [import.progress.completed as i64],
        )?;
        crate::fault::checkpoint("before-import-chunk-commit");
        tx.commit()?;
        crate::fault::checkpoint("after-import-chunk-commit");
        Ok(import.progress)
    }

    /// Verify all durable objects and atomically expose the complete initial snapshot.
    pub fn finish_import(&mut self, root: &ObjectId) -> Result<ImportProgress> {
        self.capacity()?;
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut import = required_import(&tx, root)?;
        if import.progress.complete {
            return Ok(import.progress);
        }
        require_pristine(&tx)?;
        if import.progress.completed != import.progress.total {
            return Err(Error::ImportNotReady);
        }
        for (path, entry) in &import.manifest {
            let bytes = self.objects.read(&entry.object)?;
            validate_symlink(path, entry, &bytes)?;
        }
        self.objects.put(import.json.as_bytes())?;
        tx.execute("INSERT INTO epochs VALUES(1,?1)", [root.as_str()])?;
        tx.execute("UPDATE settings SET tip=1 WHERE singleton=1", [])?;
        tx.execute("UPDATE import_progress SET complete=1 WHERE singleton=1", [])?;
        crate::fault::checkpoint("before-import-finish-commit");
        tx.commit()?;
        crate::fault::checkpoint("after-import-finish-commit");
        import.progress.complete = true;
        Ok(import.progress)
    }

    /// Read the durable cursor after an interrupted request, including its final receipt.
    pub fn status_import(&self) -> Result<Option<ImportProgress>> {
        Ok(read_import(&self.connection)?.map(|import| import.progress))
    }
}

pub(crate) fn protect_objects(
    connection: &Connection,
    protected: &mut BTreeSet<ObjectId>,
) -> Result<()> {
    if let Some(import) = read_import(connection)? {
        if !import.progress.complete {
            protected.insert(import.progress.root);
            protected.extend(import.manifest.into_values().map(|entry| entry.object));
        }
    }
    Ok(())
}

pub(crate) fn require_no_import(connection: &Connection) -> Result<()> {
    let pending: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM import_progress WHERE complete=0)",
        [],
        |row| row.get(0),
    )?;
    if pending {
        return Err(Error::ImportConflict("initial import is unfinished".into()));
    }
    Ok(())
}

fn require_pristine(connection: &Connection) -> Result<()> {
    let pristine: bool = connection.query_row(
        "SELECT tip=0 AND next_leaf=1 AND next_token=1
         AND NOT EXISTS(SELECT 1 FROM leaves)
         AND NOT EXISTS(SELECT 1 FROM proposals)
         AND NOT EXISTS(SELECT 1 FROM leases)
         AND NOT EXISTS(SELECT 1 FROM views)
         AND NOT EXISTS(SELECT 1 FROM uploads)
         FROM settings WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    if !pristine {
        return Err(Error::ImportConflict(
            "authority has already admitted workspace operations".into(),
        ));
    }
    Ok(())
}

fn read_import(connection: &Connection) -> Result<Option<Import>> {
    let row = connection
        .query_row(
            "SELECT root,manifest_json,completed,complete FROM imports JOIN import_progress USING(singleton) WHERE singleton=1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((root, json, completed, complete)) = row else {
        return Ok(None);
    };
    let completed = usize::try_from(completed).map_err(|_| Error::Schema)?;
    let root = ObjectId::parse(&root)?;
    let manifest: Snapshot = serde_json::from_str(&json)?;
    if ObjectId::from_bytes(json.as_bytes()) != root
        || completed > manifest.len()
        || (complete && completed != manifest.len())
    {
        return Err(Error::Schema);
    }
    let progress = ImportProgress { root, completed, total: manifest.len(), complete };
    Ok(Some(Import { progress, manifest, json }))
}

fn required_import(connection: &Connection, root: &ObjectId) -> Result<Import> {
    let import = read_import(connection)?
        .ok_or_else(|| Error::ImportConflict("initial import has not begun".into()))?;
    match_import(&import, root)?;
    Ok(import)
}

fn match_import(import: &Import, root: &ObjectId) -> Result<()> {
    if import.progress.root != *root {
        return Err(Error::ImportConflict("initial manifest identity differs".into()));
    }
    Ok(())
}

fn open_source(source: &Path) -> Result<File> {
    if !source.is_absolute() {
        return Err(Error::InvalidInputFile(source.into()));
    }
    let file = open(
        source,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|source_error| Error::Io { path: source.into(), source: source_error.into() })?;
    Ok(File::from(file))
}

fn read_source(
    directory: &File,
    source: &Path,
    path: &ResourcePath,
    entry: &Entry,
    max_bytes: u64,
) -> Result<Vec<u8>> {
    let absolute = source.join(path.as_str());
    let io_error = |source: std::io::Error| Error::Io { path: absolute.clone(), source };
    let mut parent = directory.try_clone().map_err(io_error)?;
    let mut components = path.as_str().split('/').peekable();
    while let Some(name) = components.next() {
        if components.peek().is_none() {
            let bytes = read_entry(&parent, name, &absolute, entry, max_bytes)?;
            if ObjectId::from_bytes(&bytes) != entry.object {
                return Err(Error::ImportSourceChanged(path.as_str().into()));
            }
            validate_symlink(path, entry, &bytes)?;
            return Ok(bytes);
        }
        parent = File::from(
            openat(
                &parent,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| io_error(error.into()))?,
        );
    }
    Err(Error::InvalidPath(path.as_str().into()))
}

fn read_entry(
    parent: &File,
    name: &str,
    absolute: &Path,
    entry: &Entry,
    max_bytes: u64,
) -> Result<Vec<u8>> {
    let io_error = |source: std::io::Error| Error::Io { path: absolute.into(), source };
    let stat =
        statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|error| io_error(error.into()))?;
    if entry.kind == EntryKind::Symlink {
        if FileType::from_raw_mode(stat.st_mode) != FileType::Symlink {
            return Err(Error::ImportSourceChanged(absolute.to_string_lossy().into_owned()));
        }
        let bytes = readlinkat(parent, name, Vec::new())
            .map_err(|error| io_error(error.into()))?
            .into_bytes();
        if bytes.len() as u64 > max_bytes {
            return Err(Error::Limit(LimitKind::ObjectBytes));
        }
        return Ok(bytes);
    }
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(Error::ImportSourceChanged(absolute.to_string_lossy().into_owned()));
    }
    let file = File::from(
        openat(parent, name, OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())
            .map_err(|error| io_error(error.into()))?,
    );
    let metadata = file.metadata().map_err(io_error)?;
    let executable = metadata.mode() & 0o100 != 0;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || executable != (entry.kind == EntryKind::Executable)
    {
        return Err(Error::ImportSourceChanged(absolute.to_string_lossy().into_owned()));
    }
    if i128::from(metadata.dev()) != i128::from(stat.st_dev) || metadata.ino() != stat.st_ino {
        return Err(Error::ImportSourceChanged(absolute.to_string_lossy().into_owned()));
    }
    if metadata.len() > max_bytes {
        return Err(Error::Limit(LimitKind::ObjectBytes));
    }
    let limit = max_bytes.checked_add(1).ok_or(Error::CounterExhausted)?;
    let mut bytes = Vec::new();
    (&file).take(limit).read_to_end(&mut bytes).map_err(io_error)?;
    if bytes.len() as u64 > max_bytes {
        return Err(Error::Limit(LimitKind::ObjectBytes));
    }
    let after = file.metadata().map_err(io_error)?;
    if metadata.dev() != after.dev()
        || metadata.ino() != after.ino()
        || metadata.len() != after.len()
        || metadata.mode() != after.mode()
        || metadata.nlink() != after.nlink()
        || metadata.mtime() != after.mtime()
        || metadata.mtime_nsec() != after.mtime_nsec()
        || metadata.ctime() != after.ctime()
        || metadata.ctime_nsec() != after.ctime_nsec()
        || after.len() != bytes.len() as u64
    {
        return Err(Error::ImportSourceChanged(absolute.to_string_lossy().into_owned()));
    }
    Ok(bytes)
}

fn validate_symlink(path: &ResourcePath, entry: &Entry, bytes: &[u8]) -> Result<()> {
    if entry.kind == EntryKind::Symlink
        && (bytes.is_empty() || bytes.contains(&0) || std::str::from_utf8(bytes).is_err())
    {
        return Err(Error::InvalidSymlink(path.as_str().into()));
    }
    Ok(())
}
