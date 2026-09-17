// Copyright (c) 2026 Windsor Nguyen

#![allow(clippy::unwrap_used)]

use cowtree_metadata::{
    Entry, EntryKind, Error, Grant, Limits, ProposalInput, RequestId, ResolutionInput,
    ResourcePath, Store,
};
use std::collections::{BTreeMap, BTreeSet};

fn capture(store: &mut Store, grant: &Grant, bytes: &[u8], sequence: u64) -> RequestId {
    let object = store.stage(grant.leaf, bytes).unwrap();
    store.edit(grant, Some(Entry { object, kind: EntryKind::File })).unwrap();
    let request = RequestId { leaf: grant.leaf, sequence };
    store.propose(ProposalInput { request, paths: BTreeSet::from([grant.path.clone()]) }).unwrap();
    request
}
fn fixture() -> (tempfile::TempDir, Store, Grant) {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::create(&temp.path().join("store"), Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let reserved = store
        .acquire(leaf, &BTreeSet::from([ResourcePath::parse("file").unwrap()]))
        .unwrap()
        .remove(0);
    let grant = store.activate(&reserved).unwrap();
    (temp, store, grant)
}

#[test]
fn explicit_old_value_selection_supersedes_captures_and_preserves_live_edits() {
    let (temp, mut store, grant) = fixture();
    let first = capture(&mut store, &grant, b"first", 1);
    let second = capture(&mut store, &grant, b"second", 2);
    let request = RequestId { leaf: grant.leaf, sequence: 3 };
    let input = ResolutionInput {
        request,
        sources: vec![second, first],
        choices: BTreeMap::from([(grant.path.clone(), first)]),
    };
    let frozen = store.resolve(input.clone()).unwrap();
    assert_eq!(store.resolve(input.clone()).unwrap(), frozen);
    assert!(matches!(store.prepare(first), Err(Error::Aborted)));
    assert!(matches!(store.prepare(second), Err(Error::Aborted)));
    let candidate = store.prepare(request).unwrap();
    let receipt = store.commit(candidate).unwrap();
    assert_eq!(store.read(receipt.version, &grant.path).unwrap().unwrap(), b"first");
    assert!(store.view(grant.leaf).unwrap()[0].dirty());
    store.maintain().unwrap();
    let mut reopened = Store::open(&temp.path().join("store")).unwrap();
    assert_eq!(reopened.resolve(input.clone()).unwrap(), frozen);
    let mut changed = input;
    changed.choices.insert(grant.path.clone(), second);
    assert!(matches!(reopened.resolve(changed), Err(Error::RequestConflict)));
}

#[test]
fn incomplete_or_foreign_selection_rolls_back_source_retirement_and_sequence() {
    let (_temp, mut store, grant) = fixture();
    let first = capture(&mut store, &grant, b"first", 1);
    let second = capture(&mut store, &grant, b"second", 2);
    let request = RequestId { leaf: grant.leaf, sequence: 3 };
    let mut input =
        ResolutionInput { request, sources: vec![first, second], choices: BTreeMap::new() };
    assert!(matches!(store.resolve(input.clone()), Err(Error::ResolutionConflict(_))));
    input.choices.insert(grant.path.clone(), RequestId { leaf: grant.leaf, sequence: 99 });
    assert!(matches!(store.resolve(input.clone()), Err(Error::ResolutionConflict(_))));
    store.prepare(first).unwrap();
    store.prepare(second).unwrap();
    input.choices.insert(grant.path.clone(), second);
    assert_eq!(store.resolve(input).unwrap().request, request);
}

#[test]
fn revoked_authority_cannot_be_reintroduced_by_selecting_a_captured_value() {
    let (_temp, mut store, grant) = fixture();
    let source = capture(&mut store, &grant, b"first", 1);
    let request = RequestId { leaf: grant.leaf, sequence: 2 };
    store.revoke(&grant.path).unwrap();
    let input = ResolutionInput {
        request,
        sources: vec![source],
        choices: BTreeMap::from([(grant.path.clone(), source)]),
    };
    assert!(matches!(store.resolve(input), Err(Error::StaleToken(_))));
    assert_eq!(store.tip().unwrap().0.get(), 0);
}

#[test]
fn explicit_choices_compose_different_captures_path_by_path() {
    let (_temp, mut store, first_grant) = fixture();
    let leaf = first_grant.leaf;
    let reserved = store
        .acquire(leaf, &BTreeSet::from([ResourcePath::parse("other").unwrap()]))
        .unwrap()
        .remove(0);
    let other_grant = store.activate(&reserved).unwrap();
    let paths = BTreeSet::from([first_grant.path.clone(), other_grant.path.clone()]);
    let mut sources = Vec::new();
    for sequence in 1..=2 {
        for grant in [&first_grant, &other_grant] {
            let data = format!("{}:{sequence}", grant.path.as_str());
            let object = store.stage(leaf, data.as_bytes()).unwrap();
            store.edit(grant, Some(Entry { object, kind: EntryKind::File })).unwrap();
        }
        let request = RequestId { leaf, sequence };
        store.propose(ProposalInput { request, paths: paths.clone() }).unwrap();
        sources.push(request);
    }
    let input = ResolutionInput {
        request: RequestId { leaf, sequence: 3 },
        choices: BTreeMap::from([
            (first_grant.path.clone(), sources[0]),
            (other_grant.path.clone(), sources[1]),
        ]),
        sources,
    };
    let proposal = store.resolve(input).unwrap();
    let candidate = store.prepare(proposal.request).unwrap();
    let receipt = store.commit(candidate).unwrap();
    assert_eq!(store.read(receipt.version, &first_grant.path).unwrap().unwrap(), b"file:1");
    assert_eq!(store.read(receipt.version, &other_grant.path).unwrap().unwrap(), b"other:2");
    let views = store.view(leaf).unwrap();
    assert!(views.iter().find(|view| view.path == first_grant.path).unwrap().dirty());
    assert!(!views.iter().find(|view| view.path == other_grant.path).unwrap().dirty());
}
