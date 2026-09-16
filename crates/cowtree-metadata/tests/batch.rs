// Copyright (c) 2026 Windsor Nguyen

#![allow(clippy::unwrap_used)]

use cowtree_metadata::{
    Entry, EntryKind, Error, Grant, Limits, ProposalInput, RequestId, ResourcePath, Store, Version,
};
use std::collections::BTreeSet;

fn change(store: &mut Store, grant: &Grant, bytes: &[u8], sequence: u64) -> RequestId {
    let object = store.stage(grant.leaf, bytes).unwrap();
    store.edit(grant, Some(Entry { object, kind: EntryKind::File })).unwrap();
    let request = RequestId { leaf: grant.leaf, sequence };
    store.propose(ProposalInput { request, paths: BTreeSet::from([grant.path.clone()]) }).unwrap();
    request
}
fn grant(store: &mut Store, leaf: cowtree_metadata::LeafId, path: &str) -> Grant {
    let grant = store
        .acquire(leaf, &BTreeSet::from([ResourcePath::parse(path).unwrap()]))
        .unwrap()
        .remove(0);
    store.activate(&grant).unwrap()
}

#[test]
fn a_batch_publishes_one_epoch_and_preserves_later_edits_across_connections() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("store");
    let mut first = Store::create(&root, Limits::default()).unwrap();
    let mut second = Store::open(&root).unwrap();
    let left = first.create_leaf().unwrap();
    let right = second.create_leaf().unwrap();
    let a = grant(&mut first, left, "a");
    let b = grant(&mut second, right, "b");
    let one = change(&mut first, &a, b"a", 1);
    let two = change(&mut second, &b, b"b", 1);
    let candidate = first.prepare_batch(vec![two, one], Version::new(0).unwrap()).unwrap();
    change(&mut second, &b, b"later", 2);
    let result = second.commit_batch(candidate.clone()).unwrap();
    assert_eq!(result.receipts.len(), 2);
    assert!(result.receipts.iter().all(|receipt| receipt.version.get() == 1));
    assert_eq!(first.commit_batch(candidate).unwrap(), result);
    assert_eq!(first.snapshot(Version::new(1).unwrap()).unwrap().len(), 2);
    assert!(first.view(right).unwrap()[0].dirty());
    for receipt in result.receipts {
        assert_eq!(first.result(receipt.request).unwrap(), Some(receipt));
    }
}

#[test]
fn a_stale_member_rejects_the_entire_batch_without_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let mut other = Store::open(&root).unwrap();
    let leaf = store.create_leaf().unwrap();
    let a = grant(&mut store, leaf, "a");
    let b = grant(&mut store, leaf, "b");
    let one = change(&mut store, &a, b"a", 1);
    let two = change(&mut store, &b, b"b", 2);
    let candidate = store.prepare_batch(vec![one, two], Version::new(0).unwrap()).unwrap();
    other.revoke(&b.path).unwrap();
    assert!(matches!(store.commit_batch(candidate), Err(Error::StaleToken(_))));
    assert_eq!(store.tip().unwrap().0.get(), 0);
    assert_eq!(store.result(one).unwrap(), None);
    assert_eq!(store.result(two).unwrap(), None);
    assert!(store.view(leaf).unwrap().iter().all(|view| view.dirty()));
}

#[test]
fn duplicate_and_overlapping_resource_sets_are_rejected_before_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::create(&temp.path().join("store"), Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let a = grant(&mut store, leaf, "a");
    let descendant = grant(&mut store, leaf, "a/b");
    let one = change(&mut store, &a, b"one", 1);
    let two = change(&mut store, &a, b"two", 2);
    let three = change(&mut store, &descendant, b"three", 3);
    let four = change(&mut store, &a, b"four", 4);
    for requests in [vec![one, one], vec![one, two], vec![one, three], vec![three, four]] {
        assert!(matches!(
            store.prepare_batch(requests, Version::new(0).unwrap()),
            Err(Error::BatchConflict(_))
        ));
    }
    let valid = store.prepare(one).unwrap();
    assert_eq!(valid.attempt, 1);
}

#[test]
fn membership_cannot_be_truncated_split_or_reused_after_repreparation() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::create(&temp.path().join("store"), Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let a = grant(&mut store, leaf, "a");
    let b = grant(&mut store, leaf, "b");
    let one = change(&mut store, &a, b"a", 1);
    let two = change(&mut store, &b, b"b", 2);
    let batch = store.prepare_batch(vec![one, two], Version::new(0).unwrap()).unwrap();
    assert!(matches!(store.commit(batch.members[0].clone()), Err(Error::BatchConflict(_))));
    let mut truncated = batch.clone();
    truncated.members.pop();
    assert!(matches!(store.commit_batch(truncated), Err(Error::CandidateMismatch)));
    store.prepare(one).unwrap();
    assert!(matches!(store.commit_batch(batch), Err(Error::CandidateMismatch)));
    assert_eq!(store.tip().unwrap().0.get(), 0);
    let fresh = store.prepare_batch(vec![one, two], Version::new(0).unwrap()).unwrap();
    store.maintain().unwrap();
    store.commit_batch(fresh).unwrap();
}

