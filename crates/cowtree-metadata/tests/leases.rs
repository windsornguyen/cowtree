// Copyright (c) 2026 Windsor Nguyen

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use cowtree_metadata::{Entry, EntryKind, Error, Grant, LeafId, Limits, ResourcePath, Store};

fn paths(values: &[&str]) -> BTreeSet<ResourcePath> {
    values.iter().map(|value| ResourcePath::parse(*value).unwrap()).collect()
}

fn reserve(store: &mut Store, leaf: LeafId, path: &str) -> Grant {
    store.acquire(leaf, &paths(&[path])).unwrap().remove(0)
}

fn stage(store: &mut Store, leaf: LeafId, bytes: &[u8]) -> Entry {
    Entry { object: store.stage(leaf, bytes).unwrap(), kind: EntryKind::File }
}

#[test]
fn activation_is_idempotent_without_erasing_later_edits() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::create(&directory.path().join("store"), Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let grant = reserve(&mut store, leaf, "file");
    let entry = stage(&mut store, leaf, b"edited");
    assert!(matches!(store.edit(&grant, Some(entry.clone())), Err(Error::NotActivated(_))));
    let active = store.activate(&grant).unwrap();
    assert!(active.activated);
    let edited = store.edit(&grant, Some(entry)).unwrap();
    assert_eq!(store.activate(&grant).unwrap(), active);
    assert_eq!(store.view(leaf).unwrap(), vec![edited]);
    assert_eq!(reserve(&mut store, leaf, "file"), active);
}

#[test]
fn old_grants_cannot_mutate_a_reassigned_reservation() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::create(&directory.path().join("store"), Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let old = reserve(&mut store, leaf, "file");
    store.activate(&old).unwrap();
    store.revoke(&old.path).unwrap();
    let fresh = reserve(&mut store, leaf, "file");
    assert!(fresh.token > old.token);
    assert!(matches!(store.activate(&old), Err(Error::StaleToken(_))));
    assert!(matches!(store.edit(&old, None), Err(Error::StaleToken(_))));
    assert!(matches!(store.release(&old), Err(Error::StaleToken(_))));
    assert_eq!(reserve(&mut store, leaf, "file"), fresh);
}

#[test]
fn overlapping_acquisition_rolls_back_every_earlier_path() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::create(&directory.path().join("store"), Limits::default()).unwrap();
    let owner = store.create_leaf().unwrap();
    let contender = store.create_leaf().unwrap();
    reserve(&mut store, owner, "z/child");
    assert!(matches!(store.acquire(contender, &paths(&["a", "z"])), Err(Error::LeaseConflict(_))));
    let available = reserve(&mut store, owner, "a");
    assert_eq!(available.token.get(), 2);
    assert!(matches!(
        store.acquire(contender, &paths(&["z/child/grandchild"])),
        Err(Error::LeaseConflict(_))
    ));
    reserve(&mut store, contender, "za");
}

#[test]
fn dirty_views_survive_release_until_explicit_discard() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let grant = reserve(&mut store, leaf, "file");
    store.activate(&grant).unwrap();
    let entry = stage(&mut store, leaf, b"keep");
    let edited = store.edit(&grant, Some(entry)).unwrap();
    store.release(&grant).unwrap();
    drop(store);
    let mut store = Store::open(&root).unwrap();
    assert_eq!(store.view(leaf).unwrap(), vec![edited]);
    assert!(matches!(store.acquire(leaf, &paths(&["file"])), Err(Error::DirtyPath(_))));
    store.discard(leaf, &grant.path).unwrap();
    assert!(store.view(leaf).unwrap().is_empty());
    assert!(reserve(&mut store, leaf, "file").token > grant.token);
}

#[test]
fn capacity_includes_unactivated_reservations_and_retained_views() {
    let directory = tempfile::tempdir().unwrap();
    let limits = Limits { max_paths: 1, ..Limits::default() };
    let mut store = Store::create(&directory.path().join("store"), limits).unwrap();
    let leaf = store.create_leaf().unwrap();
    let grant = reserve(&mut store, leaf, "a");
    assert!(matches!(store.acquire(leaf, &paths(&["b"])), Err(Error::Limit(_))));
    store.activate(&grant).unwrap();
    store.release(&grant).unwrap();
    assert!(matches!(store.acquire(leaf, &paths(&["b"])), Err(Error::Limit(_))));
    store.discard(leaf, &grant.path).unwrap();
    reserve(&mut store, leaf, "b");
}

