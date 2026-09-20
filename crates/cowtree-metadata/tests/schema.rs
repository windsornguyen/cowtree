// Copyright (c) 2026 Windsor Nguyen

//! Only the current declared schema is accepted, without migrating stored state.
#![allow(clippy::unwrap_used)]

use std::{collections::BTreeSet, fs, sync::Arc, sync::Barrier, thread};

use cowtree_metadata::{
    Entry, EntryKind, Error, Limits, ProposalInput, RequestId, ResourcePath, Store,
};

#[test]
fn reopen_preserves_pending_candidates_receipts_and_unsubmitted_cancellation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let path = ResourcePath::parse("file").unwrap();
    let paths = BTreeSet::from([path.clone()]);
    let grant = store.acquire(leaf, &paths).unwrap().remove(0);
    let grant = store.activate(&grant).unwrap();
    let object = store.stage(leaf, b"committed").unwrap();
    store.edit(&grant, Some(Entry { object, kind: EntryKind::File })).unwrap();
    let request = RequestId { leaf, sequence: 1 };
    store.propose(ProposalInput { request, paths: paths.clone() }).unwrap();
    let candidate = store.prepare(request).unwrap();
    let receipt = store.commit(candidate.clone()).unwrap();
    let grant = store.acquire(leaf, &paths).unwrap().remove(0);
    let object = store.stage(leaf, b"pending").unwrap();
    store.edit(&grant, Some(Entry { object, kind: EntryKind::File })).unwrap();
    let request = RequestId { leaf, sequence: 2 };
    store.propose(ProposalInput { request, paths }).unwrap();
    let pending = store.prepare(request).unwrap();
    drop(store);

    let mut reopened = Store::open(&root).unwrap();
    assert_eq!(reopened.commit(candidate).unwrap(), receipt);
    assert_eq!(reopened.read(receipt.version, &path).unwrap().unwrap(), b"committed");
    let latest = reopened.commit(pending).unwrap();
    assert_eq!(reopened.read(latest.version, &path).unwrap().unwrap(), b"pending");
    let canceled = RequestId { leaf, sequence: 3 };
    reopened.abort(canceled).unwrap();
    reopened.maintain().unwrap();
    assert!(matches!(reopened.abort(canceled), Err(Error::RequestExpired(3))));
}

#[test]
fn concurrent_openers_preserve_the_same_authority() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    drop(store);
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let root = root.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                Store::open(&root).unwrap().leaves().unwrap()
            })
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), vec![leaf]);
    }
}

#[test]
fn schema_drift_preserves_database_bytes_without_a_pending_journal() {
    for change in [
        "DROP TABLE client_pins",
        "CREATE TABLE extra(value TEXT)",
        "CREATE TABLE sqliteXshadow(value TEXT)",
        "ALTER TABLE leaves ADD COLUMN extra TEXT",
        "CREATE INDEX leaf_sequence ON leaves(next_sequence)",
        "CREATE VIEW leaf_view AS SELECT * FROM leaves",
        "CREATE TRIGGER leaf_trigger AFTER INSERT ON leaves BEGIN SELECT 1; END",
        "ALTER TABLE settings RENAME TO previous_settings;
         CREATE TABLE settings(singleton INTEGER PRIMARY KEY CHECK(singleton=1), limits_json TEXT NOT NULL,
         tip INTEGER NOT NULL CHECK(tip>=0), next_leaf INTEGER NOT NULL CHECK(next_leaf>=0),
         next_token INTEGER NOT NULL CHECK(next_token>0));
         INSERT INTO settings SELECT * FROM previous_settings; DROP TABLE previous_settings",
        "DROP TABLE leases;
         CREATE TABLE leases(path TEXT PRIMARY KEY, leaf INTEGER NOT NULL REFERENCES leaves(id), token INTEGER NOT NULL,
         origin TEXT NOT NULL, activated INTEGER NOT NULL CHECK(activated IN (0,1)))",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("store");
        drop(Store::create(&root, Limits::default()).unwrap());
        let path = root.join("metadata.sqlite3");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(change).unwrap();
        connection.execute_batch("PRAGMA journal_mode=DELETE").unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        let error = Store::open(&root).err();
        assert!(matches!(error, Some(Error::Schema)), "schema rejection for {change}: {error:?}");
        assert_eq!(fs::read(&path).unwrap(), before, "rewrote {change}");
    }
}

#[test]
fn foreign_application_identity_preserves_database_bytes_without_a_pending_journal() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    drop(Store::create(&root, Limits::default()).unwrap());
    let path = root.join("metadata.sqlite3");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.pragma_update(None, "application_id", 99).unwrap();
    connection.execute_batch("PRAGMA journal_mode=DELETE").unwrap();
    drop(connection);
    let before = fs::read(&path).unwrap();
    assert!(matches!(Store::open(&root), Err(Error::Schema)));
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn schema_has_no_user_version_and_allows_sqlite_internal_statistics() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    drop(Store::create(&root, Limits::default()).unwrap());
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    let version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
    assert_eq!(version, 0);
    connection.execute_batch("ANALYZE").unwrap();
    drop(connection);
    drop(Store::open(&root).unwrap());
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    let version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
    assert_eq!(version, 0);
}

#[test]
fn rejection_preserves_committed_state_from_an_uncheckpointed_wal() {
    if let Some(root) = std::env::var_os("COWTREE_SCHEMA_WAL_CHILD") {
        let connection =
            rusqlite::Connection::open(std::path::Path::new(&root).join("metadata.sqlite3"))
                .unwrap();
        connection
            .execute_batch(
                "PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE extra(value TEXT);
                 INSERT INTO extra VALUES('committed before exit');",
            )
            .unwrap();
        // Abrupt exit skips Connection::drop, leaving committed pages in the WAL.
        std::process::exit(73);
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    drop(store);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "rejection_preserves_committed_state_from_an_uncheckpointed_wal"])
        .env("COWTREE_SCHEMA_WAL_CHILD", &root)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(73));
    assert!(fs::metadata(root.join("metadata.sqlite3-wal")).unwrap().len() > 0);
    assert!(matches!(Store::open(&root), Err(Error::Schema)));
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    let (sql, value): (String, String) = connection
        .query_row(
            "SELECT sql,(SELECT value FROM extra) FROM sqlite_schema WHERE name='extra'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(sql, "CREATE TABLE extra(value TEXT)");
    assert_eq!(value, "committed before exit");
    let retained_leaf: i64 =
        connection.query_row("SELECT id FROM leaves", [], |row| row.get(0)).unwrap();
    assert_eq!(u64::try_from(retained_leaf).unwrap(), leaf.get());
}
