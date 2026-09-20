// Copyright (c) 2026 Windsor Nguyen

//! Adding batch membership preserves existing requests and concurrent openers.
#![allow(clippy::unwrap_used)]

use std::{collections::BTreeSet, path::Path, sync::Arc, sync::Barrier, thread};

use cowtree_metadata::{
    Entry, EntryKind, Error, Limits, ProposalInput, RequestId, ResourcePath, Store,
};

fn version_one(root: &Path) {
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    connection
        .execute_batch("ALTER TABLE proposals DROP COLUMN batch_id; DROP TABLE client_pins; DROP TABLE import_progress; DROP TABLE imports; PRAGMA user_version=1;")
        .unwrap();
}

#[test]
fn migration_preserves_pending_candidates_receipts_and_unsubmitted_cancellation() {
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
    version_one(&root);

    let mut reopened = Store::open(&root).unwrap();
    assert_eq!(reopened.commit(candidate).unwrap(), receipt);
    assert_eq!(reopened.read(receipt.version, &path).unwrap().unwrap(), b"committed");
    let latest = reopened.commit(pending).unwrap();
    assert_eq!(reopened.read(latest.version, &path).unwrap().unwrap(), b"pending");
    let canceled = RequestId { leaf, sequence: 3 };
    reopened.abort(canceled).unwrap();
    reopened.maintain().unwrap();
    assert!(matches!(reopened.abort(canceled), Err(Error::RequestExpired(3))));
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    let tables: i64 = connection
        .query_row("SELECT count(*) FROM sqlite_schema WHERE name='bindings'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(tables, 0);
}

#[test]
fn concurrent_openers_migrate_the_same_authority_once() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    drop(store);
    version_one(&root);
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
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    let version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
    assert_eq!(version, 4);
}

#[test]
fn unsupported_schema_is_not_rewritten() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    drop(Store::create(&root, Limits::default()).unwrap());
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    assert!(matches!(Store::open(&root), Err(Error::Schema)));
    let version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
    assert_eq!(version, 99);
}

#[test]
fn version_three_adds_import_progress_without_replacing_existing_client_pins() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let (_, empty_root) = store.tip().unwrap();
    store.replace_client_pins(&BTreeSet::from([empty_root.clone()])).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    connection
        .execute_batch("DROP TABLE import_progress; DROP TABLE imports; PRAGMA user_version=3;")
        .unwrap();
    drop(connection);
    let reopened = Store::open(&root).unwrap();
    assert_eq!(reopened.status_import().unwrap(), None);
    drop(reopened);
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    let object: String =
        connection.query_row("SELECT object FROM client_pins", [], |row| row.get(0)).unwrap();
    assert_eq!(object, empty_root.as_str());
}
