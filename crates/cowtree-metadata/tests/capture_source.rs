// Copyright (c) 2026 Windsor Nguyen

#![allow(clippy::unwrap_used)]

use cowtree_metadata::{Entry, EntryKind, Error, Limits, ResourcePath, Store, Version};
use std::{collections::BTreeSet, fs, path::Path};

fn captured(root: &Path) -> bool {
    rusqlite::Connection::open(root.join("metadata.sqlite3"))
        .unwrap()
        .query_row("SELECT captured FROM views", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn observed_values_and_logical_edits_change_provenance_with_their_revision() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("store");
    let tree = temp.path().join("tree");
    fs::create_dir(&tree).unwrap();
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    store.bind_tree(leaf, &tree, Version::new(0).unwrap()).unwrap();
    let path = ResourcePath::parse("file").unwrap();
    let grant = store.acquire(leaf, &BTreeSet::from([path.clone()])).unwrap().remove(0);
    let grant = store.activate(&grant).unwrap();
    assert!(!captured(&root));
    fs::write(tree.join("file"), b"observed").unwrap();
    let first = store.capture_files(std::slice::from_ref(&grant)).unwrap().remove(0);
    assert!(captured(&root));
    let logical = store.edit(&grant, first.value).unwrap();
    assert!(!captured(&root));
    assert!(logical.edit_revision > first.edit_revision);
    store.capture_files(std::slice::from_ref(&grant)).unwrap();
    assert!(captured(&root));
    store.discard(leaf, &path).unwrap();
    assert!(!captured(&root));
    fs::remove_file(tree.join("file")).unwrap();
    let absent = store.capture_files(std::slice::from_ref(&grant)).unwrap().remove(0);
    assert_eq!(absent.value, None);
    assert!(captured(&root));
    store.revoke(&path).unwrap();
    assert!(matches!(store.edit(&grant, None), Err(Error::StaleToken(_))));
    assert!(captured(&root));
    assert_eq!(store.view(leaf).unwrap(), vec![absent]);
}

#[test]
fn version_two_migration_preserves_values_and_marks_them_unobserved() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let path = ResourcePath::parse("file").unwrap();
    let grant = store.acquire(leaf, &BTreeSet::from([path])).unwrap().remove(0);
    let grant = store.activate(&grant).unwrap();
    let value = Entry { object: store.stage(leaf, b"logical").unwrap(), kind: EntryKind::File };
    let view = store.edit(&grant, Some(value)).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    connection
        .execute_batch("ALTER TABLE views DROP COLUMN captured; PRAGMA user_version=2;")
        .unwrap();
    drop(connection);
    let reopened = Store::open(&root).unwrap();
    assert_eq!(reopened.view(leaf).unwrap(), vec![view]);
    assert!(!captured(&root));
    let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
    let version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
    assert_eq!(version, 3);
}
