#![allow(clippy::unwrap_used)]

use cowtree_metadata::{Limits, ResourcePath, Store};
use std::collections::BTreeSet;

#[test]
fn inventory_exposes_only_live_authority_and_never_reuses_leaf_ids() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::create(&temp.path().join("store"), Limits::default()).unwrap();
    let first = store.create_leaf().unwrap();
    let second = store.create_leaf().unwrap();
    let paths = BTreeSet::from([ResourcePath::parse("source").unwrap()]);
    let grant = store.acquire(first, &paths).unwrap().remove(0);
    assert_eq!(store.leaves().unwrap(), vec![first, second]);
    assert_eq!(store.grants().unwrap(), vec![grant.clone()]);
    let activated = store.activate(&grant).unwrap();
    assert_eq!(store.grants().unwrap(), vec![activated]);
    store.drop_leaf(first).unwrap();
    assert!(store.grants().unwrap().is_empty());
    assert_eq!(store.leaves().unwrap(), vec![second]);
    let third = store.create_leaf().unwrap();
    assert!(third.get() > second.get());
}
