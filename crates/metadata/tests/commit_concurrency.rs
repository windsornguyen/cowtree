// Copyright (c) 2026 Windsor Nguyen

//! Payload verification permits concurrent writers while final fencing remains atomic.
#![cfg(feature = "fault-injection")]
#![allow(clippy::unwrap_used)]

use cowtree_metadata::{
    Candidate, Entry, EntryKind, Grant, Limits, ProposalInput, RequestId, ResourcePath, Store,
    Version,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::Write,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Paused {
    child: Option<Child>,
}
impl Paused {
    fn finish(mut self, marker: &Path) -> Value {
        std::fs::remove_file(marker).unwrap();
        let output = self.child.take().unwrap().wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
impl Drop for Paused {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn paused(root: &Path, command: Value, point: &str, marker: &Path) -> Paused {
    let child = Command::new(env!("CARGO_BIN_EXE_cowtree-metadata"))
        .arg(root)
        .env_remove("COWTREE_CRASH_AT")
        .env("COWTREE_PAUSE_AT", point)
        .env("COWTREE_PAUSE_FILE", marker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut process = Paused { child: Some(child) };
    let mut input = process.child.as_mut().unwrap().stdin.take().unwrap();
    serde_json::to_writer(&mut input, &command).unwrap();
    writeln!(input).unwrap();
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.try_exists().unwrap() {
        assert!(Instant::now() < deadline, "commit did not reach {point}");
        std::thread::sleep(Duration::from_millis(10));
    }
    process
}
fn proposal(store: &mut Store, path: &str) -> (Grant, Candidate) {
    let leaf = store.create_leaf().unwrap();
    let paths = BTreeSet::from([ResourcePath::parse(path).unwrap()]);
    let grant = store.acquire(leaf, &paths).unwrap().remove(0);
    let grant = store.activate(&grant).unwrap();
    let object = store.stage(leaf, path.as_bytes()).unwrap();
    store.edit(&grant, Some(Entry { object, kind: EntryKind::File })).unwrap();
    let request = RequestId { leaf, sequence: 1 };
    store.propose(ProposalInput { request, paths }).unwrap();
    (grant, store.prepare(request).unwrap())
}

#[test]
fn single_verification_releases_writer_lock_and_rechecks_revocation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let (grant, candidate) = proposal(&mut store, "file");
    let marker = directory.path().join("paused");
    let process = paused(
        &root,
        json!({"op":"commit","candidate":candidate}),
        "before-commit-verification",
        &marker,
    );
    store.create_leaf().unwrap();
    store.revoke(&grant.path).unwrap();
    let response = process.finish(&marker);
    assert_eq!(response["code"], "stale_token");
    assert_eq!(store.tip().unwrap().0, Version::new(0).unwrap());
    assert_eq!(store.result(candidate.request).unwrap(), None);
}

#[test]
fn batch_verification_releases_writer_lock_and_rechecks_every_member() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let (_, first) = proposal(&mut store, "first");
    let (grant, second) = proposal(&mut store, "second");
    let candidate =
        store.prepare_batch(vec![first.request, second.request], Version::new(0).unwrap()).unwrap();
    let marker = directory.path().join("paused");
    let process = paused(
        &root,
        json!({"op":"commit_batch","candidate":candidate}),
        "before-batch-commit-verification",
        &marker,
    );
    store.create_leaf().unwrap();
    store.revoke(&grant.path).unwrap();
    let response = process.finish(&marker);
    assert_eq!(response["code"], "stale_token");
    assert_eq!(store.tip().unwrap().0, Version::new(0).unwrap());
    assert_eq!(store.result(first.request).unwrap(), None);
    assert_eq!(store.result(second.request).unwrap(), None);
}

#[test]
fn superseded_attempt_is_rejected_after_verification() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let (_, candidate) = proposal(&mut store, "file");
    let marker = directory.path().join("paused");
    let process = paused(
        &root,
        json!({"op":"commit","candidate":candidate}),
        "after-commit-verification",
        &marker,
    );
    let newer = store.prepare(candidate.request).unwrap();
    assert!(newer.attempt > candidate.attempt);
    let response = process.finish(&marker);
    assert_eq!(response["code"], "candidate_mismatch");
    assert_eq!(store.result(candidate.request).unwrap(), None);
    store.commit(newer).unwrap();
}

#[test]
fn collection_after_abort_cannot_turn_verified_bytes_into_a_commit() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let (grant, candidate) = proposal(&mut store, "file");
    let marker = directory.path().join("paused");
    let process = paused(
        &root,
        json!({"op":"commit","candidate":candidate}),
        "after-commit-verification",
        &marker,
    );
    store.abort(candidate.request).unwrap();
    store.drop_leaf(grant.leaf).unwrap();
    assert!(store.maintain().unwrap().objects_removed > 0);
    let response = process.finish(&marker);
    assert_eq!(response["status"], "error");
    assert_eq!(store.tip().unwrap().0, Version::new(0).unwrap());
}

#[test]
fn committed_single_retry_does_not_read_collected_snapshots() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let limits = Limits { retained_epochs: 1, ..Limits::default() };
    let mut store = Store::create(&root, limits).unwrap();
    let (_, first) = proposal(&mut store, "first");
    let receipt = store.commit(first.clone()).unwrap();
    let (_, second) = proposal(&mut store, "second");
    store.commit(second).unwrap();
    store.maintain().unwrap();
    assert!(!root.join("objects").join(first.root.as_str()).exists());
    assert_eq!(store.commit(first).unwrap(), receipt);
}

#[test]
fn committed_batch_retry_does_not_read_collected_snapshots() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let limits = Limits { retained_epochs: 1, ..Limits::default() };
    let mut store = Store::create(&root, limits).unwrap();
    let (_, first) = proposal(&mut store, "first");
    let (_, second) = proposal(&mut store, "second");
    let batch =
        store.prepare_batch(vec![first.request, second.request], Version::new(0).unwrap()).unwrap();
    let receipt = store.commit_batch(batch.clone()).unwrap();
    let (_, next) = proposal(&mut store, "next");
    store.commit(next).unwrap();
    store.maintain().unwrap();
    assert!(!root.join("objects").join(receipt.receipts[0].root.as_str()).exists());
    assert_eq!(store.commit_batch(batch).unwrap(), receipt);
}

#[test]
fn advancing_tip_after_verification_requires_repreparation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let mut store = Store::create(&root, Limits::default()).unwrap();
    let (_, candidate) = proposal(&mut store, "first");
    let (_, independent) = proposal(&mut store, "second");
    let marker = directory.path().join("paused");
    let process = paused(
        &root,
        json!({"op":"commit","candidate":candidate}),
        "after-commit-verification",
        &marker,
    );
    let receipt = store.commit(independent).unwrap();
    let response = process.finish(&marker);
    assert_eq!(response["code"], "tip_changed");
    assert_eq!(store.tip().unwrap().0, receipt.version);
    assert_eq!(store.result(candidate.request).unwrap(), None);
}
