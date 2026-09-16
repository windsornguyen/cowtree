// Copyright (c) 2026 Windsor Nguyen

#![allow(clippy::unwrap_used, clippy::panic)]

use cowtree_metadata::{
    Entry, EntryKind, Error, Limits, ProposalInput, RequestId, ResourcePath, Store,
};
use std::collections::BTreeSet;

#[test]
fn publication_requires_maintenance_before_history_grows_without_bound() {
    let temp = tempfile::tempdir().unwrap();
    let limits =
        Limits { retained_epochs: 1, retained_receipts: 1, max_pending: 1, ..Limits::default() };
    let mut store = Store::create(&temp.path().join("store"), limits).unwrap();
    let leaf = store.create_leaf().unwrap();
    let paths = BTreeSet::from([ResourcePath::parse("file").unwrap()]);
    let mut limited = false;
    for sequence in 1..=10 {
        let grant = store.acquire(leaf, &paths).unwrap().remove(0);
        let grant = store.activate(&grant).unwrap();
        let object = store.stage(leaf, &[sequence as u8]).unwrap();
        store.edit(&grant, Some(Entry { object, kind: EntryKind::File })).unwrap();
        let request = RequestId { leaf, sequence };
        match store.propose(ProposalInput { request, paths: paths.clone() }) {
            Ok(_) => {}
            Err(Error::Limit(_)) => {
                limited = true;
                break;
            }
            Err(error) => panic!("{error}"),
        }
        let candidate = store.prepare(request).unwrap();
        store.commit(candidate).unwrap();
    }
    assert!(limited, "publication admitted ten epochs despite one-epoch retention budget");
    store.maintain().unwrap();
}

#[test]
fn retrying_the_same_upload_does_not_consume_another_slot() {
    let temp = tempfile::tempdir().unwrap();
    let limits = Limits { max_pending: 1, ..Limits::default() };
    let mut store = Store::create(&temp.path().join("store"), limits).unwrap();
    let leaf = store.create_leaf().unwrap();
    let first = store.stage(leaf, b"same").unwrap();
    assert_eq!(store.stage(leaf, b"same").unwrap(), first);
    assert!(matches!(store.stage(leaf, b"different"), Err(Error::Limit(_))));
}
