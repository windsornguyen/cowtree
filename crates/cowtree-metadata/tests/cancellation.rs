#![allow(clippy::unwrap_used)]

use cowtree_metadata::{Error, Limits, ProposalInput, RequestId, Store};
use std::collections::BTreeSet;

#[test]
fn canceled_unsubmitted_identity_cannot_be_replayed_after_collection() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::create(&temp.path().join("store"), Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let request = RequestId { leaf, sequence: 1 };
    store.abort(request).unwrap();
    store.abort(request).unwrap();
    assert!(store.propose(ProposalInput { request, paths: BTreeSet::new() }).is_err());
    store.maintain().unwrap();
    assert!(matches!(store.abort(request), Err(Error::RequestExpired(1))));
    store.abort(RequestId { leaf, sequence: 2 }).unwrap();
}