#[test]
#[cfg(feature = "fault-injection")]
fn process_exit_at_commit_boundaries_never_splits_batch_receipts() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for (fault, committed) in [("before-batch-sql-commit", false), ("after-batch-sql-commit", true)]
    {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("store");
        let mut store = Store::create(&root, Limits::default()).unwrap();
        let leaf = store.create_leaf().unwrap();
        let a = grant(&mut store, leaf, "a");
        let b = grant(&mut store, leaf, "b");
        let one = change(&mut store, &a, b"a", 1);
        let two = change(&mut store, &b, b"b", 2);
        let candidate = store.prepare_batch(vec![one, two], Version::new(0).unwrap()).unwrap();
        drop(store);
        let mut child = Command::new(env!("CARGO_BIN_EXE_cowtree-metadata"))
            .arg(&root)
            .env("COWTREE_CRASH_AT", fault)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        serde_json::to_writer(
            &mut stdin,
            &serde_json::json!({"op":"commit_batch","candidate":candidate}),
        )
        .unwrap();
        writeln!(stdin).unwrap();
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(73), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stdout.is_empty());
        let mut reopened = Store::open(&root).unwrap();
        assert_eq!(reopened.result(one).unwrap().is_some(), committed);
        assert_eq!(reopened.result(two).unwrap().is_some(), committed);
        assert_eq!(reopened.tip().unwrap().0.get(), u64::from(committed));
        let receipt = reopened.commit_batch(candidate.clone()).unwrap();
        assert_eq!(reopened.commit_batch(candidate).unwrap(), receipt);
        assert_eq!(reopened.tip().unwrap().0.get(), 1);
    }
}

#[test]
fn disjoint_batches_require_repreparation_after_another_batch_advances_tip() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let mut other = Store::open(&root).unwrap();
    let leaf = store.create_leaf().unwrap();
    let a = grant(&mut store, leaf, "a");
    let b = grant(&mut store, leaf, "ab");
    let one = change(&mut store, &a, b"a", 1);
    let two = change(&mut store, &b, b"b", 2);
    let first = store.prepare_batch(vec![one], Version::new(0).unwrap()).unwrap();
    let second = other.prepare_batch(vec![two], Version::new(0).unwrap()).unwrap();
    store.commit_batch(first).unwrap();
    assert!(matches!(other.commit_batch(second), Err(Error::TipChanged { .. })));
    assert_eq!(other.result(two).unwrap(), None);
    let second = other.prepare_batch(vec![two], Version::new(1).unwrap()).unwrap();
    other.commit_batch(second).unwrap();
    assert_eq!(store.snapshot(Version::new(2).unwrap()).unwrap().len(), 2);
}

#[test]
#[cfg(feature = "fault-injection")]
fn interrupted_batch_preparation_keeps_all_inputs_pinned_until_retry() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let leaf = store.create_leaf().unwrap();
    let a = grant(&mut store, leaf, "a");
    let b = grant(&mut store, leaf, "b");
    let one = change(&mut store, &a, b"a", 1);
    let two = change(&mut store, &b, b"b", 2);
    drop(store);
    let mut child = Command::new(env!("CARGO_BIN_EXE_cowtree-metadata"))
        .arg(&root)
        .env("COWTREE_CRASH_AT", "after-batch-candidate-persist")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    serde_json::to_writer(
        &mut stdin,
        &serde_json::json!({"op":"prepare_batch",
        "requests":[one,two],"expected_tip":0}),
    )
    .unwrap();
    writeln!(stdin).unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(73), "{}", String::from_utf8_lossy(&output.stderr));
    let mut reopened = Store::open(&root).unwrap();
    reopened.maintain().unwrap();
    assert_eq!(reopened.tip().unwrap().0.get(), 0);
    let candidate = reopened.prepare_batch(vec![one, two], Version::new(0).unwrap()).unwrap();
    assert!(candidate.members.iter().all(|member| member.attempt == 2));
    let receipt = reopened.commit_batch(candidate).unwrap();
    let snapshot = reopened.snapshot(receipt.receipts[0].version).unwrap();
    assert_eq!(snapshot.len(), 2);
}