#[test]
fn edits_require_owned_ready_bytes_and_preserve_valid_uploads_on_error() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::create(&directory.path().join("store"), Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let other = store.create_leaf().unwrap();
    let grant = reserve(&mut store, leaf, "link");
    store.activate(&grant).unwrap();
    let foreign = stage(&mut store, other, b"foreign");
    assert!(matches!(store.edit(&grant, Some(foreign)), Err(Error::UploadNotReady)));
    for bytes in [&b""[..], &b"has\0nul"[..], &[0xff][..]] {
        let entry = Entry { object: store.stage(leaf, bytes).unwrap(), kind: EntryKind::Symlink };
        assert!(matches!(store.edit(&grant, Some(entry.clone())), Err(Error::InvalidSymlink(_))));
        store.edit(&grant, Some(Entry { kind: EntryKind::File, ..entry })).unwrap();
    }
    let entry = stage(&mut store, leaf, b"../target");
    let view = store.edit(&grant, Some(entry.clone())).unwrap();
    let changed = store.edit(&grant, Some(Entry { kind: EntryKind::Symlink, ..entry })).unwrap();
    assert_eq!(changed.edit_revision, view.edit_revision + 1);
    assert_eq!(changed.origin, view.origin);
}

#[test]
fn dropping_a_leaf_releases_paths_without_reusing_its_identity() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::create(&directory.path().join("store"), Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let grant = reserve(&mut store, leaf, "file");
    store.activate(&grant).unwrap();
    stage(&mut store, leaf, b"pending");
    store.drop_leaf(leaf).unwrap();
    assert!(matches!(store.view(leaf), Err(Error::LeafInactive(_))));
    assert!(matches!(store.activate(&grant), Err(Error::LeafInactive(_))));
    let fresh = store.create_leaf().unwrap();
    assert!(fresh > leaf);
    let current = reserve(&mut store, fresh, "file");
    assert!(current.token > grant.token);
}

#[test]
fn grant_origin_is_validated_before_activation() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::create(&directory.path().join("store"), Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let mut grant = reserve(&mut store, leaf, "file");
    grant.origin = Some(stage(&mut store, leaf, b"uncommitted"));
    assert!(matches!(store.activate(&grant), Err(Error::StaleToken(_))));
    assert!(store.view(leaf).unwrap().is_empty());
}

#[test]
fn leased_discard_retains_origin_and_advances_revision() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::create(&directory.path().join("store"), Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let grant = reserve(&mut store, leaf, "file");
    store.activate(&grant).unwrap();
    let entry = stage(&mut store, leaf, b"discard");
    let edited = store.edit(&grant, Some(entry)).unwrap();
    store.discard(leaf, &grant.path).unwrap();
    let views = store.view(leaf).unwrap();
    assert_eq!(views.len(), 1);
    assert!(!views[0].dirty());
    assert_eq!(views[0].origin, edited.origin);
    assert_eq!(views[0].edit_revision, edited.edit_revision + 1);
}

#[test]
fn concurrent_connections_cannot_split_an_atomic_reservation_set() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaves = [store.create_leaf().unwrap(), store.create_leaf().unwrap()];
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let workers: Vec<_> = leaves
            .iter()
            .map(|leaf| {
                let root = &root;
                let barrier = &barrier;
                scope.spawn(move || {
                    let mut connection = Store::open(root).unwrap();
                    barrier.wait();
                    connection.acquire(*leaf, &paths(&["a", "b"]))
                })
            })
            .collect();
        workers.into_iter().map(|worker| worker.join().unwrap()).collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results.iter().filter(|result| matches!(result, Err(Error::LeaseConflict(_)))).count(),
        1
    );
    let winner = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(winner.len(), 2);
    assert_eq!(store.acquire(winner[0].leaf, &paths(&["a", "b"])).unwrap(), winner);
}

#[test]
fn dropping_a_leaf_aborts_pending_work_but_preserves_committed_receipts() {
    use cowtree_metadata::{ProposalInput, RequestId};

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let grant = reserve(&mut store, leaf, "file");
    store.activate(&grant).unwrap();
    let first = stage(&mut store, leaf, b"committed");
    store.edit(&grant, Some(first)).unwrap();
    let committed = RequestId { leaf, sequence: 1 };
    store.propose(ProposalInput { request: committed, paths: paths(&["file"]) }).unwrap();
    let candidate = store.prepare(committed).unwrap();
    let receipt = store.commit(candidate).unwrap();
    let grant = reserve(&mut store, leaf, "file");
    let second = stage(&mut store, leaf, b"abandoned");
    store.edit(&grant, Some(second)).unwrap();
    let pending = RequestId { leaf, sequence: 2 };
    store.propose(ProposalInput { request: pending, paths: paths(&["file"]) }).unwrap();
    store.prepare(pending).unwrap();
    store.drop_leaf(leaf).unwrap();
    drop(store);
    let mut store = Store::open(&root).unwrap();
    assert_eq!(store.result(committed).unwrap(), Some(receipt));
    assert!(matches!(store.prepare(pending), Err(Error::Aborted)));
    let database = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    for table in ["leases", "views", "uploads", "leaves"] {
        let count: i64 = database
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}
