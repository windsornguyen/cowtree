#![allow(clippy::unwrap_used)]

use cowtree_metadata::{Error, LimitKind, Limits, Store, objects::ObjectId};
use std::{fs, os::unix::fs::symlink};

#[test]
fn file_capture_uses_the_same_content_identity_and_limits_as_byte_input() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::write(&source, b"captured bytes").unwrap();
    let mut store = Store::create(&temp.path().join("store"), Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let identity = store.stage_file(leaf, &source).unwrap();
    assert_eq!(identity, ObjectId::from_bytes(b"captured bytes"));
    assert_eq!(identity, store.stage(leaf, b"captured bytes").unwrap());
    let link = temp.path().join("link");
    symlink(&source, &link).unwrap();
    assert!(matches!(store.stage_file(leaf, &link), Err(Error::InvalidInputFile(_))));
    assert!(matches!(store.stage_file(leaf, temp.path()), Err(Error::InvalidInputFile(_))));
    let limits = Limits { max_object_bytes: 4, ..Limits::default() };
    let mut small = Store::create(&temp.path().join("small"), limits).unwrap();
    let small_leaf = small.create_leaf().unwrap();
    assert!(matches!(
        small.stage_file(small_leaf, &source),
        Err(Error::Limit(LimitKind::ObjectBytes))
    ));
    store.drop_leaf(leaf).unwrap();
    assert!(matches!(store.stage_file(leaf, &source), Err(Error::LeafInactive(_))));
}
