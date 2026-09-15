// Copyright (c) 2026 Windsor Nguyen

//! SQLite connection setup, schema identity, and shared transaction primitives.

use crate::{
    Entry, Error, LeafId, Limits, ResourcePath, Result, Snapshot, Version,
    objects::{ObjectId, ObjectStore},
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const APPLICATION_ID: i64 = 0x43575452;
const SCHEMA_VERSION: i64 = 1;

/// One connection to the same-host SQLite authority and its immutable object directory.
pub struct Store {
    pub(crate) connection: Connection,
    pub(crate) objects: ObjectStore,
    pub(crate) root: PathBuf,
    pub(crate) limits: Limits,
}
impl Store {
    /// Create a new authority directory with persisted limits and epoch zero.
    pub fn create(root: &Path, limits: Limits) -> Result<Self> {
        limits.validate()?;
        fs::create_dir(root).map_err(|source| Error::Io { path: root.into(), source })?;
        let root =
            fs::canonicalize(root).map_err(|source| Error::Io { path: root.into(), source })?;
        let root = root.as_path();
        let objects = ObjectStore::open(root.join("objects"))?;
        let empty: Snapshot = BTreeMap::new();
        let bytes = serde_json::to_vec(&empty)?;
        let root_hash = objects.put(&bytes)?;
        let path = root.join("metadata.sqlite3");
        let connection = Connection::open(&path)?;
        connection.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; PRAGMA page_size=4096;")?;
        configure(&connection)?;
        let tx = connection.unchecked_transaction()?;
        tx.execute_batch(include_str!("schema.sql"))?;
        tx.execute("INSERT INTO settings VALUES(1,?1,0,1,1)", [serde_json::to_string(&limits)?])?;
        tx.execute("INSERT INTO epochs VALUES(0,?1)", [root_hash.as_str()])?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        crate::durability::sync_directory(root)
            .map_err(|source| Error::Io { path: root.into(), source })?;
        let parent =
            root.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
        crate::durability::sync_directory(parent)
            .map_err(|source| Error::Io { path: parent.into(), source })?;
        Ok(Self { connection, objects, root: root.into(), limits })
    }
    /// Open an existing authority, validating its schema and durability configuration.
    pub fn open(root: &Path) -> Result<Self> {
        let root =
            fs::canonicalize(root).map_err(|source| Error::Io { path: root.into(), source })?;
        let root = root.as_path();
        let connection = Connection::open_with_flags(
            root.join("metadata.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        let application: i64 =
            connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let schema: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if application != APPLICATION_ID || schema != SCHEMA_VERSION {
            return Err(Error::Schema);
        }
        configure(&connection)?;
        let raw: String = connection.query_row(
            "SELECT limits_json FROM settings WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let limits: Limits = serde_json::from_str(&raw)?;
        limits.validate()?;
        let objects = ObjectStore::open(root.join("objects"))?;
        Ok(Self { connection, objects, root: root.into(), limits })
    }
    /// Report the bundled SQLite engine version used by this authority.
    pub fn sqlite_version(&self) -> String {
        rusqlite::version().to_owned()
    }
    /// Read the capacity policy fixed when this authority was created.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
    /// Read the current committed version and immutable manifest identity together.
    pub fn tip(&self) -> Result<(Version, ObjectId)> {
        let (version, root) = read_tip(&self.connection)?;
        Ok((Version::from_sql(version)?, ObjectId::parse(&root)?))
    }
    /// Read a retained manifest while excluding concurrent object collection.
    pub fn snapshot(&mut self, version: Version) -> Result<Snapshot> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let root: String = tx
            .query_row("SELECT root FROM epochs WHERE version=?1", [version.sql()], |r| r.get(0))
            .optional()?
            .ok_or(Error::SnapshotExpired(version.sql()))?;
        let snapshot = read_snapshot(&self.objects, &root)?;
        tx.commit()?;
        Ok(snapshot)
    }
    /// Read and verify retained file bytes while excluding collection.
    pub fn read(&mut self, version: Version, path: &ResourcePath) -> Result<Option<Vec<u8>>> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let root: String = tx
            .query_row("SELECT root FROM epochs WHERE version=?1", [version.sql()], |r| r.get(0))
            .optional()?
            .ok_or(Error::SnapshotExpired(version.sql()))?;
        let snapshot = read_snapshot(&self.objects, &root)?;
        let bytes = match snapshot.get(path) {
            Some(entry) => Some(self.objects.read(&entry.object)?),
            None => None,
        };
        tx.commit()?;
        Ok(bytes)
    }
    /// Allocate a new leaf identity without ever reusing a dropped identity.
    pub fn create_leaf(&mut self) -> Result<LeafId> {
        self.capacity()?;
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count: i64 = tx.query_row("SELECT count(*) FROM leaves", [], |r| r.get(0))?;
        if count >= i64::from(self.limits.max_leaves) {
            return Err(Error::Limit(crate::LimitKind::ActiveLeaves));
        }
        let id: i64 =
            tx.query_row("SELECT next_leaf FROM settings WHERE singleton=1", [], |r| r.get(0))?;
        let next = id.checked_add(1).ok_or(Error::CounterExhausted)?;
        tx.execute("UPDATE settings SET next_leaf=?1 WHERE singleton=1", [next])?;
        tx.execute("INSERT INTO leaves(id) VALUES(?1)", [id])?;
        tx.commit()?;
        LeafId::from_sql(id)
    }
    pub(crate) fn capacity(&self) -> Result<()> {
        let path = self.root.join("metadata.sqlite3-wal");
        match fs::metadata(&path) {
            Ok(meta) if meta.len() > self.limits.max_wal_bytes => {
                Err(Error::Limit(crate::LimitKind::WalBytes))
            }
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(Error::Io { path, source }),
        }
    }
    /// Durably stage content under a generation-fenced construction pin.
    pub fn stage(&mut self, leaf: LeafId, bytes: &[u8]) -> Result<ObjectId> {
        if bytes.len() as u64 > self.limits.max_object_bytes {
            return Err(Error::Limit(crate::LimitKind::ObjectBytes));
        }
        self.capacity()?;
        let object = ObjectId::from_bytes(bytes);
        let generation = {
            let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            active(&tx, leaf)?;
            let count: i64 = tx.query_row("SELECT count(*) FROM uploads", [], |r| r.get(0))?;
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM uploads WHERE leaf=?1 AND object=?2)",
                params![leaf.sql(), object.as_str()],
                |row| row.get(0),
            )?;
            if !exists && count >= i64::from(self.limits.max_pending) {
                return Err(Error::Limit(crate::LimitKind::PendingUploads));
            }
            let generation = next_token(&tx)?;
            tx.execute(
                "INSERT INTO uploads VALUES(?1,?2,0,?3)
                 ON CONFLICT(leaf,object) DO UPDATE SET ready=0,generation=excluded.generation",
                params![leaf.sql(), object.as_str(), generation],
            )?;
            tx.commit()?;
            generation
        };
        crate::fault::checkpoint("after-upload-pin");
        self.objects.put(bytes)?;
        crate::fault::checkpoint("after-object-persist");
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        active(&tx, leaf)?;
        if tx.execute(
            "UPDATE uploads SET ready=1 WHERE leaf=?1 AND object=?2 AND generation=?3",
            params![leaf.sql(), object.as_str(), generation],
        )? != 1
        {
            return Err(Error::UploadNotReady);
        }
        tx.commit()?;
        Ok(object)
    }
    /// Cancel the current upload pin for this leaf and content identity.
    pub fn discard_upload(&mut self, leaf: LeafId, object: &ObjectId) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        active(&tx, leaf)?;
        tx.execute(
            "DELETE FROM uploads WHERE leaf=?1 AND object=?2",
            params![leaf.sql(), object.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }
}

fn configure(connection: &Connection) -> Result<()> {
    if rusqlite::version_number() < 3_051_003 {
        return Err(Error::SQLiteVersion(rusqlite::version().into()));
    }
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA trusted_schema=OFF;
         PRAGMA journal_mode=WAL;
         PRAGMA synchronous=FULL;
         PRAGMA fullfsync=ON;
         PRAGMA checkpoint_fullfsync=ON;
         PRAGMA wal_autocheckpoint=256;
         PRAGMA journal_size_limit=1048576;",
    )?;
    let mode: String = connection.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
    let sync: i64 = connection.pragma_query_value(None, "synchronous", |r| r.get(0))?;
    let foreign: i64 = connection.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
    let vacuum: i64 = connection.pragma_query_value(None, "auto_vacuum", |r| r.get(0))?;
    if mode != "wal" || sync != 2 || foreign != 1 || vacuum != 2 {
        return Err(Error::Durability);
    }
    Ok(())
}
pub(crate) fn active(connection: &Connection, leaf: LeafId) -> Result<i64> {
    let next = connection
        .query_row("SELECT next_sequence FROM leaves WHERE id=?1", [leaf.sql()], |r| r.get(0))
        .optional()?
        .ok_or(Error::LeafInactive(leaf.sql()))?;
    Ok(next)
}
pub(crate) fn read_tip(connection: &Connection) -> Result<(i64, String)> {
    let value = connection.query_row(
        "SELECT tip,root FROM settings JOIN epochs ON version=tip WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(value)
}
pub(crate) fn read_snapshot(objects: &ObjectStore, root: &str) -> Result<Snapshot> {
    let bytes = objects.read(&ObjectId::parse(root)?)?;
    let snapshot = serde_json::from_slice(&bytes)?;
    Ok(snapshot)
}
pub(crate) fn entry_json(entry: &Option<Entry>) -> Result<String> {
    Ok(serde_json::to_string(entry)?)
}
pub(crate) fn parse_entry(raw: &str) -> Result<Option<Entry>> {
    Ok(serde_json::from_str(raw)?)
}
pub(crate) fn next_token(tx: &Transaction<'_>) -> Result<i64> {
    let token: i64 =
        tx.query_row("SELECT next_token FROM settings WHERE singleton=1", [], |r| r.get(0))?;
    let next = token.checked_add(1).ok_or(Error::CounterExhausted)?;
    tx.execute("UPDATE settings SET next_token=?1 WHERE singleton=1", [next])?;
    Ok(token)
}
