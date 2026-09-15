// Copyright (c) 2026 Windsor Nguyen

//! Snapshot retention, rollback-safe object collection, and SQLite space reclamation.
//!
//! 1. Commit removal of expired epochs and receipts before unlinking any object.
//! 2. Acquire a fresh immediate transaction and derive every remaining object pin.
//! 3. Delete only unprotected objects while the metadata writer lock excludes new pins.
//! 4. Reclaim free SQLite pages and request a truncating WAL checkpoint.
//!
//! An interrupted collection can leak unreferenced bytes; rolling back its transaction
//! cannot restore a reference to bytes already removed. Retained receipts prove prior
//! publication, but do not extend snapshot retention.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, TransactionBehavior};

use crate::database::{parse_entry, read_snapshot};
use crate::objects::{ObjectError, ObjectId, ObjectStore};
use crate::{Error, Maintenance, Proposal, Result, Snapshot, Store, Version};

impl Store {
    /// Pin an existing committed snapshot until explicitly released.
    pub fn retain(&mut self, version: Version) -> Result<()> {
        self.capacity()?;
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM epochs WHERE version=?1)",
            [version.sql()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(Error::SnapshotExpired(version.sql()));
        }
        let (count, already): (i64, bool) = tx.query_row(
            "SELECT (SELECT count(*) FROM retained), EXISTS(SELECT 1 FROM retained WHERE version=?1)",
            [version.sql()], |row| Ok((row.get(0)?, row.get(1)?)))?;
        if !already && count >= i64::from(self.limits.max_pending) {
            return Err(Error::Limit(crate::LimitKind::ManualPins));
        }
        tx.execute("INSERT OR IGNORE INTO retained(version) VALUES(?1)", [version.sql()])?;
        tx.commit()?;
        Ok(())
    }

    /// Release a manual snapshot pin; repeated releases leave it unpinned.
    pub fn release_retention(&mut self, version: Version) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM retained WHERE version=?1", [version.sql()])?;
        tx.commit()?;
        Ok(())
    }

    /// Prune history, collect unreferenced bytes, and truncate the WAL.
    ///
    /// A later error does not undo earlier committed pruning or completed unlinks.
    /// Retrying is safe. File sizes describe observations after the checkpoint;
    /// concurrent writers can immediately grow the database or WAL again.
    pub fn maintain(&mut self) -> Result<Maintenance> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let epochs_removed = prune_epochs(&tx, self.limits.retained_epochs)?;
        let receipts_removed = prune_receipts(&tx, self.limits.retained_receipts)?;
        tx.commit()?;
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let protected = protected_objects(&tx, &self.objects)?;
        let mut objects_removed = 0;
        for id in self.objects.ids()? {
            if !protected.contains(&id) {
                self.objects.remove(&id)?;
                objects_removed += 1;
            }
        }
        let temporary_files_removed = self.objects.remove_pending()?;
        tx.commit()?;
        {
            let mut statement = self.connection.prepare("PRAGMA incremental_vacuum")?;
            let mut pages = statement.query([])?;
            while pages.next()?.is_some() {}
        }
        let busy: i64 =
            self.connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
        if busy != 0 {
            return Err(Error::CheckpointBusy);
        }
        let database_path = self.root.join("metadata.sqlite3");
        let database_bytes = fs::metadata(&database_path)
            .map_err(|source| Error::Io { path: database_path, source })?
            .len();
        Ok(Maintenance {
            epochs_removed,
            receipts_removed,
            objects_removed,
            temporary_files_removed,
            database_bytes,
            wal_bytes: wal_size(&self.root.join("metadata.sqlite3-wal"))?,
        })
    }
}

fn prune_epochs(connection: &Connection, keep: u32) -> Result<usize> {
    Ok(connection.execute(
        "DELETE FROM epochs
         WHERE version NOT IN (SELECT version FROM epochs ORDER BY version DESC LIMIT ?1)
         AND version NOT IN (SELECT version FROM retained)
         AND version NOT IN (SELECT captured_at FROM proposals WHERE state='pending')
         AND version NOT IN (SELECT parent FROM proposals WHERE state='pending' AND parent IS NOT NULL)",
        [keep],
    )?)
}

fn prune_receipts(connection: &Connection, keep: u32) -> Result<usize> {
    let removed = connection.execute(
        "DELETE FROM proposals WHERE state='committed' AND (leaf,sequence) NOT IN
         (SELECT leaf,sequence FROM proposals WHERE state='committed'
          ORDER BY committed_version DESC LIMIT ?1)",
        [keep],
    )?;
    connection.execute("DELETE FROM proposals WHERE state='aborted'", [])?;
    Ok(removed)
}

fn protected_objects(connection: &Connection, objects: &ObjectStore) -> Result<BTreeSet<ObjectId>> {
    let mut protected = BTreeSet::new();
    for root in text_rows(connection, "SELECT root FROM epochs")? {
        protect_snapshot(&mut protected, objects, &root)?;
    }
    for entry in text_rows(
        connection,
        "SELECT origin FROM views UNION ALL SELECT value FROM views UNION ALL SELECT origin FROM leases",
    )? {
        if let Some(entry) = parse_entry(&entry)? {
            protected.insert(entry.object);
        }
    }
    for upload in text_rows(connection, "SELECT object FROM uploads")? {
        protected.insert(ObjectId::parse(&upload)?);
    }
    for body in text_rows(connection, "SELECT body FROM proposals WHERE state='pending'")? {
        let proposal: Proposal = serde_json::from_str(&body)?;
        for change in proposal.changes {
            protected
                .extend(change.origin.into_iter().chain(change.value).map(|entry| entry.object));
        }
    }
    protect_candidates(connection, objects, &mut protected)?;
    Ok(protected)
}

fn protect_snapshot(
    protected: &mut BTreeSet<ObjectId>,
    objects: &ObjectStore,
    root: &str,
) -> Result<()> {
    protected.insert(ObjectId::parse(root)?);
    protected.extend(read_snapshot(objects, root)?.into_values().map(|entry| entry.object));
    Ok(())
}

fn protect_candidates(
    connection: &Connection,
    objects: &ObjectStore,
    protected: &mut BTreeSet<ObjectId>,
) -> Result<()> {
    let mut statement = connection
        .prepare("SELECT root,ready FROM proposals WHERE state='pending' AND root IS NOT NULL")?;
    let rows =
        statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)))?;
    for row in rows {
        let (root, ready) = row?;
        let id = ObjectId::parse(&root)?;
        protected.insert(id.clone());
        match objects.read(&id) {
            Ok(bytes) => {
                let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
                protected.extend(snapshot.into_values().map(|entry| entry.object));
            }
            Err(ObjectError::Missing { .. }) if !ready => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn text_rows(connection: &Connection, sql: &str) -> Result<Vec<String>> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([], |row| row.get(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn wal_size(path: &Path) -> Result<u64> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(source) => Err(Error::Io { path: path.to_owned(), source }),
    }
}
