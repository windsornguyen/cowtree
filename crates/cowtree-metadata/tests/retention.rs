//! Retention protects live state while bounded history and orphan bytes are reclaimed.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use cowtree_metadata::objects::{ObjectError, ObjectStore};
use cowtree_metadata::{
    Entry, EntryKind, Error, Grant, Limits, ProposalInput, Receipt, RequestId, ResourcePath, Store,
};

fn fixture() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let limits = Limits { retained_epochs: 2, retained_receipts: 2, ..Limits::default() };
    let store = Store::create(&directory.path().join("store"), limits).unwrap();
    (directory, store)
}

fn writer(store: &mut Store, path: &str) -> Grant {
    let leaf = store.create_leaf().unwrap();
    let paths = BTreeSet::from([ResourcePath::parse(path).unwrap()]);
    let grant = store.acquire(leaf, &paths).unwrap().remove(0);
    store.activate(&grant).unwrap()
}

fn edit(store: &mut Store, grant: &Grant, bytes: &[u8]) {
    let grant = store.acquire(grant.leaf, &BTreeSet::from([grant.path.clone()])).unwrap().remove(0);
    let object = store.stage(grant.leaf, bytes).unwrap();
    store.edit(&grant, Some(Entry { object, kind: EntryKind::File })).unwrap();
}

fn publish(store: &mut Store, grant: &Grant, sequence: u64, bytes: &[u8]) -> Receipt {
    edit(store, grant, bytes);
    let request = RequestId { leaf: grant.leaf, sequence };
    store.propose(ProposalInput { request, paths: BTreeSet::from([grant.path.clone()]) }).unwrap();
    let candidate = store.prepare(request).unwrap();
    store.commit(candidate).unwrap()
}

#[test]
fn manual_retention_survives_pruning_until_released() {
    let (directory, mut store) = fixture();
    let grant = writer(&mut store, "file");
    let first = publish(&mut store, &grant, 1, b"first");
    store.retain(first.version).unwrap();
    for sequence in 2..=8 {
        publish(&mut store, &grant, sequence, sequence.to_string().as_bytes());
    }
    let maintenance = store.maintain().unwrap();
    assert!(maintenance.epochs_removed > 0);
    assert_eq!(store.read(first.version, &grant.path).unwrap(), Some(b"first".to_vec()));
    store.release_retention(first.version).unwrap();
    store.maintain().unwrap();
    assert!(matches!(store.snapshot(first.version), Err(Error::SnapshotExpired(_))));
    assert!(matches!(store.retain(first.version), Err(Error::SnapshotExpired(_))));
    let objects = ObjectStore::open(directory.path().join("store/objects")).unwrap();
    assert!(matches!(objects.read(&first.root), Err(ObjectError::Missing { .. })));
}

