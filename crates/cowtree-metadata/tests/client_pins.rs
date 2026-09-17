// Copyright (c) 2026 Windsor Nguyen

//! Client origins remain readable until the owning adapter replaces its durable pins.
#![allow(clippy::unwrap_used)]

use cowtree_metadata::{
    Limits, Store,
    objects::{ObjectId, ObjectStore},
};
use std::collections::BTreeSet;

#[test]
fn client_objects_survive_upload_release_and_are_reclaimed_after_unpinning() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let object = store.stage(leaf, b"old client origin").unwrap();
    store.replace_client_pins(&BTreeSet::from([object.clone()])).unwrap();
    store.drop_leaf(leaf).unwrap();
    drop(store);
    let mut reopened = Store::open(&root).unwrap();
    reopened.maintain().unwrap();
    let objects = ObjectStore::open(root.join("objects")).unwrap();
    assert_eq!(objects.read(&object).unwrap(), b"old client origin");
    reopened.replace_client_pins(&BTreeSet::new()).unwrap();
    assert_eq!(reopened.maintain().unwrap().objects_removed, 1);
    assert!(objects.read(&object).is_err());
}

#[test]
fn invalid_replacement_cannot_release_the_previous_client_roots() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let object = store.stage(leaf, b"retained origin").unwrap();
    store.replace_client_pins(&BTreeSet::from([object.clone()])).unwrap();
    store.drop_leaf(leaf).unwrap();
    let construction = store.create_leaf().unwrap();
    let added = store.stage(construction, b"x").unwrap();
    let missing = ObjectId::from_bytes(b"absent");
    assert!(added < missing, "exercise rollback after an inserted pin");
    assert!(store.replace_client_pins(&BTreeSet::from([added.clone(), missing])).is_err());
    store.drop_leaf(construction).unwrap();
    assert_eq!(store.maintain().unwrap().objects_removed, 1);
    assert!(ObjectStore::open(root.join("objects")).unwrap().read(&added).is_err());
    assert_eq!(
        ObjectStore::open(root.join("objects")).unwrap().read(&object).unwrap(),
        b"retained origin"
    );
}

#[test]
fn version_two_migrates_without_changing_snapshot_or_active_leaves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let tip = store.tip().unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    connection.execute_batch("DROP TABLE client_pins; PRAGMA user_version=2;").unwrap();
    drop(connection);
    let mut reopened = Store::open(&root).unwrap();
    assert_eq!(reopened.tip().unwrap(), tip);
    assert_eq!(reopened.leaves().unwrap(), vec![leaf]);
    reopened.replace_client_pins(&BTreeSet::new()).unwrap();
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    assert_eq!(
        connection.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0)).unwrap(),
        3
    );
}

#[cfg(feature = "fault-injection")]
#[test]
fn process_exit_replaces_client_pins_atomically() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    for phase in ["before-client-pins-commit", "after-client-pins-commit"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("store");
        let mut store = Store::create(&root, Limits::default()).unwrap();
        let leaf = store.create_leaf().unwrap();
        let old = store.stage(leaf, b"old").unwrap();
        let new = store.stage(leaf, b"new").unwrap();
        store.replace_client_pins(&BTreeSet::from([old.clone()])).unwrap();
        // The new object is still held by its construction upload during replacement.
        drop(store);
        let mut child = Command::new(env!("CARGO_BIN_EXE_cowtree-metadata"))
            .arg(&root)
            .env("COWTREE_CRASH_AT", phase)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = serde_json::json!({"op":"replace_client_pins","objects":[new]});
        writeln!(child.stdin.take().unwrap(), "{input}").unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(73));
        assert!(output.stdout.is_empty());
        let mut reopened = Store::open(&root).unwrap();
        reopened.drop_leaf(leaf).unwrap();
        reopened.maintain().unwrap();
        let objects = ObjectStore::open(root.join("objects")).unwrap();
        assert_eq!(objects.read(&old).is_ok(), phase == "before-client-pins-commit");
        assert_eq!(objects.read(&new).is_ok(), phase == "after-client-pins-commit");
    }
}