#[test]
fn expired_receipts_cannot_reexecute_and_metadata_stays_bounded() {
    let (directory, mut store) = fixture();
    let grant = writer(&mut store, "file");
    let first = publish(&mut store, &grant, 1, b"first");
    let mut maximum_database = 0;
    for sequence in 2..=24 {
        let receipt = publish(&mut store, &grant, sequence, sequence.to_string().as_bytes());
        let result = store.maintain().unwrap();
        maximum_database = maximum_database.max(result.database_bytes);
        assert_eq!(result.wal_bytes, 0);
        assert_eq!(store.result(receipt.request).unwrap(), Some(receipt));
    }
    assert!(matches!(store.result(first.request), Err(Error::RequestExpired(1))));
    assert!(matches!(
        store.propose(ProposalInput {
            request: first.request,
            paths: BTreeSet::from([grant.path.clone()]),
        }),
        Err(Error::RequestExpired(1))
    ));
    let connection =
        rusqlite::Connection::open(directory.path().join("store/metadata.sqlite3")).unwrap();
    for table in ["epochs", "proposals"] {
        let count: i64 = connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
    assert!(maximum_database <= 256 * 1024, "database grew to {maximum_database} bytes");
}

#[test]
fn pending_proposal_pins_capture_and_prepared_candidate() {
    let (_directory, mut store) = fixture();
    let pending_writer = writer(&mut store, "pending");
    let other_writer = writer(&mut store, "other");
    let capture = store.tip().unwrap().0;
    edit(&mut store, &pending_writer, b"awaiting publication");
    let request = RequestId { leaf: pending_writer.leaf, sequence: 1 };
    store
        .propose(ProposalInput { request, paths: BTreeSet::from([pending_writer.path.clone()]) })
        .unwrap();
    for sequence in 1..=6 {
        publish(&mut store, &other_writer, sequence, sequence.to_string().as_bytes());
    }
    store.maintain().unwrap();
    assert!(store.snapshot(capture).unwrap().is_empty());
    let candidate = store.prepare(request).unwrap();
    store.maintain().unwrap();
    let receipt = store.commit(candidate).unwrap();
    assert_eq!(
        store.read(receipt.version, &pending_writer.path).unwrap(),
        Some(b"awaiting publication".to_vec())
    );
}

#[test]
fn uploads_and_dirty_values_protect_bytes_independently() {
    let (directory, mut store) = fixture();
    let grant = writer(&mut store, "file");
    let objects = ObjectStore::open(directory.path().join("store/objects")).unwrap();
    let upload = store.stage(grant.leaf, b"pending upload").unwrap();
    store.maintain().unwrap();
    assert_eq!(objects.read(&upload).unwrap(), b"pending upload");
    store.edit(&grant, Some(Entry { object: upload.clone(), kind: EntryKind::File })).unwrap();
    store.discard_upload(grant.leaf, &upload).unwrap();
    store.maintain().unwrap();
    assert_eq!(objects.read(&upload).unwrap(), b"pending upload");
    let abandoned = store.stage(grant.leaf, b"abandoned upload").unwrap();
    store.discard_upload(grant.leaf, &abandoned).unwrap();
    assert!(store.maintain().unwrap().objects_removed > 0);
    assert!(matches!(objects.read(&abandoned), Err(ObjectError::Missing { .. })));
}

#[test]
fn failed_metadata_pruning_never_deletes_rollback_references() {
    let (directory, mut store) = fixture();
    let grant = writer(&mut store, "file");
    let first = publish(&mut store, &grant, 1, b"first");
    for sequence in 2..=4 {
        publish(&mut store, &grant, sequence, sequence.to_string().as_bytes());
    }
    let connection =
        rusqlite::Connection::open(directory.path().join("store/metadata.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_pruning BEFORE DELETE ON epochs BEGIN SELECT RAISE(ABORT, 'injected pruning failure'); END;").unwrap();
    assert!(matches!(store.maintain(), Err(Error::Sqlite(_))));
    assert_eq!(store.read(first.version, &grant.path).unwrap(), Some(b"first".to_vec()));
    connection.execute_batch("DROP TRIGGER reject_pruning;").unwrap();
    store.maintain().unwrap();
    assert!(matches!(store.snapshot(first.version), Err(Error::SnapshotExpired(_))));
}

#[test]
fn external_reader_reports_checkpoint_busy_then_reclaims_after_release() {
    let (directory, mut store) = fixture();
    let grant = writer(&mut store, "file");
    publish(&mut store, &grant, 1, b"before reader");
    let reader =
        rusqlite::Connection::open(directory.path().join("store/metadata.sqlite3")).unwrap();
    reader.execute_batch("BEGIN; SELECT tip FROM settings;").unwrap();
    let latest = publish(&mut store, &grant, 2, b"after reader");
    assert!(matches!(store.maintain(), Err(Error::CheckpointBusy)));
    assert_eq!(store.read(latest.version, &grant.path).unwrap(), Some(b"after reader".to_vec()));
    reader.execute_batch("ROLLBACK;").unwrap();
    assert_eq!(store.maintain().unwrap().wal_bytes, 0);
}
